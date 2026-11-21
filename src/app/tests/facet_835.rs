//! §1.835 contracts: stage 1 regressions and stage 3 listing-proof ownership.
#![cfg(windows)]

use super::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use crate::ime_focus::typing_probe as probe;
use crate::settings::{
    FacetDurationPreset, FacetSizePreset, GridViewMode, SelectionInfoDisplayMode,
    ToolbarFacetFilterItem,
};
use std::sync::Mutex;
use std::time::{Duration, Instant, SystemTime};

mod acquisition;
mod proof;
mod ranges;
mod refresh;
mod visible_requirements;

const DEADLINE: Duration = Duration::from_secs(20);

// The typing harness's Locator / assert_field require probe::fields(), which
// only observes ime_focus helpers. DragValue's internal TextEdit bypasses those
// helpers. Reuse the real-backend frame pattern, not its field adapter or a copy
// of App::update phases. Discover widget identities from their actual AccessKit
// output, then read their actual responses inside on_end_pass (out-of-pass
// Context::read_response can return the preceding frame's response).
type Responses = Arc<Mutex<(Vec<egui::Id>, HashMap<egui::Id, DrawnResponse>)>>;

#[derive(Clone, Copy)]
struct DrawnResponse {
    rect: egui::Rect,
    enabled: bool,
    focused: bool,
}

struct Harness {
    env: AppTestEnv,
    backend: eframe::HeadlessEpiIntegration,
    ctx: egui::Context,
    nodes: Vec<(egui::accesskit::NodeId, egui::accesskit::Node)>,
    responses: Responses,
    frame: u64,
    stage: String,
    _observations: probe::Session,
}

fn configure(app: &mut App) {
    app.initialized = true;
    app.indexer_init = IndexerInit::Unavailable;
    app.settings.update_check_enabled = false;
    app.settings.minimize_to_tray_on_close = false;
    app.settings.grid_view_mode = GridViewMode::Thumbnail;
    app.settings.sort_order = crate::settings::SortOrder::FileName;
    app.settings.selection_info_display_mode = SelectionInfoDisplayMode::Hidden;
    app.settings.thumb_show_media_duration = false;
    app.settings.thumb_show_resume_meter = false;
    app.settings.details_show_page_count = false;
    app.settings.details_show_created = false;
    app.settings.details_show_image_dimensions = false;
    app.settings.details_show_video_duration = false;
    app.settings.details_show_video_dimensions = false;
    app.settings.details_show_video_codec = false;
    app.settings.show_toolbar_search_local = false;
    app.settings.show_toolbar_search_container = false;
    app.settings.show_toolbar_search_metadata = false;
    app.settings.show_toolbar_search_everything = false;
    app.settings.show_toolbar_facet_filter = true;
    app.settings.toolbar_facet_filter_items = vec![
        ToolbarFacetFilterItem::Size,
        ToolbarFacetFilterItem::Duration,
        ToolbarFacetFilterItem::AiModel,
        ToolbarFacetFilterItem::AiTool,
    ];
    app.settings.facet_filter = Default::default();
    app.settings.rating_filter = [true; 6];
    app.last_main_focused = true;
}

fn widget_id(id: egui::accesskit::NodeId) -> egui::Id {
    // This is the original high-entropy egui id, not a new hash of the node id.
    unsafe { egui::Id::from_high_entropy_bits(id.0) }
}

impl Harness {
    fn new(env: AppTestEnv) -> Self {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        crate::ime_focus::install_ime_input_policy(&ctx);
        ctx.enable_accesskit();
        let responses: Responses = Default::default();
        let captured = Arc::clone(&responses);
        ctx.on_end_pass(
            "facet_835 actual responses",
            Arc::new(move |ui| {
                let ids = captured.lock().unwrap().0.clone();
                let current = ids
                    .into_iter()
                    .filter_map(|id| {
                        ui.ctx().read_response(id).map(|response| {
                            (
                                id,
                                DrawnResponse {
                                    rect: response.rect,
                                    enabled: response.enabled(),
                                    focused: response.has_focus(),
                                },
                            )
                        })
                    })
                    .collect();
                captured.lock().unwrap().1 = current;
            }),
        );
        Self {
            env,
            backend: eframe::HeadlessEpiIntegration::new(ctx.clone()),
            ctx,
            nodes: Vec::new(),
            responses,
            frame: 0,
            stage: "initial adoption".into(),
            _observations: probe::Session::new(),
        }
    }

    fn tick(&mut self, events: Vec<egui::Event>) {
        self.frame += 1;
        probe::begin_frame();
        let modifiers = events
            .iter()
            .find_map(|event| match event {
                egui::Event::Key { modifiers, .. } => Some(*modifiers),
                _ => None,
            })
            .unwrap_or(egui::Modifiers::NONE);
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1600.0, 900.0),
            )),
            focused: true,
            events: std::iter::once(egui::Event::ModifiersChanged(modifiers))
                .chain(events)
                .collect(),
            ..Default::default()
        };
        raw.viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .focused = Some(true);
        self.responses.lock().unwrap().0 =
            self.nodes.iter().map(|(id, _)| widget_id(*id)).collect();
        let mut output = self.backend.update(&mut self.env.app, raw);
        self.nodes = output
            .platform_output
            .accesskit_update
            .take()
            .expect("real widget tree")
            .nodes;
        output.drop_without_applying_deltas();
    }

    fn diagnostic(&self) -> String {
        format!(
            "stage={} frame={} generation={} visible={:?} meta={:?} rescan={} hydration={} modal={:?}",
            self.stage,
            self.frame,
            self.env.items_generation,
            self.visible_names(),
            self.env.details_image_dims_state,
            self.env.external_rescan_pending.is_some(),
            self.env.sidecar_restore_active(),
            self.env.modal_dialog_block_reason()
        )
    }

    fn idle(&self) -> bool {
        !self.env.direct_navigation_scan_pending()
            && self.env.folder_pane_open_pending.is_none()
            && self.env.external_rescan_pending.is_none()
            && self.env.folder_refresh_pending.is_none()
            && !self.env.sidecar_restore_active()
            && !self.env.main_chrome_pointer_blocked()
            && self.env.details_meta_pending.is_none()
            && !matches!(
                self.env.details_image_dims_state,
                LazyColumnState::NotRequested
                    | LazyColumnState::Loading { .. }
                    | LazyColumnState::Reconciling { .. }
            )
            && self.env.current_folder_watch.as_ref().is_some_and(|watch| {
                watch.registration.is_none()
                    && watch.dirty.is_empty()
                    && watch.debounce_until.is_none()
            })
    }

    fn until(&mut self, mut done: impl FnMut(&Self) -> bool, mut check: impl FnMut(&Self)) {
        let deadline = Instant::now() + DEADLINE;
        loop {
            self.tick(Vec::new());
            check(self);
            if done(self) {
                return;
            }
            assert!(Instant::now() < deadline, "deadline: {}", self.diagnostic());
            // Pacing only; completion is an observed owner/state, never elapsed time.
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn settle(&mut self) {
        self.until(Self::idle, |_| {});
    }

    fn adopt(&mut self, folder: &Path, count: usize) {
        self.until(
            |h| {
                h.env.current_folder.as_deref() == Some(folder)
                    && h.env.items.len() == count
                    && h.idle()
                    && h.env
                        .folder_observation
                        .as_ref()
                        .is_some_and(|observation| observation.rows.known)
            },
            |_| {},
        );
        // One more draw observes responses for widgets first discovered at adoption.
        self.tick(Vec::new());
    }

    fn visible_names(&self) -> Vec<String> {
        self.env
            .visible_indices
            .iter()
            .map(|&idx| self.env.items[idx].name().to_string())
            .collect()
    }

    fn idx(&self, name: &str) -> usize {
        self.env
            .items
            .iter()
            .position(|item| item.name() == name)
            .unwrap_or_else(|| panic!("missing {name}: {}", self.diagnostic()))
    }

    fn response(&self, id: egui::Id) -> DrawnResponse {
        assert!(
            self.nodes.iter().any(|(node, _)| widget_id(*node) == id),
            "widget disappeared: {}",
            self.diagnostic()
        );
        self.responses
            .lock()
            .unwrap()
            .1
            .get(&id)
            .cloned()
            .unwrap_or_else(|| panic!("no current response for {id:?}: {}", self.diagnostic()))
    }

    fn labelled(&self, label: &str) -> egui::Id {
        let ids: Vec<_> = self
            .nodes
            .iter()
            .filter(|(_, node)| node.label() == Some(label))
            .map(|(id, _)| widget_id(*id))
            .collect();
        assert_eq!(
            ids.len(),
            1,
            "ambiguous/missing label {label:?}: {ids:?}; {}",
            self.diagnostic()
        );
        ids[0]
    }

    fn click(&mut self, id: egui::Id) {
        let pos = self.response(id).rect.center();
        self.tick(vec![egui::Event::PointerMoved(pos)]);
        self.tick(vec![pointer(pos, true)]);
        self.tick(vec![pointer(pos, false)]);
        self.tick(Vec::new());
    }

    fn open_menu(&mut self, base: &str) {
        let ids: Vec<_> = self
            .nodes
            .iter()
            .filter(|(_, node)| {
                node.role() == egui::Role::Button
                    && node.label().is_some_and(|label| {
                        label == base
                            || label.strip_prefix(base).is_some_and(|suffix| {
                                suffix.starts_with(" (") && suffix.ends_with(')')
                            })
                    })
            })
            .map(|(id, _)| widget_id(*id))
            .collect();
        assert_eq!(
            ids.len(),
            1,
            "ambiguous/missing menu {base}: {}",
            self.diagnostic()
        );
        self.click(ids[0]);
        self.tick(Vec::new());
    }
}

fn pointer(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        pressed,
        button: egui::PointerButton::Primary,
        modifiers: egui::Modifiers::NONE,
    }
}

fn key(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    }
}

fn png_bytes(model: &str) -> Vec<u8> {
    // Same real PNG text-chunk fixture as agent_ipc/item_info_tests::png.
    // The pre-existing app AI facet tests instead seed DetailsLazyMeta directly;
    // that cannot demonstrate acquisition or distinguish unread from read here.
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, 7, 5);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.add_text_chunk("parameters".into(), format!("cat\nNegative prompt: bad\nSteps: 25, Sampler: Euler, CFG scale: 7, Seed: 42, Size: 7x5, Model: {model}")).unwrap();
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&[0; 7 * 5 * 3])
        .unwrap();
    bytes
}

fn image_fixture() -> (Harness, PathBuf) {
    let mut env = setup_app();
    configure(&mut env);
    let folder = env.tmp.path().join("facet-835");
    std::fs::create_dir(&folder).unwrap();
    for (name, model) in [("A.png", "wanted"), ("B.png", "wanted"), ("C.png", "other")] {
        let path = folder.join(name);
        std::fs::write(&path, png_bytes(model)).unwrap();
        if name == "B.png" {
            std::fs::OpenOptions::new()
                .write(true)
                .open(path)
                .unwrap()
                .set_len(2 * 1024 * 1024)
                .unwrap();
        }
    }
    env.load_folder(folder.clone());
    let mut h = Harness::new(env);
    h.adopt(&folder, 3);
    (h, folder)
}

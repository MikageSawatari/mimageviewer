//! §1.838 stage 1: behavioral contracts against the existing production API.
//! A real duration-sort request owns every wait below. Only its receive boundary
//! is held, as in direct_navigation_worker_scan; no pending owner is fabricated.

use super::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use crate::keyboard_input::{KeyboardOwner, ShortcutSurface};
use crate::settings::{
    GridDisplayOrder, GridItemDisplayKind, GridViewMode, SelectionInfoDisplayMode, SortOrder,
};
use std::sync::Mutex;
use std::time::{Duration, Instant};

mod gamepad;
mod lifecycle;

const DEADLINE: Duration = Duration::from_secs(30);

struct HeldScan {
    tx: mpsc::Sender<std::io::Result<ScannedDir>>,
    result: std::io::Result<ScannedDir>,
    cancel: Arc<AtomicBool>,
}

/// The watch re-registers when sort/stack projection changes. Its independent
/// scan must not win this input test's scheduling race and adopt another list
/// while the explicit sort/navigation reply is held. Keep the real worker's
/// receiver, exactly as scroll_661 holds an observation's receive boundary.
/// Restoring that receiver lets the production owner validate/adopt/drop it.
struct HeldObservation {
    cancel: Arc<AtomicBool>,
    worker_rx: mpsc::Receiver<Option<ExternalRescanResult>>,
    _keep_empty_receiver_connected: mpsc::Sender<Option<ExternalRescanResult>>,
}

impl HeldScan {
    fn capture(app: &mut App) -> Self {
        let pending = app
            .folder_pane_open_pending
            .as_mut()
            .expect("real scan request");
        let result = pending
            .rx
            .recv_timeout(DEADLINE)
            .expect("real scan completion");
        let (tx, rx) = mpsc::channel();
        pending.rx = rx;
        Self {
            tx,
            result,
            cancel: pending.cancel.clone(),
        }
    }

    fn release(self) {
        assert!(
            self.tx.send(self.result).is_ok(),
            "request still owns the receiver"
        );
    }

    fn assert_retired(self) {
        assert!(self.cancel.load(Ordering::Relaxed));
        assert!(
            self.tx.send(self.result).is_err(),
            "old result has no consumer"
        );
    }
}

fn request_sort(app: &mut App, order: SortOrder) -> HeldScan {
    assert!(order.is_duration());
    app.settings.sort_order = order;
    // This is the public common path called by apply_sort_control_change.
    app.apply_sort_change_reload();
    let pending = app
        .folder_pane_open_pending
        .as_ref()
        .expect("duration worker scan");
    match &pending.purpose {
        FolderOpenScanPurpose::CurrentViewOrderRefresh {
            order: snapshot, ..
        } => {
            assert!(snapshot.matches(&app.settings));
        }
        _ => panic!("duration sort must use CurrentViewOrderRefresh"),
    }
    HeldScan::capture(app)
}

fn configure(app: &mut App) {
    app.initialized = true;
    app.indexer_init = IndexerInit::Unavailable;
    app.settings.update_check_enabled = false;
    app.settings.minimize_to_tray_on_close = false;
    app.settings.grid_view_mode = GridViewMode::Thumbnail;
    app.settings.grid_cols = 3;
    app.settings.grid_display_order = GridDisplayOrder::from_rows([
        vec![GridItemDisplayKind::Folder, GridItemDisplayKind::Archive],
        vec![GridItemDisplayKind::Image],
        vec![GridItemDisplayKind::VideoAudio],
        vec![],
    ]);
    app.settings.sort_order = SortOrder::FileNameDesc;
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
    app.settings.folder_tree_pane_visible = false;
    app.settings.facet_filter = Default::default();
    app.settings.rating_filter = [true; 6];
    app.active_quick_folder_slot = None;
    app.last_main_focused = true;
    app.folder_pane.set_focus_grid();
}

struct Harness {
    env: AppTestEnv,
    backend: eframe::HeadlessEpiIntegration,
    ctx: egui::Context,
    owner: Arc<Mutex<Option<KeyboardOwner>>>,
    painted_text: Vec<String>,
    hold_observation_delivery: bool,
    observation: Option<HeldObservation>,
}

impl Harness {
    fn images() -> Self {
        let mut env = setup_app();
        configure(&mut env);
        let folder = env.tmp.path().join("sort-hold-838");
        std::fs::create_dir(&folder).unwrap();
        for name in ["B.png", "C.png"] {
            image::RgbImage::new(8, 6).save(folder.join(name)).unwrap();
        }
        // A mixed physical folder permits duration sort in the real controls;
        // an image-only folder may have its page order locked instead.
        write_wav(&folder.join("Z.wav"));
        env.load_folder(folder);
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        crate::ime_focus::install_ime_input_policy(&ctx);
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
        let owner = Arc::new(Mutex::new(None));
        let observed = owner.clone();
        ctx.on_end_pass(
            "sort_hold_838 keyboard owner",
            Arc::new(move |ui| {
                *observed.lock().unwrap() = crate::keyboard_input::cached_keyboard_owner(ui.ctx());
            }),
        );
        let mut h = Self {
            env,
            backend: eframe::HeadlessEpiIntegration::new(ctx.clone()),
            ctx,
            owner,
            painted_text: Vec::new(),
            hold_observation_delivery: false,
            observation: None,
        };
        h.settle();
        assert_eq!(h.names(), ["C.png", "B.png", "Z.wav"]);
        assert!(h.env.listing_sort_capabilities().duration_allowed);
        h.env.move_grid_cursor(Some(0), GridCursorIntent::Automatic);
        h.tick(vec![]);
        h.assert_grid_owner();
        h.hold_observation_delivery = true;
        h
    }

    fn tick(&mut self, events: Vec<egui::Event>) {
        if self.hold_observation_delivery {
            // Start the same passive work that this frame's service will start,
            // so its receive boundary is held before App can poll a fast reply.
            // No request, proof, settings, cancel token or result is fabricated.
            self.env.poll_current_folder_watch(&self.ctx);
            if let Some(pending) = self.env.external_rescan_pending.as_mut()
                && pending.cause.is_passive_observation()
                && self
                    .observation
                    .as_ref()
                    .is_none_or(|held| !Arc::ptr_eq(&held.cancel, &pending.cancel))
            {
                let (tx, rx) = mpsc::channel();
                self.observation = Some(HeldObservation {
                    cancel: pending.cancel.clone(),
                    worker_rx: std::mem::replace(&mut pending.rx, rx),
                    _keep_empty_receiver_connected: tx,
                });
            }
        }
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
        let output = self.backend.update(&mut self.env.app, raw);
        self.painted_text.clear();
        for clipped in &output.shapes {
            collect_text(&clipped.shape, &mut self.painted_text);
        }
        output.drop_without_applying_deltas();
    }

    fn until(&mut self, done: impl Fn(&Self) -> bool) {
        let deadline = Instant::now() + DEADLINE;
        loop {
            self.tick(vec![]);
            if done(self) {
                return;
            }
            assert!(
                Instant::now() < deadline,
                "frame wait: {}",
                self.diagnostic()
            );
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    fn settle(&mut self) {
        // First let the explicit owner adopt, with passive replies still held.
        // Releasing them sooner would reproduce the fixture's competing adoption.
        self.until(|h| h.env.folder_pane_open_pending.is_none() && !h.env.sidecar_restore_active());
        if let Some(held) = self.observation.take()
            && let Some(pending) = self.env.external_rescan_pending.as_mut()
            && Arc::ptr_eq(&held.cancel, &pending.cancel)
        {
            pending.rx = held.worker_rx;
        }
        let hold_observation_delivery =
            std::mem::replace(&mut self.hold_observation_delivery, false);
        self.until(|h| {
            h.env.items.len() == 3
                && !h.env.sidecar_restore_active()
                && h.env.folder_pane_open_pending.is_none()
                && h.env.folder_refresh_pending.is_none()
                && h.env.external_rescan_pending.is_none()
                && h.env.details_meta_pending.is_none()
                && !h.env.main_chrome_pointer_blocked()
                && h.env.current_folder_watch.as_ref().is_some_and(|watch| {
                    watch.registration.is_none()
                        && watch.dirty.is_empty()
                        && watch.debounce_until.is_none()
                })
                && matches!(*h.owner.lock().unwrap(),
                    Some(KeyboardOwner::ApplicationShortcut { scope })
                        if scope.surface == ShortcutSurface::Main)
        });
        self.tick(vec![]);
        self.hold_observation_delivery = hold_observation_delivery;
    }

    fn names(&self) -> Vec<String> {
        self.env
            .items
            .iter()
            .map(|item| item.name().into_owned())
            .collect()
    }

    fn diagnostic(&self) -> String {
        format!(
            "rows={:?} selected={:?} generation={} owner={:?} focus={:?} tree={} popup={} restore={} block={:?} sort_scan={} observation={} history_wait={}",
            self.names(),
            self.env.selected,
            self.env.items_generation,
            *self.owner.lock().unwrap(),
            self.ctx.memory(|m| m.focused()),
            self.env.folder_pane_blocks_grid_keyboard(),
            self.ctx.any_popup_open(),
            self.env.sidecar_restore_active(),
            self.env.grid_item_input_block(),
            self.env.folder_pane_open_pending.is_some(),
            self.env.external_rescan_pending.is_some(),
            self.env.location_navigation_pending(),
        )
    }

    fn assert_grid_owner(&self) {
        // Inventory all competing owners, so a vacuous no-op caused by focus,
        // a modal, a popup or an empty list cannot satisfy a hold assertion.
        assert_eq!(self.env.items.len(), 3, "{}", self.diagnostic());
        assert_eq!(self.env.visible_indices.len(), 3, "{}", self.diagnostic());
        assert!(self.env.selected.is_some());
        assert!(self.env.fullscreen_idx.is_none(), "{}", self.diagnostic());
        assert!(
            !self.env.folder_pane_blocks_grid_keyboard(),
            "{}",
            self.diagnostic()
        );
        assert!(!self.env.sidecar_restore_active(), "{}", self.diagnostic());
        assert!(!self.ctx.any_popup_open(), "{}", self.diagnostic());
        let owner = *self.owner.lock().unwrap();
        assert!(
            matches!(owner,
            Some(KeyboardOwner::ApplicationShortcut { scope })
                if scope.surface == ShortcutSurface::Main && scope.viewport == egui::ViewportId::ROOT),
            "{}",
            self.diagnostic()
        );
    }

    fn press(&mut self, key: egui::Key, modifiers: egui::Modifiers) {
        self.tick(vec![key_event(key, true, modifiers)]);
        self.tick(vec![key_event(key, false, modifiers)]);
    }

    fn cell_center(&self, idx: usize) -> egui::Pos2 {
        self.ctx
            .data(|data| {
                data.get_temp::<egui::Response>(egui::Id::unique((
                    "grid_cell_input_response",
                    self.env.grid_scroll_context_id(),
                    idx,
                )))
            })
            .expect("actual painted grid cell")
            .rect
            .center()
    }

    fn click(&mut self, pos: egui::Pos2) {
        self.tick(vec![egui::Event::PointerMoved(pos)]);
        for pressed in [true, false] {
            self.tick(vec![egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Primary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            }]);
        }
        self.tick(vec![]);
    }
}

fn collect_text(shape: &egui::epaint::Shape, text: &mut Vec<String>) {
    match shape {
        egui::epaint::Shape::Text(shape) => text.push(shape.galley.job.text.clone()),
        egui::epaint::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_text(shape, text);
            }
        }
        _ => {}
    }
}

fn key_event(key: egui::Key, pressed: bool, modifiers: egui::Modifiers) -> egui::Event {
    egui::Event::Key {
        key,
        physical_key: Some(key),
        pressed,
        repeat: false,
        modifiers,
    }
}

fn write_wav(path: &Path) {
    let mut wav = Vec::new();
    wav.extend_from_slice(b"RIFF");
    wav.extend_from_slice(&44_u32.to_le_bytes());
    wav.extend_from_slice(b"WAVEfmt ");
    wav.extend_from_slice(&16_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&8000_u32.to_le_bytes());
    wav.extend_from_slice(&8000_u32.to_le_bytes());
    wav.extend_from_slice(&1_u16.to_le_bytes());
    wav.extend_from_slice(&8_u16.to_le_bytes());
    wav.extend_from_slice(b"data");
    wav.extend_from_slice(&8_u32.to_le_bytes());
    wav.extend_from_slice(&[128; 8]);
    std::fs::write(path, wav).unwrap();
}

#[test]
fn sort_hold_838_arrow_keys_do_not_move_old_selection() {
    let mut h = Harness::images();
    // Positive control: the same key really operates the visible list.
    h.press(egui::Key::ArrowRight, egui::Modifiers::NONE);
    assert_eq!(h.env.selected, Some(1));
    h.press(egui::Key::ArrowLeft, egui::Modifiers::NONE);
    assert_eq!(h.env.selected, Some(0));
    let held = request_sort(&mut h.env, SortOrder::DurationAsc);
    let generation = h.env.items_generation;
    h.assert_grid_owner();
    h.press(egui::Key::ArrowRight, egui::Modifiers::NONE);
    h.assert_grid_owner();
    assert!(Arc::ptr_eq(
        &h.env
            .folder_pane_open_pending
            .as_ref()
            .expect("same sort wait")
            .cancel,
        &held.cancel,
    ));
    assert_eq!(
        h.env.selected,
        Some(0),
        "old order must not receive the key"
    );
    assert_eq!(h.env.items_generation, generation);
}

#[test]
fn sort_hold_838_enter_does_not_open_old_item() {
    let mut h = Harness::images();
    let _held = request_sort(&mut h.env, SortOrder::DurationAsc);
    let generation = h.env.items_generation;
    h.assert_grid_owner();
    // Inspect the actual open in its key-down frame, before a release frame
    // can enter the fullscreen lifecycle and obscure the input contract.
    h.tick(vec![key_event(
        egui::Key::Enter,
        true,
        egui::Modifiers::NONE,
    )]);
    assert!(h.env.fullscreen_idx.is_none(), "old image must not open");
    assert!(!h.env.projected_viewer_session_open());
    assert_eq!(h.env.items_generation, generation);
    assert!(h.env.folder_pane_open_pending.is_some());
}

#[test]
fn sort_hold_838_click_does_not_select_old_item() {
    let mut h = Harness::images();
    let second = h.cell_center(1);
    h.click(second);
    assert_eq!(h.env.selected, Some(1), "positive control: real cell click");
    h.click(h.cell_center(0));
    assert_eq!(h.env.selected, Some(0));
    let _held = request_sort(&mut h.env, SortOrder::DurationAsc);
    h.assert_grid_owner();
    let generation = h.env.items_generation;
    h.click(second);
    assert_eq!(h.env.selected, Some(0));
    assert!(h.env.checked.is_empty());
    assert!(h.env.fullscreen_idx.is_none());
    assert_eq!(h.env.items_generation, generation);
}

#[test]
fn sort_hold_838_history_back_replaces_sort_wait() {
    let mut h = Harness::images();
    let back = h.env.tmp.path().join("back");
    std::fs::create_dir(&back).unwrap();
    h.env.folder_nav_back_stack = vec![FolderNavHistoryTarget::Path(back.clone()).into()];
    assert!(
        h.env
            .keymap
            .effective_chords(crate::keymap::KeyAction::GridHistoryBack)
            .contains(&crate::keymap::Chord::alt(crate::keymap::KeyName::Left))
    );
    let held = request_sort(&mut h.env, SortOrder::DurationAsc);
    h.press(egui::Key::ArrowLeft, egui::Modifiers::ALT);
    held.assert_retired();
    assert!(
        h.env.location_navigation_pending() || h.env.current_folder.as_ref() == Some(&back),
        "Back was admitted: {}",
        h.diagnostic()
    );
    h.until(|h| h.env.current_folder.as_ref() == Some(&back) && !h.env.sidecar_restore_active());
    assert!(h.env.grid_item_input_allowed());
}

#[test]
fn sort_hold_838_escape_keeps_stack_mode_and_sort_request() {
    let mut h = Harness::images();
    h.env.stack_mode_requested = true;
    let held = request_sort(&mut h.env, SortOrder::DurationAsc);
    h.press(egui::Key::Escape, egui::Modifiers::NONE);
    assert!(h.env.stack_mode_requested);
    assert!(!held.cancel.load(Ordering::Relaxed));
    assert!(h.env.folder_pane_open_pending.is_some());
}

#[test]
fn sort_hold_838_assigned_stack_toggle_keeps_stack_mode_and_sort_request() {
    let mut h = Harness::images();
    h.env.keymap = crate::keymap::Keymap::from_ini_str("[Grid]\nGridToggleStackMode = F16\n");
    h.env.stack_mode_requested = true;
    let held = request_sort(&mut h.env, SortOrder::DurationAsc);
    h.press(egui::Key::F16, egui::Modifiers::NONE);
    assert!(h.env.stack_mode_requested);
    assert!(!held.cancel.load(Ordering::Relaxed));
    assert!(h.env.folder_pane_open_pending.is_some());
}

#[test]
fn sort_hold_838_adoption_holds_input_through_real_sidecar_restore() {
    let mut h = Harness::images();
    // setup_app deliberately disables imports. Opt in to the real Checking
    // phase whose existing relay can hold restoration across input frames.
    h.env.settings.sidecar_backup_enabled = true;
    let held = request_sort(&mut h.env, SortOrder::DurationAsc);
    let generation = h.env.items_generation;
    held.release();
    // Use the production adoption tail, then attach the existing checking relay
    // before the next real frame can poll hydration. No race with a fast worker.
    h.env
        .post_render_main_navigation(&h.ctx, update_phases::MainFrameNavigation::default());
    assert!(h.env.folder_pane_open_pending.is_none());
    assert!(h.env.items_generation > generation);
    assert!(
        h.env.sidecar_restore_active(),
        "adoption and restore have no gap"
    );
    let relay = sidecar_restore::SidecarCheckingRelay::new();
    h.env.attach_sidecar_checking_relay_for_test(relay.clone());
    h.until(|_| relay.captured());
    let selected = h.env.selected;
    h.press(egui::Key::ArrowLeft, egui::Modifiers::NONE);
    assert_eq!(h.env.selected, selected);
    h.press(egui::Key::Enter, egui::Modifiers::NONE);
    assert_eq!(h.env.selected, selected);
    h.click(h.cell_center(if selected == Some(0) { 1 } else { 0 }));
    assert!(h.env.sidecar_restore_active());
    assert_eq!(h.env.selected, selected);
    assert!(h.env.fullscreen_idx.is_none());
    relay.release();
    h.settle();
    assert!(h.env.grid_item_input_allowed());
    assert_eq!(h.names(), ["B.png", "C.png", "Z.wav"]);
    // Reload preserves the cursor by path, not by its old index.
    assert_eq!(h.env.selected, Some(1));
    h.assert_grid_owner();
    h.press(egui::Key::ArrowLeft, egui::Modifiers::NONE);
    assert_eq!(h.env.selected, Some(0));
    assert_eq!(h.env.items[h.env.selected.unwrap()].name(), "B.png");
}

#[test]
fn sort_hold_838_wait_overlay_has_visual_grace_but_input_has_none() {
    let mut h = Harness::images();
    let _held = request_sort(&mut h.env, SortOrder::DurationAsc);
    // Same clock boundary as direct_navigation_worker_wait_overlay_grace_and_escape_keep_request.
    h.env.folder_pane_open_pending.as_mut().unwrap().started_at = Instant::now();
    h.tick(vec![]);
    assert!(!h.painted_text.iter().any(|text| text == "並べ替え中…"));
    let allowed_before_grace = h.env.grid_item_input_allowed();
    h.env.folder_pane_open_pending.as_mut().unwrap().started_at =
        Instant::now() - Duration::from_millis(501);
    h.tick(vec![]);
    // An Area's first pass may be a sizing pass; keep the same held request.
    h.tick(vec![]);
    assert!(
        h.painted_text.iter().any(|text| text == "並べ替え中…"),
        "{:?}",
        h.painted_text
    );
    assert!(
        !allowed_before_grace,
        "visual grace must not delay input ownership"
    );
    assert!(!h.env.grid_item_input_allowed());
}

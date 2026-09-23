//! Headless ROOT and child-viewport scenario driver. No native windows are launched.

#![cfg(all(test, windows))]

use super::paint_record_test_support::{PaintRecord, paint_records, with_capture};
use super::*;
use std::cell::RefCell;
use std::collections::{HashMap, HashSet};
use std::rc::Rc;
use std::sync::Arc;

const ROOT_SIZE: egui::Vec2 = egui::vec2(1200.0, 800.0);
const CHILD_SIZE: egui::Vec2 = egui::vec2(960.0, 720.0);

struct ScenarioFrame {
    number: usize,
    records: HashMap<egui::ViewportId, Vec<PaintRecord>>,
    errors: HashMap<egui::ViewportId, Vec<String>>,
}

fn collect_visible_errors(shape: &egui::Shape, errors: &mut Vec<String>) {
    match shape {
        egui::Shape::Vec(shapes) => {
            for shape in shapes {
                collect_visible_errors(shape, errors);
            }
        }
        egui::Shape::Text(text) => {
            let message = text.galley.text();
            if ["失敗", "エラー", "開けません", "読み込めません"]
                .iter()
                .any(|word| message.contains(word))
            {
                errors.push(message.to_owned());
            }
        }
        _ => {}
    }
}

fn error_messages(output: &egui::FullOutput) -> Vec<String> {
    let mut errors = Vec::new();
    for clipped in &output.shapes {
        collect_visible_errors(&clipped.shape, &mut errors);
    }
    errors
}

struct ScenarioDriver {
    ctx: egui::Context,
    frame: eframe::Frame,
    child_outputs: Rc<RefCell<Vec<(egui::ViewportId, egui::FullOutput)>>>,
    focused: Rc<RefCell<Option<egui::ViewportId>>>,
    known: Rc<RefCell<HashSet<egui::ViewportId>>>,
    callbacks: HashMap<egui::ViewportId, Arc<egui::DeferredViewportUiCallback>>,
    number: usize,
}

impl ScenarioDriver {
    fn new() -> Self {
        let ctx = egui::Context::default();
        ctx.set_embed_viewports(false);
        crate::ui_fonts::configure_fonts(&ctx);
        let child_outputs = Rc::new(RefCell::new(Vec::new()));
        let focused = Rc::new(RefCell::new(None));
        let known = Rc::new(RefCell::new(HashSet::from([egui::ViewportId::ROOT])));
        let capture = Rc::clone(&child_outputs);
        let child_focus = Rc::clone(&focused);
        let child_known = Rc::clone(&known);
        egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
            let id = viewport.ids.this;
            child_known.borrow_mut().insert(id);
            let input = Self::input(id, *child_focus.borrow(), CHILD_SIZE, &child_known.borrow());
            let output = ctx.run(input, |ctx| (viewport.viewport_ui_cb)(ctx));
            capture.borrow_mut().push((id, output));
        });
        Self {
            ctx,
            frame: eframe::Frame::_new_kittest(),
            child_outputs,
            focused,
            known,
            callbacks: HashMap::new(),
            number: 0,
        }
    }

    fn input(
        id: egui::ViewportId,
        focused: Option<egui::ViewportId>,
        size: egui::Vec2,
        known: &HashSet<egui::ViewportId>,
    ) -> egui::RawInput {
        let rect = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        let focused_here = focused.map_or(id == egui::ViewportId::ROOT, |current| id == current);
        let mut input = egui::RawInput {
            viewport_id: id,
            focused: focused_here,
            screen_rect: Some(rect),
            ..Default::default()
        };
        for viewport in known {
            let viewport_rect = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                if *viewport == egui::ViewportId::ROOT {
                    ROOT_SIZE
                } else {
                    CHILD_SIZE
                },
            );
            let info = input.viewports.entry(*viewport).or_default();
            info.focused = Some(
                focused.map_or(*viewport == egui::ViewportId::ROOT, |current| {
                    *viewport == current
                }),
            );
            info.native_pixels_per_point = Some(1.0);
            info.inner_rect = Some(viewport_rect);
            info.outer_rect = Some(viewport_rect);
            if *viewport != egui::ViewportId::ROOT {
                info.parent = Some(egui::ViewportId::ROOT);
            }
        }
        input
    }

    fn root(&mut self, app: &mut App) -> ScenarioFrame {
        self.number += 1;
        self.child_outputs.borrow_mut().clear();
        let input = Self::input(
            egui::ViewportId::ROOT,
            *self.focused.borrow(),
            ROOT_SIZE,
            &self.known.borrow(),
        );
        let output = with_capture(|| {
            self.ctx.run(input, |ctx| {
                <App as eframe::App>::update(app, ctx, &mut self.frame);
            })
        });
        let mut records = HashMap::new();
        let mut errors = HashMap::new();
        records.insert(
            egui::ViewportId::ROOT,
            paint_records(egui::ViewportId::ROOT, &output),
        );
        errors.insert(egui::ViewportId::ROOT, error_messages(&output));
        for (id, output) in self.child_outputs.borrow_mut().drain(..) {
            records.insert(id, paint_records(id, &output));
            errors.insert(id, error_messages(&output));
        }
        let registered = output
            .viewport_output
            .keys()
            .copied()
            .collect::<HashSet<_>>();
        self.callbacks.retain(|id, _| registered.contains(id));
        let mut known = registered;
        known.insert(egui::ViewportId::ROOT);
        *self.known.borrow_mut() = known;
        for (id, viewport) in output.viewport_output {
            if let Some(callback) = viewport.viewport_ui_cb {
                self.callbacks.insert(id, callback);
            }
        }
        ScenarioFrame {
            number: self.number,
            records,
            errors,
        }
    }

    /// Deferred children repaint independently after ROOT registration. Their events
    /// become visible to the App on the next ROOT update.
    fn deferred(&mut self, id: egui::ViewportId) -> ScenarioFrame {
        self.number += 1;
        let callback = Arc::clone(self.callbacks.get(&id).expect("ROOT registered child"));
        let input = Self::input(id, *self.focused.borrow(), CHILD_SIZE, &self.known.borrow());
        let output = with_capture(|| self.ctx.run(input, |ctx| callback(ctx)));
        let mut records = HashMap::new();
        let mut errors = HashMap::new();
        records.insert(id, paint_records(id, &output));
        errors.insert(id, error_messages(&output));
        ScenarioFrame {
            number: self.number,
            records,
            errors,
        }
    }

    fn focus(&mut self, id: egui::ViewportId) {
        *self.focused.borrow_mut() = Some(id);
    }
}

fn records_for(frame: &ScenarioFrame, id: egui::ViewportId) -> &[PaintRecord] {
    frame.records.get(&id).map(Vec::as_slice).unwrap_or(&[])
}

fn nearly(a: f32, b: f32, px_tolerance: f32) -> bool {
    (a - b).abs() <= px_tolerance
}

fn visible_clip(record: &PaintRecord) -> egui::Rect {
    let bounds = record
        .vertices
        .iter()
        .fold(egui::Rect::NOTHING, |mut bounds, vertex| {
            bounds.extend_with(vertex.pos);
            bounds
        });
    record.clip.intersect(bounds)
}

fn same_paint(before: &PaintRecord, after: &PaintRecord, ppp: f32) -> bool {
    let position_tolerance = 1.0 / ppp;
    let _texture_identity_can_change = before.texture != after.texture;
    // Active can submit a full-viewport clip while frozen pages submit their
    // exact image clip. Compare the effective visible clip; retain raw clips
    // in PaintRecord for diagnostics and to catch any actual crop difference.
    let before_clip = visible_clip(before);
    let after_clip = visible_clip(after);
    before.viewport == after.viewport
        && before.provenance == after.provenance
        && before.vertices.len() == after.vertices.len()
        && before.vertices.iter().zip(&after.vertices).all(|(a, b)| {
            nearly(a.pos.x, b.pos.x, position_tolerance)
                && nearly(a.pos.y, b.pos.y, position_tolerance)
                && nearly(a.uv.x, b.uv.x, 1e-4)
                && nearly(a.uv.y, b.uv.y, 1e-4)
        })
        && nearly(before_clip.min.x, after_clip.min.x, position_tolerance)
        && nearly(before_clip.min.y, after_clip.min.y, position_tolerance)
        && nearly(before_clip.max.x, after_clip.max.x, position_tolerance)
        && nearly(before_clip.max.y, after_clip.max.y, position_tolerance)
}

fn assert_i2_stable_paint(
    scenario: &str,
    viewport: egui::ViewportId,
    before: &ScenarioFrame,
    after: &ScenarioFrame,
    ppp: f32,
) {
    let before_paint = records_for(before, viewport);
    let after_paint = records_for(after, viewport);
    assert!(
        !before_paint.is_empty()
            && before_paint.len() == after_paint.len()
            && before_paint
                .iter()
                .zip(after_paint)
                .all(|(a, b)| same_paint(a, b, ppp)),
        "I2 scenario={scenario} frame={} -> {} before={before_paint:#?} after={after_paint:#?}",
        before.number,
        after.number,
    );
}

fn assert_i1_open_or_paint(
    scenario: &str,
    window_alive: bool,
    window_id: u64,
    viewport: egui::ViewportId,
    before: &ScenarioFrame,
    after: &ScenarioFrame,
) -> bool {
    let before_paint = records_for(before, viewport);
    let after_paint = records_for(after, viewport);
    let explicit_error = after
        .errors
        .get(&viewport)
        .is_some_and(|messages| !messages.is_empty());
    assert!(
        window_alive || explicit_error,
        "I1 scenario={scenario} frame={} -> {} window={window_id} vanished without an explicit error before={before_paint:#?} after={after_paint:#?} errors={:#?}",
        before.number,
        after.number,
        after.errors,
    );
    !after_paint.is_empty() || explicit_error
}

#[test]
#[should_panic(expected = "vanished without an explicit error")]
fn multiwindow_scenario_i1_ignores_root_and_sibling_errors() {
    let target = egui::ViewportId::from_hash_of("target");
    let sibling = egui::ViewportId::from_hash_of("sibling");
    let before = ScenarioFrame {
        number: 0,
        records: HashMap::new(),
        errors: HashMap::new(),
    };
    let after = ScenarioFrame {
        number: 1,
        records: HashMap::new(),
        errors: HashMap::from([
            (egui::ViewportId::ROOT, vec!["root error".to_owned()]),
            (sibling, vec!["sibling error".to_owned()]),
            (target, Vec::new()),
        ]),
    };
    assert_i1_open_or_paint("I1 sibling error", false, 7, target, &before, &after);
}

#[derive(Debug, PartialEq)]
struct StableContent {
    viewport_size: egui::Vec2,
    pixels_per_point: f32,
    items_generation: u64,
    item: String,
    page: usize,
    spread_mode: crate::settings::SpreadMode,
    singleton_preference: crate::settings::SingletonSpreadPlacementPreference,
    singleton_enabled: bool,
}

fn stable_content(app: &mut App, window_id: u64) -> Option<StableContent> {
    app.with_window_viewer_context(window_id, |owner| {
        let page = owner.fullscreen_idx?;
        let item = owner.items.get(page)?.perf_key();
        if owner.fs_pending.contains_key(&page) || owner.sidecar_restore.is_some() {
            return None;
        }
        Some(StableContent {
            viewport_size: CHILD_SIZE,
            pixels_per_point: 1.0,
            items_generation: owner.items_generation,
            item,
            page,
            spread_mode: owner.spread_mode,
            singleton_preference: owner.singleton_spread_placement_preference,
            singleton_enabled: owner.settings.singleton_spread_placement_enabled,
        })
    })
    .expect("window retains its viewer context")
}

fn save_portrait(path: &std::path::Path, color: [u8; 3]) {
    let image = image::RgbImage::from_pixel(600, 900, image::Rgb(color));
    image.save(path).expect("real scenario image");
}

fn configure_active_still(
    app: &mut App,
    ctx: &egui::Context,
    path: std::path::PathBuf,
    window_id: u64,
    color: egui::Color32,
) {
    app.current_folder = path.parent().map(std::path::Path::to_path_buf);
    app.items = vec![GridItem::Image(path)];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.image_metas = vec![None];
    app.visible_indices = vec![0];
    app.details_order = vec![0];
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    app.detached_viewer_independent_active = true;
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    app.singleton_spread_placement_preference =
        crate::settings::SingletonSpreadPlacementPreference::Place;
    app.record_page_dims_for_spread(0, (600, 900));
    let pixels = egui::ColorImage::filled([600, 900], color);
    let texture = ctx.load_texture(
        format!("scenario_window_{window_id}"),
        pixels.clone(),
        egui::TextureOptions::LINEAR,
    );
    app.fs_cache.insert(
        0,
        FsCacheEntry::Static {
            tex: texture,
            pixels: Arc::new(pixels),
            source_dims: Some([600, 900]),
            load_seq: window_id,
            animation: crate::fs_animation::StaticAnimationState::Still,
        },
    );
}

#[test]
fn multiwindow_scenario_b_activation_keeps_singleton_paint() {
    std::thread::spawn(|| {
        let mut app = setup_app_for_test();
        let mut driver = ScenarioDriver::new();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
        app.startup_done = true;
        app.startup_init = None;
        app.settings.detached_viewer_open_images_in_window = true;
        app.settings.singleton_spread_placement_enabled = true;
        app.settings.detached_viewer_window_placement =
            Some(crate::settings::DetachedViewerWindowPlacement {
                x: 80.0,
                y: 80.0,
                w: CHILD_SIZE.x,
                h: CHILD_SIZE.y,
                maximized: false,
            });
        let folder = app.tmp.path().join("scenario-b");
        std::fs::create_dir(&folder).unwrap();
        let first_path = folder.join("portrait-a.png");
        let second_path = folder.join("portrait-b.png");
        save_portrait(&first_path, [16, 88, 160]);
        save_portrait(&second_path, [200, 48, 32]);

        // Park a second real-image context, then draw the endpoint singleton in
        // the first context before requesting production activation of the second.
        let second_ctx = driver.ctx.clone();
        app.build_active_context_for_test(Some(200), DetachedSource::Book, |active| {
            configure_active_still(
                active,
                &second_ctx,
                second_path,
                200,
                egui::Color32::LIGHT_RED,
            );
        });
        assert!(app.pause_current_active_viewer_context(&driver.ctx));
        let first_ctx = driver.ctx.clone();
        app.build_active_context_for_test(Some(100), DetachedSource::Book, |active| {
            configure_active_still(
                active,
                &first_ctx,
                first_path,
                100,
                egui::Color32::LIGHT_BLUE,
            );
        });
        app.fs_viewport_shown = true;
        app.fs_viewport_presentation = Some(ViewerPresentation::DetachedWindow);
        app.set_detached_window_live_hwnds_for_test([0x1000, 0x2000]);
        app.detached_window_hwnd_set(100, 0x1000);
        app.detached_window_hwnd_set(200, 0x2000);
        let first_viewport = App::detached_image_window_viewport_id(100);
        driver.focus(first_viewport);

        let mut before = driver.root(&mut app);
        for _ in 0..12 {
            if !records_for(&before, first_viewport).is_empty() {
                break;
            }
            before = driver.root(&mut app);
        }
        assert!(
            !records_for(&before, first_viewport).is_empty(),
            "S-B active singleton must reach a real image mesh: frame={} records={:#?}",
            before.number,
            before.records
        );
        assert_eq!(app.active_detached_window_id(), Some(100));
        let before_state = stable_content(&mut app, 100).expect("active paint is stable");

        // Focus metadata follows the request, but activation is committed by
        // the same deferred request owner consumed by the ROOT update.
        app.queue_deferred_detached_window_activation(200, "scenario_test_focus");
        driver.focus(App::detached_image_window_viewport_id(200));
        let after_root = driver.root(&mut app);
        assert_eq!(app.active_detached_window_id(), Some(200));
        assert_eq!(
            app.detached_window_state(100),
            Some(DetachedWindowState::Parked)
        );
        let after = driver.deferred(first_viewport);
        let after_state = stable_content(&mut app, 100).expect("parked paint is stable");
        assert_eq!(before_state, after_state, "S-B changes only activation");
        assert_i2_stable_paint(
            "S-B portrait endpoint",
            first_viewport,
            &before,
            &after,
            1.0,
        );
        // The next ROOT pass drains the parked child event.
        let _ = driver.root(&mut app);
        assert!(after_root.number < after.number);
    })
    .join()
    .expect("scenario thread");
}

#[derive(Clone, Copy)]
enum ScenarioABook {
    Folder,
    Zip,
}

fn wait_until(label: &str, timeout: std::time::Duration, mut ready: impl FnMut() -> bool) {
    let deadline = std::time::Instant::now() + timeout;
    while !ready() {
        assert!(
            std::time::Instant::now() < deadline,
            "wait_until timed out: {label}"
        );
        std::thread::yield_now();
    }
}

fn run_scenario_a(book: ScenarioABook) {
    let label = match book {
        ScenarioABook::Folder => "S-A folder",
        ScenarioABook::Zip => "S-A ZIP",
    };
    let mut app = setup_app_for_test();
    let mut driver = ScenarioDriver::new();
    crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
    app.startup_done = true;
    app.startup_init = None;
    app.settings.sidecar_backup_enabled = true;
    app.settings.tag_sidecar_backup_enabled = false;
    app.settings.detached_viewer_open_images_in_window = true;
    app.settings.auto_fullscreen_image_folders = true;
    app.settings.auto_fullscreen_zip_pdf = true;

    let folder = app.tmp.path().join(match book {
        ScenarioABook::Folder => "scenario-a-folder",
        ScenarioABook::Zip => "scenario-a-zip",
    });
    std::fs::create_dir(&folder).unwrap();
    let page = folder.join("page.png");
    save_portrait(&page, [32, 120, 72]);
    let source = match book {
        ScenarioABook::Folder => folder.clone(),
        ScenarioABook::Zip => {
            let path = folder.join("book.zip");
            let file = std::fs::File::create(&path).unwrap();
            let mut zip = zip::ZipWriter::new(file);
            let options: zip::write::FileOptions<'_, ()> = zip::write::FileOptions::default()
                .compression_method(zip::CompressionMethod::Stored);
            zip.start_file("page.png", options).unwrap();
            std::io::copy(&mut std::fs::File::open(&page).unwrap(), &mut zip).unwrap();
            zip.finish().unwrap();
            path
        }
    };
    let mut sidecar = crate::sidecar::SidecarFile::new(folder.clone());
    sidecar.set_adjust("page.png", crate::adjustment::AdjustParams::default());
    assert!(sidecar.flush_blocking(), "valid sidecar must reach disk");
    assert!(matches!(
        crate::sidecar::SidecarFile::load_for_import(&folder),
        crate::sidecar::SidecarImportLoad::Loaded(_)
    ));

    app.build_active_context_for_test(Some(101), DetachedSource::Book, |active| {
        active.navigation_scope = ViewerNavigationScope::DetachedPhysical;
        active.viewer_presentation = ViewerPresentation::DetachedWindow;
        active.detached_viewer_independent_active = true;
    });
    app.with_active_viewer_context(|active| {
        active.pending_auto_fs_open = true;
        active.load_folder(source.clone());
    })
    .expect("detached owner mounts for real book open");
    if matches!(book, ScenarioABook::Zip) {
        wait_until(
            "ZIP enumeration",
            std::time::Duration::from_secs(10),
            || {
                app.with_active_viewer_context(|active| active.poll_zip_enumerate())
                    .expect("ZIP owner remains mountable");
                app.sidecar_restore.is_some()
            },
        );
    }
    assert!(
        app.sidecar_restore.is_some(),
        "{label}: real open must start restore"
    );
    let relay = sidecar_restore::SidecarCheckingRelay::new();
    app.attach_sidecar_checking_relay_for_test(Rc::clone(&relay));
    wait_until(
        "sidecar checking completion captured",
        std::time::Duration::from_secs(10),
        || {
            app.poll_sidecar_restore(&driver.ctx);
            relay.captured()
        },
    );
    assert!(
        app.sidecar_restore.is_some(),
        "{label}: held completion stays outstanding"
    );
    let window_id = app.active_detached_window_id().expect("opened window id");
    let viewport = App::detached_image_window_viewport_id(window_id);
    driver.focus(viewport);
    let mut before = ScenarioFrame {
        number: 0,
        records: HashMap::new(),
        errors: HashMap::new(),
    };
    let held = driver.root(&mut app);
    assert!(
        !assert_i1_open_or_paint(
            label,
            app.detached_window_state(window_id).is_some(),
            window_id,
            viewport,
            &before,
            &held,
        ),
        "{label}: a held restore cannot paint completed content"
    );
    assert!(
        app.sidecar_restore.is_some(),
        "{label}: ROOT cannot consume a held completion"
    );
    before = held;

    relay.release();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let after = driver.root(&mut app);
        if assert_i1_open_or_paint(
            label,
            app.detached_window_state(window_id).is_some(),
            window_id,
            viewport,
            &before,
            &after,
        ) && app.sidecar_restore.is_none()
            && stable_content(&mut app, window_id).is_some()
        {
            break;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "I1 scenario={label} frame={} -> {} timed out before={:#?} after={:#?}",
            before.number,
            after.number,
            records_for(&before, viewport),
            records_for(&after, viewport)
        );
        before = after;
        std::thread::yield_now();
    }
    assert!(
        app.sidecar_restore.is_none(),
        "{label}: completion delivered once"
    );
}

#[test]
fn multiwindow_scenario_a_folder_sidecar_restore_reaches_paint() {
    std::thread::spawn(|| run_scenario_a(ScenarioABook::Folder))
        .join()
        .expect("scenario thread");
}

#[test]
fn multiwindow_scenario_a_zip_sidecar_restore_reaches_paint() {
    std::thread::spawn(|| run_scenario_a(ScenarioABook::Zip))
        .join()
        .expect("scenario thread");
}

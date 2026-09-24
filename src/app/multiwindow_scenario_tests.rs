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
        self.root_with_events(app, Vec::new())
    }

    fn root_with_events(&mut self, app: &mut App, events: Vec<egui::Event>) -> ScenarioFrame {
        self.number += 1;
        self.child_outputs.borrow_mut().clear();
        let mut input = Self::input(
            egui::ViewportId::ROOT,
            *self.focused.borrow(),
            ROOT_SIZE,
            &self.known.borrow(),
        );
        input.events = events;
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

/// A synthetic search `GridItem::Image` exercises the production detached router, scan,
/// sidecar restore, and lifecycle. Double-click dispatch and search-index integration are T2.
#[test]
fn multiwindow_scenario_search_result_image_sidecar_reaches_paint() {
    std::thread::spawn(|| {
        let mut app = setup_app_for_test();
        let mut driver = ScenarioDriver::new();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
        app.startup_done = true;
        app.startup_init = None;
        app.settings.sidecar_backup_enabled = true;
        app.settings.tag_sidecar_backup_enabled = false;
        app.settings.detached_viewer_open_images_in_window = true;

        let folder = app.tmp.path().join("search-hit-parent");
        std::fs::create_dir(&folder).unwrap();
        let page = folder.join("page.png");
        save_portrait(&page, [32, 120, 72]);
        save_portrait(&folder.join("a-before.png"), [120, 32, 72]);
        let search_page = PathBuf::from(page.to_string_lossy().replace('\\', "/"));
        let mut sidecar = crate::sidecar::SidecarFile::new(folder.clone());
        sidecar.set_adjust("page.png", crate::adjustment::AdjustParams::default());
        assert!(sidecar.flush_blocking());
        assert!(matches!(
            crate::sidecar::SidecarFile::load_for_import(&folder),
            crate::sidecar::SidecarImportLoad::Loaded(_)
        ));

        // Double-click dispatch uses this router before the generic image/fullscreen branch.
        app.current_folder = Some(app.tmp.path().to_path_buf());
        app.open_global_search();
        app.global_search.focus_request = false;
        app.global_search.done = true;
        app.replace_search_view_items(vec![GridItem::Image(search_page.clone())], vec![None]);
        assert!(app.open_grid_container_in_detached_book_context(&driver.ctx, 0));
        let window_id = app.active_detached_window_id().expect("search hit opened a window");
        let viewport = App::detached_image_window_viewport_id(window_id);
        // ROOT retains focus until the newly created child host has received focus.

        let mut scan_outcome = DetachedPhysicalFolderOpenPoll::Waiting;
        wait_until(
            "search hit parent scan",
            std::time::Duration::from_secs(10),
            || {
                scan_outcome = app.with_active_viewer_context(|active| {
                    active.poll_detached_physical_folder_open(&driver.ctx)
                })
                .expect("detached owner remains mountable");
                !matches!(scan_outcome, DetachedPhysicalFolderOpenPoll::Waiting)
            },
        );
        if matches!(scan_outcome, DetachedPhysicalFolderOpenPoll::Failed) {
            // App::update uses this terminal transition for the same failed scan result.
            app.with_active_viewer_context(|active| {
                active.terminate_active_detached_open_before_viewport(
                    "detached_physical_open_failed",
                );
            })
            .expect("detached owner remains mountable");
        }
        assert!(app.sidecar_restore.is_some(), "valid parent sidecar starts a restore");
        let relay = sidecar_restore::SidecarCheckingRelay::new();
        app.attach_sidecar_checking_relay_for_test(Rc::clone(&relay));
        wait_until(
            "search hit sidecar checking completion",
            std::time::Duration::from_secs(10),
            || {
                app.poll_sidecar_restore(&driver.ctx);
                relay.captured()
            },
        );

        let mut before = ScenarioFrame {
            number: 0,
            records: HashMap::new(),
            errors: HashMap::new(),
        };
        let held = driver.root(&mut app);
        assert!(
            !assert_i1_open_or_paint(
                "search result image with sidecar",
                app.detached_window_state(window_id).is_some(),
                window_id,
                viewport,
                &before,
                &held,
            ),
            "held sidecar cannot paint completed content"
        );
        before = held;
        relay.release();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
        loop {
            let after = driver.root(&mut app);
            let painted = assert_i1_open_or_paint(
                "search result image with sidecar",
                app.detached_window_state(window_id).is_some(),
                window_id,
                viewport,
                &before,
                &after,
            );
            if painted && app.sidecar_restore.is_none() && stable_content(&mut app, window_id).is_some() {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "search hit never painted: window={:?} sidecar={} fullscreen={:?} pending={:?} paint={:?} errors={:?}",
                app.detached_window_state(window_id),
                app.sidecar_restore.is_some(),
                app.with_active_viewer_context(|active| active.fullscreen_idx),
                app.with_active_viewer_context(|active| active.fs_pending.len()),
                records_for(&after, viewport),
                after.errors,
            );
            before = after;
            std::thread::yield_now();
        }
        assert!(app.items_are_global_search_view, "main search view is unchanged");
        assert!(app.items.iter().any(|item| matches!(item, GridItem::Image(path) if path == &search_page)));
        assert!(app.with_active_viewer_context(|active| {
            active.fullscreen_idx.and_then(|idx| active.items.get(idx)).is_some_and(
                |item| matches!(item, GridItem::Image(path) if crate::path_key::eq_keep_drive(path, &search_page)),
            )
        }).unwrap_or(false), "detached viewer selected the requested search hit");
    })
    .join()
    .unwrap();
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

/// The collection order is A (folder one/2), B (folder two/1), C (folder
/// one/1). Each physical folder also has unregistered neighbours.
fn collection_order_fixture(
    app: &mut App,
    driver: &mut ScenarioDriver,
    temp: &std::path::Path,
) -> [std::path::PathBuf; 3] {
    use crate::collection_store::{
        CollectionRegistration, CollectionResolvedKind, CollectionStoreRuntime,
    };
    let first_folder = temp.join("collection-order-one");
    let second_folder = temp.join("collection-order-two");
    std::fs::create_dir(&first_folder).unwrap();
    std::fs::create_dir(&second_folder).unwrap();
    let a = first_folder.join("2.png");
    let b = second_folder.join("1.png");
    let c = first_folder.join("1.png");
    for (path, color) in [
        (first_folder.join("0.png"), [10, 20, 30]),
        (c.clone(), [30, 40, 50]),
        (a.clone(), [50, 60, 70]),
        (first_folder.join("3.png"), [70, 80, 90]),
        (second_folder.join("0.png"), [90, 100, 110]),
        (b.clone(), [110, 120, 130]),
        (second_folder.join("2.png"), [130, 140, 150]),
    ] {
        save_portrait(&path, color);
    }
    let runtime =
        CollectionStoreRuntime::start_at(temp.join("collection.db")).expect("collection actor");
    let client = runtime.client();
    app.install_collection_runtime(runtime);
    wait_until(
        "collection actor ready",
        std::time::Duration::from_secs(5),
        || {
            app.poll_collection_ui(&driver.ctx);
            matches!(app.collection_store_client_for_read(), Ok(Some(_)))
        },
    );
    let created = client
        .create_collection("Cross-folder order".into())
        .unwrap()
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap()
        .unwrap();
    let registrations = [&a, &b, &c]
        .map(|path| {
            CollectionRegistration::from_trusted_path(path, CollectionResolvedKind::Image).unwrap()
        })
        .to_vec();
    client
        .add_batch(created.collection_id(), created.revision(), registrations)
        .unwrap()
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap()
        .unwrap();
    app.open_collection_grid_from_navigation(created.collection_id());
    wait_until(
        "collection grid installed",
        std::time::Duration::from_secs(10),
        || {
            app.poll_collection_ui(&driver.ctx);
            app.poll_collection_grid(&driver.ctx);
            app.items.len() == 3
        },
    );
    assert_eq!(
        app.items,
        [&a, &b, &c]
            .map(|path| GridItem::Image(path.clone()))
            .to_vec(),
        "the production collection grid must install registration order"
    );
    [a, b, c]
}

fn collection_order_add_folder(
    app: &mut App,
    driver: &mut ScenarioDriver,
    temp: &std::path::Path,
    name: &str,
    expected_index: usize,
) -> (std::path::PathBuf, std::path::PathBuf) {
    let folder = temp.join(name);
    std::fs::create_dir(&folder).unwrap();
    let page = folder.join("page.png");
    save_portrait(&page, [140, 80, 30]);
    let session = app.top_level_grid_view.collection_session().unwrap();
    let prepared = session.prepared().unwrap();
    let client = app.collection_store_client_for_read().unwrap().unwrap();
    client
        .add_batch(
            prepared.collection_id,
            prepared.collection_revision,
            vec![
                crate::collection_store::CollectionRegistration::from_trusted_path(
                    &folder,
                    crate::collection_store::CollectionResolvedKind::Folder,
                )
                .unwrap(),
            ],
        )
        .unwrap()
        .recv_timeout(std::time::Duration::from_secs(3))
        .unwrap()
        .unwrap();
    wait_until(
        "collection folder entry",
        std::time::Duration::from_secs(10),
        || {
            driver.root(app);
            matches!(app.items.get(expected_index), Some(GridItem::Folder(path)) if path == &folder)
        },
    );
    (folder, page)
}

fn collection_order_current_path(app: &mut App, detached: bool) -> Option<std::path::PathBuf> {
    let current = |owner: &App| {
        owner
            .fullscreen_idx
            .and_then(|idx| owner.items.get(idx))
            .and_then(|item| match item {
                GridItem::Image(path) => Some(path.clone()),
                _ => None,
            })
    };
    if detached {
        app.with_active_viewer_context(|owner| current(owner))
            .flatten()
    } else {
        current(app)
    }
}

fn collection_order_painted(
    frame: &ScenarioFrame,
    viewport: Option<egui::ViewportId>,
    path: &std::path::Path,
) -> bool {
    let key = GridItem::Image(path.to_path_buf()).perf_key();
    match viewport {
        Some(viewport) => records_for(frame, viewport)
            .iter()
            .any(|record| record.provenance.item == key),
        None => frame
            .records
            .values()
            .flatten()
            .any(|record| record.provenance.item == key),
    }
}

fn collection_order_open(
    app: &mut App,
    driver: &mut ScenarioDriver,
    detached: bool,
    index: usize,
    path: &std::path::Path,
) {
    app.selected = Some(index);
    let viewport = if detached {
        let main_items = app.items.clone();
        let main_generation = app.items_generation;
        let main_surface_generation = app.top_level_grid_view.generation();
        let main_scroll = app.scroll_offset_y;
        let main_prepared = std::sync::Arc::clone(
            app.top_level_grid_view
                .collection_session()
                .and_then(|session| session.prepared())
                .expect("installed collection root"),
        );
        driver.focus(egui::ViewportId::ROOT);
        driver.root_with_events(
            app,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        let window_id = app.active_detached_window_id().expect("detached grid open");
        assert_eq!(app.items, main_items, "opening must preserve the main list");
        assert_eq!(app.items_generation, main_generation);
        assert_eq!(app.selected, Some(index));
        assert_eq!(app.scroll_offset_y, main_scroll);
        assert_eq!(
            app.top_level_grid_view.generation(),
            main_surface_generation
        );
        assert!(std::sync::Arc::ptr_eq(
            app.top_level_grid_view
                .collection_session()
                .and_then(|session| session.prepared())
                .expect("main presentation retained"),
            &main_prepared,
        ));
        app.with_active_viewer_context(|owner| {
            assert_eq!(
                owner.items, main_items,
                "new owner needs the complete root order"
            );
            assert_ne!(owner.items_generation, main_generation);
            assert_eq!(
                owner.visible_indices,
                (0..main_items.len()).collect::<Vec<_>>()
            );
            assert_eq!(
                owner.navigation_scope,
                ViewerNavigationScope::CollectionRoot
            );
            assert!(owner.current_folder.is_none());
            assert!(owner.folder_pane_open_pending.is_none(), "no physical scan");
            let session = owner
                .top_level_grid_view
                .collection_session()
                .expect("root session");
            assert!(matches!(
                session.position,
                super::top_level_grid_view::CollectionGridPosition::Root
            ));
            assert_eq!(
                session.installed_items_generation,
                Some(owner.items_generation)
            );
            assert!(std::sync::Arc::ptr_eq(
                session.prepared().expect("prepared"),
                &main_prepared
            ));
        })
        .expect("detached owner");
        let viewport = App::detached_image_window_viewport_id(window_id);
        driver.focus(viewport);
        Some(viewport)
    } else {
        app.fs_open_intent_from_grid = true;
        app.open_fullscreen(index, HistoryTrigger::UserChosen);
        assert_eq!(
            collection_order_current_path(app, false).as_deref(),
            Some(path),
            "full grid open before first frame"
        );
        None
    };
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(15);
    loop {
        let frame = driver.root(app);
        let actual = collection_order_current_path(app, detached);
        if actual.as_deref() == Some(path) && collection_order_painted(&frame, viewport, path) {
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "collection image did not paint: expected={} actual={actual:?} viewport={viewport:?} frame={} records={:#?} errors={:?}",
            path.display(),
            frame.number,
            frame.records,
            frame.errors,
        );
        std::thread::yield_now();
    }
}

fn collection_order_page_turn(
    app: &mut App,
    driver: &mut ScenarioDriver,
    detached: bool,
    forward: bool,
) {
    let turn = |owner: &mut App| {
        let from = owner.fullscreen_idx.expect("image is open");
        owner.handle_fs_navigation(
            &driver.ctx,
            false,
            false,
            None,
            None,
            None,
            crate::ui_fullscreen::FsPageNav::Delta(if forward { 1 } else { -1 }),
            None,
            from,
        );
    };
    if detached {
        app.with_active_viewer_context(turn)
            .expect("detached owner");
    } else {
        turn(app);
    }
}

fn collection_order_slideshow_tick(app: &mut App, driver: &mut ScenarioDriver, detached: bool) {
    let tick = |owner: &mut App| {
        let from = owner.fullscreen_idx.expect("image is open");
        owner.slideshow_playing = true;
        owner.slideshow_anchor_idx = Some(from);
        owner.slideshow_next_at = std::time::Instant::now();
        // This is the production timer entry, which calls advance_slideshow.
        owner.handle_fs_navigation(
            &driver.ctx,
            false,
            false,
            None,
            None,
            None,
            crate::ui_fullscreen::FsPageNav::None,
            None,
            from,
        );
    };
    if detached {
        app.with_active_viewer_context(tick)
            .expect("detached owner");
    } else {
        tick(app);
    }
}

fn collection_order_observe(
    app: &mut App,
    driver: &mut ScenarioDriver,
    detached: bool,
    expected: &std::path::Path,
) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    let viewport = detached.then(|| {
        App::detached_image_window_viewport_id(
            app.active_detached_window_id()
                .expect("detached window stays open"),
        )
    });
    loop {
        let frame = driver.root(app);
        let actual = collection_order_current_path(app, detached);
        let (pending, display_pending) = if detached {
            app.with_active_viewer_context(|owner| {
                (
                    owner.top_level_grid_view.collection_navigation_pending(),
                    owner.fs_nav_is_locked(),
                )
            })
            .expect("detached collection navigation owner")
        } else {
            (
                app.top_level_grid_view.collection_navigation_pending(),
                app.fs_nav_is_locked(),
            )
        };
        // The collection request hands its target to the fullscreen display sequence. A
        // painted thumbnail can precede that sequence's terminal readiness, when another Ctrl
        // action is still deliberately blocked. Observe both typed owners before issuing the
        // next action; a wrong target after both finish remains a hard failure.
        if !pending
            && !display_pending
            && actual.as_deref() == Some(expected)
            && collection_order_painted(&frame, viewport, expected)
        {
            return;
        }
        if !pending && !display_pending && actual.as_deref() != Some(expected) {
            assert_eq!(actual.as_deref(), Some(expected));
        }
        assert!(
            std::time::Instant::now() < deadline,
            "collection navigation did not paint: expected={} actual={actual:?} pending={pending} display_pending={display_pending} viewport={viewport:?} frame={} records={:#?} errors={:?}",
            expected.display(),
            frame.number,
            frame.records,
            frame.errors,
        );
        std::thread::yield_now();
    }
}

#[test]
fn multiwindow_scenario_collection_order_full_mode_control() {
    std::thread::spawn(|| {
        let mut app = setup_app_for_test();
        let mut driver = ScenarioDriver::new();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
        app.startup_done = true;
        app.startup_init = None;
        app.settings.detached_viewer_open_images_in_window = false;
        driver.root(&mut app);
        let temp = app.tmp.path().to_path_buf();
        let [a, b, _c] = collection_order_fixture(&mut app, &mut driver, &temp);
        collection_order_open(&mut app, &mut driver, false, 0, &a);
        collection_order_page_turn(&mut app, &mut driver, false, true);
        collection_order_observe(&mut app, &mut driver, false, &b);
        collection_order_page_turn(&mut app, &mut driver, false, false);
        collection_order_observe(&mut app, &mut driver, false, &a);
        collection_order_slideshow_tick(&mut app, &mut driver, false);
        collection_order_observe(&mut app, &mut driver, false, &b);
    })
    .join()
    .expect("scenario thread");
}

fn run_detached_collection_order_case(open_index: usize, forward: Option<bool>) {
    let mut app = setup_app_for_test();
    let mut driver = ScenarioDriver::new();
    crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
    app.startup_done = true;
    app.startup_init = None;
    app.settings.detached_viewer_open_images_in_window = true;
    driver.root(&mut app);
    let temp = app.tmp.path().to_path_buf();
    let [a, b, _c] = collection_order_fixture(&mut app, &mut driver, &temp);
    let opened = if open_index == 0 { &a } else { &b };
    let expected = if open_index == 0 { &b } else { &a };
    collection_order_open(&mut app, &mut driver, true, open_index, opened);
    if let Some(forward) = forward {
        collection_order_page_turn(&mut app, &mut driver, true, forward);
    } else {
        collection_order_slideshow_tick(&mut app, &mut driver, true);
    }
    collection_order_observe(&mut app, &mut driver, true, expected);
}

#[test]
fn multiwindow_scenario_collection_order_detached_next() {
    std::thread::spawn(|| run_detached_collection_order_case(0, Some(true)))
        .join()
        .expect("scenario thread");
}

#[test]
fn multiwindow_scenario_collection_order_detached_prev() {
    std::thread::spawn(|| run_detached_collection_order_case(1, Some(false)))
        .join()
        .expect("scenario thread");
}

#[test]
fn multiwindow_scenario_collection_order_detached_slideshow() {
    std::thread::spawn(|| run_detached_collection_order_case(0, None))
        .join()
        .expect("scenario thread");
}

#[test]
fn multiwindow_scenario_collection_root_detached_boundaries_and_spread() {
    std::thread::spawn(|| {
        let mut app = setup_app_for_test();
        let mut driver = ScenarioDriver::new();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
        app.startup_done = true;
        app.startup_init = None;
        app.settings.detached_viewer_open_images_in_window = true;
        driver.root(&mut app);
        let temp = app.tmp.path().to_path_buf();
        let [a, b, c] = collection_order_fixture(&mut app, &mut driver, &temp);
        collection_order_open(&mut app, &mut driver, true, 1, &b);
        app.with_active_viewer_context(|owner| {
            owner.handle_fullscreen_boundary_jump(&driver.ctx, 1, false, "test_home");
        })
        .expect("detached root");
        collection_order_observe(&mut app, &mut driver, true, &a);
        app.with_active_viewer_context(|owner| {
            let from = owner.fullscreen_idx.expect("Home landed");
            owner.handle_fullscreen_boundary_jump(&driver.ctx, from, true, "test_end");
        })
        .expect("detached root");
        collection_order_observe(&mut app, &mut driver, true, &c);
        app.with_active_viewer_context(|owner| {
            owner.spread_mode = crate::settings::SpreadMode::Ltr;
            let nav = owner.visible_indices.clone();
            let units = owner.spread_display_unit_pages_for_test(&nav);
            assert!(
                units
                    .iter()
                    .any(|unit| unit.contains(&0) && unit.contains(&1)),
                "spread must pair portraits across source folders: {units:?}"
            );
        })
        .expect("detached root");
        app.with_active_viewer_context(|owner| {
            let from = owner.fullscreen_idx.unwrap();
            owner.handle_fullscreen_boundary_jump(&driver.ctx, from, false, "test_spread_home");
        })
        .expect("detached root");
        collection_order_observe(&mut app, &mut driver, true, &a);
        collection_order_page_turn(&mut app, &mut driver, true, true);
        collection_order_observe(&mut app, &mut driver, true, &c);
        app.with_active_viewer_context(|owner| {
            let from = owner.fullscreen_idx.unwrap();
            owner.handle_fullscreen_boundary_jump(
                &driver.ctx,
                from,
                false,
                "test_spread_slideshow_home",
            );
        })
        .expect("detached root");
        collection_order_observe(&mut app, &mut driver, true, &a);
        collection_order_slideshow_tick(&mut app, &mut driver, true);
        collection_order_observe(&mut app, &mut driver, true, &c);
    })
    .join()
    .expect("scenario thread");
}

#[test]
fn multiwindow_scenario_collection_root_passive_reopen_without_bundle() {
    std::thread::spawn(|| {
        let mut app = setup_app_for_test();
        let mut driver = ScenarioDriver::new();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
        app.startup_done = true;
        app.startup_init = None;
        app.settings.detached_viewer_open_images_in_window = true;
        driver.root(&mut app);
        let temp = app.tmp.path().to_path_buf();
        let [a, b, _] = collection_order_fixture(&mut app, &mut driver, &temp);
        collection_order_open(&mut app, &mut driver, true, 0, &a);
        let window_id = app.active_detached_window_id().expect("detached window");
        let viewport_id = App::detached_image_window_viewport_id(window_id);
        let placement = app.active_detached_viewer_current_placement();
        assert!(app.park_and_close_current_active_detached_viewer(&driver.ctx));
        let snapshot = app
            .detached_image_windows
            .iter()
            .find(|window| window.id == window_id)
            .expect("parked still snapshot");
        assert_eq!(snapshot.title, "2.png - mimageviewer");
        assert_eq!(snapshot.location_display, "2.png");
        assert!(snapshot.reopen_collection_root.is_some());
        assert!(snapshot.reopen_descriptor.is_none(), "no physical fallback");
        let (old_context, residence) = app.locate_window_context(window_id).expect("parked bundle");
        assert_eq!(residence, ContextResidence::AtRest);
        app.retire_context(
            old_context,
            "test_force_absent_collection_root_bundle",
            |_| (),
        )
        .expect("retire parked bundle");
        assert!(app.locate_window_context(window_id).is_none());
        assert!(matches!(
            app.right_drag_viewer_identity_for_window_id(window_id),
            Some(DetachedRightDragViewerIdentity::ReopenCollectionRoot { .. })
        ));
        assert!(app.activate_detached_image_window_snapshot(&driver.ctx, window_id));
        assert_eq!(app.active_detached_window_id(), Some(window_id));
        assert_eq!(
            App::detached_image_window_viewport_id(window_id),
            viewport_id
        );
        assert_eq!(app.active_detached_viewer_current_placement(), placement);
        collection_order_observe(&mut app, &mut driver, true, &a);
        collection_order_page_turn(&mut app, &mut driver, true, true);
        collection_order_observe(&mut app, &mut driver, true, &b);
    })
    .join()
    .expect("scenario thread");
}

#[test]
fn multiwindow_scenario_collection_root_removed_source_reopen_current_behavior() {
    std::thread::spawn(|| {
        let mut app = setup_app_for_test();
        let mut driver = ScenarioDriver::new();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
        app.startup_done = true;
        app.startup_init = None;
        app.settings.detached_viewer_open_images_in_window = true;
        driver.root(&mut app);
        let temp = app.tmp.path().to_path_buf();
        let [a, _, _] = collection_order_fixture(&mut app, &mut driver, &temp);
        collection_order_open(&mut app, &mut driver, true, 0, &a);
        let window_id = app.active_detached_window_id().unwrap();
        assert!(app.park_and_close_current_active_detached_viewer(&driver.ctx));
        let (old_context, ContextResidence::AtRest) = app.locate_window_context(window_id).unwrap()
        else {
            panic!("parked still owns an AtRest bundle");
        };
        app.retire_context(old_context, "test_removed_source_reopen", |_| ())
            .unwrap();
        std::fs::remove_file(&a).unwrap();

        // §1.269: both physical and collection reopen currently commit before async decode.
        assert!(app.activate_detached_image_window_snapshot(&driver.ctx, window_id));
        assert_eq!(app.active_detached_window_id(), Some(window_id));
        assert!(
            !app.detached_image_windows
                .iter()
                .any(|window| window.id == window_id)
        );
        app.with_window_viewer_context(window_id, |owner| {
            assert_eq!(
                owner.navigation_scope,
                ViewerNavigationScope::CollectionRoot
            );
            assert_eq!(owner.fullscreen_idx, Some(0));
            assert_eq!(owner.items[0], GridItem::Image(a.clone()));
            assert!(owner.folder_pane_open_pending.is_none());
        })
        .expect("reopened window has one complete owner");
        wait_until(
            "removed-source decode terminal",
            std::time::Duration::from_secs(10),
            || {
                driver.root(&mut app);
                app.with_active_viewer_context(|owner| {
                    owner.fs_pending.is_empty() && owner.fs_upload_backlog.is_empty()
                })
                .unwrap_or(false)
            },
        );
        assert_eq!(app.active_detached_window_id(), Some(window_id));
        app.with_active_viewer_context(|owner| {
            assert_eq!(owner.fullscreen_idx, Some(0));
            assert!(
                matches!(owner.fs_cache.get(&0), Some(FsCacheEntry::Failed)),
                "missing source terminates as a failed image in the committed owner"
            );
        })
        .unwrap();
    })
    .join()
    .expect("scenario thread");
}

#[test]
fn multiwindow_scenario_collection_root_detached_edit_delete_and_reopen() {
    std::thread::spawn(|| {
        let mut app = setup_app_for_test();
        let mut driver = ScenarioDriver::new();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
        app.startup_done = true;
        app.startup_init = None;
        app.settings.detached_viewer_open_images_in_window = true;
        driver.root(&mut app);
        let temp = app.tmp.path().to_path_buf();
        let [a, _b, c] = collection_order_fixture(&mut app, &mut driver, &temp);
        let prepared = app
            .top_level_grid_view
            .collection_session()
            .and_then(|session| session.prepared())
            .expect("root prepared");
        let collection_id = prepared.collection_id;
        let revision = prepared.collection_revision;
        let removed_entry = prepared.entries[1].entry_id;
        let client = app.collection_store_client_for_read().unwrap().unwrap();
        collection_order_open(&mut app, &mut driver, true, 0, &a);
        let window_id = app.active_detached_window_id().unwrap();
        let placement = app.active_detached_viewer_current_placement();
        let removed = client
            .remove_entries(collection_id, revision, vec![removed_entry])
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap()
            .unwrap();
        wait_until(
            "detached collection edit watch",
            std::time::Duration::from_secs(10),
            || {
                driver.root(&mut app);
                app.with_active_viewer_context(|owner| {
                    let session = owner.top_level_grid_view.collection_session().unwrap();
                    session.wanted_revision >= removed.revision()
                        && owner.fullscreen_idx == Some(0)
                        && owner.items.len() == 3
                })
                .unwrap_or(false)
            },
        );
        collection_order_page_turn(&mut app, &mut driver, true, true);
        collection_order_observe(&mut app, &mut driver, true, &c);
        client
            .delete_collection(collection_id, removed.revision())
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap()
            .unwrap();
        wait_until(
            "detached collection delete watch",
            std::time::Duration::from_secs(10),
            || {
                driver.root(&mut app);
                app.with_active_viewer_context(|owner| {
                    matches!(
                        owner
                            .top_level_grid_view
                            .collection_session()
                            .map(|session| &session.load),
                        Some(super::top_level_grid_view::CollectionGridLoadState::Deleted { .. })
                    ) && owner.fullscreen_idx.is_some()
                })
                .unwrap_or(false)
            },
        );
        assert_eq!(
            collection_order_current_path(&mut app, true).as_deref(),
            Some(c.as_path())
        );
        assert!(app.park_and_close_current_active_detached_viewer(&driver.ctx));
        let parked = app
            .detached_image_windows
            .iter()
            .find(|window| window.id == window_id)
            .unwrap();
        assert!(parked.reopen_collection_root.is_some());
        assert!(super::detached_window_references_removed(
            parked,
            None,
            &|key| key == crate::adjustment_db::normalize_path(&c),
        ));
        assert!(!super::detached_window_references_removed(
            parked,
            None,
            &|key| key == collection_id.as_uuid().to_string(),
        ));
        let (old_context, _) = app.locate_window_context(window_id).unwrap();
        app.retire_context(old_context, "test_deleted_collection_reopen", |_| ())
            .unwrap();
        assert!(app.activate_detached_image_window_snapshot(&driver.ctx, window_id));
        assert_eq!(app.active_detached_viewer_current_placement(), placement);
        collection_order_observe(&mut app, &mut driver, true, &c);
    })
    .join()
    .expect("scenario thread");
}

#[test]
fn multiwindow_scenario_collection_root_f12_modes() {
    std::thread::spawn(|| {
        let mut app = setup_app_for_test();
        let mut driver = ScenarioDriver::new();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
        app.startup_done = true;
        app.startup_init = None;
        app.settings.detached_viewer_open_images_in_window = false;
        driver.root(&mut app);
        let temp = app.tmp.path().to_path_buf();
        let [a, b, _] = collection_order_fixture(&mut app, &mut driver, &temp);
        collection_order_open(&mut app, &mut driver, false, 0, &a);
        app.toggle_detached_viewer_mode();
        assert!(
            app.settings.detached_viewer_enabled,
            "F12 is enabled in full mode"
        );
        assert!(matches!(
            app.top_level_grid_view.surface(),
            super::top_level_grid_view::TopLevelGridSurface::Collection(_)
        ));
        collection_order_page_turn(&mut app, &mut driver, false, true);
        collection_order_observe(&mut app, &mut driver, false, &b);

        // In always-new mode the same operation is intentionally disabled for still images.
        app.settings.detached_viewer_open_images_in_window = true;
        let before = app.settings.detached_viewer_enabled;
        app.toggle_detached_viewer_mode();
        assert_eq!(app.settings.detached_viewer_enabled, before);
        assert!(matches!(
            app.top_level_grid_view.surface(),
            super::top_level_grid_view::TopLevelGridSurface::Collection(_)
        ));
    })
    .join()
    .expect("scenario thread");
}

#[test]
fn multiwindow_scenario_collection_root_detached_bs_and_esc() {
    std::thread::spawn(|| {
        let mut app = setup_app_for_test();
        let mut driver = ScenarioDriver::new();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
        app.startup_done = true;
        app.startup_init = None;
        app.settings.detached_viewer_open_images_in_window = true;
        driver.root(&mut app);
        let temp = app.tmp.path().to_path_buf();
        let [a, _, _] = collection_order_fixture(&mut app, &mut driver, &temp);
        collection_order_open(&mut app, &mut driver, true, 0, &a);
        app.with_active_viewer_context(|owner| {
            let from = owner.fullscreen_idx.unwrap();
            owner.handle_fs_navigation(
                &driver.ctx,
                false,
                true,
                None,
                None,
                None,
                crate::ui_fullscreen::FsPageNav::None,
                None,
                from,
            );
            assert!(owner.fullscreen_idx.is_none(), "BS closes the leaf");
            assert!(matches!(
                owner.top_level_grid_view.surface(),
                super::top_level_grid_view::TopLevelGridSurface::Collection(_)
            ));
        })
        .expect("detached owner");
        assert!(matches!(
            app.top_level_grid_view.surface(),
            super::top_level_grid_view::TopLevelGridSurface::Collection(_)
        ));

        collection_order_open(&mut app, &mut driver, true, 0, &a);
        let closed_id = app.active_detached_window_id().unwrap();
        app.with_active_viewer_context(|owner| {
            let from = owner.fullscreen_idx.unwrap();
            owner.handle_fs_navigation(
                &driver.ctx,
                true,
                false,
                None,
                None,
                None,
                crate::ui_fullscreen::FsPageNav::None,
                None,
                from,
            );
        })
        .expect("detached owner");
        driver.root(&mut app);
        assert_ne!(app.active_detached_window_id(), Some(closed_id));
        assert!(matches!(
            app.top_level_grid_view.surface(),
            super::top_level_grid_view::TopLevelGridSurface::Collection(_)
        ));
    })
    .join()
    .expect("scenario thread");
}

#[test]
fn multiwindow_scenario_collection_root_sibling_watch_owners() {
    std::thread::spawn(|| {
        let mut app = setup_app_for_test();
        let mut driver = ScenarioDriver::new();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
        app.startup_done = true;
        app.startup_init = None;
        app.settings.detached_viewer_open_images_in_window = true;
        driver.root(&mut app);
        let temp = app.tmp.path().to_path_buf();
        let [a, b, _c] = collection_order_fixture(&mut app, &mut driver, &temp);
        let prepared = app
            .top_level_grid_view
            .collection_session()
            .unwrap()
            .prepared()
            .unwrap();
        let collection_id = prepared.collection_id;
        let revision = prepared.collection_revision;
        let removed_entry = prepared.entries[2].entry_id;
        let client = app.collection_store_client_for_read().unwrap().unwrap();
        collection_order_open(&mut app, &mut driver, true, 0, &a);
        let first_id = app.active_detached_window_id().unwrap();
        let first_placement = app.active_detached_viewer_current_placement();
        collection_order_open(&mut app, &mut driver, true, 1, &b);
        let second_id = app.active_detached_window_id().unwrap();
        assert_ne!(first_id, second_id);
        let first_context = app.locate_window_context(first_id).unwrap().0;
        let before = app
            .with_viewer_context(first_context, |owner| {
                owner
                    .top_level_grid_view
                    .collection_session()
                    .unwrap()
                    .wanted_revision
            })
            .unwrap();
        let removed = client
            .remove_entries(collection_id, revision, vec![removed_entry])
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap()
            .unwrap();
        wait_until(
            "active sibling watch",
            std::time::Duration::from_secs(10),
            || {
                driver.root(&mut app);
                app.with_active_viewer_context(|owner| {
                    owner
                        .top_level_grid_view
                        .collection_session()
                        .unwrap()
                        .wanted_revision
                        >= removed.revision()
                })
                .unwrap_or(false)
            },
        );
        assert_eq!(
            app.with_viewer_context(first_context, |owner| {
                owner
                    .top_level_grid_view
                    .collection_session()
                    .unwrap()
                    .wanted_revision
            })
            .unwrap(),
            before,
            "active polling must not drain the parked sibling watch"
        );
        assert!(app.activate_detached_image_window_snapshot(&driver.ctx, first_id));
        assert_eq!(app.active_detached_window_id(), Some(first_id));
        assert_eq!(
            app.active_detached_viewer_current_placement(),
            first_placement
        );
        collection_order_observe(&mut app, &mut driver, true, &a);
        wait_until(
            "resumed sibling watch",
            std::time::Duration::from_secs(10),
            || {
                driver.root(&mut app);
                app.with_active_viewer_context(|owner| {
                    owner
                        .top_level_grid_view
                        .collection_session()
                        .unwrap()
                        .wanted_revision
                        >= removed.revision()
                })
                .unwrap_or(false)
            },
        );
        assert!(
            app.detached_image_windows
                .iter()
                .any(|window| window.id == second_id)
        );
    })
    .join()
    .expect("scenario thread");
}

#[test]
fn multiwindow_scenario_collection_root_async_sibling_result_is_owner_scoped() {
    std::thread::spawn(|| {
        let mut app = setup_app_for_test();
        let mut driver = ScenarioDriver::new();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
        app.startup_done = true;
        app.startup_init = None;
        app.settings.detached_viewer_open_images_in_window = true;
        driver.root(&mut app);
        let temp = app.tmp.path().to_path_buf();
        let [a, b, c] = collection_order_fixture(&mut app, &mut driver, &temp);
        collection_order_open(&mut app, &mut driver, true, 0, &a);
        let first_id = app.active_detached_window_id().unwrap();
        collection_order_page_turn(&mut app, &mut driver, true, true);
        wait_until(
            "first window async snapshot",
            std::time::Duration::from_secs(5),
            || {
                app.with_active_viewer_context(|owner| {
                owner.poll_collection_navigation(&driver.ctx);
                matches!(
                    owner.top_level_grid_view.collection_navigation_pending_for_test(),
                    Some(super::collection_navigation::CollectionNavigationPending::Snapshot { .. })
                )
            })
            .unwrap_or(false)
            },
        );
        collection_order_open(&mut app, &mut driver, true, 2, &c);
        let second_id = app.active_detached_window_id().unwrap();
        assert_ne!(first_id, second_id);
        for _ in 0..4 {
            driver.root(&mut app);
        }
        let first_context = app
            .locate_window_context(first_id)
            .expect("parked first owner")
            .0;
        assert!(
            app.with_viewer_context(first_context, |owner| {
                matches!(
                    owner
                        .top_level_grid_view
                        .collection_navigation_pending_for_test(),
                    Some(
                        super::collection_navigation::CollectionNavigationPending::Snapshot { .. }
                    )
                )
            })
            .unwrap(),
            "the async reply remains with its parked request owner"
        );
        assert_eq!(
            collection_order_current_path(&mut app, true).as_deref(),
            Some(c.as_path()),
            "the first window's async result must not land in the second"
        );
        assert!(app.activate_detached_image_window_snapshot(&driver.ctx, first_id));
        assert_eq!(app.active_detached_window_id(), Some(first_id));
        collection_order_observe(&mut app, &mut driver, true, &b);
        assert!(
            app.detached_image_windows
                .iter()
                .any(|window| window.id == second_id)
        );
    })
    .join()
    .expect("scenario thread");
}

#[test]
fn multiwindow_scenario_collection_root_ctrl_outer_navigation() {
    std::thread::spawn(|| {
        let mut app = setup_app_for_test();
        let mut driver = ScenarioDriver::new();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
        app.startup_done = true;
        app.startup_init = None;
        app.settings.detached_viewer_open_images_in_window = true;
        app.settings.auto_fullscreen_image_folders = true;
        driver.root(&mut app);
        let temp = app.tmp.path().to_path_buf();
        let [a, _, _] = collection_order_fixture(&mut app, &mut driver, &temp);
        let (_, first_page) =
            collection_order_add_folder(&mut app, &mut driver, &temp, "outer-first", 3);
        let (_, second_page) =
            collection_order_add_folder(&mut app, &mut driver, &temp, "outer-second", 4);
        collection_order_open(&mut app, &mut driver, true, 0, &a);
        let window_id = app.active_detached_window_id().unwrap();
        app.with_active_viewer_context(|owner| {
            owner.handle_fullscreen_ctrl_nav_context(&driver.ctx, 0, true, false);
        })
        .unwrap();
        collection_order_observe(&mut app, &mut driver, true, &first_page);
        app.with_active_viewer_context(|owner| {
            let from = owner.fullscreen_idx.unwrap();
            owner.handle_fullscreen_ctrl_nav_context(&driver.ctx, from, true, false);
        })
        .unwrap();
        collection_order_observe(&mut app, &mut driver, true, &second_page);
        app.with_active_viewer_context(|owner| {
            let from = owner.fullscreen_idx.unwrap();
            owner.handle_fullscreen_ctrl_nav_context(&driver.ctx, from, false, false);
        })
        .unwrap();
        collection_order_observe(&mut app, &mut driver, true, &first_page);
        assert_eq!(app.active_detached_window_id(), Some(window_id));
    })
    .join()
    .expect("scenario thread");
}

#[test]
fn multiwindow_scenario_collection_root_slideshow_next_folder() {
    std::thread::spawn(|| {
        let mut app = setup_app_for_test();
        let mut driver = ScenarioDriver::new();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
        app.startup_done = true;
        app.startup_init = None;
        app.settings.detached_viewer_open_images_in_window = true;
        app.settings.auto_fullscreen_image_folders = true;
        driver.root(&mut app);
        let temp = app.tmp.path().to_path_buf();
        let [_, _, c] = collection_order_fixture(&mut app, &mut driver, &temp);
        let (folder, page) = collection_order_add_folder(&mut app, &mut driver, &temp, "collection-registered-book", 3);
        collection_order_open(&mut app, &mut driver, true, 2, &c);
        app.with_active_viewer_context(|owner| {
            owner.settings.slideshow_end_action =
                crate::settings::SlideshowEndAction::NextFolder;
        })
        .unwrap();
        collection_order_slideshow_tick(&mut app, &mut driver, true);
        wait_until("detached NextFolder child landing", std::time::Duration::from_secs(10), || {
            driver.root(&mut app);
            app.with_active_viewer_context(|owner| {
                owner.fullscreen_idx.and_then(|idx| owner.items.get(idx))
                    == Some(&GridItem::Image(page.clone()))
                    && matches!(owner.top_level_grid_view.collection_session().map(|session| &session.position),
                        Some(super::top_level_grid_view::CollectionGridPosition::PhysicalSource { .. }))
            })
            .unwrap_or(false)
        });
        assert!(folder.is_dir());
    })
    .join()
    .expect("scenario thread");
}

#[test]
fn multiwindow_scenario_collection_root_detached_folder_child_restore() {
    std::thread::spawn(|| {
        let mut app = setup_app_for_test();
        let mut driver = ScenarioDriver::new();
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&driver.ctx);
        app.startup_done = true;
        app.startup_init = None;
        app.settings.detached_viewer_open_images_in_window = true;
        app.settings.auto_fullscreen_image_folders = true;
        driver.root(&mut app);
        let temp = app.tmp.path().to_path_buf();
        let _ = collection_order_fixture(&mut app, &mut driver, &temp);
        let (folder, page) = collection_order_add_folder(
            &mut app,
            &mut driver,
            &temp,
            "collection-registered-book",
            3,
        );
        let collection_id = app
            .top_level_grid_view
            .collection_session()
            .unwrap()
            .identity
            .collection_id;
        let main_items = app.items.clone();
        app.selected = Some(3);
        driver.focus(egui::ViewportId::ROOT);
        driver.root_with_events(
            &mut app,
            vec![egui::Event::Key {
                key: egui::Key::Enter,
                physical_key: None,
                pressed: true,
                repeat: false,
                modifiers: egui::Modifiers::NONE,
            }],
        );
        wait_until(
            "detached folder child open",
            std::time::Duration::from_secs(10),
            || {
                driver.root(&mut app);
                app.with_active_viewer_context(|owner| {
                    owner.fullscreen_idx.and_then(|idx| owner.items.get(idx))
                        == Some(&GridItem::Image(page.clone()))
                        && matches!(
                            owner.top_level_grid_view.return_to(),
                            Some(super::top_level_grid_view::TopLevelGridRestore::Collection(
                                _
                            ))
                        )
                })
                .unwrap_or(false)
            },
        );
        let window_id = app.active_detached_window_id().unwrap();
        app.with_active_viewer_context(|owner| {
            assert!(
                owner
                    .current_folder
                    .as_deref()
                    .is_some_and(|path| crate::folder_tree::path_eq(path, &folder))
            );
            let restore = match owner
                .top_level_grid_view
                .return_to()
                .expect("collection return")
            {
                super::top_level_grid_view::TopLevelGridRestore::Collection(restore) => {
                    restore.clone()
                }
                other => panic!("expected collection restore, got {other:?}"),
            };
            owner.close_fullscreen();
            owner.open_collection_grid(collection_id, Some(restore));
        })
        .unwrap();
        wait_until(
            "detached child restored root",
            std::time::Duration::from_secs(10),
            || {
                app.with_active_viewer_context(|owner| {
                    owner.poll_collection_grid(&driver.ctx);
                    owner.items == main_items
                        && matches!(
                            owner
                                .top_level_grid_view
                                .collection_session()
                                .map(|session| &session.position),
                            Some(super::top_level_grid_view::CollectionGridPosition::Root)
                        )
                })
                .unwrap_or(false)
            },
        );
        assert_eq!(app.active_detached_window_id(), Some(window_id));
        assert_eq!(app.items, main_items);
    })
    .join()
    .expect("scenario thread");
}

#[test]
fn multiwindow_scenario_noncollection_image_controls_remain_physical() {
    use super::top_level_grid_view::{TopLevelGridSurface, TopLevelSearchView};
    for surface in [
        TopLevelGridSurface::Search(TopLevelSearchView::Global),
        TopLevelGridSurface::Rating { stars: 5 },
        TopLevelGridSurface::ReadingHistory,
    ] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        app.settings.detached_viewer_open_images_in_window = true;
        let folder = app.tmp.path().join("physical-control");
        std::fs::create_dir(&folder).unwrap();
        let image = folder.join("page.png");
        save_portrait(&image, [40, 70, 100]);
        app.top_level_grid_view.begin(surface.clone(), None);
        app.items = vec![GridItem::Image(image.clone())];
        app.image_metas = vec![None];
        app.visible_indices = vec![0];
        app.selected = Some(0);
        app.items_are_global_search_view = matches!(surface, TopLevelGridSurface::Search(_));
        app.items_are_rating_view = matches!(surface, TopLevelGridSurface::Rating { .. });
        app.items_are_reading_history_view = matches!(surface, TopLevelGridSurface::ReadingHistory);
        assert!(matches!(
            app.detached_grid_item_open_plan(0, false),
            Some(DetachedGridItemOpenPlan::Descriptor {
                descriptor: ViewerContextDescriptor::Image { .. },
                collection_restore: None,
            })
        ));
        assert!(app.open_grid_container_in_detached_book_context(&ctx, 0));
        app.with_active_viewer_context(|owner| {
            assert_eq!(
                owner.navigation_scope,
                ViewerNavigationScope::DetachedPhysical
            );
            assert!(owner.folder_pane_open_pending.is_some());
            assert!(owner.top_level_grid_view.collection_session().is_none());
        })
        .expect("physical image owner");
    }
}

#[test]
fn multiwindow_scenario_collection_parked_live_eof_owner_poll() {
    use crate::collection_store::{
        CollectionRegistration, CollectionResolvedKind, CollectionStoreRuntime,
    };
    for is_audio in [false, true] {
        let mut app = setup_app_for_test();
        let ctx = egui::Context::default();
        let temp = app.tmp.path().to_path_buf();
        let first = temp.join(if is_audio { "first.flac" } else { "first.mp4" });
        let second = temp.join(if is_audio {
            "second.flac"
        } else {
            "second.mp4"
        });
        std::fs::write(&first, b"fixture").unwrap();
        std::fs::write(&second, b"fixture").unwrap();
        let runtime = CollectionStoreRuntime::start_at(temp.join("media-collection.db"))
            .expect("collection actor");
        let client = runtime.client();
        app.install_collection_runtime(runtime);
        wait_until(
            "media collection actor ready",
            std::time::Duration::from_secs(5),
            || {
                app.poll_collection_ui(&ctx);
                matches!(app.collection_store_client_for_read(), Ok(Some(_)))
            },
        );
        let created = client
            .create_collection("Media EOF".into())
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap()
            .unwrap();
        let kind = if is_audio {
            CollectionResolvedKind::Audio
        } else {
            CollectionResolvedKind::Video
        };
        let registrations = [&first, &second]
            .map(|path| CollectionRegistration::from_trusted_path(path, kind).unwrap())
            .to_vec();
        client
            .add_batch(created.collection_id(), created.revision(), registrations)
            .unwrap()
            .recv_timeout(std::time::Duration::from_secs(3))
            .unwrap()
            .unwrap();
        app.open_collection_grid_from_navigation(created.collection_id());
        wait_until(
            "media collection root",
            std::time::Duration::from_secs(10),
            || {
                app.poll_collection_ui(&ctx);
                app.poll_collection_grid(&ctx);
                app.items.len() == 2
            },
        );
        let grid = app.top_level_grid_view.clone();
        let items = app.items.clone();
        let metas = app.image_metas.clone();
        let window_id = if is_audio { 711 } else { 710 };
        let source = first.clone();
        app.push_window_context_for_test(&ctx, window_id, move |owner| {
            owner.top_level_grid_view = grid;
            owner.install_prepared_aggregate_items(items, metas);
            owner
                .top_level_grid_view
                .collection_session_mut()
                .unwrap()
                .installed_items_generation = Some(owner.items_generation);
            owner.fullscreen_idx = Some(0);
            owner.viewer_presentation = ViewerPresentation::DetachedWindow;
            owner.video_continuous_mode = crate::video::VideoContinuousMode::Continuous;
            owner.video_continuous_last_eof = Some((0, 1));
            owner.fs_cache.insert(
                0,
                FsCacheEntry::Video {
                    player: Box::new(crate::video::VideoPlayer::disconnected_for_test(
                        source, 3.0,
                    )),
                    load_seq: 0,
                },
            );
            owner.set_detached_window_binding_for_test(Some(window_id));
        });
        app.transition_detached_window_state(
            window_id,
            DetachedWindowState::ParkedLive,
            "test_eof",
        );
        assert!(
            app.with_window_viewer_context(window_id, |owner| {
                if is_audio {
                    owner.start_collection_music_eof_navigation(&ctx, 0, 1)
                } else {
                    owner.start_collection_video_eof_navigation(&ctx, 0, 1)
                }
            })
            .unwrap()
        );
        assert!(
            app.with_window_viewer_context(window_id, |owner| {
                matches!(
                    owner
                        .top_level_grid_view
                        .collection_navigation_pending_for_test(),
                    Some(
                        super::collection_navigation::CollectionNavigationPending::Snapshot { .. }
                    )
                )
            })
            .unwrap()
        );
        wait_until(
            "ParkedLive EOF landing",
            std::time::Duration::from_secs(10),
            || {
                app.poll_parked_live_detached_windows(&ctx);
                app.with_window_viewer_context(window_id, |owner| {
                    owner.fullscreen_idx == Some(1)
                        && match owner.items.get(1) {
                            Some(GridItem::Audio(path)) if is_audio => {
                                crate::folder_tree::path_eq(path, &second)
                            }
                            Some(GridItem::Video(path)) if !is_audio => {
                                crate::folder_tree::path_eq(path, &second)
                            }
                            _ => false,
                        }
                })
                .unwrap_or(false)
            },
        );
        assert_eq!(app.fullscreen_idx, None, "main must not consume parked EOF");
    }
}

#[test]
fn multiwindow_scenario_collection_root_stale_grid_open_is_terminal() {
    std::thread::spawn(|| {
        let mut app = setup_app_for_test();
        let mut driver = ScenarioDriver::new();
        app.startup_done = true;
        app.startup_init = None;
        app.settings.detached_viewer_open_images_in_window = true;
        driver.root(&mut app);
        let temp = app.tmp.path().to_path_buf();
        let [first, _, _] = collection_order_fixture(&mut app, &mut driver, &temp);
        let main_items = app.items.clone();
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .installed_items_generation = None;
        assert!(matches!(
            app.detached_grid_item_open_plan(0, false),
            Some(DetachedGridItemOpenPlan::CollectionRootUnavailable)
        ));
        assert!(app.open_grid_container_in_detached_book_context(&driver.ctx, 0));
        assert_eq!(app.items, main_items);
        assert!(app.active_detached_window_id().is_none());

        let entry = app
            .top_level_grid_view
            .collection_session()
            .unwrap()
            .prepared()
            .unwrap()
            .entries[0]
            .clone();
        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .position = super::top_level_grid_view::CollectionGridPosition::PhysicalSource {
            entry_id: entry.entry_id,
            source_key: entry.source_key,
            path: first.parent().unwrap().to_path_buf(),
        };
        assert!(matches!(
            app.detached_grid_item_open_plan(0, false),
            Some(DetachedGridItemOpenPlan::Descriptor { .. })
        ));
    })
    .join()
    .expect("scenario thread");
}

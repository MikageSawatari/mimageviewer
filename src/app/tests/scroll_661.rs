//! Headless investigation of 5ch #661. No native windows or real profile data.
use super::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use egui::emath::GuiRounding as _;
use std::io::{Cursor, Write};
use std::time::{Duration, Instant};

fn fixtures(root: &std::path::Path, count: usize, zip: bool) -> PathBuf {
    let folder = root.join(format!("{}-{count}", if zip { "zip" } else { "jpg" }));
    std::fs::create_dir_all(&folder).unwrap();
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        32,
        48,
        image::Rgb([80, 120, 160]),
    ))
    .write_to(&mut bytes, image::ImageFormat::Jpeg)
    .unwrap();
    for n in 0..count {
        if zip {
            let file = std::fs::File::create(folder.join(format!("{n:04}.zip"))).unwrap();
            let mut archive = zip::ZipWriter::new(file);
            archive
                .start_file("page.jpg", zip::write::SimpleFileOptions::default())
                .unwrap();
            archive.write_all(bytes.get_ref()).unwrap();
            archive.finish().unwrap();
        } else {
            std::fs::write(folder.join(format!("{n:04}.jpg")), bytes.get_ref()).unwrap();
        }
    }
    folder
}

struct Driver {
    backend: eframe::HeadlessEpiIntegration,
    ctx: egui::Context,
    frame: u64,
    previous: String,
    early_hydration: bool,
    fresh_scroll_surface: bool,
}

impl Driver {
    fn new() -> Self {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
        Self {
            backend: eframe::HeadlessEpiIntegration::new(ctx.clone()),
            ctx,
            frame: 0,
            previous: String::new(),
            early_hydration: false,
            fresh_scroll_surface: false,
        }
    }

    fn checkpoint(&mut self, app: &App, phase: &str) {
        let selected = app
            .selected
            .and_then(|i| app.items.get(i))
            .map(GridItem::name);
        let state = format!(
            "folder={:?} selected={:?}/{selected:?} saved={:?} request={:?}/ensure={} offset={} rows={} natural_h={} viewport={} cell_h={} gen={} lazy={:?} hydration={} restore_phase={:?}",
            app.current_folder.as_ref().and_then(|p| p.file_name()),
            app.selected,
            app.current_folder
                .as_ref()
                .and_then(|p| app.folder_history.get(p)),
            app.grid_scroll.pending_edge(),
            app.grid_scroll.reveal_requested(),
            app.grid_scroll.offset(),
            app.current_grid_order().len(),
            if app.settings.grid_view_mode == crate::settings::GridViewMode::Details {
                app.current_grid_order().len() as f32 * 28.0
            } else {
                app.current_grid_order()
                    .len()
                    .div_ceil(app.last_grid_cols.max(1)) as f32
                    * app.last_cell_h
            },
            app.last_viewport_h,
            app.last_cell_h,
            app.items_generation,
            app.details_image_dims_state,
            app.sidecar_restore_active(),
            app.sidecar_restore
                .as_ref()
                .map(|restore| restore.presentation().heading),
        );
        if state != self.previous {
            println!("SCROLL661 frame={} {phase}: {state}", self.frame);
            let mut scroll_states = self.ctx.data(|data| {
                data.iter_temp_by_type::<egui::scroll_area::State>()
                    .map(|state| format!("{state:?}"))
                    .collect::<Vec<_>>()
            });
            scroll_states.sort();
            println!(
                "SCROLL661_EGUI frame={} {phase}: {scroll_states:?}",
                self.frame
            );
            self.previous = state;
        }
    }

    fn tick(&mut self, app: &mut App, pane: Option<PathBuf>) {
        self.tick_events(app, pane, Vec::new());
    }

    fn tick_events(&mut self, app: &mut App, pane: Option<PathBuf>, events: Vec<egui::Event>) {
        self.frame += 1;
        let generation_before = app.items_generation;
        if let Some(path) = pane {
            // The actual folder-tree input handler's worker admission. Full App frames
            // perform scan-ready arbitration, adoption, hydration and all UI layout.
            app.start_folder_pane_open(path);
        }
        self.checkpoint(app, "before-full-frame");
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            time: Some(self.frame as f64 / 60.0),
            events,
            focused: false,
            ..Default::default()
        };
        raw.viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .focused = Some(false);
        self.backend.update(app, raw).drop_without_applying_deltas();
        self.checkpoint(app, "after-full-frame");
        // Select a valid fast-worker schedule using actual replies, never fake results.
        if self.early_hydration && app.sidecar_restore_active() {
            app.finish_page_identity_preparation_for_test();
            self.checkpoint(app, "restored-before-first-destination-grid");
        }
        if self.fresh_scroll_surface && app.items_generation != generation_before {
            // Diagnostic control only. Preserve App state; discard old egui geometry.
            let fresh = Driver::new();
            self.ctx = fresh.ctx;
            self.backend = fresh.backend;
        }
    }

    fn settled(&mut self, app: &mut App) {
        let deadline = Instant::now() + Duration::from_secs(30);
        let mut quiet = 0;
        let mut previous_scroll = app.grid_scroll.offset();
        loop {
            self.tick(app, None);
            if app.folder_pane_open_pending.is_none()
                && !app.sidecar_restore_active()
                && app.details_meta_pending.is_none()
                && matches!(
                    app.details_image_dims_state,
                    LazyColumnState::Ready { .. } | LazyColumnState::Disabled
                )
                && (app.grid_scroll.offset() - previous_scroll).abs() < 0.1
            {
                quiet += 1;
                if quiet == 4 {
                    return;
                }
            } else {
                quiet = 0;
            }
            previous_scroll = app.grid_scroll.offset();
            if Instant::now() >= deadline {
                let writer_busy = app.metadata_transfer_writers_busy_for_sidecar_restore(false);
                println!(
                    "SCROLL661_TIMEOUT writers={writer_busy:?} local_worker={} favorite_worker={} tag_maintenance={} cleanup={} rename_inflight={} rename_queue={} transfer_active={} edit_bundle={:?}",
                    app.local_adjust_write_handle
                        .as_ref()
                        .is_some_and(|handle| handle.has_unfinished_work()),
                    app.favorite_view_store_writer
                        .as_ref()
                        .is_some_and(|writer| writer.is_busy()),
                    app.tag_maintenance_rx.is_some(),
                    app.metadata_cleanup_pending.is_some(),
                    app.rename_migration_in_flight.is_some(),
                    app.rename_migration_queue.len(),
                    app.metadata_transfer_active_for_sidecar_restore(),
                    (
                        app.edit_bundle_paste_pending.is_some(),
                        app.edit_bundle_apply_pending.is_some(),
                        app.edit_bundle_bulk_pending.is_some()
                    )
                );
            }
            assert!(
                Instant::now() < deadline,
                "headless listing did not settle: bookmark={} local={} content_restore={} spread_busy={} xmp={} tag={} rating={} preview={} resume={}",
                app.book_bookmark_pending_requests.len(),
                app.local_adjust_write_pending.len(),
                app.content_identity_restore_pending.is_some(),
                app.spread_shift_service
                    .as_ref()
                    .is_some_and(|service| service.is_busy()),
                app.rating_xmp_hydration.is_busy(),
                app.tag_write_handle
                    .as_ref()
                    .is_some_and(|handle| handle.has_unconsumed_batch()),
                app.rating_write_handle
                    .as_ref()
                    .is_some_and(|handle| handle.is_busy()),
                app.edit_preview_cache
                    .as_ref()
                    .is_some_and(|cache| cache.has_pending_commands()),
                app.book_resume_writer
                    .as_ref()
                    .is_some_and(|writer| writer.is_busy())
            );
            std::thread::sleep(Duration::from_millis(16));
        }
    }

    fn open(&mut self, app: &mut App, folder: &std::path::Path) {
        self.checkpoint(app, "before-tree-open");
        self.tick(app, Some(folder.to_path_buf()));
        self.settled(app);
        assert_eq!(app.current_folder.as_deref(), Some(folder));
    }
}

fn roundtrip(backup: bool, source_count: usize, zip: bool, details: bool) {
    roundtrip_ordered(backup, source_count, zip, details, false, false);
}

fn roundtrip_ordered(
    backup: bool,
    source_count: usize,
    zip: bool,
    details: bool,
    early: bool,
    fresh_surface: bool,
) {
    roundtrip_at(
        backup,
        source_count,
        zip,
        details,
        early,
        fresh_surface,
        false,
    );
}

fn roundtrip_at(
    backup: bool,
    source_count: usize,
    zip: bool,
    details: bool,
    early: bool,
    fresh_surface: bool,
    middle: bool,
) {
    #[cfg(windows)]
    let _native_input = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    let mut env: AppTestEnv = setup_app();
    // Match the existing Full-frame headless integration fixtures: no startup
    // folder selection, network checks, tray icon or unrelated index/LUT startup.
    env.initialized = true;
    env.settings.update_check_enabled = false;
    env.settings.minimize_to_tray_on_close = false;
    env.indexer_init = IndexerInit::Unavailable;
    env.last_main_focused = false;
    env.settings.creative_luts.clear();
    env.creative_lut_library = crate::creative_lut::CreativeLutLibrary::default();
    let source = fixtures(env.tmp.path(), source_count, zip);
    let other = fixtures(
        env.tmp.path(),
        if source_count == 200 { 100 } else { 200 },
        true,
    );
    env.settings.sidecar_backup_enabled = backup;
    // The middle-position case needs room for the selected row below its two-row
    // anchor; the default four columns show fewer than three complete thumbnail rows.
    // Keep all original reporter/default-size cases unchanged.
    if middle {
        env.settings.grid_cols = 10;
        env.settings.thumb_aspect_auto = false;
    }
    env.settings.folder_tree_pane_visible = true;
    env.settings.grid_view_mode = if details {
        crate::settings::GridViewMode::Details
    } else {
        crate::settings::GridViewMode::Thumbnail
    };
    let mut driver = Driver::new();
    driver.open(&mut env, &source);
    let last = if middle {
        env.items.len() / 2
    } else {
        env.items.len() - 1
    };
    env.move_grid_cursor(Some(last), GridCursorIntent::Automatic);
    env.grid_scroll.set_reveal_requested(true);
    if middle {
        env.grid_scroll.set_row_anchor(Some(2));
    } else {
        env.grid_scroll.request_edge(GridScrollIntent::Bottom);
    }
    driver.settled(&mut env);
    if zip && details {
        assert_eq!(
            env.details_lazy_meta_for_idx(last)
                .and_then(|meta| meta.page_count),
            Some(1),
            "the real ZIP details worker must have read the image-containing archive"
        );
    }
    let saved = env.grid_scroll.offset();
    let selected = env.selected;
    assert!(saved > 0.0);
    driver.checkpoint(&env, "before-leaving-source");
    driver.open(&mut env, &other);
    assert_eq!(env.folder_history.get(&source), Some(&(saved, selected)));
    driver.early_hydration = early;
    driver.fresh_scroll_surface = fresh_surface;
    driver.open(&mut env, &source);
    println!(
        "SCROLL661 RESULT backup={backup} source_count={source_count} zip={zip} details={details} early={early} fresh_surface={fresh_surface}: saved={saved} actual={} selected_before={selected:?} selected_after={:?}",
        env.grid_scroll.offset(),
        env.selected
    );
    assert_eq!(env.selected, selected, "return must preserve the cursor");
    assert_eq!(
        env.grid_scroll.offset(),
        saved,
        "return must preserve the saved scroll position"
    );
}

#[test]
fn zip_200_to_100_backup_off_details() {
    roundtrip(false, 200, true, true);
}
#[test]
fn zip_200_to_100_backup_on_details() {
    roundtrip(true, 200, true, true);
}
#[test]
fn zip_100_to_200_backup_off_details() {
    roundtrip(false, 100, true, true);
}
#[test]
fn jpg_200_to_100_backup_off_details() {
    roundtrip(false, 200, false, true);
}
#[test]
fn zip_200_to_100_backup_off_thumbnail() {
    roundtrip(false, 200, true, false);
}

#[test]
fn zip_200_to_100_fast_hydration_details() {
    roundtrip_ordered(false, 200, true, true, true, false);
}

#[test]
fn zip_200_to_100_fast_hydration_fresh_surface_control() {
    roundtrip_ordered(false, 200, true, true, true, true);
}

fn egui_offset_after_growth(no_spring: bool) -> (f32, f32) {
    let ctx = egui::Context::default();
    if no_spring {
        ctx.all_styles_mut(|style| style.spacing.scroll.kinetic.rubber_band = false);
    }
    let mut actual = 0.0;
    let mut requested = 0.0;
    for frame in 0..5 {
        ctx.run_pass(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(500.0, 500.0))),
            time: Some(frame as f64 / 60.0),
            ..Default::default()
        }, |ctx| {
            ctx.root_ui(|root| {
                egui::CentralPanel::default().show(root, |ui| {
                    let total_h = if frame == 4 { 5600.0 } else { 2800.0 };
                    requested = if frame == 4 { total_h - ui.available_height() } else { 0.0 };
                    let output = egui::ScrollArea::vertical()
                        .id_salt("scroll661-growth")
                        .auto_shrink([false, false])
                        .external_vertical_scroll(egui::scroll_area::ExternalVerticalScroll {
                            content_height: total_h,
                            interaction_id: egui::Id::unique("scroll661-growth-transport"),
                            interaction_session: egui::Id::unique("scroll661-growth-session"),
                            position: egui::scroll_area::ExternalScrollPosition::Set(requested),
                        })
                        .show_viewport(ui, |ui, _| {
                            ui.allocate_exact_size(egui::vec2(300.0, total_h), egui::Sense::hover());
                        });
                    actual = output.state.clamped_offset().y;
                    if frame == 4 {
                        println!("SCROLL661_MINIMAL no_spring={no_spring} requested={requested} actual={actual} content={:?} viewport={:?} state={:?}", output.content_size, output.inner_rect, output.state);
                    }
                });
            });
        }).drop_without_applying_deltas();
    }
    (requested, actual)
}

#[test]
fn egui_explicit_offset_after_content_growth() {
    let (requested, actual) = egui_offset_after_growth(false);
    assert_eq!(
        actual, requested,
        "a programmatic offset inside the new extent must survive"
    );
}

#[test]
fn egui_explicit_offset_without_spring_control() {
    let (requested, actual) = egui_offset_after_growth(true);
    assert_eq!(actual, requested);
}

// RowAnchor acceptance belongs to Stage B: the final painted geometry is its source.
// Use a middle row with ample room on both sides, and change earlier stable keys so
// preserving an old index/absolute offset or merely revealing the row cannot pass.
fn row_anchor_after_list_adoption(route: &str) {
    #[cfg(windows)]
    let _native_input = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for details in [true, false] {
        let mut env = setup_app();
        env.initialized = true;
        env.settings.update_check_enabled = false;
        env.settings.minimize_to_tray_on_close = false;
        env.indexer_init = IndexerInit::Unavailable;
        env.last_main_focused = false;
        env.settings.creative_luts.clear();
        env.creative_lut_library = crate::creative_lut::CreativeLutLibrary::default();
        env.settings.sidecar_backup_enabled = false;
        env.settings.tag_sidecar_backup_enabled = false;
        env.settings.grid_cols = 10;
        env.settings.thumb_aspect_auto = false;
        env.settings.sort_order = crate::settings::SortOrder::FileName;
        env.settings.details_sort_key = crate::settings::DetailsSortKey::Name;
        env.settings.details_sort_ascending = true;
        env.settings.auto_fullscreen_image_folders = false;
        env.settings.grid_view_mode = if details {
            crate::settings::GridViewMode::Details
        } else {
            crate::settings::GridViewMode::Thumbnail
        };
        let source = fixtures(env.tmp.path(), 200, false);
        let mut driver = Driver::new();
        driver.open(&mut env, &source);
        let selected = env
            .items
            .iter()
            .position(|item| item.name() == "0100.jpg")
            .unwrap();
        env.move_grid_cursor(Some(selected), GridCursorIntent::Automatic);
        env.grid_scroll.set_row_anchor(Some(2));
        env.grid_scroll.set_reveal_requested(true);
        driver.settled(&mut env);
        assert_eq!(env.selected_rows_below_view_top(selected), 2);
        assert_eq!(env.grid_scroll.row_anchor(), None);
        let initial_offset = env.grid_scroll.offset();
        let initial_position = env
            .current_grid_order()
            .iter()
            .position(|&index| env.items[index].name() == "0100.jpg")
            .unwrap();
        let image = std::fs::read(source.join("0000.jpg")).unwrap();
        let inserted: Vec<_> = (0..20)
            .map(|i| source.join(format!("0000-before-{i:02}.jpg")))
            .collect();
        for remove in [false, true] {
            // Capture before mutation via the real cursor producer, not a test-only hint.
            let resume = match env.current_list_refresh_intent() {
                StartupListIntent::RefreshList { target, cursor, .. } => {
                    assert_eq!(
                        cursor.as_ref().and_then(|cursor| cursor.rows_above),
                        Some(2)
                    );
                    StartupListIntent::ResumeList { target, cursor }
                }
                _ => unreachable!(),
            };
            for path in &inserted {
                if remove {
                    std::fs::remove_file(path).unwrap();
                } else {
                    std::fs::write(path, &image).unwrap();
                }
            }
            let generation = env.items_generation;
            match route {
                "reload" => {
                    env.reload_current_folder_preserving_override();
                    assert!(
                        env.top_level_grid_view
                            .history_navigation_transition()
                            .is_some()
                    );
                    let deadline = Instant::now() + Duration::from_secs(15);
                    // The usual finish_staged helper also drains hydration. Stop at actual
                    // adoption instead, so this test can inspect that real owner before paint.
                    while env.items_generation == generation {
                        env.poll_open_path_classification(&driver.ctx);
                        env.poll_collection_history_transition(&driver.ctx);
                        assert!(
                            Instant::now() < deadline,
                            "real reload preflight did not adopt"
                        );
                        std::thread::sleep(Duration::from_millis(2));
                    }
                }
                "observation" => {
                    env.start_external_rescan(
                        &driver.ctx,
                        source.clone(),
                        std::time::SystemTime::now(),
                    );
                    assert!(env.external_rescan_pending.is_some());
                    // Hold only the actual worker reply, while full App frames drain
                    // unrelated completion consumers. A tight poll-only loop is not the
                    // application execution model and can strand its other worker owners.
                    let (delivery, held_rx) = std::sync::mpsc::channel();
                    let worker_rx = std::mem::replace(
                        &mut env.external_rescan_pending.as_mut().unwrap().rx,
                        held_rx,
                    );
                    let deadline = Instant::now() + Duration::from_secs(15);
                    let reply = loop {
                        match worker_rx.try_recv() {
                            Ok(reply) => break reply,
                            Err(std::sync::mpsc::TryRecvError::Empty) => {}
                            Err(error) => panic!("real observation worker disconnected: {error}"),
                        }
                        assert!(
                            Instant::now() < deadline,
                            "real observation worker did not finish"
                        );
                        driver.tick(&mut env, None);
                        assert_eq!(
                            env.items_generation, generation,
                            "the held real reply must own the next adoption"
                        );
                        std::thread::sleep(Duration::from_millis(16));
                    };
                    assert!(reply.is_some(), "real observation scan must succeed");
                    assert!(delivery.send(reply).is_ok());
                    env.poll_external_rescan(&driver.ctx);
                }
                "resume" => {
                    assert!(env.load_folder_with_scan_owned(
                        source.clone(),
                        None,
                        OpenRequestOwner::Navigation,
                        resume
                    ));
                }
                _ => unreachable!(),
            }
            assert!(
                env.items_generation != generation,
                "must adopt changed rows"
            );
            // Exercise the production hydration and its selection-priority pass before layout.
            assert!(
                env.sidecar_restore_active(),
                "must use the real hydration owner"
            );
            env.finish_page_identity_preparation_for_test();
            assert!(!env.sidecar_restore_active());
            let selected = env.selected.unwrap();
            assert_eq!(env.items[selected].name(), "0100.jpg");
            assert_eq!(env.grid_scroll.row_anchor(), Some(2));
            // Item slots can remain stable while details display order changes.
            // Verify the semantic visible position rather than assuming physical storage order.
            let position = env
                .current_grid_order()
                .iter()
                .position(|&index| index == selected)
                .unwrap();
            assert_eq!(
                position,
                if remove {
                    initial_position
                } else {
                    initial_position + inserted.len()
                },
                "new stable keys must change the selected display position before row restoration"
            );
            driver.settled(&mut env);
            assert_eq!(env.items[env.selected.unwrap()].name(), "0100.jpg");
            assert_eq!(
                env.selected_rows_below_view_top(env.selected.unwrap()),
                2,
                "route={route} details={details} remove={remove}"
            );
            assert_eq!(env.grid_scroll.row_anchor(), None);
            if remove {
                assert_eq!(env.grid_scroll.offset(), initial_offset);
            } else {
                assert_ne!(env.grid_scroll.offset(), initial_offset);
            }
        }
    }
}

#[test]
fn row_anchor_reload_keeps_middle_selected_row_in_both_renderers() {
    row_anchor_after_list_adoption("reload");
}

#[test]
fn row_anchor_observation_keeps_middle_selected_row_in_both_renderers() {
    row_anchor_after_list_adoption("observation");
}

#[test]
fn row_anchor_resume_keeps_middle_selected_row_in_both_renderers() {
    row_anchor_after_list_adoption("resume");
}

#[test]
fn zip_200_to_100_fast_hydration_thumbnail() {
    roundtrip_ordered(false, 200, true, false, true, false);
}

#[test]
fn zip_middle_position_survives_both_hydration_orders_and_renderers() {
    for details in [true, false] {
        for early in [true, false] {
            roundtrip_at(false, 200, true, details, early, false, true);
        }
    }
}

// The App resolver must carry the meaning of an already-clamped offset through
// the real ScrollArea boundary; testing Set(560) alone would miss Sync(560).
#[test]
fn app_geometry_clamp_stops_real_native_coast_and_target_before_paint() {
    use crate::app::grid_scroll::{GridScrollGeometry, GridScrollState};
    use egui::scroll_area::{ExternalScrollPosition, ExternalVerticalScroll};
    for (coast, target) in [(true, false), (false, true), (true, true)] {
        for grow_viewport in [false, true] {
            for preserve_clamp_meaning in [true, false] {
                let ctx = egui::Context::default();
                let seed_extent = if grow_viewport { 840.0 } else { 1120.0 };
                let mut canonical = GridScrollState::default();
                canonical.adopt_offset(840.0);
                let mut output = None;
                let mut drawn_viewport = egui::Rect::NOTHING;
                let mut drawn_origin = egui::Pos2::ZERO;
                let mut sequence = 0;
                let mut draw =
                    |height: f32, extent: f32, position, events, request_target: bool| {
                        sequence += 1;
                        ctx.run_pass(
                            egui::RawInput {
                                screen_rect: Some(egui::Rect::from_min_size(
                                    egui::Pos2::ZERO,
                                    egui::vec2(500.0, height),
                                )),
                                time: Some(sequence as f64 / 60.0),
                                events,
                                ..Default::default()
                            },
                            |ctx| {
                                ctx.root_ui(|root| {
                                    egui::CentralPanel::default().show(root, |ui| {
                                        let content_h = ui.available_height() + extent;
                                        output = Some(
                                            egui::ScrollArea::vertical()
                                                .scroll_source(egui::scroll_area::ScrollSource {
                                                    drag: egui::scroll_area::DragScroll::Always,
                                                    ..Default::default()
                                                })
                                                .id_salt("scroll661-app-clamp")
                                                .auto_shrink([false, false])
                                                .content_margin(egui::Margin::ZERO)
                                                .external_vertical_scroll(ExternalVerticalScroll {
                                                    content_height: content_h,
                                                    interaction_id: egui::Id::unique(
                                                        "scroll661-clamp-transport",
                                                    ),
                                                    interaction_session: egui::Id::unique(
                                                        "scroll661-clamp-session",
                                                    ),
                                                    position,
                                                })
                                                .show_viewport(ui, |ui, viewport| {
                                                    drawn_viewport = viewport;
                                                    drawn_origin = ui.max_rect().min;
                                                    if request_target {
                                                        ui.scroll_to_rect_animation(
                                                            egui::Rect::from_min_size(
                                                                ui.max_rect().min
                                                                    + egui::vec2(0.0, 200.0),
                                                                egui::vec2(10.0, 28.0),
                                                            ),
                                                            Some(egui::Align::Min),
                                                            egui::style::ScrollAnimation::duration(
                                                                1.0,
                                                            ),
                                                        );
                                                    }
                                                    ui.allocate_exact_size(
                                                        egui::vec2(300.0, content_h),
                                                        egui::Sense::hover(),
                                                    );
                                                }),
                                        );
                                    });
                                });
                            },
                        )
                        .drop_without_applying_deltas();
                        (
                            output.as_ref().unwrap().state,
                            output.as_ref().unwrap().inner_rect,
                            drawn_viewport,
                            drawn_origin,
                        )
                    };
                let button = |pressed, y| egui::Event::PointerButton {
                    pos: egui::pos2(100.0, y),
                    button: egui::PointerButton::Primary,
                    pressed,
                    modifiers: egui::Modifiers::NONE,
                };
                draw(
                    400.0,
                    seed_extent,
                    ExternalScrollPosition::Set(840.0),
                    vec![],
                    false,
                );
                draw(
                    400.0,
                    seed_extent,
                    ExternalScrollPosition::Sync(840.0),
                    vec![],
                    false,
                );
                if coast {
                    draw(
                        400.0,
                        seed_extent,
                        ExternalScrollPosition::Sync(840.0),
                        vec![
                            egui::Event::PointerMoved(egui::pos2(100.0, 220.0)),
                            button(true, 220.0),
                        ],
                        false,
                    );
                    draw(
                        400.0,
                        seed_extent,
                        ExternalScrollPosition::Sync(840.0),
                        vec![egui::Event::PointerMoved(egui::pos2(100.0, 140.0))],
                        false,
                    );
                }
                let seeded = draw(
                    400.0,
                    seed_extent,
                    ExternalScrollPosition::Sync(840.0),
                    if coast {
                        vec![button(false, 140.0)]
                    } else {
                        vec![]
                    },
                    target,
                )
                .0;
                if coast {
                    assert_ne!(seeded.velocity().y, 0.0, "must seed a real release coast");
                }
                if target {
                    assert!(
                        format!("{seeded:?}").contains("Some(ScrollingToTarget"),
                        "must seed a real native target: {seeded:?}"
                    );
                }
                let viewport_h = if grow_viewport { 664.0 } else { 384.0 };
                let geometry = GridScrollGeometry::new(1, 28.0, viewport_h, viewport_h + 560.0);
                let resolved = canonical.resolve_layout(geometry, false, None, false);
                assert!(matches!(
                    resolved.position,
                    ExternalScrollPosition::Set(560.0)
                ));
                let delivered = if preserve_clamp_meaning {
                    resolved.position
                } else {
                    ExternalScrollPosition::Sync(560.0)
                };
                let (state, inner, viewport, origin) =
                    draw(viewport_h + 16.0, 560.0, delivered, vec![], false);
                if !preserve_clamp_meaning {
                    assert!(
                        state.velocity().y != 0.0
                            || state.unclamped_offset().y != 560.0
                            || format!("{state:?}").contains("Some(ScrollingToTarget"),
                        "the Sync mutation must retain old motion/target and fail the Set contract"
                    );
                    continue;
                }
                assert_eq!(canonical.offset(), 560.0);
                assert_eq!(state.clamped_offset().y, 560.0);
                assert_eq!(state.velocity().y, 0.0);
                assert_eq!(viewport.min.y, 560.0);
                assert!((inner.min.y - origin.y - 560.0).abs() < 0.01);
                assert!(!format!("{state:?}").contains("Some(ScrollingToTarget"));
                let next = draw(
                    viewport_h + 16.0,
                    560.0,
                    ExternalScrollPosition::Sync(560.0),
                    vec![],
                    false,
                )
                .0;
                assert_eq!(next.clamped_offset().y, 560.0);
            }
        }
    }
}

// Input lifetime cases keep optional metadata chrome from moving the receiver
// between hit testing and the first destination layout. Default-column async
// schedules are covered independently by all original reporter cases above.
fn configure_pointer_fixture_columns(app: &mut App) {
    if app.settings.grid_view_mode == crate::settings::GridViewMode::Details {
        app.settings.details_show_page_count = false;
        app.settings.details_show_created = false;
        app.settings.details_show_image_dimensions = false;
        app.settings.details_show_video_duration = false;
        app.settings.details_show_video_dimensions = false;
        app.settings.details_show_video_codec = false;
        app.invalidate_details_meta_requirements();
    }
}

fn input_fixture(details: bool) -> (AppTestEnv, Driver, PathBuf) {
    let mut env = setup_app();
    env.initialized = true;
    env.settings.update_check_enabled = false;
    env.settings.minimize_to_tray_on_close = false;
    env.indexer_init = IndexerInit::Unavailable;
    env.last_main_focused = false;
    env.settings.creative_luts.clear();
    env.creative_lut_library = crate::creative_lut::CreativeLutLibrary::default();
    env.settings.sidecar_backup_enabled = false;
    env.settings.tag_sidecar_backup_enabled = false;
    env.settings.grid_cols = 10;
    env.settings.thumb_aspect_auto = false;
    env.settings.auto_fullscreen_image_folders = false;
    env.settings.grid_view_mode = if details {
        crate::settings::GridViewMode::Details
    } else {
        crate::settings::GridViewMode::Thumbnail
    };
    let source = fixtures(env.tmp.path(), 200, false);
    let other = fixtures(env.tmp.path(), 100, false);
    let mut driver = Driver::new();
    driver.open(&mut env, &source);
    configure_pointer_fixture_columns(&mut env);
    env.selected = None;
    env.grid_scroll.request_edge(GridScrollIntent::Bottom);
    driver.settled(&mut env);
    (env, driver, other)
}

fn pointer_button(pos: egui::Pos2, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: egui::PointerButton::Primary,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

fn adopt_input_destination_before_first_layout(env: &mut App, destination: &std::path::Path) {
    let previous = env.grid_scroll.session();
    // Real navigation, preparation and hydration, with no destination layout yet.
    // The previous pass's stable transport widgets still own egui hit testing.
    env.load_folder(destination.to_path_buf());
    super::phase_c_folder_nav_history_tests::finish_staged_physical_history_for_test(env);
    env.finish_page_identity_preparation_for_test();
    configure_pointer_fixture_columns(env);
    assert_eq!(env.current_folder.as_deref(), Some(destination));
    assert_ne!(env.grid_scroll.session(), previous);
}

#[derive(Clone, Copy, Debug)]
enum PointerAdmissionCase {
    NewPress,
    OldHeld,
    ReleaseThenPress,
}

fn pointer_admission_in_actual_renderers(case: PointerAdmissionCase) {
    #[cfg(windows)]
    let _native_input = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for details in [true, false] {
        for bar in [false, true] {
            let (mut env, mut driver, destination) = input_fixture(details);
            let transport = App::grid_scroll_transport_id(&driver.ctx);
            let widget = if bar {
                transport.with(1_usize)
            } else {
                transport.with("area")
            };
            let rect = driver
                .ctx
                .read_response(widget)
                .expect("the actual renderer must register its stable transport")
                .rect;
            let point = if bar {
                rect.center()
            } else {
                // A snapped bottom extent leaves a fractional blank tail. This is a
                // genuine background target, outside thumbnail/file-D&D row widgets.
                let geometry = env.grid_scroll.geometry();
                let rows = env.current_grid_order().len().div_ceil(geometry.cols);
                let blank_tail = geometry.content_h - rows as f32 * geometry.row_h;
                assert!(
                    blank_tail > 1.0,
                    "fixture needs real blank background: {geometry:?}"
                );
                egui::pos2(rect.center().x, rect.bottom() - blank_tail * 0.5)
            };
            let old_move = point - egui::vec2(0.0, 12.0);
            if !matches!(case, PointerAdmissionCase::NewPress) {
                driver.tick_events(
                    &mut env,
                    None,
                    vec![
                        egui::Event::PointerMoved(point),
                        pointer_button(point, true),
                    ],
                );
                driver.tick_events(&mut env, None, vec![egui::Event::PointerMoved(old_move)]);
                assert_eq!(
                    driver.ctx.dragged_id(),
                    Some(widget),
                    "must establish a real old drag, details={details} bar={bar}"
                );
            }
            adopt_input_destination_before_first_layout(&mut env, &destination);
            let restored = env.grid_scroll.offset();
            match case {
                PointerAdmissionCase::NewPress => driver.tick_events(
                    &mut env,
                    None,
                    vec![
                        egui::Event::PointerMoved(point),
                        pointer_button(point, true),
                    ],
                ),
                PointerAdmissionCase::ReleaseThenPress => driver.tick_events(
                    &mut env,
                    None,
                    vec![
                        pointer_button(old_move, false),
                        egui::Event::PointerMoved(point),
                        pointer_button(point, true),
                    ],
                ),
                PointerAdmissionCase::OldHeld => driver.tick_events(
                    &mut env,
                    None,
                    vec![egui::Event::PointerMoved(point - egui::vec2(0.0, 180.0))],
                ),
            }
            assert!(
                driver
                    .ctx
                    .read_response(widget)
                    .unwrap()
                    .interact_rect
                    .contains(point),
                "new-press target must remain inside the destination's actual clip"
            );
            if matches!(case, PointerAdmissionCase::OldHeld) {
                assert_eq!(
                    env.grid_scroll.offset(),
                    restored,
                    "old capture must not move the destination, details={details} bar={bar}"
                );
                driver.tick_events(
                    &mut env,
                    None,
                    vec![pointer_button(point - egui::vec2(0.0, 180.0), false)],
                );
                for _ in 0..3 {
                    driver.tick(&mut env, None);
                }
                assert_eq!(
                    env.grid_scroll.offset(),
                    restored,
                    "old release must not start destination coast"
                );
            } else {
                let after_press = env.grid_scroll.offset();
                if !bar {
                    assert_eq!(
                        after_press, restored,
                        "pre-press frame delta must not enter the new drag"
                    );
                }
                let moved = point - egui::vec2(0.0, 210.0);
                driver.tick_events(&mut env, None, vec![egui::Event::PointerMoved(moved)]);
                assert_eq!(driver.ctx.dragged_id(), Some(widget));
                assert_ne!(
                    env.grid_scroll.offset(),
                    after_press,
                    "new sequence must move without another press, details={details} bar={bar} case={case:?}"
                );
                driver.tick_events(&mut env, None, vec![pointer_button(moved, false)]);
            }
            assert_eq!(env.current_folder.as_deref(), Some(destination.as_path()));
        }
    }
}

#[test]
fn new_press_on_switch_frame_operates_both_actual_renderers_without_repress() {
    pointer_admission_in_actual_renderers(PointerAdmissionCase::NewPress);
}
#[test]
fn held_capture_from_old_list_cannot_move_or_coast_either_destination_renderer() {
    pointer_admission_in_actual_renderers(PointerAdmissionCase::OldHeld);
}
#[test]
fn old_background_or_bar_drag_release_then_new_press_on_switch_frame_needs_no_repress() {
    pointer_admission_in_actual_renderers(PointerAdmissionCase::ReleaseThenPress);
}

fn grid_fraction(app: &App, ctx: &egui::Context) -> f32 {
    crate::ui_main::grid_touch_scroll_remainder(
        ctx,
        app.projected_viewer_context_id(),
        app.grid_scroll_scope(),
        app.grid_scroll.offset(),
        app.grid_scroll.geometry().row_h,
        app.items_generation,
    )
}

fn raw_touch(id: u64, phase: egui::TouchPhase, pos: egui::Pos2) -> egui::Event {
    egui::Event::Touch {
        device_id: egui::TouchDeviceId(1),
        id: egui::TouchId(id),
        phase,
        pos,
        force: None,
    }
}

#[test]
fn real_grid_touch_old_tail_is_retired_and_new_contact_works_while_old_is_held() {
    #[cfg(windows)]
    let _native_input = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for no_draw_roundtrip in [false, true] {
        let (mut env, mut driver, destination) = input_fixture(false);
        let source = env.current_folder.clone().unwrap();
        let row_h = env.grid_scroll.geometry().row_h;
        env.grid_scroll.set_position(row_h * 2.0);
        driver.tick(&mut env, None);
        let start = egui::pos2(700.0, 500.0);
        let moved = start - egui::vec2(0.0, 50.0);
        driver.tick_events(
            &mut env,
            None,
            vec![
                raw_touch(1, egui::TouchPhase::Start, start),
                egui::Event::PointerMoved(start),
                pointer_button(start, true),
            ],
        );
        driver.tick_events(
            &mut env,
            None,
            vec![
                raw_touch(1, egui::TouchPhase::Move, moved),
                egui::Event::PointerMoved(moved),
            ],
        );
        assert!(
            grid_fraction(&env, &driver.ctx) > 0.0,
            "must establish a real fractional gesture"
        );
        let old_scope = env.grid_scroll_scope();
        adopt_input_destination_before_first_layout(&mut env, &destination);
        if no_draw_roundtrip {
            adopt_input_destination_before_first_layout(&mut env, &source);
        }
        assert_ne!(env.grid_scroll_scope(), old_scope);
        let restored = env.grid_scroll.offset();
        let old_tail = moved - egui::vec2(0.0, 40.0);
        driver.tick_events(
            &mut env,
            None,
            vec![
                raw_touch(1, egui::TouchPhase::Move, old_tail),
                egui::Event::PointerMoved(old_tail),
            ],
        );
        assert_eq!(env.grid_scroll.offset(), restored);
        assert_eq!(grid_fraction(&env, &driver.ctx), 0.0);
        let cols = env.settings.grid_cols;
        // egui-winit's pointer gate is still occupied by contact 1, so these
        // current contact samples intentionally have no synthetic pointer tail.
        driver.tick_events(
            &mut env,
            None,
            vec![raw_touch(2, egui::TouchPhase::Start, start)],
        );
        let new_move = start - egui::vec2(0.0, 110.0);
        driver.tick_events(
            &mut env,
            None,
            vec![raw_touch(2, egui::TouchPhase::Move, new_move)],
        );
        let before_cancel = env.grid_scroll.offset() + grid_fraction(&env, &driver.ctx);
        assert!(
            before_cancel > restored,
            "new raw contact must operate without releasing old contact"
        );
        driver.tick_events(
            &mut env,
            None,
            vec![
                raw_touch(1, egui::TouchPhase::Cancel, old_tail),
                egui::Event::PointerGone,
            ],
        );
        assert_eq!(
            env.grid_scroll.offset() + grid_fraction(&env, &driver.ctx),
            before_cancel,
            "retired Cancel must not settle or cancel the current contact"
        );
        let next_move = new_move - egui::vec2(0.0, 50.0);
        driver.tick_events(
            &mut env,
            None,
            vec![
                raw_touch(2, egui::TouchPhase::Move, next_move),
                egui::Event::PointerMoved(next_move),
            ],
        );
        assert!(env.grid_scroll.offset() + grid_fraction(&env, &driver.ctx) > before_cancel);
        assert_eq!(
            env.settings.grid_cols, cols,
            "old contact must not join a new pinch"
        );
        driver.tick_events(
            &mut env,
            None,
            vec![
                raw_touch(2, egui::TouchPhase::End, next_move),
                pointer_button(next_move, false),
                egui::Event::PointerGone,
            ],
        );
        assert_eq!(
            env.current_folder.as_ref(),
            Some(if no_draw_roundtrip {
                &source
            } else {
                &destination
            })
        );
    }
}

struct RetryingGridApp<'a> {
    app: &'a mut App,
    renew_after_first_pass: bool,
    observations: Vec<(u64, u64, egui::Id, f32)>,
}

impl eframe::App for RetryingGridApp<'_> {
    fn raw_input_hook(&mut self, ctx: &egui::Context, input: &mut egui::RawInput) {
        eframe::App::raw_input_hook(self.app, ctx, input);
    }
    fn logic(&mut self, ctx: &egui::Context, frame: &mut eframe::Frame) {
        eframe::App::logic(self.app, ctx, frame);
    }
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
        eframe::App::ui(self.app, ui, frame);
        let ctx = ui.ctx();
        self.observations.push((
            self.app.frame_counter,
            ctx.cumulative_frame_nr(),
            self.app.grid_scroll_scope(),
            self.app.grid_scroll.offset() + grid_fraction(self.app, ctx),
        ));
        if ctx.current_pass_index() == 0 {
            if self.renew_after_first_pass {
                self.app.grid_scroll.renew_session();
            }
            ctx.request_discard("scroll661 actual App touch replay");
        }
    }
}

#[test]
fn actual_app_discard_uses_viewport_frame_and_does_not_replay_touch_into_new_scope() {
    #[cfg(windows)]
    let _native_input = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for renew in [false, true] {
        let (mut env, mut driver, _) = input_fixture(false);
        let row_h = env.grid_scroll.geometry().row_h;
        let initial = row_h * 2.0;
        env.grid_scroll.set_position(initial);
        driver.tick(&mut env, None);
        let start = egui::pos2(700.0, 500.0);
        let moved = start - egui::vec2(0.0, 50.0);
        let before_frame = driver.ctx.cumulative_frame_nr();
        let mut retry = RetryingGridApp {
            app: &mut env,
            renew_after_first_pass: renew,
            observations: Vec::new(),
        };
        let mut input = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            focused: false,
            events: vec![
                raw_touch(77, egui::TouchPhase::Start, start),
                egui::Event::PointerMoved(start),
                pointer_button(start, true),
                raw_touch(77, egui::TouchPhase::Move, moved),
                egui::Event::PointerMoved(moved),
            ],
            ..Default::default()
        };
        input
            .viewports
            .get_mut(&egui::ViewportId::ROOT)
            .unwrap()
            .focused = Some(false);
        let output = driver.backend.update(&mut retry, input);
        output.drop_without_applying_deltas();
        assert_eq!(
            retry.observations.len(),
            2,
            "must run real App UI twice through egui discard"
        );
        let first = retry.observations[0];
        let second = retry.observations[1];
        assert!(
            second.0 > first.0,
            "App update counter must advance across passes"
        );
        assert_eq!(first.1, before_frame);
        assert_eq!(
            second.1, before_frame,
            "the viewport frame must remain constant"
        );
        assert!(
            (first.3 - (initial + 50.0)).abs() < 0.1,
            "the first real command must be applied once: {first:?}"
        );
        if renew {
            assert_ne!(first.2, second.2);
            assert_eq!(
                second.3, initial,
                "old raw Start/Move must not be recognized again in a fresh scope on the retry"
            );
        } else {
            assert_eq!(first.2, second.2);
            assert_eq!(
                second.3, first.3,
                "same-scope replay must not apply commands twice"
            );
        }
        drop(retry);
        assert_eq!(driver.ctx.cumulative_frame_nr(), before_frame + 1);
        if renew {
            // Next viewport frame accepts a genuinely new contact even though the
            // old synthetic pointer contact still occupies egui-winit's gate.
            driver.tick_events(
                &mut env,
                None,
                vec![
                    raw_touch(78, egui::TouchPhase::Start, start),
                    raw_touch(78, egui::TouchPhase::Move, moved),
                ],
            );
            assert!(env.grid_scroll.offset() + grid_fraction(&env, &driver.ctx) > initial);
        }
    }
}

fn assert_grid_paint_has_no_native_pan(app: &App, ctx: &egui::Context) {
    let witness =
        crate::ui_main::grid_scroll_render_witness(ctx, app.grid_scroll_context_id()).unwrap();
    let expected = app.grid_scroll.offset() + grid_fraction(app, ctx);
    assert_eq!(
        witness.viewport_y, expected,
        "viewport must belong to App's gesture: {witness:?}"
    );
    assert_eq!(witness.state.unclamped_offset().y, expected);
    assert_eq!(witness.state.velocity().y, 0.0);
    let expected_top = egui::pos2(0.0, witness.inner_rect.top() - expected)
        .round_to_pixels(ctx.pixels_per_point())
        .round_ui()
        .y;
    assert!(
        (witness.content_top - expected_top).abs() < 0.01,
        "content must use the same rounded origin: {witness:?}"
    );
}

#[test]
fn real_background_pinch_has_no_native_pan_and_old_zoom_end_cannot_cross_folder_adoption() {
    #[cfg(windows)]
    let _native_input = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    let (mut env, mut driver, destination) = input_fixture(false);
    let transport = App::grid_scroll_transport_id(&driver.ctx);
    let background = transport.with("area");
    let rect = driver.ctx.read_response(background).unwrap().rect;
    let geometry = env.grid_scroll.geometry();
    let blank = geometry.content_h
        - env.current_grid_order().len().div_ceil(geometry.cols) as f32 * geometry.row_h;
    assert!(blank > 1.0);
    let first = egui::pos2(rect.center().x, rect.bottom() - blank * 0.5);
    let second = first + egui::vec2(80.0, 0.0);
    driver.tick_events(
        &mut env,
        None,
        vec![
            raw_touch(1, egui::TouchPhase::Start, first),
            egui::Event::PointerMoved(first),
            pointer_button(first, true),
        ],
    );
    driver.tick_events(
        &mut env,
        None,
        vec![raw_touch(2, egui::TouchPhase::Start, second)],
    );
    let before = env.grid_scroll.offset();
    let cols = env.settings.grid_cols;
    let translated_first = first - egui::vec2(0.0, 20.0);
    let translated_second = second - egui::vec2(0.0, 20.0);
    driver.tick_events(
        &mut env,
        None,
        vec![
            raw_touch(1, egui::TouchPhase::Move, translated_first),
            egui::Event::PointerMoved(translated_first),
            raw_touch(2, egui::TouchPhase::Move, translated_second),
        ],
    );
    assert_eq!(
        driver.ctx.dragged_id(),
        Some(background),
        "must exercise a real background capture under Pinch"
    );
    assert_eq!(
        env.settings.grid_cols, cols,
        "centroid translation must not zoom"
    );
    assert_eq!(env.grid_scroll.offset(), before);
    assert_eq!(grid_fraction(&env, &driver.ctx), 0.0);
    assert!(
        crate::ui_main::grid_scroll_render_witness(&driver.ctx, env.grid_scroll_context_id())
            .unwrap()
            .pinch_owns_scroll
    );
    assert_grid_paint_has_no_native_pan(&env, &driver.ctx);
    adopt_input_destination_before_first_layout(&mut env, &destination);
    let restored = env.grid_scroll.offset();
    let far_second = translated_second + egui::vec2(100.0, 0.0);
    driver.tick_events(
        &mut env,
        None,
        vec![
            raw_touch(1, egui::TouchPhase::Move, translated_first),
            egui::Event::PointerMoved(translated_first),
            raw_touch(2, egui::TouchPhase::Move, far_second),
        ],
    );
    assert_eq!(
        env.settings.grid_cols, cols,
        "retired pinch must not generate Zoom in the destination"
    );
    assert_eq!(env.grid_scroll.offset(), restored);
    assert_grid_paint_has_no_native_pan(&env, &driver.ctx);
    driver.tick_events(
        &mut env,
        None,
        vec![
            raw_touch(1, egui::TouchPhase::End, translated_first),
            pointer_button(translated_first, false),
            egui::Event::PointerGone,
            raw_touch(2, egui::TouchPhase::End, far_second),
            egui::Event::PointerGone,
        ],
    );
    for _ in 0..3 {
        driver.tick(&mut env, None);
    }
    assert_eq!(env.settings.grid_cols, cols);
    assert_eq!(
        env.grid_scroll.offset(),
        restored,
        "old PinchEnd/release must not start native coast"
    );
    assert_grid_paint_has_no_native_pan(&env, &driver.ctx);
    // A genuinely new pinch is still functional after retiring the old one.
    driver.tick_events(
        &mut env,
        None,
        vec![
            raw_touch(3, egui::TouchPhase::Start, first),
            egui::Event::PointerMoved(first),
            pointer_button(first, true),
            raw_touch(4, egui::TouchPhase::Start, second),
        ],
    );
    let expanded = second + egui::vec2(100.0, 0.0);
    driver.tick_events(
        &mut env,
        None,
        vec![raw_touch(4, egui::TouchPhase::Move, expanded)],
    );
    assert_ne!(
        env.settings.grid_cols, cols,
        "fresh pinch must still change columns"
    );
    driver.tick_events(
        &mut env,
        None,
        vec![
            raw_touch(3, egui::TouchPhase::End, first),
            pointer_button(first, false),
            egui::Event::PointerGone,
            raw_touch(4, egui::TouchPhase::Move, expanded),
            egui::Event::PointerMoved(expanded),
            raw_touch(4, egui::TouchPhase::End, expanded),
            pointer_button(expanded, false),
            egui::Event::PointerGone,
        ],
    );
}

#[test]
#[ignore = "known pre-existing: §1.487 (v4.10.0 native touch/mouse pointer mixing)"]
fn retired_secondary_touch_move_cannot_drive_a_new_mouse_capture() {
    #[cfg(windows)]
    let _native_input = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    let mut mouse_only_delta = None;
    for include_retired_tail in [false, true] {
        let (mut env, mut driver, destination) = input_fixture(false);
        let background = App::grid_scroll_transport_id(&driver.ctx).with("area");
        let first = egui::pos2(600.0, 500.0);
        let second = first + egui::vec2(80.0, 0.0);
        // The backend gives the synthetic pointer to contact 1; contact 2 is raw only.
        driver.tick_events(
            &mut env,
            None,
            vec![
                raw_touch(1, egui::TouchPhase::Start, first),
                egui::Event::PointerMoved(first),
                pointer_button(first, true),
                raw_touch(2, egui::TouchPhase::Start, second),
            ],
        );
        adopt_input_destination_before_first_layout(&mut env, &destination);
        // Ending the old primary frees egui-winit's gate. Contact 2 remains down.
        // No intervening cursor movement: the backend release uses pointer_pos=first.
        driver.tick_events(
            &mut env,
            None,
            vec![
                raw_touch(1, egui::TouchPhase::End, first),
                pointer_button(first, false),
                egui::Event::PointerGone,
            ],
        );
        env.grid_scroll.request_edge(GridScrollIntent::Bottom);
        driver.tick(&mut env, None);
        let rect = driver.ctx.read_response(background).unwrap().interact_rect;
        let geometry = env.grid_scroll.geometry();
        let blank = geometry.content_h
            - env.current_grid_order().len().div_ceil(geometry.cols) as f32 * geometry.row_h;
        assert!(blank > 1.0);
        let mouse = egui::pos2(rect.center().x, rect.bottom() - blank * 0.5);
        driver.tick_events(
            &mut env,
            None,
            vec![
                egui::Event::PointerMoved(mouse),
                pointer_button(mouse, true),
            ],
        );
        assert_eq!(
            driver.ctx.dragged_id(),
            Some(background),
            "must establish a genuine new mouse capture"
        );
        let scope = env.grid_scroll_scope();
        let before =
            crate::ui_main::grid_scroll_render_witness(&driver.ctx, env.grid_scroll_context_id())
                .unwrap();
        assert!(!before.touch_owns_native_y);
        assert_eq!(before.viewport_y, env.grid_scroll.offset());
        // With pointer_touch_id=None, the actual egui-winit gate now emits a
        // synthetic PointerMoved even for this still-held retired secondary id.
        let old_move = mouse - egui::vec2(0.0, 80.0);
        driver.tick_events(
            &mut env,
            None,
            if include_retired_tail {
                vec![
                    raw_touch(2, egui::TouchPhase::Move, old_move),
                    egui::Event::PointerMoved(old_move),
                ]
            } else {
                vec![]
            },
        );
        let after =
            crate::ui_main::grid_scroll_render_witness(&driver.ctx, env.grid_scroll_context_id())
                .unwrap();
        assert_eq!(driver.ctx.dragged_id(), Some(background));
        assert_eq!(
            env.grid_scroll_scope(),
            scope,
            "no new list/input admission occurred"
        );
        assert!(
            !after.touch_owns_native_y,
            "retired transport cannot claim current App contact ownership"
        );
        assert_eq!(
            after.viewport_y, before.viewport_y,
            "retired touch must not become the new mouse capture's native Y delta: before={before:?} after={after:?}"
        );
        assert_eq!(
            after.state.unclamped_offset().y,
            before.state.unclamped_offset().y
        );
        // The genuine mouse sequence must remain usable without another press.
        let moved_mouse = mouse + egui::vec2(0.0, 100.0);
        driver.tick_events(&mut env, None, vec![egui::Event::PointerMoved(moved_mouse)]);
        let resumed =
            crate::ui_main::grid_scroll_render_witness(&driver.ctx, env.grid_scroll_context_id())
                .unwrap();
        assert!(resumed.viewport_y < before.viewport_y);
        let delta = resumed.viewport_y - before.viewport_y;
        if let Some(expected_delta) = mouse_only_delta {
            assert_eq!(
                delta, expected_delta,
                "retiring touch must not pollute the real mouse sequence's previous position"
            );
        } else {
            mouse_only_delta = Some(delta);
        }
        driver.tick_events(&mut env, None, vec![pointer_button(moved_mouse, false)]);
    }
}

fn cell_input_response(app: &App, ctx: &egui::Context, idx: usize) -> egui::Response {
    ctx.data(|data| {
        data.get_temp(egui::Id::unique((
            "grid_cell_input_response",
            app.grid_scroll_context_id(),
            idx,
        )))
    })
    .expect("the actual renderer must register the visible cell")
}

fn retired_cell_tap_after_adoption(details: bool, new_press_on_release: bool) {
    #[cfg(windows)]
    let _native_input = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for quick_slot in [true, false] {
        for mode in [
            crate::settings::GridClickSelectionMode::Explorer,
            crate::settings::GridClickSelectionMode::Check,
        ] {
            let (mut env, mut driver, folder_destination) = input_fixture(details);
            let destination = if quick_slot {
                let source = env.current_folder.clone().unwrap();
                env.set_quick_folder_slot_target(QuickFolderSlotId::A, source.clone());
                env.set_quick_folder_slot_target(QuickFolderSlotId::B, source.clone());
                env.active_quick_folder_slot = Some(QuickFolderSlotId::A);
                source
            } else {
                folder_destination
            };
            env.settings.grid_click_selection_mode = mode;
            env.grid_scroll.request_edge(GridScrollIntent::Top);
            driver.tick(&mut env, None);
            let response = cell_input_response(&env, &driver.ctx, 1);
            let point = response.interact_rect.center();
            driver.tick_events(
                &mut env,
                None,
                vec![
                    raw_touch(1, egui::TouchPhase::Start, point),
                    egui::Event::PointerMoved(point),
                    pointer_button(point, true),
                ],
            );
            let held = cell_input_response(&env, &driver.ctx, 1);
            assert_eq!(held.id, response.id);
            assert!(
                held.is_pointer_button_down_on(),
                "the old cell must actually own the touch press"
            );
            assert!(
                driver
                    .ctx
                    .input(|input| input.pointer.could_any_button_be_click()),
                "the old press must still be a tap candidate: {held:?}"
            );
            assert!(!env.native_drag_just_finished);
            if quick_slot {
                let scope = env.grid_scroll_scope();
                assert!(matches!(
                    env.activate_quick_folder_slot(QuickFolderSlotId::B),
                    QuickFolderSwitchTarget::Current
                ));
                assert_ne!(env.grid_scroll_scope(), scope);
            } else {
                adopt_input_destination_before_first_layout(&mut env, &destination);
            }
            // A destination may already have checks. The retired release may
            // neither select its row nor clear this new list's checked set.
            env.checked.insert(4);
            let selected = env.selected;
            let checked = env.checked.clone();
            let generation = env.items_generation;
            let next_press = point + egui::vec2(1.0, 0.0);
            let mut events = vec![
                raw_touch(1, egui::TouchPhase::End, point),
                pointer_button(point, false),
                egui::Event::PointerGone,
            ];
            if new_press_on_release {
                events.extend([
                    egui::Event::PointerMoved(next_press),
                    pointer_button(next_press, true),
                ]);
            }
            driver.tick_events(&mut env, None, events);
            let released = cell_input_response(&env, &driver.ctx, 1);
            assert_eq!(
                released.id, response.id,
                "exercise the same egui cell ID across list admission"
            );
            if quick_slot {
                assert!(
                    released.clicked_by(egui::PointerButton::Primary),
                    "egui must produce the old captured click before mIV rejects it: response={released:?}, clicks={:?}, interaction={:?}, native_drag={}, modal={}",
                    driver.ctx.input(|input| input
                        .pointer
                        .released_buttons_with_click_counts()
                        .collect::<Vec<_>>()),
                    driver.ctx.interaction_snapshot(|snapshot| (
                        snapshot.clicked,
                        snapshot.dragged,
                        snapshot.drag_started,
                        snapshot.drag_stopped
                    )),
                    env.native_drag_just_finished,
                    env.sidecar_restore_active()
                );
            }
            assert_eq!(
                env.selected, selected,
                "a retired release must not select a destination row"
            );
            assert_eq!(
                env.checked, checked,
                "a retired release must not toggle destination checks"
            );
            assert_eq!(env.items_generation, generation);
            assert_eq!(env.current_folder.as_ref(), Some(&destination));
            assert!(env.fullscreen_idx.is_none());
            if new_press_on_release {
                driver.tick_events(&mut env, None, vec![pointer_button(next_press, false)]);
            } else {
                driver.tick_events(
                    &mut env,
                    None,
                    vec![
                        egui::Event::PointerMoved(next_press),
                        pointer_button(next_press, true),
                    ],
                );
                driver.tick_events(&mut env, None, vec![pointer_button(next_press, false)]);
            }
            assert_eq!(
                env.selected,
                Some(1),
                "the subsequent real mouse must work without an extra press"
            );
            if mode == crate::settings::GridClickSelectionMode::Check {
                assert_eq!(
                    env.checked, checked,
                    "plain Check clicks preserve existing checks"
                );
            } else {
                assert!(env.checked.is_empty(), "plain Explorer clicks clear checks");
            }
            // A fresh contact is also accepted; isolating the old release must not
            // turn off current touch taps or rely on suppressing a whole frame.
            let current = cell_input_response(&env, &driver.ctx, 2)
                .interact_rect
                .center();
            driver.tick_events(
                &mut env,
                None,
                vec![
                    raw_touch(2, egui::TouchPhase::Start, current),
                    egui::Event::PointerMoved(current),
                    pointer_button(current, true),
                ],
            );
            assert!(cell_input_response(&env, &driver.ctx, 2).is_pointer_button_down_on());
            driver.tick_events(
                &mut env,
                None,
                vec![
                    raw_touch(2, egui::TouchPhase::End, current),
                    pointer_button(current, false),
                    egui::Event::PointerGone,
                ],
            );
            assert_eq!(
                env.selected,
                Some(2),
                "current touch taps must retain cell semantics"
            );
            if mode == crate::settings::GridClickSelectionMode::Check {
                assert_eq!(
                    env.checked, checked,
                    "current touch retains plain Check semantics"
                );
            } else {
                assert!(env.checked.is_empty());
            }
            assert_eq!(env.current_folder.as_ref(), Some(&destination));
            assert!(env.fullscreen_idx.is_none());
        }
    }
}

#[test]
fn retired_touch_release_cannot_click_destination_thumbnail_cell() {
    retired_cell_tap_after_adoption(false, false);
}
#[test]
fn retired_touch_release_cannot_click_destination_details_cell() {
    retired_cell_tap_after_adoption(true, false);
}
#[test]
fn retired_thumbnail_cell_release_does_not_consume_later_mouse_hold() {
    retired_cell_tap_after_adoption(false, true);
}
#[test]
fn retired_details_cell_release_does_not_consume_later_mouse_hold() {
    retired_cell_tap_after_adoption(true, true);
}

fn mouse_cell_click_before_retired_orphan_release(details: bool) {
    #[cfg(windows)]
    let _native_input = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    let (mut env, mut driver, _) = input_fixture(details);
    env.grid_scroll.request_edge(GridScrollIntent::Top);
    driver.tick(&mut env, None);
    let point = cell_input_response(&env, &driver.ctx, 1)
        .interact_rect
        .center();
    let secondary = point + egui::vec2(20.0, 0.0);
    let source = env.current_folder.clone().unwrap();
    env.set_quick_folder_slot_target(QuickFolderSlotId::A, source.clone());
    env.set_quick_folder_slot_target(QuickFolderSlotId::B, source.clone());
    env.active_quick_folder_slot = Some(QuickFolderSlotId::A);
    driver.tick_events(
        &mut env,
        None,
        vec![
            raw_touch(1, egui::TouchPhase::Start, point),
            egui::Event::PointerMoved(point),
            pointer_button(point, true),
            raw_touch(2, egui::TouchPhase::Start, secondary),
        ],
    );
    assert!(cell_input_response(&env, &driver.ctx, 1).is_pointer_button_down_on());
    let old_scope = env.grid_scroll_scope();
    assert!(matches!(
        env.activate_quick_folder_slot(QuickFolderSlotId::B),
        QuickFolderSwitchTarget::Current
    ));
    assert_ne!(env.grid_scroll_scope(), old_scope);
    let selected = env.selected;
    driver.tick_events(
        &mut env,
        None,
        vec![
            raw_touch(1, egui::TouchPhase::End, point),
            pointer_button(point, false),
            egui::Event::PointerGone,
        ],
    );
    assert_eq!(
        env.selected, selected,
        "the old captured touch release must remain rejected"
    );
    env.checked.insert(4);
    let point = cell_input_response(&env, &driver.ctx, 1)
        .interact_rect
        .center();
    let cell_id = cell_input_response(&env, &driver.ctx, 1).id;
    driver.tick_events(
        &mut env,
        None,
        vec![
            egui::Event::PointerMoved(point),
            pointer_button(point, true),
            pointer_button(point, false),
            // The old secondary never supplied a native press. egui-winit's open
            // touch gate still emits its release at the last mouse cursor point.
            raw_touch(2, egui::TouchPhase::End, secondary + egui::vec2(2.0, 1.0)),
            pointer_button(point, false),
            egui::Event::PointerGone,
        ],
    );
    let response = cell_input_response(&env, &driver.ctx, 1);
    assert_eq!(response.id, cell_id);
    assert!(response.clicked_by(egui::PointerButton::Primary));
    assert_eq!(
        driver.ctx.interaction_snapshot(|snapshot| snapshot.clicked),
        Some(cell_id)
    );
    let releases = driver.ctx.input(|input| {
        input
            .pointer
            .released_buttons_with_click_counts()
            .collect::<Vec<_>>()
    });
    assert_eq!(releases.len(), 2);
    assert!(
        releases
            .iter()
            .all(|(button, click)| *button == egui::PointerButton::Primary && click.is_some()),
        "both releases look clickable to PointerState, but only the first closes a capture"
    );
    assert_eq!(
        env.selected,
        Some(1),
        "the genuine mouse click must survive the later retired orphan release"
    );
    assert!(
        env.checked.is_empty(),
        "the genuine plain Explorer click must clear checks"
    );
    assert_eq!(env.current_folder.as_ref(), Some(&source));
    assert!(env.fullscreen_idx.is_none());
}

#[test]
fn thumbnail_mouse_click_before_retired_orphan_release_is_applied() {
    mouse_cell_click_before_retired_orphan_release(false);
}
#[test]
fn details_mouse_click_before_retired_orphan_release_is_applied() {
    mouse_cell_click_before_retired_orphan_release(true);
}

//! Headless investigation of 5ch #661. No native windows or real profile data.
use super::phase_c_support::{AppTestEnv, setup_app};
use super::*;
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
            app.pending_grid_scroll,
            app.scroll_to_selected,
            app.scroll_offset_y,
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
        let mut previous_scroll = app.scroll_offset_y;
        loop {
            self.tick(app, None);
            if app.folder_pane_open_pending.is_none()
                && !app.sidecar_restore_active()
                && app.details_meta_pending.is_none()
                && matches!(
                    app.details_image_dims_state,
                    LazyColumnState::Ready { .. } | LazyColumnState::Disabled
                )
                && (app.scroll_offset_y - previous_scroll).abs() < 0.1
            {
                quiet += 1;
                if quiet == 4 {
                    return;
                }
            } else {
                quiet = 0;
            }
            previous_scroll = app.scroll_offset_y;
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
    env.settings.folder_tree_pane_visible = true;
    env.settings.grid_view_mode = if details {
        crate::settings::GridViewMode::Details
    } else {
        crate::settings::GridViewMode::Thumbnail
    };
    let mut driver = Driver::new();
    driver.open(&mut env, &source);
    let last = env.items.len() - 1;
    env.move_grid_cursor(Some(last), GridCursorIntent::Automatic);
    env.scroll_to_selected = true;
    env.pending_grid_scroll = Some(GridScrollIntent::Bottom);
    driver.settled(&mut env);
    if zip && details {
        assert_eq!(
            env.details_lazy_meta_for_idx(last)
                .and_then(|meta| meta.page_count),
            Some(1),
            "the real ZIP details worker must have read the image-containing archive"
        );
    }
    let saved = env.scroll_offset_y;
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
        env.scroll_offset_y, env.selected
    );
    assert_eq!(env.selected, selected, "return must preserve the cursor");
    assert_eq!(
        env.scroll_offset_y, saved,
        "return must preserve the bottom scroll position"
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
                        .vertical_scroll_offset(requested)
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

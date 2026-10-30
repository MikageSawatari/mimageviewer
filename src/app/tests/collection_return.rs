//! Collection return investigation (2026-10-30), based on scroll_661 Driver. No native windows or real profile data.
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
    last_generation: u64,
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
            last_generation: 0,
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
                    .div_ceil(app.grid_scroll.geometry().cols.max(1)) as f32
                    * app.grid_scroll.geometry().row_h
            },
            app.grid_scroll.geometry().viewport_h,
            app.grid_scroll.geometry().row_h,
            app.items_generation,
            app.details_image_dims_state,
            app.sidecar_restore_active(),
            app.sidecar_restore
                .as_ref()
                .map(|restore| restore.presentation().heading),
        );
        if state != self.previous || phase.starts_with("return-") {
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
            println!(
                "COLRETURN_LOCATION frame={} address={:?} effective={:?} tree={:?} collection={:?}",
                self.frame,
                app.address,
                app.effective_folder(),
                app.folder_pane.active_path(),
                app.top_level_grid_view
                    .collection_session()
                    .map(|s| &s.position)
            );
            self.previous = state;
        }
    }

    fn tick(&mut self, app: &mut App, pane: Option<PathBuf>) {
        self.frame += 1;
        if let Some(path) = pane {
            // The actual folder-tree input handler's worker admission. Full App frames
            // perform scan-ready arbitration, adoption, hydration and all UI layout.
            app.start_folder_pane_open(path);
        }
        self.checkpoint(app, "before-full-frame");
        // CollectionSource parent navigation can adopt synchronously before the
        // frame, unlike the tree worker used in scroll_661. Cover this valid
        // worker completion order before the first destination layout too.
        if self.fresh_scroll_surface && app.items_generation != self.last_generation {
            let fresh = Driver::new();
            self.ctx = fresh.ctx;
            self.backend = fresh.backend;
        }
        self.last_generation = app.items_generation;
        if self.early_hydration && app.sidecar_restore_active() {
            app.finish_page_identity_preparation_for_test();
            self.checkpoint(app, "restored-before-first-destination-grid-pre-frame");
        }
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
        if self.fresh_scroll_surface && app.items_generation != self.last_generation {
            // Diagnostic control only. Preserve App state; discard old egui geometry.
            let fresh = Driver::new();
            self.ctx = fresh.ctx;
            self.backend = fresh.backend;
        }
        self.last_generation = app.items_generation;
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

fn key_nav(app: &mut App, key: egui::Key) -> Option<crate::ui_main::AddressBarNav> {
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
        events: vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    let nav = app.handle_keyboard(&ctx);
    ctx.end_pass().drop_without_applying_deltas();
    nav
}

impl Driver {
    fn wait_for(&mut self, app: &mut App, ready: impl Fn(&App) -> bool) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while !ready(app) || app.sidecar_restore_active() {
            self.tick(app, None);
            assert!(
                Instant::now() < deadline,
                "collection return did not settle"
            );
            std::thread::sleep(Duration::from_millis(16));
        }
        self.settled(app);
    }
    fn container(&mut self, app: &mut App, path: &std::path::Path) {
        let idx = app
            .items
            .iter()
            .position(|i| i.drag_source_path() == Some(path))
            .unwrap();
        let nav = if matches!(app.items[idx], GridItem::Folder(_)) {
            app.accept_grid_container_open(&self.ctx, idx)
        } else {
            app.open_grid_container_with_mode(
                &self.ctx,
                idx,
                if app.settings.auto_fullscreen_zip_pdf {
                    crate::app::GridContainerOpenMode::PageFullscreen
                } else {
                    crate::app::GridContainerOpenMode::PageList
                },
                "collection-return-investigation",
            )
        };
        if let Some(nav) = nav {
            assert!(app.apply_fullscreen_close_nav_immediate(nav));
        }
        self.wait_for(app, |a| {
            a.current_folder.as_deref() == Some(path)
                && a.zip_enumerate_pending.is_none()
                && a.top_level_grid_view
                    .history_navigation_transition()
                    .is_none()
        });
    }
}

#[derive(Clone, Copy, Debug)]
enum Origin {
    CollectionFolder,
    Ordinary,
    CollectionRoot,
}
#[derive(Clone, Copy, Debug)]
enum Return {
    EscThenBackspace,
    BsThenBackspace,
    Parent,
    AutoClose,
}

fn investigate(
    origin: Origin,
    ret: Return,
    details: bool,
    top: bool,
    kind: &str,
    early: bool,
    fresh: bool,
) {
    #[cfg(windows)]
    let _native_input = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    // Keep in-process PDF routes registered until App has joined all workers.
    let mut pdf_backends = Vec::new();
    let mut env: AppTestEnv = setup_app();
    env.initialized = true;
    env.settings.update_check_enabled = false;
    env.settings.minimize_to_tray_on_close = false;
    env.indexer_init = IndexerInit::Unavailable;
    env.last_main_focused = false;
    env.settings.creative_luts.clear();
    env.creative_lut_library = crate::creative_lut::CreativeLutLibrary::default();
    env.settings.folder_tree_pane_visible = true;
    env.settings.detached_viewer_open_images_in_window = false;
    // Configure this before any listing exists, so a setting-change reload
    // cannot supersede the book-open request being investigated.
    env.settings.auto_fullscreen_zip_pdf = matches!(ret, Return::AutoClose);
    env.settings.auto_fullscreen_image_folders = false;
    env.settings.grid_view_mode = if details {
        crate::settings::GridViewMode::Details
    } else {
        crate::settings::GridViewMode::Thumbnail
    };
    let shelf = fixtures(env.tmp.path(), 200, true);
    let pages = if kind == "zip-one" { 1 } else { 32 };
    let image_folder = kind == "folder";
    let child = if kind == "pdf" {
        for n in 0..200 {
            std::fs::remove_file(shelf.join(format!("{n:04}.zip"))).unwrap();
            let pdf = shelf.join(format!("{n:04}.pdf"));
            write_pdf(&pdf, 2);
            pdf_backends.push(crate::pdf_loader::RemotePdfTestBackend::for_path(&pdf));
        }
        shelf.join("0199.pdf")
    } else if image_folder {
        for n in 0..200 {
            std::fs::remove_file(shelf.join(format!("{n:04}.zip"))).unwrap();
            let folder = shelf.join(format!("{n:04}-folder"));
            std::fs::create_dir(&folder).unwrap();
            image::RgbImage::from_pixel(32, 48, image::Rgb([80u8, 120, 160]))
                .save(folder.join("page.png"))
                .unwrap();
        }
        shelf.join("0199-folder")
    } else {
        shelf.join(if top { "0000.zip" } else { "0199.zip" })
    };
    if kind == "pdf" {
        write_pdf(&child, pages);
    } else if image_folder {
        for n in 1..pages {
            image::RgbImage::from_pixel(32, 48, image::Rgb([80u8, 120, 160]))
                .save(child.join(format!("page-{n:04}.png")))
                .unwrap();
        }
    } else {
        write_book_zip(&child, pages);
    }
    let mut driver = Driver::new();
    let mut collection_id = None;
    if !matches!(origin, Origin::Ordinary) {
        use crate::collection_store::{
            CollectionRegistration, CollectionResolvedKind, CollectionStoreRuntime,
        };
        let runtime =
            CollectionStoreRuntime::start_at(env.tmp.path().join("collection.db")).unwrap();
        let client = runtime.client();
        env.install_collection_runtime(runtime);
        driver.wait_for(&mut env, |a| {
            matches!(a.collection_store_client_for_read(), Ok(Some(_)))
        });
        let created = client
            .create_collection("Return investigation".into())
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap();
        let sources = if matches!(origin, Origin::CollectionRoot) {
            (0..200)
                .map(|n| {
                    CollectionRegistration::from_trusted_path(
                        &shelf.join(format!("{n:04}.zip")),
                        CollectionResolvedKind::Zip,
                    )
                    .unwrap()
                })
                .collect()
        } else {
            vec![
                CollectionRegistration::from_trusted_path(&shelf, CollectionResolvedKind::Folder)
                    .unwrap(),
            ]
        };
        let snapshot = client
            .add_batch(created.collection_id(), created.revision(), sources)
            .unwrap()
            .recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap()
            .snapshot;
        let id = snapshot.collection_id();
        collection_id = Some(id);
        env.open_collection_grid_from_navigation(id);
        driver.wait_for(&mut env, |a| {
            a.top_level_grid_view
                .collection_session()
                .is_some_and(|s| s.prepared().is_some())
        });
        if matches!(origin, Origin::CollectionFolder) {
            driver.container(&mut env, &shelf);
        }
    } else {
        driver.open(&mut env, &shelf);
    }
    let idx = env
        .items
        .iter()
        .position(|i| i.drag_source_path() == Some(child.as_path()))
        .unwrap();
    env.move_grid_cursor(Some(idx), GridCursorIntent::Automatic);
    env.grid_scroll.set_reveal_requested(true);
    if !top {
        env.grid_scroll.request_edge(GridScrollIntent::Bottom);
    }
    driver.settled(&mut env);
    assert_eq!(env.items.len(), 200, "long-list fixture");
    if details && kind != "folder" {
        assert_eq!(
            env.details_lazy_meta_for_idx(idx)
                .and_then(|m| m.page_count),
            Some(pages as u32),
            "real details enumeration"
        );
    }
    let saved = env.grid_scroll.offset();
    assert!(
        if top { saved == 0.0 } else { saved > 0.0 },
        "initial position top={top}: {saved}"
    );
    let selected = env.selected;
    driver.checkpoint(&env, "before-book-open");
    driver.container(&mut env, &child);
    if !matches!(ret, Return::AutoClose) {
        env.open_fullscreen(0, crate::app::HistoryTrigger::UserChosen);
    }
    assert_eq!(env.fullscreen_idx, Some(0));
    driver.tick(&mut env, None);
    // Page-list mode must actually render the short list, just as Esc does.
    if !matches!(ret, Return::AutoClose) {
        if matches!(ret, Return::EscThenBackspace | Return::BsThenBackspace) {
            fullscreen_key(
                &mut env,
                if matches!(ret, Return::BsThenBackspace) {
                    egui::Key::Backspace
                } else {
                    egui::Key::Escape
                },
            );
        } else {
            env.handle_fullscreen_close_request();
        }
        assert!(env.fullscreen_idx.is_none());
        driver.settled(&mut env);
    }
    driver.early_hydration = early;
    driver.fresh_scroll_surface = fresh;
    match ret {
        Return::EscThenBackspace | Return::BsThenBackspace => {
            let nav = key_nav(&mut env, egui::Key::Backspace).expect("Backspace parent nav");
            assert!(env.apply_fullscreen_close_nav_immediate(nav));
        }
        Return::Parent => {
            let nav = env.resolve_grid_parent_nav().expect("toolbar parent nav");
            assert!(env.apply_fullscreen_close_nav_immediate(nav));
        }
        Return::AutoClose => env.handle_fullscreen_close_request_immediate(),
    }
    if matches!(origin, Origin::CollectionRoot) {
        driver.wait_for(&mut env, |a| {
            a.current_folder.is_none()
                && a.top_level_grid_view
                    .collection_session()
                    .is_some_and(|s| s.prepared().is_some())
        });
    } else {
        driver.wait_for(&mut env, |a| {
            a.current_folder.as_deref() == Some(shelf.as_path())
                && a.top_level_grid_view
                    .history_navigation_transition()
                    .is_none()
        });
    }
    driver.checkpoint(&env, "return-details-ready");
    assert_eq!(
        env.items[env.selected.unwrap()].drag_source_path(),
        Some(child.as_path()),
        "(b) selected child"
    );
    if !matches!(origin, Origin::CollectionRoot) {
        assert_eq!(
            env.current_folder.as_deref(),
            Some(shelf.as_path()),
            "(a) folder"
        );
        assert_eq!(env.address, shelf.to_string_lossy(), "(d) address");
        assert_eq!(
            env.folder_pane.active_path(),
            Some(shelf.as_path()),
            "(d) tree active folder"
        );
        assert_eq!(
            env.folder_history.get(&shelf),
            Some(&(saved, selected)),
            "saved history"
        );
    } else {
        assert_eq!(
            env.address, "コレクション: Return investigation",
            "(d) collection address"
        );
        assert_eq!(env.folder_pane.active_path(), None, "(d) root tree");
        assert_eq!(
            env.top_level_grid_view
                .collection_session()
                .unwrap()
                .identity
                .collection_id,
            collection_id.unwrap(),
            "(a) collection identity"
        );
    }
    println!(
        "COLRETURN_RESULT origin={origin:?} return={ret:?} details={details} top={top} kind={kind} early={early} fresh={fresh} saved={saved} actual={} selected={:?} gen={} collection={collection_id:?}",
        env.grid_scroll.offset(),
        env.selected,
        env.items_generation
    );
    let actual = env.grid_scroll.offset();
    env.shutdown_collection_runtime_for_exit();
    assert!(
        (actual - saved).abs() < 1.0,
        "(c) restored viewport moved: {saved} -> {actual}"
    );
}

#[test]
fn collection_zip_details_natural() {
    investigate(
        Origin::CollectionFolder,
        Return::EscThenBackspace,
        true,
        false,
        "zip",
        false,
        false,
    );
}
#[test]
fn collection_zip_thumbnail_natural() {
    investigate(
        Origin::CollectionFolder,
        Return::EscThenBackspace,
        false,
        false,
        "zip",
        false,
        false,
    );
}
#[test]
fn collection_zip_details_fast() {
    investigate(
        Origin::CollectionFolder,
        Return::EscThenBackspace,
        true,
        false,
        "zip",
        true,
        false,
    );
}
#[test]
fn collection_zip_thumbnail_fast() {
    investigate(
        Origin::CollectionFolder,
        Return::EscThenBackspace,
        false,
        false,
        "zip",
        true,
        false,
    );
}
#[test]
fn collection_zip_details_fresh_control() {
    investigate(
        Origin::CollectionFolder,
        Return::EscThenBackspace,
        true,
        false,
        "zip",
        true,
        true,
    );
}
#[test]
fn collection_zip_details_parent() {
    investigate(
        Origin::CollectionFolder,
        Return::Parent,
        true,
        false,
        "zip",
        true,
        false,
    );
}
#[test]
fn collection_zip_details_auto_close() {
    investigate(
        Origin::CollectionFolder,
        Return::AutoClose,
        true,
        false,
        "zip",
        true,
        false,
    );
}
#[test]
fn collection_zip_details_top() {
    investigate(
        Origin::CollectionFolder,
        Return::EscThenBackspace,
        true,
        true,
        "zip",
        true,
        false,
    );
}
#[test]
fn ordinary_zip_details() {
    investigate(
        Origin::Ordinary,
        Return::EscThenBackspace,
        true,
        false,
        "zip",
        true,
        false,
    );
}
#[test]
fn root_zip_details() {
    investigate(
        Origin::CollectionRoot,
        Return::EscThenBackspace,
        true,
        false,
        "zip",
        true,
        false,
    );
}
#[test]
fn collection_image_folder_details() {
    investigate(
        Origin::CollectionFolder,
        Return::EscThenBackspace,
        true,
        false,
        "folder",
        true,
        false,
    );
}

#[test]
fn collection_pdf_details() {
    investigate(
        Origin::CollectionFolder,
        Return::EscThenBackspace,
        true,
        false,
        "pdf",
        true,
        false,
    );
}

fn write_pdf(path: &std::path::Path, pages: usize) {
    let kids = (0..pages)
        .map(|n| format!("{} 0 R", 4 + n * 2))
        .collect::<Vec<_>>()
        .join(" ");
    let mut objects = vec![
        "<< /Type /Catalog /Pages 2 0 R >>".to_owned(),
        format!("<< /Type /Pages /Kids [{kids}] /Count {pages} >>"),
    ];
    for n in 0..pages {
        objects.push("<< /Length 0 >>\nstream\nendstream".into());
        objects.push(format!(
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 100] /Contents {} 0 R >>",
            3 + n * 2
        ));
    }
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = vec![0usize];
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{}\nendobj\n", index + 1, object).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(format!("xref\n0 {}\n0000000000 65535 f \n", offsets.len()).as_bytes());
    for offset in offsets.into_iter().skip(1) {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!(
            "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
            objects.len() + 1
        )
        .as_bytes(),
    );
    std::fs::write(path, bytes).unwrap();
}

fn write_book_zip(path: &std::path::Path, pages: usize) {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        32,
        48,
        image::Rgb([80, 120, 160]),
    ))
    .write_to(&mut bytes, image::ImageFormat::Jpeg)
    .unwrap();
    let mut archive = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for n in 0..pages {
        archive
            .start_file(
                format!("page-{n:04}.jpg"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
        archive.write_all(bytes.get_ref()).unwrap();
    }
    archive.finish().unwrap();
}

fn fullscreen_key(app: &mut App, key: egui::Key) {
    let ctx = egui::Context::default();
    ctx.begin_pass(egui::RawInput {
        events: vec![egui::Event::Key {
            key,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        }],
        ..Default::default()
    });
    crate::keyboard_input::cache_keyboard_owner(
        &ctx,
        crate::keyboard_input::KeyboardOwner::ApplicationShortcut {
            scope: crate::keyboard_input::ShortcutScope::new(
                ctx.viewport_id(),
                crate::keyboard_input::ShortcutSurface::Fullscreen,
            ),
        },
    );
    let idx = app.fullscreen_idx.unwrap();
    let action = app.handle_fs_key_input(&ctx, idx, false);
    assert!(
        action.close || action.close_to_page_list,
        "synthetic fullscreen key was not accepted"
    );
    app.handle_fs_navigation(
        &ctx,
        action.close,
        action.close_to_page_list,
        None,
        None,
        None,
        action.page_nav,
        None,
        idx,
    );
    ctx.end_pass().drop_without_applying_deltas();
}

#[test]
fn collection_zip_details_bs() {
    investigate(
        Origin::CollectionFolder,
        Return::BsThenBackspace,
        true,
        false,
        "zip",
        true,
        false,
    );
}

#[test]
fn collection_zip_thumbnail_fresh_control() {
    investigate(
        Origin::CollectionFolder,
        Return::EscThenBackspace,
        false,
        false,
        "zip",
        true,
        true,
    );
}
#[test]
fn ordinary_zip_thumbnail() {
    investigate(
        Origin::Ordinary,
        Return::EscThenBackspace,
        false,
        false,
        "zip",
        true,
        false,
    );
}
#[test]
fn root_zip_thumbnail() {
    investigate(
        Origin::CollectionRoot,
        Return::EscThenBackspace,
        false,
        false,
        "zip",
        true,
        false,
    );
}
#[test]
fn collection_image_folder_thumbnail() {
    investigate(
        Origin::CollectionFolder,
        Return::EscThenBackspace,
        false,
        false,
        "folder",
        true,
        false,
    );
}
#[test]
fn collection_pdf_thumbnail() {
    investigate(
        Origin::CollectionFolder,
        Return::EscThenBackspace,
        false,
        false,
        "pdf",
        true,
        false,
    );
}

#[test]
fn collection_one_page_details_control() {
    investigate(
        Origin::CollectionFolder,
        Return::EscThenBackspace,
        true,
        false,
        "zip-one",
        true,
        false,
    );
}
#[test]
fn collection_one_page_thumbnail_control() {
    investigate(
        Origin::CollectionFolder,
        Return::EscThenBackspace,
        false,
        false,
        "zip-one",
        true,
        false,
    );
}

//! §1.473: headless production open/adoption/parent routing, disposable data only.
use super::*;
use crate::ring_shortcut::{MouseButtonSlot, RingActionId, RingShortcutContext};
use std::path::Path;
use std::time::{Duration, Instant};

// App and its workers must drop before the override, temporary files and global lock.
pub(super) struct CaseEnv {
    app: App,
    _epub_pin: Option<crate::pdf_loader::TestEpubPin>,
    pdf_backend: Option<crate::pdf_loader::RemotePdfTestBackend>,
    _epoch: crate::page_edit_write_epoch::TestEpochScope,
    _guard: super::phase_c_support::OverrideGuard,
    tmp: tempfile::TempDir,
    _lock: std::sync::MutexGuard<'static, ()>,
}
impl std::ops::Deref for CaseEnv {
    type Target = App;
    fn deref(&self) -> &App {
        &self.app
    }
}
impl std::ops::DerefMut for CaseEnv {
    fn deref_mut(&mut self) -> &mut App {
        &mut self.app
    }
}
pub(super) fn setup(portable_layout: bool) -> CaseEnv {
    let lock = crate::data_dir::test_override_lock();
    let tmp = tempfile::TempDir::new().unwrap();
    let data_dir = tmp.path().join(if portable_layout {
        "exe/data"
    } else {
        "appdata/mimageviewer"
    });
    std::fs::create_dir_all(&data_dir).unwrap();
    crate::data_dir::set_test_override(Some(data_dir.clone()));
    let guard = super::phase_c_support::OverrideGuard;
    let epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
    let mut settings = crate::settings::Settings::default();
    settings.book_root = Some(tmp.path().join("books"));
    let mut app = App::new_for_test(AppTestConfig {
        data_dir,
        settings: Some(settings),
        similar_feature_capability: crate::similar_index::SimilarFeatureCapability::Enabled,
    });
    app.settings.first_setup_completed = true;
    CaseEnv {
        app,
        _epub_pin: None,
        pdf_backend: None,
        _epoch: epoch,
        _guard: guard,
        tmp,
        _lock: lock,
    }
}

#[derive(Clone, Copy, Debug)]
enum Entrance {
    Menu,
    Ring,
    Mouse,
    FolderBar,
}

fn trace(app: &App, stage: &str) {
    eprintln!(
        "1473 feedback={:?} pdf_pending={} pdf_nav={} history_pending={}",
        app.feedback_toasts_for_test,
        app.pdf_enumerate_pending.is_some(),
        app.fs_nav_after_pdf_enumerate.is_some(),
        app.top_level_grid_view
            .history_navigation_transition()
            .is_some()
    );
    eprintln!(
        "1473 {stage}: logical={:?} original={:?} backing={:?} epub={:?} return_from={:?} history_view={} zip_prefix={:?} parent={:?}",
        app.effective_folder(),
        app.archive_source_override,
        app.current_folder,
        app.display_epub_source,
        app.reading_history_return_from,
        app.items_are_reading_history_view,
        app.zip_nav.as_ref().map(|n| n.current()),
        app.grid_parent_nav_target(),
    );
    if let Some(path) = app
        .effective_folder()
        .filter(|p| p.extension().is_some_and(|e| e == "epub"))
    {
        let read = crate::pdf_loader::resolve_read_target(&path).unwrap();
        eprintln!(
            "1473 EPUB physical={:?} generation={:?}",
            read.read_path, read.stamp
        );
    }
}

pub(super) fn settle(app: &mut App) {
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(60);
    loop {
        app.settle_open_path_classification_for_test();
        app.poll_collection_history_transition(&ctx);
        ctx.run_pass(egui::RawInput::default(), |ctx| {
            app.show_archive_convert_dialog(ctx);
        })
        .drop_without_applying_deltas();
        app.poll_zip_enumerate();
        app.poll_pdf_enumerate();
        app.poll_reading_history_thumbnail_sources(&ctx);
        app.poll_spread_shift_service();
        app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
        // The real toolbar starts a bookshelf refresh; sidecar quiescence waits for it.
        app.poll_book_op_pending(&ctx);
        app.poll_sidecar_restore(&ctx);
        app.poll_book_composition_service_for_test(&ctx);
        app.poll_book_presentation_permission(&ctx);
        app.poll_bookmark_browser(&ctx);
        app.poll_rating_view();
        app.poll_tag_view();
        app.poll_favsearch();
        app.poll_search_page_edit_prepare(&ctx);
        app.poll_subfolder_expansion(&ctx);
        app.poll_smart_folder(&ctx);
        app.poll_collection_ui(&ctx);
        app.poll_collection_grid(&ctx);
        if let Some(state) = &app.archive_convert {
            if let crate::ui_dialogs::archive_convert::ArchiveConvertPhase::Error { message } =
                &state.phase
            {
                panic!("archive fixture failed: {message}");
            }
        }
        if app
            .top_level_grid_view
            .history_navigation_transition()
            .is_none()
            && app.top_level_grid_view.open_path_classification().is_none()
            && app.archive_convert.is_none()
            && app.zip_enumerate_pending.is_none()
            && app.pdf_enumerate_pending.is_none()
            && app.reading_history_sources_pending.is_empty()
            && !app.sidecar_restore_active()
            && app.bookmark_browser_pending.is_none()
            && app.rating_view_pending.is_none()
            && app.tag_view_pending.is_none()
            && app.favsearch_pending.is_none()
            && app.subfolder_expansion_pending.is_none()
            && app.subfolder_expansion_install_pending.is_none()
            && app.smart_folder_pending.is_none()
            && app.smart_folder_transition.is_none()
        {
            return;
        }
        if Instant::now() >= deadline {
            trace(app, "timeout");
            eprintln!(
                "1473 sidecar phase={:?} retained_shift={} shift_service={} composition_service={}",
                app.sidecar_restore
                    .as_ref()
                    .map(|state| state.presentation()),
                app.spread_shift.retained_readoption_pending(),
                app.spread_shift_service.is_some(),
                app.book_composition_service.is_some()
            );
            eprintln!(
                "1473 wait owners sidecar={} bookmark={} rating={} tag={} favorite={} reading={} subscan={} subinstall={} smartscan={} smarttransition={} classification={} archive={} zip={} pdf={}",
                app.sidecar_restore_active(),
                app.bookmark_browser_pending.is_some(),
                app.rating_view_pending.is_some(),
                app.tag_view_pending.is_some(),
                app.favsearch_pending.is_some(),
                app.reading_history_sources_pending.len(),
                app.subfolder_expansion_pending.is_some(),
                app.subfolder_expansion_install_pending.is_some(),
                app.smart_folder_pending.is_some(),
                app.smart_folder_transition.is_some(),
                app.top_level_grid_view.open_path_classification().is_some(),
                app.archive_convert.is_some(),
                app.zip_enumerate_pending.is_some(),
                app.pdf_enumerate_pending.is_some()
            );
            panic!(
                "1473 open did not settle; epub modal={}",
                app.epub_convert.is_some()
            );
        }
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn root_parent_button(app: &mut App) -> Option<crate::ui_main::AddressBarNav> {
    use egui_kittest::kittest::{NodeT, Queryable};
    let mut nav = None;
    let fonts = std::cell::Cell::new(false);
    app.settings.show_toolbar_parent_button = true;
    app.settings.toolbar_section_order = vec![crate::settings::ToolbarSectionId::Folder];
    {
        let mut harness = crate::test_ui::builder()
            .with_size(egui::vec2(1400.0, 280.0))
            .build_ui(|ui| {
                if !fonts.replace(true) {
                    crate::ui_fonts::configure_fonts(ui.ctx());
                    ui.ctx().request_repaint();
                    return;
                }
                if let Some(next) = app.render_toolbar(ui).1 {
                    nav = Some(next);
                }
            });
        harness.run();
        let enabled = !harness.get_by_label("⬆").accesskit_node().is_disabled();
        eprintln!("1473 root-parent-button-enabled={enabled}");
        if enabled {
            harness.get_by_label("⬆").click();
            harness.run();
        }
    }
    nav
}

fn enter_other_root(app: &mut App, kind: &str, origin: &Path) {
    match kind {
        "bookmarks" => app.open_bookmark_browser(),
        "rating" => app.enter_rating_view(3),
        "global-search" => {
            app.open_global_search();
            // Adopt an empty result through the production item producer; no external index.
            app.replace_search_view_items(Vec::new(), Vec::new());
        }
        "favorite-search" => {
            app.open_favsearch();
            app.execute_favsearch();
        }
        "tag" => app.open_tag_view_for_tag("1473 root audit"),
        "drives" => app.enter_drive_list_from_navigation(None),
        "collection" => {
            let runtime = crate::collection_store::CollectionStoreRuntime::start_at(
                crate::data_dir::get().join("collections.db"),
            )
            .unwrap();
            let client = runtime.client();
            app.install_collection_runtime(runtime);
            let ctx = egui::Context::default();
            let deadline = Instant::now() + Duration::from_secs(10);
            while !matches!(app.collection_store_client_for_read(), Ok(Some(_))) {
                app.poll_collection_ui(&ctx);
                assert!(Instant::now() < deadline);
                std::thread::sleep(Duration::from_millis(2));
            }
            let created = client
                .create_collection("1473 audit".into())
                .unwrap()
                .recv_timeout(Duration::from_secs(10))
                .unwrap()
                .unwrap();
            let source = origin.join("owned-source");
            if source.is_dir() {
                let registration =
                    crate::collection_store::CollectionRegistration::from_trusted_path(
                        &source,
                        crate::collection_store::CollectionResolvedKind::Folder,
                    )
                    .unwrap();
                client
                    .add_batch(
                        created.collection_id(),
                        created.revision(),
                        vec![registration],
                    )
                    .unwrap()
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap()
                    .unwrap();
            }
            app.open_collection_grid_from_navigation(created.collection_id());
        }
        "smart" => {
            let mut definition = crate::settings::SmartFolderDefinition::new("1473 audit");
            definition.rules.push(crate::settings::SmartFolderRule::new(
                origin.to_owned(),
                true,
                Default::default(),
            ));
            let id = definition.id;
            app.settings.smart_folders.push(definition);
            app.open_smart_folder(id, false);
        }
        "subfolder-expansion" => {
            app.start_subfolder_expansion_scan_roots(origin.to_owned(), vec![origin.to_owned()])
        }
        "snapshot" => app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed),
        "books" => app.open_books_root(),
        _ => unreachable!(),
    }
    settle(app);
    if kind == "collection" {
        let deadline = Instant::now() + Duration::from_secs(10);
        while !app
            .top_level_grid_view
            .collection_session()
            .is_some_and(|session| {
                matches!(
                    session.load,
                    top_level_grid_view::CollectionGridLoadState::Ready(_)
                        | top_level_grid_view::CollectionGridLoadState::Empty(_)
                ) && session.installed_items_generation == Some(app.items_generation)
            })
        {
            assert!(
                Instant::now() < deadline,
                "collection root did not finish preparing"
            );
            std::thread::sleep(Duration::from_millis(2));
            settle(app);
        }
    }
    let accepted = match kind {
        "bookmarks" => app.items_are_bookmark_view,
        "rating" => app.items_are_rating_view,
        "global-search" => app.global_search.active && app.items_are_global_search_view,
        "favorite-search" => app.favsearch.on_results_grid(),
        "tag" => app.tag_view.active && app.items_are_tag_view,
        "drives" => app.items_are_drive_list,
        "collection" => {
            app.top_level_grid_view.collection_session().is_some() && app.current_folder.is_none()
        }
        "smart" => app.items_are_smart_folder_view,
        "subfolder-expansion" => app.items_are_subfolder_expansion_view,
        "snapshot" => app.is_snapshot_active(),
        "books" => app.current_folder.as_ref() == app.settings.book_root.as_ref(),
        _ => false,
    };
    assert!(
        accepted,
        "{kind} root did not adopt: {:?}",
        app.feedback_toasts_for_test
    );
}

#[test]
fn section1473_other_synthetic_roots_parent_inventory() {
    let mut leaks = Vec::new();
    for portable in [false, true] {
        for kind in [
            "bookmarks",
            "rating",
            "global-search",
            "favorite-search",
            "tag",
            "drives",
            "collection",
            "smart",
            "subfolder-expansion",
            "snapshot",
            "books",
        ] {
            for route in ["button", "backspace", "alt-up", "mouse", "ring"] {
                let mut app = setup(portable);
                configure(&mut app);
                let (book, _) = fixture(&mut app, "folder");
                let origin = book.parent().unwrap().to_owned();
                app.load_folder(origin.clone());
                settle(&mut app);
                enter_other_root(&mut app, kind, &origin);
                eprintln!(
                    "1473 sibling-case portable={portable} kind={kind} route={route} before={:?} surface={:?}",
                    app.effective_folder(),
                    app.top_level_grid_view.surface()
                );
                let nav = if route == "button" {
                    root_parent_button(&mut app)
                } else {
                    root_parent_input(&mut app, route)
                };
                eprintln!("1473 sibling-parent-intent={nav:?}");
                apply_on(
                    &mut app,
                    nav,
                    if route == "button" {
                        "address"
                    } else {
                        "keyboard"
                    },
                );
                let selected = app
                    .selected
                    .and_then(|index| app.items.get(index))
                    .and_then(GridItem::container_path)
                    .map(Path::to_owned);
                eprintln!(
                    "1473 sibling-parent-adopted={:?} selected={selected:?} tag={} favorite={} global={}",
                    app.effective_folder(),
                    app.tag_view.active,
                    app.favsearch.active,
                    app.global_search.active
                );
                if app.effective_folder().is_some_and(|path| {
                    path.is_dir()
                        && crate::path_key::physical_path_is_within(&path, &crate::data_dir::get())
                }) {
                    leaks.push(format!("{portable}/{kind}/{route}"));
                }
            }
        }
    }
    assert!(
        leaks.is_empty(),
        "synthetic parents entered internal data: {leaks:?}"
    );
}

fn key(
    app: &mut App,
    key: egui::Key,
    modifiers: egui::Modifiers,
) -> Option<crate::ui_main::AddressBarNav> {
    let ctx = egui::Context::default();
    let mut nav = None;
    ctx.run_pass(
        egui::RawInput {
            events: vec![
                egui::Event::ModifiersChanged(modifiers),
                egui::Event::Key {
                    key,
                    physical_key: None,
                    pressed: true,
                    repeat: false,
                    modifiers,
                },
            ],
            ..Default::default()
        },
        |ctx| nav = app.handle_keyboard(ctx),
    )
    .drop_without_applying_deltas();
    nav
}

fn apply(app: &mut App, nav: Option<crate::ui_main::AddressBarNav>) {
    apply_on(app, nav, "keyboard");
}

fn apply_on(app: &mut App, nav: Option<crate::ui_main::AddressBarNav>, lane: &str) {
    if let Some(nav) = nav {
        let mut frame = update_phases::MainFrameNavigation {
            fav_nav: None,
            toolbar_fav_nav: None,
            fullscreen_close_nav: None,
            keyboard_nav: None,
            keyboard_nav_from_fullscreen_close: false,
            gamepad_nav: None,
            address_nav: None,
            open_folder_nav: None,
            context_nav: None,
            folder_pane_nav: None,
            grid_nav: None,
        };
        match lane {
            "grid" => frame.grid_nav = Some(nav),
            "address" => frame.address_nav = Some(nav),
            _ => frame.keyboard_nav = Some(nav),
        }
        app.post_render_main_navigation(&egui::Context::default(), frame);
    }
    settle(app);
}

fn configure(app: &mut App) {
    app.active_quick_folder_slot = None;
    app.settings.auto_fullscreen_zip_pdf = false;
    app.settings.auto_fullscreen_image_folders = false;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings
        .set_archive_file_handling(crate::settings::ArchiveFileHandling::Convert);
}

fn enter(app: &mut App, entrance: Entrance) {
    let ctx = egui::Context::default();
    match entrance {
        Entrance::Menu => app.enter_reading_history_from_menu(),
        Entrance::Ring => {
            let nav = app.apply_ring_action(
                &ctx,
                RingShortcutContext::Grid,
                RingActionId::OpenLocationReadingHistory,
                "1473-ring",
            );
            apply(app, nav);
        }
        Entrance::Mouse => {
            app.settings
                .ring_shortcuts
                .mouse_button_profile_mut(RingShortcutContext::Grid)
                .back = RingActionId::OpenLocationReadingHistory;
            let nav = app.apply_mouse_button(
                &ctx,
                MouseButtonSlot::Back,
                ActionSurface::MainWindow,
                "1473-mouse",
            );
            apply(app, nav);
        }
        Entrance::FolderBar => unreachable!(),
    }
    settle(app);
    assert!(
        app.items_are_reading_history_view,
        "history admission: surface={:?}, current={:?}, busy={}, feedback={:?}",
        app.top_level_grid_view.surface(),
        app.current_folder,
        app.sidecar_restore_active(),
        app.feedback_toasts_for_test
    );
    assert_eq!(app.current_folder, Some(reading_history_synthetic_path()));
}

fn add_history(app: &App, path: &Path, kind: crate::reading_history_db::ReadingHistoryKind) {
    let entry = crate::reading_history_db::ReadingHistoryEntry::new(
        path.to_owned(),
        kind,
        crate::reading_history_db::archive_format_for_path(path),
        "1473 fixture".into(),
        None,
        None,
    );
    app.reading_history_db
        .as_ref()
        .unwrap()
        .upsert(entry, 1000)
        .unwrap();
}

fn write_pdf(path: &Path) {
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] >>",
    ];
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (i, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", i + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 4\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Root 1 0 R /Size 4 >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    std::fs::write(path, bytes).unwrap();
}

fn write_zip(path: &Path, entries: &[&str]) {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::new(3, 4))
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for name in entries {
        zip.start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        std::io::Write::write_all(&mut zip, bytes.get_ref()).unwrap();
    }
    zip.finish().unwrap();
}

fn fixture(
    app: &mut CaseEnv,
    kind: &str,
) -> (PathBuf, crate::reading_history_db::ReadingHistoryKind) {
    use crate::reading_history_db::ReadingHistoryKind as K;
    let root = app.tmp.path().join("library");
    std::fs::create_dir_all(&root).unwrap();
    match kind {
        "folder" | "image_book" => {
            let path = root.join(kind);
            std::fs::create_dir_all(&path).unwrap();
            image::RgbImage::new(3, 4)
                .save(path.join("page.png"))
                .unwrap();
            if kind == "folder" {
                std::fs::create_dir(path.join("child")).unwrap();
            }
            (path, K::Folder)
        }
        "zip" => {
            let path = root.join("book.zip");
            write_zip(&path, &["page.png"]);
            (path, K::Zip)
        }
        "pdf" => {
            let path = root.join("book.pdf");
            write_pdf(&path);
            app.pdf_backend = Some(crate::pdf_loader::RemotePdfTestBackend::for_path(&path));
            (path, K::Pdf)
        }
        "archive" | "archive_cold" => {
            let path = root.join("book.rar");
            std::fs::copy(
                Path::new(env!("CARGO_MANIFEST_DIR"))
                    .join("testdata/archives/multiwindow-rar-nav/02-solid.rar"),
                &path,
            )
            .unwrap();
            if kind == "archive_cold" {
                return (path, K::Archive);
            }
            let db = app.archive_cache_db.as_ref().unwrap();
            let cached = db.reserve_cache_zip_path(&path).unwrap();
            crate::archive_converter::convert_to_zip(
                &path,
                &cached,
                crate::archive_converter::ArchiveFormat::Rar,
                &std::sync::atomic::AtomicBool::new(false),
                None,
            )
            .unwrap();
            let meta = std::fs::metadata(&path).unwrap();
            db.record(
                &path,
                crate::ui_helpers::mtime_secs(&meta),
                meta.len() as i64,
                crate::archive_converter::ArchiveFormat::Rar,
                &cached,
                std::fs::metadata(&cached).unwrap().len() as i64,
                1,
                false,
            )
            .unwrap();
            (path, K::Archive)
        }
        "epub" => {
            struct Source(crate::epub_cache::SourceState);
            impl crate::epub_cache::SourceGuard for Source {
                fn state(&self) -> std::io::Result<crate::epub_cache::SourceState> {
                    Ok(self.0)
                }
            }
            let path = root.join("book.epub");
            let mut epub = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
            for (name, content) in [
                ("mimetype", "application/epub+zip"),
                (
                    "META-INF/container.xml",
                    "<container xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\" version=\"1.0\"><rootfiles><rootfile full-path=\"content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>",
                ),
                (
                    "content.opf",
                    "<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"3.0\" unique-identifier=\"id\"><metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\"><dc:identifier id=\"id\">1473</dc:identifier><dc:title>1473</dc:title><dc:language>en</dc:language></metadata><manifest><item id=\"ch\" href=\"chapter.xhtml\" media-type=\"application/xhtml+xml\"/></manifest><spine><itemref idref=\"ch\"/></spine></package>",
                ),
                (
                    "chapter.xhtml",
                    "<html xmlns=\"http://www.w3.org/1999/xhtml\"><body><p>1473</p></body></html>",
                ),
            ] {
                epub.start_file(name, zip::write::SimpleFileOptions::default())
                    .unwrap();
                std::io::Write::write_all(&mut epub, content.as_bytes()).unwrap();
            }
            epub.finish().unwrap();
            let state = crate::epub_cache::source_state(&std::fs::metadata(&path).unwrap());
            let mut cache = crate::epub_cache::EpubCache::open_at(&crate::data_dir::get()).unwrap();
            let reserved = cache.reserve_output(&path).unwrap();
            std::fs::create_dir_all(reserved.final_path().parent().unwrap()).unwrap();
            write_pdf(reserved.final_path());
            let row = crate::epub_cache::GenerationRow {
                generation_id: reserved.generation_id(),
                src_path_key: crate::epub_cache::src_key(&path),
                src_path: path.clone(),
                src_state: state,
                src_sha256: "1473".into(),
                src_head_hash: "1473".into(),
                pdf_file: reserved.final_path().to_owned(),
                pdf_size: std::fs::metadata(reserved.final_path()).unwrap().len(),
                page_count: 1,
                direction: "ltr".into(),
                profile: "test".into(),
                created_at: 1,
                output_version: crate::epub_cache::CONVERTER_OUTPUT_VERSION,
            };
            assert_eq!(
                cache.publish(&row, &Source(state)).unwrap(),
                crate::epub_cache::PublishOutcome::Published
            );
            let (pin, resolved) =
                crate::pdf_loader::pin_cached_epub_for_test(&path, &crate::data_dir::get())
                    .unwrap();
            assert_eq!(resolved.read_path.as_path(), row.pdf_file);
            app._epub_pin = Some(pin);
            app.pdf_backend = Some(crate::pdf_loader::RemotePdfTestBackend::for_path(
                &row.pdf_file,
            ));
            (path, K::Pdf)
        }
        _ => unreachable!(),
    }
}

fn matrix(kind: &str) {
    for portable_layout in [false, true] {
        for entrance in [
            Entrance::Menu,
            Entrance::Ring,
            Entrance::Mouse,
            Entrance::FolderBar,
        ] {
            for parent in ["button", "backspace", "alt-up", "mouse", "ring"] {
                if kind == "archive_cold" && parent != "backspace" {
                    continue;
                }
                let mut app = setup(portable_layout);
                configure(&mut app);
                if kind == "image_book" {
                    app.settings.auto_fullscreen_zip_pdf = true;
                    app.settings.auto_fullscreen_image_folders = true;
                }
                let (path, history_kind) = fixture(&mut app, kind);
                add_history(&app, &path, history_kind);
                app.load_folder(path.parent().unwrap().to_owned());
                settle(&mut app);
                eprintln!(
                    "1473 case={kind} entrance={entrance:?} parent={parent} data_dir={:?}",
                    crate::data_dir::get()
                );
                if matches!(entrance, Entrance::FolderBar) {
                    // 履歴▼ keeps convertible sources intact; other paths use the ordinary resolver.
                    let target =
                        if history_kind == crate::reading_history_db::ReadingHistoryKind::Archive {
                            path.clone()
                        } else {
                            crate::folder_tree::resolve_openable_path(&path).unwrap()
                        };
                    apply_on(
                        &mut app,
                        Some(crate::ui_main::AddressBarNav::Direct(
                            target,
                            StartupListIntent::ExplicitList,
                        )),
                        "address",
                    );
                } else {
                    enter(&mut app, entrance);
                    trace(&app, "reading-history");
                    app.selected = app
                        .items
                        .iter()
                        .position(|it| it.container_path() == Some(path.as_path()));
                    assert!(app.selected.is_some());
                    let nav = if matches!(parent, "button" | "mouse") {
                        eprintln!("1473 open=double-click");
                        let idx = app.selected.unwrap();
                        crate::ui_main::ratingnav649_pointer_open_for_test(&mut app, idx, false)
                    } else {
                        eprintln!("1473 open=Enter");
                        key(&mut app, egui::Key::Enter, egui::Modifiers::NONE)
                    };
                    eprintln!("1473 open-intent={nav:?}");
                    trace(&app, "open-accepted-before-adoption");
                    apply_on(
                        &mut app,
                        nav,
                        if matches!(parent, "button" | "mouse") {
                            "grid"
                        } else {
                            "keyboard"
                        },
                    );
                }
                trace(&app, "opened");
                assert_eq!(app.effective_folder(), Some(path.clone()));
                assert!(!app.items_are_reading_history_view);
                let page = app
                    .items
                    .iter()
                    .position(|item| {
                        matches!(
                            item,
                            GridItem::Image(_)
                                | GridItem::ZipImage { .. }
                                | GridItem::PdfPage { .. }
                        )
                    })
                    .expect("fixture must adopt readable pages");
                let recorded_target = app.reading_history_target_for_page(page).unwrap();
                eprintln!("1473 record-target={recorded_target:?}");
                assert_eq!(
                    recorded_target.0, path,
                    "history must record the logical source"
                );
                if app.fullscreen_idx.is_some() {
                    // Parent operations below are on the page list. No native window is created.
                    app.close_fullscreen();
                    settle(&mut app);
                }
                let ctx = egui::Context::default();
                let nav = match parent {
                    "button" => app.resolve_grid_parent_nav(),
                    "backspace" => key(&mut app, egui::Key::Backspace, egui::Modifiers::NONE),
                    "alt-up" => key(&mut app, egui::Key::ArrowUp, egui::Modifiers::ALT),
                    "mouse" => {
                        app.settings
                            .ring_shortcuts
                            .mouse_button_profile_mut(RingShortcutContext::Grid)
                            .back = RingActionId::GridParentFolder;
                        app.apply_mouse_button(
                            &ctx,
                            MouseButtonSlot::Back,
                            ActionSurface::MainWindow,
                            "1473-parent",
                        )
                    }
                    "ring" => app.apply_ring_action(
                        &ctx,
                        RingShortcutContext::Grid,
                        RingActionId::GridParentFolder,
                        "1473-parent",
                    ),
                    _ => unreachable!(),
                };
                assert!(nav.is_some(), "parent input was not accepted");
                eprintln!("1473 parent-intent={nav:?}");
                apply_on(
                    &mut app,
                    nav,
                    if parent == "button" {
                        "address"
                    } else {
                        "keyboard"
                    },
                );
                trace(&app, "parent-adopted");
                if matches!(entrance, Entrance::FolderBar) {
                    assert_eq!(app.effective_folder().as_deref(), path.parent());
                } else {
                    assert!(
                        app.items_are_reading_history_view,
                        "must return to reading history"
                    );
                    assert_eq!(app.current_folder, Some(reading_history_synthetic_path()));
                }
                assert_ne!(
                    app.current_folder,
                    Some(crate::data_dir::get().join("cache"))
                );
            }
        }
    }
}

#[test]
fn section1473_folder() {
    matrix("folder");
}
#[test]
fn section1473_image_book() {
    matrix("image_book");
}
#[test]
fn section1473_zip() {
    matrix("zip");
}
#[test]
fn section1473_pdf() {
    matrix("pdf");
}
#[test]
fn section1473_converted_archive_cache() {
    matrix("archive");
}
#[test]
fn section1473_epub_cache() {
    matrix("epub");
}

#[test]
fn section1473_cold_solid_rar_conversion() {
    matrix("archive_cold");
}

#[test]
fn section1473_zip_inner_parent_then_history() {
    let mut app = setup(true);
    configure(&mut app);
    let root = app.tmp.path().join("library");
    std::fs::create_dir_all(&root).unwrap();
    let path = root.join("chapters.zip");
    write_zip(&path, &["root.png", "chapter/page.png"]);
    add_history(
        &app,
        &path,
        crate::reading_history_db::ReadingHistoryKind::Zip,
    );
    enter(&mut app, Entrance::Menu);
    app.selected = Some(0);
    let nav = key(&mut app, egui::Key::Enter, egui::Modifiers::NONE);
    apply(&mut app, nav);
    app.zip_nav_enter("chapter/");
    settle(&mut app);
    trace(&app, "zip-child");
    assert!(app.reading_history_back_nav().is_none());
    let nav = key(&mut app, egui::Key::Backspace, egui::Modifiers::NONE);
    apply(&mut app, nav);
    trace(&app, "zip-root");
    assert!(app.zip_nav.as_ref().unwrap().at_root());
    assert_eq!(app.effective_folder(), Some(path));
    let nav = key(&mut app, egui::Key::Backspace, egui::Modifiers::NONE);
    apply(&mut app, nav);
    assert!(app.items_are_reading_history_view);
}

#[test]
fn section1473_v490_folder_enter_producer_boundary() {
    // The v4.9.0 handle_keyboard Folder arm ended in grid_physical_navigation without
    // note_reading_history_open. That function is byte-identical in v4.9.0 and master.
    // This tests that unchanged boundary, not a complete v4.9.0 build.
    let mut app = setup(true);
    configure(&mut app);
    let (path, kind) = fixture(&mut app, "folder");
    add_history(&app, &path, kind);
    enter(&mut app, Entrance::Menu);
    let nav = app.grid_physical_navigation(0, path.clone(), false);
    apply(&mut app, Some(nav));
    trace(&app, "v490-enter-boundary");
    assert!(app.reading_history_return_from.is_none());
    let nav = app.resolve_grid_parent_nav();
    apply(&mut app, nav);
    assert_eq!(app.effective_folder().as_deref(), path.parent());
    assert!(!app.items_are_reading_history_view);
    assert_ne!(
        app.effective_folder(),
        Some(crate::data_dir::get().join("cache"))
    );
}

fn root_parent_input(app: &mut App, route: &str) -> Option<crate::ui_main::AddressBarNav> {
    let ctx = egui::Context::default();
    match route {
        "button" => root_parent_button(app),
        "backspace" => key(app, egui::Key::Backspace, egui::Modifiers::NONE),
        "alt-up" => key(app, egui::Key::ArrowUp, egui::Modifiers::ALT),
        "mouse" => {
            app.settings
                .ring_shortcuts
                .mouse_button_profile_mut(RingShortcutContext::Grid)
                .back = RingActionId::GridParentFolder;
            app.apply_mouse_button(
                &ctx,
                MouseButtonSlot::Back,
                ActionSurface::MainWindow,
                "1473-root-parent",
            )
        }
        "ring" => app.apply_ring_action(
            &ctx,
            RingShortcutContext::Grid,
            RingActionId::GridParentFolder,
            "1473-root-parent",
        ),
        _ => unreachable!(),
    }
}

#[test]
fn section1473_reading_history_root_parent_never_opens_internal_data() {
    for portable in [false, true] {
        for returned_from_book in [false, true] {
            for entrance in [Entrance::Menu, Entrance::Ring, Entrance::Mouse] {
                for route in ["button", "backspace", "alt-up", "mouse", "ring"] {
                    let mut app = setup(portable);
                    configure(&mut app);
                    let (book, kind) = fixture(&mut app, "zip");
                    let origin = book.parent().unwrap().to_owned();
                    app.load_folder(origin.clone());
                    settle(&mut app);
                    let origin_selection = app
                        .selected
                        .and_then(|index| app.items.get(index))
                        .and_then(GridItem::container_path)
                        .map(Path::to_owned);
                    add_history(&app, &book, kind);
                    enter(&mut app, entrance);
                    if returned_from_book {
                        app.selected = Some(0);
                        let nav = key(&mut app, egui::Key::Enter, egui::Modifiers::NONE);
                        apply(&mut app, nav);
                        assert_eq!(app.effective_folder(), Some(book.clone()));
                        let nav = root_parent_input(&mut app, route);
                        apply_on(
                            &mut app,
                            nav,
                            if route == "button" {
                                "address"
                            } else {
                                "keyboard"
                            },
                        );
                        assert!(app.items_are_reading_history_view);
                    }
                    eprintln!(
                        "1473 root-case portable={portable} returned={returned_from_book} entrance={entrance:?} route={route} back={:?}",
                        app.folder_history_back_target()
                    );
                    let data_dir = crate::data_dir::get();
                    let nav = root_parent_input(&mut app, route);
                    eprintln!("1473 root-parent-intent={nav:?}");
                    apply_on(
                        &mut app,
                        nav,
                        if route == "button" {
                            "address"
                        } else {
                            "keyboard"
                        },
                    );
                    let rows: Vec<_> = app
                        .items
                        .iter()
                        .map(|item| item.name().into_owned())
                        .collect();
                    let selected = app
                        .selected
                        .and_then(|index| app.items.get(index))
                        .and_then(GridItem::container_path)
                        .map(Path::to_owned);
                    eprintln!(
                        "1473 root-parent-adopted logical={:?} selected={selected:?} rows={rows:?}",
                        app.effective_folder()
                    );
                    assert!(
                        !app.effective_folder().is_some_and(|path| {
                            path.is_dir()
                                && crate::path_key::physical_path_is_within(&path, &data_dir)
                        }),
                        "synthetic root parent entered internal data"
                    );
                    assert_eq!(
                        app.effective_folder(),
                        Some(origin),
                        "one parent must exit to the entrance: {portable}/{returned_from_book}/{entrance:?}/{route}"
                    );
                    assert_eq!(
                        selected, origin_selection,
                        "entrance selection must be restored"
                    );
                }
            }
        }
    }
}

#[test]
fn section1473_returned_history_back_candidate_consumes_self_marker_first() {
    let mut app = setup(true);
    configure(&mut app);
    let (book, kind) = fixture(&mut app, "zip");
    let origin = book.parent().unwrap().to_owned();
    app.load_folder(origin.clone());
    settle(&mut app);
    add_history(&app, &book, kind);
    enter(&mut app, Entrance::Menu);
    app.selected = Some(0);
    let nav = key(&mut app, egui::Key::Enter, egui::Modifiers::NONE);
    apply(&mut app, nav);
    let nav = root_parent_input(&mut app, "backspace");
    apply(&mut app, nav);
    assert!(app.items_are_reading_history_view);
    assert!(matches!(
        app.folder_history_back_target(),
        Some(FolderNavHistoryTarget::ReadingHistory(_))
    ));
    app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
    settle(&mut app);
    eprintln!(
        "1473 candidate-HistoryBack-first={:?} back={:?}",
        app.effective_folder(),
        app.folder_history_back_target()
    );
    assert!(
        app.items_are_reading_history_view,
        "a raw HistoryBack does not exit returned root"
    );
    app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
    settle(&mut app);
    eprintln!(
        "1473 candidate-HistoryBack-second={:?}",
        app.effective_folder()
    );
    assert_eq!(app.effective_folder(), Some(origin));
}

#[test]
fn section1473_folder_bar_second_parent_keeps_physical_ancestors() {
    for portable in [false, true] {
        for route in ["button", "backspace", "alt-up", "mouse", "ring"] {
            let mut app = setup(portable);
            configure(&mut app);
            let (book, _) = fixture(&mut app, "zip");
            let origin = book.parent().unwrap().to_owned();
            let grandparent = origin.parent().unwrap().to_owned();
            app.load_folder(origin.clone());
            settle(&mut app);
            let openable = crate::folder_tree::resolve_openable_path(&book).unwrap();
            apply_on(
                &mut app,
                Some(crate::ui_main::AddressBarNav::Direct(
                    openable,
                    StartupListIntent::ExplicitList,
                )),
                "address",
            );
            for expected in [&origin, &grandparent] {
                let nav = root_parent_input(&mut app, route);
                apply_on(
                    &mut app,
                    nav,
                    if route == "button" {
                        "address"
                    } else {
                        "keyboard"
                    },
                );
                assert_eq!(app.effective_folder().as_deref(), Some(expected.as_path()));
                assert!(!app.items_are_reading_history_view);
            }
            let selected = app
                .selected
                .and_then(|index| app.items.get(index))
                .and_then(GridItem::container_path);
            eprintln!(
                "1473 folder-bar-second-parent portable={portable} route={route} logical={:?} selected={selected:?}",
                app.effective_folder()
            );
            assert_eq!(selected, Some(origin.as_path()));
            let nav = key(&mut app, egui::Key::Enter, egui::Modifiers::NONE);
            apply(&mut app, nav);
            assert_eq!(app.effective_folder(), Some(origin));
        }
    }
}

fn adopted_history_root(app: &App) -> top_level_grid_view::ReadingHistoryRootRestore {
    match app
        .current_top_level_restore_snapshot()
        .expect("adopted location")
    {
        top_level_grid_view::TopLevelGridRestore::ReadingHistory(root) => root,
        other => panic!("expected a typed history root, got {other:?}"),
    }
}

fn parent_and_settle(app: &mut App, route: &str) {
    let before = app.folder_nav_history_snapshot();
    let _ = app.grid_parent_nav_target();
    assert_eq!(
        app.folder_nav_history_snapshot(),
        before,
        "preview must not advance history"
    );
    let nav = root_parent_input(app, route);
    apply_on(
        app,
        nav,
        if route == "button" {
            "address"
        } else {
            "keyboard"
        },
    );
}

fn open_container_path(app: &mut App, path: &Path) {
    app.selected = Some(
        app.items
            .iter()
            .position(|item| {
                item.container_path()
                    .is_some_and(|candidate| crate::folder_tree::path_eq(candidate, path))
            })
            .expect("fixture must expose the requested container"),
    );
    let nav = key(app, egui::Key::Enter, egui::Modifiers::NONE);
    apply(app, nav);
    if app.fullscreen_idx.is_some() {
        app.close_fullscreen();
        settle(app);
    }
    assert_eq!(app.effective_folder().as_deref(), Some(path));
}

fn history_book_roundtrip(app: &mut App, book: &Path, route: &str) {
    let root = adopted_history_root(app);
    open_container_path(app, book);
    parent_and_settle(app, route);
    assert_eq!(
        adopted_history_root(app),
        root,
        "Return must preserve the exact visit and exit payload"
    );
}

fn nested_rating_history_case(portable: bool, roundtrip: bool, physical_child: bool, route: &str) {
    let mut app = setup(portable);
    configure(&mut app);
    let (book, kind) = fixture(&mut app, "zip");
    let (rated, _) = fixture(&mut app, "folder");
    let origin = book.parent().unwrap().to_owned();
    app.rating_db
        .as_ref()
        .unwrap()
        .set(&crate::adjustment_db::normalize_path(&rated), 3)
        .unwrap();
    app.load_folder(origin.clone());
    settle(&mut app);
    add_history(&app, &book, kind);
    enter(&mut app, Entrance::Menu);
    let h1 = adopted_history_root(&app);
    app.enter_rating_view(3);
    settle(&mut app);
    assert!(app.items_are_rating_view);
    let h1_restore = top_level_grid_view::TopLevelGridRestore::ReadingHistory(h1.clone());
    assert_eq!(
        app.rating_view_return_to.as_ref().unwrap().restore,
        h1_restore
    );
    if physical_child {
        open_container_path(&mut app, &rated);
    }
    let rating_restore = app.current_top_level_restore_snapshot().unwrap();
    enter(&mut app, Entrance::Menu);
    let h2 = adopted_history_root(&app);
    assert_ne!(
        h1.visit, h2.visit,
        "same synthetic path must not merge distinct visits"
    );
    match &h2.exit {
        top_level_grid_view::ReadingHistoryExit::RecordedEntrance(exit) => {
            assert_eq!(exit.restore, rating_restore)
        }
        other => panic!("rating entrance must be recorded: {other:?}"),
    }
    // The payload must survive the actual ZIP adoption clearing native Rating fields.
    // Also cover direct exit with no surviving native slot at all.
    app.rating_view_return_to = None;
    if roundtrip {
        history_book_roundtrip(&mut app, &book, route);
    }
    parent_and_settle(&mut app, route);
    assert_eq!(
        app.current_top_level_restore_snapshot().unwrap(),
        rating_restore
    );
    if physical_child {
        assert_eq!(app.effective_folder(), Some(rated));
        parent_and_settle(&mut app, route);
    }
    assert!(app.items_are_rating_view);
    assert_eq!(
        app.rating_view_return_to.as_ref().unwrap().restore,
        h1_restore
    );
    parent_and_settle(&mut app, route);
    assert_eq!(
        adopted_history_root(&app),
        h1,
        "Rating close must restore H1, not create a new root"
    );
    parent_and_settle(&mut app, route);
    assert_eq!(
        app.effective_folder(),
        Some(origin),
        "H1 must still exit to O"
    );
}

#[test]
fn section1473_equal_entrance_revisit_via_rating_keeps_complete_back_forward() {
    for portable in [false, true] {
        let mut app = setup(portable);
        configure(&mut app);
        let (origin, kind) = fixture(&mut app, "folder");
        app.load_folder(origin.clone());
        settle(&mut app);
        let previous = app.tmp.path().join("previous");
        std::fs::create_dir_all(&previous).unwrap();
        app.push_nav_history_entry(previous);
        let prefix = app.folder_nav_back_stack.clone();
        let entrance = app.folder_nav_current_entry().unwrap();
        add_history(&app, &origin, kind);
        enter(&mut app, Entrance::Menu);
        let root = adopted_history_root(&app);
        let history = app.folder_nav_current_entry().unwrap();
        open_container_path(&mut app, &origin);
        let revisit = app.folder_nav_current_entry().unwrap();
        assert!(matches!(
            &revisit.location,
            FolderNavHistoryTarget::Physical(_)
        ));
        assert!(App::folder_nav_targets_eq(
            &entrance.location,
            &revisit.location
        ));
        assert_eq!(entrance.route, revisit.route);
        app.enter_rating_view(3);
        settle(&mut app);
        parent_and_settle(&mut app, "backspace");
        assert_eq!(app.folder_nav_current_entry(), Some(revisit.clone()));
        parent_and_settle(&mut app, "backspace");
        assert_eq!(adopted_history_root(&app), root);
        assert_eq!(app.folder_nav_current_entry(), Some(history.clone()));
        let mut expected_back = prefix.clone();
        expected_back.extend([entrance.clone(), history.clone(), revisit.clone()]);
        assert_eq!(app.folder_nav_back_stack, expected_back);
        assert!(app.folder_nav_forward_stack.is_empty());
        let future: FolderNavHistoryEntry =
            FolderNavHistoryTarget::Path(app.tmp.path().join("future")).into();
        app.folder_nav_forward_stack.push(future.clone());
        parent_and_settle(&mut app, "backspace");
        assert_eq!(app.effective_folder(), Some(origin));
        assert_eq!(app.folder_nav_current_entry(), Some(entrance));
        assert_eq!(app.folder_nav_back_stack, prefix);
        assert_eq!(
            app.folder_nav_forward_stack,
            vec![future, history.clone(), revisit, history]
        );
    }
}

#[test]
fn section1473_both_markers_truncated_equal_revisit_keeps_entrance_destination() {
    for portable in [false, true] {
        let mut app = setup(portable);
        configure(&mut app);
        let (origin, kind) = fixture(&mut app, "folder");
        app.load_folder(origin.clone());
        settle(&mut app);
        let entrance = app.folder_nav_current_entry().unwrap();
        add_history(&app, &origin, kind);
        enter(&mut app, Entrance::Menu);
        let root = adopted_history_root(&app);
        let history = app.folder_nav_current_entry().unwrap();
        open_container_path(&mut app, &origin);
        let revisit = app.folder_nav_current_entry().unwrap();
        assert!(App::folder_nav_targets_eq(
            &entrance.location,
            &revisit.location
        ));
        assert_eq!(entrance.route, revisit.route);
        app.enter_rating_view(3);
        settle(&mut app);
        // Complete every real Rating adoption, overflowing both the entrance and H marker.
        for number in 0..MAX_FOLDER_NAV_STACK + 5 {
            app.enter_rating_view((number % 5 + 1) as u8);
            settle(&mut app);
            assert!(app.items_are_rating_view);
        }
        assert_eq!(app.folder_nav_back_stack.len(), MAX_FOLDER_NAV_STACK);
        assert!(
            app.folder_nav_back_stack
                .iter()
                .all(|entry| matches!(entry.location, FolderNavHistoryTarget::Rating(_)))
        );
        parent_and_settle(&mut app, "backspace");
        assert_eq!(app.folder_nav_current_entry(), Some(revisit.clone()));
        app.enter_rating_view(3);
        settle(&mut app);
        parent_and_settle(&mut app, "backspace");
        parent_and_settle(&mut app, "backspace");
        assert_eq!(adopted_history_root(&app), root);
        assert_eq!(app.folder_nav_current_entry(), Some(history.clone()));
        assert_eq!(app.folder_nav_back_stack.last(), Some(&revisit));
        assert!(!app.folder_nav_back_stack.iter().any(
            |entry| matches!(&entry.location, FolderNavHistoryTarget::ReadingHistory(marker)
                if marker.visit == root.visit)
        ));
        // After both markers expire beyond the 100-entry limit, an equal revisit may
        // affect Back/Forward ordering. This accepted tradeoff must still reach O;
        // do not require Direct or freeze the stacks for this rare combination.
        parent_and_settle(&mut app, "backspace");
        assert_eq!(app.effective_folder(), Some(origin));
    }
}

#[test]
fn section1473_nested_history_rating_preserves_both_visits() {
    for portable in [false, true] {
        for roundtrip in [false, true] {
            for route in ["button", "backspace", "alt-up", "mouse", "ring"] {
                nested_rating_history_case(portable, roundtrip, false, route);
            }
        }
    }
}

#[test]
fn section1473_nested_history_rating_physical_child_keeps_root_exit() {
    for portable in [false, true] {
        for roundtrip in [false, true] {
            nested_rating_history_case(portable, roundtrip, true, "backspace");
        }
    }
}

#[test]
fn section1473_expansion_entrance_owns_complete_restore() {
    for portable in [false, true] {
        for roundtrip in [false, true] {
            let mut app = setup(portable);
            configure(&mut app);
            let (book, kind) = fixture(&mut app, "zip");
            let (folder, _) = fixture(&mut app, "folder");
            let origin = book.parent().unwrap().to_owned();
            let other = app.tmp.path().join("other-root");
            std::fs::create_dir_all(&other).unwrap();
            image::RgbImage::new(3, 4)
                .save(other.join("other.png"))
                .unwrap();
            app.load_folder(origin.clone());
            settle(&mut app);
            app.start_subfolder_expansion_scan_roots(origin.clone(), vec![origin.clone(), other]);
            settle(&mut app);
            assert!(app.items_are_subfolder_expansion_view);
            assert!(
                app.folder_nav_subfolder_restore.is_none(),
                "new expansion has no generic backup slot"
            );
            app.subfolder_expansion_removed_paths
                .insert(crate::adjustment_db::normalize_path(&folder));
            let expected = app.current_top_level_restore_snapshot().unwrap();
            add_history(&app, &book, kind);
            enter(&mut app, Entrance::Menu);
            let root = adopted_history_root(&app);
            app.folder_nav_subfolder_restore = None;
            if roundtrip {
                history_book_roundtrip(&mut app, &book, "backspace");
            }
            parent_and_settle(&mut app, "backspace");
            assert!(app.items_are_subfolder_expansion_view);
            assert_eq!(app.current_top_level_restore_snapshot().unwrap(), expected);
            assert!(
                !app.items
                    .iter()
                    .any(|item| item.container_path() == Some(folder.as_path())),
                "removed path must not reappear"
            );
            // Forward returns to the retained root payload, independent of native slots.
            app.dispatch_main_folder_history_input(FolderHistoryDirection::Forward);
            settle(&mut app);
            assert_eq!(adopted_history_root(&app), root);
            app.folder_nav_subfolder_restore = None;
            parent_and_settle(&mut app, "backspace");
            assert_eq!(app.current_top_level_restore_snapshot().unwrap(), expected);
            parent_and_settle(&mut app, "backspace");
            assert_eq!(app.effective_folder(), Some(origin));
        }
    }
}

#[test]
fn section1473_rating_exit_keeps_its_own_complete_origin() {
    for portable in [false, true] {
        for entrance in ["folder", "drives", "subfolder-expansion"] {
            for roundtrip in [false, true] {
                let mut app = setup(portable);
                configure(&mut app);
                let (book, kind) = fixture(&mut app, "zip");
                let origin = book.parent().unwrap().to_owned();
                app.load_folder(origin.clone());
                settle(&mut app);
                if entrance != "folder" {
                    enter_other_root(&mut app, entrance, &origin);
                }
                let before_rating = app.current_top_level_restore_snapshot().unwrap();
                app.enter_rating_view(3);
                settle(&mut app);
                let rating_restore = app.current_top_level_restore_snapshot().unwrap();
                add_history(&app, &book, kind);
                enter(&mut app, Entrance::Menu);
                app.rating_view_return_to = None;
                if roundtrip {
                    history_book_roundtrip(&mut app, &book, "backspace");
                }
                parent_and_settle(&mut app, "backspace");
                assert_eq!(
                    app.current_top_level_restore_snapshot().unwrap(),
                    rating_restore
                );
                parent_and_settle(&mut app, "backspace");
                assert_eq!(
                    app.current_top_level_restore_snapshot().unwrap(),
                    before_rating
                );
                if entrance == "subfolder-expansion" {
                    parent_and_settle(&mut app, "backspace");
                    assert_eq!(app.effective_folder(), Some(origin));
                }
            }
        }
    }
}

#[test]
fn section1473_bookmark_entrance_direct_exit_must_not_cycle_parent() {
    let mut results = Vec::new();
    for portable in [false, true] {
        let mut app = setup(portable);
        configure(&mut app);
        let (book, _) = fixture(&mut app, "zip");
        let origin = book.parent().unwrap().to_owned();
        app.load_folder(origin.clone());
        settle(&mut app);
        enter_other_root(&mut app, "bookmarks", &origin);
        let bookmarks = app.current_top_level_restore_snapshot().unwrap();
        enter(&mut app, Entrance::Menu);
        let history = adopted_history_root(&app);
        parent_and_settle(&mut app, "backspace");
        assert_eq!(
            app.current_top_level_restore_snapshot(),
            Some(bookmarks.clone())
        );
        let after_exit = app.folder_nav_history_snapshot();
        parent_and_settle(&mut app, "backspace");
        let after_two = app.effective_folder();
        let returned_to_history = matches!(app.current_top_level_restore_snapshot(),
            Some(top_level_grid_view::TopLevelGridRestore::ReadingHistory(ref root))
                if root == &history);
        parent_and_settle(&mut app, "backspace");
        let returned_to_bookmarks = app.current_top_level_restore_snapshot() == Some(bookmarks);
        let repeated_cursor = app.folder_nav_history_snapshot() == after_exit;
        results.push((
            portable,
            after_two == Some(origin),
            returned_to_history,
            returned_to_bookmarks,
            repeated_cursor,
        ));
    }
    assert!(
        results.iter().all(|result| result.1),
        "(portable, reaches O, returns H, returns Bookmarks, complete cursor repeats): {results:?}"
    );
}

#[test]
fn section1473_other_owned_entrances_survive_history_and_book() {
    for portable in [false, true] {
        for kind in ["bookmarks", "smart", "collection", "drives"] {
            for roundtrip in [false, true] {
                let mut app = setup(portable);
                configure(&mut app);
                let (book, history_kind) = fixture(&mut app, "zip");
                let origin = book.parent().unwrap().to_owned();
                app.load_folder(origin.clone());
                settle(&mut app);
                enter_other_root(&mut app, kind, &origin);
                let expected = app.current_top_level_restore_snapshot().unwrap();
                add_history(&app, &book, history_kind);
                enter(&mut app, Entrance::Menu);
                if roundtrip {
                    history_book_roundtrip(&mut app, &book, "backspace");
                }
                parent_and_settle(&mut app, "backspace");
                assert_eq!(
                    app.current_top_level_restore_snapshot().unwrap(),
                    expected,
                    "complete entrance must survive: {kind}/{portable}/{roundtrip}"
                );
                if kind == "bookmarks" {
                    parent_and_settle(&mut app, "backspace");
                    assert_eq!(app.effective_folder(), Some(origin));
                } else {
                    let before = app.folder_nav_history_snapshot();
                    parent_and_settle(&mut app, "backspace");
                    assert_eq!(app.current_top_level_restore_snapshot().unwrap(), expected);
                    assert_eq!(
                        app.folder_nav_history_snapshot(),
                        before,
                        "synthetic root keeps its existing stop rule"
                    );
                }
            }
        }
    }
}

#[test]
fn section1473_search_transfers_complete_canonical_origin() {
    for portable in [false, true] {
        for origin_kind in ["rating", "subfolder-expansion"] {
            for search in ["global-search", "favorite-search", "tag"] {
                let mut app = setup(portable);
                configure(&mut app);
                let (book, history_kind) = fixture(&mut app, "zip");
                let origin = book.parent().unwrap().to_owned();
                app.load_folder(origin.clone());
                settle(&mut app);
                enter_other_root(&mut app, origin_kind, &origin);
                let expected = app.current_top_level_restore_snapshot().unwrap();
                enter_other_root(&mut app, search, &origin);
                add_history(&app, &book, history_kind);
                // File-menu admission is intentionally different from blocked location jumps.
                enter(&mut app, Entrance::Menu);
                app.rating_view_return_to = None;
                app.folder_nav_subfolder_restore = None;
                history_book_roundtrip(&mut app, &book, "backspace");
                parent_and_settle(&mut app, "backspace");
                assert!(!app.global_search.active && !app.favsearch.active && !app.tag_view.active);
                assert_eq!(
                    app.current_top_level_restore_snapshot().unwrap(),
                    expected,
                    "search must transfer its complete canonical origin: {origin_kind}/{search}"
                );
                parent_and_settle(&mut app, "backspace");
                assert_eq!(app.effective_folder(), Some(origin));
            }
        }
    }
}

#[test]
fn section1473_snapshot_keeps_input_specific_admission_and_parent_owner() {
    for portable in [false, true] {
        for route in ["backspace", "alt-up", "mouse", "ring"] {
            let mut app = setup(portable);
            configure(&mut app);
            let (book, kind) = fixture(&mut app, "zip");
            let origin = book.parent().unwrap().to_owned();
            app.load_folder(origin.clone());
            settle(&mut app);
            let expected = app.capture_top_level_origin().unwrap();
            app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
            settle(&mut app);
            assert!(app.is_snapshot_active());
            let ctx = egui::Context::default();
            assert!(
                app.apply_ring_action(
                    &ctx,
                    RingShortcutContext::Grid,
                    RingActionId::OpenLocationReadingHistory,
                    "1473-snapshot"
                )
                .is_none()
            );
            assert!(app.is_snapshot_active());
            assert_eq!(app.effective_folder(), Some(origin.clone()));
            add_history(&app, &book, kind);
            // The file menu does not impose the location-menu Snapshot prohibition.
            enter(&mut app, Entrance::Menu);
            assert!(app.is_snapshot_active());
            let visible = app.capture_top_level_origin().unwrap();
            let history = app.folder_nav_history_snapshot();
            assert!(root_parent_button(&mut app).is_none());
            assert!(app.is_snapshot_active());
            assert_eq!(app.capture_top_level_origin().unwrap(), visible);
            assert_eq!(app.folder_nav_history_snapshot(), history);
            settle(&mut app);
            parent_and_settle(&mut app, route);
            assert!(!app.is_snapshot_active());
            assert_eq!(app.effective_folder(), Some(origin));
            assert!(!app.items_are_reading_history_view);
            assert!(matches!(
                app.top_level_grid_view.surface(),
                top_level_grid_view::TopLevelGridSurface::Folder
            ));
            assert_eq!(app.capture_top_level_origin().unwrap(), expected);
            assert!(app.projection_recipe.extracted_surface().is_none());
        }
    }
}

#[test]
fn section1473_snapshot_same_path_history_has_distinct_root_and_exits_to_its_captured_visit() {
    for portable in [false, true] {
        for route in ["backspace", "alt-up", "mouse", "ring"] {
            let mut app = setup(portable);
            configure(&mut app);
            let (book, kind) = fixture(&mut app, "zip");
            let origin = book.parent().unwrap().to_owned();
            app.load_folder(origin.clone());
            settle(&mut app);
            add_history(&app, &book, kind);
            enter(&mut app, Entrance::Menu);
            let h1 = app.capture_top_level_origin().unwrap();
            let root1 = adopted_history_root(&app);
            app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
            settle(&mut app);
            assert!(app.is_snapshot_active());
            enter(&mut app, Entrance::Menu);
            let root2 = adopted_history_root(&app);
            assert_ne!(root1.visit, root2.visit);
            assert!(
                matches!(&root2.exit, top_level_grid_view::ReadingHistoryExit::RecordedEntrance(entrance) if entrance.as_ref() == &h1)
            );
            assert_eq!(app.current_folder, Some(reading_history_synthetic_path()));
            assert_eq!(
                app.snapshot.as_ref().unwrap().origin,
                reading_history_synthetic_path()
            );
            // Menu on the already visible H2 retains its visit even while fixed state exists.
            enter(&mut app, Entrance::Menu);
            assert_eq!(adopted_history_root(&app), root2);
            assert!(root_parent_button(&mut app).is_none());
            assert!(app.is_snapshot_active());
            assert_eq!(adopted_history_root(&app), root2);
            settle(&mut app);
            parent_and_settle(&mut app, route);
            assert!(!app.is_snapshot_active());
            assert_eq!(app.capture_top_level_origin().unwrap(), h1);
            assert_eq!(adopted_history_root(&app), root1);
            assert!(app.items_are_reading_history_view);
            parent_and_settle(&mut app, route);
            assert_eq!(app.effective_folder(), Some(origin));
            assert!(!app.items_are_reading_history_view);
        }
    }
}

#[test]
fn section1473_snapshot_exit_cancel_keeps_visible_root_and_ends_fixed_scope() {
    for portable in [false, true] {
        for recorded in [false, true] {
            let mut app = setup(portable);
            configure(&mut app);
            let (book, kind) = fixture(&mut app, "zip");
            let origin = book.parent().unwrap().to_owned();
            app.load_folder(origin.clone());
            settle(&mut app);
            app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
            settle(&mut app);
            add_history(&app, &book, kind);
            enter(&mut app, Entrance::Menu);
            let root = app.capture_top_level_origin().unwrap();
            if !recorded {
                for number in 0..105 {
                    let later = app.tmp.path().join(format!("later-{number}"));
                    std::fs::create_dir_all(&later).unwrap();
                    app.push_nav_history_entry(later);
                }
            }
            let before = app.folder_nav_history_snapshot();
            app.exit_reading_history_root();
            assert!(!app.is_snapshot_active());
            assert_eq!(app.capture_top_level_origin().unwrap(), root);
            assert_eq!(app.folder_nav_history_snapshot(), before);
            assert!(
                app.top_level_grid_view
                    .history_navigation_transition()
                    .is_some()
            );
            app.retire_main_list_requests_for_surface_switch();
            settle(&mut app);
            assert!(!app.is_snapshot_active());
            assert_eq!(app.capture_top_level_origin().unwrap(), root);
            assert_eq!(app.folder_nav_history_snapshot(), before);
            parent_and_settle(&mut app, "backspace");
            assert_eq!(app.effective_folder(), Some(origin));
            assert!(!app.items_are_reading_history_view);
        }
    }
}

#[test]
fn section1473_snapshot_exit_restores_complete_native_canonical_owners() {
    for portable in [false, true] {
        for kind in [
            "rating",
            "bookmarks",
            "subfolder-expansion",
            "smart",
            "collection",
        ] {
            let mut app = setup(portable);
            configure(&mut app);
            let (book, history_kind) = fixture(&mut app, "zip");
            let origin = book.parent().unwrap().to_owned();
            let folder = origin.join("owned-source");
            std::fs::create_dir_all(folder.join("child")).unwrap();
            app.rating_db
                .as_ref()
                .unwrap()
                .set(&crate::adjustment_db::normalize_path(&folder), 3)
                .unwrap();
            let video = origin.join("marker.mp4");
            std::fs::write(&video, []).unwrap();
            app.video_bookmark_db
                .as_ref()
                .unwrap()
                .add(&video, 1.0, None, &[])
                .unwrap();
            app.load_folder(origin.clone());
            settle(&mut app);
            enter_other_root(&mut app, kind, &origin);
            let expected = app.capture_top_level_origin().unwrap();
            app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
            settle(&mut app);
            assert!(app.is_snapshot_active(), "{kind}");
            add_history(&app, &book, history_kind);
            enter(&mut app, Entrance::Menu);
            parent_and_settle(&mut app, "backspace");
            assert!(!app.is_snapshot_active());
            assert!(!app.items_are_reading_history_view);
            assert_eq!(app.capture_top_level_origin().unwrap(), expected, "{kind}");
        }
    }
}

#[test]
fn section1473_folder_descendant_does_not_resurrect_expired_book_return() {
    for portable in [false, true] {
        let mut app = setup(portable);
        configure(&mut app);
        let (folder, kind) = fixture(&mut app, "folder");
        let origin = folder.parent().unwrap().to_owned();
        app.load_folder(origin.clone());
        settle(&mut app);
        add_history(&app, &folder, kind);
        enter(&mut app, Entrance::Menu);
        let root = adopted_history_root(&app);
        open_container_path(&mut app, &folder);
        open_container_path(&mut app, &folder.join("child"));
        assert!(app.reading_history_return_from.is_none());
        parent_and_settle(&mut app, "backspace");
        assert_eq!(app.effective_folder(), Some(folder));
        parent_and_settle(&mut app, "backspace");
        assert_eq!(app.effective_folder(), Some(origin.clone()));
        for _ in 0..8 {
            if app.items_are_reading_history_view {
                break;
            }
            app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
            settle(&mut app);
        }
        assert_eq!(adopted_history_root(&app), root);
        parent_and_settle(&mut app, "backspace");
        assert_eq!(app.effective_folder(), Some(origin));
    }
}

#[test]
fn section1473_marker_presence_selects_back_or_direct_without_losing_entrance_position() {
    for portable in [false, true] {
        for mode in ["recorded", "search", "truncated"] {
            let mut app = setup(portable);
            configure(&mut app);
            let (book, kind) = fixture(&mut app, "zip");
            let origin = book.parent().unwrap().to_owned();
            for number in 0..70 {
                let path = origin.join(format!("folder-{number:03}"));
                std::fs::create_dir_all(path).unwrap();
            }
            app.load_folder(origin.clone());
            settle(&mut app);
            let selected_path = origin.join("folder-040");
            app.selected = Some(
                app.items
                    .iter()
                    .position(|item| item.container_path() == Some(selected_path.as_path()))
                    .unwrap(),
            );
            app.grid_scroll.set_position(360.0);
            for number in 0..2 {
                let path = app.tmp.path().join(format!("previous-{number}"));
                std::fs::create_dir_all(&path).unwrap();
                app.push_nav_history_entry(path);
            }
            let old_prefix = app.folder_nav_back_stack.clone();
            add_history(&app, &book, kind);
            if mode == "search" {
                app.open_global_search();
                app.replace_search_view_items(Vec::new(), Vec::new());
                settle(&mut app);
            }
            enter(&mut app, Entrance::Menu);
            let history_entry = app.folder_nav_current_entry().unwrap();
            if mode == "truncated" {
                // Use the real bounded-stack producer, rather than fabricating a truncated vector.
                for number in 0..105 {
                    let path = app.tmp.path().join(format!("later-{number:03}"));
                    std::fs::create_dir_all(&path).unwrap();
                    app.push_nav_history_entry(path);
                }
                assert_eq!(app.folder_nav_back_stack.len(), 100);
                assert!(
                    !app.folder_nav_back_stack
                        .iter()
                        .any(|entry| matches!(&entry.location,
                    FolderNavHistoryTarget::Path(path) if path == &origin))
                );
            }
            let future = app.tmp.path().join("future");
            std::fs::create_dir_all(&future).unwrap();
            let future_entry: FolderNavHistoryEntry = FolderNavHistoryTarget::Path(future).into();
            app.folder_nav_forward_stack.push(future_entry.clone());
            let before_back = app.folder_nav_back_stack.clone();
            parent_and_settle(&mut app, "backspace");
            assert_eq!(app.effective_folder(), Some(origin.clone()), "mode={mode}");
            assert_eq!(
                app.selected
                    .and_then(|index| app.items.get(index))
                    .and_then(GridItem::container_path),
                Some(selected_path.as_path()),
                "mode={mode}"
            );
            assert_eq!(app.grid_scroll.offset(), 360.0, "mode={mode}");
            if mode == "recorded" {
                assert_eq!(app.folder_nav_back_stack, old_prefix);
                assert_eq!(
                    app.folder_nav_forward_stack,
                    vec![future_entry, history_entry]
                );
            } else {
                let mut expected_back = before_back;
                App::push_folder_nav_stack(&mut expected_back, history_entry);
                assert_eq!(
                    app.folder_nav_back_stack, expected_back,
                    "markerless exit uses ordinary Direct"
                );
                assert!(
                    app.folder_nav_forward_stack.is_empty(),
                    "ordinary Direct clears forward"
                );
            }
        }
    }
}

#[test]
fn section1473_history_without_entrance_keeps_drive_exit_after_book_and_replay() {
    for portable in [false, true] {
        for roundtrip in [false, true] {
            let mut app = setup(portable);
            configure(&mut app);
            let (book, kind) = fixture(&mut app, "zip");
            assert!(app.current_top_level_restore_snapshot().is_none());
            add_history(&app, &book, kind);
            enter(&mut app, Entrance::Menu);
            let root = adopted_history_root(&app);
            assert!(matches!(
                root.exit,
                top_level_grid_view::ReadingHistoryExit::DriveList
            ));
            if roundtrip {
                history_book_roundtrip(&mut app, &book, "backspace");
            }
            parent_and_settle(&mut app, "backspace");
            assert!(app.items_are_drive_list);
            app.dispatch_main_folder_history_input(FolderHistoryDirection::Back);
            settle(&mut app);
            assert_eq!(adopted_history_root(&app), root);
            parent_and_settle(&mut app, "backspace");
            assert!(app.items_are_drive_list);
        }
    }
}

#[test]
fn section1473_bookmark_entrance_restores_stable_key_and_scroll_then_its_parent() {
    for portable in [false, true] {
        let mut app = setup(portable);
        configure(&mut app);
        let (book, kind) = fixture(&mut app, "zip");
        let origin = book.parent().unwrap().to_owned();
        app.load_folder(origin.clone());
        settle(&mut app);
        for number in 0..45 {
            let path = origin.join(format!("video-{number:03}.mp4"));
            std::fs::write(&path, []).unwrap();
            app.video_bookmark_db
                .as_ref()
                .unwrap()
                .add(&path, number as f64, None, &[])
                .unwrap();
        }
        app.open_bookmark_browser();
        settle(&mut app);
        assert_eq!(app.bookmark_browser_rows.len(), 45);
        app.selected = Some(31);
        app.grid_scroll.set_position(280.0);
        let expected = app.current_top_level_restore_snapshot().unwrap();
        let key = app.bookmark_browser_rows[31].stable_key();
        add_history(&app, &book, kind);
        enter(&mut app, Entrance::Menu);
        history_book_roundtrip(&mut app, &book, "backspace");
        parent_and_settle(&mut app, "backspace");
        assert_eq!(app.current_top_level_restore_snapshot().unwrap(), expected);
        assert_eq!(
            app.selected
                .map(|index| app.bookmark_browser_rows[index].stable_key()),
            Some(key)
        );
        assert_eq!(app.grid_scroll.offset(), 280.0);
        parent_and_settle(&mut app, "backspace");
        assert_eq!(app.effective_folder(), Some(origin));
    }
}

#[test]
fn section1473_smart_and_collection_physical_entrances_keep_native_root_and_origin() {
    for portable in [false, true] {
        for kind in ["smart", "collection"] {
            for roundtrip in [false, true] {
                let mut app = setup(portable);
                configure(&mut app);
                let (book, history_kind) = fixture(&mut app, "zip");
                let origin = book.parent().unwrap().to_owned();
                let source = origin.join("owned-source");
                std::fs::create_dir_all(source.join("child")).unwrap();
                app.load_folder(origin.clone());
                settle(&mut app);
                enter_other_root(&mut app, kind, &origin);
                let root = app.current_top_level_restore_snapshot().unwrap();
                open_container_path(&mut app, &source);
                let physical = app.current_top_level_restore_snapshot().unwrap();
                add_history(&app, &book, history_kind);
                enter(&mut app, Entrance::Menu);
                if roundtrip {
                    history_book_roundtrip(&mut app, &book, "backspace");
                }
                parent_and_settle(&mut app, "backspace");
                assert_eq!(app.current_top_level_restore_snapshot().unwrap(), physical);
                parent_and_settle(&mut app, "backspace");
                assert_eq!(app.current_top_level_restore_snapshot().unwrap(), root);
                // The native temporary-search return must still point to this root.
                enter_other_root(&mut app, "global-search", &origin);
                app.close_global_search();
                settle(&mut app);
                assert_eq!(app.current_top_level_restore_snapshot().unwrap(), root);
            }
        }
    }
}

#[test]
fn section1473_snapshot_subset_cannot_overwrite_ordinary_entrance_position() {
    for portable in [false, true] {
        let mut app = setup(portable);
        configure(&mut app);
        let (book, _) = fixture(&mut app, "zip");
        let origin = book.parent().unwrap().to_owned();
        for number in 0..70 {
            std::fs::create_dir_all(origin.join(format!("folder-{number:03}"))).unwrap();
        }
        app.load_folder(origin.clone());
        settle(&mut app);
        let selected_path = origin.join("folder-040");
        let selected = app
            .items
            .iter()
            .position(|item| item.container_path() == Some(selected_path.as_path()))
            .unwrap();
        app.selected = Some(selected);
        app.grid_scroll.set_position(360.0);
        app.visible_indices = (20..60).collect();
        app.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
        settle(&mut app);
        assert!(app.is_snapshot_active());
        let saved = app.folder_history.get(&origin).copied().unwrap();
        assert_eq!(saved, (360.0, Some(selected)));
        app.selected = Some(0);
        app.grid_scroll.set_position(40.0);
        // This is the production handoff used when a new view accepts the fixed owner.
        app.dismiss_snapshot_with_origin();
        let other = app.settings.book_root.as_ref().unwrap().join("other");
        std::fs::create_dir_all(&other).unwrap();
        app.load_folder(other);
        settle(&mut app);
        assert_eq!(app.folder_history.get(&origin).copied(), Some(saved));
        app.load_folder(origin);
        settle(&mut app);
        assert_eq!(
            app.selected
                .and_then(|index| app.items.get(index))
                .and_then(GridItem::container_path),
            Some(selected_path.as_path())
        );
        assert_eq!(app.grid_scroll.offset(), 360.0);
    }
}

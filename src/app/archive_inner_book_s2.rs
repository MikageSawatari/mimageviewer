//! S2 resume regressions using real archive/index workers and disposable profiles.
use super::tests::phase_c_support::setup_app;
use super::*;
use crate::book_resume_db::{PageIdentity, ReadingMeterValue, ReadingPosition};
use crate::book_resume_key::BookResumeKey;
use crate::settings::ResumeMode;
use std::io::Write;
use std::time::Duration;

const WAIT: Duration = Duration::from_secs(30);

fn png() -> Vec<u8> {
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(8, 12)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}
fn zip_bytes(entries: &[(&str, Vec<u8>)]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    for (name, bytes) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
fn write_pages(path: &Path, names: &[&str]) {
    let entries = names
        .iter()
        .map(|name| {
            let bytes = if name.ends_with(".jpg") {
                let mut bytes = std::io::Cursor::new(Vec::new());
                image::RgbImage::new(8, 12)
                    .write_to(&mut bytes, image::ImageFormat::Jpeg)
                    .unwrap();
                bytes.into_inner()
            } else {
                png()
            };
            (*name, bytes)
        })
        .collect::<Vec<_>>();
    std::fs::write(path, zip_bytes(&entries)).unwrap();
}
fn settle(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + WAIT;
    loop {
        app.poll_open_path_classification(ctx);
        app.poll_collection_history_transition(ctx);
        app.poll_zip_enumerate();
        app.poll_sidecar_restore(ctx);
        app.poll_book_composition_service_for_test(ctx);
        app.poll_fs_nav_lock(ctx);
        if app.zip_enumerate_pending.is_none()
            && app.top_level_grid_view.open_path_classification().is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && !app.sidecar_restore_active()
            && !app.book_composition.is_pending()
            && !app.book_composition_permission.pending()
            && !app.fs_nav_deferred_reopen_wait_active()
        {
            return;
        }
        assert!(Instant::now() < deadline, "archive resume did not settle");
        std::thread::yield_now();
    }
}
fn open_list(app: &mut App, ctx: &egui::Context, path: &Path) {
    app.indexer_init = IndexerInit::Unavailable;
    app.settings.detached_viewer_open_images_in_window = false;
    assert!(matches!(
        app.load_folder_or_convert_archive_with_auto_fullscreen(path.to_owned(), false),
        FolderOpenOutcome::Loaded | FolderOpenOutcome::Classifying
    ));
    settle(app, ctx);
    assert!(app.fullscreen_idx.is_none());
    assert!(app.zip_nav.as_ref().unwrap().at_root());
}
fn child(app: &App, prefix: &str) -> usize {
    app.items
        .iter()
        .position(
            |item| matches!(item, GridItem::ZipDir { dir_prefix, .. } if dir_prefix == prefix),
        )
        .unwrap_or_else(|| panic!("missing {prefix}: {:?}", app.items))
}
fn activate(app: &mut App, ctx: &egui::Context, prefix: &str, as_book: bool) {
    app.settings.auto_fullscreen_zip_pdf = as_book;
    let index = child(app, prefix);
    app.selected = Some(index);
    assert_eq!(
        app.activate_main_zipdir(index),
        if as_book {
            ZipDirActivationOutcome::BookRequested
        } else {
            ZipDirActivationOutcome::List
        }
    );
    settle(app, ctx);
}
fn current_entry(app: &App) -> &str {
    match &app.items[app.fullscreen_idx.expect("reader is open")] {
        GridItem::ZipImage { entry_name, .. } => entry_name,
        other => panic!("expected ZIP page, got {other:?}"),
    }
}
fn identity(source: &Path, entry: &str) -> Option<PageIdentity> {
    PageIdentity::for_item(
        &GridItem::ZipImage {
            zip_path: source.to_owned(),
            entry_name: entry.into(),
        },
        Some(source),
    )
}
fn read(app: &App, key: &BookResumeKey) -> Option<ReadingPosition> {
    let writer = app.book_resume_writer.as_ref().unwrap();
    let cutoff = writer.watermarks().admitted;
    let reply = writer
        .read_position_after(&key.storage_key(), cutoff)
        .recv_timeout(WAIT)
        .unwrap();
    reply.status.unwrap();
    assert!(reply.completed_through >= cutoff);
    reply.position
}
fn seed(app: &mut App, key: &BookResumeKey, source: &Path, entry: &str, page: usize) {
    assert!(app.persist_book_resume_key_with_identity(
        key.clone(),
        page,
        ReadingMeterValue::new(page + 1, 3),
        identity(source, entry)
    ));
}
fn record(app: &mut App, entry: &str) -> BookResumeKey {
    let index = app
        .items
        .iter()
        .position(
            |item| matches!(item, GridItem::ZipImage { entry_name, .. } if entry_name == entry),
        )
        .unwrap();
    app.record_book_resume(index);
    let key = app.current_book_resume_key().unwrap();
    assert_eq!(
        read(app, &key).unwrap().identity,
        PageIdentity::for_item(&app.items[index], app.archive_source_override.as_deref())
    );
    key
}
fn close_to_root(app: &mut App, ctx: &egui::Context) {
    app.handle_fullscreen_close_request_immediate();
    settle(app, ctx);
    assert!(app.fullscreen_idx.is_none());
    assert!(app.zip_nav.as_ref().unwrap().at_root());
}
fn target(app: &App) -> DeferredFsReopen {
    let nav = app.zip_nav.as_ref().unwrap();
    let source = app
        .archive_source_override
        .as_deref()
        .unwrap_or(&nav.tree.zip_path);
    DeferredFsReopen {
        history_trigger: HistoryTrigger::UserChosen,
        resume_slideshow: false,
        target: DeferredFsTarget::CurrentArchiveBook(ArchiveBookTarget {
            requested_prefix: nav.current().join("/"),
            scope: crate::book_scope::BookScopeKey::archive(source, nav.current_raw()).unwrap(),
            entry: ArchiveBookEntry::Activation,
        }),
        resume_to_last_page: true.into(),
        from_explicit_open: true,
        preserve_after_password_prompt: false,
    }
}

#[test]
fn section1675_inner_resume_independent_books() {
    independent_books(false);
}
#[test]
fn section1675_inner_resume_case_distinct_books() {
    independent_books(true);
}
fn independent_books(case_distinct: bool) {
    for shape in ["folder", "nested", "deep"] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("outer.zip");
        let b = if case_distinct { "a" } else { "B" };
        let (a_prefix, b_prefix, a_page, b_page): (String, String, String, String) = if shape
            == "nested"
        {
            let inner = zip_bytes(&[("001.png", png()), ("002.png", png()), ("003.png", png())]);
            let b_file = format!("{b}.zip");
            std::fs::write(
                &book,
                zip_bytes(&[("A.zip", inner.clone()), (&b_file, inner)]),
            )
            .unwrap();
            (
                "A.zip/".into(),
                format!("{b}.zip/"),
                "A.zip/002.png".into(),
                format!("{b}.zip/003.png"),
            )
        } else {
            let suffix = if shape == "deep" { "one/two/" } else { "" };
            let names = ["A", b]
                .into_iter()
                .flat_map(|prefix| (1..=3).map(move |n| format!("{prefix}/{suffix}{n:03}.png")))
                .collect::<Vec<_>>();
            write_pages(&book, &names.iter().map(String::as_str).collect::<Vec<_>>());
            (
                "A/".into(),
                format!("{b}/"),
                format!("A/{suffix}002.png"),
                format!("{b}/{suffix}003.png"),
            )
        };
        open_list(&mut app, &ctx, &book);
        app.settings.book_open_resume = ResumeMode::Resume;
        activate(&mut app, &ctx, &a_prefix, true);
        let akey = record(&mut app, &a_page);
        close_to_root(&mut app, &ctx);
        activate(&mut app, &ctx, &b_prefix, true);
        let bkey = record(&mut app, &b_page);
        assert_ne!(akey.storage_key(), bkey.storage_key(), "{shape}");
        close_to_root(&mut app, &ctx);
        for (prefix, expected) in [
            (&a_prefix, &a_page),
            (&b_prefix, &b_page),
            (&a_prefix, &a_page),
        ] {
            activate(&mut app, &ctx, prefix, true);
            assert_eq!(current_entry(&app), expected, "{shape}");
            close_to_root(&mut app, &ctx);
        }
        assert_eq!(
            read(&app, &akey).unwrap().identity,
            identity(&book, &a_page)
        );
        assert_eq!(
            read(&app, &bkey).unwrap().identity,
            identity(&book, &b_page)
        );
    }
}

#[test]
fn section1675_inner_resume_navigation_policy() {
    for open_resume in [ResumeMode::FromStart, ResumeMode::Resume] {
        for nav_resume in [ResumeMode::FromStart, ResumeMode::Resume] {
            for slideshow in [false, true] {
                let mut app = setup_app();
                let ctx = egui::Context::default();
                let book = app.tmp.path().join("navigation.zip");
                write_pages(
                    &book,
                    &[
                        "A/001.png",
                        "A/002.png",
                        "A/003.png",
                        "B/001.png",
                        "B/002.png",
                        "B/003.png",
                    ],
                );
                open_list(&mut app, &ctx, &book);
                let akey = BookResumeKey::archive(&book, &["A".into()]).unwrap();
                let bkey = BookResumeKey::archive(&book, &["B".into()]).unwrap();
                seed(&mut app, &akey, &book, "A/002.png", 1);
                seed(&mut app, &bkey, &book, "B/003.png", 2);
                read(&app, &bkey);
                app.settings.book_open_resume = open_resume;
                app.settings.book_nav_resume = nav_resume;
                activate(&mut app, &ctx, "A/", true);
                assert_eq!(
                    current_entry(&app),
                    if open_resume.resumes() {
                        "A/002.png"
                    } else {
                        "A/001.png"
                    }
                );
                let index = app.fullscreen_idx.unwrap();
                assert!(app.zip_nav_dfs_fullscreen((!slideshow).then_some(&ctx), index, true));
                settle(&mut app, &ctx);
                assert_eq!(
                    current_entry(&app),
                    if !slideshow && nav_resume.resumes() {
                        "B/003.png"
                    } else {
                        "B/001.png"
                    }
                );
                assert_eq!(app.slideshow_playing, slideshow);
            }
        }
    }
}

#[test]
fn section1675_inner_resume_case_distinct_dfs_admits_fresh_fence() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("case-dfs.zip");
    write_pages(
        &book,
        &[
            "A/001.png",
            "A/002.png",
            "A/003.png",
            "a/001.png",
            "a/002.png",
            "a/003.png",
        ],
    );
    open_list(&mut app, &ctx, &book);
    let upper = BookResumeKey::archive(&book, &["A".into()]).unwrap();
    let lower = BookResumeKey::archive(&book, &["a".into()]).unwrap();
    assert_ne!(upper, lower);
    assert_eq!(
        crate::book_scope::BookScopeKey::archive(&book, &["A".into()]).unwrap(),
        crate::book_scope::BookScopeKey::archive(&book, &["a".into()]).unwrap()
    );
    seed(&mut app, &upper, &book, "A/002.png", 1);
    seed(&mut app, &lower, &book, "a/001.png", 0);
    read(&app, &lower);
    app.settings.book_open_resume = ResumeMode::Resume;
    app.settings.book_nav_resume = ResumeMode::Resume;
    activate(&mut app, &ctx, "A/", true);
    assert_eq!(current_entry(&app), "A/002.png");
    let release = app.book_resume_writer.as_ref().unwrap().pause_for_test();
    seed(&mut app, &lower, &book, "a/003.png", 2);
    let last_resume_before_dfs = app.last_book_resume.clone();
    let index = app.fullscreen_idx.unwrap();
    assert!(app.zip_nav_dfs_fullscreen(Some(&ctx), index, true));
    assert_eq!(app.zip_nav.as_ref().unwrap().current(), &["a"]);
    app.poll_sidecar_restore(&ctx);
    assert!(
        app.sidecar_restore_active(),
        "same composition scope still requires the destination book's fresh FIFO read"
    );
    assert!(app.fs_nav_is_locked());
    assert_eq!(
        app.fullscreen_idx,
        Some(index),
        "holdover retains the old reader until target admission"
    );
    assert_eq!(
        app.last_book_resume, last_resume_before_dfs,
        "pending destination must not record an unaccepted page"
    );
    release.send(()).unwrap();
    settle(&mut app, &ctx);
    assert_eq!(
        current_entry(&app),
        "a/003.png",
        "neither A's position nor the stale a snapshot may win"
    );
    assert_eq!(
        read(&app, &upper).unwrap().identity,
        identity(&book, "A/002.png")
    );
}

#[test]
fn section1675_inner_resume_saved_page_hidden_by_filter() {
    for any_visible in [false, true] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("ratings.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "A/003.png", "B/001.png"]);
        if any_visible {
            let key = crate::edit_source::page_key_for_grid_item(&GridItem::ZipImage {
                zip_path: book.clone(),
                entry_name: "A/003.png".into(),
            })
            .unwrap();
            app.rating_db.as_ref().unwrap().set(&key, 5).unwrap();
        }
        open_list(&mut app, &ctx, &book);
        let key = BookResumeKey::archive(&book, &["A".into()]).unwrap();
        seed(&mut app, &key, &book, "A/002.png", 1);
        read(&app, &key);
        app.settings.rating_filter = [false, false, false, false, false, true];
        activate(&mut app, &ctx, "A/", true);
        assert!(app.rating_filter_suppressed_at.is_none());
        assert_eq!(app.fullscreen_idx.is_some(), any_visible);
        if any_visible {
            assert_eq!(current_entry(&app), "A/003.png");
        } else {
            assert!(app.visible_indices.is_empty());
        }
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .expect("filtered resume notice")
                .0
                .contains("非表示")
        );
        app.fs_feedback_toast = None;
        settle(&mut app, &ctx);
        assert!(app.fs_feedback_toast.is_none(), "notice is consumed once");
    }
}

#[test]
fn section1675_inner_resume_projection_and_notice() {
    for refusal in ["type", "missing", "outer-only"] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("projection.zip");
        write_pages(&book, &["A/001.png", "A/002.jpg", "B/001.png"]);
        if refusal == "type" {
            app.settings
                .file_type_visibility
                .excluded_extensions
                .insert("jpg".into());
        }
        open_list(&mut app, &ctx, &book);
        let root = BookResumeKey::legacy(&book);
        seed(&mut app, &root, &book, "A/002.jpg", 1);
        let before = read(&app, &root);
        let inner = BookResumeKey::archive(&book, &["A".into()]).unwrap();
        if refusal != "outer-only" {
            seed(
                &mut app,
                &inner,
                &book,
                if refusal == "missing" {
                    "A/deleted.png"
                } else {
                    "A/002.jpg"
                },
                1,
            );
            read(&app, &inner);
        }
        activate(&mut app, &ctx, "A/", true);
        assert_eq!(current_entry(&app), "A/001.png");
        assert_eq!(read(&app, &root), before);
        assert_eq!(
            app.fs_feedback_toast
                .as_ref()
                .is_some_and(|toast| toast.0.contains("非表示")),
            refusal != "outer-only"
        );
    }
}

#[test]
fn section1675_inner_resume_fence_after_rapid_reopen() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("rapid.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "A/003.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    let key = BookResumeKey::archive(&book, &["A".into()]).unwrap();
    seed(&mut app, &key, &book, "A/001.png", 0);
    read(&app, &key);
    let release = app.book_resume_writer.as_ref().unwrap().pause_for_test();
    seed(&mut app, &key, &book, "A/003.png", 2);
    app.settings.auto_fullscreen_zip_pdf = true;
    let index = child(&app, "A/");
    assert_eq!(
        app.activate_main_zipdir(index),
        ZipDirActivationOutcome::BookRequested
    );
    app.poll_sidecar_restore(&ctx);
    assert!(app.sidecar_restore_active());
    assert!(
        app.fullscreen_idx.is_none(),
        "FIFO must prevent stale restoration"
    );
    release.send(()).unwrap();
    settle(&mut app, &ctx);
    assert_eq!(current_entry(&app), "A/003.png");
}

#[test]
fn section1675_inner_resume_composition_projection_and_notice() {
    use crate::book_composition::{
        BookComposition, BookCompositionService, CompositionPageRef, PageDomain,
    };
    for hidden in [false, true] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("composed.zip");
        write_pages(&book, &["A/001.png", "A/002.png", "A/003.png", "B/001.png"]);
        open_list(&mut app, &ctx, &book);
        let key = BookResumeKey::archive(&book, &["A".into()]).unwrap();
        seed(&mut app, &key, &book, "A/002.png", 1);
        read(&app, &key);
        let scope = crate::book_scope::BookScopeKey::archive(&book, &["A".into()]).unwrap();
        let pages = ["A/003.png", "A/001.png", "A/002.png"]
            .into_iter()
            .map(|entry_name| CompositionPageRef::ArchiveEntry {
                entry_name: entry_name.into(),
            })
            .collect();
        let mut composition =
            BookComposition::new(crate::book_composition::ReadingDirection::Ltr, pages);
        if hidden {
            composition
                .hidden_pages
                .push(CompositionPageRef::ArchiveEntry {
                    entry_name: "A/002.png".into(),
                });
        }
        let (service, owner) =
            BookCompositionService::spawn_at(app.tmp.path().join("composition"), None).unwrap();
        let domain = PageDomain::ArchiveLevel {
            prefix: scope.level_prefix.clone(),
        };
        service
            .save(scope, domain, composition)
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .unwrap();
        app.install_book_composition_runtime(service);
        activate(&mut app, &ctx, "A/", true);
        assert!(app.composed_book_layout().is_some());
        assert_eq!(
            current_entry(&app),
            if hidden { "A/003.png" } else { "A/002.png" }
        );
        assert_eq!(
            app.fs_feedback_toast
                .as_ref()
                .is_some_and(|toast| toast.0.contains("非表示")),
            hidden
        );
        owner.shutdown_and_join();
    }
}

#[test]
fn section1675_inner_resume_fence_across_composition_wait() {
    for live in [false, true] {
        #[cfg(not(windows))]
        if live {
            continue;
        }
        for saved in [false, true] {
            let mut app = setup_app();
            let ctx = egui::Context::default();
            let book = app.tmp.path().join("fence.zip");
            let composition_data = app.tmp.path().join("composition");
            write_pages(&book, &["A/001.png", "A/002.png", "A/003.png", "B/001.png"]);
            open_list(&mut app, &ctx, &book);
            activate(&mut app, &ctx, "A/", false);
            #[cfg(windows)]
            if live {
                // Use a real independently owned, window-bound payload. Scope
                // selection is already accepted here; descriptor activation is S3.
                let main = app.viewer_context_main();
                let before = (
                    app.items.clone(),
                    app.items_generation,
                    app.fullscreen_idx,
                    app.selected,
                );
                let reader = app.fork_materialized_still_context(&book, 0);
                assert_ne!(reader, main);
                app.with_window_context_for_test(reader, |owner| {
                    owner.bind_mounted_context_for_test(167502);
                    owner.viewer_presentation = ViewerPresentation::DetachedWindow;
                    owner.navigation_scope = ViewerNavigationScope::DetachedPhysical;
                    owner.detached_viewer_independent_active = true;
                    owner.fullscreen_idx = None;
                    assert_eq!(owner.projected_viewer_context_id(), reader);
                    composition_wait_case(owner, &ctx, &book, &composition_data, true, saved);
                    assert_eq!(owner.projected_viewer_context_id(), reader);
                });
                assert_eq!(app.projected_viewer_context_id(), main);
                assert_eq!(
                    (
                        app.items.clone(),
                        app.items_generation,
                        app.fullscreen_idx,
                        app.selected
                    ),
                    before,
                    "independent Live completion must preserve main"
                );
                app.retire_context(reader, "inner_resume_live_fence_test", |_| ())
                    .unwrap();
            } else {
                composition_wait_case(&mut app, &ctx, &book, &composition_data, false, saved);
            }
            #[cfg(not(windows))]
            composition_wait_case(&mut app, &ctx, &book, &composition_data, false, saved);
        }
    }
}

fn composition_wait_case(
    app: &mut App,
    ctx: &egui::Context,
    book: &Path,
    data: &Path,
    live: bool,
    saved: bool,
) {
    use crate::book_composition::{
        BookComposition, BookCompositionService, CompositionPageRef, PageDomain,
    };
    let context = app.projected_viewer_context_id();
    let key = app.current_book_resume_key().unwrap();
    let (scope, domain) = app.composition_scope_domain().unwrap();
    assert!(matches!(domain, PageDomain::ArchiveLevel { .. }));
    let (service, owner) = BookCompositionService::spawn_at(data.to_owned(), None).unwrap();
    let pages = ["A/003.png", "A/001.png", "A/002.png"]
        .into_iter()
        .map(|entry_name| CompositionPageRef::ArchiveEntry {
            entry_name: entry_name.into(),
        })
        .collect();
    service
        .save(
            scope,
            domain,
            BookComposition::new(crate::book_composition::ReadingDirection::Ltr, pages),
        )
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    app.install_book_composition_runtime(service.clone());
    let (entered, composition_release) = service.hold_worker_for_test();
    entered.recv_timeout(WAIT).unwrap();

    // Both owners admit a real FIFO restore while the saved write is paused.
    // The absent case must freeze absence with the same cutoff semantics.
    let resume_release = app.book_resume_writer.as_ref().unwrap().pause_for_test();
    if saved {
        seed(app, &key, book, "A/002.png", 1);
    }
    let deferred = target(app);
    if live {
        let (tx, _thumbs) = mpsc::channel();
        let now = Instant::now();
        let continuation = sidecar_restore::SidecarLoadContinuation {
            navigation_after_hydration: None,
            audio_art_prune: None,
            first_display_action: sidecar_restore::FirstDisplayAction::Zip {
                has_foreign_archives: false,
            },
            restore_intent: StartupListIntent::PageContinuation,
            auto_aspect_seed: None,
            source_path: book.to_owned(),
            source_is_directory: false,
            prepared_subfolder: None,
            prepared_page_edits: None,
            prepared_aggregate: None,
            catalog_existing_keys: HashSet::new(),
            video_items: Vec::new(),
            sli_seq: 0,
            sli_t0: now,
            items_len: app.items.len(),
            detached_physical: true,
            tx,
            cancel: Arc::new(AtomicBool::new(false)),
            restore_started_at: now,
        };
        app.fs_nav_after_pdf_enumerate = Some(deferred);
        assert!(app.begin_sidecar_restore(continuation, false).is_ok());
    } else {
        let policy = app.settings.file_type_visibility.clone();
        app.prepare_current_listing_page_identity_index(policy, None);
        assert!(app.attach_archive_book_index_continuation(deferred));
    }
    app.poll_sidecar_restore(ctx);
    assert!(app.sidecar_restore_active());
    assert!(app.fullscreen_idx.is_none());
    resume_release.send(()).unwrap();
    let deadline = Instant::now() + WAIT;
    while app.sidecar_restore_active() {
        app.poll_sidecar_restore(ctx);
        assert!(Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert_eq!(app.projected_viewer_context_id(), context);
    assert!(
        app.book_composition.is_pending(),
        "live={live} saved={saved}: composition must suspend display"
    );
    assert!(app.book_deferred_open_wait_active());
    assert!(app.fullscreen_idx.is_none());
    assert_eq!(
        app.fenced_book_resume_key(&key),
        None,
        "FIFO fence must end before composition finishes"
    );
    seed(app, &key, book, "A/001.png", 0);
    assert_eq!(
        read(app, &key).unwrap().identity,
        identity(book, "A/001.png")
    );
    composition_release.send(()).unwrap();
    settle(app, ctx);
    assert_eq!(
        current_entry(app),
        if saved { "A/002.png" } else { "A/003.png" },
        "live={live} saved={saved}"
    );
    assert!(app.composed_book_layout().is_some());
    owner.shutdown_and_join();
}

#[test]
fn section1675_inner_resume_root_keys_unchanged() {
    for wrapper in [false, true] {
        for converted in [false, true] {
            let mut app = setup_app();
            let ctx = egui::Context::default();
            let book = app.tmp.path().join("root.zip");
            write_pages(
                &book,
                if wrapper {
                    &["wrapper/001.png", "wrapper/002.png"]
                } else {
                    &["001.png", "002.png"]
                },
            );
            open_list(&mut app, &ctx, &book);
            let expected = BookResumeKey::legacy(&book);
            assert_eq!(app.current_book_resume_key(), Some(expected.clone()));
            let logical = if converted {
                app.tmp.path().join("source.rar")
            } else {
                book.clone()
            };
            if converted {
                app.archive_source_override = Some(logical.clone());
                let remote_root = BookResumeKey::legacy(&logical);
                seed(
                    &mut app,
                    &remote_root,
                    &logical,
                    if wrapper {
                        "wrapper/001.png"
                    } else {
                        "001.png"
                    },
                    0,
                );
                assert_eq!(read(&app, &remote_root).unwrap().page, 0);
            }
            assert_eq!(
                app.current_book_resume_key(),
                Some(expected.clone()),
                "PC converted root retains backing key"
            );
            let entry = if wrapper {
                "wrapper/002.png"
            } else {
                "002.png"
            };
            seed(&mut app, &expected, &logical, entry, 1);
            read(&app, &expected);
            // Rebuild the prepared index with the accepted logical alias, just as
            // converted-source installation does before the resume consumer.
            app.page_identity_index =
                Arc::new(file_type_resume::PreparedPageIdentityIndex::prepare(
                    &app.items,
                    converted.then_some(logical.as_path()),
                    app.settings.file_type_visibility.clone(),
                ));
            assert_eq!(app.resume_page_for_container(), Some(1));
        }
    }
}

#[test]
fn section1675_inner_resume_cache_rebuild() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let logical = app.tmp.path().join("source.rar");
    let old = app.tmp.path().join("old-cache.zip");
    let new = app.tmp.path().join("new-cache.zip");
    for backing in [&old, &new] {
        write_pages(
            backing,
            &["A/001.png", "A/002.png", "A/003.png", "B/001.png"],
        );
    }
    open_list(&mut app, &ctx, &old);
    app.archive_source_override = Some(logical.clone());
    activate(&mut app, &ctx, "A/", true);
    let key = record(&mut app, "A/002.png");
    close_to_root(&mut app, &ctx);
    // Removing a disposable backing must not delete the logical resume family.
    std::fs::remove_file(&old).unwrap();
    open_list(&mut app, &ctx, &new);
    app.archive_source_override = Some(logical);
    activate(&mut app, &ctx, "A/", true);
    assert_eq!(app.current_book_resume_key(), Some(key));
    assert_eq!(current_entry(&app), "A/002.png");
    assert_eq!(read(&app, &BookResumeKey::legacy(&new)), None);
}

#[test]
fn section1675_inner_resume_dfs_skips_composition_hidden_book() {
    use crate::book_composition::{
        BookComposition, BookCompositionService, CompositionPageRef, PageDomain,
    };
    for slideshow in [false, true] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let book = app.tmp.path().join("dfs-hidden.zip");
        write_pages(&book, &["A/001.png", "B/001.png", "C/001.png"]);
        open_list(&mut app, &ctx, &book);
        let page = CompositionPageRef::ArchiveEntry {
            entry_name: "B/001.png".into(),
        };
        let mut composition = BookComposition::new(
            crate::book_composition::ReadingDirection::Ltr,
            vec![page.clone()],
        );
        composition.hidden_pages.push(page);
        let (service, owner) =
            BookCompositionService::spawn_at(app.tmp.path().join("composition"), None).unwrap();
        let scope = crate::book_scope::BookScopeKey::archive(&book, &["B".into()]).unwrap();
        let domain = PageDomain::ArchiveLevel {
            prefix: scope.level_prefix.clone(),
        };
        service
            .save(scope, domain, composition)
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .unwrap();
        app.install_book_composition_runtime(service.clone());
        activate(&mut app, &ctx, "A/", true);
        let index = app.fullscreen_idx.unwrap();
        let (entered, release) = service.hold_worker_for_test();
        entered.recv_timeout(WAIT).unwrap();
        assert!(app.zip_nav_dfs_fullscreen((!slideshow).then_some(&ctx), index, true));
        let deadline = Instant::now() + WAIT;
        while app.sidecar_restore_active() {
            app.poll_sidecar_restore(&ctx);
            assert!(Instant::now() < deadline);
            std::thread::yield_now();
        }
        assert!(app.book_composition.is_pending());
        assert!(app.book_deferred_open_wait_active());
        assert_eq!(app.fullscreen_idx, Some(index));
        release.send(()).unwrap();
        settle(&mut app, &ctx);
        assert_eq!(current_entry(&app), "C/001.png");
        assert_eq!(app.slideshow_playing, slideshow);
        owner.shutdown_and_join();
    }
}

#[test]
fn section1675_inner_resume_meter_clear_and_reload() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let book = app.tmp.path().join("meters.zip");
    write_pages(&book, &["A/001.png", "A/002.png", "A/003.png", "B/001.png"]);
    open_list(&mut app, &ctx, &book);
    let root = BookResumeKey::legacy(&book);
    let inner = BookResumeKey::archive(&book, &["A".into()]).unwrap();
    seed(&mut app, &root, &book, "A/001.png", 0);
    seed(&mut app, &inner, &book, "A/003.png", 2);
    read(&app, &inner);
    app.reload_book_resume_meters();
    read(&app, &inner);
    app.poll_book_resume_meters(&ctx);
    assert_eq!(app.book_resume_entry_count(), 2);
    assert_eq!(
        app.book_resume_meters.get(&book),
        ReadingMeterValue::new(1, 3)
    );
    activate(&mut app, &ctx, "A/", false);
    assert!(
        (0..app.items.len()).all(|i| app.thumbnail_book_resume_meter(i).is_none()),
        "internal positions must not become root thumbnail meters"
    );
    app.clear_book_resume_async();
    read(&app, &inner);
    app.poll_book_resume_meters(&ctx);
    assert_eq!(app.book_resume_entry_count(), 0);
    assert_eq!(read(&app, &root), None);
    assert_eq!(read(&app, &inner), None);
    assert!(
        !app.book_resume_writer
            .as_ref()
            .unwrap()
            .position_snapshot()
            .unwrap()
            .values
            .contains_key(&inner.storage_key())
    );
}

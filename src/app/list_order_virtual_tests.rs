use super::tests::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use collection_navigation::{PhysicalHistoryPreflight, PhysicalHistoryPreflightPayload};
use std::time::{Duration, Instant};

#[derive(Clone, Copy, Debug)]
enum TargetKind {
    Zip,
    ColdPdf,
    WarmPdf,
    Folder,
    AudioAndInnerZip,
    CachedArchive,
}

fn write_page(path: &Path) {
    image::RgbaImage::from_pixel(2, 2, image::Rgba([30, 60, 90, 255]))
        .save(path)
        .unwrap();
}

fn write_zip(path: &Path) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    for name in ["001.png", "002.png", "003.png"] {
        zip.start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        // Enumeration and item adoption are the tested boundary; pixels are not rendered.
        std::io::Write::write_all(&mut zip, b"page").unwrap();
    }
    zip.finish().unwrap();
}

fn pages() -> crate::pdf_loader::PdfEnumerateResult {
    (0..3)
        .map(|page_num| crate::pdf_loader::PdfPageEntry {
            page_num,
            mtime: 1,
            file_size: 1,
        })
        .collect()
}

fn configure_fixture(
    app: &mut AppTestEnv,
    kind: TargetKind,
) -> (PathBuf, PathBuf, Arc<ListOrderSnapshot>) {
    app.active_quick_folder_slot = None;
    app.settings.sort_order = crate::settings::SortOrder::FileName;
    let root = app.tmp.path().join("virtual-navigation");
    let source = root.join("a-source");
    std::fs::create_dir_all(&source).unwrap();
    write_page(&source.join("001.png"));
    let target = match kind {
        TargetKind::Zip => root.join("b-target.zip"),
        TargetKind::ColdPdf | TargetKind::WarmPdf => root.join("b-target.pdf"),
        TargetKind::Folder | TargetKind::AudioAndInnerZip => root.join("b-target"),
        TargetKind::CachedArchive => root.join("b-target.7z"),
    };
    let target_item = match kind {
        TargetKind::Zip => {
            write_zip(&target);
            GridItem::ZipFile(target.clone())
        }
        TargetKind::ColdPdf | TargetKind::WarmPdf => {
            std::fs::write(&target, b"%PDF-1.4\n").unwrap();
            if matches!(kind, TargetKind::WarmPdf) {
                let metadata = std::fs::metadata(&target).unwrap();
                app.get_or_open_catalog(&root)
                    .unwrap()
                    .set_pdf_meta(
                        "b-target.pdf",
                        crate::ui_helpers::mtime_secs(&metadata),
                        metadata.len() as i64,
                        3,
                        false,
                    )
                    .unwrap();
            }
            GridItem::PdfFile(target.clone())
        }
        TargetKind::Folder => {
            std::fs::create_dir(&target).unwrap();
            for name in ["001.png", "002.png", "003.png"] {
                write_page(&target.join(name));
            }
            GridItem::Folder(target.clone())
        }
        TargetKind::AudioAndInnerZip => {
            std::fs::create_dir(&target).unwrap();
            std::fs::write(target.join("audio.mp3"), b"audio").unwrap();
            write_zip(&target.join("inner.zip"));
            GridItem::Folder(target.clone())
        }
        TargetKind::CachedArchive => {
            std::fs::write(&target, b"cached source archive").unwrap();
            let cached = app.tmp.path().join("prepared-cache.zip");
            write_zip(&cached);
            let metadata = std::fs::metadata(&target).unwrap();
            app.archive_cache_db
                .as_ref()
                .unwrap()
                .record(
                    &target,
                    crate::ui_helpers::mtime_secs(&metadata),
                    metadata.len() as i64,
                    crate::archive_converter::ArchiveFormat::SevenZ,
                    &cached,
                    std::fs::metadata(&cached).unwrap().len() as i64,
                    3,
                    false,
                )
                .unwrap();
            app.settings
                .set_archive_file_handling(crate::settings::ArchiveFileHandling::Convert);
            GridItem::ConvertibleArchive {
                path: target.clone(),
                format: crate::archive_converter::ArchiveFormat::SevenZ,
            }
        }
    };
    app.current_folder = Some(root);
    app.items = vec![GridItem::Folder(source.clone()), target_item];
    app.image_metas = vec![None; 2];
    app.thumbnails = vec![ThumbnailState::Pending; 2];
    app.visible_indices = vec![0, 1];
    app.capture_main_folder_list_order(0);
    let snapshot = app.list_order.as_ref().unwrap().clone();
    // Hydration reads the writer's fenced publication. A direct BookResumeDb::set
    // bypasses that publication and can leave its already-loaded snapshot empty.
    // Seed the same FIFO used by a real viewed page, and prove the saved position
    // is visible before any destination restore is admitted.
    let writer = app.book_resume_writer.as_ref().unwrap();
    assert!(writer.record(&target, 2, None));
    let saved = writer
        .read_position_after(
            &crate::path_key::normalize(&target),
            writer.watermarks().admitted,
        )
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(saved.status.is_ok());
    assert_eq!(
        saved.position.as_ref().map(|position| position.page),
        Some(2)
    );
    assert_eq!(app.book_resume_db.as_ref().unwrap().get(&target), Some(2));
    app.load_folder(source.clone());
    app.finish_page_identity_preparation_for_test();
    app.fullscreen_idx = Some(0);
    (source, target, snapshot)
}

fn wait_for_folder_result(app: &mut App, ctx: &egui::Context) -> FolderNavResult {
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        app.poll_sidecar_restore(ctx);
        if let Some(result) = app.poll_folder_nav() {
            return result;
        }
        assert!(Instant::now() < deadline, "folder worker did not finish");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn replace_pdf_preflight(app: &mut App) {
    let Some(HistoryNavigationTransition::Physical(request)) =
        app.top_level_grid_view.history_navigation_transition_mut()
    else {
        panic!("PDF traversal must keep its physical history owner");
    };
    let PhysicalHistoryPhase::Preflighting { preflight, .. } = &mut request.phase else {
        panic!("PDF traversal must reach preflight before adoption");
    };
    *preflight = PhysicalHistoryPreflight::ready_for_test(
        PhysicalHistoryPreflightPayload::PdfPages(pages()),
    );
}

fn finish_reopen(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.top_level_grid_view.open_path_classification().is_some()
        || app
            .top_level_grid_view
            .history_navigation_transition()
            .is_some()
        || app.zip_enumerate_pending.is_some()
        || app.pdf_enumerate_pending.is_some()
        || app.fs_nav_after_pdf_enumerate.is_some()
        || app.sidecar_restore_active()
        || app.fullscreen_idx.is_none()
    {
        app.settle_open_path_classification_for_test();
        app.poll_history_navigation_transition_for_test(ctx);
        app.poll_zip_enumerate();
        app.poll_pdf_enumerate();
        app.poll_sidecar_restore(ctx);
        app.poll_book_composition_service_for_test(ctx);
        app.poll_book_presentation_permission(ctx);
        assert!(Instant::now() < deadline, "staged traversal did not reopen");
        std::thread::sleep(Duration::from_millis(2));
    }
}

fn traverse(
    kind: TargetKind,
    list_order: bool,
    auto_open: bool,
    resume: bool,
) -> (usize, Vec<String>) {
    let mut app = setup_app();
    let (source, target, snapshot) = configure_fixture(&mut app, kind);
    app.settings.auto_fullscreen_zip_pdf = auto_open;
    app.settings.auto_fullscreen_image_folders = auto_open;
    app.settings.book_nav_resume = if resume {
        crate::settings::ResumeMode::Resume
    } else {
        crate::settings::ResumeMode::FromStart
    };
    app.settings.book_open_resume = if resume {
        crate::settings::ResumeMode::FromStart
    } else {
        crate::settings::ResumeMode::Resume
    };
    let ctx = egui::Context::default();
    if list_order {
        app.handle_fullscreen_list_order_nav_context(&ctx, 0, true, false);
    } else {
        app.handle_fullscreen_ctrl_nav_context(&ctx, 0, true, false);
    }
    assert!(app.folder_nav_pending.is_some(), "{kind:?}");
    let result = wait_for_folder_result(&mut app, &ctx);
    assert_eq!(result.target.as_ref().unwrap().logical_source(), target);
    assert_eq!(
        matches!(result.mode, FolderNavMode::ListOrder { .. }),
        list_order
    );
    app.apply_folder_nav_result(&ctx, result);
    app.settle_open_path_classification_for_test();
    if matches!(
        kind,
        TargetKind::Zip | TargetKind::ColdPdf | TargetKind::WarmPdf | TargetKind::CachedArchive
    ) {
        let Some(HistoryNavigationTransition::Physical(request)) =
            app.top_level_grid_view.history_navigation_transition()
        else {
            panic!("virtual traversal bypassed the physical history owner: {kind:?}");
        };
        let continuation = request.dfs_continuation.as_ref().unwrap();
        assert!(continuation.fullscreen);
        assert_eq!(
            matches!(continuation.mode, FolderNavMode::ListOrder { .. }),
            list_order
        );
        assert_eq!(
            app.effective_folder().as_ref(),
            Some(&source),
            "source remains visible until preflight adopts"
        );
        if matches!(kind, TargetKind::ColdPdf | TargetKind::WarmPdf) {
            replace_pdf_preflight(&mut app);
        }
    }
    finish_reopen(&mut app, &ctx);
    let destination = if matches!(kind, TargetKind::AudioAndInnerZip) {
        target.join("inner.zip")
    } else {
        target.clone()
    };
    assert_eq!(app.effective_folder().as_ref(), Some(&destination));
    assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
    assert_eq!(app.list_order_position().unwrap().1, 1);
    assert!(app.fs_nav_after_pdf_enumerate.is_none());
    let index = app.fullscreen_idx.unwrap();
    let names = app
        .items
        .iter()
        .map(|item| item.name().into_owned())
        .collect();
    (index, names)
}

#[test]
fn list_order_virtual_staged_zip_pdf_and_folder_reopen_match_existing_fullscreen_navigation() {
    for kind in [
        TargetKind::Zip,
        TargetKind::ColdPdf,
        TargetKind::WarmPdf,
        TargetKind::Folder,
    ] {
        for auto_open in [false, true] {
            for resume in [false, true] {
                let expected = traverse(kind, false, auto_open, resume);
                let actual = traverse(kind, true, auto_open, resume);
                assert_eq!(
                    actual, expected,
                    "{kind:?}, auto_open={auto_open}, resume={resume}"
                );
            }
        }
    }
}

#[test]
fn list_order_virtual_folder_audio_and_inner_zip_keeps_parent_row_position() {
    let expected = traverse(TargetKind::AudioAndInnerZip, false, false, false);
    let actual = traverse(TargetKind::AudioAndInnerZip, true, false, false);
    assert_eq!(actual, expected);
}

#[test]
fn list_order_virtual_converted_archive_cache_reopen_matches_existing_fullscreen_navigation() {
    let expected = traverse(TargetKind::CachedArchive, false, false, false);
    let actual = traverse(TargetKind::CachedArchive, true, false, false);
    assert_eq!(actual, expected);
}

#[test]
fn list_order_virtual_pending_pdf_reopen_captures_navigation_resume_and_not_open_resume() {
    for warm in [false, true] {
        for resume in [false, true] {
            let mut app = setup_app();
            let kind = if warm {
                TargetKind::WarmPdf
            } else {
                TargetKind::ColdPdf
            };
            let (_, target, snapshot) = configure_fixture(&mut app, kind);
            app.settings.book_nav_resume = if resume {
                crate::settings::ResumeMode::Resume
            } else {
                crate::settings::ResumeMode::FromStart
            };
            app.settings.book_open_resume = if resume {
                crate::settings::ResumeMode::FromStart
            } else {
                crate::settings::ResumeMode::Resume
            };
            let ctx = egui::Context::default();
            let _ = app.load_pdf_as_folder_owned(
                target.clone(),
                OpenRequestOwner::Navigation,
                StartupListIntent::PageContinuation,
            );
            assert!(
                matches!(
                    app.pdf_enumerate_pending.as_ref().map(|pending| &pending.5),
                    Some(PdfOpenPhase::CommittedVerification { .. })
                ) == warm
            );
            assert_eq!(
                app.reopen_fullscreen_after_folder_nav_load(
                    &ctx,
                    false,
                    false,
                    HistoryTrigger::UserChosen
                ),
                "enumerate_defer"
            );
            let deferred = app.fs_nav_after_pdf_enumerate.as_ref().unwrap();
            assert_eq!(deferred.resume_to_last_page, resume);
            assert!(!deferred.from_explicit_open);
            assert!(!deferred.preserve_after_password_prompt);
            app.pdf_enumerate_pending.as_mut().unwrap().2 =
                crate::pdf_loader::completed_enumerate_result_handle(&target, Ok(pages()));
            app.poll_pdf_enumerate();
            finish_reopen(&mut app, &ctx);
            assert_eq!(app.fullscreen_idx, Some(if resume { 2 } else { 0 }));
            assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
            assert_eq!(app.list_order_position().unwrap().1, 1);
        }
    }
}

#[test]
fn list_order_virtual_pending_zip_reopen_captures_navigation_resume_and_restores_page() {
    for resume in [false, true] {
        let mut app = setup_app();
        let (_, target, snapshot) = configure_fixture(&mut app, TargetKind::Zip);
        app.settings.book_nav_resume = if resume {
            crate::settings::ResumeMode::Resume
        } else {
            crate::settings::ResumeMode::FromStart
        };
        app.settings.book_open_resume = if resume {
            crate::settings::ResumeMode::FromStart
        } else {
            crate::settings::ResumeMode::Resume
        };
        let ctx = egui::Context::default();
        app.load_zip_as_folder(target, StartupListIntent::PageContinuation);
        assert!(app.zip_enumerate_pending.is_some());
        assert_eq!(
            app.reopen_fullscreen_after_folder_nav_load(
                &ctx,
                false,
                false,
                HistoryTrigger::UserChosen
            ),
            "enumerate_defer"
        );
        let deferred = app.fs_nav_after_pdf_enumerate.as_ref().unwrap();
        assert_eq!(deferred.resume_to_last_page, resume);
        assert!(!deferred.from_explicit_open);
        assert!(!deferred.preserve_after_password_prompt);
        finish_reopen(&mut app, &ctx);
        assert_eq!(app.fullscreen_idx, Some(if resume { 2 } else { 0 }));
        assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
        assert_eq!(app.list_order_position().unwrap().1, 1);
    }
}

//! §1.853 D2: search stack effects commit with the accepted physical listing.
#![cfg(windows)]
use super::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use std::io::Write;

fn directory(app: &AppTestEnv, relative: &str) -> PathBuf {
    let path = app.tmp.path().join(relative);
    std::fs::create_dir_all(&path).unwrap();
    path
}

fn source(app: &mut App, path: &Path, stack: &[PathBuf], tag: bool) {
    app.settings.auto_fullscreen_zip_pdf = false;
    app.settings.auto_fullscreen_image_folders = false;
    app.active_quick_folder_slot = None;
    app.load_folder_and_finish_for_test(path.to_owned());
    if tag {
        app.tag_view.active = true;
        app.tag_view.run = 7;
        app.tag_view.last_executed = "#chosen".into();
        app.tag_view.nav_stack = stack.to_vec();
        app.update_tag_view_address();
    } else {
        app.favsearch.active = true;
        app.favsearch.last_executed = "chosen".into();
        app.favsearch.nav_stack = stack.to_vec();
        app.update_favsearch_address();
    }
}

fn stack(app: &App, tag: bool) -> &[PathBuf] {
    if tag {
        &app.tag_view.nav_stack
    } else {
        &app.favsearch.nav_stack
    }
}

fn back(app: &mut App, tag: bool) {
    if tag {
        app.tag_view_back();
    } else {
        app.favsearch_back();
    }
}

fn settle(app: &mut App) {
    let ctx = egui::Context::default();
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    loop {
        app.poll_open_path_classification(&ctx);
        app.poll_direct_navigation_scan_for_test(&ctx);
        app.poll_collection_history_transition(&ctx);
        app.poll_zip_enumerate();
        app.poll_sidecar_restore(&ctx);
        app.poll_book_composition_service_for_test(&ctx);
        if !app.direct_navigation_scan_pending()
            && app.top_level_grid_view.open_path_classification().is_none()
            && app
                .top_level_grid_view
                .history_navigation_transition()
                .is_none()
            && app.zip_enumerate_pending.is_none()
            && !app.sidecar_restore_active()
            && !app.book_composition.is_pending()
            && !app.book_composition_permission.pending()
        {
            app.finish_page_identity_preparation_for_test();
            return;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "D2 navigation did not settle"
        );
        std::thread::yield_now();
    }
}

#[test]
fn section1853_favorite_back_defers_effects_and_preserves_released_duplicate_parent() {
    let mut app = setup_app();
    let root = directory(&app, "favorite-root");
    let child = directory(&app, "favorite-root/child");
    let original = vec![root.clone(), child.clone()];
    source(&mut app, &child, &original, false);
    let address = app.address.clone();
    let scans = folder_scan::caller_scan_count_for_test();
    app.favsearch_back();
    assert!(app.direct_navigation_scan_pending());
    assert_eq!(app.favsearch.nav_stack, original);
    assert_eq!(app.address, address);
    assert_eq!(app.current_folder.as_ref(), Some(&child));
    settle(&mut app);
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.current_folder.as_ref(), Some(&root));
    assert_eq!(app.items[app.selected.unwrap()], GridItem::Folder(child));
    // Existing source does pop then loader push, so [root, child] -> [root, root].
    // Correcting that released bug requires the outstanding behavior decision.
    assert_eq!(app.favsearch.nav_stack, vec![root.clone(), root.clone()]);
    let root_address = app.address.clone();
    app.favsearch_back();
    settle(&mut app);
    assert_eq!(app.favsearch.nav_stack, vec![root.clone(), root]);
    assert_eq!(
        app.address, root_address,
        "the next BS retains the released result"
    );
}

#[test]
fn section1853_favorite_sibling_defers_replace_and_preserves_released_duplicate_root() {
    let mut app = setup_app();
    let first = directory(&app, "first");
    let child = directory(&app, "first/child");
    let next = directory(&app, "next");
    let original = vec![first.clone(), child.clone()];
    source(&mut app, &child, &original, false);
    app.favsearch.results_paths = vec![first, next.clone()];
    let address = app.address.clone();
    let scans = folder_scan::caller_scan_count_for_test();
    assert!(app.favsearch_navigate_sibling(1));
    assert_eq!(app.favsearch.nav_stack, original);
    assert_eq!(app.address, address);
    settle(&mut app);
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.current_folder.as_ref(), Some(&next));
    // Existing Sibling first replaces with [next], then loader pushes next.
    assert_eq!(app.favsearch.nav_stack, vec![next.clone(), next]);
}

#[test]
fn section1853_tag_back_pops_only_at_adoption_and_next_back_requeries() {
    let mut app = setup_app();
    let root = directory(&app, "tag-root");
    let child = directory(&app, "tag-root/child");
    let original = vec![root.clone(), child.clone()];
    source(&mut app, &child, &original, true);
    let address = app.address.clone();
    let scans = folder_scan::caller_scan_count_for_test();
    app.tag_view_back();
    assert_eq!(app.tag_view.nav_stack, original);
    assert_eq!(app.address, address);
    settle(&mut app);
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.current_folder.as_ref(), Some(&root));
    assert_eq!(app.tag_view.nav_stack, vec![root.clone()]);
    assert_eq!(app.items[app.selected.unwrap()], GridItem::Folder(child));
    let run = app.tag_view.run;
    app.tag_view_back();
    assert!(app.tag_view.nav_stack.is_empty());
    assert_eq!(app.tag_view.run, run + 1);
    assert_eq!(
        app.select_after_load,
        Some(AfterLoadSelection::ExactPath(root))
    );
}

#[test]
fn section1853_search_back_immediate_refusal_restores_hint_without_stack_or_address_change() {
    for tag in [false, true] {
        let mut app = setup_app();
        let root = directory(&app, "refused-root");
        let child = directory(&app, "refused-root/child");
        let original = vec![root, child.clone()];
        source(&mut app, &child, &original, tag);
        let address = app.address.clone();
        let hint = Some(AfterLoadSelection::Name("previous-hint".into()));
        app.select_after_load = hint.clone();
        let pdf = app.tmp.path().join("modal.pdf");
        app.pdf_password_request = Some(PdfPasswordRequest::legacy(pdf));
        assert!(app.document_open_modal_admission_blocked());
        back(&mut app, tag);
        assert_eq!(stack(&app, tag), original);
        assert_eq!(app.address, address);
        assert_eq!(app.select_after_load, hint);
        assert!(!app.direct_navigation_scan_pending());
    }
}

#[test]
fn section1853_search_back_failure_or_replacement_keeps_old_stack_and_address() {
    for tag in [false, true] {
        for failed in [false, true] {
            let mut app = setup_app();
            let root = directory(&app, "retired-root");
            let child = directory(&app, "retired-root/child");
            let original = vec![root, child.clone()];
            source(&mut app, &child, &original, tag);
            let address = app.address.clone();
            back(&mut app, tag);
            if failed {
                let policy = app.folder_scan_policy();
                let (tx, rx) = mpsc::channel();
                tx.send(FolderOpenScanResult {
                    scan: Err(std::io::Error::from(std::io::ErrorKind::PermissionDenied)),
                    policy,
                    policy_mixed: false,
                })
                .unwrap();
                app.folder_pane_open_pending.as_mut().unwrap().rx = rx;
                app.post_render_main_navigation(
                    &egui::Context::default(),
                    update_phases::MainFrameNavigation::default(),
                );
            } else {
                let pending = app.folder_pane_open_pending.as_ref().unwrap();
                let cancel = pending.cancel.clone();
                app.retire_main_list_requests_for_surface_switch();
                assert!(cancel.load(Ordering::Relaxed));
            }
            assert_eq!(stack(&app, tag), original);
            assert_eq!(app.address, address);
            assert_eq!(app.current_folder.as_ref(), Some(&child));
            assert!(!app.direct_navigation_scan_pending());
            assert!(app.select_after_load.is_none());
        }
    }
}

#[test]
fn section1853_tag_back_survives_classification_and_staged_zip_adoption() {
    let mut app = setup_app();
    let child = directory(&app, "zip-child");
    let archive = app.tmp.path().join("return.zip");
    let mut writer = zip::ZipWriter::new(std::io::Cursor::new(Vec::new()));
    writer
        .start_file("page.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    let mut image = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(4, 8)
        .write_to(&mut image, image::ImageFormat::Png)
        .unwrap();
    writer.write_all(&image.into_inner()).unwrap();
    std::fs::write(&archive, writer.finish().unwrap().into_inner()).unwrap();
    let original = vec![archive.clone(), child.clone()];
    source(&mut app, &child, &original, true);
    let scans = folder_scan::caller_scan_count_for_test();
    app.tag_view_back();
    let candidate = app.top_level_grid_view.open_path_classification().unwrap();
    assert_eq!(
        candidate
            .navigation
            .as_ref()
            .unwrap()
            .search_admission
            .tag_view,
        Some(TagViewNavAdmission::Back)
    );
    assert_eq!(app.tag_view.nav_stack, original);
    settle(&mut app);
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    assert_eq!(app.effective_folder(), Some(archive.clone()));
    assert_eq!(app.tag_view.nav_stack, vec![archive]);
    assert!(matches!(app.items.first(), Some(GridItem::ZipImage { .. })));
}

#[test]
fn section1853_tag_back_intent_is_owned_by_pdf_and_conversion_classification() {
    for suffix in ["pdf", "rar", "7z"] {
        let mut app = setup_app();
        let child = directory(&app, "typed-child");
        let target = app.tmp.path().join(format!("typed-return.{suffix}"));
        std::fs::write(&target, b"classification fixture").unwrap();
        let original = vec![target, child.clone()];
        source(&mut app, &child, &original, true);
        app.tag_view_back();
        let candidate = app.top_level_grid_view.open_path_classification().unwrap();
        assert_eq!(
            candidate
                .navigation
                .as_ref()
                .unwrap()
                .search_admission
                .tag_view,
            Some(TagViewNavAdmission::Back)
        );
        assert_eq!(app.tag_view.nav_stack, original);
        app.retire_main_list_requests_for_surface_switch();
    }
}

#[test]
fn section1853_tag_back_is_preserved_by_prepared_direct_pdf_adoption() {
    let mut app = setup_app();
    let child = directory(&app, "prepared-pdf-child");
    let pdf = app.tmp.path().join("prepared-return.pdf");
    std::fs::write(&pdf, b"enumeration already prepared").unwrap();
    source(&mut app, &child, &[pdf.clone(), child.clone()], true);
    let mut navigation = app
        .capture_physical_open_navigation(&OpenRequestOwner::Navigation, &pdf)
        .unwrap();
    navigation.search_admission.tag_view = Some(TagViewNavAdmission::Back);
    let source = crate::pdf_loader::LeasedEpubPath::try_new(pdf.clone()).unwrap();
    assert!(app.adopt_direct_pdf_rows(
        &pdf,
        &OpenRequestOwner::Navigation,
        &source,
        Some(Box::new(DirectPdfAdoption {
            navigation,
            grid_open_effects: None
        })),
        vec![GridItem::PdfPage {
            pdf_path: pdf.clone(),
            page_num: 0,
            content_type: None
        }],
        vec![None],
        HashSet::new(),
        sidecar_restore::FirstDisplayAction::None,
        StartupListIntent::ExplicitList,
    ));
    assert_eq!(app.tag_view.nav_stack, vec![pdf.clone()]);
    assert_eq!(app.current_folder.as_ref(), Some(&pdf));
}

#[test]
fn section1853_conversion_phase_uses_captured_search_back_instead_of_new_descend() {
    for tag in [false, true] {
        let mut app = setup_app();
        let child = directory(&app, "conversion-child");
        let target = app.tmp.path().join("conversion-return.7z");
        source(&mut app, &child, &[target.clone(), child.clone()], tag);
        let mut navigation = app
            .capture_physical_open_navigation(&OpenRequestOwner::Navigation, &target)
            .unwrap();
        if tag {
            navigation.search_admission.tag_view = Some(TagViewNavAdmission::Back);
        } else {
            navigation.search_admission.favsearch = Some(FavsearchNavAdmission::Back);
        }
        assert!(app.admit_physical_history_phase(
            PhysicalHistoryIntent::Navigation {
                replay: None,
                auto_fullscreen: false
            },
            target,
            None,
            Some(OpenPathKind::File),
            PhysicalHistoryPhase::ArchiveConverting,
            navigation,
            None,
            StartupListIntent::ExplicitList,
            MainListRequestStage::Admission,
            None,
        ));
        let Some(HistoryNavigationTransition::Physical(request)) =
            app.top_level_grid_view.history_navigation_transition()
        else {
            panic!("the admitted conversion phase must retain its typed request");
        };
        if tag {
            assert_eq!(request.tag_view_origin, Some(TagViewNavAdmission::Back));
        } else {
            assert_eq!(request.favsearch_origin, Some(FavsearchNavAdmission::Back));
        }
    }
}

#[test]
fn section1853_search_back_classifies_suffix_directories_before_virtual_dispatch() {
    for tag in [false, true] {
        for suffix in ["zip", "pdf", "epub", "7z"] {
            let mut app = setup_app();
            let root = directory(&app, &format!("directory.{suffix}"));
            let child = directory(&app, &format!("directory.{suffix}/child"));
            let original = vec![root.clone(), child.clone()];
            source(&mut app, &child, &original, tag);
            let scans = folder_scan::caller_scan_count_for_test();
            #[cfg(feature = "test-script")]
            let requests = app.test_script_folder_load_requests;
            back(&mut app, tag);
            let candidate = app.top_level_grid_view.open_path_classification().unwrap();
            let admitted = &candidate.navigation.as_ref().unwrap().search_admission;
            if tag {
                assert_eq!(admitted.tag_view, Some(TagViewNavAdmission::Back));
            } else {
                assert_eq!(admitted.favsearch, Some(FavsearchNavAdmission::Back));
            }
            assert_eq!(stack(&app, tag), original);
            settle(&mut app);
            assert_eq!(app.current_folder.as_ref(), Some(&root));
            assert_eq!(app.items[app.selected.unwrap()], GridItem::Folder(child));
            assert!(app.archive_source_override.is_none());
            assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
            if tag {
                assert_eq!(stack(&app, tag), vec![root]);
            } else {
                assert_eq!(stack(&app, tag), vec![root.clone(), root]);
            }
            #[cfg(feature = "test-script")]
            {
                assert_eq!(app.test_script_folder_load_requests, requests + 1);
                settle(&mut app);
                assert_eq!(app.test_script_folder_load_requests, requests + 1);
            }
        }
    }
}

#[test]
fn section1853_favorite_sibling_keeps_suffix_directory_classification_and_single_count() {
    let mut app = setup_app();
    let first = directory(&app, "suffix-first");
    let child = directory(&app, "suffix-first/child");
    let target = directory(&app, "suffix-next.zip");
    source(&mut app, &child, &[first.clone(), child.clone()], false);
    app.favsearch.results_paths = vec![first, target.clone()];
    let scans = folder_scan::caller_scan_count_for_test();
    #[cfg(feature = "test-script")]
    let requests = app.test_script_folder_load_requests;
    assert!(app.favsearch_navigate_sibling(1));
    assert!(app.top_level_grid_view.open_path_classification().is_some());
    settle(&mut app);
    assert_eq!(app.current_folder.as_ref(), Some(&target));
    assert_eq!(app.favsearch.nav_stack, vec![target.clone(), target]);
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    #[cfg(feature = "test-script")]
    assert_eq!(app.test_script_folder_load_requests, requests + 1);
}

#[test]
fn section1853_search_back_keeps_ignored_file_classification_without_stack_effects() {
    for tag in [false, true] {
        let mut app = setup_app();
        let child = directory(&app, "ignored-child");
        let target = app.tmp.path().join("ignored-return.7z");
        std::fs::write(&target, b"classification fixture").unwrap();
        source(&mut app, &child, &[target.clone(), child.clone()], tag);
        app.settings.archive_file_handling = crate::settings::ArchiveFileHandling::Ignore;
        let old_stack = stack(&app, tag).to_vec();
        let address = app.address.clone();
        let scans = folder_scan::caller_scan_count_for_test();
        back(&mut app, tag);
        assert!(app.top_level_grid_view.open_path_classification().is_some());
        settle(&mut app);
        assert_eq!(stack(&app, tag), old_stack);
        assert_eq!(app.address, address);
        assert_eq!(app.current_folder.as_ref(), Some(&child));
        assert!(app.archive_convert.is_none());
        assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
    }
}

#[test]
fn section1853_temporary_view_exit_shares_folder_preflight_and_keeps_exit_authority() {
    let mut app = setup_app();
    let child = directory(&app, "temporary-source");
    let target = directory(&app, "temporary-exit.zip");
    source(&mut app, &child, &[child.clone()], false);
    let scans = folder_scan::caller_scan_count_for_test();
    app.load_folder_after_temporary_view_exit(target.clone());
    let candidate = app.top_level_grid_view.open_path_classification().unwrap();
    assert!(matches!(
        candidate.navigation.as_ref().unwrap().history,
        MainHistoryOperation::Direct(DirectNavigationPurpose::ExitTemporaryView { .. })
    ));
    assert!(app.favsearch.active);
    assert_eq!(app.current_folder.as_ref(), Some(&child));
    settle(&mut app);
    assert!(!app.favsearch.active);
    assert_eq!(app.current_folder.as_ref(), Some(&target));
    assert_eq!(folder_scan::caller_scan_count_for_test(), scans);
}

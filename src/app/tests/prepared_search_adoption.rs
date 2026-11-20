use super::*;
use crate::app::file_type_resume::{PositionResolution, PreparedPageIdentityIndex};
use crate::search_index_db::{IndexEntry, IndexKind};
use crate::tag_view::{
    PreparedTagViewResult, TagViewEntry, TagViewItemKind, TagViewKindFilter, TagViewResult,
};

fn indexed(path: PathBuf, kind: IndexKind) -> IndexEntry {
    IndexEntry {
        display_name: path.file_name().unwrap().to_string_lossy().into_owned(),
        path,
        kind,
        mtime: 17,
    }
}

fn tag_result(entries: Vec<TagViewEntry>) -> TagViewResult {
    TagViewResult {
        query: "alpha".into(),
        kind_filter: TagViewKindFilter::All,
        summaries: Vec::new(),
        entries,
        truncated: false,
        video_thumb_overrides: Default::default(),
    }
}

fn tagged(path: PathBuf, kind: TagViewItemKind, time: i64) -> TagViewEntry {
    TagViewEntry {
        path,
        kind,
        mtime: 17,
        file_size: 23,
        applied_at_ms: Some(time),
    }
}

fn assert_exact_index(app: &App) {
    let expected = PreparedPageIdentityIndex::prepare_for_source(
        &app.items,
        None,
        app.settings.file_type_visibility.clone(),
        file_type_resume::LegacyResumeSource::General,
    );
    assert_eq!(*app.page_identity_index, expected);
    for (idx, item) in app.items.iter().enumerate() {
        if let Some(identity) = crate::book_resume_db::PageIdentity::for_item(item, None) {
            assert_eq!(
                app.page_identity_index
                    .resolve(&crate::book_resume_db::ReadingPosition {
                        page: 999,
                        identity: Some(identity)
                    }),
                PositionResolution::Restored(idx)
            );
        }
    }
}

#[test]
fn favorite_prepared_rows_keep_paths_video_slots_and_index_together() {
    let mut app = setup_app_for_test();
    app.open_favsearch();
    app.settings
        .file_type_visibility
        .excluded_extensions
        .insert("zip".into());
    app.settings.epub_file_handling = crate::settings::EpubFileHandling::Ignore;
    let paths = [
        "folder",
        "hidden.zip",
        "movie.mp4",
        "ignored.epub",
        "kept.pdf",
    ]
    .map(|name| app.tmp.path().join(name));
    let entries = vec![
        indexed(paths[0].clone(), IndexKind::Folder),
        indexed(paths[1].clone(), IndexKind::ZipFile),
        indexed(paths[2].clone(), IndexKind::VideoFile),
        indexed(paths[3].clone(), IndexKind::PdfFile),
        indexed(paths[4].clone(), IndexKind::PdfFile),
    ];
    let prepared =
        prepare_favsearch_results(entries, &app.settings, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        prepared.results_paths,
        [paths[0].clone(), paths[2].clone(), paths[4].clone()]
    );
    assert_eq!(prepared.rows.video_items, [(1, paths[2].clone(), 0)]);
    assert_eq!(prepared.rows.image_metas, vec![Some((17, 0)); 3]);
    let index = Arc::clone(&prepared.rows.page_identity_index);
    app.apply_prepared_favsearch_results(prepared);
    assert!(!app.sidecar_restore_active());
    assert!(Arc::ptr_eq(&index, &app.page_identity_index));
    assert_eq!(
        app.items
            .iter()
            .filter_map(GridItem::drag_source_path)
            .collect::<Vec<_>>(),
        app.favsearch
            .results_paths
            .iter()
            .map(PathBuf::as_path)
            .collect::<Vec<_>>()
    );
    assert_exact_index(&app);
    assert_eq!(
        app.spread_shift.availability,
        crate::spread_shift::PairingEditAvailability::ExtractedResult
    );
    assert!(app.current_spread_shift_scope().is_none());
}

#[test]
fn tag_prepared_rows_keep_applied_times_video_slots_and_index_on_both_adoptions() {
    let mut app = setup_app_for_test();
    app.tag_view.active = true;
    app.tag_view.last_executed = "alpha".into();
    app.settings.epub_file_handling = crate::settings::EpubFileHandling::Ignore;
    let image = app.tmp.path().join("a.jpg");
    let epub = app.tmp.path().join("b.epub");
    let video = app.tmp.path().join("c.mp4");
    for repeat in [false, true] {
        assert_eq!(app.items_are_tag_view, repeat);
        let result = PreparedTagViewResult::prepare(
            tag_result(vec![
                tagged(image.clone(), TagViewItemKind::Image, 11),
                tagged(epub.clone(), TagViewItemKind::PdfFile, 12),
                tagged(video.clone(), TagViewItemKind::Video, 13),
            ]),
            &app.settings,
        );
        assert_eq!(result.rows.video_items, [(1, video.clone(), 23)]);
        assert!(
            !result
                .applied_times
                .contains_key(&crate::tags_db::item_key_for_path(&epub))
        );
        let index = Arc::clone(&result.rows.page_identity_index);
        app.apply_prepared_tag_view_result(result);
        assert!(!app.sidecar_restore_active());
        assert!(Arc::ptr_eq(&index, &app.page_identity_index));
        assert_exact_index(&app);
        assert_eq!(app.tag_view.result_count, 2);
        assert_eq!(app.tag_view_applied_at_ms(0), Some(11));
        assert_eq!(app.tag_view_applied_at_ms(1), Some(13));
    }
}

#[test]
fn prepared_search_condition_mismatch_discards_the_whole_completion() {
    for tag in [false, true] {
        for change_epub in [false, true] {
            let mut app = setup_app_for_test();
            let old = GridItem::Image(app.tmp.path().join("old.jpg"));
            app.items = vec![old.clone()];
            let previous_index = Arc::clone(&app.page_identity_index);
            let generation = app.items_generation;
            app.video_thumb_overrides
                .insert("old".into(), PathBuf::from("old-cover.jpg"));
            app.favsearch.results_paths = vec![PathBuf::from("old-result")];
            app.tag_view.result_count = 7;
            let settings = app.settings.thumbnail_source_discovery_snapshot();
            if change_epub {
                app.settings.epub_file_handling = crate::settings::EpubFileHandling::Ignore;
            } else {
                app.settings
                    .file_type_visibility
                    .excluded_extensions
                    .insert("png".into());
            }
            if tag {
                let prepared = PreparedTagViewResult::prepare(
                    tag_result(vec![tagged(
                        app.tmp.path().join("new.jpg"),
                        TagViewItemKind::Image,
                        10,
                    )]),
                    &settings,
                );
                app.apply_prepared_tag_view_result(prepared);
            } else {
                let prepared = prepare_favsearch_results(
                    vec![indexed(
                        app.tmp.path().join("new-folder"),
                        IndexKind::Folder,
                    )],
                    &settings,
                    &AtomicBool::new(false),
                )
                .unwrap();
                app.apply_prepared_favsearch_results(prepared);
            }
            assert_eq!(app.items, [old]);
            assert_eq!(app.items_generation, generation);
            assert!(Arc::ptr_eq(&app.page_identity_index, &previous_index));
            assert_eq!(app.favsearch.results_paths, [PathBuf::from("old-result")]);
            assert_eq!(app.tag_view.result_count, 7);
            assert_eq!(app.video_thumb_overrides.len(), 1);
            assert!(app.favsearch_pending.is_none());
            assert!(app.tag_view_pending.is_none());
            assert!(!app.sidecar_restore_active());
        }
    }
}

#[test]
fn prepared_search_replacement_and_dismissal_disconnect_old_completions() {
    for tag in [false, true] {
        for dismiss in [false, true] {
            let mut app = setup_app_for_test();
            let cancel = Arc::new(AtomicBool::new(false));
            let old_items = app.items.clone();
            let old_index = Arc::clone(&app.page_identity_index);
            if tag {
                app.tag_view.active = true;
                let (tx, rx) = mpsc::channel();
                app.tag_view_pending = Some(crate::tag_view::TagViewPending::for_test(
                    cancel.clone(),
                    rx,
                ));
                let queued = PreparedTagViewResult::prepare(tag_result(Vec::new()), &app.settings);
                assert!(tx.send(Ok(queued)).is_ok());
                if dismiss {
                    app.dismiss_tag_view_without_restore();
                } else {
                    app.tag_view.query = "replacement".into();
                    app.execute_tag_view();
                }
                assert!(cancel.load(Ordering::Relaxed));
                let late = PreparedTagViewResult::prepare(tag_result(Vec::new()), &app.settings);
                assert!(tx.send(Ok(late)).is_err());
                // Do not poll the live replacement: only prove the retired receiver is gone.
                if dismiss {
                    app.poll_tag_view();
                }
            } else {
                app.favsearch.active = true;
                let (tx, rx) = mpsc::channel();
                app.favsearch_pending = Some(FavSearchPending {
                    cancel: cancel.clone(),
                    rx,
                });
                let queued =
                    prepare_favsearch_results(Vec::new(), &app.settings, &AtomicBool::new(false))
                        .unwrap();
                assert!(tx.send(Ok(queued)).is_ok());
                if dismiss {
                    app.dismiss_favsearch_without_restore();
                } else {
                    app.favsearch.query = "replacement".into();
                    app.execute_favsearch();
                }
                assert!(cancel.load(Ordering::Relaxed));
                let late =
                    prepare_favsearch_results(Vec::new(), &app.settings, &AtomicBool::new(false))
                        .unwrap();
                assert!(tx.send(Ok(late)).is_err());
                if dismiss {
                    app.poll_favsearch();
                }
            }
            assert_eq!(app.items, old_items);
            assert!(Arc::ptr_eq(&app.page_identity_index, &old_index));
        }
    }
}

#[test]
fn search_result_children_replace_parent_index_and_restore_their_own_position() {
    for pdf in [false, true] {
        let mut app = setup_app_for_test();
        app.open_favsearch();
        let source = app
            .tmp
            .path()
            .join(if pdf { "book.pdf" } else { "book.zip" });
        let prepared = prepare_favsearch_results(
            vec![indexed(
                source.clone(),
                if pdf {
                    IndexKind::PdfFile
                } else {
                    IndexKind::ZipFile
                },
            )],
            &app.settings,
            &AtomicBool::new(false),
        )
        .unwrap();
        app.apply_prepared_favsearch_results(prepared);
        let parent_index = Arc::clone(&app.page_identity_index);
        let child_items = (0..3)
            .map(|n| {
                if pdf {
                    GridItem::PdfPage {
                        pdf_path: source.clone(),
                        page_num: n,
                        content_type: None,
                    }
                } else {
                    GridItem::ZipImage {
                        zip_path: source.clone(),
                        entry_name: format!("{n}.jpg"),
                    }
                }
            })
            .collect::<Vec<_>>();
        app.book_resume_db
            .as_ref()
            .unwrap()
            .set(&source, 2)
            .unwrap();
        // A materialized child is an ordinary install, never a prepared search install.
        // Enumeration is represented by its completed rows, so no PDFium process is needed.
        app.favsearch.nav_stack.push(source.clone());
        app.start_loading_items(
            source.clone(),
            child_items.clone(),
            vec![None; 3],
            Default::default(),
            Vec::new(),
            None,
        );
        assert!(app.sidecar_restore_active());
        app.finish_page_identity_preparation_for_test();
        assert!(!Arc::ptr_eq(&parent_index, &app.page_identity_index));
        assert_eq!(app.items, child_items);
        assert_eq!(app.resume_page_for_container(), Some(2));
        for (idx, item) in app.items.iter().enumerate() {
            let position = crate::book_resume_db::ReadingPosition {
                page: 999,
                identity: crate::book_resume_db::PageIdentity::for_item(item, None),
            };
            assert_eq!(
                app.page_identity_index.resolve(&position),
                PositionResolution::Restored(idx)
            );
            assert_eq!(
                parent_index.resolve(&position),
                PositionResolution::HiddenIdentity
            );
        }
    }
}

#[test]
fn physical_nonempty_list_retains_hydration_owner_and_input_gate() {
    let mut app = setup_app_for_test();
    let source = app.tmp.path().join("physical");
    std::fs::create_dir(&source).unwrap();
    let item = GridItem::Image(source.join("page.jpg"));
    for sidecars in [false, true] {
        app.settings.sidecar_backup_enabled = sidecars;
        app.start_loading_items(
            source.clone(),
            vec![item.clone()],
            vec![None],
            Default::default(),
            Vec::new(),
            None,
        );
        assert!(app.sidecar_restore_active());
        assert_eq!(app.modal_dialog_block_reason(), Some("sidecar_restore"));
        app.finish_page_identity_preparation_for_test();
        assert!(!app.sidecar_restore_active());
    }
}

fn finish_favorite(app: &mut App) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.favsearch_pending.is_some() {
        app.poll_favsearch();
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    assert!(!app.sidecar_restore_active());
    assert_exact_index(app);
}

#[test]
fn favorite_combo_or_and_back_keep_the_existing_execution_triggers() {
    use egui_kittest::kittest::Queryable;
    let _input = crate::key_input::lock_test_input();
    let mut app = setup_app_for_test();
    let root = app.tmp.path().join("favorite");
    let other = app.tmp.path().join("other-favorite");
    std::fs::create_dir(&root).unwrap();
    std::fs::create_dir(&other).unwrap();
    let entries = ["alpha", "beta"].map(|name| {
        let path = root.join(name);
        std::fs::create_dir(&path).unwrap();
        indexed(path, IndexKind::Folder)
    });
    let db =
        crate::search_index_db::SearchIndexDb::open_at(&app.tmp.path().join("search.db")).unwrap();
    db.upsert_children(&root, &root, &entries).unwrap();
    app.search_index_db = Some(Arc::new(db));
    app.settings.favorites.clear();
    for (name, path) in [("primary", root.clone()), ("other", other)] {
        let mut favorite = crate::settings::FavoriteEntry::new(name.into(), path);
        favorite.auto_index_structure = true;
        app.settings.favorites.push(favorite);
    }
    app.settings.show_toolbar_search_container = false;
    app.open_favsearch();
    app.favsearch.query = "alpha beta".into();
    app.execute_favsearch();
    finish_favorite(&mut app);
    assert!(app.items.is_empty());
    let mut fonts_ready = false;
    let mut harness = crate::test_ui::builder()
        .with_size(egui::vec2(1600.0, 700.0))
        .build_ui_state(
            move |root, app: &mut AppTestEnvForTest| {
                let ctx = root.ctx().clone();
                if !fonts_ready {
                    crate::ime_focus::install_ime_input_policy(&ctx);
                    crate::ui_fonts::configure_fonts(&ctx);
                    fonts_ready = true;
                    ctx.request_repaint();
                    return;
                }
                app.update_ime_state(&ctx);
                app.render_favsearch_bar(root);
            },
            app,
        );
    harness.run();
    harness.get_by_label("OR").click();
    harness.run();
    assert!(harness.state().favsearch_pending.is_some());
    finish_favorite(harness.state_mut());
    assert_eq!(harness.state().items.len(), 2);
    harness
        .get_by_label("\u{304a}\u{6c17}\u{306b}\u{5165}\u{308a}")
        .click();
    harness.run();
    harness.get_by_label("other").click();
    harness.run();
    assert!(harness.state().favsearch_pending.is_some());
    finish_favorite(harness.state_mut());
    assert!(harness.state().items.is_empty());
    harness
        .get_by_label("\u{304a}\u{6c17}\u{306b}\u{5165}\u{308a}")
        .click();
    harness.run();
    harness.get_by_label("primary").click();
    harness.run();
    finish_favorite(harness.state_mut());
    assert_eq!(harness.state().items.len(), 2);
    harness.get_by_label("\u{7a2e}\u{5225}").click();
    harness.run();
    harness.get_by_label("ZIP").click();
    harness.run();
    assert_eq!(
        harness.state().favsearch.kind_filter,
        Some(IndexKind::ZipFile)
    );
    assert!(harness.state().favsearch_pending.is_some());
    finish_favorite(harness.state_mut());
    assert!(harness.state().items.is_empty());
    harness.get_by_label("\u{7a2e}\u{5225}").click();
    harness.run();
    harness
        .get_by_label("\u{30d5}\u{30a9}\u{30eb}\u{30c0}")
        .click();
    harness.run();
    finish_favorite(harness.state_mut());
    let app = harness.state_mut();
    let child = root.join("alpha");
    app.favsearch.nav_stack.push(child);
    app.favsearch_back();
    assert!(app.favsearch.nav_stack.is_empty());
    assert!(app.favsearch_pending.is_some());
    finish_favorite(app);
    assert_eq!(app.items.len(), 2);
    assert_eq!(app.favsearch.last_executed, "alpha beta");
    assert!(app.favsearch.or_mode);
}

#[test]
fn prepared_index_argument_does_not_bypass_nonsearch_hydration() {
    for search_surface in [false, true] {
        let mut app = setup_app_for_test();
        let source = if search_surface {
            search_results_synthetic_path()
        } else {
            app.tmp.path().join("ordinary")
        };
        let item = GridItem::Image(app.tmp.path().join("a.jpg"));
        let index = Arc::new(PreparedPageIdentityIndex::prepare(
            &[item.clone()],
            None,
            app.settings.file_type_visibility.clone(),
        ));
        // Even supplying an index cannot opt a physical or a different extracted
        // result into the Favorite/Tag-only contract.
        if search_surface {
            app.start_loading_extracted_items(
                book_materialization::ExtractedSurface::Rating,
                source,
                vec![item],
                vec![None],
                Default::default(),
                Vec::new(),
                None,
                Some(index),
            );
        } else {
            app.start_loading_items_inner(
                source,
                vec![item],
                vec![None],
                Default::default(),
                Vec::new(),
                None,
                None,
                None,
                None,
                None,
                None,
                Some(index),
                VisibleInstallAuthority::Ordinary,
            );
        }
        assert!(app.sidecar_restore_active());
        app.finish_page_identity_preparation_for_test();
    }
}

#[test]
fn tag_nonempty_query_zero_results_uses_prepared_index_on_initial_and_repeat() {
    let mut app = setup_app_for_test();
    app.tag_view.active = true;
    app.tag_view.last_executed = "absent".into();
    for repeat in [false, true] {
        assert_eq!(app.items_are_tag_view, repeat);
        let mut result = tag_result(Vec::new());
        result.query = "absent".into();
        let prepared = PreparedTagViewResult::prepare(result, &app.settings);
        let index = Arc::clone(&prepared.rows.page_identity_index);
        app.apply_prepared_tag_view_result(prepared);
        assert!(app.items.is_empty());
        assert_eq!(app.tag_view.result_count, 0);
        assert!(!app.sidecar_restore_active());
        assert!(Arc::ptr_eq(&index, &app.page_identity_index));
        assert_exact_index(&app);
    }
}

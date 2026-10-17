// G3 review regressions use actual open/snapshot consumers and explicit FIFO fences.

#[cfg(windows)]
#[test]
#[cfg(test)]
fn g3_hidden_extracted_candidate_terminates_initial_detached_permission_open() {
    use crate::book_composition::fixtures::document_page;
    let mut env = setup_app();
    let root = env.tmp.path().join("result-hidden.pdf");
    let scope = BookScopeKey::document(&root).unwrap();
    let main_items = env.items.clone();
    let main_generation = env.items_generation;
    let data = env.tmp.path().join("result-hidden-data");
    let (service, owner) = runtime(&mut env, &data);
    let mut composition = four_page_book(ReadingDirection::Ltr);
    composition.hidden_pages.push(document_page(0));
    service
        .save(scope.clone(), PageDomain::Document, composition)
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    let window = 9923;
    env.build_active_context_for_test(Some(window), DetachedSource::Book, |app| {
        install_document(app, &root);
        app.projection_recipe = book_materialization::ProjectionRecipe::ExtractedResult(
            book_materialization::ExtractedSurface::Rating,
        );
        app.begin_extracted_result_view();
        app.viewer_presentation = ViewerPresentation::DetachedWindow;
        app.navigation_scope = ViewerNavigationScope::DetachedPhysical;
        app.detached_viewer_independent_active = true;
        app.fullscreen_idx = None;
    });
    env.with_active_viewer_context(|app| {
        // This path checks the candidate instead of adopting a Book projection.
        // It must share the same terminal rejection, not just its toast.
        app.open_fullscreen(0, HistoryTrigger::UserChosen);
        barrier(&service, &scope);
        app.poll_book_presentation_permission(&egui::Context::default());
        assert!(app.fullscreen_idx.is_none());
        assert_eq!(
            app.detached_window_state(window),
            Some(DetachedWindowState::Closing)
        );
    })
    .unwrap();
    let ctx = egui::Context::default();
    ctx.run_pass(egui::RawInput::default(), |ctx| {
        ctx.root_ui(|ui| env.update_active_viewer_context(ui, None));
    })
    .drop_without_applying_deltas();
    assert!(env.active_viewer_context_id().is_none());
    assert!(env.active_detached_session.is_none());
    assert_eq!(env.detached_window_state(window), None);
    assert_eq!(env.items, main_items);
    assert_eq!(env.items_generation, main_generation);
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
#[cfg(test)]
fn g3_hidden_required_detached_open_terminates_session_runtime_and_preserves_siblings() {
    use crate::book_composition::fixtures::document_page;
    for shell_active in [false, true] {
        let mut env = setup_app();
        let main = env.tmp.path().join("main.pdf");
        let main_scope = install_document(&mut env, &main);
        let data = env.tmp.path().join("hidden-detached-data");
        let (service, owner) = runtime(&mut env, &data);
        adopt(&mut env, &service, &main_scope);
        env.selected = Some(2);
        let main_items = env.items.clone();
        let main_generation = env.items_generation;
        let main_composition = env.book_composition.clone();
        let sibling_window = 9921;
        let sibling_root = env.tmp.path().join("sibling.pdf");
        let sibling = env.build_window_context_for_test(sibling_window, |app| {
            install_document(app, &sibling_root);
            app.selected = Some(3);
        });
        let sibling_before = env.with_window_context_for_test(sibling, |app| {
            (
                app.items.clone(),
                app.items_generation,
                app.book_composition.clone(),
                app.selected,
                Arc::clone(&app.cancel_token),
            )
        });
        let sibling_runtime = env.detached_window_state(sibling_window);
        let root = env.tmp.path().join("hidden-initial.pdf");
        let scope = BookScopeKey::document(&root).unwrap();
        let mut composition = four_page_book(ReadingDirection::Ltr);
        composition.hidden_pages.push(document_page(0));
        service
            .save(scope.clone(), PageDomain::Document, composition)
            .recv_timeout(WAIT)
            .unwrap()
            .status
            .unwrap();
        let window = 9922;
        env.build_active_context_for_test(Some(window), DetachedSource::Book, |app| {
            install_document(app, &root);
            app.viewer_presentation = ViewerPresentation::DetachedWindow;
            app.navigation_scope = ViewerNavigationScope::DetachedPhysical;
            app.detached_viewer_independent_active = true;
            app.fullscreen_idx = None;
        });
        assert_eq!(
            env.active_detached_session.unwrap().content_phase,
            DetachedSessionContentPhase::Preparing
        );
        if shell_active {
            env.transition_detached_window_state(
                window,
                DetachedWindowState::Active,
                "test_loading_shell_presented",
            );
        }
        env.with_active_viewer_context(|app| {
            let deferred = DeferredFsReopen {
                history_trigger: HistoryTrigger::UserChosen,
                resume_slideshow: false,
                target: DeferredFsTarget::Required(crate::snapshot::SnapshotTarget::PdfPage {
                    pdf_path: root.clone(),
                    page_num: 0,
                }),
                resume_to_last_page: false,
                from_explicit_open: true,
                preserve_after_password_prompt: true,
            };
            assert_eq!(
                app.open_deferred_fullscreen_after_enumerate(deferred),
                DeferredFsOpenOutcome::WaitingComposition
            );
            adopt(app, &service, &scope);
            assert!(!app.book_deferred_open_wait_active());
            assert!(app.fullscreen_idx.is_none());
            assert_eq!(
                app.detached_window_state(window),
                Some(DetachedWindowState::Closing)
            );
        })
        .expect("hidden open must finish in its owning context");
        let ctx = egui::Context::default();
        ctx.run_pass(egui::RawInput::default(), |ctx| {
            ctx.root_ui(|ui| env.update_active_viewer_context(ui, None));
        })
        .drop_without_applying_deltas();
        assert!(env.active_viewer_context_id().is_none());
        assert!(env.active_detached_session.is_none());
        assert_eq!(env.detached_window_state(window), None);
        assert_eq!(env.items, main_items);
        assert_eq!(env.items_generation, main_generation);
        assert_eq!(env.book_composition, main_composition);
        assert_eq!(env.selected, Some(2));
        assert_eq!(env.detached_window_state(sibling_window), sibling_runtime);
        env.with_window_context_for_test(sibling, |app| {
            assert_eq!(app.items, sibling_before.0);
            assert_eq!(app.items_generation, sibling_before.1);
            assert_eq!(app.book_composition, sibling_before.2);
            assert_eq!(app.selected, sibling_before.3);
            assert!(Arc::ptr_eq(&app.cancel_token, &sibling_before.4));
            assert!(!app.cancel_token.load(Ordering::Relaxed));
        });
        owner.shutdown_and_join();
    }
}

#[test]
#[cfg(test)]
fn g3_automatic_zip_snapshot_return_preserves_memory_off_session_shift_after_generation_poll() {
    use super::spread_shift_owner::ViewerShift;
    use crate::spread_shift::{
        ShiftPersistence, ShiftResolution, SpreadShiftPageRef, SpreadShiftState,
    };
    use std::io::Write;
    let mut env = setup_app();
    let root = env.tmp.path().join("automatic-session.zip");
    let mut writer = zip::ZipWriter::new(std::fs::File::create(&root).unwrap());
    for name in ["a.jpg", "b.jpg", "c.jpg", "d.jpg"] {
        writer
            .start_file(name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(b"test page").unwrap();
    }
    writer.finish().unwrap();
    let tree = Arc::new(crate::zip_tree::ZipTree::build(
        root.clone(),
        crate::zip_loader::enumerate_image_entries(&root).unwrap(),
    ));
    let nav = crate::zip_tree::ZipNavState::new(tree);
    let prefix = nav.current_raw().to_vec();
    let (items, metas) = nav.materialize_current(crate::settings::SortOrder::FileName);
    env.current_folder = Some(root.clone());
    env.items = items;
    env.image_metas = metas;
    env.thumbnails = vec![ThumbnailState::Pending; 4];
    env.visible_indices = (0..4).collect();
    env.page_identity_index = Arc::new(file_type_resume::PreparedPageIdentityIndex::prepare(
        &env.items,
        Some(&root),
        env.settings.file_type_visibility.clone(),
    ));
    env.projection_recipe =
        book_materialization::ProjectionRecipe::Book(book_materialization::BookScope::Archive {
            logical_root: root.clone(),
            prefix: prefix.clone(),
        });
    env.zip_nav = Some(nav);
    env.spread_mode = crate::settings::SpreadMode::Ltr;
    env.settings.remember_spread_shift_per_book = false;
    let scope = BookScopeKey::archive(&root, &prefix).unwrap();
    let data = env.tmp.path().join("automatic-session-data");
    let (service, owner) = runtime(&mut env, &data);
    adopt(&mut env, &service, &scope);
    assert!(env.set_live_spread_shift(1));
    assert_eq!(env.spread_shift_anchor_idx(), Some(1));
    let before = database_bytes(&data);
    env.activate_snapshot(crate::snapshot::SnapshotSourceLabel::Mixed);
    assert!(env.extracted_result_surface().is_some());
    env.deactivate_snapshot();
    env.settle_snapshot_page_index_for_test();
    assert_eq!(env.spread_shift_anchor_idx(), Some(1));
    // Returning installs the retained rows with a new generation. The runtime
    // must reclassify Automatic without turning this into CompositionRemoved.
    adopt(&mut env, &service, &scope);
    env.viewer_navigation_caches.invalidate();
    let indices = env.get_nav_indices();
    let index = Arc::clone(&env.page_identity_index);
    env.spread_shift.resolve(&index, &indices);
    env.poll_retained_shift_readoption(&egui::Context::default());
    assert_eq!(env.spread_shift_anchor_idx(), Some(1));
    assert!(
        matches!(&env.spread_shift.state, ViewerShift::Book(SpreadShiftState::Adjusted {
        anchor_ref: SpreadShiftPageRef::ArchiveEntry(name), resolution: ShiftResolution::Resolved(1),
        persistence: ShiftPersistence::SessionOnly,
    }) if name == "b.jpg")
    );
    assert!(!env.spread_shift.retained_readoption_pending());
    assert_eq!(database_bytes(&data), before);
    assert!(service.snapshot().rows.is_empty());
    owner.shutdown_and_join();
}

#[test]
#[cfg(test)]
fn g3_document_direction_mode_only_release_and_pc_reentry_agree() {
    use crate::pdf_loader::PdfReadingDirection;
    use crate::settings::{ReadingDirection as Direction, SpreadMode};
    use crate::spread_db::StoredViewMode;
    use crate::ui_fullscreen::FullscreenViewMode;

    for (view, class, rtl_mode, ltr_mode) in [
        (
            FullscreenViewMode::Single,
            StoredViewMode::Single,
            SpreadMode::Single,
            SpreadMode::Single,
        ),
        (
            FullscreenViewMode::Spread,
            StoredViewMode::Spread,
            SpreadMode::RtlCover,
            SpreadMode::LtrCover,
        ),
        (
            FullscreenViewMode::LandscapeSplit,
            StoredViewMode::LandscapeSplit,
            SpreadMode::SplitRtl,
            SpreadMode::SplitLtr,
        ),
    ] {
        for follow in [true, false] {
            let mut env = setup_app();
            let root = env.tmp.path().join("rtl-mode-only.epub");
            let _pin = crate::pdf_loader::pin_epub_with_direction_for_test(
                &root,
                707,
                400,
                PdfReadingDirection::R2L,
            );
            let scope = install_document(&mut env, &root);
            env.settings.follow_document_reading_direction = follow;
            env.settings.default_spread_mode = SpreadMode::LtrCover;
            env.settings.default_reading_direction = Direction::Ltr;
            let data = crate::data_dir::get();
            env.spread_db =
                Ok(crate::spread_db::SpreadDb::open_at(&data.join("spread.db")).unwrap());
            let (service, owner) = runtime(&mut env, &data);
            save(&service, &scope, ReadingDirection::Ltr);
            admit_projection(&mut env, &service, &scope);
            settle_projection(&mut env);
            assert!(env.composed_book_layout().is_some());
            // Selecting the already-active Spread is a no-op, not a save.
            env.apply_fullscreen_view_mode(
                &egui::Context::default(),
                0,
                FullscreenViewMode::Single,
            );
            env.apply_fullscreen_view_mode(&egui::Context::default(), 0, view);
            barrier(&service, &scope);
            let stored = env
                .spread_db
                .as_ref()
                .unwrap()
                .get_state_with_fallback(&root, None);
            assert_eq!(stored.mode, None);
            assert_eq!(stored.direction, None);
            assert_eq!(stored.view_mode, Some(class));

            service
                .remove(scope.clone(), PageDomain::Document)
                .recv_timeout(WAIT)
                .unwrap()
                .status
                .unwrap();
            admit_projection(&mut env, &service, &scope);
            settle_projection(&mut env);
            assert!(env.composed_book_layout().is_none());
            let expected = (
                if follow { rtl_mode } else { ltr_mode },
                if follow {
                    Direction::Rtl
                } else {
                    Direction::Ltr
                },
            );
            let released = (env.spread_mode, env.reading_direction);
            assert_eq!(released, expected, "release {class:?}, follow={follow}");

            install_document(&mut env, &root);
            let defaults = SpreadRestoreDefaults::for_book(&env.settings);
            env.apply_spread_for_key_with_document_direction(
                &root,
                None,
                defaults,
                Some(PdfReadingDirection::R2L),
            )
            .unwrap();
            adopt(&mut env, &service, &scope);
            assert_eq!(
                (env.spread_mode, env.reading_direction),
                released,
                "PC reentry {class:?}, follow={follow}"
            );
            assert_eq!(
                env.spread_db
                    .as_ref()
                    .unwrap()
                    .get_state_with_fallback(&root, None),
                stored
            );
            owner.shutdown_and_join();
        }
    }
}

#[test]
#[cfg(test)]
fn g3_document_direction_release_retains_accepted_epub_generation() {
    use crate::pdf_loader::PdfReadingDirection;
    use crate::settings::{ReadingDirection as Direction, SpreadMode};
    let mut env = setup_app();
    let root = env.tmp.path().join("retained-rtl.epub");
    let _old_pin = crate::pdf_loader::pin_epub_with_direction_for_test(
        &root,
        801,
        400,
        PdfReadingDirection::R2L,
    );
    let scope = install_document(&mut env, &root);
    env.settings.follow_document_reading_direction = true;
    env.settings.default_spread_mode = SpreadMode::LtrCover;
    env.settings.default_reading_direction = Direction::Ltr;
    let data = crate::data_dir::get();
    env.spread_db = Ok(crate::spread_db::SpreadDb::open_at(&data.join("spread.db")).unwrap());
    let (service, owner) = runtime(&mut env, &data);
    save(&service, &scope, ReadingDirection::Ltr);
    admit_projection(&mut env, &service, &scope);
    settle_projection(&mut env);
    env.apply_fullscreen_view_mode(
        &egui::Context::default(),
        0,
        crate::ui_fullscreen::FullscreenViewMode::Single,
    );
    env.apply_fullscreen_view_mode(
        &egui::Context::default(),
        0,
        crate::ui_fullscreen::FullscreenViewMode::Spread,
    );
    barrier(&service, &scope);
    // Another reader can publish a newer generation. Removal is a projection of
    // this viewer's accepted source, not a new open of the latest generation.
    let _new_pin = crate::pdf_loader::pin_epub_with_direction_for_test(
        &root,
        802,
        500,
        PdfReadingDirection::L2R,
    );
    service
        .remove(scope.clone(), PageDomain::Document)
        .recv_timeout(WAIT)
        .unwrap()
        .status
        .unwrap();
    admit_projection(&mut env, &service, &scope);
    settle_projection(&mut env);
    assert_eq!(
        (env.spread_mode, env.reading_direction),
        (SpreadMode::RtlCover, Direction::Rtl)
    );
    owner.shutdown_and_join();
}

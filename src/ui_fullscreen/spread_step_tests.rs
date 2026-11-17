// §1.796: these exercise the accepted handler and its actual topology owner.
fn step_app(count: usize, mode: SpreadMode) -> crate::app::AppTestEnvForTest {
    let mut app = final_cover_local_test_app(count);
    app.thumbnails = vec![ThumbnailState::Pending; count];
    app.image_metas = vec![None; count];
    app.spread_mode = mode;
    app.fullscreen_idx = Some(0);
    app.settings.page_turn_animation_enabled = false;
    app.settings.prefetch_forward = 0;
    app.settings.prefetch_back = 0;
    app
}

fn step_pages(app: &mut crate::app::AppTestEnvForTest) -> Vec<Vec<usize>> {
    let nav = app.get_nav_indices();
    app.build_spread_display_units_for_nav(&nav)
        .iter()
        .map(|u| u.pages.clone())
        .collect()
}

fn step_target(app: &mut crate::app::AppTestEnvForTest, dir: i32) -> Option<usize> {
    let mut action = FsKeyAction::default();
    let idx = app.fullscreen_idx.unwrap();
    app.queue_spread_step_action(idx, dir, &mut action);
    match action.page_nav {
        FsPageNav::Target(idx) => {
            app.fullscreen_idx = Some(idx);
            Some(idx)
        }
        FsPageNav::None => None,
        other => panic!("unexpected step navigation: {other:?}"),
    }
}

#[test]
fn spread_step_persists_through_ordinary_navigation_and_repeated_steps() {
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        let mut app = step_app(7, mode);
        assert_eq!(step_target(&mut app, 1), Some(1));
        assert_eq!(
            step_pages(&mut app),
            vec![vec![0], vec![1, 2], vec![3, 4], vec![5, 6]]
        );
        assert_eq!(app.spread_page_nav(1), FsPageNav::Target(3));
        app.fullscreen_idx = Some(3);
        assert_eq!(step_target(&mut app, -1), Some(2));
        assert_eq!(
            step_pages(&mut app),
            vec![vec![0, 1], vec![2, 3], vec![4, 5], vec![6]]
        );
        assert!(app.spread_step.is_some());
        assert_eq!(step_target(&mut app, 1), Some(3));
        assert_eq!(step_target(&mut app, 1), Some(4));
        assert_eq!(
            step_pages(&mut app),
            vec![vec![0, 1], vec![2, 3], vec![4, 5], vec![6]]
        );
    }
}

#[test]
fn spread_step_ends_do_not_wrap_or_change_state() {
    let mut app = step_app(5, SpreadMode::Ltr);
    app.settings.fullscreen_manual_nav_wrap = true;
    assert_eq!(step_target(&mut app, -1), None);
    assert!(app.spread_step.is_none());
    app.fullscreen_idx = Some(4);
    assert_eq!(step_target(&mut app, 1), None);
    assert!(app.spread_step.is_none());
    app.fullscreen_idx = Some(2);
    assert_eq!(step_target(&mut app, 1), Some(3));
    // The last page of a pair is still a valid exact start.
    assert_eq!(step_target(&mut app, 1), Some(4));
    let last = app.spread_step;
    assert_eq!(step_target(&mut app, 1), None);
    assert_eq!(app.spread_step, last);
}

#[test]
fn spread_step_uses_nav_positions_and_effective_unit_start() {
    let mut app = step_app(7, SpreadMode::Rtl);
    app.visible_indices = vec![6, 2, 5, 1, 4];
    app.details_order = app.visible_indices.clone();
    app.viewer_navigation_caches.invalidate();
    app.fullscreen_idx = Some(2); // second page of the first unit
    let nav = app.get_nav_indices();
    let units = app.build_spread_display_units_for_nav(&nav);
    let start = find_spread_display_unit(&units, 2).unwrap().1.nav_start;
    assert_eq!(step_target(&mut app, 1), Some(nav[start + 1]));
    assert_eq!(app.spread_step.unwrap().start_idx, nav[start + 1]);
}

#[test]
fn spread_step_exact_start_preserves_cover_and_forced_body_boundaries() {
    use crate::settings::{PageAlonePreference, PageAlonePreferences};
    for mode in [SpreadMode::LtrCover, SpreadMode::RtlCover] {
        let mut app = step_app(7, mode);
        app.fullscreen_idx = Some(1);
        assert_eq!(step_target(&mut app, -1), Some(0));
        assert_eq!(step_pages(&mut app)[0], vec![0]);
        app.page_alone_preferences = PageAlonePreferences {
            after_cover: PageAlonePreference::On,
            last: PageAlonePreference::On,
        };
        app.fullscreen_idx = Some(2);
        assert_eq!(step_target(&mut app, 1), Some(3));
        assert_eq!(
            step_pages(&mut app),
            vec![vec![0], vec![1], vec![2], vec![3, 4], vec![5], vec![6]]
        );
    }
}

#[test]
fn spread_step_landscape_and_media_boundaries_keep_singletons() {
    let mut app = step_app(7, SpreadMode::Ltr);
    app.record_page_dims_for_spread(2, (2000, 1000));
    app.items[4] = GridItem::Video(PathBuf::from("c:/book/4.mp4"));
    assert_eq!(step_target(&mut app, 1), Some(1));
    assert_eq!(
        step_pages(&mut app),
        vec![vec![0], vec![1], vec![2], vec![3], vec![4], vec![5, 6]]
    );
}

#[test]
fn spread_step_does_not_write_live_shift_or_notify_with_memory_on() {
    let mut app = step_app(6, SpreadMode::Ltr);
    app.settings.remember_spread_shift_per_book = true;
    app.set_spread_shift_anchor_for_test(Some(1));
    app.get_nav_indices();
    let before = format!("{:?}", app.spread_shift);
    let toast = app.fs_feedback_toast.clone();
    assert_eq!(step_target(&mut app, 1), Some(1));
    assert_eq!(format!("{:?}", app.spread_shift), before);
    assert_eq!(app.fs_feedback_toast, toast);
    assert_eq!(app.spread_shift_anchor_idx(), Some(1));
    assert_eq!(step_target(&mut app, 1), Some(2));
    assert_eq!(
        step_pages(&mut app),
        vec![vec![0, 1], vec![2, 3], vec![4, 5]]
    );
    assert_eq!(format!("{:?}", app.spread_shift), before);
    assert_eq!(app.fs_feedback_toast, toast);
    assert_eq!(app.spread_shift_anchor_idx(), Some(1));
}

#[test]
fn spread_step_nonspread_and_continuous_use_ordinary_page_resolution() {
    for mode in [SpreadMode::Single, SpreadMode::SplitLtr, SpreadMode::Ltr] {
        for flow in [ReadingFlow::Paged, ReadingFlow::Horizontal] {
            if mode.is_spread() && flow.is_paged() {
                continue;
            }
            for dir in [-1, 1] {
                let mut app = step_app(5, mode);
                app.reading_flow = flow;
                app.settings.fullscreen_manual_nav_wrap = true;
                if mode.is_split() {
                    app.record_page_dims_for_spread(0, (2000, 1000));
                }
                let expected = app.spread_page_nav(dir);
                if mode.is_split() && flow.is_paged() && dir == 1 {
                    assert!(matches!(expected,FsPageNav::Split(step) if step.source_idx == 0));
                }
                let mut action = FsKeyAction::default();
                app.queue_spread_step_action(0, dir, &mut action);
                assert_eq!(action.page_nav, expected, "{mode:?} {flow:?}");
                assert!(app.spread_step.is_none());
            }
        }
    }
}

#[test]
fn spread_step_refused_sources_leave_state_and_topology_unchanged() {
    use crate::spread_shift::PairingEditAvailability;
    for availability in [
        PairingEditAvailability::Unreadable,
        PairingEditAvailability::ExtractedResult,
    ] {
        let mut app = book_composition_tests::composed_app(
            &crate::book_composition::fixtures::four_page_book(
                crate::book_composition::ReadingDirection::Ltr,
            ),
        );
        app.fullscreen_idx = Some(0);
        if availability == PairingEditAvailability::Unreadable {
            app.book_composition =
                crate::app::book_composition_runtime::ViewerBookComposition::Blocked {
                    scope: crate::book_scope::BookScopeKey::document("c:/g3-display.pdf").unwrap(),
                    token: crate::book_composition::CompositionToken {
                        service_epoch: 3,
                        publication_sequence: 7,
                    },
                    error: "injected unreadable composition".into(),
                };
        } else {
            app.projection_recipe =
                crate::app::book_materialization::ProjectionRecipe::ExtractedResult(
                    crate::app::book_materialization::ExtractedSurface::Rating,
                );
        }
        let old = step_pages(&mut app);
        assert_eq!(step_target(&mut app, 1), None);
        assert!(app.spread_step.is_none());
        assert_eq!(step_pages(&mut app), old);
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .unwrap()
                .0
                .contains("1 ページ送り")
        );
        // Source refusal still precedes the ordinary in-flight target guard.
        app.fs_feedback_toast = None;
        assert!(app.begin_fs_page_navigation_sequence(&egui::Context::default(), 0, 1, false));
        assert!(app.fs_navigation_sequence_blocks_new_target());
        assert_eq!(step_target(&mut app, 1), None);
        assert!(app.spread_step.is_none());
        assert!(
            app.fs_feedback_toast
                .as_ref()
                .unwrap()
                .0
                .contains("1 ページ送り")
        );
    }
}

#[test]
fn spread_step_retirement_mode_flow_direction_and_shift_roundtrips() {
    let ctx = egui::Context::default();
    for transition in [0, 1, 2, 3, 4, 5] {
        let mut app = step_app(6, SpreadMode::Ltr);
        step_target(&mut app, 1);
        assert!(app.spread_step.is_some());
        match transition {
            0 => {
                app.apply_fullscreen_spread_mode(&ctx, 1, SpreadMode::Single);
                app.apply_fullscreen_spread_mode(&ctx, 1, SpreadMode::Ltr);
            }
            1 => {
                app.set_reading_flow_for_fullscreen(&ctx, 1, ReadingFlow::Horizontal);
                app.set_reading_flow_for_fullscreen(&ctx, 1, ReadingFlow::Paged);
            }
            2 => {
                app.set_reading_direction_for_fullscreen(&ctx, 1, ReadingDirection::Rtl);
                app.set_reading_direction_for_fullscreen(&ctx, 1, ReadingDirection::Ltr);
            }
            3 => {
                let mut action = FsKeyAction::default();
                app.queue_spread_shift_action(1, 1, &mut action);
                assert_eq!(action.page_nav, FsPageNav::Target(2));
            }
            4 => {
                app.apply_fullscreen_spread_mode(&ctx, 1, SpreadMode::Ltr);
            }
            _ => {
                app.reset_live_spread_shift(true);
            }
        }
        assert!(app.spread_step.is_none(), "transition {transition}");
        assert_eq!(
            step_pages(&mut app),
            vec![vec![0, 1], vec![2, 3], vec![4, 5]]
        );
    }
}

#[test]
fn spread_step_rotation_reuse_and_final_demand_validate_exact_start() {
    let mut app = step_app(6, SpreadMode::Ltr);
    let nav = app.get_nav_indices();
    app.build_spread_display_units_for_nav(&nav);
    step_target(&mut app, 1);
    assert!(!app.final_demand_visible_inputs_are_current());
    assert!(
        app.spread_display_units_cache
            .units_during_rotation_reload(
                &nav,
                app.items_generation,
                app.spread_mode,
                app.spread_shift_anchor_idx(),
                Some(1),
                app.page_alone_enabled_for_current_book()
            )
            .is_none()
    );
    let units = app.build_spread_display_units_for_nav(&nav);
    assert!(app.final_demand_visible_inputs_are_current());
    assert_eq!(units[1].pages, vec![1, 2]);
    app.settings.page_turn_animation_enabled = true;
    app.prepare_final_demand_topologies();
    assert!(
        app.spread_display_units_cache
            .final_demand_topologies
            .as_ref()
            .unwrap()
            .same_visible(
                app.spread_display_units_cache.token.as_ref().unwrap(),
                &units
            )
    );
}

#[test]
fn spread_step_still_only_nav_does_not_retire_missing_media_start() {
    let mut app = step_app(6, SpreadMode::Ltr);
    app.items[3] = GridItem::Video(PathBuf::from("c:/book/movie.mp4"));
    step_target(&mut app, 1);
    let still = app.get_still_image_indices();
    let units = app.build_spread_display_units_for_nav(&still);
    assert_eq!(units[1].pages, vec![1, 2]);
    assert_eq!(
        app.spread_page_nav_for_indices(&still, 1, 1),
        FsPageNav::Target(4)
    );
    app.spread_step = Some(crate::app::spread_shift_owner::SpreadStep { start_idx: 3 });
    let units = app.build_spread_display_units_for_nav(&still);
    assert_eq!(units[0].pages, vec![0, 1]);
    assert_eq!(app.spread_step.unwrap().start_idx, 3);
}

#[test]
fn spread_step_keymap_no_defaults_and_coalescing_inventory() {
    for action in [KeyAction::FsSpreadStepPrev, KeyAction::FsSpreadStepNext] {
        assert!(action.default_chords().is_empty());
        assert!(FS_PAGE_TURN_COALESCE_ACTIONS.contains(&action));
        assert_eq!(action.context(), crate::keymap::KeyContext::FsImage);
    }
}

#[test]
fn spread_step_composed_directives_direction_cover_and_final_companions() {
    use crate::book_composition::ReadingDirection as D;
    use crate::book_composition::{PageOverride, PageOverrideEntry, PageSide, fixtures};
    for direction in [D::Ltr, D::Rtl] {
        for cover in [false, true] {
            for directive in [
                PageOverride::Alone,
                PageOverride::BlankOpposite {
                    page_side: PageSide::Leading,
                },
            ] {
                for page in [1, 3] {
                    let mut book = fixtures::four_page_book(direction);
                    book.cover_enabled = cover;
                    book.page_overrides.push(PageOverrideEntry {
                        page: fixtures::document_page(page),
                        directive,
                    });
                    let mut app = book_composition_tests::composed_app(&book);
                    app.fullscreen_idx = Some(if page == 1 { 0 } else { 2 });
                    app.settings.final_cover_spread_enabled = true;
                    app.settings.page_after_cover_alone_enabled = cover;
                    app.settings.last_page_alone_enabled = cover;
                    let shift = format!("{:?}", app.spread_shift);
                    let record = format!("{:?}", app.book_composition);
                    assert_eq!(step_target(&mut app, 1), Some(page as usize));
                    let nav = app.get_nav_indices();
                    let units = app.build_spread_display_units_for_nav(&nav);
                    let (_, unit) = find_spread_display_unit(&units, page as usize).unwrap();
                    assert_eq!(unit.pages, vec![page as usize]);
                    let composition = app.spread_display_composition_for_anchor(page as usize);
                    let pages = composition.pages_in_screen_order(app.spread_mode);
                    if matches!(directive, PageOverride::BlankOpposite { .. }) {
                        assert!(
                            pages
                                .iter()
                                .all(|p| p.role != SpreadPageRole::FrontCoverSupplement)
                        );
                    } else if cover && page == 3 {
                        assert!(
                            pages
                                .iter()
                                .any(|p| p.role == SpreadPageRole::FrontCoverSupplement)
                        );
                    }
                    assert_eq!(format!("{:?}", app.spread_shift), shift);
                    assert_eq!(format!("{:?}", app.book_composition), record);
                }
            }
        }
    }
}

#[test]
fn spread_step_composed_permission_wait_keeps_origin_and_edit_retires() {
    use crate::book_composition::{
        BookCompositionService, PageDomain, ReadingDirection as D, fixtures,
    };
    use std::time::Duration;
    let data = tempfile::tempdir().unwrap();
    let scope = crate::book_scope::BookScopeKey::document("c:/g3-display.pdf").unwrap();
    let (service, owner) =
        BookCompositionService::spawn_at(data.path().to_path_buf(), None).unwrap();
    service
        .save(
            scope.clone(),
            PageDomain::Document,
            fixtures::four_page_book(D::Ltr),
        )
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    let mut app = book_composition_tests::composed_app(&fixtures::four_page_book(D::Ltr));
    app.install_book_composition_runtime(service.clone());
    // The accepted rows belong to this publication. Keep the fixture's
    // Unavailable source so qualification still goes through the held worker.
    if let crate::app::book_composition_runtime::ViewerBookComposition::Accepted {
        token,
        layout: crate::book_composition::CompositionProjectionOutcome::Composed(projection),
        ..
    } = &mut app.book_composition
    {
        *token = service.snapshot().token_for(&scope);
        Arc::make_mut(projection).token = *token;
    } else {
        panic!("expected accepted composition");
    }
    app.fullscreen_idx = Some(0);
    let ctx = egui::Context::default();
    let (entered, release) = service.hold_worker_for_test();
    entered.recv_timeout(Duration::from_secs(30)).unwrap();
    let mut action = FsKeyAction::default();
    app.queue_spread_step_action(0, 1, &mut action);
    assert_eq!(action.page_nav, FsPageNav::Target(1));
    let demand = app
        .spread_display_composition_for_anchor(1)
        .presentation_demand(app.spread_mode, app.fullscreen_page_slice);
    app.handle_fs_navigation(
        &ctx,
        false,
        false,
        None,
        None,
        None,
        action.page_nav,
        None,
        0,
    );
    assert_eq!(app.spread_step.unwrap().start_idx, 1);
    assert_eq!(
        app.fullscreen_idx,
        Some(0),
        "landing really waits for permission"
    );
    assert_eq!(app.pending_book_target_demand_for_test(), demand);
    release.send(()).unwrap();
    service
        .prepare(scope, PageDomain::Document)
        .recv_timeout(Duration::from_secs(30))
        .unwrap()
        .status
        .unwrap();
    app.poll_book_presentation_permission(&ctx);
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.spread_step.unwrap().start_idx, 1);
    app.enter_page_edit_single_view(1);
    assert!(app.spread_step.is_none());
    owner.shutdown_and_join();
}

#[test]
fn spread_step_composed_mode_only_and_flow_roundtrips_retire() {
    use crate::book_composition::{
        BookCompositionService, PageDomain, ReadingDirection as D, fixtures,
    };
    let ctx = egui::Context::default();
    for flow_change in [false, true] {
        let data = tempfile::tempdir().unwrap();
        let (service, owner) =
            BookCompositionService::spawn_at(data.path().to_path_buf(), None).unwrap();
        service
            .save(
                crate::book_scope::BookScopeKey::document("c:/g3-display.pdf").unwrap(),
                PageDomain::Document,
                fixtures::four_page_book(D::Rtl),
            )
            .recv_timeout(std::time::Duration::from_secs(30))
            .unwrap()
            .status
            .unwrap();
        let mut app = book_composition_tests::composed_app(&fixtures::four_page_book(D::Rtl));
        app.install_book_composition_runtime(service);
        app.fullscreen_idx = Some(0);
        step_target(&mut app, 1);
        assert!(app.spread_step.is_some());
        if flow_change {
            app.set_reading_flow_for_fullscreen(&ctx, 1, ReadingFlow::Horizontal);
            app.set_reading_flow_for_fullscreen(&ctx, 1, ReadingFlow::Paged);
        } else {
            app.apply_fullscreen_view_mode(&ctx, 1, FullscreenViewMode::Single);
            app.apply_fullscreen_view_mode(&ctx, 1, FullscreenViewMode::Spread);
        }
        assert!(app.spread_step.is_none());
        assert_eq!(step_pages(&mut app), vec![vec![0, 1], vec![2, 3]]);
        owner.shutdown_and_join();
    }
}

#[test]
fn spread_step_page_alone_accepted_change_retires_but_failed_change_keeps() {
    use crate::settings::{PageAlonePreference as P, PageAlonePreferences};
    let ctx = egui::Context::default();
    let mut app = step_app(7, SpreadMode::LtrCover);
    step_target(&mut app, 1);
    assert!(app.spread_step.is_some());
    let before = app.spread_step;
    app.spread_db = Err("injected unavailable store".into());
    let preferences = PageAlonePreferences {
        after_cover: P::On,
        last: P::On,
    };
    app.set_page_alone_preferences_for_fullscreen(&ctx, preferences);
    assert_eq!(app.spread_step, before);
    let temp = tempfile::tempdir().unwrap();
    app.spread_db =
        Ok(crate::spread_db::SpreadDb::open_at(&temp.path().join("spread.db")).unwrap());
    app.set_page_alone_preferences_for_fullscreen(&ctx, preferences);
    assert!(app.spread_step.is_none());
}

#[test]
fn spread_step_details_reorder_retires_only_when_reader_order_changes() {
    let mut app = step_app(6, SpreadMode::Ltr);
    app.settings.grid_view_mode = crate::settings::GridViewMode::Details;
    app.settings.details_sort_key = crate::settings::DetailsSortKey::Name;
    app.settings.details_sort_ascending = true;
    app.rebuild_details_order();
    step_target(&mut app, 1);
    app.rebuild_details_order();
    assert!(app.spread_step.is_some());
    app.settings.details_sort_ascending = false;
    app.rebuild_details_order();
    assert!(app.spread_step.is_none());
}

#[test]
fn spread_step_details_view_exit_retires_only_when_reader_order_changes() {
    let mut app = step_app(6, SpreadMode::Ltr);
    app.settings.grid_view_mode = crate::settings::GridViewMode::Details;
    app.settings.details_sort_key = crate::settings::DetailsSortKey::Name;
    app.settings.details_sort_ascending = false;
    app.rebuild_details_order();
    app.fullscreen_idx = app.details_order.first().copied();
    step_target(&mut app, 1);
    assert!(app.spread_step.is_some());
    app.set_grid_view_mode(crate::settings::GridViewMode::Thumbnail);
    assert!(app.spread_step.is_none());
}

#[test]
fn spread_step_real_key_handler_uses_reading_order_and_coalesces_a_frame() {
    #[cfg(windows)]
    let _input = crate::key_input::lock_test_input();
    #[cfg(windows)]
    crate::key_input::clear_test_frame();
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        let mut app = step_app(6, mode);
        app.keymap = crate::keymap::Keymap::from_ini_str("[FsImage]\nFsSpreadStepNext = F13\n");
        let ctx = egui::Context::default();
        let press = egui::Event::Key {
            key: egui::Key::F13,
            physical_key: None,
            pressed: true,
            repeat: false,
            modifiers: egui::Modifiers::NONE,
        };
        ctx.run_pass(
            egui::RawInput {
                focused: true,
                events: vec![press.clone(), press],
                ..Default::default()
            },
            |ctx| {
                let action = app.handle_fs_key_input(ctx, 0, false);
                assert_eq!(action.page_nav, FsPageNav::Target(1));
                assert_eq!(app.spread_step.unwrap().start_idx, 1);
                let replay = app.handle_fs_key_input(ctx, 0, false);
                assert!(replay.page_nav.is_none());
                assert_eq!(app.spread_step.unwrap().start_idx, 1);
            },
        )
        .drop_without_applying_deltas();
    }
}

#[test]
fn spread_step_same_book_seek_and_reopen_keep_the_origin() {
    let mut app = step_app(7, SpreadMode::Ltr);
    let ctx = egui::Context::default();
    assert_eq!(step_target(&mut app, 1), Some(1));
    app.handle_fs_navigation(
        &ctx,
        false,
        false,
        None,
        None,
        None,
        FsPageNav::None,
        Some(4),
        1,
    );
    assert_eq!(app.spread_step.unwrap().start_idx, 1);
    app.open_fullscreen(2, crate::app::HistoryTrigger::UserChosen);
    assert_eq!(app.spread_step.unwrap().start_idx, 1);
    assert_eq!(
        step_pages(&mut app),
        vec![vec![0], vec![1, 2], vec![3, 4], vec![5, 6]]
    );
}

#[test]
fn spread_step_slideshow_final_demand_derives_still_units_without_replacing_visible_proof() {
    let mut app = step_app(6, SpreadMode::Ltr);
    app.items[3] = GridItem::Video(PathBuf::from("c:/book/movie.mp4"));
    assert_eq!(step_target(&mut app, 1), Some(1));
    app.settings.page_turn_animation_enabled = true;
    let nav = app.get_nav_indices();
    let visible = app.build_spread_display_units_for_nav(&nav);
    app.prepare_final_demand_topologies();
    assert!(Arc::ptr_eq(&visible, &app.spread_display_units_cache.units));
    app.slideshow_playing = true;
    let Some(NextFinalUnitDemand::Unit(pages)) = app.next_final_unit_demand_from_snapshot(1) else {
        panic!()
    };
    assert_eq!(pages.iter().map(|p| p.idx).collect::<Vec<_>>(), vec![4, 5]);
    assert_eq!(app.spread_step.unwrap().start_idx, 1);
    // The same state is absent only from the still derivation if it names media.
    app.spread_step = Some(crate::app::spread_shift_owner::SpreadStep { start_idx: 3 });
    let visible = app.build_spread_display_units_for_nav(&nav);
    app.prepare_final_demand_topologies();
    assert!(Arc::ptr_eq(&visible, &app.spread_display_units_cache.units));
    let Some(NextFinalUnitDemand::Unit(pages)) = app.next_final_unit_demand_from_snapshot(2) else {
        panic!()
    };
    assert_eq!(pages.iter().map(|p| p.idx).collect::<Vec<_>>(), vec![5]);
    assert_eq!(app.spread_step.unwrap().start_idx, 3);
}

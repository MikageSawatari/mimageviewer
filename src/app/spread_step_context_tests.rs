#[cfg(windows)]
fn seed_step_context(app: &mut App, start: usize) {
    app.current_folder = Some(PathBuf::from("c:/step-book"));
    app.items = (0..6)
        .map(|idx| GridItem::Image(PathBuf::from(format!("c:/step-book/{idx}.png"))))
        .collect();
    app.visible_indices = (0..6).collect();
    app.thumbnails = vec![crate::grid_item::ThumbnailState::Pending; 6];
    app.image_metas = vec![None; 6];
    app.details_order = app.visible_indices.clone();
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    app.reading_flow = crate::settings::ReadingFlow::Paged;
    app.fullscreen_idx = Some(start);
    app.spread_step = Some(super::super::spread_shift_owner::SpreadStep { start_idx: start });
    app.viewer_navigation_caches.invalidate();
}

#[cfg(windows)]
#[test]
fn spread_step_context_materialized_fork_omits_step_and_effective_projection() {
    let mut app = crate::app::setup_app_for_test();
    seed_step_context(&mut app, 1);
    let source = app.spread_display_composition_for_anchor(1);
    let generation = app.items_generation;
    let mut fork = app.split_materialized_physical_context_for_detached_scope(
        Path::new("c:/step-book"),
        1,
        generation,
    );
    assert!(fork.spread_step.is_none());
    app.swap_viewer_context_bundle(&mut fork);
    assert_eq!(
        app.spread_display_composition_for_anchor(1)
            .navigation_pages(),
        &[0, 1]
    );
    app.swap_viewer_context_bundle(&mut fork);
    assert_eq!(app.spread_step.unwrap().start_idx, 1);
    assert_eq!(app.spread_display_composition_for_anchor(1), source);
}

#[cfg(windows)]
#[test]
fn spread_step_context_parked_page_alone_change_retires_only_inheriting_context() {
    use crate::settings::{PageAlonePreference as P, SpreadMode};
    let mut app = crate::app::setup_app_for_test();
    seed_step_context(&mut app, 1);
    let inherited = app.build_window_context_for_test(1797, |owner| {
        seed_step_context(owner, 1);
        owner.spread_mode = SpreadMode::LtrCover;
    });
    let explicit = app.build_window_context_for_test(1798, |owner| {
        seed_step_context(owner, 3);
        owner.spread_mode = SpreadMode::LtrCover;
        owner.page_alone_preferences.after_cover = P::On;
    });
    app.invalidate_page_alone_in_parked_contexts(true, false);
    app.with_viewer_context(inherited, |owner| assert!(owner.spread_step.is_none()))
        .unwrap();
    app.with_viewer_context(explicit, |owner| {
        assert_eq!(owner.spread_step.unwrap().start_idx, 3)
    })
    .unwrap();
    assert_eq!(app.spread_step.unwrap().start_idx, 1);
}

#[cfg(windows)]
#[test]
fn spread_step_context_collection_root_refused_but_opened_book_accepted_on_both_surfaces() {
    use super::super::top_level_grid_view::{
        CollectionGridIdentity, CollectionGridPosition, TopLevelGridSurface,
    };
    for detached in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let run = |owner: &mut App| {
            seed_step_context(owner, 0);
            owner.spread_step = None;
            owner
                .top_level_grid_view
                .replace_surface(TopLevelGridSurface::Collection(CollectionGridIdentity {
                    collection_id: crate::collection_store::CollectionId::new(),
                }));
            let mut action = crate::ui_fullscreen::FsKeyAction::default();
            owner.queue_spread_step_action(0, 1, &mut action);
            assert!(action.page_nav.is_none());
            assert!(owner.spread_step.is_none());
            // Fallback precedes collection admission.
            owner.spread_mode = crate::settings::SpreadMode::Single;
            owner.queue_spread_step_action(0, 1, &mut action);
            assert_eq!(action.page_nav, crate::ui_fullscreen::FsPageNav::Delta(1));
            owner
                .top_level_grid_view
                .collection_session_mut()
                .unwrap()
                .position = CollectionGridPosition::PhysicalSource {
                entry_id: crate::collection_store::CollectionEntryId::new(),
                source_key: crate::collection_store::CollectionSourcePath::from_trusted(
                    "c:/step-book",
                )
                .unwrap()
                .key()
                .clone(),
                path: PathBuf::from("c:/step-book"),
            };
            owner.spread_mode = crate::settings::SpreadMode::Ltr;
            owner.queue_spread_step_action(0, 1, &mut crate::ui_fullscreen::FsKeyAction::default());
            assert_eq!(owner.spread_step.unwrap().start_idx, 1);
        };
        if detached {
            let id = app.build_window_context_for_test(1799, |_| {});
            app.with_viewer_context(id, run).unwrap();
        } else {
            run(&mut app);
        }
    }
}

#[cfg(windows)]
#[test]
fn spread_step_context_edit_and_close_retire_without_touching_sibling() {
    let mut app = crate::app::setup_app_for_test();
    seed_step_context(&mut app, 1);
    let sibling = app.build_window_context_for_test(1800, |owner| seed_step_context(owner, 3));
    app.enter_page_edit_single_view(1);
    assert!(app.spread_step.is_none());
    app.leave_page_edit_single_view(Some(super::super::PageEditSpreadPivot {
        saved_mode: crate::settings::SpreadMode::Ltr,
        pair: (1, 2),
        navigation_anchor_idx: 1,
    }));
    assert_eq!(app.spread_mode, crate::settings::SpreadMode::Ltr);
    assert!(app.spread_step.is_none());
    seed_step_context(&mut app, 1);
    app.close_fullscreen();
    assert!(app.spread_step.is_none());
    app.with_viewer_context(sibling, |owner| {
        assert_eq!(owner.spread_step.unwrap().start_idx, 3)
    })
    .unwrap();
}

#[cfg(windows)]
#[test]
fn spread_step_context_same_generation_book_and_pdf_rule_adoption_retires_after_success() {
    let mut app = crate::app::setup_app_for_test();
    seed_step_context(&mut app, 1);
    let temp = tempfile::tempdir().unwrap();
    let defaults = super::super::SpreadRestoreDefaults::for_book(&app.settings);
    app.spread_db = Err("injected unavailable store".into());
    assert!(
        app.apply_spread_for_key_with_fallback(Path::new("c:/step-book"), None, defaults)
            .is_err()
    );
    assert!(app.spread_step.is_some());
    app.spread_db =
        Ok(crate::spread_db::SpreadDb::open_at(&temp.path().join("spread.db")).unwrap());
    app.apply_spread_for_key_with_fallback(Path::new("c:/step-book"), None, defaults)
        .unwrap();
    assert!(app.spread_step.is_none());
    app.spread_step = Some(super::super::spread_shift_owner::SpreadStep { start_idx: 1 });
    app.apply_spread_for_key_with_document_direction(
        Path::new("c:/step-book"),
        None,
        defaults,
        Some(crate::pdf_loader::PdfReadingDirection::R2L),
    )
    .unwrap();
    assert!(app.spread_step.is_none());
}

#[cfg(windows)]
#[test]
fn spread_step_context_f12_real_presentation_switch_keeps_origin() {
    let mut app = crate::app::setup_app_for_test();
    seed_step_context(&mut app, 1);
    app.viewer_presentation = super::super::ViewerPresentation::Fullscreen;
    app.settings.detached_viewer_enabled = false;
    app.toggle_detached_viewer_mode();
    assert!(app.settings.detached_viewer_enabled);
    assert_eq!(
        app.viewer_presentation,
        super::super::ViewerPresentation::DetachedWindow
    );
    assert_eq!(app.spread_step.unwrap().start_idx, 1);
    app.toggle_detached_viewer_mode();
    assert!(!app.settings.detached_viewer_enabled);
    assert_eq!(app.spread_step.unwrap().start_idx, 1);
}

#[cfg(windows)]
#[test]
fn spread_step_context_imported_container_rules_retire_after_current_generation_adoption_only() {
    use super::super::metadata_import_refresh::{ContainerStateResult, ContextResult};
    let mut app = crate::app::setup_app_for_test();
    seed_step_context(&mut app, 1);
    let generation = app.items_generation;
    let context_id = app.projected_viewer_context_id();
    let result = |items_generation| ContextResult {
        context_id,
        items_generation,
        rating_cache: None,
        tags_cache: None,
        current_rating: None,
        page_state: None,
        folder_pin_map: None,
        folder_pin_materializations: None,
        video_pin_blobs: None,
        video_items: None,
        shift_read: None,
        container_state: Some(ContainerStateResult {
            spread: crate::spread_db::StoredSpreadState {
                mode: Some(crate::settings::SpreadMode::Ltr),
                flow: Some(crate::settings::ReadingFlow::Paged),
                direction: Some(crate::settings::ReadingDirection::Ltr),
                ..Default::default()
            },
            final_cover_spread_preference: Default::default(),
            singleton_spread_endpoint_preferences: Default::default(),
            page_alone_preferences: crate::settings::PageAlonePreferences {
                after_cover: crate::settings::PageAlonePreference::On,
                ..Default::default()
            },
            view_trim: crate::view_trim_db::ViewTrimBookLoad::Missing,
        }),
    };
    let changed = crate::metadata_transfer::ImportChangedSections {
        container_state: true,
        ..Default::default()
    };
    assert!(!app.apply_current_metadata_import_terminal_result(result(generation + 1), changed));
    assert!(app.spread_step.is_some());
    assert!(app.apply_current_metadata_import_terminal_result(result(generation), changed));
    assert!(app.spread_step.is_none());
}

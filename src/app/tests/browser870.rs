use super::*;
use crate::settings::FinalCoverSpreadPreference as FinalCover;
use crate::settings::{ReadingDirection as Direction, ReadingFlow as Flow, SpreadMode as Mode};
use crate::ui_fullscreen::SpreadPresentationPersistence as Persistence;

fn reading(app: &App) -> (Mode, Flow, Direction) {
    (app.spread_mode, app.reading_flow, app.reading_direction)
}

fn stored(app: &App, folder: &Path) -> crate::spread_db::StoredSpreadState {
    app.spread_db
        .as_ref()
        .unwrap()
        .get_state_with_fallback(folder, None)
}

fn save_fixture(app: &App, folder: &Path, mode: Mode) {
    app.spread_db
        .as_ref()
        .unwrap()
        .set_presentation_state(
            folder,
            mode,
            Flow::Vertical,
            Direction::Rtl,
            (Mode::Single, Flow::Paged, Direction::Ltr),
        )
        .unwrap();
}

fn adopt(app: &mut App, folder: &Path, directory: bool) {
    app.start_loading_items(
        folder.to_path_buf(),
        vec![
            GridItem::Image(folder.join("001.jpg")),
            GridItem::Image(folder.join("002.jpg")),
        ],
        vec![None; 2],
        Default::default(),
        Vec::new(),
        directory.then(|| folder_scan::FolderScanSignature::for_test(870)),
    );
    app.finish_page_identity_preparation_for_test();
}

#[test]
fn browser870_load_matrix_ignores_only_directory_presentation_and_preserves_rows() {
    let mut app = phase_c_support::setup_app();
    let root = app.tmp.path().join("browser");
    std::fs::create_dir_all(&root).unwrap();
    app.settings.browser_view_output_dir = Some(root.clone());
    for inside in [false, true] {
        for directory in [false, true] {
            for enabled in [false, true] {
                for book in [false, true] {
                    for saved in [None, Some(Mode::Single), Some(Mode::LtrCover)] {
                        plain_folder_spread::configure(&mut app, enabled, book);
                        let parent = if inside {
                            root.clone()
                        } else {
                            app.tmp.path().join("outside")
                        };
                        let folder = plain_folder_spread::make_folder(
                            &parent,
                            &format!("{directory}-{enabled}-{book}-{saved:?}"),
                        );
                        if let Some(mode) = saved {
                            save_fixture(&app, &folder, mode);
                        }
                        let before = stored(&app, &folder);
                        adopt(&mut app, &folder, directory);
                        if inside && directory {
                            assert_eq!(reading(&app), (Mode::Single, Flow::Paged, Direction::Ltr));
                        } else if let Some(mode) = saved {
                            assert_eq!(
                                reading(&app),
                                (
                                    mode,
                                    Flow::Vertical,
                                    mode.reading_direction().unwrap_or(Direction::Rtl)
                                )
                            );
                        } else {
                            let defaults = SpreadRestoreDefaults::for_physical_folder(
                                book,
                                directory,
                                enabled,
                                Mode::RtlCover,
                                Flow::Horizontal,
                                Direction::Ltr,
                            );
                            assert_eq!(
                                reading(&app),
                                (
                                    defaults.spread_mode,
                                    defaults.reading_flow,
                                    defaults
                                        .spread_mode
                                        .reading_direction()
                                        .unwrap_or(defaults.reading_direction),
                                )
                            );
                        }
                        assert_eq!(stored(&app, &folder), before);
                    }
                }
            }
        }
    }
}

#[test]
fn browser870_directory_books_and_three_favorite_guards_do_not_remember() {
    use book_materialization::{BookScope, ProjectionRecipe};
    for kind in 0..3 {
        let mut app = phase_c_support::setup_app();
        plain_folder_spread::configure(&mut app, true, kind == 0);
        let root = app.tmp.path().join("browser");
        let folder =
            plain_folder_spread::make_folder(&root, if kind == 2 { "month.zip" } else { "month" });
        app.settings.browser_view_output_dir = Some(root.clone());
        if kind == 1 {
            app.settings.book_root = Some(root);
        }
        save_fixture(&app, &folder, Mode::LtrCover);
        if kind == 2 {
            assert!(crate::folder_tree::is_open_as_container(&folder));
            adopt(&mut app, &folder, true);
        } else {
            app.load_folder(folder.clone());
            app.finish_page_identity_preparation_for_test();
        }
        assert_eq!(app.spread_mode, Mode::Single);
        if kind == 0 {
            assert_eq!(
                app.projection_recipe,
                ProjectionRecipe::Book(BookScope::ImageFolder(folder.clone()))
            );
        }
        if kind == 1 {
            assert_eq!(
                app.projection_recipe,
                ProjectionRecipe::Book(BookScope::CompiledFolder(folder.clone()))
            );
        }
        assert!(!app.composition_owns_view_settings());
        app.settings.remember_favorite_view_state = true;
        let favorite = crate::settings::FavoriteEntry::new("browser".into(), folder.clone());
        let favorite_view_state = crate::settings::FavoriteViewState::from_settings(&app.settings);
        app.favorite_view_states
            .insert(favorite.id, favorite_view_state);
        app.settings.favorites.push(favorite);
        app.transition_favorite_view_for_path(Some(&folder));
        assert!(app.settings.active_favorite_view_id().is_some());
        let defaults = (
            app.settings.default_spread_mode,
            app.settings.default_reading_flow,
        );
        let before = stored(&app, &folder);
        let ctx = egui::Context::default();
        assert!(app.apply_fullscreen_spread_mode(&ctx, 0, Mode::Ltr));
        assert_eq!(app.spread_mode, Mode::Ltr);
        app.set_reading_flow_for_fullscreen(&ctx, 0, Flow::Vertical);
        app.set_reading_direction_for_fullscreen(&ctx, 0, Direction::Rtl);
        assert_eq!(reading(&app), (Mode::Rtl, Flow::Vertical, Direction::Rtl));
        assert_eq!(
            (
                app.settings.default_spread_mode,
                app.settings.default_reading_flow
            ),
            defaults
        );
        assert_eq!(stored(&app, &folder), before);
        assert!(app.apply_fullscreen_spread_mode(&ctx, 0, Mode::Single));
        assert_eq!(app.spread_mode, Mode::Single);
        assert_eq!(stored(&app, &folder), before);
        assert_eq!(
            app.persist_spread_presentation(Mode::Single, Flow::Paged, Direction::Ltr)
                .unwrap(),
            Persistence::NotRemembered
        );
        assert_eq!(stored(&app, &folder), before);
        adopt(&mut app, &folder, true);
        assert_eq!(reading(&app), (Mode::Single, Flow::Paged, Direction::Ltr));
    }
}

#[test]
fn browser870_live_root_switch_controls_saving_before_reload_and_retains_old_rows() {
    let mut app = phase_c_support::setup_app();
    plain_folder_spread::configure(&mut app, true, false);
    let a = plain_folder_spread::make_folder(app.tmp.path(), "a");
    let b = plain_folder_spread::make_folder(app.tmp.path(), "b");
    save_fixture(&app, &a, Mode::LtrCover);
    app.settings.browser_view_output_dir = Some(b.clone());
    app.load_folder(a.clone());
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.spread_mode, Mode::LtrCover);
    let before = stored(&app, &a);
    app.settings.browser_view_output_dir = Some(a.clone());
    assert_eq!(
        app.persist_spread_presentation(Mode::Ltr, Flow::Paged, Direction::Ltr)
            .unwrap(),
        Persistence::NotRemembered
    );
    assert_eq!(stored(&app, &a), before);
    app.load_folder(a.clone());
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.spread_mode, Mode::Single);
    app.settings.browser_view_output_dir = Some(b);
    assert_eq!(
        app.persist_spread_presentation(Mode::LtrCover, Flow::Vertical, Direction::Rtl)
            .unwrap(),
        Persistence::Saved
    );
    assert_eq!(stored(&app, &a), before);
    app.load_folder(a.clone());
    app.finish_page_identity_preparation_for_test();
    assert_eq!(
        app.spread_mode,
        Mode::LtrCover,
        "the old row becomes effective again"
    );
    assert_eq!(
        app.persist_spread_presentation(Mode::Ltr, Flow::Paged, Direction::Ltr)
            .unwrap(),
        Persistence::Saved
    );
    app.load_folder(a.clone());
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.spread_mode, Mode::Ltr);
    assert_ne!(stored(&app, &a), before);
}

#[test]
fn browser870_non_directory_and_replacement_do_not_inherit_directory_provenance() {
    let mut app = phase_c_support::setup_app();
    let root = app.tmp.path().join("browser");
    let folder = plain_folder_spread::make_folder(&root, "month");
    app.settings.browser_view_output_dir = Some(root.clone());
    app.load_folder(folder.clone());
    app.finish_page_identity_preparation_for_test();
    assert!(!app.remembers_spread_presentation());
    for name in ["book.zip", "document.pdf"] {
        let container = root.join(name);
        adopt(&mut app, &container, false);
        assert!(app.remembers_spread_presentation());
        assert_eq!(
            app.persist_spread_presentation(Mode::Ltr, Flow::Paged, Direction::Ltr)
                .unwrap(),
            Persistence::Saved
        );
        assert_eq!(stored(&app, &container).mode, Some(Mode::Ltr));
    }
    app.load_folder(folder.clone());
    app.finish_page_identity_preparation_for_test();
    app.install_new_items(vec![GridItem::Image(folder.join("result.jpg"))], vec![None]);
    assert_eq!(
        app.spread_presentation_source,
        SpreadPresentationSource::Other
    );
    app.spread_presentation_source = SpreadPresentationSource::PhysicalDirectory {
        key: folder.clone(),
    };
    app.current_folder = Some(root.join("different"));
    assert!(
        app.remembers_spread_presentation(),
        "a stale source key must never govern a different listing"
    );
}

#[cfg(windows)]
#[test]
fn browser870_metadata_completion_preserves_live_presentation_and_imports_other_values() {
    use crate::settings::{
        FinalCoverSpreadPreference, PageAlonePreference, PageAlonePreferences,
        SingletonSpreadEndpointPreferences, SingletonSpreadPlacementPreference,
    };
    for enabled in [false, true] {
        let mut app = phase_c_support::setup_app();
        plain_folder_spread::configure(&mut app, enabled, false);
        let folder = plain_folder_spread::make_folder(app.tmp.path(), "browser");
        app.settings.browser_view_output_dir = Some(folder.clone());
        app.load_folder(folder.clone());
        app.finish_page_identity_preparation_for_test();
        let ctx = egui::Context::default();
        app.apply_fullscreen_spread_mode(&ctx, 0, Mode::Ltr);
        let expected = reading(&app);
        let final_cover = FinalCoverSpreadPreference::On;
        let endpoints = SingletonSpreadEndpointPreferences {
            first: SingletonSpreadPlacementPreference::Place,
            last: SingletonSpreadPlacementPreference::Place,
        };
        let alone = PageAlonePreferences {
            after_cover: PageAlonePreference::On,
            last: PageAlonePreference::On,
        };
        let result = metadata_import_refresh::ContextResult {
            container_state: Some(metadata_import_refresh::ContainerStateResult {
                spread: crate::spread_db::StoredSpreadState {
                    mode: Some(Mode::RtlCover),
                    flow: Some(Flow::Vertical),
                    direction: Some(Direction::Rtl),
                    ..Default::default()
                },
                final_cover_spread_preference: final_cover,
                singleton_spread_endpoint_preferences: endpoints,
                page_alone_preferences: alone,
                view_trim: crate::view_trim_db::ViewTrimBookLoad::Stored(
                    crate::view_trim::ViewTrimBookState {
                        preference: crate::view_trim::ViewTrimBookPreference::Manual,
                        ..Default::default()
                    },
                    crate::view_trim::ViewTrimBookProvenance::Current,
                ),
            }),
            rating_cache: None,
            ..metadata_context_result(app.viewer_context_main(), app.items_generation, 0)
        };
        assert!(app.apply_current_metadata_import_terminal_result(
            result,
            crate::metadata_transfer::ImportChangedSections {
                container_state: true,
                ..Default::default()
            }
        ));
        assert_eq!(reading(&app), expected);
        assert_eq!(app.final_cover_spread_preference, final_cover);
        assert_eq!(app.singleton_spread_endpoint_preferences, endpoints);
        assert_eq!(app.page_alone_preferences, alone);
        assert_eq!(
            app.view_trim_book_preference,
            crate::view_trim::ViewTrimBookPreference::Manual
        );
    }
}

#[test]
fn browser870_composition_owner_and_extracted_policy_keep_existing_contracts() {
    use book_materialization::{BookScope, ExtractedSurface, ProjectionRecipe};
    let mut app = phase_c_support::setup_app();
    let root = app.tmp.path().join("browser");
    let document = root.join("book.pdf");
    app.settings.browser_view_output_dir = Some(root);
    app.current_folder = Some(document.clone());
    app.projection_recipe = ProjectionRecipe::Book(BookScope::Document(document.clone()));
    // Even if the same path has a directory fact, an actual composition owner wins.
    app.spread_presentation_source = SpreadPresentationSource::PhysicalDirectory {
        key: document.clone(),
    };
    app.book_composition = book_composition_runtime::ViewerBookComposition::Blocked {
        scope: crate::book_scope::BookScopeKey::document(&document).unwrap(),
        token: crate::book_composition::CompositionToken {
            service_epoch: 1,
            publication_sequence: 1,
        },
        error: "unreadable fixture".into(),
    };
    assert!(app.composition_owns_view_settings());
    assert!(app.remembers_spread_presentation());
    assert_eq!(
        app.persist_spread_presentation(Mode::Ltr, Flow::Paged, Direction::Ltr)
            .unwrap(),
        Persistence::Saved
    );
    let before = stored(&app, &document);
    app.projection_recipe = ProjectionRecipe::ExtractedResult(ExtractedSurface::Rating);
    assert!(!app.remembers_spread_presentation());
    assert_eq!(
        app.persist_spread_presentation(Mode::Single, Flow::Paged, Direction::Ltr)
            .unwrap(),
        Persistence::NotRemembered
    );
    assert_eq!(stored(&app, &document), before);
}

#[test]
fn browser870_load_preserves_non_presentation_preferences() {
    let mut app = phase_c_support::setup_app();
    let folder = plain_folder_spread::make_folder(app.tmp.path(), "browser");
    app.settings.browser_view_output_dir = Some(folder.clone());
    let final_cover = crate::settings::FinalCoverSpreadPreference::On;
    let endpoints = crate::settings::SingletonSpreadEndpointPreferences {
        first: crate::settings::SingletonSpreadPlacementPreference::Place,
        last: crate::settings::SingletonSpreadPlacementPreference::Center,
    };
    let alone = crate::settings::PageAlonePreferences {
        after_cover: crate::settings::PageAlonePreference::On,
        last: crate::settings::PageAlonePreference::Off,
    };
    let db = app.spread_db.as_ref().unwrap();
    db.set_final_cover_spread_preference(&folder, None, final_cover)
        .unwrap();
    db.set_singleton_spread_endpoint_preferences(&folder, None, endpoints)
        .unwrap();
    db.set_page_alone_preferences(&folder, None, alone).unwrap();
    save_fixture(&app, &folder, Mode::RtlCover);
    app.load_folder(folder);
    app.finish_page_identity_preparation_for_test();
    assert_eq!(reading(&app), (Mode::Single, Flow::Paged, Direction::Ltr));
    assert_eq!(app.final_cover_spread_preference, final_cover);
    assert_eq!(app.singleton_spread_endpoint_preferences, endpoints);
    assert_eq!(app.page_alone_preferences, alone);
}

fn has_final_cover_supplement(app: &mut App) -> bool {
    let last = app.items.len() - 1;
    app.spread_display_composition_for_anchor(last)
        .pages_in_reading_order()
        .iter()
        .any(|page| page.role == crate::ui_fullscreen::SpreadPageRole::FrontCoverSupplement)
}

fn adopt_final_cover_container(app: &mut App, path: &Path, pdf: bool) {
    let pages = (0..2)
        .map(|index| {
            if pdf {
                GridItem::PdfPage {
                    pdf_path: path.to_path_buf(),
                    page_num: index,
                    content_type: None,
                }
            } else {
                GridItem::ZipImage {
                    zip_path: path.to_path_buf(),
                    entry_name: format!("{index:03}.jpg"),
                }
            }
        })
        .collect();
    app.start_loading_items(
        path.to_path_buf(),
        pages,
        vec![None; 2],
        Default::default(),
        Vec::new(),
        None,
    );
    app.finish_page_identity_preparation_for_test();
    for index in 0..2 {
        app.record_page_dims_for_spread(index, (8, 12));
    }
}

#[test]
fn browser870_final_cover_defaults_overrides_and_rows_survive_open_close() {
    let mut app = phase_c_support::setup_app();
    let root = app.tmp.path().join("browser");
    let inside = plain_folder_spread::make_folder(&root, "month");
    let outside = plain_folder_spread::make_folder(app.tmp.path(), "outside");
    app.settings.browser_view_output_dir = Some(root);
    for global in [false, true] {
        app.settings.final_cover_spread_enabled = global;
        for folder in [&inside, &outside] {
            for preference in [FinalCover::FollowGlobal, FinalCover::On, FinalCover::Off] {
                save_fixture(&app, folder, Mode::RtlCover);
                app.spread_db
                    .as_ref()
                    .unwrap()
                    .set_final_cover_spread_preference(folder, None, preference)
                    .unwrap();
                let before = stored(&app, folder);
                adopt(&mut app, folder, true);
                app.spread_mode = Mode::LtrCover;
                let expected = preference.effective(global && folder == &outside);
                assert_eq!(app.final_cover_spread_preference, preference);
                assert_eq!(app.final_cover_spread_enabled_for_current_book(), expected);
                assert_eq!(has_final_cover_supplement(&mut app), expected);
                // Replacing the listing closes this book without modifying its saved values.
                let parent = app.tmp.path().to_path_buf();
                adopt(&mut app, &parent, true);
                assert_eq!(app.settings.final_cover_spread_enabled, global);
                assert_eq!(stored(&app, folder), before);
                assert_eq!(
                    app.spread_db
                        .as_ref()
                        .unwrap()
                        .get_final_cover_spread_preference_with_fallback(folder, None),
                    preference
                );
            }
        }
    }
}

#[test]
fn browser870_final_cover_zip_composition_and_extracted_keep_existing_policy() {
    use book_materialization::{BookScope, ExtractedSurface, ProjectionRecipe};
    let mut app = phase_c_support::setup_app();
    let root = app.tmp.path().join("browser");
    let zip = root.join("book.zip");
    app.settings.browser_view_output_dir = Some(root.clone());
    adopt_final_cover_container(&mut app, &zip, false);
    for global in [false, true] {
        app.settings.final_cover_spread_enabled = global;
        assert!(!app.current_view_is_browser_directory());
        assert_eq!(app.final_cover_spread_enabled_for_current_book(), global);
    }
    let document = root.join("book.pdf");
    app.current_folder = Some(document.clone());
    app.projection_recipe = ProjectionRecipe::Book(BookScope::Document(document.clone()));
    app.spread_presentation_source = SpreadPresentationSource::PhysicalDirectory {
        key: document.clone(),
    };
    app.book_composition = book_composition_runtime::ViewerBookComposition::Blocked {
        scope: crate::book_scope::BookScopeKey::document(&document).unwrap(),
        token: crate::book_composition::CompositionToken {
            service_epoch: 1,
            publication_sequence: 1,
        },
        error: "unreadable fixture".into(),
    };
    for global in [false, true] {
        app.settings.final_cover_spread_enabled = global;
        assert!(app.composition_owns_view_settings());
        assert!(!app.current_view_is_browser_directory());
        assert!(app.remembers_spread_presentation());
        assert_eq!(app.final_cover_spread_enabled_for_current_book(), global);
    }
    app.projection_recipe = ProjectionRecipe::ExtractedResult(ExtractedSurface::Rating);
    app.final_cover_spread_preference = FinalCover::On;
    assert!(!app.final_cover_spread_enabled_for_current_book());
}

#[cfg(windows)]
#[test]
fn browser870_final_cover_context_swap_preserves_each_effective_value() {
    let mut app = phase_c_support::setup_app();
    let root = app.tmp.path().join("browser");
    let month = plain_folder_spread::make_folder(&root, "month");
    let outside = plain_folder_spread::make_folder(app.tmp.path(), "outside");
    app.settings.browser_view_output_dir = Some(root);
    app.settings.final_cover_spread_enabled = true;
    adopt(&mut app, &month, true);
    let window = app.allocate_detached_viewer_window_id();
    let sibling = app.build_window_context_for_test(window, |viewer| {
        adopt(viewer, &outside, true);
    });
    for _ in 0..2 {
        assert!(!app.final_cover_spread_enabled_for_current_book());
        app.with_viewer_context(sibling, |viewer| {
            assert!(viewer.final_cover_spread_enabled_for_current_book());
        })
        .unwrap();
    }
}

#[cfg(windows)]
fn seed_final_cover_display(app: &mut App) {
    app.fs_pan = egui::vec2(12.0, 34.0);
    app.fs_vertical_scroll = 123.0;
    app.fs_free_rotation = 27.0;
    app.fullscreen_page_layout
        .begin(crate::displayed_image_transform::FullscreenPageLayoutKind::Continuous);
}

#[cfg(windows)]
fn assert_final_cover_display(app: &App, invalidated: bool) {
    use crate::displayed_image_transform::FullscreenPageLayoutKind as Layout;
    assert_eq!(
        app.fs_pan,
        if invalidated {
            egui::Vec2::ZERO
        } else {
            egui::vec2(12.0, 34.0)
        }
    );
    assert_eq!(
        app.fs_vertical_scroll,
        if invalidated { 0.0 } else { 123.0 }
    );
    assert_eq!(app.fs_free_rotation, if invalidated { 0.0 } else { 27.0 });
    assert_eq!(
        app.fullscreen_page_layout.kind(),
        if invalidated {
            Layout::Empty
        } else {
            Layout::Continuous
        }
    );
}

#[cfg(windows)]
#[test]
fn browser870_final_cover_root_change_invalidates_only_affected_mounted_and_parked_contexts() {
    let mut app = phase_c_support::setup_app();
    let old_root = app.tmp.path().join("browser");
    let new_root = app.tmp.path().join("new-browser");
    let month = plain_folder_spread::make_folder(&old_root, "month");
    let new_month = plain_folder_spread::make_folder(&new_root, "month");
    let outside = plain_folder_spread::make_folder(app.tmp.path(), "outside");
    app.settings.final_cover_spread_enabled = true;
    app.settings.browser_view_output_dir = Some(old_root.clone());
    adopt(&mut app, &month, true);
    app.spread_mode = Mode::LtrCover;
    let mut contexts = Vec::new();
    for (path, directory, affected) in [
        (month.clone(), true, true),
        (new_month, true, true),
        (outside, true, false),
        (old_root.join("book.zip"), false, false),
        (old_root.join("book.pdf"), false, false),
    ] {
        let window = app.allocate_detached_viewer_window_id();
        let id = app.build_window_context_for_test(window, |viewer| {
            if directory {
                adopt(viewer, &path, true);
            } else {
                adopt_final_cover_container(
                    viewer,
                    &path,
                    path.extension().is_some_and(|extension| extension == "pdf"),
                );
            }
            viewer.spread_mode = Mode::LtrCover;
            seed_final_cover_display(viewer);
        });
        contexts.push((id, path, directory, affected));
    }
    let generation = app.items_generation;
    for (old, new, browser_enabled) in [(&old_root, &new_root, true), (&new_root, &old_root, false)]
    {
        seed_final_cover_display(&mut app);
        app.settings.browser_view_output_dir = Some(new.clone());
        app.invalidate_final_cover_after_shared_input_change(
            &egui::Context::default(),
            FinalCoverSharedInputChange::BrowserRoot {
                old: Some(old),
                new: Some(new),
            },
        );
        assert_eq!(app.items_generation, generation, "no listing reload");
        assert_eq!(
            app.final_cover_spread_enabled_for_current_book(),
            browser_enabled
        );
        assert_final_cover_display(&app, true);
        assert_eq!(has_final_cover_supplement(&mut app), browser_enabled);
        for (id, path, directory, affected) in &contexts {
            app.with_viewer_context(*id, |viewer| {
                assert_final_cover_display(viewer, *affected);
                let expected = !crate::browser_view_policy::is_browser_view_directory(
                    path,
                    *directory,
                    Some(new),
                );
                assert_eq!(
                    viewer.final_cover_spread_enabled_for_current_book(),
                    expected
                );
                assert_eq!(has_final_cover_supplement(viewer), expected);
                seed_final_cover_display(viewer);
            })
            .unwrap();
        }
    }
    // The same selector also leaves an unrelated *mounted* directory/PDF/ZIP untouched.
    for (id, _, _, affected) in contexts {
        if !affected {
            app.with_viewer_context(id, |viewer| {
                viewer.settings.browser_view_output_dir = Some(new_root.clone());
                viewer.invalidate_final_cover_after_shared_input_change(
                    &egui::Context::default(),
                    FinalCoverSharedInputChange::BrowserRoot {
                        old: Some(&old_root),
                        new: Some(&new_root),
                    },
                );
                assert_final_cover_display(viewer, false);
            })
            .unwrap();
        }
    }
}

#[cfg(windows)]
#[test]
fn browser870_final_cover_shared_changes_preserve_explicit_preferences() {
    let mut app = phase_c_support::setup_app();
    let old = app.tmp.path().join("browser");
    let new = app.tmp.path().join("elsewhere");
    let month = plain_folder_spread::make_folder(&old, "month");
    app.settings.browser_view_output_dir = Some(old.clone());
    adopt(&mut app, &month, true);
    let mut contexts = Vec::new();
    for preference in [FinalCover::FollowGlobal, FinalCover::On, FinalCover::Off] {
        let window = app.allocate_detached_viewer_window_id();
        let id = app.build_window_context_for_test(window, |viewer| {
            adopt(viewer, &month, true);
            viewer.final_cover_spread_preference = preference;
            seed_final_cover_display(viewer);
        });
        contexts.push((id, preference));
    }
    for root_change in [false, true] {
        app.settings.final_cover_spread_enabled = !app.settings.final_cover_spread_enabled;
        app.settings.browser_view_output_dir = Some(new.clone());
        for preference in [FinalCover::FollowGlobal, FinalCover::On, FinalCover::Off] {
            app.final_cover_spread_preference = preference;
            seed_final_cover_display(&mut app);
            app.invalidate_final_cover_after_shared_input_change(
                &egui::Context::default(),
                if root_change {
                    FinalCoverSharedInputChange::BrowserRoot {
                        old: Some(&old),
                        new: Some(&new),
                    }
                } else {
                    FinalCoverSharedInputChange::GlobalDefault
                },
            );
            assert_final_cover_display(&app, preference == FinalCover::FollowGlobal);
        }
        for (id, preference) in &contexts {
            app.with_viewer_context(*id, |viewer| {
                assert_final_cover_display(viewer, *preference == FinalCover::FollowGlobal);
                assert_eq!(viewer.final_cover_spread_preference, *preference);
                seed_final_cover_display(viewer);
            })
            .unwrap();
        }
    }
}

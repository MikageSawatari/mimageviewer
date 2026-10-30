use super::*;
use crate::settings::{ReadingDirection as Direction, ReadingFlow as Flow, SpreadMode as Mode};

#[test]
fn plain_folder_spread_resolver_table() {
    for enabled in [false, true] {
        for book in [false, true] {
            for directory in [false, true] {
                for mode in [
                    Mode::Single,
                    Mode::LtrCover,
                    Mode::RtlCover,
                    Mode::SplitRtl,
                    Mode::Vertical,
                ] {
                    let defaults = SpreadRestoreDefaults::for_physical_folder(
                        book,
                        directory,
                        enabled,
                        mode,
                        Flow::Horizontal,
                        Direction::Rtl,
                    );
                    let expected_mode = if book || enabled && directory && mode != Mode::Vertical {
                        mode
                    } else {
                        Mode::Single
                    };
                    assert_eq!(
                        defaults.spread_mode, expected_mode,
                        "enabled={enabled} book={book} directory={directory} mode={mode:?}"
                    );
                    assert_eq!(
                        defaults.reading_flow,
                        if book { Flow::Horizontal } else { Flow::Paged }
                    );
                    assert_eq!(
                        defaults.reading_direction,
                        if book { Direction::Rtl } else { Direction::Ltr }
                    );
                    let resolved = crate::spread_db::StoredSpreadState::default().resolve_reading(
                        defaults.spread_mode,
                        defaults.reading_direction,
                        false,
                        None,
                    );
                    assert_eq!(resolved.mode, expected_mode);
                    assert_eq!(
                        resolved.direction,
                        expected_mode
                            .reading_direction()
                            .unwrap_or(defaults.reading_direction)
                    );
                }
            }
        }
    }
}

pub(crate) fn configure(app: &mut App, enabled: bool, book: bool) {
    app.settings.apply_default_spread_to_plain_folders = enabled;
    app.settings.auto_fullscreen_zip_pdf = book;
    app.settings.auto_fullscreen_image_folders = book;
    app.settings.default_spread_mode = Mode::RtlCover;
    app.settings.default_reading_flow = Flow::Horizontal;
    // Deliberately disagree: the embedded page-layout direction must win for a plain folder.
    app.settings.default_reading_direction = Direction::Ltr;
}

pub(crate) fn make_folder(root: &Path, name: &str) -> PathBuf {
    let folder = root.join(name);
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("001.jpg"), b"page").unwrap();
    std::fs::write(folder.join("002.jpg"), b"page").unwrap();
    folder
}

fn reading(app: &App) -> (Mode, Flow, Direction) {
    (app.spread_mode, app.reading_flow, app.reading_direction)
}

#[test]
fn plain_folder_spread_install_uses_scan_and_does_not_persist_defaults() {
    let mut app = phase_c_support::setup_app();
    for (enabled, book, expected_mode, expected_flow) in [
        (false, false, Mode::Single, Flow::Paged),
        (true, false, Mode::RtlCover, Flow::Paged),
        (false, true, Mode::RtlCover, Flow::Horizontal),
        (true, true, Mode::RtlCover, Flow::Horizontal),
    ] {
        configure(&mut app, enabled, book);
        let folder = make_folder(app.tmp.path(), &format!("folder-{enabled}-{book}.txt"));
        app.load_folder(folder.clone());
        app.finish_page_identity_preparation_for_test();
        assert_eq!(app.current_folder.as_ref(), Some(&folder));
        assert!(app.folder_observation.is_some());
        assert_eq!(app.spread_mode, expected_mode);
        assert_eq!(app.reading_flow, expected_flow);
        assert_eq!(
            app.reading_direction,
            expected_mode.reading_direction().unwrap_or(Direction::Ltr)
        );
        assert_eq!(
            app.page_order_locked_for_items(&folder, &app.items, PageOrderViewKind::Physical),
            book
        );
        assert_eq!(
            app.spread_db
                .as_ref()
                .unwrap()
                .get_state_with_fallback(&folder, None),
            crate::spread_db::StoredSpreadState::default()
        );
    }
}

#[test]
fn plain_folder_spread_requires_a_scan_fact_and_normalizes_only_the_new_default() {
    let mut app = phase_c_support::setup_app();
    configure(&mut app, true, false);
    let folder = make_folder(app.tmp.path(), "scan-required");
    // Even an existing directory in a Physical view must have adopted scan evidence.
    app.start_loading_items(
        folder.clone(),
        vec![GridItem::Image(folder.join("001.jpg"))],
        vec![None],
        Default::default(),
        Vec::new(),
        None,
    );
    app.finish_page_identity_preparation_for_test();
    assert_eq!(reading(&app), (Mode::Single, Flow::Paged, Direction::Ltr));
    app.settings.default_spread_mode = Mode::Vertical;
    app.load_folder(folder);
    app.finish_page_identity_preparation_for_test();
    assert_eq!(reading(&app), (Mode::Single, Flow::Paged, Direction::Ltr));
}

#[test]
fn plain_folder_spread_saved_values_and_legacy_flow_win() {
    let mut app = phase_c_support::setup_app();
    configure(&mut app, true, false);
    let folder = make_folder(app.tmp.path(), "stored");
    for mode in [Mode::LtrCover, Mode::Vertical] {
        app.spread_db
            .as_ref()
            .unwrap()
            .set(&folder, mode, Mode::Single, Flow::Paged, Direction::Ltr)
            .unwrap();
        app.spread_db
            .as_ref()
            .unwrap()
            .set_flow(
                &folder,
                Flow::Horizontal,
                Direction::Ltr,
                Mode::Single,
                Flow::Paged,
                Direction::Ltr,
            )
            .unwrap();
        app.load_folder(folder.clone());
        app.finish_page_identity_preparation_for_test();
        assert_eq!(
            reading(&app),
            if mode == Mode::Vertical {
                (Mode::Single, Flow::Vertical, Direction::Ltr)
            } else {
                (Mode::LtrCover, Flow::Horizontal, Direction::Ltr)
            }
        );
    }
}

#[test]
fn plain_folder_spread_extracted_surfaces_keep_single_and_stored_flow() {
    let mut app = phase_c_support::setup_app();
    configure(&mut app, true, false);
    use book_materialization::ExtractedSurface;
    for surface in [
        ExtractedSurface::Search(top_level_grid_view::TopLevelSearchView::Global),
        ExtractedSurface::Search(top_level_grid_view::TopLevelSearchView::Tag),
        ExtractedSurface::Rating,
        ExtractedSurface::Collection,
    ] {
        let path = search_results_synthetic_path();
        app.spread_db
            .as_ref()
            .unwrap()
            .set_flow(
                &path,
                Flow::Vertical,
                Direction::Rtl,
                Mode::Single,
                Flow::Paged,
                Direction::Ltr,
            )
            .unwrap();
        app.start_loading_extracted_items(
            surface,
            path,
            vec![GridItem::Image(PathBuf::from("page.jpg"))],
            vec![None],
            Default::default(),
            Vec::new(),
            None,
        );
        app.finish_page_identity_preparation_for_test();
        assert_eq!(app.spread_mode, Mode::Single);
        assert_eq!(app.reading_flow, Flow::Vertical);
    }
}

#[cfg(windows)]
#[test]
fn plain_folder_spread_import_refresh_limits_opt_in_and_preserves_sibling_context() {
    let mut app = phase_c_support::setup_app();
    let sibling_window = app.allocate_detached_viewer_window_id();
    let sibling = app.build_window_context_for_test(sibling_window, |viewer| {
        viewer.spread_mode = Mode::SplitLtr;
        viewer.reading_flow = Flow::Vertical;
        viewer.reading_direction = Direction::Ltr;
        viewer.set_detached_window_binding_for_test(Some(sibling_window));
    });
    let folder = make_folder(app.tmp.path(), "refresh");
    // The three legacy cases must keep 8b9205743^'s RtlCover/Horizontal/Ltr calculation,
    // even though initial open resolves the embedded Rtl direction differently.
    for (enabled, book, expected) in [
        (
            false,
            false,
            (Mode::LtrCover, Flow::Horizontal, Direction::Ltr),
        ),
        (
            false,
            true,
            (Mode::LtrCover, Flow::Horizontal, Direction::Ltr),
        ),
        (
            true,
            true,
            (Mode::LtrCover, Flow::Horizontal, Direction::Ltr),
        ),
        (true, false, (Mode::RtlCover, Flow::Paged, Direction::Rtl)),
    ] {
        configure(&mut app, enabled, book);
        app.load_folder(folder.clone());
        app.finish_page_identity_preparation_for_test();
        let initial = reading(&app);
        if enabled && !book {
            assert_eq!(
                initial, expected,
                "opt-in plain folder must match normal open"
            );
        }
        for flow in [None, Some(Flow::Vertical)] {
            let result = metadata_import_refresh::ContextResult {
                container_state: Some(metadata_import_refresh::ContainerStateResult {
                    spread: crate::spread_db::StoredSpreadState {
                        flow,
                        ..Default::default()
                    },
                    final_cover_spread_preference: Default::default(),
                    singleton_spread_endpoint_preferences: Default::default(),
                    page_alone_preferences: Default::default(),
                    view_trim: crate::view_trim_db::ViewTrimBookLoad::Missing,
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
            assert_eq!(
                reading(&app),
                (expected.0, flow.unwrap_or(expected.1), expected.2),
                "enabled={enabled} book={book} stored_flow={flow:?}"
            );
            app.with_viewer_context(sibling, |viewer| {
                assert_eq!(
                    reading(viewer),
                    (Mode::SplitLtr, Flow::Vertical, Direction::Ltr)
                );
            })
            .unwrap();
        }
    }
    assert_eq!(
        app.spread_db
            .as_ref()
            .unwrap()
            .get_state_with_fallback(&folder, None),
        crate::spread_db::StoredSpreadState::default()
    );
}

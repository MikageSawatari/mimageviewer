use super::*;
use crate::app::book_materialization::{ExtractedSurface, ProjectionRecipe};
use crate::app::top_level_grid_view::TopLevelSearchView;
use crate::settings::{ReadingDirection, ReadingFlow, SpreadMode};

#[derive(Clone, Copy, Debug)]
enum Entrance {
    Tag,
    Favorite,
    Rating,
}

impl Entrance {
    fn surface(self) -> ExtractedSurface {
        match self {
            Self::Tag => ExtractedSurface::Search(TopLevelSearchView::Tag),
            Self::Favorite => ExtractedSurface::Search(TopLevelSearchView::Favorite),
            Self::Rating => ExtractedSurface::Rating,
        }
    }

    fn key(self) -> PathBuf {
        match self {
            Self::Tag | Self::Favorite => search_results_synthetic_path(),
            Self::Rating => rating_view_synthetic_path(),
        }
    }

    fn adopt(self, app: &mut App, book: &Path, page: &Path) {
        match self {
            Self::Tag => {
                app.tag_view.active = true;
                app.tag_view.saved_folder = Some(book.to_path_buf());
                app.apply_tag_view_result(crate::tag_view::TagViewResult {
                    query: "#g1".into(),
                    kind_filter: crate::tag_view::TagViewKindFilter::All,
                    summaries: Vec::new(),
                    entries: vec![crate::tag_view::TagViewEntry {
                        applied_at_ms: None,
                        path: page.to_path_buf(),
                        kind: crate::tag_view::TagViewItemKind::Image,
                        mtime: 1,
                        file_size: 1,
                    }],
                    truncated: false,
                    video_thumb_overrides: Default::default(),
                });
            }
            Self::Favorite => {
                app.favsearch.active = true;
                app.favsearch.saved_folder = Some(book.to_path_buf());
                // Favorite search produces containers, rather than image leaves. Reopening
                // its result list uses this same real completion handler; opening a row
                // drills into its Book and is deliberately not an A2 image-view session.
                app.apply_favsearch_results(vec![crate::search_index_db::IndexEntry {
                    path: book.to_path_buf(),
                    display_name: "book".into(),
                    kind: crate::search_index_db::IndexKind::Folder,
                    mtime: 1,
                }]);
            }
            Self::Rating => {
                app.rating_view_stars = 3;
                app.rating_view_saved_folder = Some(book.to_path_buf());
                app.apply_rating_view_result(crate::rating_view::RatingViewBuildResult {
                    stars: 3,
                    rows: vec![crate::rating_view::RatingViewRow {
                        key: page.to_string_lossy().into_owned(),
                        item: GridItem::Image(page.to_path_buf()),
                        image_meta: Some((1, 1)),
                        rated_at_ms: Some(1),
                    }],
                    skipped: 0,
                    visibility_notice: None,
                    prepared: None,
                });
            }
        }
        app.finish_page_identity_preparation_for_test();
    }

    fn close(self, app: &mut App) {
        match self {
            Self::Tag => app.close_tag_view(),
            Self::Favorite => app.close_favsearch(),
            Self::Rating => app.close_rating_view(),
        }
        app.settle_open_path_classification_for_test();
        app.finish_history_restore_preparation_for_test();
    }
}

fn exercise_entrance(entrance: Entrance) {
    for (saved_presentation, expected_flow) in [
        (None, ReadingFlow::Paged),
        (
            Some((SpreadMode::RtlCover, ReadingFlow::Horizontal)),
            ReadingFlow::Horizontal,
        ),
        (
            Some((SpreadMode::RtlCover, ReadingFlow::Vertical)),
            ReadingFlow::Vertical,
        ),
        (
            Some((SpreadMode::Vertical, ReadingFlow::Paged)),
            ReadingFlow::Vertical,
        ),
        // The old Vertical encoding overrides even a retained Horizontal flow column.
        (
            Some((SpreadMode::Vertical, ReadingFlow::Horizontal)),
            ReadingFlow::Vertical,
        ),
    ] {
        let _epoch = crate::page_edit_write_epoch::TestEpochScope::fresh();
        let mut app = setup_app_for_test();
        let book = app.tmp.path().join("g1-book");
        std::fs::create_dir_all(&book).unwrap();
        let page = book.join("01.png");
        image::RgbaImage::new(2, 2).save(&page).unwrap();
        app.settings.auto_fullscreen_zip_pdf = true;
        app.settings.auto_fullscreen_image_folders = true;
        app.active_quick_folder_slot = None;
        app.spread_db =
            Ok(crate::spread_db::SpreadDb::open_at(&app.tmp.path().join("g1-spread.db")).unwrap());
        app.spread_db
            .as_ref()
            .unwrap()
            .set_presentation_state(
                &book,
                SpreadMode::RtlCover,
                ReadingFlow::Vertical,
                ReadingDirection::Rtl,
                (
                    SpreadMode::Single,
                    ReadingFlow::Paged,
                    ReadingDirection::Ltr,
                ),
            )
            .unwrap();
        if let Some((mode, flow)) = saved_presentation {
            app.spread_db
                .as_ref()
                .unwrap()
                .set_presentation_state(
                    &entrance.key(),
                    mode,
                    flow,
                    ReadingDirection::Rtl,
                    (
                        SpreadMode::Single,
                        ReadingFlow::Paged,
                        ReadingDirection::Ltr,
                    ),
                )
                .unwrap();
        }
        app.load_folder(book.clone());
        app.settle_open_path_classification_for_test();
        app.finish_history_restore_preparation_for_test();
        assert!(matches!(app.projection_recipe, ProjectionRecipe::Book(_)));
        assert_eq!(app.reading_flow, ReadingFlow::Vertical);

        let saved_state = app
            .spread_db
            .as_ref()
            .unwrap()
            .get_state_with_fallback(&entrance.key(), None);
        let ctx = egui::Context::default();
        for _ in 0..2 {
            entrance.adopt(&mut app, &book, &page);
            assert_eq!(app.extracted_result_surface(), Some(entrance.surface()));
            assert_eq!(app.spread_mode, SpreadMode::Single);
            assert_eq!(app.reading_flow, expected_flow, "{entrance:?}");
            if !matches!(entrance, Entrance::Favorite) {
                app.open_fullscreen(0, HistoryTrigger::UserChosen);
                assert_eq!(app.spread_mode, SpreadMode::Single);
                app.apply_fullscreen_spread_mode(&ctx, 0, SpreadMode::RtlCover);
                assert!(!app.spread_mode.has_cover());
                app.close_fullscreen();
                app.open_fullscreen(0, HistoryTrigger::UserChosen);
                assert_eq!(app.spread_mode, SpreadMode::Single);
                assert_eq!(app.reading_flow, expected_flow);
                app.close_fullscreen();
            }
            entrance.close(&mut app);
            assert_eq!(app.current_folder.as_ref(), Some(&book));
            assert!(matches!(app.projection_recipe, ProjectionRecipe::Book(_)));
            assert_eq!(app.spread_mode, SpreadMode::RtlCover);
            assert_eq!(app.reading_flow, ReadingFlow::Vertical);
            assert_eq!(
                app.spread_db
                    .as_ref()
                    .unwrap()
                    .get_state_with_fallback(&entrance.key(), None),
                saved_state
            );
        }
    }
}

#[test]
fn g1_tag_handler_restores_entrance_flow_and_book_on_return() {
    exercise_entrance(Entrance::Tag);
}

#[test]
fn g1_favorite_handler_restores_entrance_flow_and_book_on_return() {
    exercise_entrance(Entrance::Favorite);
}

#[test]
fn g1_rating_handler_restores_entrance_flow_and_book_on_return() {
    exercise_entrance(Entrance::Rating);
}

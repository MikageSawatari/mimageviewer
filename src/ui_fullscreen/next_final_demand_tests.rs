use super::*;

#[test]
fn d3_viewport_entry_adopts_before_its_first_selector() {
    let ctx = egui::Context::default();
    let mut app =
        crate::ui_fullscreen::page_turn_submission_tests::ready_app(&ctx, SpreadMode::Single);
    app.settings.page_turn_animation_enabled = true;
    let key = crate::app::final_prepare_tests::queued_final(&mut app, 0);
    crate::app::final_prepare_tests::expect_adopted_before_lookup(key, || {
        assert!(matches!(
            app.begin_page_turn_viewer_frame(&ctx, true, None),
            PageTurnFrameEntry::Continue { .. }
        ));
        let _ = app.resolve_fs_display_tex(0, false).unwrap();
    });
}

fn demand(
    units: &[SpreadDisplayUnit],
    current: usize,
    mode: SpreadMode,
    cover: bool,
) -> NextFinalUnitDemand {
    next_final_unit_demand(units, current, mode, cover, true.into(), true)
}

#[test]
fn d3_next_single_unit_uses_reader_ordinal_and_boundary() {
    let units: Vec<_> = [17, 3, 91]
        .into_iter()
        .enumerate()
        .map(|(pos, idx)| {
            SpreadDisplayUnit::singleton(pos, idx, SpreadDisplaySingletonCause::UnpairedSlot)
        })
        .collect();
    assert_eq!(
        demand(&units, 0, SpreadMode::Single, false),
        NextFinalUnitDemand::Unit(vec![SpreadPageOccurrence::navigation(3, 3)])
    );
    assert_eq!(
        demand(&units, 2, SpreadMode::Single, false),
        NextFinalUnitDemand::Boundary
    );
}

#[test]
fn d3_next_spread_keeps_all_reading_occurrences_for_ltr_rtl_and_cover() {
    for mode in [
        SpreadMode::Ltr,
        SpreadMode::Rtl,
        SpreadMode::LtrCover,
        SpreadMode::RtlCover,
    ] {
        let units = build_spread_display_units_with_predicates(
            &[8, 5, 19, 2, 60, 31],
            mode,
            None,
            |_, _| false,
            |_| true,
        );
        let NextFinalUnitDemand::Unit(pages) = demand(&units, 0, mode, false) else {
            panic!()
        };
        let expected = if mode.has_cover() {
            vec![5, 19]
        } else {
            vec![19, 2]
        };
        assert_eq!(
            pages.iter().map(|page| page.idx).collect::<Vec<_>>(),
            expected,
            "{mode:?}"
        );
        assert!(
            pages
                .iter()
                .all(|page| page.role == SpreadPageRole::Navigation)
        );
    }
}

#[test]
fn d3_next_landscape_alone_blank_and_vacant_never_invent_pixel_pages() {
    use crate::book_composition::{PageOverride, PageSide};
    for mode in [SpreadMode::Ltr, SpreadMode::Rtl] {
        for directive in [
            PageOverride::Alone,
            PageOverride::BlankOpposite {
                page_side: PageSide::Leading,
            },
            PageOverride::BlankOpposite {
                page_side: PageSide::Trailing,
            },
        ] {
            let overrides = HashMap::from([(2, directive)]);
            let units = build_spread_display_units_with_overrides(
                &[0, 1, 2, 3],
                mode,
                None,
                Default::default(),
                &overrides,
                |_, _| false,
                |_| true,
            );
            assert_eq!(
                demand(&units, 0, mode, true),
                NextFinalUnitDemand::Unit(vec![SpreadPageOccurrence::navigation(2, 2)]),
                "{mode:?} {directive:?}"
            );
        }
        let units = build_spread_display_units_with_predicates(
            &[0, 1, 2, 3],
            mode,
            None,
            |_, idx| idx == 2,
            |_| true,
        );
        assert_eq!(
            demand(&units, 0, mode, false),
            NextFinalUnitDemand::Unit(vec![SpreadPageOccurrence::navigation(2, 2)])
        );
        assert_eq!(
            demand(&units, 1, mode, false),
            NextFinalUnitDemand::Unit(vec![SpreadPageOccurrence::navigation(3, 3)])
        );
    }
}

#[test]
fn d3_next_final_cover_preserves_supplement_occurrence_then_boundary() {
    for mode in [SpreadMode::LtrCover, SpreadMode::RtlCover] {
        let units = build_spread_display_units_with_predicates(
            &[10, 30, 20, 70],
            mode,
            None,
            |_, _| false,
            |_| true,
        );
        let final_pos = units.len() - 1;
        assert_eq!(
            demand(&units, final_pos - 1, mode, true),
            NextFinalUnitDemand::Unit(vec![
                SpreadPageOccurrence::navigation(70, 70),
                SpreadPageOccurrence::final_cover_supplement(10, 70)
            ])
        );
        assert_eq!(
            demand(&units, final_pos, mode, true),
            NextFinalUnitDemand::Boundary
        );
    }
}

#[test]
fn d3_slideshow_observation_uses_actual_still_units_without_mutating_visible_layout() {
    let mut app = crate::app::setup_app_for_test();
    app.items = vec![
        GridItem::Image(PathBuf::from("C:/d3/a.png")),
        GridItem::Video(PathBuf::from("C:/d3/video.mp4")),
        GridItem::Image(PathBuf::from("C:/d3/b.png")),
        GridItem::Image(PathBuf::from("C:/d3/c.png")),
    ];
    app.thumbnails = vec![ThumbnailState::Pending; 4];
    app.visible_indices = vec![0, 1, 2, 3];
    app.details_order = app.visible_indices.clone();
    app.fullscreen_idx = Some(0);
    for mode in [SpreadMode::Single, SpreadMode::Ltr, SpreadMode::Rtl] {
        app.spread_mode = mode;
        for idx in 0..4 {
            app.rotation_cache
                .insert(idx, crate::rotation_db::Rotation::None);
        }
        let nav = app.get_nav_indices();
        app.build_spread_display_units_for_nav(&nav);
        let layout = Arc::clone(&app.spread_display_units_cache.units);
        let generation = app.items_generation;
        app.slideshow_playing = true;
        let NextFinalUnitDemand::Unit(pages) = app.next_final_unit_demand_from_snapshot(0).unwrap()
        else {
            panic!()
        };
        assert_eq!(
            pages.iter().map(|page| page.idx).collect::<Vec<_>>(),
            if mode == SpreadMode::Single {
                vec![2]
            } else {
                vec![3]
            }
        );
        assert!(Arc::ptr_eq(&layout, &app.spread_display_units_cache.units));
        assert_eq!(app.items_generation, generation);
        assert!(app.final_effect_pending.is_empty());
        app.slideshow_playing = false;
        if mode == SpreadMode::Single {
            assert_eq!(
                app.next_final_unit_demand_from_snapshot(0),
                Some(NextFinalUnitDemand::Unit(Vec::new())),
                "manual next is video, do not skip ahead to an image"
            );
        }
    }
}

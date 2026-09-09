use super::*;

fn paged_spread_app(mode: crate::settings::SpreadMode) -> crate::app::AppTestEnvForTest {
    let mut app = setup_app_for_test();
    app.items = (0..8)
        .map(|page| GridItem::Image(PathBuf::from(format!("C:/prefetch/{page:02}.png"))))
        .collect();
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    app.visible_indices = (0..app.items.len()).collect();
    app.viewer_navigation_caches.invalidate();
    app.spread_mode = mode;
    app
}

#[test]
fn default_prefetch_range_covers_both_adjacent_spread_units_from_the_landing_anchor() {
    for (mode, anchor, expected_pair, previous_anchor, next_anchor, expected_targets) in [
        (
            crate::settings::SpreadMode::Ltr,
            2,
            crate::ui_fullscreen::SpreadPair::Double { left: 2, right: 3 },
            0,
            4,
            vec![3, 1, 4, 0, 5],
        ),
        (
            crate::settings::SpreadMode::Rtl,
            2,
            crate::ui_fullscreen::SpreadPair::Double { left: 3, right: 2 },
            0,
            4,
            vec![3, 1, 4, 0, 5],
        ),
        (
            crate::settings::SpreadMode::LtrCover,
            3,
            crate::ui_fullscreen::SpreadPair::Double { left: 3, right: 4 },
            1,
            5,
            vec![4, 2, 5, 1, 6],
        ),
        (
            crate::settings::SpreadMode::RtlCover,
            3,
            crate::ui_fullscreen::SpreadPair::Double { left: 4, right: 3 },
            1,
            5,
            vec![4, 2, 5, 1, 6],
        ),
    ] {
        let mut app = paged_spread_app(mode);
        let nav = (0..app.items.len()).collect::<Vec<_>>();

        assert_eq!(app.settings.ai_upscale_prefetch_forward, 3);
        assert_eq!(app.settings.ai_upscale_prefetch_back, 2);
        assert_eq!(
            app.spread_display_unit_pages_for_test(&nav),
            match mode {
                crate::settings::SpreadMode::Ltr => {
                    vec![vec![0, 1], vec![2, 3], vec![4, 5], vec![6, 7]]
                }
                crate::settings::SpreadMode::Rtl => {
                    vec![vec![1, 0], vec![3, 2], vec![5, 4], vec![7, 6]]
                }
                crate::settings::SpreadMode::LtrCover => {
                    vec![vec![0], vec![1, 2], vec![3, 4], vec![5, 6], vec![7]]
                }
                crate::settings::SpreadMode::RtlCover => {
                    vec![vec![0], vec![2, 1], vec![4, 3], vec![6, 5], vec![7]]
                }
                _ => unreachable!(),
            }
        );

        // Direct-open / seek landing resolves to the unit's reading-order anchor.
        app.fullscreen_idx = Some(anchor);
        assert_eq!(app.resolve_spread_pair(anchor), expected_pair);
        assert_eq!(
            app.spread_page_nav_for_indices(&nav, anchor, -1),
            crate::ui_fullscreen::FsPageNav::Target(previous_anchor)
        );
        assert_eq!(
            app.spread_page_nav_for_indices(&nav, anchor, 1),
            crate::ui_fullscreen::FsPageNav::Target(next_anchor)
        );

        // From anchor 2, distance 1 is the current partner. Backward 2 and
        // forward 3 are therefore the first ranges that contain every page of
        // the previous and next two-page units.
        let targets = app.ai_prefetch_targets(anchor);
        assert_eq!(targets, expected_targets);
        let adjacent_pages = match mode.has_cover() {
            false => [0, 1, 4, 5],
            true => [1, 2, 5, 6],
        };
        for adjacent_page in adjacent_pages {
            assert!(
                targets.contains(&adjacent_page),
                "{mode:?}: adjacent spread page {adjacent_page} must be prefetched"
            );
        }
    }
}

#[test]
fn first_spread_anchor_uses_one_forward_slot_for_its_current_partner() {
    for mode in [
        crate::settings::SpreadMode::Ltr,
        crate::settings::SpreadMode::Rtl,
    ] {
        let mut app = paged_spread_app(mode);
        app.fullscreen_idx = Some(0);
        assert_eq!(
            app.ai_prefetch_targets(0),
            vec![1, 2, 3],
            "{mode:?}: forward 3 must include current partner plus both next-unit pages"
        );
    }
}

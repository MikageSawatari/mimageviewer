use super::*;
use crate::app::App;
use crate::app::AppTestEnvForTest;
use crate::ui_dialogs::image_combine::CombineEligibilityWork;

fn images() -> AppTestEnvForTest {
    let mut app = crate::app::setup_app_for_test();
    app.items = (0..10_000)
        .map(|n| GridItem::Image(PathBuf::from(format!("C:/images/{n}.png"))))
        .collect();
    app.visible_indices = (0..app.items.len()).collect();
    app.checked = (0..app.items.len()).collect();
    app.move_grid_cursor(Some(0), crate::app::GridCursorIntent::Automatic);
    app.cached_handlers.insert(".png".into(), Vec::new());
    app
}

fn stacks(groups: usize, members: usize) -> AppTestEnvForTest {
    use crate::filename_stack::{StackMember, StackView};
    let mut app = crate::app::setup_app_for_test();
    let media = (0..groups)
        .flat_map(|group| {
            (0..members).map(move |member| StackMember {
                path: PathBuf::from(format!("C:/images/group{group:05}_{member:05}.png")),
                mtime: 0,
                size: Some(8),
                sort_meta: crate::settings::ListingSortMetadata::new(0, Some(8)),
                is_video: false,
            })
        })
        .collect();
    let view = StackView::build(
        PathBuf::from("C:/images"),
        Vec::new(),
        Vec::new(),
        media,
        '_',
        crate::settings::SortOrder::FileName,
    );
    app.items = view.materialize_aggregated().0;
    assert_eq!(app.items.len(), groups);
    app.stack_view = Some(std::sync::Arc::new(view));
    app.visible_indices = (0..app.items.len()).collect();
    app.checked = (0..app.items.len()).collect();
    app.move_grid_cursor(Some(0), crate::app::GridCursorIntent::Automatic);
    app.cached_handlers.insert(".png".into(), Vec::new());
    app
}

fn menu_repaints(app: &mut App, rows: usize, stack_lookups: usize) {
    let ctx = egui::Context::default();
    // Existing Shell/other-command snapshots are outside combine's admission
    // work. Exercise the actual menu builder after capturing the open owner.
    let target = app.context_menu_target(
        0,
        app.items[0].clone(),
        false,
        true,
        None,
        ContextMenuSurface::Grid,
        None,
    );
    app.image_combine
        .eligibility_work
        .set(CombineEligibilityWork::default());
    app.context_menu_idx = Some(app.capture_grid_context_menu_owner(0));
    assert_eq!(app.context_menu_idx.unwrap().combine_target_count, 101);
    for _ in 0..5 {
        let nodes = app.context_menu_nodes(&ctx, &target, false);
        let combine = nodes
            .iter()
            .find(|node| {
                matches!(
                    node,
                    MenuNode::Item {
                        command: MenuCommand::CombineSelection,
                        ..
                    }
                )
            })
            .unwrap();
        assert!(
            matches!(combine, MenuNode::Item { enabled: false, disabled_reason: Some(reason), .. } if reason.contains("101 枚以上"))
        );
    }
    assert_eq!(
        app.image_combine.eligibility_work.get(),
        CombineEligibilityWork {
            passes: 1,
            rows,
            stack_lookups,
            materializations: 0,
            page_keys: 0,
        }
    );
    // Both execution routes must reject before sorting/materializing anything.
    app.open_image_combine_context_dialog(&ctx, Some(0), false);
    app.open_image_combine_dialog(&ctx);
    assert!(app.image_combine.session.is_none());
    assert_eq!(app.image_combine.eligibility_work.get().materializations, 0);
    assert_eq!(app.image_combine.eligibility_work.get().page_keys, 0);
}

#[test]
fn image_combine_large_checked_production_menu_counts_once_without_page_keys() {
    menu_repaints(&mut images(), 101, 0);
}

#[test]
fn image_combine_large_stack_production_menu_uses_indexed_bounded_counts() {
    menu_repaints(&mut stacks(10_000, 2), 51, 51);
}

#[test]
fn image_combine_one_large_stack_production_menu_reads_one_cached_member_count() {
    menu_repaints(&mut stacks(1, 20_000), 1, 1);
}

#[test]
fn image_combine_reopen_production_menu_requires_one_real_image_and_uses_checked_target() {
    let mut app = crate::app::setup_app_for_test();
    app.items = vec![
        GridItem::Image(PathBuf::from("C:/images/a.png")),
        GridItem::Image(PathBuf::from("C:/images/b.png")),
        GridItem::Video(PathBuf::from("C:/images/c.mp4")),
        GridItem::ZipImage {
            zip_path: PathBuf::from("C:/images/book.zip"),
            entry_name: "page.png".into(),
        },
    ];
    app.visible_indices = (0..app.items.len()).collect();
    app.move_grid_cursor(Some(0), crate::app::GridCursorIntent::Automatic);
    app.cached_handlers.insert(".png".into(), Vec::new());
    app.cached_handlers.insert(".mp4".into(), Vec::new());
    let ctx = egui::Context::default();
    for (checked, clicked, expected) in [
        (vec![], 0, true),
        (vec![], 2, false),
        (vec![], 3, false),
        (vec![1], 2, true),
        (vec![2], 0, false),
        (vec![0, 1], 0, false),
    ] {
        app.checked = checked.into_iter().collect();
        assert_eq!(
            app.image_combine_reopen_visible(Some(clicked), false),
            expected
        );
        let target = app.context_menu_target(
            clicked,
            app.items[clicked].clone(),
            false,
            !app.checked.is_empty(),
            None,
            ContextMenuSurface::Grid,
            None,
        );
        let shown = app.context_menu_nodes(&ctx, &target, false);
        assert_eq!(
            shown.iter().any(|node| matches!(
                node,
                MenuNode::Item {
                    command: MenuCommand::CombineReopen,
                    enabled: true,
                    ..
                }
            )),
            expected
        );
    }
}

#[test]
fn image_combine_reopen_production_menu_uses_loaded_index_for_the_effective_image() {
    let mut app = crate::app::setup_app_for_test();
    let first = PathBuf::from("C:/images/a.png");
    let second = PathBuf::from("C:/images/b.png");
    app.items = vec![
        GridItem::Image(first.clone()),
        GridItem::Image(second.clone()),
    ];
    app.visible_indices = vec![0, 1];
    app.move_grid_cursor(Some(0), crate::app::GridCursorIntent::Automatic);
    app.cached_handlers.insert(".png".into(), Vec::new());
    app.image_combine.index.set_keys_for_test(
        [crate::adjustment_db::normalize_path(&second)]
            .into_iter()
            .collect(),
    );
    assert!(!app.image_combine_reopen_visible(Some(0), false));
    assert!(
        app.image_combine_reopen_disabled_reason(Some(0), false)
            .is_some()
    );
    let ctx = egui::Context::default();
    for (checked, expected) in [(vec![], false), (vec![1], true)] {
        app.checked = checked.into_iter().collect();
        let target = app.context_menu_target(
            0,
            app.items[0].clone(),
            false,
            !app.checked.is_empty(),
            None,
            ContextMenuSurface::Grid,
            None,
        );
        let shown = app.context_menu_nodes(&ctx, &target, false);
        assert_eq!(
            shown.iter().any(|node| matches!(
                node,
                MenuNode::Item {
                    command: MenuCommand::CombineReopen,
                    enabled: true,
                    ..
                }
            )),
            expected
        );
    }
    app.image_combine
        .index
        .set_keys_for_test(Default::default());
    assert!(!app.image_combine_reopen_visible(Some(0), false));
    assert!(
        app.image_combine_reopen_disabled_reason(Some(0), false)
            .is_some()
    );
}

#[test]
fn image_combine_tools_production_menu_reuses_snapshot_and_recounts_after_close() {
    let mut app = images();
    let ctx = egui::Context::default();
    app.image_combine
        .eligibility_work
        .set(CombineEligibilityWork::default());
    let mut first = None;
    for _ in 0..5 {
        let reason = app.combine_tools_menu_disabled_reason();
        assert!(reason.as_ref().unwrap().contains("101 枚以上"));
        if first.is_none() {
            first = reason.clone();
        }
        assert_eq!(reason, first);
        ctx.run_ui(Default::default(), |ui| {
            let _ = crate::ui_tools_menu::draw(
                ui,
                &[crate::keymap::MenuCommandId::ToolsCombineImages],
                &app.keymap,
                crate::ui_tools_menu::ToolsMenuState {
                    convert_enabled: false,
                    combine_disabled_reason: reason.as_deref(),
                    combine_reopen_disabled_reason: None,
                    caption_disabled_reason: None,
                    caption_progress_available: false,
                    transcript_disabled_reason: None,
                    transcript_progress_available: false,
                    video_selected: false,
                },
            );
        })
        .drop_without_applying_deltas();
    }
    assert_eq!(
        app.image_combine.eligibility_work.get(),
        CombineEligibilityWork {
            passes: 1,
            rows: 101,
            ..Default::default()
        }
    );
    // The menu bar clears this owner when MenuResponse.inner is None.
    app.image_combine.tools_menu_owner = None;
    app.checked.clear();
    app.checked.insert(0);
    assert!(app.combine_tools_menu_disabled_reason().is_none());
    assert_eq!(
        app.image_combine.eligibility_work.get(),
        CombineEligibilityWork {
            passes: 2,
            rows: 102,
            ..Default::default()
        }
    );
}

use super::*;
use crate::app::smart_folder::{SmartChildKind, SmartHistoryDirection};
use crate::app::top_level_grid_view::{
    SmartFolderPosition, SmartFolderViewState, TopLevelGridSurface,
};

fn smart_child_fixture() -> (phase_c_support::AppTestEnv, PathBuf, PathBuf, uuid::Uuid) {
    let mut app = fixture();
    app.active_quick_folder_slot = None;
    app.settings.sort_order = SortOrder::DurationAsc;
    app.settings.remember_favorite_view_state = false;
    let prior = app.tmp.path().join("duration-smart-prior");
    let source = app.tmp.path().join("duration-smart-source");
    let child = source.join("book");
    std::fs::create_dir_all(&prior).unwrap();
    std::fs::create_dir_all(&child).unwrap();
    for name in ["a-long.mp4", "b-short.mp4"] {
        std::fs::write(child.join(name), b"fixture").unwrap();
    }
    app.current_folder = Some(prior.clone());
    app.address = prior.display().to_string();
    app.install_new_items(vec![GridItem::Folder(prior.join("visible"))], vec![None]);
    app.visible_indices = vec![0];
    app.top_level_grid_view
        .replace_surface(TopLevelGridSurface::Folder);
    let mut definition = crate::settings::SmartFolderDefinition::new("Duration Smart Child");
    definition.rules.push(crate::settings::SmartFolderRule::new(
        source,
        true,
        Default::default(),
    ));
    let id = definition.id;
    app.settings.smart_folders = vec![definition];
    (app, prior, child, id)
}

fn cache_child_durations(app: &mut App, child: &Path) {
    let scan = folder_scan::scan_directory_with_settings(child, &app.settings).unwrap();
    assert_eq!(scan.all_media.len(), 2);
    for entry in scan.all_media {
        let seconds = if entry.path.ends_with("a-long.mp4") {
            600.0
        } else {
            assert!(entry.path.ends_with("b-short.mp4"));
            30.0
        };
        app.details_lazy_meta.insert(
            crate::adjustment_db::normalize_path(&entry.path),
            DetailsLazyMeta {
                source_mtime: entry.mtime,
                source_size: entry.file_size,
                media: DetailsMediaMeta::Read(DetailsVideoProbe {
                    duration_secs: Some(seconds),
                    dims: None,
                    codec: None,
                }),
                ..Default::default()
            },
        );
        assert_eq!(
            app.listing_duration_for_path(
                &entry.path,
                crate::settings::ListingSortMetadata::new(entry.mtime, Some(entry.file_size)),
            ),
            Some(crate::settings::ListingDuration::Known(
                (seconds * 1000.0) as u64,
            )),
            "the adoption must see known durations with matching source stamps",
        );
    }
}

fn settle_smart(app: &mut App, ctx: &egui::Context, ready: impl Fn(&App) -> bool) {
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !ready(app) || app.sidecar_restore_active() {
        assert!(
            std::time::Instant::now() < deadline,
            "Smart destination adoption did not finish: current={:?}, surface={:?}, transition={}",
            app.current_folder,
            app.top_level_grid_view.surface(),
            app.smart_folder_transition.is_some(),
        );
        app.poll_sidecar_restore(ctx);
        app.poll_smart_folder(ctx);
        std::thread::yield_now();
    }
}

fn assert_name_sorted_smart_child(app: &App, child: &Path, id: uuid::Uuid) {
    assert_eq!(app.current_folder.as_deref(), Some(child));
    assert_eq!(app.settings.sort_order, SortOrder::DurationAsc);
    assert!(!app.items_are_smart_folder_view);
    let state = app.top_level_grid_view.smart_folder().unwrap();
    assert_eq!(state.definition_id, id);
    assert_eq!(state.scoped_current(), Some(child));
    assert!(app.top_level_grid_view.smart_folder_session().is_some());
    assert_eq!(
        app.items
            .iter()
            .map(|item| match item {
                GridItem::Video(path) => path.file_name().unwrap().to_str().unwrap(),
                other => panic!("unexpected Smart child row: {other:?}"),
            })
            .collect::<Vec<_>>(),
        ["a-long.mp4", "b-short.mp4"],
        "Smart physical children use name order even with a common duration sort",
    );
    assert!(!app.listing_sort_capabilities().duration_allowed);
}

#[test]
fn duration_762_offscreen_smart_history_child_uses_destination_name_sort() {
    let (mut app, prior, child, id) = smart_child_fixture();
    cache_child_durations(&mut app, &child);
    assert!(app.top_level_grid_view.smart_folder_session().is_none());
    assert!(matches!(
        app.top_level_grid_view.surface(),
        TopLevelGridSurface::Folder,
    ));
    assert!(app.listing_sort_capabilities().duration_allowed);
    let mut target = SmartFolderViewState::root(id, vec![child.clone()]);
    target.position = SmartFolderPosition::Scoped {
        entry_index: 0,
        entry_root: child.clone(),
        current: child.clone(),
        current_kind: SmartChildKind::Folder,
        back_stack: Vec::new(),
    };
    app.folder_nav_back_stack
        .push(FolderNavHistoryTarget::SmartFolder(target.clone().into()).into());
    let ctx = egui::Context::default();

    // With no resident session, history rebuilds the root offscreen and prepares the child
    // while the visible source still owns an ordinary Folder surface.
    assert!(app.begin_smart_history_navigation(target, SmartHistoryDirection::Back));
    assert_eq!(app.current_folder.as_deref(), Some(prior.as_path()));
    assert_eq!(app.folder_nav_back_stack.len(), 1);
    settle_smart(&mut app, &ctx, |app| {
        if app.current_folder.as_deref() != Some(child.as_path()) {
            assert_eq!(app.current_folder.as_deref(), Some(prior.as_path()));
            assert!(matches!(
                app.top_level_grid_view.surface(),
                TopLevelGridSurface::Folder,
            ));
            assert!(app.top_level_grid_view.smart_folder_session().is_none());
        }
        app.smart_folder_transition.is_none()
            && app.current_folder.as_deref() == Some(child.as_path())
    });

    assert_name_sorted_smart_child(&app, &child, id);
    assert!(app.folder_nav_back_stack.is_empty());
    assert_eq!(
        app.folder_nav_forward_stack
            .last()
            .map(|entry| &entry.location),
        Some(&FolderNavHistoryTarget::Path(prior)),
    );
}

#[test]
fn duration_762_resident_smart_child_uses_destination_name_sort() {
    let (mut app, _prior, child, id) = smart_child_fixture();
    let ctx = egui::Context::default();
    app.open_smart_folder(id, false);
    settle_smart(&mut app, &ctx, |app| {
        app.smart_folder_transition.is_none()
            && app.items_are_smart_folder_view
            && app.current_smart_folder_id == Some(id)
    });
    assert!(app.top_level_grid_view.smart_folder_session().is_some());
    assert_eq!(app.settings.sort_order, SortOrder::DurationAsc);
    cache_child_durations(&mut app, &child);

    assert!(app.begin_staged_smart_drill(&child));
    settle_smart(&mut app, &ctx, |app| {
        app.smart_folder_transition.is_none()
            && app.current_folder.as_deref() == Some(child.as_path())
    });

    assert_name_sorted_smart_child(&app, &child, id);
}

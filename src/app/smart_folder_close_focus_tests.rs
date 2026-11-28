//! R1: actual Smart dispatch owners retain entry-frame Focus, never adoption Focus.
use super::*;
use crate::app::top_level_grid_view::{
    SmartFolderPosition, SmartFolderViewState, TopLevelGridSurface,
};
use crate::app::{App, AppTestEnvForTest, FullscreenCloseRequestOutcome, ViewerPresentation};
use crate::ui_main::AddressBarNav;

fn frame(
    ctx: &egui::Context,
    target: egui::ViewportId,
    surface: egui::ViewportId,
    run: impl FnOnce(),
) -> Vec<egui::ViewportId> {
    ctx.begin_pass(egui::RawInput {
        viewport_id: target,
        viewports: [
            (egui::ViewportId::ROOT, egui::ViewportInfo::default()),
            (
                surface,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    ..Default::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    });
    run();
    let output = ctx.end_pass();
    let targets = output
        .viewport_output
        .iter()
        .flat_map(|(target, viewport)| {
            viewport.commands.iter().filter_map(move |command| {
                matches!(command, egui::ViewportCommand::Focus).then_some(*target)
            })
        })
        .collect();
    output.drop_without_applying_deltas();
    targets
}

fn reader(physical_child: bool) -> (AppTestEnvForTest, uuid::Uuid, PathBuf) {
    reader_with_parent(physical_child, "child")
}

fn reader_with_parent(
    physical_child: bool,
    parent: &str,
) -> (AppTestEnvForTest, uuid::Uuid, PathBuf) {
    let mut app = crate::app::setup_app_for_test();
    app.indexer_init.mark_unavailable_if_not_ready();
    app.active_quick_folder_slot = None;
    let entry = app.tmp.path().join("smart-close-book");
    let current = if physical_child {
        entry.join(parent).join("leaf")
    } else {
        entry.clone()
    };
    std::fs::create_dir_all(&current).unwrap();
    let page = current.join("page.png");
    image::RgbImage::new(2, 3).save(&page).unwrap();
    let other = app.tmp.path().join("smart-close-other");
    std::fs::create_dir(&other).unwrap();
    let id = super::tests::install_smart_identity_test_root_with_items(
        &mut app,
        Default::default(),
        vec![GridItem::Folder(entry.clone()), GridItem::Folder(other)],
    );
    let definition = app
        .top_level_grid_view
        .smart_folder_session()
        .unwrap()
        .root_snapshot()
        .unwrap()
        .definition
        .clone();
    app.settings.smart_folders = vec![definition];
    let grid = app.take_visible_smart_folder_grid();
    let mut session = app.top_level_grid_view.take_smart_folder_session().unwrap();
    session.phase = SmartFolderOpenPhase::Child {
        logical_path: current.clone(),
        parked_root: SmartFolderRootPayload::Visible(grid),
    };
    app.top_level_grid_view
        .install_smart_folder_session(session);
    let mut state = SmartFolderViewState::root(id, vec![entry]);
    assert!(state.enter_containing_path(&current));
    app.top_level_grid_view
        .replace_surface(TopLevelGridSurface::SmartFolder(state));
    app.current_folder = Some(current.clone());
    app.items_are_smart_folder_view = false;
    app.items = vec![GridItem::Image(page)];
    app.thumbnails = vec![ThumbnailState::Failed];
    app.image_metas = vec![None];
    app.visible_indices = vec![0];
    app.details_order = vec![0];
    app.selected = Some(0);
    app.page_identity_index = Arc::new(
        super::super::file_type_resume::PreparedPageIdentityIndex::prepare(
            &app.items,
            None,
            app.settings.file_type_visibility.clone(),
        ),
    );
    app.fullscreen_idx = Some(0);
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.auto_fullscreen_zip_pdf = true;
    app.settings.auto_fullscreen_image_folders = true;
    (app, id, current)
}

fn request_close(app: &mut App, ctx: &egui::Context) -> AddressBarNav {
    let outcome = app.handle_fullscreen_close_request();
    assert_eq!(outcome, FullscreenCloseRequestOutcome::ReturnAccepted);
    app.route_fullscreen_close_entry_focus(ctx, outcome, true, "smart-close-focus-test");
    app.take_pending_return_to_parent_nav_with_focus(ctx)
        .unwrap()
}

#[test]
fn section1853_close_focus_smart_resident_root_closes_and_focuses_in_dispatch_frame() {
    for child_viewport in [false, true] {
        let (mut app, id, _) = reader(false);
        let ctx = egui::Context::default();
        let surface = app.fullscreen_viewport_id();
        let target = if child_viewport {
            surface
        } else {
            egui::ViewportId::ROOT
        };
        assert!(app.resident_smart_root_restore_is_exact(&app.settings.smart_folders[0]));
        assert_eq!(
            frame(&ctx, target, surface, || {
                let nav = request_close(&mut app, &ctx);
                assert!(
                    matches!(&nav, AddressBarNav::Direct(path, _) if is_smart_folder_synthetic_path(path))
                );
                assert!(app.apply_fullscreen_close_nav_immediate_with_focus(&ctx, nav));
                assert!(
                    app.take_pending_return_to_parent_nav_with_focus(&ctx)
                        .is_none()
                );
            }),
            [target]
        );
        assert_eq!(app.fullscreen_idx, None);
        assert_eq!(
            app.effective_folder(),
            Some(smart_folder_synthetic_path(id))
        );
        assert!(app.smart_folder_transition.is_none());
        assert!(
            frame(&ctx, egui::ViewportId::ROOT, surface, || {
                app.poll_smart_folder(&ctx);
            })
            .is_empty()
        );
    }
}

#[test]
fn section1853_close_focus_smart_staged_root_and_physical_child_focus_only_in_dispatch_frame() {
    for physical_child in [false, true] {
        for child_viewport in [false, true] {
            for ordinary_tail in [false, true] {
                let (mut app, id, source) = reader(physical_child);
                let ctx = egui::Context::default();
                let surface = app.fullscreen_viewport_id();
                let target = if child_viewport {
                    surface
                } else {
                    egui::ViewportId::ROOT
                };
                let destination = if physical_child {
                    source.parent().unwrap().to_path_buf()
                } else {
                    // A real metadata stamp difference forces worker reprepare of the root.
                    app.smart_folder_metadata_revision =
                        app.smart_folder_metadata_revision.wrapping_add(1);
                    assert!(
                        !app.resident_smart_root_restore_is_exact(&app.settings.smart_folders[0])
                    );
                    smart_folder_synthetic_path(id)
                };
                assert_eq!(
                    frame(&ctx, target, surface, || {
                        let nav = request_close(&mut app, &ctx);
                        assert!(
                            matches!(&nav, AddressBarNav::Direct(path, _) if path == &destination)
                        );
                        if ordinary_tail {
                            app.post_render_main_navigation(
                                &ctx,
                                super::super::update_phases::MainFrameNavigation {
                                    fullscreen_close_nav: Some(nav),
                                    ..Default::default()
                                },
                            );
                        } else {
                            assert!(app.apply_fullscreen_close_nav_immediate_with_focus(&ctx, nav));
                        }
                        assert!(
                            app.take_pending_return_to_parent_nav_with_focus(&ctx)
                                .is_none()
                        );
                    }),
                    [target],
                    "physical={physical_child}, child={child_viewport}, ordinary={ordinary_tail}"
                );
                assert_eq!(app.fullscreen_idx, Some(0));
                assert_eq!(app.current_folder.as_ref(), Some(&source));
                assert!(!app.fs_close_return_navigation_wait_active());
                assert!(app.folder_pane_open_pending.is_none());
                let transition = app.smart_folder_transition.as_ref().unwrap();
                assert!(app.main_list_navigation_is_current(&transition.navigation));
                assert!(match (&transition.target, physical_child) {
                    (SmartFolderTransitionTarget::Root(_), false) => true,
                    (SmartFolderTransitionTarget::Child { state, .. }, true) =>
                        matches!(&state.position, SmartFolderPosition::Scoped { current, .. } if current == &destination),
                    _ => false,
                });
                let deadline = Instant::now() + Duration::from_secs(30);
                while app.smart_folder_transition.is_some() || app.sidecar_restore_active() {
                    assert!(
                        Instant::now() < deadline,
                        "Smart close return did not adopt"
                    );
                    assert!(
                        frame(&ctx, egui::ViewportId::ROOT, surface, || {
                            app.poll_smart_folder(&ctx);
                            app.finish_page_identity_preparation_for_test();
                            app.poll_sidecar_restore(&ctx);
                        })
                        .is_empty(),
                        "Smart adoption must not add a second Focus"
                    );
                    std::thread::yield_now();
                }
                assert_eq!(app.fullscreen_idx, None);
                assert_eq!(app.effective_folder(), Some(destination));
            }
        }
    }
}

#[test]
fn section1853_close_focus_smart_refusal_has_no_focus_or_later_focus_owner() {
    let (mut app, _, source) = reader(false);
    let ctx = egui::Context::default();
    let surface = app.fullscreen_viewport_id();
    assert!(
        frame(&ctx, egui::ViewportId::ROOT, surface, || {
            let nav = request_close(&mut app, &ctx);
            // Refuse the real Smart restore after dispatch resolved its destination.
            app.settings.smart_folders.clear();
            assert!(app.apply_fullscreen_close_nav_immediate_with_focus(&ctx, nav));
            assert!(
                app.take_pending_return_to_parent_nav_with_focus(&ctx)
                    .is_none()
            );
        })
        .is_empty()
    );
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.current_folder.as_ref(), Some(&source));
    assert!(app.smart_folder_transition.is_none());
    assert!(!app.fs_close_return_navigation_wait_active());
    assert!(
        frame(&ctx, egui::ViewportId::ROOT, surface, || {
            app.poll_smart_folder(&ctx);
        })
        .is_empty()
    );
}

#[test]
fn section1853_close_focus_smart_suffix_parent_classification_focuses_only_in_dispatch_frame() {
    use crate::app::ClassifiedOpenContinuation;
    for child in [false, true] {
        for ordinary_tail in [false, true] {
            let (mut app, _, source) = reader_with_parent(true, "parent.7z");
            let destination = source.parent().unwrap().to_path_buf();
            let ctx = egui::Context::default();
            let surface = app.fullscreen_viewport_id();
            let target = if child {
                surface
            } else {
                egui::ViewportId::ROOT
            };
            assert_eq!(
                frame(&ctx, target, surface, || {
                    let nav = request_close(&mut app, &ctx);
                    assert!(matches!(&nav, AddressBarNav::Direct(path, _) if path == &destination));
                    if ordinary_tail {
                        app.post_render_main_navigation(
                            &ctx,
                            super::super::update_phases::MainFrameNavigation {
                                fullscreen_close_nav: Some(nav),
                                ..Default::default()
                            },
                        );
                    } else {
                        assert!(app.apply_fullscreen_close_nav_immediate_with_focus(&ctx, nav));
                    }
                }),
                vec![target]
            );
            let candidate = app.top_level_grid_view.open_path_classification().unwrap();
            assert!(match (candidate.continuation.as_deref(), ordinary_tail) {
                (Some(ClassifiedOpenContinuation::Direct { .. }), false)
                | (Some(ClassifiedOpenContinuation::DirectNavigation { .. }), true) => true,
                _ => false,
            });
            assert!(
                candidate
                    .navigation
                    .as_ref()
                    .is_some_and(|nav| app.main_list_navigation_is_current(nav))
            );
            assert_eq!(app.fullscreen_idx, Some(0));
            assert!(!app.fs_close_return_navigation_wait_active());
            assert!(app.folder_pane_open_pending.is_none());
            let deadline = Instant::now() + Duration::from_secs(30);
            while app.top_level_grid_view.open_path_classification().is_some()
                || app.smart_folder_transition.is_some()
                || app.sidecar_restore_active()
            {
                assert!(
                    frame(&ctx, egui::ViewportId::ROOT, surface, || {
                        app.poll_open_path_classification(&ctx);
                        app.poll_smart_folder(&ctx);
                        app.finish_page_identity_preparation_for_test();
                        app.poll_sidecar_restore(&ctx);
                        assert!(!app.fs_close_return_navigation_wait_active());
                        assert!(app.folder_pane_open_pending.is_none());
                    })
                    .is_empty()
                );
                assert!(
                    Instant::now() < deadline,
                    "classified Smart parent did not adopt"
                );
                std::thread::yield_now();
            }
            assert_eq!(app.fullscreen_idx, None);
            assert_eq!(app.effective_folder(), Some(destination.clone()));
            assert!(
                matches!(&app.top_level_grid_view.smart_folder().unwrap().position,
                SmartFolderPosition::Scoped { current, .. } if current == &destination)
            );
        }
    }
}

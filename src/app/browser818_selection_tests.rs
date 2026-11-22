//! Headless handler coverage of the hydration/explicit-open ownership boundary.
use super::*;
use crate::app::external_open::{ViewerOpenIntent, ViewerOpenRequest};
use crate::app::tests::phase_c_support::{AppTestEnv, setup_app};

fn fixture() -> (AppTestEnv, PathBuf, Receiver<ThumbMsg>) {
    let mut app = setup_app();
    let folder = app.tmp.path().join("browser818-selection");
    std::fs::create_dir(&folder).unwrap();
    for name in ["a.png", "b.png"] {
        image::RgbaImage::new(2, 2).save(folder.join(name)).unwrap();
    }
    app.current_folder = Some(folder.clone());
    app.items = vec![
        GridItem::Image(folder.join("a.png")),
        GridItem::Image(folder.join("b.png")),
    ];
    app.thumbnails = vec![ThumbnailState::Pending; 2];
    app.image_metas = vec![None; 2];
    app.visible_indices = vec![0, 1];
    app.selected = Some(0);
    app.folder_history.insert(folder.clone(), (123.0, Some(0)));
    app.select_after_load = Some("a.png".into());
    app.settings.detached_viewer_enabled = false;
    app.settings.detached_viewer_open_images_in_window = false;
    app.page_identity_index = Arc::new(
        crate::app::file_type_resume::PreparedPageIdentityIndex::prepare(
            &app.items,
            None,
            app.settings.file_type_visibility.clone(),
        ),
    );
    for idx in 0..2 {
        app.adjustment_page_params
            .insert(idx, crate::adjustment::AdjustParams::default());
    }
    let rx = app.activate_live_sidecar_restore_for_test(folder.clone());
    let ContinuationOwner::Live(continuation) =
        &mut app.sidecar_restore.as_mut().unwrap().common.continuation
    else {
        panic!("live hydration");
    };
    continuation.detached_physical = false;
    continuation.restore_intent = crate::app::StartupListIntent::PageContinuation;
    (app, folder, rx)
}

fn overlay() -> ViewerOpenRequest {
    ViewerOpenRequest::new(ViewerOpenIntent::BrowserOverlay {
        placement: crate::detached_window_geometry::PhysicalWindowPlacement {
            outer: crate::detached_window_geometry::PhysicalRect {
                left: 10,
                top: 20,
                right: 810,
                bottom: 620,
            },
            maximized: false,
        },
    })
}

fn request_open(app: &mut App, idx: usize, request: ViewerOpenRequest) {
    let GridItem::Image(path) = &app.items[idx] else {
        panic!("physical image fixture");
    };
    let path = path.clone();
    assert_eq!(
        app.open_loaded_file_fullscreen_with_adoption(
            &path,
            "browser818_selection_test",
            ViewerOpenAdoption::requested(request),
        ),
        Ok(idx)
    );
    let deferred = app
        .sidecar_restore
        .as_ref()
        .unwrap()
        .common
        .deferred_fullscreen
        .as_ref()
        .unwrap();
    assert_eq!(deferred.viewer_adoption.request(), Some(request));
    assert!(deferred.viewer_adoption.owns_file_selection());
}

#[test]
fn browser818_selection_explicit_open_beats_history_and_name_hint() {
    for request in [ViewerOpenRequest::preferences(), overlay()] {
        let (mut app, _folder, _rx) = fixture();
        request_open(&mut app, 1, request);
        assert_eq!(app.fullscreen_idx, None, "hydration is deliberately held");
        assert_eq!(app.selected, Some(1));
        app.finish_page_identity_preparation_for_test();
        assert_eq!(app.fullscreen_idx, Some(1));
        assert_eq!(app.selected, Some(1));
        assert!(app.select_after_load.is_none());
    }
}

#[test]
fn browser818_selection_hydration_preserves_exact_selection_before_publication() {
    for history in [false, true] {
        let (mut app, folder, _rx) = fixture();
        if !history {
            app.folder_history.clear();
            app.select_after_load = None;
            app.book_resume_db
                .as_ref()
                .unwrap()
                .set(&folder, 0)
                .unwrap();
            assert_eq!(
                app.resume_page_for_container(),
                Some(0),
                "saved position is a real competing selection"
            );
        }
        request_open(&mut app, 1, overlay());
        let state = app.sidecar_restore.take().unwrap();
        let selection = app.sidecar_restore_explicit_selection(&state.common);
        assert_eq!(selection, Some(1));
        let continuation = state.common.continuation.take_live().unwrap();
        // Exercise the actual restore handler before its caller publishes DeferredFullscreen.
        // Final-open cursor alignment must not conceal a history/resume overwrite here.
        app.resume_loading_items_after_sidecar(
            continuation,
            &state.common.modal_observation,
            selection,
        );
        assert_eq!(app.fullscreen_idx, None);
        assert_eq!(app.selected, Some(1));
        assert!(app.select_after_load.is_none());
    }
}

#[test]
fn browser818_open_owner_index_only_restore_retains_overlay_and_selection() {
    let (mut app, _folder, _rx) = fixture();
    app.sidecar_restore.as_mut().unwrap().common.continuation = ContinuationOwner::IndexOnly {
        cancel: Arc::new(AtomicBool::new(false)),
        after_prepare: IndexOnlyAfterPrepare::None,
    };
    let request = overlay();
    request_open(&mut app, 1, request);
    assert_eq!(
        app.sidecar_restore
            .as_ref()
            .unwrap()
            .viewer_open_adoption_for_test()
            .unwrap()
            .request(),
        Some(request)
    );
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.selected, Some(1));
    assert_eq!(app.fullscreen_idx, Some(1));
}

#[test]
#[cfg(windows)]
fn browser818_selection_linked_f12_reopen_keeps_target_selection_and_sync_stamp() {
    let (mut app, _folder, _rx) = fixture();
    // An already-open linked F12 window precedes the second-instance request.
    let restore = app.sidecar_restore.take();
    app.settings.detached_viewer_enabled = true;
    app.open_fullscreen(0, HistoryTrigger::UserChosen);
    let window = app
        .detached_viewer_window_id()
        .expect("linked window allocated");
    app.sidecar_restore = restore;
    request_open(&mut app, 1, ViewerOpenRequest::preferences());
    app.finish_page_identity_preparation_for_test();
    let stamp = app
        .last_viewer_sync_stamp
        .clone()
        .expect("publication stamp");
    assert_eq!(stamp.idx, 1);
    assert_eq!(stamp.items_generation, app.items_generation);
    assert_eq!(app.selected, Some(1));
    assert_eq!(app.fullscreen_idx, Some(1));
    app.sync_detached_viewer_to_selected(&egui::Context::default());
    assert_eq!(app.last_viewer_sync_stamp, Some(stamp));
    assert_eq!(app.fullscreen_idx, Some(1));
    assert_eq!(app.detached_viewer_window_id(), Some(window));
}

#[test]
fn browser818_selection_same_file_replacement_keeps_new_request_and_cancel_retires_it() {
    let (mut app, folder, _rx) = fixture();
    let old = ViewerOpenRequest::preferences();
    let new = overlay();
    request_open(&mut app, 1, old);
    request_open(&mut app, 1, new);
    assert_ne!(old, new);
    let state = app.sidecar_restore.as_ref().unwrap();
    assert_eq!(
        state
            .common
            .deferred_fullscreen
            .as_ref()
            .unwrap()
            .viewer_adoption
            .request(),
        Some(new)
    );
    app.cancel_sidecar_restore_fullscreen_reopen();
    assert!(
        app.sidecar_restore
            .as_ref()
            .unwrap()
            .common
            .deferred_fullscreen
            .is_none()
    );
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.fullscreen_idx, None);
    assert_eq!(
        app.selected,
        Some(0),
        "cancelled request leaves ordinary history in charge"
    );
    assert_eq!(app.current_folder.as_ref(), Some(&folder));
}

#[test]
fn browser818_selection_close_retires_exact_open_without_cancelling_hydration() {
    let (mut app, _folder, _rx) = fixture();
    request_open(&mut app, 1, overlay());
    let cancel = Arc::clone(
        app.sidecar_restore
            .as_ref()
            .unwrap()
            .common
            .continuation
            .cancel(),
    );
    app.close_fullscreen();
    assert!(
        app.sidecar_restore
            .as_ref()
            .unwrap()
            .common
            .deferred_fullscreen
            .is_none()
    );
    assert!(!cancel.load(Ordering::Relaxed));
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.fullscreen_idx, None);
    assert_eq!(app.selected, Some(0));
}

#[test]
fn browser818_selection_close_retires_direct_media_hydration_action() {
    let (mut app, folder, _rx) = fixture();
    let action = MainListAfterHydration::DirectMedia {
        prepared: crate::app::startup_ops::DirectMediaPrepared::image_fixture_for_test(
            folder.join("hidden.png"),
        ),
        viewer_adoption: ViewerOpenAdoption::requested(overlay()).explicit_file(),
    };
    assert!(
        app.sidecar_restore
            .as_mut()
            .unwrap()
            .defer_main_list_action(action)
            .is_ok()
    );
    app.close_fullscreen();
    let ContinuationOwner::Live(continuation) =
        &app.sidecar_restore.as_ref().unwrap().common.continuation
    else {
        panic!("live");
    };
    assert!(continuation.navigation_after_hydration.is_none());
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.items.len(), 2);
    assert_eq!(app.fullscreen_idx, None);
    assert_eq!(app.selected, Some(0));
}

#[test]
fn browser818_open_owner_direct_media_rehydration_retains_overlay_and_exact_target() {
    let (mut app, folder, _rx) = fixture();
    let path = folder.join("hidden.png");
    image::RgbaImage::new(2, 2).save(&path).unwrap();
    app.settings
        .file_type_visibility
        .excluded_extensions
        .insert("png".into());
    let request = overlay();
    let action = MainListAfterHydration::DirectMedia {
        prepared: crate::app::startup_ops::DirectMediaPrepared::image_fixture_for_test(
            path.clone(),
        ),
        viewer_adoption: ViewerOpenAdoption::requested(request).explicit_file(),
    };
    assert!(
        app.sidecar_restore
            .as_mut()
            .unwrap()
            .defer_main_list_action(action)
            .is_ok()
    );
    app.poll_sidecar_restore(&egui::Context::default());
    let ContinuationOwner::Live(continuation) = &app
        .sidecar_restore
        .as_ref()
        .expect("reader rehydration owner")
        .common
        .continuation
    else {
        panic!("live");
    };
    let Some(MainListAfterHydration::RequiredFullscreen {
        viewer_adoption, ..
    }) = &continuation.navigation_after_hydration
    else {
        panic!("exact target continuation");
    };
    assert_eq!(viewer_adoption.request(), Some(request));
    assert!(viewer_adoption.owns_file_selection());
    app.finish_page_identity_preparation_for_test();
    let idx = app
        .items
        .iter()
        .position(|item| matches!(item, GridItem::Image(candidate) if candidate == &path))
        .unwrap();
    assert_eq!(app.selected, Some(idx));
    assert_eq!(app.fullscreen_idx, Some(idx));
}

#[test]
fn browser818_selection_replacement_retires_old_exact_hydration_action() {
    let (mut app, folder, _rx) = fixture();
    let old = ViewerOpenAdoption::requested(ViewerOpenRequest::preferences()).explicit_file();
    let state = app.sidecar_restore.as_mut().unwrap();
    assert!(
        state
            .defer_main_list_action(MainListAfterHydration::RequiredFullscreen {
                target: crate::snapshot::SnapshotTarget::Fs(folder.join("a.png")),
                history_trigger: HistoryTrigger::UserChosen,
                viewer_adoption: old,
            })
            .is_ok()
    );
    request_open(&mut app, 1, overlay());
    let ContinuationOwner::Live(continuation) =
        &app.sidecar_restore.as_ref().unwrap().common.continuation
    else {
        panic!("live");
    };
    assert!(continuation.navigation_after_hydration.is_none());
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.selected, Some(1));
    assert_eq!(app.fullscreen_idx, Some(1));
}

#[test]
fn browser818_selection_missing_target_relinquishes_selection_before_restore() {
    let (mut app, folder, _rx) = fixture();
    request_open(&mut app, 1, overlay());
    app.items[1] = GridItem::Image(folder.join("replacement.png"));
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.fullscreen_idx, None);
    assert_eq!(app.selected, Some(0));
}

#[test]
fn browser818_selection_stale_context_generation_folder_and_error_never_publish() {
    for reason in ["context", "generation", "folder", "error", "discard"] {
        let (mut app, folder, _rx) = fixture();
        request_open(&mut app, 1, overlay());
        match reason {
            "context" => {
                app.sidecar_restore.as_mut().unwrap().common.target_context =
                    ViewerContextId::for_test(999)
            }
            "generation" => app.items_generation += 1,
            "folder" => app.current_folder = Some(folder.join("elsewhere")),
            "discard" => {
                app.sidecar_restore
                    .as_mut()
                    .unwrap()
                    .common
                    .discard_target();
            }
            "error" => {
                let (tx, rx) = mpsc::channel();
                drop(tx);
                app.sidecar_restore.as_mut().unwrap().phase =
                    Phase::PreparingPageIndex(PreparingPageIndex {
                        rx,
                        next: PageIndexContinuation::ResumeDisplay,
                    });
            }
            _ => unreachable!(),
        }
        app.finish_page_identity_preparation_for_test();
        assert_eq!(app.fullscreen_idx, None, "{reason}");
        assert!(!app.sidecar_restore_active(), "{reason}");
    }
}

#[test]
#[cfg(windows)]
fn browser818_selection_restore_and_cancel_leave_sibling_resources_and_binding_unchanged() {
    for cancel in [false, true] {
        let (mut app, _folder, _rx) = fixture();
        let sibling = app.build_window_context_for_test(818_202, |owner| {
            owner.current_folder = Some(PathBuf::from("sibling"));
            owner.items = vec![GridItem::Image(PathBuf::from("sibling/a.png"))];
            owner.fs_cache.insert(818, FsCacheEntry::Failed);
        });
        let sender = app
            .with_viewer_context(sibling, |owner| owner.tx.clone())
            .unwrap();
        let message = || ThumbMsg {
            idx: 818,
            input_seq: 818,
            items_gen: 818,
            request_id: crate::thumb_loader::ThumbnailRequestId::Standalone,
            payload: crate::thumb_loader::ThumbMsgPayload::Canceled,
        };
        sender.send(message()).unwrap();
        let before = app
            .with_viewer_context(sibling, |owner| {
                (
                    owner.items.clone(),
                    owner.items_generation,
                    Arc::clone(&owner.cancel_token),
                    Arc::clone(&owner.page_identity_index),
                    owner.fs_cache.len(),
                )
            })
            .unwrap();
        let binding = app.viewer_context_window(sibling);
        request_open(&mut app, 1, overlay());
        if cancel {
            app.cancel_sidecar_restore_fullscreen_reopen();
        }
        app.finish_page_identity_preparation_for_test();
        assert_eq!(app.viewer_context_window(sibling), binding);
        app.with_viewer_context(sibling, |owner| {
            assert_eq!(owner.items, before.0);
            assert_eq!(owner.items_generation, before.1);
            assert!(Arc::ptr_eq(&owner.cancel_token, &before.2));
            assert!(!owner.cancel_token.load(Ordering::Relaxed));
            assert!(Arc::ptr_eq(&owner.page_identity_index, &before.3));
            assert_eq!(owner.fs_cache.len(), before.4);
            assert!(matches!(
                owner.fs_cache.get(&818),
                Some(FsCacheEntry::Failed)
            ));
            assert_eq!(
                owner.rx.try_recv().unwrap().idx,
                818,
                "queued sibling result was retained"
            );
            sender.send(message()).unwrap();
            assert_eq!(
                owner.rx.try_recv().unwrap().input_seq,
                818,
                "the sibling channel was not replaced"
            );
        })
        .unwrap();
    }
}

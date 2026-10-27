//! Runtime coverage for state-only access to the existing viewer payloads.

use super::tests::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use std::panic::{AssertUnwindSafe, catch_unwind};

const THREE_STARS: [bool; 6] = [false, false, false, true, false, false];

struct BoundaryTrace;

impl BoundaryTrace {
    fn start() -> Self {
        App::start_rating_boundary_trace_for_test();
        Self
    }

    fn take(&self) -> Vec<(&'static str, ViewerContextId)> {
        App::take_rating_boundary_trace_for_test()
    }
}

impl Drop for BoundaryTrace {
    fn drop(&mut self) {
        App::stop_rating_boundary_trace_for_test();
    }
}

pub(super) struct Fixture {
    pub(super) env: AppTestEnv,
    pub(super) main: ViewerContextId,
    pub(super) other: ViewerContextId,
    key: String,
    pub(super) old_stamp: crate::page_edit_write_epoch::WriteStamp,
    ready_stamp: crate::page_edit_write_epoch::WriteStamp,
}

fn seed_page(app: &mut App, path: &Path) {
    app.items = vec![GridItem::Image(path.to_path_buf())];
    app.thumbnails = vec![ThumbnailState::Pending];
    app.image_metas = vec![None];
    app.visible_indices = vec![0];
    app.selected = Some(0);
    app.rating_cache.insert(0, 3);
}

fn seed_ready_page_edit(
    app: &mut App,
    old_stamp: crate::page_edit_write_epoch::WriteStamp,
    ready_stamp: crate::page_edit_write_epoch::WriteStamp,
) {
    let order = Arc::new(page_edit_snapshot::PageKeyOrder::from_items(&app.items).unwrap());
    let snapshot = page_edit_snapshot::PageEditSnapshot {
        order: Arc::clone(&order),
        stamp: Some(old_stamp),
        ..Default::default()
    };
    let mut updated = snapshot.clone();
    updated.mask.insert(app.page_path_key(0).unwrap());
    let projection = updated.project(&app.items);
    let (sender, receiver) = mpsc::channel();
    assert!(
        sender
            .send(Ok(Some(page_edit_snapshot::ReconcileReady::Full(
                page_edit_snapshot::StablePageEditProjection {
                    snapshot: updated,
                    projection,
                    stamp: ready_stamp,
                },
            ))))
            .is_ok()
    );
    app.page_edit_snapshot = Some(snapshot);
    app.page_edit_reconcile_pending = Some(page_edit_snapshot::ReconcileStatus::Running(
        page_edit_snapshot::ReconcilePending {
            context_id: app.virtual_list_context_id(),
            items_generation: app.items_generation,
            page_edit_revision: app.page_edit_revision,
            order: Some(order),
            cancel: Arc::new(AtomicBool::new(false)),
            receiver,
            started_stamp: old_stamp,
            retry_attempt: 0,
        },
    ));
}

impl Fixture {
    pub(super) fn new() -> Self {
        let mut env = setup_app();
        env.settings.remember_favorite_view_state = true;
        env.settings.rating_filter = THREE_STARS;
        let path = env.tmp.path().join("shared-page.jpg");
        seed_page(&mut env, &path);
        let key = env.rating_path_key(0).unwrap();
        env.rating_db.as_ref().unwrap().set(&key, 3).unwrap();
        let a = crate::settings::FavoriteEntry::new("a".into(), env.tmp.path().join("a"));
        let b = crate::settings::FavoriteEntry::new("b".into(), env.tmp.path().join("b"));
        let mut a_state = crate::settings::FavoriteViewState::from_settings(&env.settings);
        a_state.grid_cols = 4;
        let mut b_state = a_state.clone();
        b_state.grid_cols = 7;
        env.favorite_view_states.insert(a.id, a_state);
        env.favorite_view_states.insert(b.id, b_state);
        env.settings.favorites.extend([a.clone(), b.clone()]);
        env.current_folder = Some(a.path.clone());
        env.transition_favorite_view_for_path(Some(&a.path));
        let main = env.viewer_context_main();
        let other = env
            .build_viewer_context("context_state_fixture", |app, _| {
                seed_page(app, &path);
                app.current_folder = Some(b.path.clone());
                app.transition_favorite_view_for_path(Some(&b.path));
                viewer_context_registry::BuildOutcome::Commit
            })
            .unwrap();
        let ctx = egui::Context::default();
        crate::page_edit_write_epoch::PAGE_EDIT_WRITES.register_repaint_context(&ctx);
        let old_stamp = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.sample();
        drop(crate::page_edit_write_epoch::PAGE_EDIT_WRITES.begin());
        let ready_stamp = crate::page_edit_write_epoch::PAGE_EDIT_WRITES.sample();
        for id in [main, other] {
            env.with_viewer_context_state(id, |app| {
                seed_ready_page_edit(app, old_stamp, ready_stamp);
            })
            .unwrap();
        }
        env.write_user_rating_shared_deferred(&key, 4, None)
            .unwrap();
        Self {
            env,
            main,
            other,
            key,
            old_stamp,
            ready_stamp,
        }
    }

    pub(super) fn assert_pending(
        app: &App,
        id: ViewerContextId,
        cols: usize,
        old_stamp: crate::page_edit_write_epoch::WriteStamp,
    ) {
        assert_eq!(app.projected_viewer_context_id(), id);
        assert_eq!(app.settings.grid_cols, cols);
        assert_eq!(app.effective_rating_filter(), THREE_STARS);
        assert_eq!(app.rating_cache.get(&0), Some(&3));
        assert!(app.rating_session_write_seen_generation < app.rating_session_write_generation);
        assert_eq!(app.visible_indices, vec![0]);
        assert_eq!(app.selected, Some(0));
        assert!(app.fullscreen_idx.is_none());
        assert_eq!(
            app.page_edit_snapshot.as_ref().unwrap().stamp,
            Some(old_stamp)
        );
        assert!(app.mask_pages.is_empty());
        assert!(matches!(
            app.page_edit_reconcile_pending,
            Some(page_edit_snapshot::ReconcileStatus::Running(_))
        ));
    }
}

#[test]
fn egui037_continuation_context_state_retains_rating_and_page_edit_until_ui_boundary() {
    let Fixture {
        mut env,
        main,
        other,
        key,
        old_stamp,
        ready_stamp,
    } = Fixture::new();
    let trace = BoundaryTrace::start();
    let publications = env.rating_publication_count;
    let generation = env.items_generation;
    Fixture::assert_pending(&env, main, 4, old_stamp);
    for id in [other, main, other] {
        env.with_viewer_context_state(id, |app| {
            Fixture::assert_pending(app, id, if id == main { 4 } else { 7 }, old_stamp);
        })
        .unwrap();
        Fixture::assert_pending(&env, main, 4, old_stamp);
    }
    assert!(trace.take().is_empty());
    assert_eq!(env.rating_publication_count, publications);
    assert_eq!(env.items_generation, generation);

    env.with_viewer_context(other, |app| {
        assert_eq!(app.rating_cache.get(&0), Some(&4));
        assert!(app.visible_indices.is_empty());
        assert!(app.selected.is_none());
        assert_eq!(
            app.page_edit_snapshot.as_ref().unwrap().stamp,
            Some(ready_stamp)
        );
        assert!(app.page_edit_snapshot.as_ref().unwrap().mask.contains(&key));
        assert!(app.mask_pages.contains(&0));
        assert!(app.page_edit_reconcile_pending.is_none());
    })
    .unwrap();
    assert_eq!(env.projected_viewer_context_id(), main);
    assert_eq!(env.settings.grid_cols, 4);
    assert_eq!(env.rating_cache.get(&0), Some(&4));
    assert!(env.visible_indices.is_empty());
    assert_eq!(
        env.page_edit_snapshot.as_ref().unwrap().stamp,
        Some(ready_stamp)
    );
    assert!(env.mask_pages.contains(&0));
    assert!(env.page_edit_reconcile_pending.is_none());
    assert_eq!(
        trace.take(),
        vec![
            ("mount_outgoing", main),
            ("mount_incoming", other),
            ("return_outgoing", other),
            ("return_incoming", main),
        ]
    );
    let publications = env.rating_publication_count;
    env.with_viewer_context(other, |_| ()).unwrap();
    assert_eq!(env.rating_publication_count, publications);
}

#[test]
fn egui037_continuation_context_state_nested_same_owner_and_unwind_restore_projection() {
    let Fixture {
        mut env,
        main,
        other,
        old_stamp,
        ..
    } = Fixture::new();
    let trace = BoundaryTrace::start();
    let publications = env.rating_publication_count;
    let outcome = catch_unwind(AssertUnwindSafe(|| {
        env.with_viewer_context_state(other, |app| {
            Fixture::assert_pending(app, other, 7, old_stamp);
            app.with_viewer_context_state(other, |app| {
                Fixture::assert_pending(app, other, 7, old_stamp);
            })
            .unwrap();
            app.with_viewer_context_state(main, |app| {
                Fixture::assert_pending(app, main, 4, old_stamp);
            })
            .unwrap();
            Fixture::assert_pending(app, other, 7, old_stamp);
            let nested = catch_unwind(AssertUnwindSafe(|| {
                app.with_viewer_context_state(main, |app| {
                    Fixture::assert_pending(app, main, 4, old_stamp);
                    panic!("state-only nested callback unwind");
                })
                .unwrap();
            }));
            assert!(nested.is_err());
            Fixture::assert_pending(app, other, 7, old_stamp);
            panic!("state-only outer callback unwind");
        })
        .unwrap();
    }));
    assert!(outcome.is_err());
    Fixture::assert_pending(&env, main, 4, old_stamp);
    env.with_viewer_context_state(other, |app| {
        Fixture::assert_pending(app, other, 7, old_stamp);
    })
    .unwrap();
    assert_eq!(env.rating_publication_count, publications);
    assert!(trace.take().is_empty());
}

#[test]
fn egui037_continuation_context_state_window_service_preserves_sibling_publications() {
    let Fixture {
        mut env,
        main,
        other,
        old_stamp,
        ..
    } = Fixture::new();
    env.bind_window(other, 39_137).unwrap();
    let trace = BoundaryTrace::start();
    let publications = env.rating_publication_count;
    env.with_window_viewer_context_state(39_137, |app| {
        Fixture::assert_pending(app, other, 7, old_stamp);
    })
    .unwrap();
    Fixture::assert_pending(&env, main, 4, old_stamp);
    assert_eq!(
        env.locate_window_context(39_137).map(|(id, _)| id),
        Some(other)
    );
    assert_eq!(env.rating_publication_count, publications);
    assert!(trace.take().is_empty());
}

#[test]
fn egui037_continuation_context_state_real_logic_preserves_unadopted_owner_results() {
    let Fixture {
        mut env,
        main,
        other,
        old_stamp,
        ..
    } = Fixture::new();
    let ctx = egui::Context::default();
    let mut integration = eframe::HeadlessEpiIntegration::new(ctx.clone());
    let trace = BoundaryTrace::start();
    let generation = env.items_generation;
    let publications = env.rating_publication_count;
    let original_events = vec![
        egui::Event::ModifiersChanged(egui::Modifiers::CTRL),
        egui::Event::WindowFocused(false),
    ];
    let mut pending = egui::RawInput {
        events: original_events.clone(),
        focused: false,
        ..Default::default()
    };
    for _ in 0..3 {
        let (_, returned) = integration.update_logic_only(&mut env.app, pending);
        pending = returned;
        assert_eq!(pending.viewport_id, egui::ViewportId::ROOT);
        assert_eq!(pending.events, original_events);
        assert_eq!(ctx.cumulative_frame_nr(), 0);
        Fixture::assert_pending(&env, main, 4, old_stamp);
        env.with_viewer_context_state(other, |app| {
            Fixture::assert_pending(app, other, 7, old_stamp);
        })
        .unwrap();
        assert_eq!(env.projected_viewer_context_id(), main);
        assert_eq!(env.items_generation, generation);
        assert_eq!(env.rating_publication_count, publications);
        assert!(
            trace.take().is_empty(),
            "real logic cannot cross a UI adoption boundary"
        );
    }
}

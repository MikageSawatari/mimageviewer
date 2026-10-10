//! D2 regressions exercise the accepted viewer owner and the real isolated writer.
use super::{spread_shift_owner::*, tests::phase_c_support::setup_app, *};
use crate::{book_scope::BookScopeKey, spread_shift::*};
use std::time::{Duration, Instant};

fn prepared(root: &Path, names: &[&str]) -> Arc<file_type_resume::PreparedPageIdentityIndex> {
    let items = names
        .iter()
        .map(|name| GridItem::Image(root.join(name)))
        .collect::<Vec<_>>();
    Arc::new(file_type_resume::PreparedPageIdentityIndex::prepare(
        &items,
        Some(root),
        Default::default(),
    ))
}

fn book_state(root: &Path, name: &str, index: usize) -> ViewerSpreadShift {
    let mut shift = ViewerSpreadShift::default();
    shift.scope = Some(BookScopeKey::folder(root).unwrap());
    shift.state = ViewerShift::Book(SpreadShiftState::Adjusted {
        anchor_ref: SpreadShiftPageRef::RelativePath(name.into()),
        resolution: ShiftResolution::Resolved(index),
        persistence: ShiftPersistence::SessionOnly,
    });
    shift
}

fn install_book(app: &mut App, root: &Path) {
    app.current_folder = Some(root.to_path_buf());
    app.items = (0..6)
        .map(|page| GridItem::Image(root.join(format!("{page}.png"))))
        .collect();
    app.thumbnails = vec![ThumbnailState::Pending; app.items.len()];
    app.image_metas = vec![None; app.items.len()];
    app.visible_indices = (0..app.items.len()).collect();
    app.page_identity_index = Arc::new(file_type_resume::PreparedPageIdentityIndex::prepare(
        &app.items,
        Some(root),
        app.settings.file_type_visibility.clone(),
    ));
    app.viewer_navigation_caches.invalidate();
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    app.spread_shift = ViewerSpreadShift::default();
    app.spread_shift.scope = app.current_spread_shift_scope();
    assert_eq!(
        app.spread_shift.scope,
        Some(BookScopeKey::folder(root).unwrap())
    );
}

fn runtime(app: &mut App, data: &Path) -> (SpreadShiftService, SpreadShiftProcessOwner) {
    let (service, owner) = SpreadShiftService::spawn_at(data.to_path_buf(), None).unwrap();
    app.spread_shift_service = Some(service.clone());
    (service, owner)
}

fn row(service: &SpreadShiftService, scope: &BookScopeKey) -> Option<SpreadShiftPageRef> {
    let reply = service
        .read_after(scope.clone(), service.watermarks().admitted)
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(reply.status.is_ok(), "{:?}", reply.status);
    reply.row
}

fn persistence(app: &App) -> ShiftPersistence {
    let ViewerShift::Book(SpreadShiftState::Adjusted { persistence, .. }) = &app.spread_shift.state
    else {
        panic!(
            "expected an adjusted book, got {:?}",
            app.spread_shift.state
        );
    };
    persistence.clone()
}

fn finish_restore(app: &mut App, restore: &mut ViewerBookRestore) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !app.poll_book_shift_restore(restore).unwrap() {
        assert!(Instant::now() < deadline, "book restore did not settle");
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[test]
fn identity_projection_follows_reorder_filter_missing_and_return_without_index_fallback() {
    let root = Path::new("C:/books/identity");
    let mut shift = book_state(root, "Anchor.PNG", 1);
    let original = prepared(root, &["before.png", "anchor.png", "after.png"]);
    let nav = Arc::new(vec![0, 1, 2]);
    assert!(!shift.resolve(&original, &nav));
    assert!(
        !shift.resolve(&original, &nav),
        "unchanged projection is reusable"
    );

    let reordered = prepared(root, &["new.png", "after.png", "before.png", "anchor.png"]);
    assert!(shift.resolve(&reordered, &Arc::new(vec![0, 1, 2, 3])));
    assert_eq!(shift.anchor_idx(), Some(3));
    assert!(shift.resolve(&reordered, &Arc::new(vec![0, 1, 2])));
    assert_eq!(shift.anchor_idx(), None, "a filtered anchor is dormant");
    assert!(shift.resolve(&reordered, &Arc::new(vec![0, 1, 2, 3])));
    assert_eq!(shift.anchor_idx(), Some(3));

    let missing = prepared(
        root,
        &["new.png", "after.png", "before.png", "different.png"],
    );
    assert!(shift.resolve(&missing, &Arc::new(vec![0, 1, 2, 3])));
    assert_eq!(
        shift.anchor_idx(),
        None,
        "same numeric slot is not an identity"
    );
    let returned = prepared(root, &["ANCHOR.png", "after.png"]);
    assert!(shift.resolve(&returned, &Arc::new(vec![0, 1])));
    assert_eq!(shift.anchor_idx(), Some(0));
    assert!(
        matches!(shift.state, ViewerShift::Book(SpreadShiftState::Adjusted {
        anchor_ref: SpreadShiftPageRef::RelativePath(ref name),
        persistence: ShiftPersistence::SessionOnly, ..
    }) if name == "Anchor.PNG")
    );
}

#[test]
fn integer_remap_moves_survivor_and_keeps_deleted_book_reference_dormant() {
    let root = Path::new("C:/books/remap");
    let original = prepared(root, &["zero.png", "one.png", "anchor.png", "last.png"]);
    let mut shift = book_state(root, "anchor.png", 2);
    shift.resolve(&original, &Arc::new(vec![0, 1, 2, 3]));
    shift.remap(&[0]);
    let surviving = Arc::new(original.without_removed_indices(&[0]));
    assert_eq!(shift.anchor_idx(), Some(1));
    assert!(!shift.resolve(&surviving, &Arc::new(vec![0, 1, 2])));
    shift.remap(&[1]);
    let removed = Arc::new(surviving.without_removed_indices(&[1]));
    assert_eq!(shift.anchor_idx(), None);
    shift.resolve(&removed, &Arc::new(vec![0, 1]));
    assert_eq!(shift.anchor_idx(), None);
    assert!(shift.resolve(
        &prepared(root, &["last.png", "anchor.png"]),
        &Arc::new(vec![0, 1])
    ));
    assert_eq!(shift.anchor_idx(), Some(1));

    let mut ordinary = ViewerSpreadShift::default();
    ordinary.state = ViewerShift::Ordinary(2);
    ordinary.remap(&[0]);
    assert_eq!(ordinary.anchor_idx(), Some(1));
    ordinary.remap(&[1]);
    assert_eq!(
        ordinary.state,
        ViewerShift::None,
        "ordinary lists have no retained book reference"
    );
}

#[test]
fn actual_row_removal_updates_identity_and_live_anchor_without_persistence_write() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/removal");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let (service, owner) = runtime(&mut env, &data);
    env.settings.remember_spread_shift_per_book = false;
    env.set_live_spread_shift(3);
    env.remove_items_batch(&[1, 0]);
    assert_eq!(env.spread_shift_anchor_idx(), Some(1));
    assert_eq!(env.items[1], GridItem::Image(root.join("3.png")));
    let nav = env.get_nav_indices();
    let index = env.page_identity_index.clone();
    env.spread_shift.resolve(&index, &nav);
    assert_eq!(env.spread_shift_anchor_idx(), Some(1));
    env.remove_items_batch(&[1]);
    let nav = env.get_nav_indices();
    let index = env.page_identity_index.clone();
    env.spread_shift.resolve(&index, &nav);
    assert_eq!(env.spread_shift_anchor_idx(), None);
    assert_eq!(service.watermarks().admitted, 0);
    owner.shutdown_and_join();
}

#[test]
fn memory_off_preserves_saved_row_and_live_shift_and_new_open_ignores_saved_row() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/off");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    env.set_live_spread_shift(1);
    assert_eq!(
        row(&service, &scope),
        Some(SpreadShiftPageRef::RelativePath("1.png".into()))
    );
    env.poll_spread_shift_service();
    let before = service.watermarks().admitted;
    env.spread_shift_memory_setting_changed(false);
    assert_eq!(env.spread_shift_anchor_idx(), Some(1));
    assert_eq!(persistence(&env), ShiftPersistence::SessionOnly);
    env.set_live_spread_shift(3);
    assert_eq!(env.spread_shift_anchor_idx(), Some(3));
    assert_eq!(service.watermarks().admitted, before);
    assert_eq!(
        row(&service, &scope),
        Some(SpreadShiftPageRef::RelativePath("1.png".into()))
    );
    let mut restore = env.begin_book_shift_restore(8).unwrap();
    finish_restore(&mut env, &mut restore);
    assert_eq!(env.spread_shift_anchor_idx(), None);
    assert_eq!(
        row(&service, &scope),
        Some(SpreadShiftPageRef::RelativePath("1.png".into()))
    );
    env.spread_shift_memory_setting_changed(true);
    assert_eq!(
        env.spread_shift_anchor_idx(),
        None,
        "ON does not restore an old value in place"
    );
    assert_eq!(service.watermarks().admitted, before);
    let mut restore = env.begin_book_shift_restore(9).unwrap();
    finish_restore(&mut env, &mut restore);
    assert_eq!(env.spread_shift_anchor_idx(), Some(1));
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
fn memory_on_saves_only_action_source_and_keeps_sibling_session_and_units() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/shared");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    env.settings.remember_spread_shift_per_book = false;
    env.set_live_spread_shift(1);
    let sibling = env.build_window_context_for_test(136_401, |app| {
        install_book(app, &root);
        app.set_live_spread_shift(3);
    });
    crate::ui_fullscreen::reset_spread_display_units_build_count_for_test();
    let sibling_units = env.with_window_context_for_test(sibling, |app| {
        app.spread_display_unit_pages_for_test(&[0, 1, 2, 3, 4, 5])
    });
    assert_eq!(
        service.watermarks().admitted,
        0,
        "build/mount/session nudge do not persist"
    );
    env.spread_shift_memory_setting_changed(true);
    assert_eq!(service.watermarks().admitted, 1);
    assert_eq!(
        row(&service, &scope),
        Some(SpreadShiftPageRef::RelativePath("1.png".into()))
    );
    env.poll_spread_shift_service();
    assert!(matches!(persistence(&env), ShiftPersistence::Saved(_)));
    env.with_window_context_for_test(sibling, |app| {
        assert_eq!(app.spread_shift_anchor_idx(), Some(3));
        assert_eq!(persistence(app), ShiftPersistence::SessionOnly);
        assert_eq!(
            app.spread_display_unit_pages_for_test(&[0, 1, 2, 3, 4, 5]),
            sibling_units
        );
    });
    assert_eq!(
        crate::ui_fullscreen::spread_display_units_build_count_for_test(),
        1,
        "saving the source does not invalidate the sibling's unchanged units"
    );
    env.retire_context(sibling, "shift_memory_source_test", |_| ())
        .unwrap();
    assert_eq!(
        service.watermarks().admitted,
        1,
        "closing sibling does not save its old session"
    );
    owner.shutdown_and_join();
}

#[test]
fn implicit_reset_off_preserves_saved_row_but_explicit_clear_off_deletes_it() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/reset-off");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    env.set_live_spread_shift(1);
    row(&service, &scope);
    env.poll_spread_shift_service();
    env.spread_shift_memory_setting_changed(false);
    let before = service.watermarks().admitted;
    env.apply_fullscreen_spread_mode(
        &egui::Context::default(),
        0,
        crate::settings::SpreadMode::Ltr,
    );
    assert_eq!(env.spread_shift.state, ViewerShift::None);
    assert_eq!(service.watermarks().admitted, before);
    assert_eq!(
        row(&service, &scope),
        Some(SpreadShiftPageRef::RelativePath("1.png".into()))
    );
    assert!(
        !env.reset_live_spread_shift(true),
        "saved-only clear need not have a live shift"
    );
    assert_eq!(service.watermarks().admitted, before + 1);
    assert_eq!(row(&service, &scope), None);
    owner.shutdown_and_join();
}

#[test]
fn q3_successful_mode_repress_mode_flow_and_direction_resets_clear_in_fifo_order() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/reset-on");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    let ctx = egui::Context::default();
    for operation in 0..4 {
        env.set_live_spread_shift(1);
        let before = service.watermarks().admitted;
        match operation {
            0 => env.apply_fullscreen_spread_mode(&ctx, 0, crate::settings::SpreadMode::Ltr),
            1 => env.apply_fullscreen_spread_mode(&ctx, 0, crate::settings::SpreadMode::LtrCover),
            2 => {
                env.set_reading_flow_for_fullscreen(&ctx, 0, crate::settings::ReadingFlow::Vertical)
            }
            _ => env.set_reading_direction_for_fullscreen(
                &ctx,
                0,
                crate::settings::ReadingDirection::Rtl,
            ),
        }
        assert_eq!(env.spread_shift.state, ViewerShift::None);
        assert_eq!(service.watermarks().admitted, before + 1);
        assert_eq!(row(&service, &scope), None);
        env.poll_spread_shift_service();
    }
    env.set_live_spread_shift(3);
    assert_eq!(
        row(&service, &scope),
        Some(SpreadShiftPageRef::RelativePath("3.png".into()))
    );
    owner.shutdown_and_join();
}

#[test]
fn explicit_clear_and_q3_resets_during_real_maintenance_keep_fifo_clear_and_notify_success() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/maintenance-reset");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    let ctx = egui::Context::default();
    for action in 0..5 {
        // Start with a durable older row to prove maintenance keeps the admitted
        // Set/Clear order rather than reporting failure or resurrecting that row.
        env.set_live_spread_shift(1);
        row(&service, &scope);
        env.poll_spread_shift_service();
        let lease = maintenance_at(&data).unwrap();
        let before = service.watermarks().admitted;
        env.fs_feedback_toast = None;
        env.set_live_spread_shift(3);
        let current_mode = env.spread_mode;
        match action {
            0 => {
                env.reset_live_spread_shift(true);
            }
            1 => env.apply_fullscreen_spread_mode(&ctx, 0, current_mode),
            2 => env.apply_fullscreen_spread_mode(&ctx, 0, crate::settings::SpreadMode::LtrCover),
            3 => {
                env.set_reading_flow_for_fullscreen(&ctx, 0, crate::settings::ReadingFlow::Vertical)
            }
            _ => env.set_reading_direction_for_fullscreen(
                &ctx,
                0,
                crate::settings::ReadingDirection::Rtl,
            ),
        }
        assert_eq!(env.spread_shift.state, ViewerShift::None);
        assert_eq!(service.watermarks().admitted, before + 2);
        assert!(
            env.fs_feedback_toast.is_none(),
            "lease admission is not a write failure"
        );
        drop(lease);
        assert_eq!(row(&service, &scope), None);
        env.poll_spread_shift_service();
        assert!(
            env.fs_feedback_toast
                .as_ref()
                .unwrap()
                .0
                .contains("解除しました")
        );
    }
    owner.shutdown_and_join();
}

#[test]
fn q3_failed_base_setter_does_not_clear_live_or_saved_shift() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/reset-failed");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    env.set_live_spread_shift(1);
    row(&service, &scope);
    env.poll_spread_shift_service();
    let before = service.watermarks().admitted;
    let state = env.spread_shift.clone();
    env.spread_db = Err("test base setter failure".into());
    let ctx = egui::Context::default();
    env.apply_fullscreen_spread_mode(&ctx, 0, crate::settings::SpreadMode::Rtl);
    env.set_reading_flow_for_fullscreen(&ctx, 0, crate::settings::ReadingFlow::Vertical);
    env.set_reading_direction_for_fullscreen(&ctx, 0, crate::settings::ReadingDirection::Rtl);
    assert_eq!(env.spread_shift, state);
    assert_eq!(service.watermarks().admitted, before);
    assert_eq!(
        row(&service, &scope),
        Some(SpreadShiftPageRef::RelativePath("1.png".into()))
    );
    owner.shutdown_and_join();
}

#[test]
fn pairing_gate_rejects_shift_and_clear_but_preserves_single_page_fallback() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/qualification");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let (service, owner) = runtime(&mut env, &data);
    env.settings.remember_spread_shift_per_book = false;
    env.set_live_spread_shift(1);
    for availability in [
        PairingEditAvailability::Composed,
        PairingEditAvailability::Unreadable,
        PairingEditAvailability::ExtractedResult,
    ] {
        env.spread_shift.availability = availability;
        env.spread_mode = crate::settings::SpreadMode::Ltr;
        let state = env.spread_shift.clone();
        let mut action = crate::ui_fullscreen::FsKeyAction::default();
        env.queue_spread_shift_action(2, 1, &mut action);
        assert!(action.page_nav.is_none());
        assert!(!env.set_live_spread_shift(3));
        assert!(!env.reset_live_spread_shift(true));
        assert_eq!(env.spread_shift, state);
        assert_eq!(service.watermarks().admitted, 0);
        assert_eq!(
            env.spread_shift_anchor_idx(),
            None,
            "dormant shift does not enter composed units"
        );

        env.spread_mode = crate::settings::SpreadMode::Single;
        env.queue_spread_shift_action(2, 1, &mut action);
        assert!(matches!(
            action.page_nav,
            crate::ui_fullscreen::FsPageNav::Delta(1)
        ));
        assert_eq!(env.spread_shift, state);
        assert_eq!(service.watermarks().admitted, 0);
    }
    owner.shutdown_and_join();
}

#[test]
fn nudge_saves_internal_anchor_instead_of_navigation_target_and_edge_is_noop() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/nudge");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    env.spread_mode = crate::settings::SpreadMode::LtrCover;
    assert_eq!(env.compute_spread_offset_nudge(2, 1), Some((2, 0)));
    let mut action = crate::ui_fullscreen::FsKeyAction::default();
    env.queue_spread_shift_action(2, 1, &mut action);
    assert!(matches!(
        action.page_nav,
        crate::ui_fullscreen::FsPageNav::Target(2)
    ));
    assert_eq!(env.spread_shift_anchor_idx(), Some(0));
    assert_eq!(
        row(&service, &scope),
        Some(SpreadShiftPageRef::RelativePath("0.png".into()))
    );
    let before = service.watermarks().admitted;
    let mut edge = crate::ui_fullscreen::FsKeyAction::default();
    env.queue_spread_shift_action(0, -1, &mut edge);
    assert!(edge.page_nav.is_none());
    assert_eq!(service.watermarks().admitted, before);
    owner.shutdown_and_join();
}

#[test]
fn stale_or_mismatched_save_reply_never_overwrites_latest_failure_or_moves_target() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/replies");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    let pause = service.pause_for_test();
    env.set_live_spread_shift(1);
    env.set_live_spread_shift(3);
    let latest = service.watermarks().admitted;
    assert_eq!(persistence(&env), ShiftPersistence::Saving(latest));
    pause.send(()).unwrap();
    row(&service, &scope);
    let replies = service.drain_results();
    assert_eq!(replies.len(), 2);
    assert_eq!(env.spread_shift.accept_write_reply(&replies[0], true), None);
    assert_eq!(persistence(&env), ShiftPersistence::Saving(latest));
    for mismatch in 0..3 {
        let mut reply = replies[1].clone();
        match mismatch {
            0 => reply.scope = BookScopeKey::folder(root.join("other")).unwrap(),
            1 => reply.operation.items_generation += 1,
            _ => reply.operation.context_id = ViewerContextId::for_test(999),
        }
        assert_eq!(env.spread_shift.accept_write_reply(&reply, true), None);
    }
    let mut failed = replies[1].clone();
    failed.status = Err("test write failure".into());
    assert_eq!(
        env.spread_shift.accept_write_reply(&failed, true),
        Some(false)
    );
    assert_eq!(persistence(&env), ShiftPersistence::SaveFailed);
    assert_eq!(env.spread_shift.accept_write_reply(&replies[0], true), None);
    assert_eq!(
        env.spread_shift.accept_write_reply(&replies[1], true),
        None,
        "reply is consumed once"
    );
    assert_eq!(persistence(&env), ShiftPersistence::SaveFailed);
    assert_eq!(env.spread_shift_anchor_idx(), Some(3));
    assert_eq!(env.fullscreen_idx, None);
    owner.shutdown_and_join();
}

#[test]
fn reply_after_memory_off_stays_session_only_and_clear_supersedes_pending_set() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/off-reply");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    let pause = service.pause_for_test();
    env.set_live_spread_shift(1);
    env.spread_shift_memory_setting_changed(false);
    pause.send(()).unwrap();
    row(&service, &scope);
    let reply = service.drain_results().pop().unwrap();
    assert_eq!(
        env.spread_shift.accept_write_reply(&reply, false),
        Some(false)
    );
    assert_eq!(persistence(&env), ShiftPersistence::SessionOnly);
    env.spread_shift_memory_setting_changed(true);
    env.reset_live_spread_shift(true);
    assert_eq!(row(&service, &scope), None);
    let replies = service.drain_results();
    assert_eq!(replies.len(), 2);
    assert_eq!(env.spread_shift.accept_write_reply(&replies[0], true), None);
    assert_eq!(
        env.spread_shift.accept_write_reply(&replies[1], true),
        Some(true)
    );
    assert_eq!(env.spread_shift.state, ViewerShift::None);
    owner.shutdown_and_join();
}

#[test]
fn restore_uses_fixed_fifo_read_and_rebuilds_preexisting_unit_cache() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/restore");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    let base = env.spread_display_unit_pages_for_test(&[0, 1, 2, 3, 4, 5]);
    assert_eq!(base, vec![vec![0, 1], vec![2, 3], vec![4, 5]]);
    let pause = service.pause_for_test();
    env.set_live_spread_shift(1);
    let mut restore = env.begin_book_shift_restore(20).unwrap();
    assert!(!env.poll_book_shift_restore(&mut restore).unwrap());
    // Later durable changes affect the next entry, not this already admitted read.
    service
        .set_shift(
            scope.clone(),
            SpreadShiftPageRef::RelativePath("3.png".into()),
            ShiftOperation {
                context_id: ViewerContextId::for_test(999),
                items_generation: 0,
                operation_sequence: 1,
            },
        )
        .unwrap();
    pause.send(()).unwrap();
    finish_restore(&mut env, &mut restore);
    assert_eq!(env.spread_shift_anchor_idx(), Some(1));
    let shifted = env.spread_display_unit_pages_for_test(&[0, 1, 2, 3, 4, 5]);
    assert_ne!(shifted, base);
    assert!(shifted.contains(&vec![1, 2]));
    assert_eq!(
        row(&service, &scope),
        Some(SpreadShiftPageRef::RelativePath("3.png".into()))
    );
    env.poll_spread_shift_service();
    assert_eq!(
        env.spread_shift_anchor_idx(),
        Some(1),
        "old operation replies do not replace adopted state"
    );
    let mut next = env.begin_book_shift_restore(21).unwrap();
    finish_restore(&mut env, &mut next);
    assert_eq!(env.spread_shift_anchor_idx(), Some(3));
    owner.shutdown_and_join();
}

#[test]
fn superseded_restore_rejects_changed_items_settings_or_book_scope() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/stale-restore");
    install_book(&mut env, &root);
    for mismatch in 0..3 {
        install_book(&mut env, &root);
        let mut restore = env.begin_book_shift_restore(30 + mismatch).unwrap();
        match mismatch {
            0 => env.items_generation += 1,
            1 => env.spread_shift_memory_setting_changed(false),
            _ => env.current_folder = Some(env.tmp.path().join("books/other")),
        }
        assert!(env.poll_book_shift_restore(&mut restore).is_err());
        assert_eq!(env.spread_shift.state, ViewerShift::None);
    }
}

#[test]
fn retained_readoption_uses_latest_saved_pair_for_live_page_without_resume_read_or_write() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/retained-on");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    env.book_resume_writer = crate::book_resume_db::BookResumeWriter::spawn_at(
        env.tmp.path().join("retained-resume.db"),
    );
    let resume_pause = env.book_resume_writer.as_ref().unwrap().pause_for_test();
    assert!(
        env.book_resume_writer
            .as_ref()
            .unwrap()
            .record(&root, 5, None)
    );
    let resume_before = env.book_resume_writer.as_ref().unwrap().watermarks();
    env.settings.remember_spread_shift_per_book = false;
    env.set_live_spread_shift(0);
    service
        .set_shift(
            scope.clone(),
            SpreadShiftPageRef::RelativePath("1.png".into()),
            ShiftOperation {
                context_id: ViewerContextId::for_test(999),
                items_generation: 0,
                operation_sequence: 1,
            },
        )
        .unwrap();
    row(&service, &scope);
    env.settings.remember_spread_shift_per_book = true;
    env.fullscreen_idx = Some(2);
    let mut restore = env.capture_book_shift_restore(40, true).unwrap();
    finish_restore(&mut env, &mut restore);
    assert_eq!(env.spread_shift_anchor_idx(), Some(1));
    assert_eq!(
        env.fullscreen_idx,
        Some(1),
        "the restored unit still contains live page 2"
    );
    assert_eq!(env.selected, Some(1));
    assert_eq!(env.fenced_book_resume(&root), None);
    assert_eq!(
        env.book_resume_writer.as_ref().unwrap().watermarks(),
        resume_before
    );

    let shift_pause = service.pause_for_test();
    service
        .set_shift(
            scope.clone(),
            SpreadShiftPageRef::RelativePath("3.png".into()),
            ShiftOperation {
                context_id: ViewerContextId::for_test(999),
                items_generation: 0,
                operation_sequence: 2,
            },
        )
        .unwrap();
    env.fullscreen_idx = Some(4);
    env.begin_retained_shift_readoption(&egui::Context::default());
    assert!(
        env.sidecar_restore_active(),
        "pending shift uses the existing first-display gate"
    );
    assert_eq!(env.fullscreen_idx, Some(4));
    shift_pause.send(()).unwrap();
    env.finish_page_identity_preparation_for_test();
    assert_eq!(env.spread_shift_anchor_idx(), Some(3));
    assert_eq!(
        env.fullscreen_idx,
        Some(3),
        "live page 4 remains in the accepted unit"
    );
    assert_eq!(
        env.book_resume_writer.as_ref().unwrap().watermarks(),
        resume_before
    );
    resume_pause.send(()).unwrap();
    let saved_resume = env
        .book_resume_writer
        .as_ref()
        .unwrap()
        .read_position_after(&crate::path_key::normalize(&root), resume_before.admitted)
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert_eq!(
        saved_resume.position.unwrap().page,
        5,
        "readoption does not publish a background resume"
    );
    owner.shutdown_and_join();
}

#[test]
fn retained_readoption_off_keeps_session_and_does_not_wait_for_saved_read() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/retained-off");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let (service, owner) = runtime(&mut env, &data);
    env.settings.remember_spread_shift_per_book = false;
    env.set_live_spread_shift(1);
    env.fullscreen_idx = Some(2);
    let before = env.spread_shift.clone();
    let pause = service.pause_for_test();
    env.begin_retained_shift_readoption(&egui::Context::default());
    assert!(!env.sidecar_restore_active());
    assert_eq!(env.spread_shift, before);
    assert_eq!(env.fullscreen_idx, Some(2));
    assert_eq!(service.watermarks().admitted, 0);
    pause.send(()).unwrap();
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
fn book_rename_saved_rows_move_but_open_scopes_and_stale_actions_stay_old() {
    let mut env = setup_app();
    let books = env.tmp.path().join("books");
    let source = books.join("Old");
    let target = books.join("New");
    let unrelated = books.join("Old-sibling");
    std::fs::create_dir_all(&source).unwrap();
    for page in 0..6 {
        std::fs::write(source.join(format!("{page}.png")), b"page").unwrap();
    }
    env.settings.book_root = Some(books);
    env.settings.sidecar_backup_enabled = false;
    env.settings.remember_spread_shift_per_book = false;
    install_book(&mut env, &source);
    assert!(env.set_live_spread_shift(3));
    let main_shift = env.spread_shift.clone();
    let main_index = Arc::clone(&env.page_identity_index);
    let main_generation = env.items_generation;
    let main_cancel = Arc::clone(&env.cancel_token);
    let stale = env.build_window_context_for_test(136_421, |app| {
        install_book(app, &source);
        assert!(app.set_live_spread_shift(0));
        app.fullscreen_idx = Some(2);
    });
    let sibling = env.build_window_context_for_test(136_422, |app| {
        install_book(app, &unrelated);
        assert!(app.set_live_spread_shift(3));
    });
    let data = env.tmp.path().join("rename-shifts");
    let (service, owner) = runtime(&mut env, &data);
    env.settings.remember_spread_shift_per_book = true;
    let old_scope = BookScopeKey::folder(&source).unwrap();
    let new_scope = BookScopeKey::folder(&target).unwrap();
    let saved = SpreadShiftPageRef::RelativePath("1.png".into());
    service
        .set_shift(
            old_scope.clone(),
            saved.clone(),
            ShiftOperation {
                context_id: stale,
                items_generation: 0,
                operation_sequence: 1,
            },
        )
        .unwrap();
    assert_eq!(row(&service, &old_scope), Some(saved.clone()));
    let ctx = egui::Context::default();
    env.start_book_rename(&ctx, "Old".into(), "New".into());
    finish_test_book_rename(&mut env, &ctx);
    assert!(!source.exists());
    assert!(target.exists());
    assert!(
        env.pending_reload,
        "released main-list reload is still requested"
    );
    assert_eq!(row(&service, &old_scope), None);
    assert_eq!(row(&service, &new_scope), Some(saved.clone()));
    assert_eq!(env.current_folder.as_deref(), Some(source.as_path()));
    assert_eq!(env.spread_shift, main_shift);
    assert!(Arc::ptr_eq(&env.page_identity_index, &main_index));
    assert_eq!(env.items_generation, main_generation);
    assert!(Arc::ptr_eq(&env.cancel_token, &main_cancel));
    env.with_window_context_for_test(stale, |app| {
        assert_eq!(app.current_folder.as_deref(), Some(source.as_path()));
        assert_eq!(app.current_spread_shift_scope(), Some(old_scope.clone()));
        assert_eq!(app.spread_shift.scope, Some(old_scope.clone()));
        assert_eq!(app.items[2], GridItem::Image(source.join("2.png")));
        assert_eq!(app.spread_shift_anchor_idx(), Some(0));
        assert_eq!(app.fullscreen_idx, Some(2));
        // A stale-view operation retains the old key and cannot alter the new row.
        assert!(app.set_live_spread_shift(5));
    });
    assert_eq!(
        row(&service, &old_scope),
        Some(SpreadShiftPageRef::RelativePath("5.png".into()))
    );
    assert_eq!(row(&service, &new_scope), Some(saved.clone()));
    env.with_window_context_for_test(stale, |app| {
        assert!(app.reset_live_spread_shift(true));
    });
    assert_eq!(row(&service, &old_scope), None);
    assert_eq!(row(&service, &new_scope), Some(saved));
    env.with_window_context_for_test(sibling, |app| {
        assert_eq!(app.current_folder.as_deref(), Some(unrelated.as_path()));
        assert_eq!(app.spread_shift_anchor_idx(), Some(3));
    });
    // Consume the existing main-list request: its target remains the obsolete folder.
    env.poll_pending_reload(&ctx);
    assert!(!env.pending_reload);
    assert_eq!(env.current_folder.as_deref(), Some(source.as_path()));
    assert_ne!(env.current_folder.as_deref(), Some(target.as_path()));
    env.retire_context(stale, "rename_stale_test", |_| ())
        .unwrap();
    env.retire_context(sibling, "rename_sibling_test", |_| ())
        .unwrap();
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
fn imported_read_applies_only_current_memory_on_scope_and_preserves_sibling() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/imported");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    env.settings.remember_spread_shift_per_book = false;
    env.set_live_spread_shift(1);
    let sibling = env.build_window_context_for_test(136_403, |app| {
        install_book(app, &root);
        app.set_live_spread_shift(5);
    });
    assert!(env.admit_imported_shift_read(true).is_none());
    service
        .set_shift(
            scope.clone(),
            SpreadShiftPageRef::RelativePath("3.png".into()),
            ShiftOperation {
                context_id: ViewerContextId::for_test(999),
                items_generation: 0,
                operation_sequence: 1,
            },
        )
        .unwrap();
    row(&service, &scope);
    env.settings.remember_spread_shift_per_book = true;
    assert!(env.admit_imported_shift_read(false).is_none());
    let (captured_scope, rx) = env.admit_imported_shift_read(true).unwrap();
    let read = rx.recv_timeout(Duration::from_secs(10)).unwrap();
    assert_eq!(read.status, Ok(()));
    let before = env.spread_shift.clone();
    env.current_folder = Some(env.tmp.path().join("books/other-import"));
    env.adopt_imported_shift_read(captured_scope.clone(), read.clone());
    assert_eq!(
        env.spread_shift, before,
        "changed scope rejects a completed import read"
    );
    env.current_folder = Some(root.clone());
    env.settings.remember_spread_shift_per_book = false;
    env.adopt_imported_shift_read(captured_scope.clone(), read.clone());
    assert_eq!(env.spread_shift, before, "OFF preserves current session");
    env.settings.remember_spread_shift_per_book = true;
    env.fullscreen_idx = Some(4);
    env.adopt_imported_shift_read(captured_scope, read);
    assert_eq!(env.spread_shift_anchor_idx(), Some(3));
    assert_eq!(env.fullscreen_idx, Some(3));
    env.with_window_context_for_test(sibling, |app| {
        assert_eq!(app.spread_shift_anchor_idx(), Some(5));
        assert_eq!(persistence(app), ShiftPersistence::SessionOnly);
    });
    assert_eq!(
        service.watermarks().admitted,
        1,
        "read/adoption never writes"
    );
    owner.shutdown_and_join();
}

#[test]
fn notification_save_and_clear_wait_for_poll_while_off_session_is_immediate() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/notifications");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    env.fs_feedback_toast = None;
    env.set_live_spread_shift(1);
    assert!(env.fs_feedback_toast.is_none());
    row(&service, &scope);
    assert!(
        env.fs_feedback_toast.is_none(),
        "commit alone does not claim success before reply dispatch"
    );
    env.poll_spread_shift_service();
    assert!(
        env.fs_feedback_toast
            .as_ref()
            .unwrap()
            .0
            .contains("見開き調整を保存しました")
    );
    env.fs_feedback_toast = None;
    env.reset_live_spread_shift(true);
    assert!(env.fs_feedback_toast.is_none());
    assert_eq!(row(&service, &scope), None);
    env.poll_spread_shift_service();
    assert!(
        env.fs_feedback_toast
            .as_ref()
            .unwrap()
            .0
            .contains("解除しました")
    );
    env.spread_shift_memory_setting_changed(false);
    env.set_live_spread_shift(3);
    assert!(
        env.fs_feedback_toast
            .as_ref()
            .unwrap()
            .0
            .contains("この閲覧中のみ")
    );
    owner.shutdown_and_join();
}

#[test]
fn obsolete_success_notification_does_not_replace_latest_failure_toast() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/stale-notification");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    env.set_live_spread_shift(1);
    env.set_live_spread_shift(3);
    row(&service, &scope);
    let failure = ShiftWriteReply {
        sequence: service.watermarks().admitted,
        scope,
        kind: ShiftWriteKind::Set,
        operation: ShiftOperation {
            context_id: env.spread_shift_context_id(),
            items_generation: env.items_generation,
            operation_sequence: 2,
        },
        changed: false,
        status: Err("latest test failure".into()),
    };
    assert_eq!(
        env.spread_shift.accept_write_reply(&failure, true),
        Some(false)
    );
    env.show_feedback_toast("最新の保存に失敗しました".into());
    env.poll_spread_shift_service();
    assert_eq!(
        env.fs_feedback_toast.as_ref().unwrap().0,
        "最新の保存に失敗しました"
    );
    assert_eq!(persistence(&env), ShiftPersistence::SaveFailed);
    assert_eq!(env.spread_shift_anchor_idx(), Some(3));
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
fn retained_sibling_fence_is_independent_of_other_contexts_pending_global_checking() {
    use super::sidecar_restore::{
        FirstDisplayAction, SidecarCheckingRelay, SidecarLoadContinuation,
    };
    let mut env = setup_app();
    let root = env.tmp.path().join("books/global-checking");
    let other = env.tmp.path().join("books/independent-retained");
    let data = env.tmp.path().join("shift-data");
    std::fs::create_dir_all(&root).unwrap();
    install_book(&mut env, &root);
    let (service, owner) = runtime(&mut env, &data);
    let main_context = env.spread_shift_context_id();
    env.settings.sidecar_backup_enabled = true;
    let now = Instant::now();
    let continuation = SidecarLoadContinuation {
        navigation_after_hydration: None,
        audio_art_prune: None,
        first_display_action: FirstDisplayAction::None,
        restore_intent: StartupListIntent::ExplicitList,
        auto_aspect_seed: None,
        source_path: root.clone(),
        source_is_directory: true,
        prepared_subfolder: None,
        prepared_page_edits: None,
        prepared_aggregate: None,
        catalog_existing_keys: Default::default(),
        video_items: Vec::new(),
        sli_seq: 0,
        sli_t0: now,
        items_len: env.items.len(),
        detached_physical: false,
        tx: env.tx.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
        restore_started_at: now,
    };
    assert!(env.begin_sidecar_restore(continuation, false).is_ok());
    let relay = SidecarCheckingRelay::new();
    env.attach_sidecar_checking_relay_for_test(relay.clone());
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(10);
    while !relay.captured() {
        assert!(
            Instant::now() < deadline,
            "global checking worker did not settle"
        );
        env.poll_sidecar_restore(&ctx);
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(
        env.sidecar_restore.as_ref().unwrap().target_context(),
        main_context
    );
    assert!(env.sidecar_restore_blocks_projected_context());
    let main_shift = env.spread_shift.clone();
    for (window, retained_root) in [(136_404, &root), (136_405, &other)] {
        let sibling = env.build_window_context_for_test(window, |app| {
            install_book(app, retained_root);
            app.settings.remember_spread_shift_per_book = false;
            app.set_live_spread_shift(0);
            app.settings.remember_spread_shift_per_book = true;
            app.fullscreen_idx = Some(2);
        });
        let scope = BookScopeKey::folder(retained_root).unwrap();
        let pause = service.pause_for_test();
        service
            .set_shift(
                scope.clone(),
                SpreadShiftPageRef::RelativePath("1.png".into()),
                ShiftOperation {
                    context_id: ViewerContextId::for_test(999),
                    items_generation: 0,
                    operation_sequence: window,
                },
            )
            .unwrap();
        env.with_window_context_for_test(sibling, |app| {
            assert!(
                !app.sidecar_restore_blocks_projected_context(),
                "global checking belongs to main"
            );
            app.begin_retained_shift_readoption(&ctx);
            assert!(app.spread_shift.retained_readoption_pending());
            assert!(
                app.sidecar_restore_blocks_projected_context(),
                "this retained context owns its own first-unit fence"
            );
            assert_eq!(
                app.sidecar_restore.as_ref().unwrap().target_context(),
                main_context
            );
            let fork_copy = app.spread_shift.fork_accepted();
            assert!(
                !fork_copy.retained_readoption_pending(),
                "accepted state clone never shares a receiver"
            );
            assert_eq!(fork_copy.anchor_idx(), Some(0));
            assert_eq!(app.fullscreen_idx, Some(2));
        });
        assert_eq!(env.spread_shift, main_shift);
        pause.send(()).unwrap();
        assert_eq!(
            row(&service, &scope),
            Some(SpreadShiftPageRef::RelativePath("1.png".into()))
        );
        env.with_window_context_for_test(sibling, |app| {
            app.poll_mounted_document_open_owners(&ctx);
            assert!(!app.spread_shift.retained_readoption_pending());
            assert!(!app.sidecar_restore_blocks_projected_context());
            assert_eq!(app.spread_shift_anchor_idx(), Some(1));
            assert_eq!(app.fullscreen_idx, Some(1));
            assert_eq!(
                app.sidecar_restore.as_ref().unwrap().target_context(),
                main_context
            );
        });
        assert_eq!(env.spread_shift, main_shift);
        assert!(relay.captured());
        assert!(
            env.sidecar_restore.is_some(),
            "global checking was neither replaced nor consumed"
        );
        env.retire_context(sibling, "retained_independent_global_checking_test", |_| ())
            .unwrap();
    }
    assert_eq!(
        service.watermarks().admitted,
        2,
        "readoption/mount/close create no writes"
    );
    relay.release();
    env.finish_page_identity_preparation_for_test();
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
fn parked_retained_read_is_cancelled_and_reactivation_reads_fresh_cutoff() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/parked-read");
    let main = env.tmp.path().join("books/unchanged-main");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &main);
    let (service, owner) = runtime(&mut env, &data);
    env.settings.remember_spread_shift_per_book = false;
    env.set_live_spread_shift(0);
    env.settings.remember_spread_shift_per_book = true;
    let main_shift = env.spread_shift.clone();
    let sibling = env.build_window_context_for_test(136_406, |app| {
        install_book(app, &root);
        app.settings.remember_spread_shift_per_book = false;
        app.set_live_spread_shift(1);
        app.settings.remember_spread_shift_per_book = true;
    });
    let scope = BookScopeKey::folder(&root).unwrap();
    let operation = |sequence| ShiftOperation {
        context_id: ViewerContextId::for_test(999),
        items_generation: 0,
        operation_sequence: sequence,
    };
    let ctx = egui::Context::default();
    let pause = service.pause_for_test();
    service
        .set_shift(
            scope.clone(),
            SpreadShiftPageRef::RelativePath("3.png".into()),
            operation(1),
        )
        .unwrap();
    env.with_window_context_for_test(sibling, |app| {
        app.begin_retained_shift_readoption(&ctx);
        assert!(app.spread_shift.retained_readoption_pending());
        assert!(
            viewer_context_registry::ContextAsyncOwner::RetainedSpreadShift
                .is_pending(viewer_context_registry::ContextRef::mounted(app))
        );
        app.pause_mounted_background_work_keep_current_frame();
        assert!(!app.spread_shift.retained_readoption_pending());
        assert!(!app.sidecar_restore_blocks_projected_context());
        assert_eq!(app.spread_shift_anchor_idx(), Some(1));
    });
    service
        .set_shift(
            scope.clone(),
            SpreadShiftPageRef::RelativePath("5.png".into()),
            operation(2),
        )
        .unwrap();
    pause.send(()).unwrap();
    assert_eq!(
        row(&service, &scope),
        Some(SpreadShiftPageRef::RelativePath("5.png".into()))
    );
    env.with_window_context_for_test(sibling, |app| {
        app.poll_mounted_document_open_owners(&ctx);
        assert_eq!(
            app.spread_shift_anchor_idx(),
            Some(1),
            "parked old read cannot adopt after completion"
        );
        app.begin_retained_shift_readoption(&ctx);
        assert!(!app.spread_shift.retained_readoption_pending());
        assert_eq!(
            app.spread_shift_anchor_idx(),
            Some(5),
            "reactivation captures the later admitted cutoff"
        );
    });
    assert_eq!(env.spread_shift, main_shift);
    let pause = service.pause_for_test();
    service
        .set_shift(
            scope.clone(),
            SpreadShiftPageRef::RelativePath("3.png".into()),
            operation(3),
        )
        .unwrap();
    env.with_window_context_for_test(sibling, |app| {
        app.begin_retained_shift_readoption(&ctx);
        assert!(app.spread_shift.retained_readoption_pending());
    });
    env.retire_context(sibling, "retained_read_drop_test", |_| ())
        .unwrap();
    pause.send(()).unwrap();
    row(&service, &scope);
    env.poll_mounted_document_open_owners(&ctx);
    assert_eq!(env.spread_shift, main_shift);
    assert_eq!(
        service.watermarks().admitted,
        3,
        "park/drop/reopen never save a session"
    );
    owner.shutdown_and_join();
}

#[test]
fn session_only_cancels_retained_read_without_changing_accepted_session() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/retained-policy");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    env.settings.remember_spread_shift_per_book = false;
    env.set_live_spread_shift(1);
    env.settings.remember_spread_shift_per_book = true;
    let accepted = env.spread_shift.state.clone();
    let pause = service.pause_for_test();
    service
        .set_shift(
            scope.clone(),
            SpreadShiftPageRef::RelativePath("3.png".into()),
            ShiftOperation {
                context_id: ViewerContextId::for_test(999),
                items_generation: 0,
                operation_sequence: 1,
            },
        )
        .unwrap();
    let ctx = egui::Context::default();
    env.begin_retained_shift_readoption(&ctx);
    assert!(env.spread_shift.retained_readoption_pending());
    env.spread_shift.session_only();
    assert!(!env.spread_shift.retained_readoption_pending());
    assert_eq!(env.spread_shift.state, accepted);
    pause.send(()).unwrap();
    row(&service, &scope);
    env.poll_retained_shift_readoption(&ctx);
    assert_eq!(
        env.spread_shift.state, accepted,
        "the superseded read cannot replace the shown session"
    );
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
fn sibling_memory_toggle_refreshes_fresh_common_policy_and_keeps_original_resume_fence() {
    use super::sidecar_restore::{FirstDisplayAction, SidecarLoadContinuation};
    let mut env = setup_app();
    let root = env.tmp.path().join("books/fresh-policy");
    let sibling_root = env.tmp.path().join("books/policy-source");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    let sibling = env.build_window_context_for_test(136_407, |app| {
        install_book(app, &sibling_root);
        app.settings.remember_spread_shift_per_book = false;
        app.set_live_spread_shift(1);
        app.settings.remember_spread_shift_per_book = true;
    });
    let sibling_reader = env.build_window_context_for_test(136_409, |app| {
        install_book(app, &root);
    });
    env.book_resume_writer = crate::book_resume_db::BookResumeWriter::spawn_at(
        env.tmp.path().join("fresh-policy-resume.db"),
    );
    let resume_pause = env.book_resume_writer.as_ref().unwrap().pause_for_test();
    assert!(
        env.book_resume_writer
            .as_ref()
            .unwrap()
            .record(&root, 2, None)
    );
    let resume_cutoff = env
        .book_resume_writer
        .as_ref()
        .unwrap()
        .watermarks()
        .admitted;
    let shift_pause = service.pause_for_test();
    service
        .set_shift(
            scope.clone(),
            SpreadShiftPageRef::RelativePath("3.png".into()),
            ShiftOperation {
                context_id: ViewerContextId::for_test(999),
                items_generation: 0,
                operation_sequence: 1,
            },
        )
        .unwrap();
    let now = Instant::now();
    let continuation = SidecarLoadContinuation {
        navigation_after_hydration: None,
        audio_art_prune: None,
        first_display_action: FirstDisplayAction::None,
        restore_intent: StartupListIntent::ExplicitList,
        auto_aspect_seed: None,
        source_path: root.clone(),
        source_is_directory: true,
        prepared_subfolder: None,
        prepared_page_edits: None,
        prepared_aggregate: None,
        catalog_existing_keys: Default::default(),
        video_items: Vec::new(),
        sli_seq: 0,
        sli_t0: now,
        items_len: env.items.len(),
        detached_physical: false,
        tx: env.tx.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
        restore_started_at: now,
    };
    assert!(env.begin_sidecar_restore(continuation, false).is_ok());
    let main_context = env.spread_shift_context_id();
    env.with_window_context_for_test(sibling_reader, |app| {
        assert!(!app.sidecar_restore_blocks_projected_context());
        // The fresh-open resume read is already queued before this sibling Record.
        // Policy changes must preserve that exact read, not reread its old cutoff.
        app.record_book_resume(4);
    });
    env.with_window_context_for_test(sibling, |app| {
        assert!(!app.sidecar_restore_blocks_projected_context());
        app.spread_shift_memory_setting_changed(false);
        app.spread_shift_memory_setting_changed(true);
        app.spread_shift_memory_setting_changed(false);
        assert_eq!(app.spread_shift_anchor_idx(), Some(1));
        assert_eq!(
            app.sidecar_restore.as_ref().unwrap().target_context(),
            main_context
        );
    });
    shift_pause.send(()).unwrap();
    assert_eq!(
        row(&service, &scope),
        Some(SpreadShiftPageRef::RelativePath("3.png".into()))
    );
    let ctx = egui::Context::default();
    env.poll_sidecar_restore(&ctx);
    assert!(
        env.sidecar_restore.is_some(),
        "OFF removes the shift read but cannot bypass the originally admitted resume"
    );
    assert!(env.sidecar_restore_blocks_projected_context());
    assert_eq!(env.spread_shift_anchor_idx(), None);
    assert_eq!(
        env.book_resume_writer
            .as_ref()
            .unwrap()
            .watermarks()
            .admitted,
        resume_cutoff + 1
    );
    resume_pause.send(()).unwrap();
    env.finish_page_identity_preparation_for_test();
    assert_eq!(env.spread_shift_anchor_idx(), None);
    assert_eq!(
        env.spread_display_unit_pages_for_test(&[0, 1, 2, 3, 4, 5]),
        vec![vec![0, 1], vec![2, 3], vec![4, 5]]
    );
    assert_eq!(
        env.selected,
        Some(2),
        "first adoption used the original resume fence"
    );
    assert_eq!(
        env.fenced_book_resume(&root),
        None,
        "the open dependency has ended"
    );
    let writes = service.watermarks().admitted;
    env.spread_shift_memory_setting_changed(true);
    assert_eq!(
        env.spread_shift_anchor_idx(),
        None,
        "ON after adoption does not resurrect the old saved row in place"
    );
    assert_eq!(service.watermarks().admitted, writes);
    env.with_window_context_for_test(sibling, |app| {
        assert_eq!(app.spread_shift_anchor_idx(), Some(1));
        assert_eq!(persistence(app), ShiftPersistence::SessionOnly);
    });
    assert_eq!(
        row(&service, &scope),
        Some(SpreadShiftPageRef::RelativePath("3.png".into()))
    );
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
fn parked_reply_targets_its_owner_and_retired_reply_keeps_new_context_unchanged() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/parked");
    let other = env.tmp.path().join("books/main");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &other);
    let (service, owner) = runtime(&mut env, &data);
    env.settings.remember_spread_shift_per_book = false;
    env.set_live_spread_shift(1);
    env.settings.remember_spread_shift_per_book = true;
    let pause = service.pause_for_test();
    let parked = env.build_window_context_for_test(136_402, |app| {
        install_book(app, &root);
        app.set_live_spread_shift(3);
    });
    let main_state = env.spread_shift.clone();
    pause.send(()).unwrap();
    assert_eq!(
        row(&service, &BookScopeKey::folder(&root).unwrap()),
        Some(SpreadShiftPageRef::RelativePath("3.png".into()))
    );
    env.poll_spread_shift_service();
    assert_eq!(env.spread_shift, main_state);
    env.with_window_context_for_test(parked, |app| {
        assert_eq!(app.spread_shift_anchor_idx(), Some(3));
        assert!(matches!(persistence(app), ShiftPersistence::Saved(_)));
    });
    let pause = service.pause_for_test();
    env.with_window_context_for_test(parked, |app| {
        app.set_live_spread_shift(5);
    });
    env.retire_context(parked, "shift_retired_reply_test", |_| ())
        .unwrap();
    let successor = env.build_window_context_for_test(136_402, |app| {
        install_book(app, &root);
        app.settings.remember_spread_shift_per_book = false;
        app.set_live_spread_shift(1);
        app.settings.remember_spread_shift_per_book = true;
    });
    pause.send(()).unwrap();
    assert_eq!(
        row(&service, &BookScopeKey::folder(&root).unwrap()),
        Some(SpreadShiftPageRef::RelativePath("5.png".into()))
    );
    env.poll_spread_shift_service();
    assert_eq!(env.spread_shift, main_state);
    env.with_window_context_for_test(successor, |app| {
        assert_eq!(app.spread_shift_anchor_idx(), Some(1));
        assert_eq!(persistence(app), ShiftPersistence::SessionOnly);
    });
    let pause = service.pause_for_test();
    env.with_window_context_for_test(successor, |app| {
        app.set_live_spread_shift(5);
        app.reset_live_spread_shift(true);
    });
    env.retire_context(successor, "shift_retired_clear_reply_test", |_| ())
        .unwrap();
    let clear_successor = env.build_window_context_for_test(136_402, |app| {
        install_book(app, &root);
        app.settings.remember_spread_shift_per_book = false;
        app.set_live_spread_shift(1);
        app.settings.remember_spread_shift_per_book = true;
    });
    env.fs_feedback_toast = None;
    pause.send(()).unwrap();
    assert_eq!(row(&service, &BookScopeKey::folder(&root).unwrap()), None);
    env.poll_spread_shift_service();
    assert_eq!(env.spread_shift, main_state);
    assert!(
        env.fs_feedback_toast
            .as_ref()
            .unwrap()
            .0
            .contains("解除しました"),
        "retired Clear still has its own typed completion kind"
    );
    env.with_window_context_for_test(clear_successor, |app| {
        assert_eq!(app.spread_shift_anchor_idx(), Some(1));
        assert_eq!(persistence(app), ShiftPersistence::SessionOnly);
    });
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
fn f12_materialized_fork_moves_live_identity_and_units_without_write_on_mount_or_drop() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/f12");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let (service, owner) = runtime(&mut env, &data);
    env.settings.remember_spread_shift_per_book = false;
    env.set_live_spread_shift(1);
    let shifted = env.spread_display_unit_pages_for_test(&[0, 1, 2, 3, 4, 5]);
    let detached = env.fork_materialized_still_context(&root, 2);
    env.with_window_context_for_test(detached, |app| {
        assert_eq!(app.spread_shift_anchor_idx(), Some(1));
        assert_eq!(persistence(app), ShiftPersistence::SessionOnly);
        assert_eq!(
            app.spread_display_unit_pages_for_test(&[0, 1, 2, 3, 4, 5]),
            shifted
        );
        app.set_live_spread_shift(3);
    });
    assert_eq!(
        env.spread_shift_anchor_idx(),
        Some(1),
        "forked reader edits remain local"
    );
    env.with_window_context_for_test(detached, |app| {
        assert_eq!(app.spread_shift_anchor_idx(), Some(3));
    });
    env.retire_context(detached, "shift_f12_drop_test", |_| ())
        .unwrap();
    assert_eq!(env.spread_shift_anchor_idx(), Some(1));
    assert_eq!(service.watermarks().admitted, 0);
    owner.shutdown_and_join();
}

#[cfg(windows)]
#[test]
fn f12_fork_of_pending_save_copies_layout_without_the_source_command_or_completion() {
    let mut env = setup_app();
    let root = env.tmp.path().join("books/f12-pending");
    let data = env.tmp.path().join("shift-data");
    install_book(&mut env, &root);
    let scope = env.current_spread_shift_scope().unwrap();
    let (service, owner) = runtime(&mut env, &data);
    let pause = service.pause_for_test();
    assert!(env.set_live_spread_shift(1));
    assert!(matches!(persistence(&env), ShiftPersistence::Saving(_)));
    let source_units = env.spread_display_unit_pages_for_test(&[0, 1, 2, 3, 4, 5]);
    let admitted = service.watermarks().admitted;
    let detached = env.fork_materialized_still_context(&root, 2);
    env.with_window_context_for_test(detached, |app| {
        assert_eq!(persistence(app), ShiftPersistence::SessionOnly);
        assert_eq!(app.spread_shift_anchor_idx(), Some(1));
        assert_eq!(
            app.spread_display_unit_pages_for_test(&[0, 1, 2, 3, 4, 5]),
            source_units
        );
    });
    assert_eq!(service.watermarks().admitted, admitted);
    pause.send(()).unwrap();
    row(&service, &scope);
    let replies = service.drain_results();
    assert_eq!(replies.len(), 1);
    env.with_window_context_for_test(detached, |app| {
        assert_eq!(app.spread_shift.accept_write_reply(&replies[0], true), None);
        assert_eq!(persistence(app), ShiftPersistence::SessionOnly);
        assert_eq!(app.spread_shift_anchor_idx(), Some(1));
    });
    assert_eq!(
        env.spread_shift.accept_write_reply(&replies[0], true),
        Some(false)
    );
    assert!(matches!(persistence(&env), ShiftPersistence::Saved(_)));
    env.retire_context(detached, "shift_pending_fork_drop_test", |_| ())
        .unwrap();
    assert_eq!(service.watermarks().admitted, admitted);
    owner.shutdown_and_join();
}

#[test]
fn fresh_common_resume_fence_ends_before_later_session_resume_reads() {
    use super::sidecar_restore::{FirstDisplayAction, SidecarLoadContinuation};
    let mut env = setup_app();
    let root = env.tmp.path().join("books/resume-lifetime");
    install_book(&mut env, &root);
    assert!(
        env.book_resume_writer
            .as_ref()
            .unwrap()
            .record(&root, 1, None)
    );
    let now = Instant::now();
    let continuation = SidecarLoadContinuation {
        navigation_after_hydration: None,
        audio_art_prune: None,
        first_display_action: FirstDisplayAction::None,
        restore_intent: StartupListIntent::ExplicitList,
        auto_aspect_seed: None,
        source_path: root.clone(),
        source_is_directory: true,
        prepared_subfolder: None,
        prepared_page_edits: None,
        prepared_aggregate: None,
        catalog_existing_keys: Default::default(),
        video_items: Vec::new(),
        sli_seq: 0,
        sli_t0: now,
        items_len: env.items.len(),
        detached_physical: false,
        tx: env.tx.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
        restore_started_at: now,
    };
    assert!(env.begin_sidecar_restore(continuation, false).is_ok());
    env.finish_page_identity_preparation_for_test();
    assert_eq!(env.selected, Some(1));
    assert_eq!(env.fenced_book_resume(&root), None);
    let writer = env.book_resume_writer.as_ref().unwrap();
    assert!(writer.record(&root, 4, None));
    let read = writer
        .read_position_after(
            &crate::path_key::normalize(&root),
            writer.watermarks().admitted,
        )
        .recv_timeout(Duration::from_secs(10))
        .unwrap();
    assert!(read.status.is_ok());
    assert_eq!(read.position.unwrap().page, 4);
    assert_eq!(env.resume_page_for_container(), Some(4));
}

#[test]
fn collapsed_zip_root_and_converted_archive_keep_producer_resume_key_and_fixed_read() {
    for converted in [false, true] {
        let mut env = setup_app();
        let physical = env.tmp.path().join(if converted {
            "archive-cache/pages.zip"
        } else {
            "books/wrapped.zip"
        });
        let logical = if converted {
            env.tmp.path().join("books/source.rar")
        } else {
            physical.clone()
        };
        let entries = (0..6)
            .map(|page| crate::zip_loader::ZipImageEntry {
                entry_name: format!("wrapper/{page}.png"),
                uncompressed_size: 0,
                mtime: 0,
                container_ancestry: Vec::new(),
            })
            .collect();
        let tree = Arc::new(crate::zip_tree::ZipTree::build(physical.clone(), entries));
        let nav = crate::zip_tree::ZipNavState::new(tree.clone());
        assert!(nav.at_root());
        assert!(!nav.current().is_empty(), "root is the collapsed wrapper");
        let (items, metas) = tree.materialize_level(nav.current(), Default::default());
        env.current_folder = Some(physical.clone());
        env.archive_source_override = converted.then_some(logical.clone());
        env.items = items;
        env.image_metas = metas;
        env.thumbnails = vec![ThumbnailState::Pending; env.items.len()];
        env.visible_indices = (0..env.items.len()).collect();
        env.zip_nav = Some(nav);
        env.viewer_navigation_caches.invalidate();
        env.page_identity_index = Arc::new(file_type_resume::PreparedPageIdentityIndex::prepare(
            &env.items,
            Some(&logical),
            env.settings.file_type_visibility.clone(),
        ));
        env.settings.remember_spread_shift_per_book = false;
        // An absent UI read handle makes any synchronous SELECT fallback observable.
        env.book_resume_db = None;
        env.book_resume_writer = crate::book_resume_db::BookResumeWriter::spawn_at(
            env.tmp.path().join("archive-resume.db"),
        );
        let pause = env.book_resume_writer.as_ref().unwrap().pause_for_test();
        if converted {
            assert!(
                env.book_resume_writer
                    .as_ref()
                    .unwrap()
                    .record(&logical, 5, None)
            );
        }
        env.record_book_resume(2);
        let cutoff = env
            .book_resume_writer
            .as_ref()
            .unwrap()
            .watermarks()
            .admitted;
        let mut restore = env.begin_book_shift_restore(136_410).unwrap();
        assert!(!env.poll_book_shift_restore(&mut restore).unwrap());
        env.record_book_resume(4);
        pause.send(()).unwrap();
        finish_restore(&mut env, &mut restore);
        let latest = env
            .book_resume_writer
            .as_ref()
            .unwrap()
            .read_position_after(&crate::path_key::normalize(&physical), cutoff + 1)
            .recv_timeout(Duration::from_secs(10))
            .unwrap();
        assert!(latest.status.is_ok());
        assert_eq!(latest.position.unwrap().page, 4);
        let initial = env.fenced_book_resume(&physical).unwrap().unwrap();
        assert_eq!(
            initial.page, 2,
            "initial admission must precede the later Record"
        );
        assert_eq!(env.resume_page_for_container(), Some(2));
        env.spread_shift.finish_fenced_book_resume();
        assert_eq!(env.fenced_book_resume(&physical), None);
    }
}

fn begin_test_book_open(app: &mut App, root: &Path) {
    use super::sidecar_restore::{FirstDisplayAction, SidecarLoadContinuation};
    let now = Instant::now();
    let continuation = SidecarLoadContinuation {
        navigation_after_hydration: None,
        audio_art_prune: None,
        first_display_action: FirstDisplayAction::None,
        restore_intent: StartupListIntent::ExplicitList,
        auto_aspect_seed: None,
        source_path: root.to_path_buf(),
        source_is_directory: true,
        prepared_subfolder: None,
        prepared_page_edits: None,
        prepared_aggregate: None,
        catalog_existing_keys: Default::default(),
        video_items: Vec::new(),
        sli_seq: 0,
        sli_t0: now,
        items_len: app.items.len(),
        detached_physical: false,
        tx: app.tx.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
        restore_started_at: now,
    };
    assert!(app.begin_sidecar_restore(continuation, false).is_ok());
}

fn finish_test_book_rename(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(20);
    while app.book_op_pending.is_some() {
        assert!(Instant::now() < deadline, "book rename did not finish");
        app.poll_book_op_pending(ctx);
        std::thread::sleep(Duration::from_millis(2));
    }
}

#[cfg(windows)]
#[test]
fn book_rename_does_not_wait_for_same_book_pending_open_or_unrelated_open() {
    for (pending, resume_pending) in [
        (None, false),
        (Some("Old"), true),
        (Some("Old"), false),
        (Some("Old-other"), true),
        (Some("Old-other"), false),
    ] {
        let mut env = setup_app();
        let books = env.tmp.path().join("books");
        let old = books.join("Old");
        let new = books.join("New");
        std::fs::create_dir_all(&old).unwrap();
        std::fs::write(old.join("0.png"), b"page").unwrap();
        env.settings.book_root = Some(books.clone());
        env.settings.sidecar_backup_enabled = false;
        let data = env.tmp.path().join("pending-rename-shifts");
        let (service, owner) = runtime(&mut env, &data);
        let old_scope = BookScopeKey::folder(&old).unwrap();
        let anchor = SpreadShiftPageRef::RelativePath("0.png".into());
        service
            .set_shift(
                old_scope.clone(),
                anchor.clone(),
                ShiftOperation {
                    context_id: env.projected_viewer_context_id(),
                    items_generation: env.items_generation,
                    operation_sequence: 1,
                },
            )
            .unwrap();
        assert_eq!(row(&service, &old_scope), Some(anchor.clone()));
        let epoch = service.maintenance_epoch();
        // The original open's shift result stays unconsumed across maintenance.
        let pause = service.pause_for_test();
        let accepted = pending.map(|name| books.join(name));
        let mut resume_release = None;
        let mut sibling = None;
        if let Some(source) = &accepted {
            install_book(&mut env, source);
            sibling = Some(env.build_window_context_for_test(136_411, |app| {
                install_book(app, source);
            }));
            let db_path = env.tmp.path().join("pending-rename-resume.db");
            env.book_resume_db =
                Some(crate::book_resume_db::BookResumeDb::open_at(&db_path).unwrap());
            env.book_resume_writer = crate::book_resume_db::BookResumeWriter::spawn_at(db_path);
            let writer = env.book_resume_writer.as_ref().unwrap();
            assert!(writer.record(source, 0, None));
            writer
                .read_position_after(&crate::path_key::normalize(source), 1)
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            assert_eq!(
                env.book_resume_db
                    .as_ref()
                    .unwrap()
                    .get_position(source)
                    .unwrap()
                    .page,
                0
            );
            resume_release = Some(writer.pause_for_test());
            // This admitted save is pending when the original open fixes its read.
            assert!(writer.record(source, 2, None));
            begin_test_book_open(&mut env, source);
            assert!(env.sidecar_restore_active());
            // The same-book sibling's later save must not replace the original read.
            env.with_window_context_for_test(sibling.unwrap(), |app| app.record_book_resume(4));
            if !resume_pending {
                resume_release.take().unwrap().send(()).unwrap();
                let latest = env
                    .book_resume_writer
                    .as_ref()
                    .unwrap()
                    .read_position_after(&crate::path_key::normalize(source), 3)
                    .recv_timeout(Duration::from_secs(10))
                    .unwrap();
                assert_eq!(latest.position.unwrap().page, 4);
            }
        }
        let before = env.spread_shift.clone();
        let ctx = egui::Context::default();
        env.start_book_rename(&ctx, "Old".into(), "New".into());
        assert!(env.book_op_pending.is_some());
        finish_test_book_rename(&mut env, &ctx);
        assert!(!old.exists());
        assert!(new.exists());
        assert_eq!(env.spread_shift, before);
        if let Some(source) = &accepted {
            assert!(
                env.sidecar_restore_active(),
                "rename did not wait for open adoption"
            );
            assert_eq!(env.current_folder.as_deref(), Some(source.as_path()));
            assert_eq!(
                env.spread_shift.scope,
                Some(BookScopeKey::folder(source).unwrap())
            );
        }
        pause.send(()).unwrap();
        assert_eq!(row(&service, &old_scope), None);
        assert_eq!(
            row(&service, &BookScopeKey::folder(&new).unwrap()),
            Some(anchor)
        );
        assert!(service.maintenance_epoch() > epoch);
        if let Some(source) = &accepted {
            if resume_pending {
                // Drive Common to its resume-dependent terminal while Record is
                // still held. Epoch rejection must not bypass this dependency.
                for _ in 0..20 {
                    env.poll_sidecar_restore(&ctx);
                    assert!(env.sidecar_restore_active(), "resume fence was discarded");
                    std::thread::sleep(Duration::from_millis(2));
                }
                assert_eq!(
                    env.book_resume_db
                        .as_ref()
                        .unwrap()
                        .get_position(source)
                        .unwrap()
                        .page,
                    0
                );
                resume_release.take().unwrap().send(()).unwrap();
            }
            let latest = env
                .book_resume_writer
                .as_ref()
                .unwrap()
                .read_position_after(&crate::path_key::normalize(source), 3)
                .recv_timeout(Duration::from_secs(10))
                .unwrap();
            assert_eq!(latest.position.unwrap().page, 4);
            env.finish_page_identity_preparation_for_test();
            assert_eq!(
                env.selected,
                Some(2),
                "Common must adopt the original fixed resume, not DB page 0 or 4"
            );
            assert_eq!(
                env.spread_shift_anchor_idx(),
                None,
                "the stale shift recipe was rejected"
            );
            assert_eq!(env.current_folder.as_deref(), Some(source.as_path()));
            assert_eq!(
                env.fenced_book_resume(source),
                None,
                "initial adoption ended the fence"
            );
            env.with_window_context_for_test(sibling.unwrap(), |app| {
                assert_eq!(app.current_folder.as_deref(), Some(source.as_path()));
                assert_eq!(
                    app.spread_shift.scope,
                    Some(BookScopeKey::folder(source).unwrap())
                );
                assert_eq!(app.spread_shift_anchor_idx(), None);
            });
        }
        owner.shutdown_and_join();
    }
}

#[cfg(windows)]
#[test]
fn adjusted_retained_f12_keeps_session_across_unrelated_and_same_book_rename() {
    for renamed in ["Other", "Shown"] {
        let mut env = setup_app();
        let books = env.tmp.path().join("books");
        let shown = books.join("Shown");
        let other = books.join("Other");
        let main = books.join("Main");
        for root in [&shown, &other] {
            std::fs::create_dir_all(root).unwrap();
            std::fs::write(root.join("0.png"), b"page").unwrap();
        }
        env.settings.book_root = Some(books.clone());
        env.settings.sidecar_backup_enabled = false;
        install_book(&mut env, &main);
        env.settings.remember_spread_shift_per_book = false;
        env.set_live_spread_shift(0);
        env.settings.remember_spread_shift_per_book = true;
        let (service, owner) = runtime(&mut env, &books.join("shift-data"));
        let scope = BookScopeKey::folder(&shown).unwrap();
        let operation = |sequence| ShiftOperation {
            context_id: ViewerContextId::for_test(999),
            items_generation: 0,
            operation_sequence: sequence,
        };
        service
            .set_shift(
                BookScopeKey::folder(&other).unwrap(),
                SpreadShiftPageRef::RelativePath("0.png".into()),
                operation(1),
            )
            .unwrap();
        row(&service, &BookScopeKey::folder(&other).unwrap());
        let sibling = env.build_window_context_for_test(136_412, |app| {
            install_book(app, &shown);
            app.settings.remember_spread_shift_per_book = false;
            app.set_live_spread_shift(1);
            app.settings.remember_spread_shift_per_book = true;
            app.fullscreen_idx = Some(3);
            app.selected = Some(3);
        });
        let main_before = env.spread_shift.clone();
        let main_items = env.items.clone();
        let main_index = env.page_identity_index.clone();
        let before_epoch = service.maintenance_epoch_for(&scope);
        let pause = service.pause_for_test();
        // Force retained re-entry to wait for a real writer read. Its reply is
        // captured before the subsequent rename but polled after it completes.
        let saved = service
            .set_shift(
                scope.clone(),
                SpreadShiftPageRef::RelativePath("1.png".into()),
                operation(2),
            )
            .unwrap();
        let ctx = egui::Context::default();
        let mut accepted = None;
        env.with_window_context_for_test(sibling, |app| {
            accepted = Some(app.spread_shift.clone());
            app.begin_retained_shift_readoption(&ctx);
            assert!(app.spread_shift.retained_readoption_pending());
            assert_eq!(app.spread_shift_anchor_idx(), Some(1));
        });
        let target_name = format!("{renamed}-renamed");
        env.start_book_rename(&ctx, renamed.into(), target_name.clone());
        finish_test_book_rename(&mut env, &ctx);
        assert!(books.join(&target_name).exists());
        pause.send(()).unwrap();
        row(
            &service,
            &BookScopeKey::folder(books.join(&target_name)).unwrap(),
        );
        if renamed == "Other" {
            assert_eq!(
                service.maintenance_epoch_for(&scope),
                before_epoch,
                "an unrelated book must not invalidate this reply"
            );
        } else {
            assert!(service.maintenance_epoch_for(&scope) > before_epoch);
        }
        env.with_window_context_for_test(sibling, |app| {
            app.poll_retained_shift_readoption(&ctx);
            assert!(!app.spread_shift.retained_readoption_pending());
            assert_eq!(app.spread_shift_anchor_idx(), Some(1));
            assert_eq!(app.fullscreen_idx, Some(3));
            assert_eq!(app.selected, Some(3));
            assert_eq!(app.current_folder.as_deref(), Some(shown.as_path()));
            if renamed == "Shown" {
                assert_eq!(
                    app.spread_shift,
                    accepted.unwrap(),
                    "expiry preserves accepted state, command and projection"
                );
                // A future ordinary retained read captures the new old-key state;
                // only that valid absence may clear the adjustment, not expiry.
                app.begin_retained_shift_readoption(&ctx);
                assert!(!app.spread_shift.retained_readoption_pending());
                assert_eq!(app.spread_shift_anchor_idx(), None);
            } else {
                assert_eq!(
                    persistence(app),
                    ShiftPersistence::Saved(saved),
                    "unrelated rename leaves the original captured read eligible"
                );
            }
        });
        assert_eq!(env.spread_shift, main_before);
        assert_eq!(env.items, main_items);
        assert!(Arc::ptr_eq(&env.page_identity_index, &main_index));
        owner.shutdown_and_join();
    }
}

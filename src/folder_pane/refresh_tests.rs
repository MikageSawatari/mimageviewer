use super::*;

fn p(path: &str) -> PathBuf {
    PathBuf::from(path)
}
fn synthetic_tree(root: &Path, children: Vec<PathBuf>) -> FolderPaneState {
    let mut state = FolderPaneState::test_tree(root, children);
    state.scan_backend = Some(Arc::new(|_, _, _| Err("synthetic fixture scan".into())));
    state
}
fn listing(options: FolderPaneListingOptions, children: Vec<PathBuf>) -> CachedListing {
    CachedListing {
        membership: children.iter().map(|path| key_for(path)).collect(),
        children,
        options,
    }
}

#[test]
fn folder_pane_unresolved_drive_arrival_is_a_following_fallback_not_an_open_target() {
    let mut state = FolderPaneState::default();
    state.last_drive_refresh = Some(std::time::Instant::now());
    state.drives.clear();
    state.selection = FolderPaneSelection::FollowActive(FolderPanePosition::default());
    state.set_focus_tree_at_active();
    assert!(state.cursor_path().is_none());
    state.apply_drive_list(vec![p(r"C:\"), p(r"G:\")]);
    state.sync_to_active(None, state.listing_options);
    assert_eq!(state.cursor_path(), Some(Path::new(r"C:\")));
    assert!(state.cursor_nav_target_if_moved().is_none());
    state.sync_to_active(Some(Path::new(r"G:\late\current")), state.listing_options);
    assert_eq!(state.selected_drive(), Some(Path::new(r"G:\")));
    assert_eq!(state.cursor_path(), Some(Path::new(r"G:\late\current")));
    assert!(state.selection.follows_active());
}

#[test]
fn folder_pane_explicit_cursor_is_preserved_until_grid_navigation_changes_active() {
    let root = p(r"C:\");
    let a = root.join("a");
    let b = root.join("b");
    let mut state = synthetic_tree(&root, vec![a.clone(), b.clone()]);
    state.active_path = Some(a.clone());
    state.active_key = Some(key_for(&a));
    state.set_cursor(b.clone());
    state.sync_to_active(Some(&root), state.listing_options);
    assert_eq!(state.cursor_path(), Some(b.as_path()));
    state.set_focus_grid();
    state.set_focus_grid();
    state.sync_to_active(Some(&root), state.listing_options);
    assert_eq!(
        state.cursor_path(),
        Some(b.as_path()),
        "focus loss alone must preserve the shown position"
    );
    assert_eq!(state.cursor_nav_target_if_moved(), Some(b));
    state.sync_to_active(Some(&a), state.listing_options);
    assert_eq!(state.cursor_path(), Some(a.as_path()));
}

#[test]
fn folder_pane_explicit_drive_is_preserved_and_focus_loss_does_not_split_the_pair() {
    let mut state = synthetic_tree(Path::new(r"C:\"), vec![]);
    state.active_path = Some(p(r"C:\a"));
    state.active_key = Some(key_for(Path::new(r"C:\a")));
    state.select_drive(p(r"G:\"));
    state.sync_to_active(Some(Path::new(r"C:\b")), state.listing_options);
    assert_eq!(state.selected_drive(), Some(Path::new(r"G:\")));
    assert_eq!(state.cursor_path(), Some(Path::new(r"G:\")));
    assert_eq!(state.cursor_nav_target_if_moved(), Some(p(r"G:\")));
    state.set_focus_grid();
    state.sync_to_active(Some(Path::new(r"C:\b")), state.listing_options);
    assert_eq!(state.selected_drive(), Some(Path::new(r"G:\")));
    assert_eq!(state.cursor_path(), Some(Path::new(r"G:\")));
    state.sync_to_active(Some(Path::new(r"C:\c")), state.listing_options);
    assert_eq!(state.selected_drive(), Some(Path::new(r"C:\")));
    assert_eq!(state.cursor_path(), Some(Path::new(r"C:\c")));
}

#[test]
fn folder_pane_expand_and_collapse_without_cursor_motion_are_explicit_choices() {
    for key in [FolderPaneTreeKey::Left, FolderPaneTreeKey::Right] {
        let root = p(r"C:\");
        let child = root.join("child");
        let mut state = synthetic_tree(&root, vec![child.clone()]);
        state.active_path = Some(root.clone());
        state.active_key = Some(key_for(&root));
        if key == FolderPaneTreeKey::Right {
            state.user_expanded.clear();
        }
        state.set_focus_tree_at_active();
        state.handle_tree_key(key);
        assert!(!state.selection.follows_active());
        state.sync_to_active(Some(&child), state.listing_options);
        assert_eq!(state.cursor_path(), Some(root.as_path()));
    }
}

#[test]
fn folder_pane_drive_disappearance_repairs_both_fields_and_keeps_origin() {
    let mut state = synthetic_tree(Path::new(r"G:\"), vec![]);
    state.set_cursor(p(r"G:\manual"));
    state.apply_drive_list(vec![p(r"C:\")]);
    assert_eq!(state.selected_drive(), Some(Path::new(r"C:\")));
    assert_eq!(state.cursor_path(), Some(Path::new(r"C:\")));
    assert!(!state.selection.follows_active());
    state.set_focus_grid();
    state.apply_drive_list(vec![p(r"G:\")]);
    assert_eq!(state.selected_drive(), Some(Path::new(r"G:\")));
    assert_eq!(state.cursor_path(), Some(Path::new(r"G:\")));
    assert!(state.selection.follows_active());
    assert!(state.selection.has_explicit_position());
    state.selection = FolderPaneSelection::FollowActive(FolderPanePosition::default());
    state.active_path = Some(p(r"G:\late"));
    state.apply_drive_list(vec![]);
    assert_eq!(
        state.selected_drive(),
        Some(Path::new(r"G:\")),
        "active root need not await enumeration"
    );
    assert_eq!(state.cursor_path(), Some(Path::new(r"G:\late")));
    assert!(state.selection.follows_active());
}

#[test]
fn folder_pane_passive_cursor_repair_keeps_follow_origin_without_reviving_a_missing_active() {
    let root = p(r"C:\");
    let active = root.join("missing");
    let mut state = synthetic_tree(&root, vec![active.clone()]);
    state.active_path = Some(active.clone());
    state.active_key = Some(key_for(&active));
    state.set_focus_tree_at_active();
    let reply = pending(&mut state, &root);
    reply
        .send(Ok(listing(state.listing_options, vec![])))
        .unwrap();
    state.poll_pending();
    assert_eq!(state.cursor_path(), Some(root.as_path()));
    assert!(state.selection.follows_active());
    state.sync_to_active(Some(&active), state.listing_options);
    assert_eq!(state.cursor_path(), Some(root.as_path()));
    assert!(!state.nodes.contains_key(&key_for(&active)));
}

struct Reply {
    tx: mpsc::Sender<Result<CachedListing, String>>,
    cancel: Arc<AtomicBool>,
}
impl Reply {
    fn send(
        &self,
        result: Result<CachedListing, String>,
    ) -> Result<(), mpsc::SendError<Result<CachedListing, String>>> {
        // Match the production worker's pre-send cancellation boundary.
        if self.cancel.load(Ordering::Relaxed) {
            return Err(mpsc::SendError(result));
        }
        self.tx.send(result)
    }
}

fn pending(state: &mut FolderPaneState, path: &Path) -> Reply {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    state.ensure_node(path.to_path_buf());
    state.demand.insert(key_for(path));
    state.nodes.get_mut(&key_for(path)).unwrap().scan = NodeScan::Running(
        FolderPaneScanPending {
            cancel: cancel.clone(),
            rx,
            options: state.listing_options,
        },
        FollowUp::None,
    );
    Reply { tx, cancel }
}

#[test]
fn successful_refresh_prunes_only_missing_components_and_rejects_child_late_reply() {
    for parent_first in [true, false] {
        let root = p(r"C:\data");
        let aaa = root.join("aaa");
        let aaa2 = root.join("aaa2");
        let mut state = synthetic_tree(&root, vec![aaa.clone(), aaa2.clone()]);
        state.test_seed_listing(&aaa, vec![aaa.join("deep")], true);
        state.test_seed_listing(&aaa2, vec![aaa2.join("keep")], true);
        state.selection.position_mut().cursor = Some(aaa.join("deep"));
        state.active_path = Some(aaa.clone());
        let parent_tx = pending(&mut state, &root);
        let child_tx = pending(&mut state, &aaa);
        if !parent_first {
            child_tx
                .send(Ok(listing(state.listing_options, vec![aaa.join("late")])))
                .unwrap();
            state.poll_pending();
        }
        parent_tx
            .send(Ok(listing(state.listing_options, vec![aaa2.clone()])))
            .unwrap();
        if parent_first {
            child_tx
                .send(Ok(listing(state.listing_options, vec![aaa.join("late")])))
                .unwrap();
        }
        state.poll_pending();
        assert!(!state.nodes.contains_key(&key_for(&aaa)));
        assert!(!state.nodes.contains_key(&key_for(&aaa.join("late"))));
        assert_eq!(state.test_children(&aaa2), vec![aaa2.join("keep")]);
        assert!(state.user_expanded.contains(&key_for(&aaa2)));
        assert_eq!(state.cursor_path(), Some(root.as_path()));
        assert!(
            child_tx
                .send(Ok(listing(state.listing_options, vec![])))
                .is_err()
        );
    }
}

#[test]
fn dirty_after_send_and_repeated_dirty_coalesce_without_adopting_the_old_reply() {
    let root = p(r"C:\data");
    let old = root.join("old");
    let mut state = synthetic_tree(&root, vec![old.clone()]);
    let tx = pending(&mut state, &root);
    tx.send(Ok(listing(state.listing_options, vec![root.join("stale")])))
        .unwrap();
    for _ in 0..10 {
        state.observe_accepted_folder(&root);
    }
    assert!(matches!(
        state.nodes[&key_for(&root)].scan,
        NodeScan::Running(_, FollowUp::Recheck)
    ));
    // Keep the follow-up deterministic rather than racing a filesystem error.
    state.scan_backend = Some(Arc::new(|_, options, _| {
        Ok(listing(options, vec![p(r"C:\data\fresh")]))
    }));
    state.poll_pending();
    assert_eq!(state.test_children(&root), vec![old]);
    state.test_settle();
    assert_eq!(state.test_children(&root), vec![root.join("fresh")]);
}

#[test]
fn refresh_failure_disconnect_and_option_changes_keep_cache_and_do_not_retry_per_frame() {
    for disconnect in [true, false] {
        let root = p(r"C:\data");
        let old = root.join("old");
        let mut state = synthetic_tree(&root, vec![old.clone()]);
        let tx = pending(&mut state, &root);
        if !disconnect {
            tx.send(Err("entry/classification failure".into())).unwrap();
        }
        drop(tx);
        state.poll_pending();
        assert_eq!(state.test_children(&root), vec![old]);
        assert!(state.visible_rows()[0].error.is_some());
        for _ in 0..10 {
            state.reconcile_demand();
            state.poll_pending();
        }
        assert!(!state.has_pending());
        assert_eq!(state.cursor_scroll, FolderPaneCursorScroll::Idle);
    }
    let root = p(r"C:\");
    let mut state = synthetic_tree(&root, vec![root.join("old")]);
    let a = pending(&mut state, &root);
    let mut options = state.listing_options;
    options.sort_order = FolderTreeSortOrder::NameDesc;
    state.sync_to_active(None, options);
    assert!(a.send(Ok(listing(options, vec![]))).is_err());
    let b = pending(&mut state, &root);
    options.sort_order = FolderTreeSortOrder::NumericDesc;
    state.sync_to_active(None, options);
    assert!(b.send(Ok(listing(options, vec![]))).is_err());
    let c = pending(&mut state, &root);
    c.send(Ok(listing(options, vec![root.join("latest")])))
        .unwrap();
    state.poll_pending();
    assert_eq!(state.test_children(&root), vec![root.join("latest")]);
}

#[test]
fn sort_hidden_refresh_preserves_manual_expansion_cursor_and_cached_rows_until_success() {
    let root = p(r"C:\");
    let active = root.join("active");
    let manual = root.join("manual");
    let mut state = synthetic_tree(&root, vec![active.clone(), manual.clone()]);
    state.test_seed_listing(&manual, vec![manual.join("old")], true);
    state.active_path = Some(active.clone());
    state.active_key = Some(key_for(&active));
    state.selection.position_mut().cursor = Some(manual.clone());
    state.cursor_scroll = FolderPaneCursorScroll::Idle;
    state.scan_backend = Some(Arc::new(|_, _, _| Err("not yet verified".into())));
    let mut options = state.listing_options;
    options.sort_order = FolderTreeSortOrder::DateDesc;
    options.show_hidden_files = true;
    state.sync_to_active(Some(&active), options);
    assert_eq!(state.test_children(&manual), vec![manual.join("old")]);
    assert!(state.user_expanded.contains(&key_for(&manual)));
    assert_eq!(state.selection.position().cursor, Some(manual.clone()));
    assert_eq!(state.cursor_scroll, FolderPaneCursorScroll::Idle);
    state.test_settle();
    assert_eq!(state.test_children(&manual), vec![manual.join("old")]);
    assert_eq!(state.selection.position().cursor, Some(manual));
    assert_eq!(state.cursor_scroll, FolderPaneCursorScroll::Idle);
}

#[test]
fn focus_active_does_not_clear_unrelated_explicit_collapses_and_manual_expansion_survives_navigation()
 {
    let root = p(r"C:\");
    let a = root.join("a");
    let b = root.join("b");
    let mut state = synthetic_tree(&root, vec![a.clone(), b.clone()]);
    state.test_seed_listing(&a, vec![], true);
    state.test_seed_listing(&b, vec![], true);
    state.active_path = Some(a.clone());
    state.user_collapsed.insert(key_for(&b));
    state.set_focus_tree_at_active();
    assert!(state.user_collapsed.contains(&key_for(&b)));
    state.suspend_display();
    state.sync_to_active(Some(&b), state.listing_options);
    assert!(state.user_expanded.contains(&key_for(&a)));
    assert!(state.user_expanded.contains(&key_for(&b)));
}

#[cfg(windows)]
#[test]
fn complete_scan_membership_includes_hidden_and_system_directories_before_display_filter() {
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::{
        FILE_ATTRIBUTE_HIDDEN, FILE_ATTRIBUTE_SYSTEM, SetFileAttributesW,
    };
    let tmp = tempfile::tempdir().unwrap();
    let hidden = tmp.path().join("hidden");
    let system = tmp.path().join("system");
    for (path, attributes) in [
        (&hidden, FILE_ATTRIBUTE_HIDDEN),
        (&system, FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM),
    ] {
        std::fs::create_dir(path).unwrap();
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            SetFileAttributesW(windows::core::PCWSTR(wide.as_ptr()), attributes).unwrap();
        }
    }
    let listing = scan_complete_listing(tmp.path(), Default::default(), None).unwrap();
    assert!(listing.children.is_empty());
    assert_eq!(
        listing.membership,
        [key_for(&hidden), key_for(&system)].into_iter().collect()
    );
    let listing = scan_complete_listing(
        tmp.path(),
        FolderPaneListingOptions {
            show_hidden_files: true,
            ..Default::default()
        },
        None,
    )
    .unwrap();
    assert_eq!(listing.children, vec![hidden]);
    assert!(listing.membership.contains(&key_for(&system)));
}

#[test]
fn active_change_rechecks_first_stale_ancestor_once_but_same_location_cannot_revive_removed_branch()
{
    let root = p(r"D:\");
    let data = root.join("data");
    let work = root.join("work");
    let new = data.join("new");
    let mut state = synthetic_tree(&root, vec![data.clone(), work.clone()]);
    state.test_seed_listing(&data, vec![], true);
    state.active_path = Some(work.clone());
    state.active_key = Some(key_for(&work));
    state.set_focus_tree_at_active();
    state.sync_to_active(Some(&new), state.listing_options);
    assert!(matches!(
        state.nodes[&key_for(&data)].scan,
        NodeScan::Running(..)
    ));
    let tx = pending(&mut state, &data);
    tx.send(Ok(listing(state.listing_options, vec![]))).unwrap();
    state.poll_pending();
    for _ in 0..10 {
        state.sync_to_active(Some(&new), state.listing_options);
    }
    assert!(!state.nodes.contains_key(&key_for(&new)));
    assert!(!state.test_unverified(&data));
    assert_eq!(state.active_path(), Some(new.as_path()));
    assert!(state.handle_tree_key(FolderPaneTreeKey::Enter).is_some()); // only explicit input opens
}

#[test]
fn active_ancestor_reveal_continues_after_first_contradiction_has_been_rechecked() {
    let root = p(r"D:\");
    let data = root.join("data");
    let new = data.join("new");
    let mut state = synthetic_tree(&root, vec![data.clone()]);
    state.test_seed_listing(&data, vec![], true);
    state.sync_to_active(Some(&new), state.listing_options);
    let tx = pending(&mut state, &data);
    tx.send(Ok(listing(state.listing_options, vec![new.clone()])))
        .unwrap();
    state.poll_pending();
    assert!(
        state
            .visible_rows()
            .iter()
            .any(|row| row.path == new && row.is_active)
    );
}

#[test]
fn hidden_membership_preserves_cached_branch_but_retires_its_running_scan() {
    let root = p(r"C:\data");
    let hidden = root.join("hidden");
    let mut state = synthetic_tree(&root, vec![hidden.clone()]);
    state.test_seed_listing(&hidden, vec![hidden.join("kept")], true);
    let child_tx = pending(&mut state, &hidden);
    let tx = pending(&mut state, &root);
    tx.send(Ok(CachedListing {
        children: vec![],
        membership: [key_for(&hidden)].into_iter().collect(),
        options: state.listing_options,
    }))
    .unwrap();
    state.poll_pending();
    assert_eq!(state.test_children(&hidden), vec![hidden.join("kept")]);
    assert!(state.user_expanded.contains(&key_for(&hidden)));
    assert!(!state.demand.contains(&key_for(&hidden)));
    assert!(
        child_tx
            .send(Ok(listing(state.listing_options, vec![])))
            .is_err()
    );
}

#[test]
fn collapse_drive_switch_and_suspend_cancel_private_receivers_only_in_the_pane() {
    let root = p(r"C:\data");
    let branch = root.join("branch");
    for route in ["collapse", "drive", "hide"] {
        let mut state = synthetic_tree(&root, vec![branch.clone()]);
        state.test_seed_listing(&branch, vec![], true);
        let tx = pending(&mut state, &branch);
        state.selection.position_mut().cursor = Some(branch.clone());
        match route {
            "collapse" => state.collapse_cursor(),
            "drive" => state.select_drive(p(r"D:\")),
            _ => state.suspend_display(),
        }
        assert!(
            tx.send(Ok(listing(state.listing_options, vec![]))).is_err(),
            "{route}"
        );
        assert!(state.nodes.contains_key(&key_for(&branch)));
        assert!(state.test_unverified(&branch));
    }
}

#[test]
fn explicitly_collapsed_auto_ancestor_leaves_demand_and_cannot_retry_in_the_background() {
    let root = p(r"D:\");
    let parent = root.join("parent");
    let active = parent.join("active");
    let mut state = synthetic_tree(&root, vec![parent.clone()]);
    state.test_seed_listing(&parent, vec![active.clone()], false);
    state.auto_expanded.insert(key_for(&parent));
    state.demand.insert(key_for(&parent));
    let tx = pending(&mut state, &parent);
    state.selection.position_mut().cursor = Some(parent.clone());
    state.collapse_cursor();
    assert!(!state.demand.contains(&key_for(&parent)));
    assert!(tx.send(Ok(listing(state.listing_options, vec![]))).is_err());
    for _ in 0..10 {
        state.reconcile_demand();
        assert!(!state.has_pending());
    }
    state.expand_cursor();
    assert!(state.demand.contains(&key_for(&parent)));
}

#[test]
fn same_path_replacement_is_retrieved_by_reexpand_redisplay_or_explicit_reload() {
    for route in ["expand", "display", "reload"] {
        let tmp = tempfile::tempdir().unwrap();
        let root = tmp.path();
        let branch = root.join("branch");
        std::fs::create_dir(&branch).unwrap();
        std::fs::create_dir(branch.join("old")).unwrap();
        let mut state = FolderPaneState::test_tree(root, vec![branch.clone()]);
        state.test_seed_listing(&branch, vec![branch.join("old")], true);
        std::fs::rename(&branch, root.join("moved")).unwrap();
        std::fs::create_dir(&branch).unwrap();
        std::fs::create_dir(branch.join("new")).unwrap();
        // No watch: leave the cache intact until a supported (c) or manual entry.
        assert_eq!(state.test_children(&branch), vec![branch.join("old")]);
        match route {
            "expand" => {
                state.selection.position_mut().cursor = Some(branch.clone());
                state.collapse_cursor();
                state.expand_cursor();
            }
            "display" => {
                state.suspend_display();
                state.resume_display();
                state.reconcile_demand();
            }
            _ => {
                state.reload_for_active(None, state.listing_options);
                state.test_settle();
                state.selection.position_mut().cursor = Some(root.to_path_buf());
                state.expand_cursor();
                state.test_settle();
                state.selection.position_mut().cursor = Some(branch.clone());
                state.expand_cursor();
            }
        }
        state.test_settle();
        assert_eq!(
            state.test_children(&branch),
            vec![branch.join("new")],
            "{route}"
        );
    }
}

#[test]
fn deleted_current_folder_is_removed_by_parent_reconciliation_on_redisplay() {
    let tmp = tempfile::tempdir().unwrap();
    let root = tmp.path();
    let active = root.join("active");
    std::fs::create_dir(&active).unwrap();
    let mut state = FolderPaneState::test_tree(root, vec![active.clone()]);
    state.test_seed_listing(&active, vec![], true);
    state.active_path = Some(active.clone());
    state.active_key = Some(key_for(&active));
    std::fs::remove_dir(&active).unwrap();
    // The list's metadata failure never becomes an accepted observation.
    assert_eq!(state.test_children(root), vec![active.clone()]);
    state.suspend_display();
    state.resume_display();
    state.reconcile_demand();
    state.test_settle();
    assert!(state.test_children(root).is_empty());
    assert!(!state.nodes.contains_key(&key_for(&active)));
    assert_eq!(state.active_path(), Some(active.as_path()));
}

#[test]
fn complete_scan_cancellation_and_read_failure_are_never_empty_success() {
    let tmp = tempfile::tempdir().unwrap();
    std::fs::create_dir(tmp.path().join("keep")).unwrap();
    let cancel = AtomicBool::new(true);
    assert_eq!(
        scan_complete_listing(tmp.path(), Default::default(), Some(&cancel))
            .unwrap_err()
            .kind(),
        std::io::ErrorKind::Interrupted
    );
    assert!(scan_complete_listing(&tmp.path().join("missing"), Default::default(), None).is_err());
}

#[test]
fn keyboard_navigation_and_manual_reset_contract_are_preserved() {
    let root = p(r"C:\");
    let a = root.join("a");
    let b = root.join("b");
    let mut state = synthetic_tree(&root, vec![a.clone(), b.clone()]);
    state.selection.position_mut().cursor = Some(root.clone());
    state.handle_tree_key(FolderPaneTreeKey::Down);
    assert_eq!(
        state.handle_tree_key(FolderPaneTreeKey::Enter),
        Some(FolderPaneCommand::Open(a.clone()))
    );
    state.test_seed_listing(&a, vec![], true);
    state.user_collapsed.insert(key_for(&b));
    state.reload_for_active(Some(&a), state.listing_options);
    assert!(state.user_expanded.is_empty());
    assert!(state.user_collapsed.is_empty());
    assert_eq!(state.selection.position().cursor, Some(a));
    assert_eq!(state.cursor_scroll, FolderPaneCursorScroll::Requested);
}

#[test]
fn hundreds_of_expanded_parents_queue_with_a_bounded_start_and_no_idle_work() {
    let root = p(r"C:\data");
    let branches: Vec<_> = (0..300).map(|i| root.join(format!("{i:03}"))).collect();
    let mut state = synthetic_tree(&root, branches.clone());
    for branch in &branches {
        state.test_seed_listing(branch, vec![], true);
    }
    for _ in 0..10 {
        state.reconcile_demand();
        state.poll_pending();
        assert!(!state.has_pending());
    }
    state.scan_backend = Some(Arc::new(|_, options, _| Ok(listing(options, vec![]))));
    for branch in &branches {
        state.observe_accepted_folder(branch);
    }
    state.start_queued();
    assert_eq!(
        state
            .nodes
            .values()
            .filter(|node| matches!(node.scan, NodeScan::Running(..)))
            .count(),
        4
    );
    state.test_settle();
}

#[test]
fn blocked_scan_runs_off_ui_and_cancel_hide_drive_and_other_inputs_return_without_waiting() {
    use std::sync::Barrier;
    let root = p(r"C:\data");
    let mut state = synthetic_tree(&root, vec![]);
    let entered = Arc::new(Barrier::new(2));
    let release = Arc::new(Barrier::new(2));
    let (tx, rx) = mpsc::channel();
    let e = entered.clone();
    let r = release.clone();
    state.scan_backend = Some(Arc::new(move |_, options, _| {
        tx.send(std::thread::current().id()).unwrap();
        e.wait();
        r.wait();
        Ok(listing(options, vec![]))
    }));
    state.observe_accepted_folder(&root);
    state.start_queued();
    entered.wait();
    assert_ne!(rx.recv().unwrap(), std::thread::current().id());
    let ctx = egui::Context::default();
    crate::ui_fonts::configure_fonts(&ctx);
    let mut passes = 0;
    for _ in 0..3 {
        ctx.run_ui(Default::default(), |ui| {
            passes += 1;
            ui.label("UI remains responsive");
            state.handle_tree_key(FolderPaneTreeKey::Down);
        })
        .drop_without_applying_deltas();
    }
    assert!(passes >= 3);
    // A permanently blocked backend must not block any of these synchronous UI calls.
    state.set_focus_tree();
    state.handle_tree_key(FolderPaneTreeKey::Down);
    state.suspend_display();
    state.select_drive(p(r"D:\"));
    assert!(!state.has_pending());
    release.wait();
}

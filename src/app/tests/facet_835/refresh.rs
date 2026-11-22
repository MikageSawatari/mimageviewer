use super::*;

fn synthetic_notifications(h: &mut Harness) {
    // As in section375_tests: verify real registration, then isolate delivery
    // from late OS notifications which could extend the synthetic debounce.
    assert!(
        h.env
            .current_folder_watch
            .as_ref()
            .unwrap()
            .watcher
            .is_some()
    );
    let watch = h.env.current_folder_watch.as_mut().unwrap();
    watch.watcher = None;
    let (tx, rx) = crossbeam_channel::bounded(256);
    watch.tx = tx;
    watch.rx = rx;
    watch.dirty.clear();
    watch.debounce_until = None;
}

fn notify(h: &mut Harness, path: PathBuf, kind: notify::EventKind) {
    let watch = h.env.current_folder_watch.as_mut().unwrap();
    let sequence = watch
        .sequence
        .fetch_add(1, Ordering::AcqRel)
        .wrapping_add(1);
    watch
        .tx
        .send((sequence, Ok(notify::Event::new(kind).add_path(path))))
        .unwrap();
}

#[test]
fn facet_835_t8_notifications_add_and_delete_with_active_facet() {
    let (mut h, folder) = image_fixture();
    h.env.settings.facet_filter.exts.insert("png".into());
    h.env.rebuild_visible_indices();
    h.settle();
    synthetic_notifications(&mut h);
    let generation = h.env.items_generation;
    let added = folder.join("D.png");
    std::fs::write(&added, png_bytes("wanted")).unwrap();
    std::fs::write(folder.join("excluded.jpg"), png_bytes("other")).unwrap();
    h.stage = "T8: notified addition with PNG facet".into();
    notify(
        &mut h,
        added.clone(),
        notify::EventKind::Create(notify::event::CreateKind::File),
    );
    h.until(
        |h| {
            h.idle()
                && h.env.items_generation > generation
                && h.env.items.len() == 5
                && h.visible_names() == ["A.png", "B.png", "C.png", "D.png"]
        },
        |_| {},
    );
    // The actual debounce is driven by full frames; no product clock is changed.
    if h.env
        .current_folder_watch
        .as_ref()
        .unwrap()
        .watcher
        .is_some()
    {
        // A full adoption may have registered a new physical subscription.
        synthetic_notifications(&mut h);
    }
    let generation = h.env.items_generation;
    let removed = folder.join("A.png");
    std::fs::remove_file(&removed).unwrap();
    h.stage = "T8: notified deletion with PNG facet".into();
    notify(
        &mut h,
        removed,
        notify::EventKind::Remove(notify::event::RemoveKind::File),
    );
    h.until(
        |h| {
            h.idle()
                && h.env.items_generation > generation
                && h.env.items.len() == 4
                && h.visible_names() == ["B.png", "C.png", "D.png"]
        },
        |_| {},
    );
    assert!(h.env.settings.facet_filter.exts.contains("png"));
}

fn hold_rescan(
    h: &mut Harness,
    folder: &Path,
    different: bool,
) -> (
    mpsc::Sender<Option<ExternalRescanResult>>,
    Option<ExternalRescanResult>,
) {
    h.env
        .start_external_rescan(&h.ctx, folder.to_path_buf(), SystemTime::UNIX_EPOCH);
    let pending = h
        .env
        .external_rescan_pending
        .as_mut()
        .expect("real rescan owner");
    let result = pending
        .rx
        .recv_timeout(DEADLINE)
        .expect("real completed scan");
    assert!(
        if different {
            matches!(result, Some(ExternalRescanResult::Different(..)))
        } else {
            matches!(result, Some(ExternalRescanResult::Equal(..)))
        },
        "wrong real worker result"
    );
    let (tx, rx) = mpsc::channel();
    pending.rx = rx;
    // Hold the actual materialized payload outside its delivery channel; do not
    // synthesize an Equal/Different proof or reconstruct the pending request.
    // The caller releases via the returned sender after changing the facet.
    (tx, result)
}

#[test]
fn facet_835_t8_equal_started_before_facet_change_is_accepted() {
    let (mut h, folder) = image_fixture();
    synthetic_notifications(&mut h);
    let generation = h.env.items_generation;
    let rows = Arc::clone(&h.env.folder_observation.as_ref().unwrap().rows);
    // No App frame/poll occurs between capturing the result and changing facet.
    let (delivery, result) = hold_rescan(&mut h, &folder, false);
    h.env.settings.facet_filter.exts.insert("jpg".into());
    h.env.rebuild_visible_indices();
    delivery.send(result).unwrap();
    h.env.poll_external_rescan(&h.ctx);
    assert!(
        !Arc::ptr_eq(&rows, &h.env.folder_observation.as_ref().unwrap().rows),
        "Equal was rejected after a display-only facet change"
    );
    assert_eq!(h.env.items_generation, generation);
    assert!(
        h.visible_names().is_empty(),
        "accepted Equal must keep the current facet projection"
    );
    h.until(Harness::idle, |h| {
        assert_eq!(h.env.items_generation, generation);
        assert!(h.visible_names().is_empty());
    });
}

#[test]
fn facet_835_t8_different_started_before_facet_change_uses_current_facet() {
    let (mut h, folder) = image_fixture();
    synthetic_notifications(&mut h);
    let generation = h.env.items_generation;
    std::fs::write(folder.join("D.jpg"), png_bytes("wanted")).unwrap();
    let (delivery, result) = hold_rescan(&mut h, &folder, true);
    h.env.settings.facet_filter.exts.insert("jpg".into());
    h.env.rebuild_visible_indices();
    delivery.send(result).unwrap();
    h.env.poll_external_rescan(&h.ctx);
    assert!(
        h.env.folder_refresh_pending.is_none(),
        "Different was rejected and deferred, rather than accepted"
    );
    assert!(
        h.env.items_generation > generation
            || h.env.sidecar_restore_active()
            || h.env.direct_navigation_scan_pending(),
        "accepted Different must begin adoption without another rescan"
    );
    h.until(|h| h.idle() && h.env.items.len() == 4, |_| {});
    assert_eq!(
        h.visible_names(),
        ["D.jpg"],
        "Different must materialize using the new facet"
    );
    assert_eq!(
        h.env.items_generation,
        generation.wrapping_add(1),
        "one accepted scan installs once"
    );
}

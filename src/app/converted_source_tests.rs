//! Handler-level coverage for source decisions and late thumbnail adoption (§1.803).
use super::*;
use crate::thumb_loader::{ConvertedSourceBinding, ThumbLoadOrigin};

fn key(path: &Path) -> String {
    crate::path_key::normalize_keep_drive(path)
}

fn binding(source: &Path, read: &Path) -> ConvertedSourceBinding {
    ConvertedSourceBinding {
        archive_key: key(source),
        read_archive: key(read),
    }
}

fn candidate_item(path: &Path) -> GridItem {
    GridItem::ConvertibleArchive {
        path: path.to_path_buf(),
        format: crate::archive_converter::ArchiveFormat::Rar,
    }
}

fn install_rows(app: &mut App, items: Vec<GridItem>) {
    app.thumbnails = vec![ThumbnailState::Pending; items.len()];
    app.image_metas = vec![None; items.len()];
    app.keep_set = (0..items.len()).collect();
    app.items = items;
    app.converted_archive_cache_paths =
        ConvertedArchiveSources::new(app.archive_cache_db.as_ref().map_or(0, |db| db.revision()));
}

fn decision_batch(app: &mut App) -> (mpsc::Sender<ConvertedArchiveCachePathsMsg>, Arc<AtomicBool>) {
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    app.converted_archive_cache_paths_pending = Some(ConvertedArchiveCachePathsPending {
        generation: app.items_generation,
        cascade_depth: app.settings.folder_thumb_depth as usize,
        cancel: Arc::clone(&cancel),
        desired_indices: Arc::new(std::sync::RwLock::new((0..app.items.len()).collect())),
        rx,
        pin_archive_dependencies: HashMap::new(),
    });
    (tx, cancel)
}

fn resolved(source: &Path, state: ConvertedArchiveSourceState) -> ConvertedArchiveCachePathsMsg {
    ConvertedArchiveCachePathsMsg::Resolved {
        archive_key: key(source),
        state,
        idx: 0,
        ordinal: 0,
        elapsed_ms: 0.0,
        input_seq: 0,
    }
}

fn cached(source: &Path, read: &Path) -> ConvertedArchiveSourceState {
    ConvertedArchiveSourceState::CachedZip {
        logical_source: source.to_path_buf(),
        path: read.to_path_buf(),
    }
}

fn record(db: &crate::archive_cache::ArchiveCacheDb, source: &Path, read: &Path) {
    db.record(
        source,
        1,
        2,
        crate::archive_converter::ArchiveFormat::Rar,
        read,
        3,
        1,
        true,
    )
    .unwrap();
}

fn loaded(
    ctx: &egui::Context,
    name: &str,
    source_binding: Option<ConvertedSourceBinding>,
) -> ThumbnailState {
    let tex = ctx.load_texture(
        name,
        egui::ColorImage::filled([2, 2], egui::Color32::WHITE),
        Default::default(),
    );
    let mut state = ThumbnailState::loaded_raster(
        tex,
        ThumbLoadOrigin::FinalCache,
        false,
        2,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((2, 2)),
        None,
    );
    if let ThumbnailState::Loaded {
        converted_source_binding,
        ..
    } = &mut state
    {
        *converted_source_binding = source_binding;
    }
    state
}

fn pixels(
    app: &mut App,
    idx: usize,
    source_binding: Option<ConvertedSourceBinding>,
    origin: ThumbLoadOrigin,
    kind: ThumbnailRequestKind,
) -> ThumbMsg {
    let id = app
        .requested
        .begin(idx, app.items_generation, kind, None)
        .unwrap();
    let msg = ThumbMsg::from_legacy_parts(
        idx,
        Some(egui::ColorImage::filled([2, 2], egui::Color32::GRAY)),
        origin,
        false,
        None,
        Some((2, 2)),
        None,
        false,
        false,
        0,
        app.items_generation,
    )
    .with_request_id(id)
    .with_converted_source_binding(source_binding);
    if origin.from_cache() {
        msg
    } else {
        msg.source_pixels()
    }
}

#[test]
fn converted_sources_resolution_batch_traverses_thumbnails_once() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let sources: Vec<_> = (0..8)
        .map(|i| app.tmp.path().join(format!("book-{i}.rar")))
        .collect();
    let cached_paths: Vec<_> = (0..8)
        .map(|i| app.tmp.path().join(format!("book-{i}.zip")))
        .collect();
    // Most rows have no binding, as during initial navigation in a large search list.
    install_rows(&mut app, vec![candidate_item(&sources[0]); 513]);
    for source in &sources {
        app.converted_archive_cache_paths
            .insert(key(source), ConvertedArchiveSourceState::Pending);
    }
    app.thumbnails[400] = loaded(
        &ctx,
        "stale-sibling",
        Some(binding(&sources[7], &sources[7])),
    );
    app.thumbnails[401] = loaded(
        &ctx,
        "matching-sibling",
        Some(binding(&sources[7], &cached_paths[7])),
    );
    let retained = match &app.thumbnails[401] {
        ThumbnailState::Loaded { tex, .. } => tex.id(),
        _ => unreachable!(),
    };
    let (tx, _) = decision_batch(&mut app);
    for (source, cached_path) in sources.iter().zip(&cached_paths) {
        tx.send(resolved(source, cached(source, cached_path)))
            .unwrap();
    }
    tx.send(ConvertedArchiveCachePathsMsg::Finished {
        peeked: 8,
        hits: 8,
        elapsed_ms: 0.0,
        input_seq: 0,
    })
    .unwrap();
    converted_archive_sources::take_thumbnail_visits();
    app.poll_converted_archive_cache_paths(&ctx);
    assert_eq!(
        converted_archive_sources::take_thumbnail_visits(),
        app.thumbnails.len(),
        "one full-list traversal per drain, independent of reply count"
    );
    assert!(matches!(app.thumbnails[400], ThumbnailState::Evicted));
    assert!(
        matches!(&app.thumbnails[401], ThumbnailState::Loaded { tex, .. } if tex.id() == retained)
    );
    app.poll_converted_archive_cache_paths(&ctx);
    assert_eq!(converted_archive_sources::take_thumbnail_visits(), 0);
}

#[test]
fn converted_sources_catch_up_batches_history_before_single_state_traversal() {
    let app = setup_app_for_test();
    let db = app.archive_cache_db.as_ref().unwrap().clone();
    let source = app.tmp.path().join("book.rar");
    let read = app.tmp.path().join("cache.zip");
    let unrelated = app.tmp.path().join("unrelated.rar");
    let mut table = ConvertedArchiveSources::new(db.revision());
    table.insert(
        key(&source),
        ConvertedArchiveSourceState::Direct(source.clone()),
    );
    table.insert("alias.rar".into(), cached(&source, &read));
    table.insert(
        key(&unrelated),
        ConvertedArchiveSourceState::Direct(unrelated.clone()),
    );
    for _ in 0..64 {
        record(&db, &source, &read);
    }
    converted_archive_sources::take_source_state_visits();
    assert!(table.catch_up(Some(&db)));
    assert_eq!(
        converted_archive_sources::take_source_state_visits(),
        table.len(),
        "change history must be deduplicated before traversing states"
    );
    assert_eq!(
        table.get(&key(&source)),
        Some(&ConvertedArchiveSourceState::Pending)
    );
    assert_eq!(
        table.get("alias.rar"),
        Some(&ConvertedArchiveSourceState::Pending)
    );
    assert_eq!(
        table.get(&key(&unrelated)),
        Some(&ConvertedArchiveSourceState::Direct(unrelated))
    );
    assert!(!table.catch_up(Some(&db)));
    assert_eq!(converted_archive_sources::take_source_state_visits(), 0);
}

#[test]
fn converted_sources_record_cancels_pending_batch_before_queued_direct_reply() {
    let mut app = setup_app_for_test();
    let source = app.tmp.path().join("book.rar");
    let read = app.tmp.path().join("book.zip");
    install_rows(&mut app, vec![candidate_item(&source)]);
    app.converted_archive_cache_paths
        .insert(key(&source), ConvertedArchiveSourceState::Pending);
    let (tx, cancel) = decision_batch(&mut app);
    tx.send(resolved(
        &source,
        ConvertedArchiveSourceState::Direct(source.clone()),
    ))
    .unwrap();
    record(app.archive_cache_db.as_ref().unwrap(), &source, &read);

    app.poll_converted_archive_cache_paths(&egui::Context::default());
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.converted_archive_cache_paths_pending.is_none());
    assert_eq!(
        app.converted_archive_cache_paths.get(&key(&source)),
        Some(&ConvertedArchiveSourceState::Pending)
    );
    assert!(tx.send(resolved(&source, cached(&source, &read))).is_err());

    let (current, _) = decision_batch(&mut app);
    current
        .send(resolved(&source, cached(&source, &read)))
        .unwrap();
    app.poll_converted_archive_cache_paths(&egui::Context::default());
    assert_eq!(
        app.converted_archive_cache_paths.get(&key(&source)),
        Some(&cached(&source, &read))
    );
}

#[test]
fn converted_sources_clear_cancels_pending_batch_before_queued_cached_reply() {
    let mut app = setup_app_for_test();
    let source = app.tmp.path().join("book.rar");
    let read = app.tmp.path().join("book.zip");
    record(app.archive_cache_db.as_ref().unwrap(), &source, &read);
    install_rows(&mut app, vec![candidate_item(&source)]);
    app.converted_archive_cache_paths
        .insert(key(&source), ConvertedArchiveSourceState::Pending);
    let (tx, cancel) = decision_batch(&mut app);
    tx.send(resolved(&source, cached(&source, &read))).unwrap();
    assert_eq!(
        app.archive_cache_db.as_ref().unwrap().clear_all().unwrap(),
        1
    );

    app.poll_converted_archive_cache_paths(&egui::Context::default());
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.converted_archive_cache_paths_pending.is_none());
    assert_eq!(
        app.converted_archive_cache_paths.get(&key(&source)),
        Some(&ConvertedArchiveSourceState::Pending)
    );
}

#[test]
fn converted_sources_pin_root_keeps_missing_candidates_when_next_message_is_canceled() {
    let mut app = setup_app_for_test();
    let source = app.tmp.path().join("missing.rar");
    let retained = app.tmp.path().join("retained.rar");
    let unrelated = app.tmp.path().join("unrelated.rar");
    let read = app.tmp.path().join("unrelated.zip");
    let folder = app.tmp.path().join("folder");
    install_rows(&mut app, vec![GridItem::Folder(folder)]);
    app.converted_archive_cache_paths.insert(
        key(&retained),
        ConvertedArchiveSourceState::Direct(retained.clone()),
    );
    let (tx, cancel) = decision_batch(&mut app);
    let root_key = "root".to_string();
    let depth = app.settings.folder_thumb_depth as usize;
    tx.send(ConvertedArchiveCachePathsMsg::PinRootResolved {
        root_key: root_key.clone(),
        cascade_depth: depth,
        candidates: [&source, &retained]
            .into_iter()
            .map(|path| ConvertedArchivePinCandidate {
                archive_key: key(path),
                path: path.to_path_buf(),
                mtime: 1,
                file_size: 2,
            })
            .collect(),
    })
    .unwrap();
    app.poll_converted_archive_cache_paths(&egui::Context::default());
    assert_eq!(
        app.converted_archive_cache_paths.get(&key(&source)),
        Some(&ConvertedArchiveSourceState::Pending)
    );
    assert_eq!(
        app.converted_archive_cache_paths.get(&key(&retained)),
        Some(&ConvertedArchiveSourceState::Direct(retained.clone()))
    );
    tx.send(ConvertedArchiveCachePathsMsg::PinDependency {
        idx: 0,
        archive_key: key(&source),
    })
    .unwrap();
    record(app.archive_cache_db.as_ref().unwrap(), &unrelated, &read);
    app.poll_converted_archive_cache_paths(&egui::Context::default());

    assert!(cancel.load(Ordering::Relaxed));
    assert!(
        matches!(app.converted_archive_pin_root_states.get(&root_key), Some(ConvertedArchivePinRootState::Resolved { candidates, .. }) if candidates.len() == 2)
    );
    assert_eq!(
        app.converted_archive_cache_paths.get(&key(&source)),
        Some(&ConvertedArchiveSourceState::Pending)
    );
    assert_eq!(
        app.converted_archive_cache_paths.get(&key(&retained)),
        Some(&ConvertedArchiveSourceState::Direct(retained))
    );
}

#[test]
fn converted_sources_unknown_logical_source_retries_recorded_key_and_accepts_cached_zip() {
    let mut app = setup_app_for_test();
    let source = app.tmp.path().join("encrypted.rar");
    let read = app.tmp.path().join("decrypted.zip");
    install_rows(&mut app, vec![candidate_item(&source)]);
    app.converted_archive_cache_paths.insert(
        key(&source),
        ConvertedArchiveSourceState::Unavailable {
            logical_source: None,
        },
    );
    record(app.archive_cache_db.as_ref().unwrap(), &source, &read);
    app.poll_converted_archive_cache_paths(&egui::Context::default());
    assert_eq!(
        app.converted_archive_cache_paths.get(&key(&source)),
        Some(&ConvertedArchiveSourceState::Pending)
    );
    let (tx, _) = decision_batch(&mut app);
    tx.send(resolved(&source, cached(&source, &read))).unwrap();
    app.poll_converted_archive_cache_paths(&egui::Context::default());
    assert_eq!(
        app.converted_archive_cache_paths
            .get(&key(&source))
            .and_then(ConvertedArchiveSourceState::load_path),
        Some(read.as_path())
    );
}

#[test]
fn converted_sources_resolution_reconciles_all_bound_siblings_and_preserves_matching_materials() {
    for to_cached in [true, false] {
        for candidate_matches in [true, false] {
            let mut app = setup_app_for_test();
            let ctx = egui::Context::default();
            let source = app.tmp.path().join("book.rar");
            let zip = app.tmp.path().join("book.zip");
            let other = app.tmp.path().join("other.rar");
            let (old, new, state) = if to_cached {
                (&source, &zip, cached(&source, &zip))
            } else {
                (
                    &zip,
                    &source,
                    ConvertedArchiveSourceState::Direct(source.clone()),
                )
            };
            let folder = app.tmp.path().join("pin");
            let items = vec![
                candidate_item(&source),
                GridItem::ZipDir {
                    zip_path: source.clone(),
                    dir_prefix: "one/".into(),
                    is_archive: false,
                    representative: Some("one/cover.png".into()),
                },
                GridItem::ZipDir {
                    zip_path: source.clone(),
                    dir_prefix: "two/".into(),
                    is_archive: false,
                    representative: Some("two/cover.png".into()),
                },
                GridItem::Folder(folder.clone()),
                GridItem::Folder(folder),
                candidate_item(&other),
            ];
            install_rows(&mut app, items);
            app.converted_archive_cache_paths
                .insert(key(&source), ConvertedArchiveSourceState::Pending);
            app.thumbnails = [
                Some(binding(&source, if candidate_matches { new } else { old })),
                Some(binding(&source, old)),
                Some(binding(&source, new)),
                Some(binding(&source, new)),
                None, // A pin that actually used an ordinary-folder representative.
                Some(binding(&other, old)),
            ]
            .into_iter()
            .enumerate()
            .map(|(idx, bound)| loaded(&ctx, &format!("bound-{idx}"), bound))
            .collect();
            let texture_ids: Vec<_> = app
                .thumbnails
                .iter()
                .map(|thumb| match thumb {
                    ThumbnailState::Loaded { tex, .. } => tex.id(),
                    _ => unreachable!(),
                })
                .collect();
            let (tx, _) = decision_batch(&mut app);
            tx.send(ConvertedArchiveCachePathsMsg::PinDependency {
                idx: 3,
                archive_key: key(&source),
            })
            .unwrap();
            tx.send(resolved(&source, state)).unwrap();
            app.poll_converted_archive_cache_paths(&ctx);

            for idx in [1, 3] {
                assert!(matches!(app.thumbnails[idx], ThumbnailState::Evicted));
            }
            if candidate_matches {
                assert!(
                    matches!(&app.thumbnails[0], ThumbnailState::Loaded { tex, .. } if tex.id() == texture_ids[0])
                );
            } else {
                assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
            }
            for idx in [2, 4, 5] {
                assert!(
                    matches!(&app.thumbnails[idx], ThumbnailState::Loaded { tex, .. } if tex.id() == texture_ids[idx])
                );
            }
        }
    }
}

#[test]
fn converted_sources_late_worker_and_idle_upgrade_pixels_cannot_replace_current_material() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let source = app.tmp.path().join("book.rar");
    let read = app.tmp.path().join("book.zip");
    for to_cached in [true, false] {
        install_rows(&mut app, vec![candidate_item(&source); 3]);
        let (old_read, current_read, state) = if to_cached {
            (&source, &read, cached(&source, &read))
        } else {
            (
                &read,
                &source,
                ConvertedArchiveSourceState::Direct(source.clone()),
            )
        };
        app.converted_archive_cache_paths
            .insert(key(&source), state);
        for (idx, kind, origin) in [
            (0, ThumbnailRequestKind::Normal, ThumbLoadOrigin::FinalCache),
            (
                1,
                ThumbnailRequestKind::Normal,
                ThumbLoadOrigin::SourceIntrinsic,
            ),
            (
                2,
                ThumbnailRequestKind::Upgrade,
                ThumbLoadOrigin::SourceIntrinsic,
            ),
        ] {
            app.thumbnails[idx] = loaded(&ctx, "current", Some(binding(&source, current_read)));
            app.idle_upgrade_cache_bypass_ineligible.insert(idx);
            let message = pixels(
                &mut app,
                idx,
                Some(binding(&source, old_read)),
                origin,
                kind,
            );
            app.tx.send(message).unwrap();
        }
        app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
        for idx in 0..3 {
            assert!(matches!(app.thumbnails[idx], ThumbnailState::Evicted));
            assert!(!app.requested.contains_key(&idx));
            assert!(!app.idle_upgrade_cache_bypass_ineligible.contains(&idx));
        }
        assert_eq!(app.requested.images_len(), 0);
    }
}

#[test]
fn converted_sources_cpu_image_queue_rechecks_source_at_adoption() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let source = app.tmp.path().join("book.rar");
    let read = app.tmp.path().join("book.zip");
    install_rows(&mut app, vec![candidate_item(&source); 2]);
    app.converted_archive_cache_paths.insert(
        key(&source),
        ConvertedArchiveSourceState::Direct(source.clone()),
    );
    let stale = pixels(
        &mut app,
        0,
        Some(binding(&source, &source)),
        ThumbLoadOrigin::FinalCache,
        ThumbnailRequestKind::Normal,
    );
    let matching = pixels(
        &mut app,
        1,
        Some(binding(&source, &read)),
        ThumbLoadOrigin::FinalCache,
        ThumbnailRequestKind::Normal,
    );
    assert!(matches!(
        app.requested.receive(stale),
        ThumbnailReceipt::Queued
    ));
    assert!(matches!(
        app.requested.receive(matching),
        ThumbnailReceipt::Queued
    ));
    assert_eq!(app.requested.images_len(), 2);
    app.converted_archive_cache_paths
        .insert(key(&source), cached(&source, &read));

    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
    assert!(!app.requested.contains_key(&0));
    assert!(
        matches!(&app.thumbnails[1], ThumbnailState::Loaded { converted_source_binding: Some(bound), .. } if bound == &binding(&source, &read))
    );
    assert_eq!(app.requested.images_len(), 0);
}

#[test]
fn converted_sources_pending_still_adopts_cached_pixels_until_source_is_confirmed() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let source = app.tmp.path().join("book.rar");
    let read = app.tmp.path().join("book.zip");
    install_rows(&mut app, vec![candidate_item(&source)]);
    app.converted_archive_cache_paths
        .insert(key(&source), ConvertedArchiveSourceState::Pending);
    let message = pixels(
        &mut app,
        0,
        Some(binding(&source, &source)),
        ThumbLoadOrigin::FinalCache,
        ThumbnailRequestKind::Normal,
    );
    app.tx.send(message).unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert!(matches!(app.thumbnails[0], ThumbnailState::Loaded { .. }));

    let (tx, _) = decision_batch(&mut app);
    tx.send(resolved(&source, cached(&source, &read))).unwrap();
    app.poll_converted_archive_cache_paths(&ctx);
    assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
}

#[test]
fn converted_sources_restore_retains_matching_pending_and_legacy_controls() {
    use crate::cover_source_scope::{CoverSourceScope, LoadedCoverIdentity};
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let source = app.tmp.path().join("resolved.rar");
    let pending = app.tmp.path().join("pending.rar");
    let read = app.tmp.path().join("resolved.zip");
    install_rows(&mut app, vec![candidate_item(&source); 5]);
    app.items[2] = candidate_item(&pending);
    app.converted_archive_cache_paths
        .insert(key(&source), cached(&source, &read));
    app.converted_archive_cache_paths
        .insert(key(&pending), ConvertedArchiveSourceState::Pending);
    app.thumbnails = vec![
        loaded(&ctx, "stale", Some(binding(&source, &source))),
        loaded(&ctx, "matching", Some(binding(&source, &read))),
        loaded(&ctx, "pending", Some(binding(&pending, &source))),
        loaded(&ctx, "legacy", None),
        loaded(&ctx, "fallback", None),
    ];
    if let ThumbnailState::Loaded { cover, .. } = &mut app.thumbnails[3] {
        *cover = LoadedCoverIdentity::Legacy {
            scope: Some(CoverSourceScope::archive(&source)),
        };
    }
    let retained_ids: Vec<_> = app
        .thumbnails
        .iter()
        .skip(1)
        .map(|state| match state {
            ThumbnailState::Loaded { tex, .. } => tex.id(),
            _ => unreachable!(),
        })
        .collect();
    assert!(app.reconcile_restored_cover_thumbnails());
    assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
    for (idx, expected) in (1..5).zip(retained_ids) {
        assert!(
            matches!(&app.thumbnails[idx], ThumbnailState::Loaded { tex, .. } if tex.id() == expected)
        );
    }
    assert!(!app.reconcile_restored_cover_thumbnails());
}

#[cfg(windows)]
#[test]
fn converted_sources_context_swap_moves_revision_and_batch_without_touching_parked_sibling() {
    let mut app = setup_app_for_test();
    let ctx = egui::Context::default();
    let source = app.tmp.path().join("shared.rar");
    let read = app.tmp.path().join("shared.zip");
    let db = app.archive_cache_db.clone().unwrap();
    record(
        &db,
        &app.tmp.path().join("earlier.rar"),
        &app.tmp.path().join("earlier.zip"),
    );
    let original_revision = db.revision();
    assert!(original_revision > 0);
    let mut target_batch = None;
    let target = app.build_window_context_for_test(18031, |owner| {
        install_rows(owner, vec![candidate_item(&source)]);
        owner.converted_archive_cache_paths.insert(
            key(&source),
            ConvertedArchiveSourceState::Direct(source.clone()),
        );
        owner.thumbnails[0] = loaded(&ctx, "target-context", Some(binding(&source, &source)));
        target_batch = Some(decision_batch(owner));
    });
    let (target_tx, target_cancel) = target_batch.unwrap();
    let mut sibling_batch = None;
    let sibling = app.build_window_context_for_test(18032, |owner| {
        install_rows(owner, vec![candidate_item(&source)]);
        owner.converted_archive_cache_paths.insert(
            key(&source),
            ConvertedArchiveSourceState::Direct(source.clone()),
        );
        owner.thumbnails[0] = loaded(&ctx, "sibling-context", Some(binding(&source, &source)));
        sibling_batch = Some(decision_batch(owner));
    });
    let (sibling_tx, sibling_cancel) = sibling_batch.unwrap();
    // This uses the registry's real bundle exchange, with no DB change to hide a lost revision.
    app.with_viewer_context_state(target, |owner| {
        assert_eq!(
            owner.converted_archive_cache_paths.synced_revision(),
            original_revision
        );
        assert_eq!(
            owner.converted_archive_cache_paths.get(&key(&source)),
            Some(&ConvertedArchiveSourceState::Direct(source.clone()))
        );
        assert!(Arc::ptr_eq(
            &owner
                .converted_archive_cache_paths_pending
                .as_ref()
                .unwrap()
                .cancel,
            &target_cancel
        ));
        assert!(!target_cancel.load(Ordering::Relaxed));
    })
    .unwrap();
    let sibling_texture = app
        .with_viewer_context_ref(sibling, |owner| {
            match &owner.texture_caches().thumbnails[0] {
                ThumbnailState::Loaded { tex, .. } => tex.id(),
                _ => unreachable!(),
            }
        })
        .unwrap();
    target_tx
        .send(resolved(
            &source,
            ConvertedArchiveSourceState::Direct(source.clone()),
        ))
        .unwrap();
    sibling_tx
        .send(resolved(
            &source,
            ConvertedArchiveSourceState::Direct(source.clone()),
        ))
        .unwrap();
    record(&db, &source, &read);
    assert!(!target_cancel.load(Ordering::Relaxed));
    assert!(!sibling_cancel.load(Ordering::Relaxed));

    let mut replacement_cancel = None;
    app.with_viewer_context_state(target, |owner| {
        assert!(target_cancel.load(Ordering::Relaxed));
        assert!(owner.converted_archive_cache_paths_pending.is_none());
        assert_eq!(
            owner.converted_archive_cache_paths.synced_revision(),
            db.revision()
        );
        assert_eq!(
            owner.converted_archive_cache_paths.get(&key(&source)),
            Some(&ConvertedArchiveSourceState::Pending)
        );
        owner.poll_converted_archive_cache_paths(&ctx);
        assert_eq!(
            owner.converted_archive_cache_paths.get(&key(&source)),
            Some(&ConvertedArchiveSourceState::Pending)
        );
        assert!(
            target_tx
                .send(resolved(&source, cached(&source, &read)))
                .is_err()
        );
        let (tx, cancel) = decision_batch(owner);
        tx.send(resolved(&source, cached(&source, &read))).unwrap();
        owner.poll_converted_archive_cache_paths(&ctx);
        assert!(matches!(owner.thumbnails[0], ThumbnailState::Evicted));
        assert!(!cancel.load(Ordering::Relaxed));
        // Keep the new batch open to prove a repeated no-change mount does not cancel it.
        replacement_cancel = Some((tx, cancel));
    })
    .unwrap();
    assert!(
        !sibling_cancel.load(Ordering::Relaxed),
        "catch-up must not broadcast cancellation to parked contexts"
    );
    app.with_viewer_context_ref(sibling, |owner| {
        assert!(matches!(&owner.texture_caches().thumbnails[0], ThumbnailState::Loaded { tex, .. } if tex.id() == sibling_texture));
    }).unwrap();
    let (_replacement_tx, replacement_cancel) = replacement_cancel.unwrap();
    app.with_viewer_context_state(target, |owner| {
        assert_eq!(
            owner.converted_archive_cache_paths.get(&key(&source)),
            Some(&cached(&source, &read))
        );
        assert_eq!(
            owner.converted_archive_cache_paths.synced_revision(),
            db.revision()
        );
        assert!(Arc::ptr_eq(
            &owner
                .converted_archive_cache_paths_pending
                .as_ref()
                .unwrap()
                .cancel,
            &replacement_cancel
        ));
        assert!(!replacement_cancel.load(Ordering::Relaxed));
    })
    .unwrap();
    app.with_viewer_context_state(sibling, |owner| {
        assert!(sibling_cancel.load(Ordering::Relaxed));
        assert!(owner.converted_archive_cache_paths_pending.is_none());
        assert_eq!(owner.converted_archive_cache_paths.synced_revision(), db.revision());
        assert_eq!(owner.converted_archive_cache_paths.get(&key(&source)), Some(&ConvertedArchiveSourceState::Pending));
        assert!(matches!(&owner.thumbnails[0], ThumbnailState::Loaded { tex, .. } if tex.id() == sibling_texture));
        owner.poll_converted_archive_cache_paths(&ctx);
        assert_eq!(owner.converted_archive_cache_paths.get(&key(&source)), Some(&ConvertedArchiveSourceState::Pending));
        assert!(sibling_tx.send(resolved(&source, cached(&source, &read))).is_err());
    }).unwrap();
    assert!(!replacement_cancel.load(Ordering::Relaxed));
}

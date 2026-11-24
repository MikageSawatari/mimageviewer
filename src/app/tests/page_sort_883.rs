//! Section 1.883 stage 2: whole-listing page-count demand and its ownership.
use super::*;
use crate::settings::{
    GridViewMode, LazyMetric, ListingPageCount, SelectionInfoDisplayMode, SortOrder,
};

fn fixture() -> phase_c_support::AppTestEnv {
    let mut app = phase_c_support::setup_app();
    app.settings.grid_view_mode = GridViewMode::Thumbnail;
    app.settings.sort_order = SortOrder::PageCountAsc;
    app.settings.thumb_show_page_count = false;
    app.settings.thumb_show_media_duration = false;
    app.settings.thumb_show_resume_meter = false;
    app.settings.selection_info_display_mode = SelectionInfoDisplayMode::Hidden;
    app.settings.details_show_page_count = false;
    app.settings.details_show_created = false;
    app.settings.details_show_image_dimensions = false;
    app.settings.details_show_video_duration = false;
    app.settings.details_show_video_dimensions = false;
    app.settings.details_show_video_codec = false;
    app.settings.facet_filter = Default::default();
    let root = app.tmp.path().join("uncreated-page-sort-sources");
    app.install_new_items(
        vec![
            GridItem::ZipFile(root.join("book.zip")),
            GridItem::PdfFile(root.join("book.pdf")),
            GridItem::Folder(root.join("photos")),
            GridItem::ConvertibleArchive {
                path: root.join("book.rar"),
                format: crate::archive_converter::ArchiveFormat::Rar,
            },
            GridItem::ConvertibleArchive {
                path: root.join("book.7z"),
                format: crate::archive_converter::ArchiveFormat::SevenZ,
            },
            GridItem::Video(root.join("video.mp4")),
            GridItem::Audio(root.join("audio.flac")),
            GridItem::Image(root.join("image.png")),
            GridItem::PdfFile(root.join("book.epub")),
        ],
        vec![Some((100, 2048)); 9],
    );
    app.image_metas[2] = Some((100, 0));
    app.ai_model_facet_requested = false;
    app.duration_facet_requested = false;
    app.selected = None;
    app.visible_indices = vec![0];
    app.details_tag_prewarm_indices = vec![0];
    app.keep_set = HashSet::from([0]);
    app
}

fn fingerprint(app: &App) -> i64 {
    folder_scan::image_page_recognition_fingerprint(&app.settings)
}

fn cache_pages(app: &mut App, idx: usize, count: Option<u32>, failed: bool) {
    let key = app.details_lazy_cache_key(idx).unwrap();
    let (source_mtime, source_size) = app.image_metas[idx].unwrap();
    let password_revision = match &app.items[idx] {
        GridItem::PdfFile(path) => Some(app.pdf_passwords.credential_revision(path)),
        _ => None,
    };
    let page_count_generation = (idx == 8).then_some(app.items_generation);
    let page_count_fingerprint = fingerprint(app);
    app.details_lazy_meta.insert(
        key,
        DetailsLazyMeta {
            source_mtime,
            source_size,
            page_count: count,
            page_count_checked: true,
            page_count_failed: failed,
            page_count_fingerprint,
            page_count_generation,
            page_count_pdf_password_revision: password_revision,
            ..Default::default()
        },
    );
}

fn cache_all(app: &mut App) {
    for idx in [0, 1, 2, 3, 8] {
        cache_pages(app, idx, Some((idx as u32 + 1) * 10), false);
    }
}

fn pending(
    app: &mut App,
    scope: DetailsMetaScanScope,
    stale_pool: bool,
) -> (Arc<AtomicBool>, mpsc::Sender<DetailsMetaEvent>) {
    let (tx, rx) = mpsc::channel();
    let (priority_tx, _priority_rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    app.details_meta_pending = Some(DetailsMetaPending {
        pool_revision: crate::susie_loader::pool_revision().wrapping_add(u64::from(stale_pool)),
        scan_scope: scope,
        listing_meta_scan: (scope == DetailsMetaScanScope::AllRequirements)
            .then(|| app.listing_meta_scan_snapshot()),
        visible_revision: app.details_lazy_visible_revision,
        selection_target_key: None,
        normal_target_keys: HashSet::new(),
        cancel: Arc::clone(&cancel),
        phase: DetailsMetaPendingPhase::Loading { rx, priority_tx },
    });
    app.details_image_dims_state = LazyColumnState::Loading { done: 0, total: 5 };
    (cancel, tx)
}

#[test]
fn page_sort_883_whole_worker_counts_hidden_books_with_all_page_displays_off() {
    use std::io::Write;
    let mut app = fixture();
    assert!(crate::catalog::default_cache_dir().starts_with(app.tmp.path()));
    let mut items = Vec::new();
    let mut metas = Vec::new();
    for (name, count) in [("visible.zip", 2), ("hidden.zip", 3)] {
        let path = app.tmp.path().join(name);
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for page in 0..count {
            zip.start_file(
                format!("{page:03}.jpg"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(b"count only; no image decoding").unwrap();
        }
        zip.finish().unwrap();
        metas.push(crate::catalog::media_source_identity(&path));
        items.push(GridItem::ZipFile(path));
    }
    let video = app.tmp.path().join("unprobed.mp4");
    items.push(GridItem::Video(video));
    metas.push(Some((100, 100)));
    app.install_new_items(items, metas);
    app.visible_indices = vec![0];
    app.search_filter = Some(HashSet::from([0]).into());
    app.details_tag_prewarm_indices = vec![0];
    let ctx = egui::Context::default();
    app.start_details_meta_load(&ctx);
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while app.details_meta_pending.is_some() {
        app.poll_details_meta_load(&ctx);
        assert!(
            std::time::Instant::now() < deadline,
            "whole-listing page worker did not finish"
        );
        std::thread::yield_now();
    }
    assert!(app.page_count_scan_ready());
    assert!(app.page_counts_wanted());
    assert_eq!(
        app.details_lazy_meta_for_idx(0).unwrap().page_count,
        Some(2)
    );
    assert_eq!(
        app.details_lazy_meta_for_idx(1).unwrap().page_count,
        Some(3)
    );
    assert!(app.details_lazy_meta_for_idx(2).is_none());
    assert_eq!(app.visible_indices, [0]);
    for idx in [0, 1] {
        let meta = app.details_lazy_meta_for_idx(idx).unwrap();
        assert!(!meta.ai_metadata_checked);
        assert!(meta.created_at.is_none() && meta.image_dims.is_none());
        assert!(matches!(meta.media, DetailsMediaMeta::NotFetched));
    }
}

#[test]
fn page_sort_883_stale_session_restarts_and_completes_from_current_memory() {
    for stale_pool in [true, false] {
        let mut app = fixture();
        cache_all(&mut app);
        let (cancel, tx) = pending(&mut app, DetailsMetaScanScope::AllRequirements, stale_pool);
        tx.send(DetailsMetaEvent::Stale {
            generation: app.items_generation,
        })
        .unwrap();
        app.poll_details_meta_load(&egui::Context::default());
        assert!(cancel.load(Ordering::Relaxed));
        assert!(app.details_meta_pending.is_none());
        assert!(app.page_count_scan_ready());
        assert_eq!(
            app.details_image_dims_state,
            LazyColumnState::Ready { failed: 0 }
        );
    }
}

#[test]
fn page_sort_883_requirements_pass_all_target_gates_with_badge_and_column_off() {
    let app = fixture();
    assert!(app.page_counts_wanted());
    assert!(app.whole_listing_meta_scan_wanted());
    assert!(app.details_lazy_columns_visible());
    assert!(!app.thumbnail_badge_metadata_enabled());
    let requirements = app.listing_meta_requirements();
    assert!(requirements.page_count);
    assert!(!requirements.duration && !requirements.ai);
    for idx in 0..app.items.len() {
        // false deliberately exercises allow_page_count outside the visible stage.
        let target = app.details_meta_target_for_idx(idx, &HashSet::from([0]), false);
        if [0, 1, 2, 3, 8].contains(&idx) {
            let target = target.expect("whole-listing book demand bypasses display gates");
            assert!(target.load_page_count);
            assert!(!target.load_video_meta && !target.load_ai_metadata);
            assert!(!target.load_created_at && !target.load_image_dims);
            assert_eq!(
                target.priority,
                if idx == 0 {
                    crate::io_semaphore::IoPriority::Normal
                } else {
                    crate::io_semaphore::IoPriority::Low
                }
            );
        } else {
            assert!(
                target.is_none(),
                "page-only request must not fetch item {idx}"
            );
        }
    }
}

#[test]
fn page_sort_883_planning_includes_filtered_offscreen_books_and_rejects_stale_cache() {
    let mut app = fixture();
    let count = DETAILS_META_TARGET_SCAN_BUDGET_PER_FRAME + 1;
    let root = app.tmp.path().join("bounded-page-plan");
    let mut items = (0..count)
        .map(|idx| GridItem::Image(root.join(format!("{idx}.png"))))
        .collect::<Vec<_>>();
    items[0] = GridItem::ZipFile(root.join("visible.zip"));
    items[1] = GridItem::Folder(root.join("filtered-book"));
    app.install_new_items(items, vec![Some((100, 0)); count]);
    cache_pages(&mut app, 0, Some(20), false);
    cache_pages(&mut app, 1, Some(30), false);
    let key = app.details_lazy_cache_key(1).unwrap();
    let stale_fingerprint = fingerprint(&app).wrapping_add(1);
    app.details_lazy_meta
        .get_mut(&key)
        .unwrap()
        .page_count_fingerprint = stale_fingerprint;
    app.visible_indices = vec![0];
    app.search_filter = Some(HashSet::from([0]).into());
    app.details_tag_prewarm_indices = vec![0];
    app.start_details_meta_load(&egui::Context::default());
    let pending = app.details_meta_pending.as_ref().unwrap();
    assert!(pending.listing_meta_scan.unwrap().requirements.page_count);
    let DetailsMetaPendingPhase::Planning(plan) = &pending.phase else {
        panic!("keep the request within bounded Planning");
    };
    assert!(matches!(plan.order, DetailsMetaScanOrder::WholeListing { len, .. } if len == count));
    assert!(
        plan.cached_done >= 1,
        "matching cached book satisfies Planning"
    );
    assert!(!plan.background_targets.iter().any(|target| target.idx == 0));
    let target = plan
        .background_targets
        .iter()
        .find(|target| target.idx == 1)
        .unwrap();
    assert!(target.load_page_count);
    assert!(!target.load_ai_metadata && !target.load_video_meta);
    assert!(!target.load_created_at && !target.load_image_dims);
    assert!(!app.page_count_scan_ready());
    app.cancel_details_meta_loading();
}

#[test]
fn page_sort_883_cached_unavailable_zero_and_empty_listing_complete_without_worker() {
    let mut app = fixture();
    cache_all(&mut app);
    cache_pages(&mut app, 1, None, true);
    cache_pages(&mut app, 2, Some(0), false);
    app.start_details_meta_load(&egui::Context::default());
    assert!(app.details_meta_pending.is_none());
    assert!(app.page_count_scan_ready());
    assert!(
        app.page_counts_wanted(),
        "completion must not remove continuing demand"
    );
    assert!(
        app.lazy_metric_loading_text(LazyMetric::PageCount)
            .is_none()
    );
    assert!(app.listing_meta_scan_proof.unwrap().requirements.page_count);
    app.install_new_items(Vec::new(), Vec::new());
    assert!(!app.page_count_scan_ready());
    app.start_details_meta_load(&egui::Context::default());
    assert!(app.details_meta_pending.is_none());
    assert!(app.page_count_scan_ready());
    assert_eq!(app.listing_meta_scan_proof.unwrap().items_len, 0);
}

#[test]
fn page_sort_883_finished_and_visible_stage_proofs_do_not_reorder_items() {
    let mut app = fixture();
    cache_all(&mut app);
    let before = app.items.clone();
    let (_, tx) = pending(&mut app, DetailsMetaScanScope::AllRequirements, false);
    assert!(
        app.lazy_metric_loading_text(LazyMetric::PageCount)
            .unwrap()
            .contains("0/5")
    );
    tx.send(DetailsMetaEvent::Finished {
        generation: app.items_generation,
        failed: 0,
    })
    .unwrap();
    app.poll_details_meta_load(&egui::Context::default());
    assert!(app.page_count_scan_ready());
    assert_eq!(app.items, before);
    let proof = app.listing_meta_scan_proof;
    let (_, tx) = pending(&mut app, DetailsMetaScanScope::VisibleStage, false);
    tx.send(DetailsMetaEvent::Finished {
        generation: app.items_generation,
        failed: 0,
    })
    .unwrap();
    app.poll_details_meta_load(&egui::Context::default());
    assert_eq!(app.listing_meta_scan_proof, proof);
    assert!(app.page_count_scan_ready());
}

#[test]
fn page_sort_883_membership_changes_retire_sessions_and_reprove_current_snapshot() {
    let mut app = fixture();
    cache_all(&mut app);
    let (cancel, old_tx) = pending(&mut app, DetailsMetaScanScope::AllRequirements, false);
    let added = app.tmp.path().join("added.png");
    app.items.push(GridItem::Image(added));
    app.image_metas.push(Some((100, 10)));
    app.poll_details_meta_load(&egui::Context::default());
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.page_count_scan_ready());
    assert_eq!(app.listing_meta_scan_proof.unwrap().items_len, 10);
    assert!(
        old_tx
            .send(DetailsMetaEvent::Finished {
                generation: app.items_generation,
                failed: 0
            })
            .is_err()
    );

    let (cancel, _tx) = pending(&mut app, DetailsMetaScanScope::AllRequirements, false);
    app.items.pop();
    app.image_metas.pop();
    app.bump_items_generation();
    cache_all(&mut app); // EPUB must be current for the replacement zero-target plan.
    assert!(!app.page_count_scan_ready());
    app.poll_details_meta_load(&egui::Context::default());
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.page_count_scan_ready());
    assert_eq!(
        app.listing_meta_scan_proof.unwrap().generation,
        app.items_generation
    );
}

#[test]
fn page_sort_883_stop_survives_facet_rebuild_until_explicit_resume() {
    let mut app = fixture();
    cache_all(&mut app);
    let (cancel, _tx) = pending(&mut app, DetailsMetaScanScope::AllRequirements, false);
    app.cancel_details_meta_loading();
    app.settings.facet_filter.size_preset = Some(crate::settings::FacetSizePreset::Under1MiB);
    app.rebuild_visible_indices();
    app.poll_details_meta_load(&egui::Context::default());
    app.reconcile_details_lazy_session_after_grid(&egui::Context::default());
    assert!(cancel.load(Ordering::Relaxed));
    assert_eq!(app.details_image_dims_state, LazyColumnState::Cancelled);
    assert!(app.details_meta_pending.is_none());
    assert!(!app.page_count_scan_ready());
    assert_eq!(
        app.lazy_metric_loading_text(LazyMetric::PageCount)
            .as_deref(),
        Some("ページ数の読み込みを停止中")
    );
    app.resume_details_meta_loading();
    app.poll_details_meta_load(&egui::Context::default());
    assert!(app.page_count_scan_ready());
}

#[test]
fn page_sort_883_stale_susie_or_worker_event_restarts_without_false_proof_or_user_stop() {
    for stale_pool in [true, false] {
        let mut app = fixture();
        let count = DETAILS_META_TARGET_SCAN_BUDGET_PER_FRAME + 1;
        let root = app.tmp.path().join("stale-whole-page-plan");
        app.install_new_items(
            (0..count)
                .map(|idx| GridItem::ZipFile(root.join(format!("{idx}.zip"))))
                .collect(),
            vec![Some((100, 2048)); count],
        );
        app.visible_indices = vec![0];
        app.details_tag_prewarm_indices = vec![0];
        let (cancel, tx) = pending(&mut app, DetailsMetaScanScope::AllRequirements, stale_pool);
        tx.send(if stale_pool {
            DetailsMetaEvent::Finished {
                generation: app.items_generation,
                failed: 0,
            }
        } else {
            DetailsMetaEvent::Stale {
                generation: app.items_generation,
            }
        })
        .unwrap();
        app.poll_details_meta_load(&egui::Context::default());
        assert!(cancel.load(Ordering::Relaxed));
        assert!(!app.page_count_scan_ready());
        assert!(app.listing_meta_scan_proof.is_none());
        assert!(!matches!(
            app.details_image_dims_state,
            LazyColumnState::Cancelled
        ));
        let replacement = app
            .details_meta_pending
            .as_ref()
            .expect("automatic fresh planning");
        assert!(matches!(
            replacement.phase,
            DetailsMetaPendingPhase::Planning(_)
        ));
        assert_eq!(
            replacement.pool_revision,
            crate::susie_loader::pool_revision()
        );
        assert!(
            replacement
                .listing_meta_scan
                .unwrap()
                .requirements
                .page_count
        );
        assert!(!Arc::ptr_eq(&replacement.cancel, &cancel));
        assert!(app.details_lazy_meta.is_empty());
        app.cancel_details_meta_loading();
    }
}

#[test]
fn page_sort_883_memory_and_worker_snapshot_match_stamp_fingerprint_and_folder_zero_size() {
    let mut app = fixture();
    cache_all(&mut app);
    let fingerprint = fingerprint(&app);
    for idx in [0, 1, 2, 3, 8] {
        let key = app.details_lazy_cache_key(idx).unwrap();
        let item = &app.items[idx];
        let stamp = app.image_metas[idx].unwrap();
        let expected = ListingPageCount::Known((idx as u32 + 1) * 10);
        assert_eq!(
            app.listing_page_count_for_item(item, stamp, fingerprint),
            Some(expected)
        );
        assert_eq!(
            app.listing_page_count_for_item(item, (stamp.0 + 1, stamp.1), fingerprint),
            None
        );
        assert_eq!(
            app.listing_page_count_for_item(item, (stamp.0, stamp.1 + 1), fingerprint),
            None
        );
        assert_eq!(
            app.listing_page_count_for_item(item, stamp, fingerprint.wrapping_add(1)),
            None
        );
        let snapshot = app.listing_metadata_snapshot(fingerprint, LazyMetric::PageCount);
        assert_eq!(
            snapshot.page_count.get(&(key, stamp.0, stamp.1)).copied(),
            Some(expected)
        );
    }
    assert!(
        app.listing_metadata_snapshot(fingerprint.wrapping_add(1), LazyMetric::PageCount)
            .page_count
            .is_empty()
    );
    assert!(
        app.listing_metadata_snapshot(fingerprint, LazyMetric::Duration)
            .page_count
            .is_empty()
    );
    assert_eq!(app.image_metas[2], Some((100, 0)));
}

#[test]
fn page_sort_883_pdf_epub_named_image_folders_keep_memory_sort_badge_and_generation_identity() {
    let mut app = fixture();
    app.settings.thumb_show_page_count = true;
    let root = app.tmp.path().join("extension-named-image-folders");
    for (name, count) in [("Photos.pdf", 2), ("Book.epub", 3)] {
        let folder = root.join(name);
        std::fs::create_dir_all(&folder).unwrap();
        for page in 0..count {
            std::fs::write(folder.join(format!("{page}.jpg")), b"page count only").unwrap();
        }
        assert_eq!(
            folder_scan::image_folder_page_count(
                &folder,
                &folder_scan::image_folder_page_count_options(&app.settings),
            )
            .unwrap(),
            Some(count),
        );
    }
    let scan = folder_scan::scan_directory_with_settings(&root, &app.settings).unwrap();
    assert!(scan.page_count_facts.is_none());
    let items = ["Photos.pdf", "Book.epub"]
        .into_iter()
        .map(|name| GridItem::Folder(root.join(name)))
        .collect::<Vec<_>>();
    let metas = items
        .iter()
        .map(|item| {
            let entry = scan
                .folders
                .iter()
                .find(|entry| &entry.item == item)
                .unwrap();
            assert_eq!(entry.sort_meta.file_size, None);
            assert_eq!(entry.display_meta.unwrap().1, 0);
            entry.display_meta
        })
        .collect();
    app.install_new_items(items, metas);
    for idx in 0..2 {
        cache_pages(&mut app, idx, Some(idx as u32 + 2), false);
        let meta = app.details_lazy_meta_for_idx(idx).unwrap();
        assert_eq!(meta.page_count_pdf_password_revision, None);
        assert_eq!(meta.page_count_generation, None);
    }
    let fingerprint = fingerprint(&app);
    let ui = folder_scan::materialize_local_folder_listing_with_order_and_metadata(
        &root,
        scan.clone_for_projection(),
        &app.settings,
        crate::rating_sort::ListingOrderRequest::Standard(SortOrder::PageCountAsc),
        |_| panic!("page sort must not read ratings"),
        |_, _| panic!("page sort must not read media duration"),
        fingerprint,
        |item, stamp| app.listing_page_count_for_item(item, stamp, fingerprint),
    );
    let snapshot = app.listing_metadata_snapshot(fingerprint, LazyMetric::PageCount);
    assert_eq!(snapshot.page_count.len(), 2);
    let worker = folder_scan::materialize_local_folder_listing_with_order_and_metadata(
        &root,
        scan,
        &app.settings,
        crate::rating_sort::ListingOrderRequest::Standard(SortOrder::PageCountAsc),
        |_| panic!("page sort must not read ratings"),
        |_, _| panic!("page sort must not read media duration"),
        fingerprint,
        |item, (mtime, size)| {
            let path = folder_scan::listing_page_count_path(item).unwrap();
            snapshot
                .page_count
                .get(&(crate::adjustment_db::normalize_path(path), mtime, size))
                .copied()
        },
    );
    assert_eq!(ui.items, worker.items);
    assert_eq!(ui.sort_metas, worker.sort_metas);
    assert_eq!(ui.items[0], GridItem::Folder(root.join("Photos.pdf")));
    assert_eq!(ui.sort_metas[0].page_count, ListingPageCount::Known(2));
    assert_eq!(ui.sort_metas[1].page_count, ListingPageCount::Known(3));
    assert!(ui.sort_metas.iter().all(|meta| meta.file_size.is_none()));

    app.bump_items_generation();
    for idx in 0..2 {
        let count = idx as u32 + 2;
        let stamp = app.image_metas[idx].unwrap();
        assert_eq!(stamp.1, 0);
        assert_eq!(
            app.listing_page_count_for_item(&app.items[idx], stamp, fingerprint),
            Some(ListingPageCount::Known(count))
        );
        assert_eq!(
            app.details_page_count_value(idx),
            Some(PageCountValue::Known(count))
        );
        assert_eq!(
            app.thumbnail_page_count_text(idx),
            Some(format!("{count}P"))
        );
        assert!(app.details_page_count_meta_satisfies_idx(
            idx,
            app.details_lazy_meta_for_idx(idx).unwrap()
        ));
        assert!(
            app.details_meta_target_for_idx(idx, &HashSet::new(), false)
                .is_none(),
            "folder names never request EPUB generation refetch"
        );
    }
}

#[test]
fn page_sort_883_epub_sort_ignores_generation_but_scan_requires_refetch() {
    let mut app = fixture();
    cache_pages(&mut app, 8, Some(42), false);
    let key = app.details_lazy_cache_key(8).unwrap();
    let stamp = app.image_metas[8].unwrap();
    let fingerprint = fingerprint(&app);
    app.bump_items_generation();
    assert_eq!(
        app.listing_page_count_for_item(&app.items[8], stamp, fingerprint),
        Some(ListingPageCount::Known(42))
    );
    assert_eq!(
        app.listing_metadata_snapshot(fingerprint, LazyMetric::PageCount)
            .page_count
            .get(&(key, stamp.0, stamp.1))
            .copied(),
        Some(ListingPageCount::Known(42))
    );
    assert!(
        app.details_meta_target_for_idx(8, &HashSet::new(), false)
            .unwrap()
            .load_page_count
    );
    cache_pages(&mut app, 8, Some(42), false);
    assert!(
        app.details_meta_target_for_idx(8, &HashSet::new(), false)
            .is_none()
    );
}

#[test]
fn page_sort_883_memory_preserves_unavailable_and_rejects_unchecked_or_old_pdf_credentials() {
    let mut app = fixture();
    let fingerprint = fingerprint(&app);
    let key = app.details_lazy_cache_key(0).unwrap();
    let stamp = app.image_metas[0].unwrap();
    for (count, failed) in [(None, false), (Some(19), true)] {
        cache_pages(&mut app, 0, count, failed);
        assert_eq!(
            app.listing_page_count_for_item(&app.items[0], stamp, fingerprint),
            Some(ListingPageCount::Unavailable)
        );
        assert_eq!(
            app.listing_metadata_snapshot(fingerprint, LazyMetric::PageCount)
                .page_count
                .get(&(key.clone(), stamp.0, stamp.1))
                .copied(),
            Some(ListingPageCount::Unavailable)
        );
    }
    app.details_lazy_meta
        .get_mut(&key)
        .unwrap()
        .page_count_checked = false;
    assert_eq!(
        app.listing_page_count_for_item(&app.items[0], stamp, fingerprint),
        None
    );
    cache_pages(&mut app, 1, Some(5), false);
    let key = app.details_lazy_cache_key(1).unwrap();
    let stamp = app.image_metas[1].unwrap();
    let current_revision = app.details_lazy_meta[&key]
        .page_count_pdf_password_revision
        .unwrap();
    for invalid_revision in [None, Some(current_revision.wrapping_add(1))] {
        app.details_lazy_meta
            .get_mut(&key)
            .unwrap()
            .page_count_pdf_password_revision = invalid_revision;
        assert_eq!(
            app.listing_page_count_for_item(&app.items[1], stamp, fingerprint),
            None
        );
        assert!(
            !app.listing_metadata_snapshot(fingerprint, LazyMetric::PageCount)
                .page_count
                .contains_key(&(key.clone(), stamp.0, stamp.1))
        );
    }
}

#[test]
fn page_sort_883_pending_snapshot_keeps_promised_page_fields_after_sort_change() {
    let mut app = fixture();
    let (_, _tx) = pending(&mut app, DetailsMetaScanScope::AllRequirements, false);
    app.settings.sort_order = SortOrder::FileName;
    assert!(!app.page_counts_wanted());
    assert!(!app.listing_meta_requirements().page_count);
    assert!(app.active_listing_meta_requirements().page_count);
    assert!(
        app.details_meta_target_for_idx(2, &HashSet::new(), false)
            .unwrap()
            .load_page_count
    );
    app.cancel_details_meta_loading();
}

#[test]
fn page_sort_883_duration_and_ai_requirements_do_not_count_books() {
    let mut app = fixture();
    app.settings.sort_order = SortOrder::FileName;
    for ai in [false, true] {
        app.ai_model_facet_requested = ai;
        app.duration_facet_requested = !ai;
        assert!(!app.page_counts_wanted());
        assert!(!app.listing_meta_requirements().page_count);
        for idx in [0, 1, 2, 3, 8] {
            if let Some(target) = app.details_meta_target_for_idx(idx, &HashSet::new(), false) {
                assert!(!target.load_page_count);
            }
        }
        let idx = if ai { 7 } else { 5 };
        let target = app
            .details_meta_target_for_idx(idx, &HashSet::new(), false)
            .unwrap();
        assert_eq!(target.load_ai_metadata, ai);
        assert_eq!(target.load_video_meta, !ai);
        assert!(!target.load_page_count && !target.load_created_at && !target.load_image_dims);
    }
}

#[test]
fn page_sort_883_requirements_algebra_keeps_page_count_independent() {
    let pages = ListingMetaRequirements {
        page_count: true,
        ..Default::default()
    };
    let duration = ListingMetaRequirements {
        duration: true,
        ..Default::default()
    };
    let ai = ListingMetaRequirements {
        ai: true,
        ..Default::default()
    };
    assert!(!pages.is_empty());
    assert!(!duration.contains(pages) && !ai.contains(pages));
    let all = pages.union(duration).union(ai);
    assert!(all.contains(pages) && all.contains(duration) && all.contains(ai));
    assert_eq!(all.without(duration.union(ai)), pages);
    assert_eq!(all.without(pages), duration.union(ai));
}

#[test]
fn page_sort_883_sort_selection_and_reselection_request_worker_refresh() {
    for sort in [SortOrder::PageCountAsc, SortOrder::PageCountDesc] {
        let mut app = fixture();
        let folder = app.tmp.path().join("page-sort-refresh");
        std::fs::create_dir(&folder).unwrap();
        app.current_folder = Some(folder);
        app.settings.sort_order = sort;
        app.apply_sort_change_reload();
        let request = app
            .folder_pane_open_pending
            .as_ref()
            .expect("worker scan request");
        assert!(matches!(
            request.purpose,
            FolderOpenScanPurpose::CurrentViewOrderRefresh { .. }
        ));
        request.cancel.store(true, Ordering::Relaxed);
    }
}

#[cfg(windows)]
#[test]
fn page_sort_883_proof_cache_and_pending_follow_context_park_mount_drop() {
    let mut app = fixture();
    cache_all(&mut app);
    app.start_details_meta_load(&egui::Context::default());
    let main_proof = app.listing_meta_scan_proof;
    assert!(app.page_count_scan_ready());
    let mut sibling = app.split_current_context_preserving_main_grid();
    assert_eq!(app.listing_meta_scan_proof, main_proof);
    app.swap_viewer_context_bundle(&mut sibling);
    assert!(app.listing_meta_scan_proof.is_none());
    assert!(app.details_lazy_meta.is_empty());
    let sibling_path = app.tmp.path().join("sibling.png");
    app.install_new_items(vec![GridItem::Image(sibling_path)], vec![Some((200, 1))]);
    let snapshot = app.listing_meta_scan_snapshot();
    app.complete_listing_meta_scan(Some(snapshot));
    let sibling_proof = app.listing_meta_scan_proof;
    let (sibling_cancel, _tx) = pending(&mut app, DetailsMetaScanScope::VisibleStage, false);
    app.swap_viewer_context_bundle(&mut sibling);
    assert_eq!(app.listing_meta_scan_proof, main_proof);
    assert_eq!(app.details_lazy_meta.len(), 5);
    assert!(app.details_meta_pending.is_none());
    assert!(!sibling_cancel.load(Ordering::Relaxed));
    app.swap_viewer_context_bundle(&mut sibling);
    assert_eq!(app.listing_meta_scan_proof, sibling_proof);
    assert!(app.details_lazy_meta.is_empty());
    assert!(app.details_meta_pending.is_some());
    app.swap_viewer_context_bundle(&mut sibling);
    drop(sibling);
    assert!(sibling_cancel.load(Ordering::Relaxed));
    assert_eq!(app.listing_meta_scan_proof, main_proof);
    assert!(app.page_count_scan_ready());
    assert_eq!(app.details_lazy_meta.len(), 5);
}

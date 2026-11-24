//! Section 1.883 stage 1: page badges share existing metadata ownership and demand.
use super::*;
use crate::settings::{DetailsSortKey, GridViewMode, SelectionInfoDisplayMode};

fn fixture() -> phase_c_support::AppTestEnv {
    let mut app = phase_c_support::setup_app();
    app.settings.grid_view_mode = GridViewMode::Thumbnail;
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
    let root = app.tmp.path().join("uncreated-page-badge-sources");
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
            GridItem::ZipFile(root.join("offscreen.zip")),
        ],
        vec![Some((100, 2048)); 10],
    );
    app.ai_model_facet_requested = false;
    app.duration_facet_requested = false;
    app.selected = None;
    app.visible_indices = (0..app.items.len()).collect();
    app.details_tag_prewarm_indices = vec![0];
    app.keep_set = HashSet::from([0]);
    app
}

fn cache_pages(app: &mut App, idx: usize, count: Option<u32>, failed: bool) {
    let key = app.details_lazy_cache_key(idx).unwrap();
    let (source_mtime, source_size) = app.image_metas[idx].unwrap_or((0, 0));
    let password_revision = match &app.items[idx] {
        GridItem::PdfFile(path) => Some(app.pdf_passwords.credential_revision(path)),
        _ => None,
    };
    let generation = matches!(&app.items[idx], GridItem::PdfFile(path) if path.extension().and_then(|ext| ext.to_str()).is_some_and(|ext| ext.eq_ignore_ascii_case("epub")))
        .then_some(app.items_generation);
    let fingerprint = crate::app::folder_scan::image_page_recognition_fingerprint(&app.settings);
    app.details_lazy_meta.insert(
        key,
        DetailsLazyMeta {
            source_mtime,
            source_size,
            page_count: count,
            page_count_checked: true,
            page_count_failed: failed,
            page_count_fingerprint: fingerprint,
            page_count_generation: generation,
            page_count_pdf_password_revision: password_revision,
            ..Default::default()
        },
    );
}

#[test]
fn page_badge_883_page_only_requests_near_books_without_media_probe() {
    let mut app = fixture();
    app.settings.thumb_show_page_count = true;
    let near: HashSet<_> = (0..9).collect();
    assert!(app.thumbnail_badge_metadata_enabled());
    assert!(app.thumbnail_page_count_enabled());
    assert!(!app.thumbnail_media_fields_enabled());
    assert!(app.details_lazy_uses_visible_stages());
    assert!(!app.selection_info_only_lazy_load());
    for idx in 0..app.items.len() {
        let target = app.details_meta_target_for_idx(idx, &near, near.contains(&idx));
        if [0, 1, 2, 3, 8].contains(&idx) {
            let target = target.expect("near supported book");
            assert!(target.load_page_count);
            assert!(!target.load_video_meta && !target.load_ai_metadata);
            assert!(!target.load_image_dims && !target.load_created_at);
            assert!(app.details_meta_target_for_idx(idx, &near, false).is_none());
        } else {
            assert!(target.is_none(), "unsupported or offscreen item {idx}");
        }
    }
}

#[test]
fn page_badge_883_media_and_resume_only_keep_page_fields_off() {
    let mut app = fixture();
    let near: HashSet<_> = (0..app.items.len()).collect();
    for resume in [false, true] {
        app.settings.thumb_show_media_duration = !resume;
        app.settings.thumb_show_resume_meter = resume;
        assert!(app.thumbnail_badge_metadata_enabled());
        assert!(app.thumbnail_media_fields_enabled());
        assert!(!app.thumbnail_page_count_enabled());
        for idx in 0..app.items.len() {
            let target = app.details_meta_target_for_idx(idx, &near, true);
            if [5, 6].contains(&idx) {
                let target = target.expect("media duration is still required");
                assert!(target.load_video_meta);
                assert!(!target.load_page_count);
            } else {
                assert!(target.is_none(), "media-only item {idx}");
            }
        }
    }
}

#[test]
fn page_badge_883_all_badges_off_do_not_start_a_session() {
    let mut app = fixture();
    assert!(!app.thumbnail_badge_metadata_enabled());
    assert!(!app.details_lazy_columns_visible());
    assert!(!app.details_lazy_uses_visible_stages());
    let ctx = egui::Context::default();
    for _ in 0..4 {
        app.poll_details_meta_load(&ctx);
        app.reconcile_details_lazy_session_after_grid(&ctx);
        assert!(app.details_meta_pending.is_none());
        assert!(!matches!(
            app.details_image_dims_state,
            LazyColumnState::Loading { .. }
        ));
    }
}

#[test]
fn page_badge_883_selection_only_keeps_single_book_requirement() {
    let mut app = fixture();
    app.settings.selection_info_display_mode = SelectionInfoDisplayMode::BottomBar;
    app.settings.details_show_page_count = true;
    app.selected = Some(0);
    assert!(app.selection_info_only_lazy_load());
    assert_eq!(app.selection_info_lazy_target_idx(), Some(0));
    let near = HashSet::from([1]);
    assert!(
        app.details_meta_target_for_idx(0, &near, true)
            .unwrap()
            .load_page_count
    );
    assert!(app.details_meta_target_for_idx(1, &near, true).is_none());
    assert!(!app.thumbnail_badge_metadata_enabled());
}

#[test]
fn page_badge_883_whole_listing_ai_and_duration_do_not_expand_page_scope() {
    let mut app = fixture();
    for page_badge in [false, true] {
        app.settings.thumb_show_page_count = page_badge;
        app.ai_model_facet_requested = true;
        app.duration_facet_requested = true;
        // Book 1 is near but filtered out; book 9 is projected but offscreen.
        app.visible_indices = vec![0, 5, 7, 9];
        let near = HashSet::from([0, 1, 5, 7]);
        assert!(app.whole_listing_meta_scan_wanted());
        for idx in [0, 1, 9] {
            let target = app.details_meta_target_for_idx(idx, &near, near.contains(&idx));
            if page_badge && idx == 0 {
                assert!(target.unwrap().load_page_count);
            } else {
                assert!(target.is_none(), "page_badge={page_badge}, book={idx}");
            }
        }
        let hidden_media = app.details_meta_target_for_idx(6, &near, false).unwrap();
        assert!(hidden_media.load_video_meta && !hidden_media.load_page_count);
        let ai = app.details_meta_target_for_idx(7, &near, false).unwrap();
        assert!(ai.load_ai_metadata && !ai.load_page_count && !ai.load_video_meta);
    }
}

#[test]
fn page_badge_883_fingerprint_refetch_keeps_last_display_and_sort_values() {
    let mut app = fixture();
    app.settings.thumb_show_page_count = true;
    for idx in [0, 1, 2, 3, 8] {
        cache_pages(&mut app, idx, Some(123), false);
    }
    app.details_image_dims_state = LazyColumnState::Ready { failed: 0 };
    assert!(!app.details_visible_stage_needs_load());
    let old = app
        .details_lazy_meta_for_idx(0)
        .unwrap()
        .page_count_fingerprint;
    app.settings.show_hidden_files = !app.settings.show_hidden_files;
    assert_ne!(
        old,
        crate::app::folder_scan::image_page_recognition_fingerprint(&app.settings)
    );
    assert!(app.details_visible_stage_needs_load());
    for idx in [0, 1, 2, 3, 8] {
        let meta = app.details_lazy_meta_for_idx(idx).unwrap();
        assert!(!app.details_page_count_meta_satisfies_idx(idx, meta));
        assert!(
            app.details_meta_target_for_idx(idx, &HashSet::from([idx]), true)
                .unwrap()
                .load_page_count
        );
        assert_eq!(
            app.details_page_count_value(idx),
            Some(PageCountValue::Known(123))
        );
        assert_eq!(app.details_page_count_text(idx), "123");
        assert_eq!(app.details_page_count_sort_value(idx), Some(123));
        assert_eq!(app.thumbnail_page_count_text(idx).as_deref(), Some("123P"));
    }
    app.settings.grid_view_mode = GridViewMode::Details;
    app.settings.details_show_page_count = true;
    app.settings.details_sort_key = DetailsSortKey::PageCount;
    assert!(!app.details_lazy_uses_visible_stages());
    app.poll_details_meta_load(&egui::Context::default());
    assert!(
        app.details_meta_pending.is_none(),
        "completed full sort waits for its next explicit request"
    );
    assert_eq!(
        app.details_image_dims_state,
        LazyColumnState::Ready { failed: 0 }
    );
    assert_eq!(app.details_page_count_sort_value(0), Some(123));
    assert!(
        app.details_meta_target_for_idx(0, &HashSet::from([0]), true)
            .unwrap()
            .load_page_count
    );
}

#[test]
fn page_badge_883_epub_generation_refetches_success_and_absence_only_once() {
    for failed in [false, true] {
        let mut app = fixture();
        app.settings.thumb_show_page_count = true;
        let count = (!failed).then_some(42);
        cache_pages(&mut app, 8, count, failed);
        cache_pages(&mut app, 0, Some(9), false);
        app.details_tag_prewarm_indices = vec![8];
        app.details_image_dims_state = LazyColumnState::Ready { failed: 0 };
        assert!(!app.details_visible_stage_needs_load());
        assert!(
            app.details_meta_target_for_idx(8, &HashSet::from([8]), true)
                .is_none()
        );
        app.items_generation = app.items_generation.wrapping_add(1);
        assert!(app.details_visible_stage_needs_load());
        assert!(
            app.details_meta_target_for_idx(8, &HashSet::from([8]), true)
                .unwrap()
                .load_page_count
        );
        assert!(
            app.details_meta_target_for_idx(0, &HashSet::from([0]), true)
                .is_none(),
            "ZIP ignores listing generation"
        );
        assert_eq!(
            app.thumbnail_page_count_text(8),
            count.map(|n| format!("{n}P"))
        );
        assert_eq!(app.details_page_count_sort_value(8), count.map(u64::from));
        cache_pages(&mut app, 8, count, failed);
        assert!(!app.details_visible_stage_needs_load());
        assert!(
            app.details_meta_target_for_idx(8, &HashSet::from([8]), true)
                .is_none()
        );
    }
}

#[test]
fn page_badge_883_values_hide_unchecked_absent_zero_failed_and_drive_rows() {
    let mut app = fixture();
    app.settings.thumb_show_page_count = true;
    assert_eq!(app.details_page_count_value(0), None);
    assert_eq!(app.thumbnail_page_count_text(0), None);
    app.details_image_dims_state = LazyColumnState::Ready { failed: 0 };
    for (count, failed, expected) in [
        (None, false, None),
        (Some(0), false, None),
        (Some(99), true, None),
        (Some(123456), false, Some("123456P")),
    ] {
        cache_pages(&mut app, 0, count, failed);
        let value = if failed {
            PageCountValue::Unavailable
        } else {
            count
                .map(PageCountValue::Known)
                .unwrap_or(PageCountValue::Unavailable)
        };
        assert_eq!(app.details_page_count_value(0), Some(value));
        assert_eq!(
            app.details_page_count_sort_value(0),
            if failed { None } else { count.map(u64::from) }
        );
        assert_eq!(
            app.details_page_count_text(0),
            if failed {
                "-".to_owned()
            } else {
                count
                    .map(|count| count.to_string())
                    .unwrap_or_else(|| "-".to_owned())
            }
        );
        assert_eq!(app.thumbnail_page_count_text(0).as_deref(), expected);
    }
    app.settings.thumb_show_page_count = false;
    assert_eq!(app.thumbnail_page_count_text(0), None);
    app.settings.thumb_show_page_count = true;
    assert_eq!(app.thumbnail_page_count_text(0).as_deref(), Some("123456P"));
    app.items_are_drive_list = true;
    assert_eq!(app.thumbnail_page_count_text(0), None);
    assert!(!app.details_lazy_columns_visible());
    assert!(app.details_meta_pending.is_none());
}

fn zip_child(app: &mut App, root: &Path) {
    let entries = [
        "book.zip/001.jpg",
        "book.zip/chapter/002.jpg",
        "book.zip/chapter/003.jpg",
        "other.zip/001.jpg",
    ]
    .into_iter()
    .map(|entry_name| crate::zip_loader::ZipImageEntry {
        entry_name: entry_name.to_owned(),
        uncompressed_size: 100,
        mtime: 100,
        container_ancestry: Vec::new(),
    })
    .collect();
    let tree = Arc::new(crate::zip_tree::ZipTree::build(
        root.join("outer.zip"),
        entries,
    ));
    let (items, metas) = tree.materialize_level(&[], crate::settings::SortOrder::FileName);
    app.install_new_items(items, metas);
    app.zip_nav = Some(crate::zip_tree::ZipNavState::new(tree));
    assert!(
        matches!(&app.items[0], GridItem::ZipDir { dir_prefix, .. } if dir_prefix == "book.zip/")
    );
    app.visible_indices = (0..app.items.len()).collect();
    app.details_tag_prewarm_indices = vec![0];
    app.keep_set = HashSet::from([0]);
    app.selected = Some(0);
}

#[test]
fn page_badge_883_zipdir_reads_tree_and_never_starts_repeated_lazy_sessions() {
    for has_tree in [false, true] {
        for mode in 0..3 {
            let mut app = fixture();
            let root = app.tmp.path().to_path_buf();
            zip_child(&mut app, &root);
            // A stale lazy entry must never become a second owner of the count.
            cache_pages(&mut app, 0, Some(999), false);
            if !has_tree {
                app.zip_nav = None;
            }
            match mode {
                0 => {
                    app.settings.selection_info_display_mode = SelectionInfoDisplayMode::BottomBar;
                    app.settings.details_show_page_count = true;
                }
                1 => {
                    app.settings.grid_view_mode = GridViewMode::Details;
                    app.settings.details_show_page_count = true;
                    app.settings.details_sort_key = DetailsSortKey::PageCount;
                }
                _ => app.settings.thumb_show_page_count = true,
            }
            assert!(app.details_item_supports_page_count(0));
            assert!(!app.details_item_page_count_needs_fetch(0));
            assert!(!app.selection_info_item_requires_lazy_meta(0));
            assert_eq!(app.selection_info_lazy_target_idx(), None);
            assert!(!app.details_item_requires_lazy_meta(0));
            assert!(!app.details_visible_stage_needs_load());
            assert!(
                app.details_meta_target_for_idx(0, &HashSet::from([0]), true)
                    .is_none()
            );
            let _observations = crate::ime_focus::typing_probe::Session::new();
            let ctx = egui::Context::default();
            app.poll_details_meta_load(&ctx);
            app.reconcile_details_lazy_session_after_grid(&ctx);
            let revision = app.details_lazy_visible_revision;
            let session_starts = || {
                crate::ime_focus::typing_probe::jobs()
                    .iter()
                    .filter(|job| {
                        job.field == "details-meta-session"
                            && job.outcome == crate::ime_focus::typing_probe::Outcome::Started
                    })
                    .count()
            };
            let initial_sessions = session_starts();
            assert!(initial_sessions <= 1);
            for _ in 0..8 {
                app.poll_details_meta_load(&ctx);
                app.reconcile_details_lazy_session_after_grid(&ctx);
                assert!(
                    app.details_meta_pending.is_none(),
                    "tree={has_tree}, mode={mode}"
                );
                assert!(!matches!(
                    app.details_image_dims_state,
                    LazyColumnState::Loading { .. }
                ));
                if mode != 0 {
                    assert_eq!(app.details_lazy_visible_revision, revision);
                }
                assert_eq!(
                    session_starts(),
                    initial_sessions,
                    "ZipDir demand must not repeatedly start zero-target sessions"
                );
            }
            app.details_image_dims_state = LazyColumnState::Ready { failed: 0 };
            let expected = if has_tree {
                PageCountValue::Known(3)
            } else {
                PageCountValue::Unavailable
            };
            assert_eq!(app.details_page_count_value(0), Some(expected));
            assert_eq!(
                app.details_page_count_text(0),
                if has_tree { "3" } else { "-" }
            );
            assert_eq!(app.details_page_count_sort_value(0), has_tree.then_some(3));
            app.settings.grid_view_mode = GridViewMode::Thumbnail;
            app.settings.thumb_show_page_count = true;
            assert_eq!(
                app.thumbnail_page_count_text(0).as_deref(),
                has_tree.then_some("3P")
            );
        }
    }
}

fn assert_stale_session_restarts(stale_pool_revision: bool) {
    let mut app = fixture();
    app.settings.thumb_show_page_count = true;
    let count = DETAILS_META_TARGET_SCAN_BUDGET_PER_FRAME + 1;
    let root = app.tmp.path().join("stale-session-books");
    app.install_new_items(
        (0..count)
            .map(|idx| GridItem::ZipFile(root.join(format!("{idx}.zip"))))
            .collect(),
        vec![Some((100, 2048)); count],
    );
    app.visible_indices = vec![0];
    app.keep_set = HashSet::from([0]);
    app.details_tag_prewarm_indices = vec![0];
    app.duration_facet_requested = true;
    let scan = app.listing_meta_scan_snapshot();
    app.complete_listing_meta_scan(Some(scan));
    let proof = app.listing_meta_scan_proof;
    assert!(app.duration_scan_ready());
    let (tx, rx) = mpsc::channel();
    let (priority_tx, _priority_rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    app.details_meta_pending = Some(DetailsMetaPending {
        pool_revision: crate::susie_loader::pool_revision()
            .wrapping_add(u64::from(stale_pool_revision)),
        scan_scope: DetailsMetaScanScope::VisibleStage,
        listing_meta_scan: None,
        visible_revision: app.details_lazy_visible_revision,
        selection_target_key: None,
        normal_target_keys: HashSet::new(),
        cancel: Arc::clone(&cancel),
        phase: DetailsMetaPendingPhase::Loading { rx, priority_tx },
    });
    app.details_image_dims_state = LazyColumnState::Loading { done: 0, total: 1 };
    let event = if stale_pool_revision {
        DetailsMetaEvent::Finished {
            generation: app.items_generation,
            failed: 0,
        }
    } else {
        DetailsMetaEvent::Stale {
            generation: app.items_generation,
        }
    };
    tx.send(event).unwrap();
    app.poll_details_meta_load(&egui::Context::default());
    assert!(cancel.load(Ordering::Relaxed));
    // poll reuses the existing NotRequested -> AllRequirements path immediately.
    // Keep the replacement in bounded Planning so no worker races these assertions.
    let replacement = app
        .details_meta_pending
        .as_ref()
        .expect("stale sessions automatically restart");
    assert_eq!(
        replacement.pool_revision,
        crate::susie_loader::pool_revision()
    );
    assert!(!Arc::ptr_eq(&replacement.cancel, &cancel));
    assert!(matches!(
        replacement.phase,
        DetailsMetaPendingPhase::Planning(_)
    ));
    assert!(!matches!(
        app.details_image_dims_state,
        LazyColumnState::Cancelled
    ));
    assert_eq!(app.listing_meta_scan_proof, proof);
    assert!(app.duration_scan_ready());
    assert!(app.details_lazy_meta.is_empty());
    assert!(
        tx.send(DetailsMetaEvent::Finished {
            generation: app.items_generation,
            failed: 0
        })
        .is_err()
    );
    app.cancel_details_meta_loading();
}

#[test]
fn page_badge_883_stale_susie_pending_restarts_preserving_proof() {
    assert_stale_session_restarts(true);
}

#[test]
fn page_badge_883_stale_worker_event_restarts_preserving_proof() {
    assert_stale_session_restarts(false);
}

#[test]
fn page_badge_883_preference_toggle_cancels_old_requirements_but_keeps_values() {
    let mut app = fixture();
    app.settings.thumb_show_page_count = true;
    cache_pages(&mut app, 0, Some(12), false);
    let cancel = install_fake_media_duration_pending(&mut app);
    let revision = app.details_lazy_visible_revision;
    let mut edited = app.settings.clone();
    edited.thumb_show_page_count = false;
    app.install_preferences_settings(edited);
    assert!(cancel.load(Ordering::Relaxed));
    assert!(app.details_meta_pending.is_none());
    assert_eq!(app.details_image_dims_state, LazyColumnState::Disabled);
    assert_eq!(app.details_lazy_visible_revision, revision.wrapping_add(1));
    assert_eq!(
        app.details_page_count_value(0),
        Some(PageCountValue::Known(12))
    );
    assert_eq!(app.thumbnail_page_count_text(0), None);
    let mut edited = app.settings.clone();
    edited.thumb_show_page_count = true;
    app.install_preferences_settings(edited);
    assert_eq!(app.thumbnail_page_count_text(0).as_deref(), Some("12P"));
    assert!(
        app.details_meta_target_for_idx(0, &HashSet::from([0]), true)
            .is_none()
    );
}

#[test]
fn page_badge_883_planning_keeps_one_recognition_snapshot_until_next_job() {
    let mut app = fixture();
    let count = DETAILS_META_TARGET_SCAN_BUDGET_PER_FRAME * 2 + 1;
    let root = app.tmp.path().join("snapshot-books");
    app.install_new_items(
        (0..count)
            .map(|idx| GridItem::ZipFile(root.join(format!("{idx}.zip"))))
            .collect(),
        vec![Some((100, 2048)); count],
    );
    app.visible_indices = (0..count).collect();
    app.details_tag_prewarm_indices = vec![0];
    app.settings.thumb_show_page_count = true;
    app.duration_facet_requested = true;
    let ctx = egui::Context::default();
    app.start_details_meta_load(&ctx);
    let snapshot = |app: &App| {
        let pending = app.details_meta_pending.as_ref().expect("bounded planning");
        let DetailsMetaPendingPhase::Planning(plan) = &pending.phase else {
            panic!("plan must stay bounded");
        };
        let options = plan
            .page_count_config
            .image_folder_options
            .as_ref()
            .unwrap();
        assert_eq!(plan.page_count_config.fingerprint, options.fingerprint);
        assert_eq!(plan.page_count_config.pool_revision, options.pool_revision);
        assert_eq!(pending.pool_revision, options.pool_revision);
        (
            plan.page_count_config.fingerprint,
            options.pool_revision,
            options.show_hidden_files,
        )
    };
    let original = snapshot(&app);
    assert_eq!(
        original.0,
        crate::app::folder_scan::image_page_recognition_fingerprint(&app.settings)
    );
    app.settings.show_hidden_files = !app.settings.show_hidden_files;
    let changed = crate::app::folder_scan::image_page_recognition_fingerprint(&app.settings);
    assert_ne!(original.0, changed);
    app.advance_details_meta_target_plan(&ctx);
    assert_eq!(
        snapshot(&app),
        original,
        "an in-flight job owns its original options and fingerprint"
    );
    app.invalidate_details_meta_requirements();
    app.start_details_meta_load(&ctx);
    let replacement = snapshot(&app);
    assert_eq!(replacement.0, changed);
    assert_eq!(replacement.2, app.settings.show_hidden_files);
    app.cancel_details_meta_loading();
}

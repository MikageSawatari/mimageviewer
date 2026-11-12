//! §1.697: grid demand owns no-art transitions; request owners keep their normal lifetime.
use super::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use crate::grid_item::{
    ContainerIconKind, ContainerIconKinds, container_icon_kind, container_icon_only,
    effective_thumbnail_state,
};
use crate::thumb_loader::{ThumbLoadOrigin, ThumbMsg};

fn kinds() -> Vec<(GridItem, bool)> {
    vec![
        (GridItem::Folder("c:/shelf/book".into()), true),
        (GridItem::ZipFile("c:/shelf/book.zip".into()), true),
        (GridItem::PdfFile("c:/shelf/book.pdf".into()), true),
        (
            GridItem::ConvertibleArchive {
                path: "c:/shelf/book.7z".into(),
                format: crate::archive_converter::ArchiveFormat::SevenZ,
            },
            true,
        ),
        (
            GridItem::ZipDir {
                zip_path: "c:/shelf/book.zip".into(),
                dir_prefix: "chapter/".into(),
                is_archive: false,
                representative: Some("chapter/1.png".into()),
            },
            true,
        ),
        (
            GridItem::SearchContainer {
                path: "c:/shelf/book".into(),
                kind: crate::grid_item::SearchContainerKind::Folder,
                hit_count: 2,
                representative: Some(crate::grid_item::ContainerRepresentative {
                    path: "c:/shelf/book/1.png".into(),
                    zip_entry: None,
                    pdf_page: None,
                }),
            },
            true,
        ),
        (GridItem::Image("c:/shelf/1.png".into()), false),
        (GridItem::Video("c:/shelf/1.mp4".into()), false),
        (GridItem::Audio("c:/shelf/1.mp3".into()), false),
        (
            GridItem::ZipImage {
                zip_path: "c:/shelf/book.zip".into(),
                entry_name: "1.png".into(),
            },
            false,
        ),
        (
            GridItem::PdfPage {
                pdf_path: "c:/shelf/book.pdf".into(),
                page_num: 0,
                content_type: None,
            },
            false,
        ),
        (
            GridItem::Stack {
                key: "pages".into(),
                representative: "c:/shelf/1.png".into(),
                count: 2,
            },
            false,
        ),
        (
            GridItem::CollectionPlaceholder {
                path: "c:/missing".into(),
                last_known_kind: crate::collection_store::CollectionResolvedKind::Image,
                reason: crate::grid_item::CollectionPlaceholderReason::Missing,
            },
            false,
        ),
    ]
}

fn fixture(items: Vec<GridItem>) -> AppTestEnv {
    let mut env = setup_app();
    let app = &mut env.app;
    let count = items.len();
    app.items = items;
    app.image_metas = vec![Some((10, 20)); count];
    app.thumbnails = vec![ThumbnailState::Pending; count];
    app.visible_indices = (0..count).collect();
    app.keep_set.extend(0..count);
    app.keep_range = (0, count);
    app.last_vis_range = (0, count);
    app.settings.grid_cols = 1;
    app.last_cell_size = 120.0;
    app.last_cell_h = 120.0;
    app.last_viewport_h = 120.0 * count as f32;
    app.last_scroll_change_time = Instant::now() - Duration::from_secs(2);
    app.last_input_at = Some(Instant::now() - Duration::from_secs(2));
    app.reload_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
    app.heavy_io_queue = Some(Arc::new((Mutex::new(Vec::new()), Condvar::new())));
    env
}

fn loaded(ctx: &egui::Context, ratio: f32) -> ThumbnailState {
    ThumbnailState::loaded_raster(
        ctx.load_texture(
            "container-type-test",
            egui::ColorImage::filled([32, 32], egui::Color32::RED),
            Default::default(),
        ),
        ThumbLoadOrigin::UpgradeableCache,
        false,
        32,
        crate::source_coverage::SourceCoverage::FullSource,
        Some((100, (100.0 * ratio) as u32)),
        None,
    )
}

#[test]
fn container_type_icons_kind_table_and_state_projection() {
    let ctx = egui::Context::default();
    let mut rows = kinds()
        .into_iter()
        .map(|(item, _)| item)
        .collect::<Vec<_>>();
    rows.extend([
        GridItem::Folder("Q:/".into()),
        GridItem::ZipDir {
            zip_path: "book.zip".into(),
            dir_prefix: "inner.zip/".into(),
            is_archive: true,
            representative: None,
        },
        GridItem::SearchContainer {
            path: "book.zip".into(),
            kind: crate::grid_item::SearchContainerKind::Zip,
            hit_count: 1,
            representative: None,
        },
        GridItem::PdfFile("book.epub".into()),
    ]);
    use ContainerIconKind::{Archive, Folder, PdfEpub};
    let expected = [
        Some(Folder),
        Some(Archive),
        Some(PdfEpub),
        Some(Archive),
        Some(Archive),
        Some(Folder),
        None,
        None,
        None,
        None,
        None,
        None,
        None,
        Some(Folder),
        Some(Archive),
        Some(Archive),
        Some(PdfEpub),
    ];
    for (item, kind) in rows.into_iter().zip(expected) {
        assert_eq!(container_icon_kind(&item), kind, "{item:?}");
        for mask in 0..8 {
            let selected = ContainerIconKinds {
                folder: mask & 1 != 0,
                archive: mask & 2 != 0,
                pdf_epub: mask & 4 != 0,
            };
            let target = kind.is_some_and(|kind| selected.contains(kind));
            assert_eq!(container_icon_only(&item, selected), target, "{item:?}");
            for state in [
                ThumbnailState::Pending,
                ThumbnailState::Evicted,
                ThumbnailState::NoArt,
                ThumbnailState::Failed,
                loaded(&ctx, 1.5),
            ] {
                let effective = effective_thumbnail_state(&item, &state, selected);
                if target {
                    assert!(matches!(effective, ThumbnailState::NoArt));
                } else {
                    assert!(std::ptr::eq(effective, &state));
                }
            }
        }
    }
}

#[test]
fn container_icon_kinds_partial_selection_gates_only_selected_demand() {
    let ctx = egui::Context::default();
    for selected in [
        ContainerIconKinds {
            folder: true,
            ..ContainerIconKinds::NONE
        },
        ContainerIconKinds {
            archive: true,
            ..ContainerIconKinds::NONE
        },
        ContainerIconKinds {
            pdf_epub: true,
            ..ContainerIconKinds::NONE
        },
    ] {
        let mut items = kinds()
            .into_iter()
            .take(7)
            .map(|(item, _)| item)
            .collect::<Vec<_>>();
        items.extend([
            GridItem::SearchContainer {
                path: "c:/shelf/book.zip".into(),
                kind: crate::grid_item::SearchContainerKind::Zip,
                hit_count: 1,
                representative: Some(crate::grid_item::ContainerRepresentative {
                    path: "c:/shelf/book.zip".into(),
                    zip_entry: Some("1.png".into()),
                    pdf_page: None,
                }),
            },
            GridItem::ZipDir {
                zip_path: "c:/shelf/book.zip".into(),
                dir_prefix: "inner.zip/".into(),
                is_archive: true,
                representative: Some("inner.zip/1.png".into()),
            },
            GridItem::ZipImage {
                zip_path: "c:/shelf/book.zip".into(),
                entry_name: "1.png".into(),
            },
        ]);
        let mut env = fixture(items);
        let app = &mut env.app;
        app.settings.thumb_container_icon_kinds = selected;
        app.converted_archive_cache_paths.insert(
            crate::path_key::normalize_keep_drive(Path::new("c:/shelf/book.7z")),
            ConvertedArchiveSourceState::Pending,
        );
        let suppressed = [
            selected.folder,
            selected.archive,
            selected.pdf_epub,
            selected.archive,
            selected.archive,
            selected.folder,
            false,
            selected.archive,
            selected.archive,
            false,
        ];
        for (idx, suppressed) in suppressed.into_iter().enumerate() {
            if suppressed {
                assert_eq!(
                    app.enqueue_priority_thumbnail(idx),
                    PriorityThumbnailAdmission::NoArt
                );
            }
        }
        app.update_keep_range_and_requests(&ctx, Instant::now());
        for (idx, suppressed) in suppressed.into_iter().enumerate() {
            assert_eq!(
                app.requested.contains_key(&idx),
                !suppressed,
                "{selected:?}, idx={idx}"
            );
            assert_eq!(
                matches!(app.thumbnails[idx], ThumbnailState::NoArt),
                suppressed
            );
            let image = loaded(&ctx, 1.5);
            assert_eq!(
                matches!(
                    effective_thumbnail_state(&app.items[idx], &image, selected),
                    ThumbnailState::NoArt
                ),
                suppressed
            );
        }
    }
}

#[test]
fn container_icon_kinds_partial_selection_auto_aspect_counts_archive() {
    let ctx = egui::Context::default();
    let mut env = fixture(vec![
        GridItem::Folder("folder".into()),
        GridItem::ZipFile("book.zip".into()),
    ]);
    let app = &mut env.app;
    app.settings.thumb_container_icon_kinds = ContainerIconKinds {
        folder: true,
        ..ContainerIconKinds::NONE
    };
    app.settings.thumb_aspect_auto = true;
    app.thumbnails = vec![ThumbnailState::NoArt, ThumbnailState::Pending];
    assert_eq!(app.auto_aspect_eligible_total(), 1);
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert_eq!(
        app.auto_aspect.current, None,
        "pending archive still prevents completion"
    );
    app.thumbnails[1] = loaded(&ctx, 1.5);
    app.rebuild_auto_aspect_samples_from_loaded();
    assert_eq!(app.auto_aspect.samples, HashMap::from([(1, 1.5)]));
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    let (candidate, _, count) = app.auto_aspect.streak.unwrap();
    assert_eq!(candidate, crate::settings::ThumbAspect::Portrait2x3);
    assert_eq!(count, 1);
    app.auto_aspect.streak = Some((candidate, Instant::now() - Duration::from_secs(1), count));
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert_eq!(
        app.auto_aspect.current,
        Some(crate::settings::ThumbAspect::Portrait2x3)
    );
}

#[test]
fn container_icon_kinds_preferences_swap_preserves_requests_and_parked_viewer() {
    let ctx = egui::Context::default();
    for old in [
        ContainerIconKinds::ALL,
        ContainerIconKinds {
            folder: true,
            archive: false,
            pdf_epub: true,
        },
    ] {
        let mut env = fixture(vec![
            GridItem::Folder("folder".into()),
            GridItem::ZipFile("book.zip".into()),
            GridItem::PdfFile("book.pdf".into()),
            GridItem::Image("image.png".into()),
        ]);
        let app = &mut env.app;
        app.settings.thumb_container_icon_kinds = old;
        app.thumbnails = vec![
            ThumbnailState::NoArt,
            if old.archive {
                ThumbnailState::NoArt
            } else {
                ThumbnailState::Pending
            },
            ThumbnailState::NoArt,
            ThumbnailState::Pending,
        ];
        let id = app
            .requested
            .begin(3, app.items_generation, ThumbnailRequestKind::Normal, None)
            .unwrap();
        let sibling = app.build_window_context_for_test(1770, |mounted| {
            mounted.items = vec![GridItem::Folder("sibling".into())];
            mounted.thumbnails = vec![ThumbnailState::NoArt];
            mounted.items_generation = 1770;
        });
        let items = app.items.clone();
        let generation = app.items_generation;
        app.selected = Some(2);
        app.grid_scroll.set_position(120.0);
        let scroll = app.grid_scroll.clone();
        app.set_visibility_preferences_for_test(app.settings.file_type_visibility.clone());
        app.pref_state
            .as_mut()
            .unwrap()
            .settings
            .thumb_container_icon_kinds = ContainerIconKinds {
            folder: false,
            archive: true,
            pdf_epub: true,
        };
        app.apply_preferences_dialog(&ctx);
        assert!(matches!(app.thumbnails[0], ThumbnailState::Pending));
        assert_eq!(
            matches!(app.thumbnails[1], ThumbnailState::NoArt),
            old.archive
        );
        assert!(matches!(app.thumbnails[2], ThumbnailState::NoArt));
        assert_eq!(app.items, items);
        assert_eq!(app.items_generation, generation);
        assert_eq!(app.selected, Some(2));
        assert_eq!(app.grid_scroll, scroll);
        assert_eq!(app.requested.len(), 1);
        assert_eq!(app.requested.id(3), Some(id));
        app.update_keep_range_and_requests(&ctx, Instant::now());
        assert!(matches!(app.thumbnails[1], ThumbnailState::NoArt));
        assert!(!app.requested.contains_key(&1));
        assert_eq!(app.requested.id(3), Some(id));
        app.with_viewer_context(sibling, |mounted| {
            assert!(matches!(mounted.thumbnails[0], ThumbnailState::NoArt));
            assert_eq!(mounted.items_generation, 1770);
            assert!(mounted.requested.is_empty());
        })
        .unwrap();
    }
}

#[test]
fn container_type_icons_keep_and_priority_demand_gate() {
    let ctx = egui::Context::default();
    for enabled in [false, true] {
        // Video's dedicated batch producer is unchanged; its generic admission has no request.
        let mut env = fixture(kinds().into_iter().take(12).map(|(item, _)| item).collect());
        let app = &mut env.app;
        app.settings.thumb_container_icon_kinds = if enabled {
            crate::grid_item::ContainerIconKinds::ALL
        } else {
            crate::grid_item::ContainerIconKinds::NONE
        };
        app.converted_archive_cache_paths.insert(
            crate::path_key::normalize_keep_drive(Path::new("c:/shelf/book.7z")),
            ConvertedArchiveSourceState::Pending,
        );
        app.thumbnails[1] = ThumbnailState::Evicted;
        if enabled {
            // Priority demand must be gated even before keep has normalized display state.
            for idx in 0..6 {
                assert_eq!(
                    app.enqueue_priority_thumbnail(idx),
                    PriorityThumbnailAdmission::NoArt
                );
            }
            assert!(app.requested.is_empty());
            assert!(matches!(app.thumbnails[0], ThumbnailState::Pending));
            assert!(matches!(app.thumbnails[1], ThumbnailState::Evicted));
        }
        app.update_keep_range_and_requests(&ctx, Instant::now());
        for idx in 0..6 {
            assert_eq!(app.requested.contains_key(&idx), !enabled, "idx={idx}");
            if enabled {
                assert!(matches!(app.thumbnails[idx], ThumbnailState::NoArt));
                assert_eq!(
                    app.enqueue_priority_thumbnail(idx),
                    PriorityThumbnailAdmission::NoArt
                );
            }
        }
        for idx in [6, 8, 9, 10, 11] {
            assert!(
                app.requested.contains_key(&idx),
                "leaf/stack demand must survive idx={idx}"
            );
        }
        assert!(!app.requested.contains_key(&7));
        assert!(matches!(app.thumbnails[7], ThumbnailState::Pending));
    }
}

#[test]
fn container_type_icons_drive_pin_demand_gate() {
    use crate::folder_thumb_pins::{FileKind, FolderPinSource};
    let ctx = egui::Context::default();
    for enabled in [false, true] {
        let mut env = fixture(vec![GridItem::Folder("Q:/".into())]);
        let app = &mut env.app;
        app.items_are_drive_list = true;
        app.settings.thumb_container_icon_kinds = if enabled {
            crate::grid_item::ContainerIconKinds::ALL
        } else {
            crate::grid_item::ContainerIconKinds::NONE
        };
        app.folder_pin_map.insert(
            crate::path_key::normalize_keep_drive(Path::new("Q:/")),
            FolderPinSource::File {
                rel: "cover.png".into(),
                kind: FileKind::Image,
            }
            .into(),
        );
        app.update_keep_range_and_requests(&ctx, Instant::now());
        assert_eq!(app.requested.contains_key(&0), !enabled);
        assert_eq!(matches!(app.thumbnails[0], ThumbnailState::NoArt), enabled);
    }
}

#[test]
fn container_type_icons_finished_deferred_slot_is_no_art_without_removing_admission_owner() {
    let ctx = egui::Context::default();
    let mut env = fixture(vec![kinds().remove(3).0]);
    let app = &mut env.app;
    let id = app
        .requested
        .begin(0, app.items_generation, ThumbnailRequestKind::Normal, None)
        .unwrap();
    app.send_thumbnail_for_test(ThumbMsg {
        idx: 0,
        items_gen: app.items_generation,
        request_id: id,
        input_seq: 0,
        payload: crate::thumb_loader::ThumbMsgPayload::CacheOnlyMiss,
    })
    .unwrap();
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert_eq!(app.requested.id(0), None);
    assert!(
        app.requested.contains_key(&0),
        "completed miss retains its archive-judgement admission slot"
    );
    app.settings.thumb_container_icon_kinds = crate::grid_item::ContainerIconKinds::ALL;
    app.update_keep_range_and_requests(&ctx, Instant::now());
    assert!(matches!(app.thumbnails[0], ThumbnailState::NoArt));
    assert!(
        app.requested.contains_key(&0),
        "NoArt does not retire deferred admission"
    );
}

#[test]
fn container_type_icons_toggle_keeps_request_until_both_source_terminals() {
    let ctx = egui::Context::default();
    for finalized_first in [false, true] {
        let mut env = fixture(vec![GridItem::Folder("c:/shelf/book".into())]);
        let app = &mut env.app;
        app.update_keep_range_and_requests(&ctx, Instant::now());
        let id = app.requested.id(0).unwrap();
        app.heavy_io_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .clear();
        app.reload_queue.as_ref().unwrap().0.lock().unwrap().clear();
        app.settings.thumb_container_icon_kinds = crate::grid_item::ContainerIconKinds::ALL;
        app.settings.thumb_aspect_auto = true;
        app.update_keep_range_and_requests(&ctx, Instant::now());
        assert_eq!(app.requested.id(0), Some(id));
        assert!(matches!(app.thumbnails[0], ThumbnailState::Pending));
        let message = |pixels, finalized| {
            ThumbMsg::from_legacy_parts(
                0,
                pixels,
                ThumbLoadOrigin::UpgradeableCache,
                false,
                None,
                Some((100, 150)),
                None,
                false,
                finalized,
                0,
                app.items_generation,
            )
            .with_request_id(id)
        };
        let pixel_message = message(
            Some(egui::ColorImage::filled([32, 48], egui::Color32::RED)),
            false,
        )
        .source_pixels();
        let finalize_message = message(None, true);
        let (first, second) = if finalized_first {
            (finalize_message, pixel_message)
        } else {
            (pixel_message, finalize_message)
        };
        app.send_thumbnail_for_test(first).unwrap();
        app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
        assert_eq!(app.requested.id(0), Some(id));
        app.update_keep_range_and_requests(&ctx, Instant::now());
        assert_eq!(app.requested.id(0), Some(id));
        app.send_thumbnail_for_test(second).unwrap();
        app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
        assert!(!app.requested.contains_key(&0));
        assert!(
            app.auto_aspect.samples.is_empty(),
            "late container pixels must not become aspect samples"
        );
        assert!(matches!(app.thumbnails[0], ThumbnailState::Loaded { .. }));
        assert!(matches!(
            effective_thumbnail_state(
                &app.items[0],
                &app.thumbnails[0],
                crate::grid_item::ContainerIconKinds::ALL
            ),
            ThumbnailState::NoArt
        ));
        app.enqueue_idle_upgrades();
        assert!(app.requested.is_empty());
    }
}

#[test]
fn container_type_icons_loaded_containers_skip_idle_upgrade_but_images_do_not() {
    let ctx = egui::Context::default();
    let mut env = fixture(vec![
        GridItem::Folder("c:/shelf/book".into()),
        GridItem::Image("c:/shelf/page.png".into()),
    ]);
    let app = &mut env.app;
    app.settings.thumb_container_icon_kinds = crate::grid_item::ContainerIconKinds::ALL;
    app.settings.thumb_idle_upgrade = true;
    app.thumbnails.fill(loaded(&ctx, 1.5));
    app.enqueue_idle_upgrades();
    assert!(!app.requested.contains_key(&0));
    assert!(app.requested.contains_key(&1));
}

#[test]
fn container_type_icons_auto_aspect_all_containers_finish_beyond_keep() {
    let ctx = egui::Context::default();
    let mut env = fixture(
        (0..100)
            .map(|idx| GridItem::Folder(format!("c:/shelf/{idx}").into()))
            .collect(),
    );
    let app = &mut env.app;
    app.settings.thumb_aspect_auto = true;
    app.settings.thumb_container_icon_kinds = crate::grid_item::ContainerIconKinds::ALL;
    app.last_viewport_h = 120.0;
    app.settings.thumb_next_pages = 0;
    app.update_keep_range_and_requests(&ctx, Instant::now());
    assert!(
        app.thumbnails
            .iter()
            .any(|state| matches!(state, ThumbnailState::Pending))
    );
    assert_eq!(app.auto_aspect_eligible_total(), 0);
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert_eq!(
        app.auto_aspect.current,
        Some(crate::settings::ThumbAspect::Square)
    );
    app.auto_aspect.current = None;
    app.reset_and_seed_auto_aspect(&Arc::new(std::sync::RwLock::new(HashMap::new())));
    assert_eq!(
        app.auto_aspect.current,
        Some(crate::settings::ThumbAspect::Square)
    );
}

#[test]
fn container_type_icons_auto_aspect_ignores_retained_samples_on_poll() {
    let ctx = egui::Context::default();
    let mut env = fixture(
        (0..10)
            .map(|idx| {
                if idx < 8 {
                    GridItem::Folder(format!("c:/shelf/{idx}").into())
                } else {
                    GridItem::Image(format!("c:/shelf/{idx}.png").into())
                }
            })
            .collect(),
    );
    let folder = env.tmp.path().join("aspect");
    let app = &mut env.app;
    app.current_folder = Some(folder.clone());
    app.settings.thumb_aspect_auto = true;
    app.auto_aspect.current = Some(crate::settings::ThumbAspect::Square);
    app.auto_aspect.samples.extend((0..8).map(|idx| (idx, 1.5)));
    app.auto_aspect.streak = Some((
        crate::settings::ThumbAspect::Portrait2x3,
        Instant::now() - Duration::from_secs(3),
        8,
    ));
    app.settings.thumb_container_icon_kinds = crate::grid_item::ContainerIconKinds::ALL;
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert_eq!(
        app.auto_aspect.current,
        Some(crate::settings::ThumbAspect::Square)
    );
    assert_eq!(app.auto_aspect.streak, None);
    assert_eq!(
        app.auto_aspect.samples.len(),
        8,
        "toggle judgement must not delete stored samples"
    );
    app.auto_aspect.samples.insert(8, 1.5);
    app.auto_aspect.samples.insert(9, 1.5);
    app.auto_aspect.cached_sample_gate = Some(10);
    app.maybe_apply_auto_aspect(false);
    let (candidate, _, count) = app.auto_aspect.streak.unwrap();
    assert_eq!(count, 2, "streak must count the same effective sample set");
    app.auto_aspect.streak = Some((candidate, Instant::now() - Duration::from_secs(2), count));
    app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
    assert_eq!(
        app.auto_aspect.current,
        Some(crate::settings::ThumbAspect::Portrait2x3)
    );
    assert_eq!(app.auto_aspect.cached_sample_gate, None);
    assert_eq!(app.auto_aspect.samples.len(), 10);
    assert_eq!(
        app.auto_aspect_cache_db
            .as_ref()
            .unwrap()
            .get(&folder)
            .unwrap()
            .sample_count,
        2
    );
}

#[test]
fn container_type_icons_auto_aspect_loaded_rebuild_and_cache_seed_exclude_containers() {
    let ctx = egui::Context::default();
    let mut env = fixture(vec![
        GridItem::Folder("c:/shelf/book".into()),
        GridItem::Image("c:/shelf/page.png".into()),
    ]);
    let app = &mut env.app;
    app.settings.thumb_aspect_auto = true;
    app.settings.thumb_container_icon_kinds = crate::grid_item::ContainerIconKinds::ALL;
    app.thumbnails = vec![loaded(&ctx, 2.0), loaded(&ctx, 1.0)];
    app.rebuild_auto_aspect_samples_from_loaded();
    assert_eq!(app.auto_aspect.samples, HashMap::from([(1, 1.0)]));
    let map = Arc::new(std::sync::RwLock::new(HashMap::from([(
        "page.png".into(),
        crate::catalog::CacheEntry {
            cover: crate::cover_identity::CoverIdentity::Legacy,
            mtime: 10,
            file_size: 20,
            jpeg_data: Vec::new(),
            source_dims: Some((100, 100)),
            layout_dims: None,
            folder_provenance: None,
            selection_proof: None,
        },
    )])));
    let request = make_load_request(
        &app.items[0],
        0,
        10,
        20,
        false,
        None,
        Some(app.settings.folder_thumb_sort),
        app.settings.folder_thumb_depth,
        &app.folder_pin_map,
        &app.converted_archive_cache_paths,
        None,
        None,
        app.folder_thumb_pin_db.as_deref(),
        app.video_pin_db.as_ref(),
        false,
    )
    .unwrap();
    let container_key = crate::thumb_loader::cache_key_for_request(&request)
        .unwrap()
        .into_owned();
    map.write().unwrap().insert(
        container_key,
        crate::catalog::CacheEntry {
            cover: crate::cover_identity::CoverIdentity::Legacy,
            mtime: request.mtime,
            file_size: request.file_size,
            jpeg_data: Vec::new(),
            source_dims: Some((100, 200)),
            layout_dims: None,
            folder_provenance: None,
            selection_proof: None,
        },
    );
    app.reset_and_seed_auto_aspect_with_seed(&map, None, Some(2));
    assert_eq!(app.auto_aspect.samples, HashMap::from([(1, 1.0)]));
    assert_eq!(app.auto_aspect_eligible_total(), 1);
}

#[test]
fn container_type_icons_preferences_leave_parked_viewer_untouched() {
    let mut env = fixture(vec![GridItem::Folder("c:/shelf/book".into())]);
    let app = &mut env.app;
    app.settings.thumb_container_icon_kinds = crate::grid_item::ContainerIconKinds::ALL;
    app.thumbnails[0] = ThumbnailState::NoArt;
    let sibling = app.build_window_context_for_test(1697, |mounted| {
        mounted.items = vec![GridItem::Folder("c:/sibling/book".into())];
        mounted.thumbnails = vec![ThumbnailState::NoArt];
        mounted.items_generation = 1697;
    });
    app.set_visibility_preferences_for_test(app.settings.file_type_visibility.clone());
    app.pref_state
        .as_mut()
        .unwrap()
        .settings
        .thumb_container_icon_kinds = crate::grid_item::ContainerIconKinds::NONE;
    app.apply_preferences_dialog(&egui::Context::default());
    assert!(matches!(app.thumbnails[0], ThumbnailState::Pending));
    app.with_viewer_context(sibling, |mounted| {
        assert!(matches!(mounted.thumbnails[0], ThumbnailState::NoArt));
        assert_eq!(mounted.items_generation, 1697);
        assert!(mounted.requested.is_empty());
    })
    .unwrap();
}

#[test]
fn container_type_icons_preferences_restore_pinned_and_plain_cache_without_deleting_rows() {
    use crate::folder_thumb_pins::{FileKind, FolderPinSource};
    use crate::thumb_loader::CacheDecision;

    let ctx = egui::Context::default();
    for pinned in [false, true] {
        let mut env = fixture(vec![GridItem::Folder("unused".into())]);
        let parent = env.tmp.path().join("shelf");
        let folder = parent.join("book");
        std::fs::create_dir_all(&folder).unwrap();
        // A cache miss cannot decode this file: a Loaded result proves cache reuse.
        std::fs::write(
            folder.join("cover.png"),
            b"source is deliberately not decodable",
        )
        .unwrap();
        let archive = folder.with_extension("zip");
        std::fs::write(&archive, b"archive is deliberately not decodable").unwrap();
        let cache_dir = env.tmp.path().join("catalogs");
        let catalog = Arc::new(crate::catalog::CatalogDb::open(&cache_dir, &parent).unwrap());
        let app = &mut env.app;
        app.current_folder = Some(parent);
        app.items[0] = if pinned {
            GridItem::Folder(folder.clone())
        } else {
            GridItem::ZipFile(archive)
        };
        let pin = FolderPinSource::File {
            rel: "cover.png".into(),
            kind: FileKind::Image,
        };
        if pinned {
            app.folder_thumb_pin_db
                .as_ref()
                .unwrap()
                .set(&folder, &pin)
                .unwrap();
            app.folder_pin_map.insert(
                crate::path_key::normalize_keep_drive(&folder),
                pin.clone().into(),
            );
        }
        assert_eq!(
            app.enqueue_priority_thumbnail(0),
            PriorityThumbnailAdmission::Enqueued
        );
        let mut request = app
            .heavy_io_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .pop()
            .unwrap();
        let key = crate::thumb_loader::cache_key_for_request(&request)
            .unwrap()
            .into_owned();
        assert_eq!(key.contains("#pin:"), pinned);
        let mut encoded = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            40,
            60,
            image::Rgba([90, 140, 180, 255]),
        ))
        .write_to(&mut encoded, image::ImageFormat::Png)
        .unwrap();
        catalog
            .save(
                &key,
                request.mtime,
                request.file_size,
                40,
                60,
                Some((40, 60)),
                encoded.get_ref(),
                &crate::cover_identity::CoverIdentity::Legacy,
            )
            .unwrap();
        // The first demand is canceled before execution; the next keep pass owns NoArt.
        app.requested.cancel_queued(0, request.request_id);
        if pinned {
            app.items.extend([
                GridItem::ZipFile("other.zip".into()),
                GridItem::PdfFile("other.pdf".into()),
            ]);
            app.thumbnails
                .extend([ThumbnailState::NoArt, ThumbnailState::NoArt]);
            app.image_metas.extend([Some((10, 20)); 2]);
            app.visible_indices.extend([1, 2]);
        }
        app.set_visibility_preferences_for_test(app.settings.file_type_visibility.clone());
        app.pref_state
            .as_mut()
            .unwrap()
            .settings
            .thumb_container_icon_kinds = crate::grid_item::ContainerIconKinds::ALL;
        app.apply_preferences_dialog(&ctx);
        assert!(
            matches!(app.thumbnails[0], ThumbnailState::Pending),
            "ON does not reset display state"
        );
        app.update_keep_range_and_requests(&ctx, Instant::now());
        assert!(matches!(app.thumbnails[0], ThumbnailState::NoArt));
        assert!(app.requested.is_empty());
        app.set_visibility_preferences_for_test(app.settings.file_type_visibility.clone());
        app.pref_state
            .as_mut()
            .unwrap()
            .settings
            .thumb_container_icon_kinds = if pinned {
            ContainerIconKinds {
                folder: false,
                archive: true,
                pdf_epub: true,
            }
        } else {
            ContainerIconKinds::NONE
        };
        app.apply_preferences_dialog(&ctx);
        assert!(matches!(app.thumbnails[0], ThumbnailState::Pending));
        if pinned {
            assert!(matches!(app.thumbnails[1], ThumbnailState::NoArt));
            assert!(matches!(app.thumbnails[2], ThumbnailState::NoArt));
        }
        app.update_keep_range_and_requests(&ctx, Instant::now());
        request = app
            .heavy_io_queue
            .as_ref()
            .unwrap()
            .0
            .lock()
            .unwrap()
            .pop()
            .unwrap();
        assert_eq!(
            crate::thumb_loader::cache_key_for_request(&request).unwrap(),
            key
        );
        let cache_map = std::sync::RwLock::new(catalog.load_all().unwrap());
        crate::thumb_loader::process_load_request(
            &mut request,
            &cache_map,
            &app.tx,
            Some(&catalog),
            64,
            75,
            120,
            CacheDecision::from_settings(&app.settings),
            &app.cache_gen_done,
            &Arc::new(Mutex::new(crate::stats::ThumbStats::default())),
            None,
            &app.keep_start_shared,
            &app.keep_end_shared,
            None,
            None,
            None,
            None,
            None,
        );
        app.poll_thumbnails(&ctx, ThumbnailConsumptionPolicy::Grid);
        assert!(matches!(
            app.thumbnails[0],
            ThumbnailState::Loaded {
                origin: ThumbLoadOrigin::UpgradeableCache | ThumbLoadOrigin::FinalCache,
                ..
            }
        ));
        assert!(app.requested.is_empty());
        assert_eq!(
            catalog.load_one(&key).unwrap().unwrap().jpeg_data,
            *encoded.get_ref()
        );
        assert_eq!(
            app.folder_thumb_pin_db.as_ref().unwrap().lookup(&folder),
            pinned.then_some(pin.into())
        );
    }
}

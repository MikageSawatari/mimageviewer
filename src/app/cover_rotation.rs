//! Book tiles keep unrotated pixels and obtain their source page angle at draw time.

use super::{App, GridItem, ThumbnailState};
use crate::cover_source_scope::LoadedCoverIdentity;
use crate::rotation_db::Rotation;

impl App {
    /// A retained list skips worker adoption. Recheck old materials once when that list
    /// becomes live, using the same predicate and reload path as a rotation change.
    pub(crate) fn reconcile_restored_cover_thumbnails(&mut self) -> bool {
        let matching = self
            .items
            .iter()
            .zip(&self.thumbnails)
            .enumerate()
            .filter_map(|(idx, (item, thumbnail))| {
                legacy_book_cover_needs_reload(item, thumbnail, &self.rotation_page_keys)
                    .then_some(idx)
            })
            .collect::<Vec<_>>();
        let changed = !matching.is_empty();
        for idx in matching {
            self.evict_thumbnail_for_reload(idx);
        }
        changed
    }

    /// Ordinary page tiles retain their existing cache/DB lookup semantics.
    pub(crate) fn grid_tile_rotation(&mut self, idx: usize) -> Rotation {
        if self.items.get(idx).is_some_and(|item| {
            matches!(
                item,
                GridItem::Image(_)
                    | GridItem::ZipImage { .. }
                    | GridItem::PdfPage { .. }
                    | GridItem::Video(_)
            )
        }) {
            self.get_rotation(idx)
        } else {
            self.grid_tile_rotation_memory(idx)
        }
    }

    /// Hover and cover drawing never read the DB or resolve the configured books root.
    pub(crate) fn grid_tile_rotation_memory(&self, idx: usize) -> Rotation {
        let Some(item) = self.items.get(idx) else {
            return Rotation::None;
        };
        let key = match item {
            GridItem::Image(_)
            | GridItem::ZipImage { .. }
            | GridItem::PdfPage { .. }
            | GridItem::Video(_) => self.rotation_key_for_idx(idx),
            GridItem::Stack { representative, .. } => {
                Some(crate::adjustment_db::normalize_path(representative))
            }
            GridItem::SearchContainer {
                representative: Some(rep),
                ..
            } => Some(match (&rep.zip_entry, rep.pdf_page) {
                (Some(entry), _) => crate::adjustment_db::zip_entry_key(&rep.path, entry),
                (None, Some(page)) => crate::edit_source::page_key_for_pdf(&rep.path, page),
                (None, None) => crate::adjustment_db::normalize_path(&rep.path),
            }),
            GridItem::Folder(_)
            | GridItem::ZipFile(_)
            | GridItem::PdfFile(_)
            | GridItem::ConvertibleArchive { .. }
            | GridItem::ZipDir { .. } => match self.thumbnails.get(idx) {
                Some(ThumbnailState::Loaded {
                    cover: LoadedCoverIdentity::Page(key),
                    ..
                }) => Some(key.clone()),
                _ => None,
            },
            _ => None,
        };
        key.as_deref()
            .map(|key| self.rotation_for_page_key(key))
            .unwrap_or_default()
    }
}

pub(super) fn legacy_book_cover_needs_reload(
    item: &GridItem,
    thumbnail: &ThumbnailState,
    owner: &std::collections::BTreeMap<String, Rotation>,
) -> bool {
    matches!(
        item,
        GridItem::Folder(_)
            | GridItem::ZipFile(_)
            | GridItem::PdfFile(_)
            | GridItem::ConvertibleArchive { .. }
            | GridItem::ZipDir { .. }
    ) && matches!(thumbnail, ThumbnailState::Loaded {
            cover: LoadedCoverIdentity::Legacy { scope: Some(scope) }, ..
        } if scope.has_rotation(owner))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::cover_source_scope::CoverSourceScope;
    use std::path::PathBuf;

    fn loaded(ctx: &egui::Context, cover: LoadedCoverIdentity) -> ThumbnailState {
        let tex = ctx.load_texture(
            "cover-rotation",
            egui::ColorImage::filled([2, 1], egui::Color32::WHITE),
            Default::default(),
        );
        let mut state = ThumbnailState::loaded_raster(
            tex,
            crate::thumb_loader::ThumbLoadOrigin::SourceGenerated {
                evaluated_display_px: 2,
            },
            false,
            2,
            crate::source_coverage::SourceCoverage::FullSource,
            Some((2, 1)),
            None,
        );
        if let ThumbnailState::Loaded {
            cover: identity, ..
        } = &mut state
        {
            *identity = cover;
        }
        state
    }

    fn prepare_adoption(app: &mut App) {
        app.items = vec![GridItem::ZipFile(PathBuf::from("c:/adoption.zip"))];
        app.thumbnails = vec![ThumbnailState::Pending];
        app.image_metas = vec![Some((1, 1))];
        app.visible_indices = vec![0];
        app.keep_set.insert(0);
        app.keep_range = (0, 1);
        app.settings.thumb_aspect_auto = false;
    }

    fn begin_adoption_request(
        app: &mut App,
        scope: Option<CoverSourceScope>,
        refresh_legacy_cover: bool,
    ) -> crate::thumb_loader::LoadRequest {
        let mut req = crate::thumb_loader::LoadRequest {
            idx: 0,
            path: PathBuf::from("c:/adoption.zip"),
            items_gen: app.items_generation,
            source_policy: if scope.is_none() {
                crate::thumb_loader::LoadSourcePolicy::CacheOnly
            } else {
                crate::thumb_loader::LoadSourcePolicy::CacheOrSource
            },
            cover_scope: scope,
            refresh_legacy_cover,
            ..Default::default()
        };
        req.request_id = app
            .requested
            .begin_for_request(&req, super::super::ThumbnailRequestKind::Normal, None)
            .unwrap();
        req
    }

    fn legacy_result(req: &crate::thumb_loader::LoadRequest) -> crate::thumb_loader::ThumbMsg {
        crate::thumb_loader::ThumbMsg::from_legacy_parts(
            req.idx,
            Some(egui::ColorImage::filled([2, 1], egui::Color32::WHITE)),
            crate::thumb_loader::ThumbLoadOrigin::FinalCache,
            false,
            None,
            Some((2, 1)),
            None,
            false,
            false,
            0,
            req.items_gen,
        )
        .with_request_id(req.request_id)
        .with_cover(crate::cover_identity::CoverIdentity::Legacy)
    }

    #[test]
    fn cover_rotation_adoption_rejects_legacy_when_rotation_overtakes_worker_or_cpu_queue() {
        for cpu_queued in [false, true] {
            let mut app = super::super::setup_app_for_test();
            let ctx = egui::Context::default();
            prepare_adoption(&mut app);
            let scope = CoverSourceScope::archive(std::path::Path::new("c:/adoption.zip"));
            let req = begin_adoption_request(&mut app, Some(scope), false);
            let msg = legacy_result(&req);
            if cpu_queued {
                assert!(matches!(
                    app.requested.receive(msg),
                    super::super::ThumbnailReceipt::Queued
                ));
            } else {
                app.send_thumbnail_for_test(msg).unwrap();
            }
            app.update_rotation_page_owner([("c:/adoption.zip::cover.jpg".into(), Rotation::Cw90)]);
            assert!(
                app.requested.contains_key(&0),
                "pending material remains owned until its result is checked"
            );
            app.poll_thumbnails(&ctx, super::super::ThumbnailConsumptionPolicy::Grid);
            assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
            assert!(!app.requested.contains_key(&0));
            assert!(!app.thumb_pixels.contains_key(&0));
            assert!(app.requested.next_image().is_none());
        }
    }

    #[test]
    fn cover_rotation_adoption_retires_shown_provisional_and_ignores_its_late_result() {
        let mut app = super::super::setup_app_for_test();
        let ctx = egui::Context::default();
        prepare_adoption(&mut app);
        let scope = CoverSourceScope::archive(std::path::Path::new("c:/adoption.zip"));
        let req = begin_adoption_request(&mut app, Some(scope.clone()), false);
        app.send_thumbnail_for_test(legacy_result(&req).provisional_pixels())
            .unwrap();
        app.poll_thumbnails(&ctx, super::super::ThumbnailConsumptionPolicy::Grid);
        assert!(matches!(&app.thumbnails[0], ThumbnailState::Loaded {
            cover: LoadedCoverIdentity::Legacy { scope: Some(loaded_scope) }, ..
        } if loaded_scope == &scope));
        assert!(
            app.requested.contains_key(&0),
            "provisional display keeps its unfinished producer"
        );
        app.send_thumbnail_for_test(legacy_result(&req)).unwrap();
        app.update_rotation_page_owner([("c:/adoption.zip::cover.jpg".into(), Rotation::Cw90)]);
        assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
        assert!(!app.requested.contains_key(&0));
        // The ordinary next-frame request has new immutable source facts and a new ID.
        let renewed = begin_adoption_request(&mut app, Some(scope), true);
        app.poll_thumbnails(&ctx, super::super::ThumbnailConsumptionPolicy::Grid);
        assert_eq!(
            app.requested.id(0),
            Some(renewed.request_id),
            "late old pixels cannot retire or replace the renewal"
        );
        assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
    }

    #[test]
    fn cover_rotation_adoption_accepts_unreadable_refreshed_legacy_without_requeueing() {
        let mut app = super::super::setup_app_for_test();
        let ctx = egui::Context::default();
        prepare_adoption(&mut app);
        app.update_rotation_page_owner([("c:/adoption.zip::cover.jpg".into(), Rotation::Cw90)]);
        let scope = CoverSourceScope::archive(std::path::Path::new("c:/adoption.zip"));
        let req = begin_adoption_request(&mut app, Some(scope.clone()), true);
        app.send_thumbnail_for_test(legacy_result(&req)).unwrap();
        for _ in 0..3 {
            app.poll_thumbnails(&ctx, super::super::ThumbnailConsumptionPolicy::Grid);
            assert!(matches!(&app.thumbnails[0], ThumbnailState::Loaded {
                cover: LoadedCoverIdentity::Legacy { scope: Some(loaded_scope) },
                origin: crate::thumb_loader::ThumbLoadOrigin::FinalCache, ..
            } if loaded_scope == &scope));
            assert!(app.requested.is_empty());
        }
        assert!(
            app.idle_upgrade_cache_bypass_ineligible.contains(&0),
            "offline fallback is a final cache material, avoiding idle source retries"
        );
    }

    #[test]
    fn cover_rotation_adoption_accepts_cache_only_legacy_without_source_scope() {
        let mut app = super::super::setup_app_for_test();
        let ctx = egui::Context::default();
        prepare_adoption(&mut app);
        app.update_rotation_page_owner([("c:/adoption.zip::cover.jpg".into(), Rotation::Cw90)]);
        let req = begin_adoption_request(&mut app, None, false);
        app.send_thumbnail_for_test(legacy_result(&req)).unwrap();
        app.poll_thumbnails(&ctx, super::super::ThumbnailConsumptionPolicy::Grid);
        assert!(matches!(
            app.thumbnails[0],
            ThumbnailState::Loaded {
                cover: LoadedCoverIdentity::Legacy { scope: None },
                ..
            }
        ));
        assert!(app.requested.is_empty());
        app.update_rotation_page_owner([("c:/adoption.zip::cover.jpg".into(), Rotation::Cw180)]);
        assert!(
            app.thumbnails[0].has_pixels(),
            "CacheOnly material has no decode path to refresh"
        );
    }

    #[test]
    fn cover_rotation_shared_catalog_keeps_other_drive_tile_independent() {
        let mut app = super::super::setup_app_for_test();
        let cache = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        let original = PathBuf::from("D:/books/book.zip");
        let other = PathBuf::from("E:/books/book.zip");
        let original_key = crate::adjustment_db::zip_entry_key(&original, "cover.png");
        let other_key = crate::adjustment_db::zip_entry_key(&other, "cover.png");
        let writer =
            crate::catalog::CatalogDb::open(cache.path(), original.parent().unwrap()).unwrap();
        writer
            .save(
                "archivethumb:book.zip",
                1,
                2,
                2,
                1,
                None,
                b"same unrotated pixels",
                &crate::cover_identity::CoverIdentity::Page(original_key.clone()),
            )
            .unwrap();
        let reader = crate::catalog::CatalogDb::open_existing_read_only(
            cache.path(),
            other.parent().unwrap(),
        )
        .unwrap()
        .unwrap();
        let original_cover = writer
            .load_one("archivethumb:book.zip")
            .unwrap()
            .unwrap()
            .cover;
        let other_cover = reader
            .load_one("archivethumb:book.zip")
            .unwrap()
            .unwrap()
            .cover;
        assert_eq!(
            original_cover,
            crate::cover_identity::CoverIdentity::Page(original_key)
        );
        assert_eq!(
            other_cover,
            crate::cover_identity::CoverIdentity::Page(other_key)
        );
        app.items = vec![
            GridItem::ZipFile(original.clone()),
            GridItem::ZipFile(other.clone()),
            GridItem::ZipImage {
                zip_path: original,
                entry_name: "cover.png".into(),
            },
            GridItem::ZipImage {
                zip_path: other,
                entry_name: "cover.png".into(),
            },
        ];
        app.thumbnails = vec![
            loaded(
                &ctx,
                LoadedCoverIdentity::from_material(original_cover, None),
            ),
            loaded(&ctx, LoadedCoverIdentity::from_material(other_cover, None)),
            ThumbnailState::Pending,
            ThumbnailState::Pending,
        ];
        app.set_image_rotation(2, Rotation::Cw90).unwrap();
        assert_eq!(app.grid_tile_rotation(0), Rotation::Cw90);
        assert_eq!(app.grid_tile_rotation(1), Rotation::None);
        assert_eq!(app.grid_tile_rotation(2), Rotation::Cw90);
        assert_eq!(app.grid_tile_rotation(3), Rotation::None);
    }

    #[test]
    fn cover_rotation_converted_archive_catalog_round_trip_matches_actual_page_drive() {
        for converted_drive in ['c', 'd'] {
            let mut app = super::super::setup_app_for_test();
            let cache = tempfile::tempdir().unwrap();
            let ctx = egui::Context::default();
            let library = PathBuf::from("D:/books");
            let original = library.join("book.rar");
            let converted =
                PathBuf::from(format!("{converted_drive}:/data/archive_cache/book.zip"));
            let cover = crate::thumb_loader::cover_identity_for_location(
                &crate::thumb_loader::ReadLocation::ArchiveEntry {
                    archive: converted.clone(),
                    entry: "cover.png".into(),
                },
                Some(std::path::Path::new("D:/compiled-books")),
            );
            let writer = crate::catalog::CatalogDb::open(cache.path(), &library).unwrap();
            writer
                .save("archivethumb:book.rar", 1, 2, 2, 1, None, b"pixels", &cover)
                .unwrap();
            drop(writer);
            let reader = crate::catalog::CatalogDb::open_existing_read_only(cache.path(), &library)
                .unwrap()
                .unwrap();
            let cached = reader.load_one("archivethumb:book.rar").unwrap().unwrap();
            assert_eq!(cached.cover, cover);
            app.items = vec![
                GridItem::ConvertibleArchive {
                    path: original,
                    format: crate::archive_converter::ArchiveFormat::Rar,
                },
                GridItem::ZipImage {
                    zip_path: converted,
                    entry_name: "cover.png".into(),
                },
            ];
            app.thumbnails = vec![
                loaded(&ctx, LoadedCoverIdentity::from_material(cached.cover, None)),
                ThumbnailState::Pending,
            ];
            app.set_image_rotation(1, Rotation::Cw90).unwrap();
            assert_eq!(app.grid_tile_rotation(0), Rotation::Cw90);
            assert_eq!(app.grid_tile_rotation(1), Rotation::Cw90);
        }
    }

    #[test]
    fn cover_rotation_uses_material_page_and_current_owner_for_pin_zero_and_clear() {
        let mut app = super::super::setup_app_for_test();
        let ctx = egui::Context::default();
        let book = PathBuf::from("c:/books/comic.zip");
        let cover = crate::adjustment_db::zip_entry_key(&book, "cover.jpg");
        let second = crate::adjustment_db::zip_entry_key(&book, "second.jpg");
        app.items = vec![GridItem::ZipFile(book)];
        app.thumbnails = vec![loaded(&ctx, LoadedCoverIdentity::Page(cover.clone()))];
        app.update_rotation_page_owner([(second.clone(), Rotation::Cw270)]);
        assert_eq!(app.grid_tile_rotation(0), Rotation::None);
        assert!(
            app.thumbnails[0].has_pixels(),
            "known identity never evicts for another page"
        );
        app.update_rotation_page_owner([(cover.clone(), Rotation::Cw90)]);
        assert_eq!(app.grid_tile_rotation(0), Rotation::Cw90);
        app.thumbnails[0] = loaded(&ctx, LoadedCoverIdentity::Page(second.clone()));
        assert_eq!(
            app.grid_tile_rotation(0),
            Rotation::Cw270,
            "new pin follows its own page"
        );
        app.update_rotation_page_owner([(second, Rotation::None)]);
        assert_eq!(app.grid_tile_rotation(0), Rotation::None);
        app.thumbnails[0] = loaded(&ctx, LoadedCoverIdentity::Page(cover));
        app.replace_rotation_page_owner(Default::default());
        assert_eq!(app.grid_tile_rotation(0), Rotation::None);
        assert!(app.thumbnails[0].has_pixels());
    }

    #[test]
    fn cover_rotation_memory_uses_page_stack_and_search_keys_without_db_reads() {
        use crate::grid_item::{ContainerRepresentative, SearchContainerKind};
        let mut app = super::super::setup_app_for_test();
        let path = PathBuf::from("c:/photos/page.jpg");
        let zip = PathBuf::from("c:/books/book.zip");
        let pdf = PathBuf::from("c:/books/book.pdf");
        app.items = vec![
            GridItem::Image(path.clone()),
            GridItem::Video(path.clone()),
            GridItem::ZipImage {
                zip_path: zip.clone(),
                entry_name: "a.jpg".into(),
            },
            GridItem::PdfPage {
                pdf_path: pdf.clone(),
                page_num: 2,
                content_type: None,
            },
            GridItem::Stack {
                key: "page".into(),
                representative: path.clone(),
                count: 2,
            },
            GridItem::SearchContainer {
                path: zip.clone(),
                kind: SearchContainerKind::Zip,
                hit_count: 1,
                representative: Some(ContainerRepresentative {
                    path: zip.clone(),
                    zip_entry: Some("a.jpg".into()),
                    pdf_page: None,
                }),
            },
            GridItem::SearchContainer {
                path: pdf.clone(),
                kind: SearchContainerKind::Folder,
                hit_count: 1,
                representative: Some(ContainerRepresentative {
                    path: pdf.clone(),
                    zip_entry: None,
                    pdf_page: Some(2),
                }),
            },
        ];
        app.update_rotation_page_owner([
            (crate::adjustment_db::normalize_path(&path), Rotation::Cw90),
            (
                crate::adjustment_db::zip_entry_key(&zip, "a.jpg"),
                Rotation::Cw180,
            ),
            (
                crate::edit_source::page_key_for_pdf(&pdf, 2),
                Rotation::Cw270,
            ),
        ]);
        let reads = crate::rotation_db::read_operations_for_test();
        for (idx, expected) in [
            Rotation::Cw90,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ]
        .into_iter()
        .enumerate()
        {
            assert_eq!(app.grid_tile_rotation_memory(idx), expected);
        }
        assert_eq!(crate::rotation_db::read_operations_for_test(), reads);
    }

    #[test]
    fn cover_rotation_preserves_ordinary_page_cache_semantics() {
        let mut app = super::super::setup_app_for_test();
        app.items = vec![GridItem::Image(PathBuf::from("c:/page.jpg"))];
        app.rotation_cache.insert(0, Rotation::Cw270);
        assert_eq!(app.grid_tile_rotation(0), Rotation::Cw270);
        assert_eq!(
            app.grid_tile_rotation_memory(0),
            Rotation::None,
            "hover reads only the angle owner, including the documented EPUB-copy limitation"
        );
    }

    #[test]
    fn cover_rotation_legacy_only_evicts_refreshable_book_rows_with_rotations() {
        let mut app = super::super::setup_app_for_test();
        let ctx = egui::Context::default();
        app.edit_preview_repaint_ctx = Some(ctx.clone());
        app.items = (0..5)
            .map(|idx| GridItem::ZipFile(PathBuf::from(format!("c:/book{idx}.zip"))))
            .collect();
        app.items
            .push(GridItem::Image(PathBuf::from("c:/page.jpg")));
        let matching = CoverSourceScope::archive(std::path::Path::new("c:/book0.zip"));
        app.thumbnails = vec![
            loaded(
                &ctx,
                LoadedCoverIdentity::Legacy {
                    scope: Some(matching.clone()),
                },
            ),
            loaded(
                &ctx,
                LoadedCoverIdentity::Legacy {
                    scope: Some(CoverSourceScope::archive(std::path::Path::new(
                        "c:/unrelated.zip",
                    ))),
                },
            ),
            loaded(&ctx, LoadedCoverIdentity::Legacy { scope: None }),
            loaded(&ctx, LoadedCoverIdentity::Unknown),
            loaded(&ctx, LoadedCoverIdentity::Unrotatable),
            loaded(
                &ctx,
                LoadedCoverIdentity::Legacy {
                    scope: Some(matching),
                },
            ),
        ];
        app.update_rotation_page_owner([("c:/book0.zip::cover.jpg".into(), Rotation::Cw90)]);
        assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
        assert!(app.thumbnails[1..].iter().all(ThumbnailState::has_pixels));
        assert_eq!(app.grid_tile_rotation_memory(2), Rotation::None);
        assert_eq!(app.grid_tile_rotation_memory(3), Rotation::None);
        assert_eq!(app.grid_tile_rotation_memory(4), Rotation::None);
        app.evict_legacy_cover_thumbnails();
        assert!(app.thumbnails[1..].iter().all(ThumbnailState::has_pixels));
    }

    #[cfg(windows)]
    #[test]
    fn cover_rotation_change_evicts_matching_legacy_across_sibling_contexts_only() {
        let mut app = super::super::setup_app_for_test();
        let ctx = egui::Context::default();
        let path = PathBuf::from("c:/book.zip");
        let scope = CoverSourceScope::archive(&path);
        app.items = vec![GridItem::ZipFile(path.clone())];
        app.thumbnails = vec![loaded(
            &ctx,
            LoadedCoverIdentity::Legacy {
                scope: Some(scope.clone()),
            },
        )];
        let sibling = app.build_window_context_for_test(171801, |owner| {
            owner.items = vec![
                GridItem::ZipFile(path.clone()),
                GridItem::ZipFile(PathBuf::from("c:/other.zip")),
            ];
            owner.thumbnails = vec![
                loaded(&ctx, LoadedCoverIdentity::Legacy { scope: Some(scope) }),
                loaded(
                    &ctx,
                    LoadedCoverIdentity::Legacy {
                        scope: Some(CoverSourceScope::archive(std::path::Path::new(
                            "c:/other.zip",
                        ))),
                    },
                ),
            ];
            owner.requested.extend([(0, false), (1, false)]);
            for idx in 0..2 {
                owner.thumb_pixels.insert(
                    idx,
                    std::sync::Arc::new(egui::ColorImage::filled([1, 1], egui::Color32::WHITE)),
                );
            }
        });
        app.update_rotation_page_owner([(
            crate::adjustment_db::zip_entry_key(&path, "cover.jpg"),
            Rotation::Cw90,
        )]);
        assert!(matches!(app.thumbnails[0], ThumbnailState::Evicted));
        app.with_owner_viewer_context(sibling, |owner| {
            assert!(matches!(owner.thumbnails[0], ThumbnailState::Evicted));
            assert!(owner.thumbnails[1].has_pixels());
            assert!(!owner.requested.contains_key(&0));
            assert!(owner.requested.contains_key(&1));
            assert!(!owner.thumb_pixels.contains_key(&0));
            assert!(owner.thumb_pixels.contains_key(&1));
        })
        .unwrap();
    }
}

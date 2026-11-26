//! Reopen sources use the same snapshot rules as mounted batch-export pages.
//! Only the reopen worker reads page adjustments; the UI shares the existing
//! saved-edit snapshot builder and its per-frame preparation budget (§1.177).

use crate::app::{App, PageAdjustmentTarget, PageIndexHint};
use crate::export_batch::BatchExportItem;
use crate::grid_item::GridItem;
use crate::image_combine::CombineSourceRef;
use std::path::Path;

/// Built once per reopen, yielding between rows rather than scanning per source.
pub(super) struct ReopenLookup {
    cursor: usize,
    generation: u64,
    keys: std::collections::HashMap<String, Option<usize>>,
}

impl ReopenLookup {
    pub(super) fn new(app: &App, sources: impl Iterator<Item = String>) -> Self {
        Self {
            cursor: 0,
            generation: app.items_generation,
            keys: sources.map(|key| (key, None)).collect(),
        }
    }

    pub(super) fn advance(
        &mut self,
        app: &App,
        mut remaining: impl FnMut() -> bool,
    ) -> Result<bool, String> {
        if app.items_generation != self.generation {
            return Err("一覧が変更されました。結合を開き直してください".into());
        }
        while self.cursor < app.items.len() {
            let idx = self.cursor;
            self.cursor += 1;
            if let Some(key) = app.page_path_key(idx)
                && let Some(found) = self.keys.get_mut(&key)
                && found.is_none()
            {
                *found = Some(idx);
            }
            if !remaining() {
                return Ok(self.cursor == app.items.len());
            }
        }
        Ok(true)
    }

    pub(super) fn get(&self, source: &CombineSourceRef) -> Option<usize> {
        self.keys.get(&source_page_key(source)).copied().flatten()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SourcePreparation {
    page_key: String,
    page_params: Option<crate::adjustment::AdjustParams>,
}

/// Worker only. Never create or migrate a store while resolving a recipe.
pub(crate) fn read_source_preparation(
    data_dir: &Path,
    source: &CombineSourceRef,
) -> Result<SourcePreparation, String> {
    use rusqlite::OptionalExtension;
    let page_key = source_page_key(source);
    let db_path = data_dir.join("adjustment.db");
    let page_params = match std::fs::metadata(&db_path) {
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => None,
        Err(error) => return Err(format!("補正設定を読み取れません: {error}")),
        Ok(_) => {
            let conn = rusqlite::Connection::open_with_flags(
                db_path,
                rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY,
            )
            .map_err(|error| format!("補正設定を読み取れません: {error}"))?;
            conn.busy_timeout(std::time::Duration::from_secs(2))
                .map_err(|error| format!("補正設定を読み取れません: {error}"))?;
            let json = conn
                .query_row(
                    "SELECT params_json FROM page_params WHERE page_path = ?1",
                    [&page_key],
                    |row| row.get::<_, String>(0),
                )
                .optional()
                .map_err(|error| format!("補正設定を読み取れません: {error}"))?;
            json.map(|json| serde_json::from_str(&json))
                .transpose()
                .map_err(|error| format!("補正設定を読み取れません: {error}"))?
        }
    };
    Ok(SourcePreparation {
        page_key,
        page_params,
    })
}

pub(super) fn source_page_key(source: &CombineSourceRef) -> String {
    match source {
        CombineSourceRef::File { path } => crate::adjustment_db::normalize_path(path),
        CombineSourceRef::ZipEntry {
            zip_path,
            entry_name,
        } => crate::adjustment_db::zip_entry_key(zip_path, entry_name),
        CombineSourceRef::PdfPage { pdf_path, page_num } => {
            crate::edit_source::page_key_for_pdf(pdf_path, *page_num)
        }
    }
}

fn source_grid_item(source: &CombineSourceRef) -> GridItem {
    match source {
        CombineSourceRef::File { path } => GridItem::Image(path.clone()),
        CombineSourceRef::ZipEntry {
            zip_path,
            entry_name,
        } => GridItem::ZipImage {
            zip_path: zip_path.clone(),
            entry_name: entry_name.clone(),
        },
        CombineSourceRef::PdfPage { pdf_path, page_num } => GridItem::PdfPage {
            pdf_path: pdf_path.clone(),
            page_num: *page_num,
            content_type: None,
        },
    }
}

impl App {
    pub(crate) fn image_combine_item_for_source(
        &mut self,
        source: &CombineSourceRef,
        prepared: &SourcePreparation,
        annotation_space: Option<[u32; 2]>,
        composition_identity: Option<&crate::book_resume_db::PageIdentity>,
        mounted_idx: Option<usize>,
    ) -> Result<BatchExportItem, String> {
        if annotation_space.is_some_and(|[width, height]| width == 0 || height == 0) {
            return Err("結合の構成に無効な注釈の基準寸法があります".into());
        }
        let annotation_space =
            annotation_space.map(|[width, height]| [width as usize, height as usize]);
        let key = source_page_key(source);
        if key != prepared.page_key {
            return Err("結合の元画像と補正設定が一致しません".to_string());
        }
        // Mounted pages retain live hydrated edits and the ordinary output
        // permission. Do not rebuild their snapshots through the absent path.
        if let Some(idx) = mounted_idx {
            if self.page_path_key(idx).as_deref() != Some(key.as_str()) {
                return Err("一覧が変更されました。結合を開き直してください".into());
            }
            // A recipe owns its source list independently of the current smart
            // folder's rule mask. The key was resolved above; direct snapshotting
            // also retains hydrated edits of a currently filtered-out row.
            let mut item = self
                .batch_export_item_for_idx(idx)?
                .ok_or_else(|| "この元画像は結合できません".to_string())?;
            if let CombineSourceRef::PdfPage { pdf_path, .. } = source
                && let crate::books::CompositeSource::PdfPage { password, .. } = &mut item.source
            {
                *password = self.image_combine_pdf_password(pdf_path);
            }
            // Reopen uses the recorded authoring canvas, including for a page
            // currently mounted at a different PDF render size. Missing PDF
            // spaces are resolved (or rejected) by the worker before composition.
            item.edits.comic_source_dims = annotation_space;
            if let Some(identity) = composition_identity
                && let Some(check) = item.composition_check.take()
            {
                item.composition_check = Some(check.with_identity(identity.clone()));
            }
            return Ok(item);
        }

        let (path, location_path, filename, composite_source) = match source {
            CombineSourceRef::File { path } => (
                path,
                path.parent().map(Path::to_path_buf),
                crate::capture::basename_for_path(path),
                crate::books::CompositeSource::File { path: path.clone() },
            ),
            CombineSourceRef::ZipEntry {
                zip_path,
                entry_name,
            } => {
                let entry = Path::new(crate::zip_loader::entry_basename(entry_name))
                    .file_stem()
                    .and_then(|stem| stem.to_str())
                    .map(crate::capture::basename_from_text)
                    .unwrap_or_else(|| "entry".to_string());
                (
                    zip_path,
                    Some(zip_path.clone()),
                    format!("{}_{entry}", crate::capture::basename_for_path(zip_path)),
                    crate::books::CompositeSource::ZipEntry {
                        zip_path: zip_path.clone(),
                        entry_name: entry_name.clone(),
                    },
                )
            }
            CombineSourceRef::PdfPage { pdf_path, page_num } => (
                pdf_path,
                Some(pdf_path.clone()),
                format!(
                    "{}_p{:04}",
                    crate::capture::basename_for_path(pdf_path),
                    page_num + 1
                ),
                crate::books::CompositeSource::PdfPage {
                    pdf_path: pdf_path.clone(),
                    page_num: *page_num,
                    password: self.image_combine_pdf_password(pdf_path),
                },
            ),
        };
        let compiled_book = matches!(source, CombineSourceRef::File { .. })
            && path.parent().is_some_and(|parent| {
                crate::books::is_direct_book_folder(&self.book_root_path(), parent)
            });
        let params = prepared.page_params.clone().unwrap_or_else(|| {
            self.adjustment_standard_params_for_target(&PageAdjustmentTarget {
                page_key: key.clone(),
                location_path: (!compiled_book).then_some(location_path).flatten(),
                sidecar_coords: None,
                compiled_book,
                idx_hint: PageIndexHint::Absent,
            })
        });
        // rotation_page_keys is the in-memory persistent owner. Opening a DB
        // here would add synchronous UI I/O and could miss a pending write.
        let rotation = self
            .rotation_page_keys
            .get(&key)
            .copied()
            .unwrap_or_default();
        let mut edits = self.book_baked_edit_snapshot(
            &key,
            None,
            params,
            rotation,
            self.settings.bake_stage_export_batch,
        )?;
        edits.comic_source_dims = annotation_space;
        let mut item = BatchExportItem::new(
            filename,
            Self::batch_export_dirname_for_path(path),
            composite_source,
            edits,
        );
        // Absent ZIP/PDF pages need the same worker-side composition admission.
        // No current navigation/tree state is borrowed for an unrelated source.
        if !matches!(source, CombineSourceRef::File { .. })
            && let Some(service) = self.book_composition_service.as_ref()
        {
            let grid_item = source_grid_item(source);
            if let Some(identity) = composition_identity
                .cloned()
                .or_else(|| crate::book_resume_db::PageIdentity::for_item(&grid_item, None))
            {
                item.composition_check =
                    Some(crate::book_composition::CompositionOutputCheck::new(
                        service.clone(),
                        crate::book_composition::CandidatePage {
                            identity,
                            physical_archive: match source {
                                CombineSourceRef::ZipEntry { zip_path, .. } => {
                                    Some(zip_path.clone())
                                }
                                _ => None,
                            },
                            raw_tree: None,
                        },
                    ));
            }
        }
        Ok(item)
    }

    fn image_combine_pdf_password(&self, pdf_path: &Path) -> Option<String> {
        // The shared open policy also offers an unrelated current password as
        // an open attempt. Reopen must leave that target unauthenticated instead.
        let password = self.pdf_open_password(pdf_path);
        if self
            .current_folder
            .as_deref()
            .is_some_and(|current| crate::folder_tree::path_eq(current, pdf_path))
            || self.pdf_passwords.get(pdf_path).is_some()
        {
            password
        } else {
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::adjustment::AdjustParams;
    use crate::rotation_db::Rotation;

    impl App {
        fn image_combine_test_item_for_source(
            &mut self,
            source: &CombineSourceRef,
            prepared: &SourcePreparation,
            annotation_space: Option<[u32; 2]>,
        ) -> Result<BatchExportItem, String> {
            let mut lookup = ReopenLookup::new(self, std::iter::once(source_page_key(source)));
            assert!(lookup.advance(self, || true).unwrap());
            self.image_combine_item_for_source(
                source,
                prepared,
                annotation_space,
                None,
                lookup.get(source),
            )
        }
    }

    #[test]
    fn image_combine_reopen_large_absent_sources_lookup_is_linear_and_yields_between_rows() {
        let mut app = crate::app::setup_app_for_test();
        app.items = (0..100_000)
            .map(|idx| GridItem::Image(app.tmp.path().join(format!("row{idx}.png"))))
            .collect();
        let sources = (0..100)
            .map(|idx| CombineSourceRef::File {
                path: app.tmp.path().join(format!("absent{idx}.png")),
            })
            .collect::<Vec<_>>();
        let before = app.image_combine.eligibility_work.get().page_keys;
        let mut lookup = ReopenLookup::new(&app, sources.iter().map(source_page_key));
        let mut done = false;
        while !done {
            let start = lookup.cursor;
            let mut rows = 0;
            done = lookup
                .advance(&app, || {
                    rows += 1;
                    rows < 257
                })
                .unwrap();
            assert!(lookup.cursor - start <= 257);
        }
        assert_eq!(
            app.image_combine.eligibility_work.get().page_keys - before,
            100_000
        );
        let prepared = SourcePreparation {
            page_key: String::new(),
            page_params: None,
        };
        for source in sources {
            assert_eq!(lookup.get(&source), None);
            let prepared = SourcePreparation {
                page_key: source_page_key(&source),
                ..prepared.clone()
            };
            app.image_combine_item_for_source(&source, &prepared, None, None, lookup.get(&source))
                .unwrap();
        }
        assert_eq!(
            app.image_combine.eligibility_work.get().page_keys - before,
            100_000,
            "absent snapshot preparation never scans mounted rows"
        );
    }

    #[test]
    fn image_combine_reopen_lookup_rejects_replaced_list() {
        let mut app = crate::app::setup_app_for_test();
        let mut lookup = ReopenLookup::new(&app, std::iter::empty());
        app.items_generation += 1;
        assert!(lookup.advance(&app, || true).is_err());
    }

    #[test]
    fn image_combine_reopen_converted_archive_mounted_and_off_list_reject_newly_hidden_page() {
        use crate::book_composition::{
            BookComposition, BookCompositionService, CompositionPageRef, PageDomain,
            ReadingDirection,
        };
        use std::io::Write;
        let mut app = crate::app::setup_app_for_test();
        let physical = app.tmp.path().join("converted.zip");
        let logical = app.tmp.path().join("original.rar");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&physical).unwrap());
        zip.start_file("page.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(b"fixture").unwrap();
        zip.finish().unwrap();
        let (service, owner) =
            BookCompositionService::spawn_at(app.tmp.path().join("composition"), None).unwrap();
        app.book_composition_service = Some(service.clone());
        let scope = crate::book_scope::BookScopeKey::archive(&logical, &[]).unwrap();
        let domain = PageDomain::ArchiveLevel {
            prefix: String::new(),
        };
        service
            .prepare(scope.clone(), domain.clone())
            .recv()
            .unwrap()
            .status
            .unwrap();
        let source = CombineSourceRef::ZipEntry {
            zip_path: physical.clone(),
            entry_name: "page.png".into(),
        };
        app.items = vec![source_grid_item(&source)];
        app.current_folder = Some(physical);
        app.archive_source_override = Some(logical);
        let mounted = app.batch_export_item_for_idx(0).unwrap().unwrap();
        let original_check = mounted.composition_check.as_ref().unwrap();
        assert!(original_check.permits().unwrap());
        let spec = crate::image_combine::CombineItemSpec {
            source: source.clone(),
            label_override: None,
            annotation_space: None,
            composition_identity: Some(original_check.identity().clone()),
        };
        // Exercise persistence: the physical cache and original identity are independent.
        let output = app.tmp.path().join("combined.png");
        let recipe = crate::image_combine::CombineRecipe {
            v: 1,
            options: Default::default(),
            format: crate::image_combine::CombineFormat::Png,
            items: vec![spec],
        };
        crate::image_combine::store::save_recipe(app.tmp.path(), &output, &recipe).unwrap();
        let spec = crate::image_combine::store::load_recipe(app.tmp.path(), &output)
            .unwrap()
            .unwrap()
            .items
            .remove(0);
        let page = CompositionPageRef::ArchiveEntry {
            entry_name: "page.png".into(),
        };
        let mut book = BookComposition::new(ReadingDirection::Ltr, vec![page.clone()]);
        book.hidden_pages = vec![page];
        service
            .save(scope, domain, book)
            .recv()
            .unwrap()
            .status
            .unwrap();
        // The mounted check was captured before the hide and must still read fresh policy.
        assert!(!original_check.permits().unwrap());
        let prepared = read_source_preparation(&crate::data_dir::get(), &source).unwrap();
        for mounted_idx in [Some(0), None] {
            if mounted_idx.is_none() {
                app.items.clear();
                app.current_folder = None;
                app.archive_source_override = None;
            }
            let reopened = app
                .image_combine_item_for_source(
                    &spec.source,
                    &prepared,
                    None,
                    spec.composition_identity.as_ref(),
                    mounted_idx,
                )
                .unwrap();
            assert!(
                !reopened
                    .composition_check
                    .as_ref()
                    .unwrap()
                    .permits()
                    .unwrap()
            );
        }
        owner.shutdown_and_join();
    }

    fn params(brightness: f32) -> AdjustParams {
        AdjustParams {
            brightness,
            ..Default::default()
        }
    }

    fn assert_snapshot_eq(expected: &BatchExportItem, actual: &BatchExportItem) {
        assert_eq!(expected.filename, actual.filename);
        assert_eq!(expected.dirname, actual.dirname);
        assert_eq!(expected.source_dir, actual.source_dir);
        assert_eq!(expected.source_ext, actual.source_ext);
        match (&expected.source, &actual.source) {
            (
                crate::books::CompositeSource::File { path: expected },
                crate::books::CompositeSource::File { path: actual },
            ) => assert_eq!(expected, actual),
            (
                crate::books::CompositeSource::ZipEntry {
                    zip_path: expected_path,
                    entry_name: expected_entry,
                },
                crate::books::CompositeSource::ZipEntry {
                    zip_path: actual_path,
                    entry_name: actual_entry,
                },
            ) => {
                assert_eq!(expected_path, actual_path);
                assert_eq!(expected_entry, actual_entry);
            }
            (
                crate::books::CompositeSource::PdfPage {
                    pdf_path: expected_path,
                    page_num: expected_page,
                    password: expected_password,
                },
                crate::books::CompositeSource::PdfPage {
                    pdf_path: actual_path,
                    page_num: actual_page,
                    password: actual_password,
                },
            ) => {
                assert_eq!(expected_path, actual_path);
                assert_eq!(expected_page, actual_page);
                assert_eq!(expected_password, actual_password);
            }
            _ => panic!("source types differ"),
        }
        assert_eq!(expected.edits.params, actual.edits.params);
        assert_eq!(expected.edits.rotation, actual.edits.rotation);
        assert_eq!(expected.edits.stage, actual.edits.stage);
        assert_eq!(
            expected.edits.comic_source_dims,
            actual.edits.comic_source_dims
        );
        assert_eq!(expected.edits.export_crop, actual.edits.export_crop);
        assert_eq!(
            expected.edits.crop_legacy_writeback,
            actual.edits.crop_legacy_writeback
        );
        assert_eq!(expected.edits.erase.is_some(), actual.edits.erase.is_some());
        assert_eq!(
            expected.edits.local_adjust.is_some(),
            actual.edits.local_adjust.is_some()
        );
        assert_eq!(expected.edits.comic.is_some(), actual.edits.comic.is_some());
        assert_eq!(
            expected.edits.creative_lut.is_some(),
            actual.edits.creative_lut.is_some()
        );
        assert_eq!(expected.edits.ai.is_some(), actual.edits.ai.is_some());
        assert_eq!(
            expected.composition_check.is_some(),
            actual.composition_check.is_some()
        );
        match (&expected.edits.conceal, &actual.edits.conceal) {
            (None, None) => {}
            (Some(expected), Some(actual)) => {
                assert_eq!(expected.mask.size, actual.mask.size);
                assert_eq!(expected.mask.bitmap, actual.mask.bitmap);
                assert_eq!(expected.mask.shapes.len(), actual.mask.shapes.len());
            }
            _ => panic!("conceal snapshots differ"),
        }
    }

    fn mounted_and_absent(
        app: &mut App,
        source: &CombineSourceRef,
    ) -> (BatchExportItem, BatchExportItem) {
        let preparation = read_source_preparation(&crate::data_dir::get(), source).unwrap();
        let target = super::super::ExportBatchTarget::Item {
            idx: 0,
            page_key: source_page_key(source),
        };
        let expected = app.batch_export_item_for_target(&target).unwrap().unwrap();
        let mounted = app
            .image_combine_test_item_for_source(source, &preparation, None)
            .unwrap();
        assert_snapshot_eq(&expected, &mounted);
        app.items.clear();
        app.adjustment_page_params.clear();
        app.rotation_cache.clear();
        app.export_crop_page_settings.clear();
        let actual = app
            .image_combine_test_item_for_source(source, &preparation, None)
            .unwrap();
        (expected, actual)
    }

    #[test]
    fn image_combine_source_defaults_match_mounted_file_zip_and_pdf() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.bake_stage_export_batch = crate::bake_stage::BakeStage::Edits;
        app.settings.global_preset = params(9.0);
        let directory = app.tmp.path().join("pictures");
        let parent = crate::settings::FavoriteEntry::new("parent".into(), directory.clone());
        app.adjustment_favorite_params
            .insert(parent.id, params(17.0));
        app.settings.favorites.push(parent);
        let sources = [
            (
                CombineSourceRef::File {
                    path: app.tmp.path().join("outside.png"),
                },
                9.0,
            ),
            (
                CombineSourceRef::File {
                    path: directory.join("plain.png"),
                },
                17.0,
            ),
            (
                CombineSourceRef::ZipEntry {
                    zip_path: directory.join("book.zip"),
                    entry_name: "pages/001.png".into(),
                },
                31.0,
            ),
            (
                CombineSourceRef::PdfPage {
                    pdf_path: directory.join("book.pdf"),
                    page_num: 2,
                },
                43.0,
            ),
        ];
        for (source, brightness) in sources {
            if let CombineSourceRef::ZipEntry { zip_path, .. } = &source {
                let favorite = crate::settings::FavoriteEntry::new("zip".into(), zip_path.clone());
                app.adjustment_favorite_params
                    .insert(favorite.id, params(brightness));
                app.settings.favorites.push(favorite);
            }
            if let CombineSourceRef::PdfPage { pdf_path, .. } = &source {
                let favorite = crate::settings::FavoriteEntry::new("pdf".into(), pdf_path.clone());
                app.adjustment_favorite_params
                    .insert(favorite.id, params(brightness));
                app.settings.favorites.push(favorite);
            }
            app.items = vec![source_grid_item(&source)];
            let (expected, actual) = mounted_and_absent(&mut app, &source);
            assert_snapshot_eq(&expected, &actual);
            assert_eq!(actual.edits.params.brightness, brightness);
        }
    }

    #[test]
    fn image_combine_compiled_page_defaults_and_legacy_saved_rotation_match_mounted() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.bake_stage_export_batch = crate::bake_stage::BakeStage::Edits;
        let root = app.tmp.path().join("books");
        app.settings.book_root = Some(root.clone());
        app.settings.global_preset = params(99.0);
        let favorite = crate::settings::FavoriteEntry::new("books".into(), root.clone());
        app.adjustment_favorite_params
            .insert(favorite.id, params(55.0));
        app.settings.favorites.push(favorite);
        let source = CombineSourceRef::File {
            path: root.join("book/001.png"),
        };
        let key = source_page_key(&source);
        app.rotation_page_keys.insert(key, Rotation::Cw180);
        app.rotation_cache.insert(0, Rotation::Cw180);
        app.items = vec![source_grid_item(&source)];
        let (expected, actual) = mounted_and_absent(&mut app, &source);
        assert_snapshot_eq(&expected, &actual);
        assert_eq!(actual.edits.params, AdjustParams::default());
        assert_eq!(actual.edits.rotation, Rotation::Cw180);
    }

    #[test]
    fn image_combine_saved_adjustment_rotation_crop_and_conceal_match_mounted() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.bake_stage_export_batch = crate::bake_stage::BakeStage::Edits;
        let source = CombineSourceRef::ZipEntry {
            zip_path: app.tmp.path().join("book.zip"),
            entry_name: "pages/002.png".into(),
        };
        let key = source_page_key(&source);
        let data_dir = crate::data_dir::get();
        let db =
            crate::adjustment_db::AdjustmentDb::open_at(&data_dir.join("adjustment.db")).unwrap();
        db.set_page_params(&key, &params(77.0)).unwrap();
        app.adjustment_db = Some(db);
        app.adjustment_page_params.insert(0, params(77.0));
        app.rotation_page_keys.insert(key.clone(), Rotation::Cw270);
        app.rotation_cache.insert(0, Rotation::Cw270);
        let crop = crate::export_crop::CropSettings::authored(
            crate::export_crop::CropRect {
                min_x: 1.0,
                min_y: 2.0,
                max_x: 8.0,
                max_y: 9.0,
            },
            crate::export_crop::CropAspectMode::Free,
            [10, 10],
        );
        let crop_db =
            crate::export_crop::CropDb::open_at(&data_dir.join("export_crop.db")).unwrap();
        crop_db.set(&key, crop).unwrap();
        app.export_crop_db = Some(crop_db);
        app.export_crop_page_settings.insert(0, crop);
        let conceal_db =
            crate::conceal_db::ConcealDb::open_at(&data_dir.join("conceal.db")).unwrap();
        conceal_db
            .set(&key, &[true, false, false, true], &[], 2, 2)
            .unwrap();
        app.conceal_db = Some(conceal_db);
        app.conceal_page_keys.insert(key);
        app.items = vec![source_grid_item(&source)];
        let (expected, actual) = mounted_and_absent(&mut app, &source);
        assert_snapshot_eq(&expected, &actual);
        assert_eq!(actual.edits.params.brightness, 77.0);
        assert_eq!(actual.edits.rotation, Rotation::Cw270);
        assert_eq!(actual.edits.export_crop, Some(crop));
        assert!(actual.edits.conceal.is_some());
    }

    #[test]
    fn image_combine_pdf_session_password_is_scoped_to_source() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.bake_stage_export_batch = crate::bake_stage::BakeStage::Edits;
        let pdf_path = app.tmp.path().join("target.pdf");
        let source = CombineSourceRef::PdfPage {
            pdf_path: pdf_path.clone(),
            page_num: 0,
        };
        let prepared = read_source_preparation(&crate::data_dir::get(), &source).unwrap();
        app.current_folder = Some(app.tmp.path().join("other.pdf"));
        app.pdf_current_password = Some("unrelated-secret".into());
        // Exercise the mounted override too: batch export uses current_password.
        for mounted in [false, true] {
            app.items = if mounted {
                vec![source_grid_item(&source)]
            } else {
                Vec::new()
            };
            let actual = app
                .image_combine_test_item_for_source(&source, &prepared, None)
                .unwrap();
            assert!(matches!(
                actual.source,
                crate::books::CompositeSource::PdfPage { password: None, .. }
            ));
        }
        app.current_folder = Some(pdf_path);
        let actual = app
            .image_combine_test_item_for_source(&source, &prepared, None)
            .unwrap();
        assert!(
            matches!(actual.source, crate::books::CompositeSource::PdfPage { password: Some(password), .. } if password == "unrelated-secret")
        );
    }

    #[cfg(windows)]
    #[test]
    fn image_combine_saved_pdf_password_matches_mounted_and_beats_unrelated_session() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.bake_stage_export_batch = crate::bake_stage::BakeStage::Edits;
        let pdf_path = app.tmp.path().join("saved.pdf");
        app.pdf_passwords.set(&pdf_path, "saved-secret");
        let source = CombineSourceRef::PdfPage {
            pdf_path: pdf_path.clone(),
            page_num: 0,
        };
        app.current_folder = Some(pdf_path.clone());
        app.pdf_current_password = Some("saved-secret".into());
        app.items = vec![source_grid_item(&source)];
        let (expected, actual) = mounted_and_absent(&mut app, &source);
        assert_snapshot_eq(&expected, &actual);
        app.current_folder = Some(app.tmp.path().join("other.pdf"));
        app.pdf_current_password = Some("wrong-secret".into());
        let prepared = read_source_preparation(&crate::data_dir::get(), &source).unwrap();
        let actual = app
            .image_combine_test_item_for_source(&source, &prepared, None)
            .unwrap();
        assert!(
            matches!(actual.source, crate::books::CompositeSource::PdfPage { password: Some(password), .. } if password == "saved-secret")
        );
    }

    #[test]
    fn image_combine_source_preparation_reads_without_creating_db_and_rejects_bad_rows() {
        let tmp = tempfile::tempdir().unwrap();
        let source = CombineSourceRef::File {
            path: tmp.path().join("source.png"),
        };
        assert!(
            read_source_preparation(tmp.path(), &source)
                .unwrap()
                .page_params
                .is_none()
        );
        let db_path = tmp.path().join("adjustment.db");
        assert!(!db_path.exists());
        let db = crate::adjustment_db::AdjustmentDb::open_at(&db_path).unwrap();
        db.set_page_params(&source_page_key(&source), &params(66.0))
            .unwrap();
        assert_eq!(
            read_source_preparation(tmp.path(), &source)
                .unwrap()
                .page_params
                .unwrap()
                .brightness,
            66.0
        );
        let conn = rusqlite::Connection::open(&db_path).unwrap();
        conn.execute("UPDATE page_params SET params_json = 'bad json'", [])
            .unwrap();
        assert!(read_source_preparation(tmp.path(), &source).is_err());
    }

    #[test]
    fn image_combine_source_preparation_cannot_be_applied_to_another_source() {
        let mut app = crate::app::setup_app_for_test();
        let first = CombineSourceRef::File {
            path: app.tmp.path().join("first.png"),
        };
        let second = CombineSourceRef::File {
            path: app.tmp.path().join("second.png"),
        };
        let prepared = read_source_preparation(&crate::data_dir::get(), &first).unwrap();
        assert!(
            app.image_combine_test_item_for_source(&second, &prepared, None)
                .is_err()
        );
    }

    #[test]
    fn image_combine_mounted_snapshot_preserves_current_memory_over_worker_adjustment() {
        let mut app = crate::app::setup_app_for_test();
        app.settings.bake_stage_export_batch = crate::bake_stage::BakeStage::Edits;
        let source = CombineSourceRef::File {
            path: app.tmp.path().join("current.png"),
        };
        let prepared = SourcePreparation {
            page_key: source_page_key(&source),
            page_params: Some(params(11.0)),
        };
        app.items = vec![source_grid_item(&source)];
        app.visible_indices.clear();
        app.adjustment_page_params.insert(0, params(29.0));
        app.rotation_cache.insert(0, Rotation::Cw90);
        let actual = app
            .image_combine_test_item_for_source(&source, &prepared, None)
            .unwrap();
        assert_eq!(actual.edits.params.brightness, 29.0);
        assert_eq!(actual.edits.rotation, Rotation::Cw90);
    }

    #[test]
    fn image_combine_vector_pdf_reopen_recorded_annotation_space_matches_in_list_snapshot() {
        use std::sync::Arc;
        let mut app = crate::app::setup_app_for_test();
        app.settings.bake_stage_export_batch = crate::bake_stage::BakeStage::Edits;
        let source = CombineSourceRef::PdfPage {
            pdf_path: app.tmp.path().join("vector.pdf"),
            page_num: 0,
        };
        let key = source_page_key(&source);
        let mut bubble = comic_core::BubbleObject::default();
        bubble.text = comic_core::TextBlock::default();
        bubble.auto_size = false;
        let objects = vec![comic_core::AnnotationObject::new_bubble(
            1,
            (200.0, 300.0),
            bubble,
        )];
        let fonts = comic_core::FontSet::new();
        let font_keys = crate::comic_overlay::referenced_font_keys(&objects)
            .into_iter()
            .collect();
        app.comic_fonts
            .seed_for_test(Some(Arc::new(fonts)), font_keys);
        let db =
            crate::comic_db::ComicDb::open_at(&crate::data_dir::get().join("comic.db")).unwrap();
        db.set(&key, &objects).unwrap();
        app.comic_db = Some(db);
        app.comic_page_keys.insert(key);
        app.items = vec![GridItem::PdfPage {
            pdf_path: crate::image_combine::store::source_path(&source).to_path_buf(),
            page_num: 0,
            content_type: Some(crate::pdf_loader::PdfPageContentType::Vector),
        }];
        let generation = app.items_generation;
        app.page_dims_cache.record(
            generation,
            0,
            crate::page_dims::PagePixelDimensions::Canonical(
                crate::page_dims::CanonicalPageDimensions::new(1600, 2400),
            ),
        );
        let expected = app.batch_export_item_for_idx(0).unwrap().unwrap();
        assert!(expected.edits.comic.is_some());
        assert_eq!(expected.edits.comic_source_dims, Some([1600, 2400]));
        let recorded = expected
            .edits
            .comic_source_dims
            .map(|[w, h]| [w as u32, h as u32]);
        let prepared = read_source_preparation(&crate::data_dir::get(), &source).unwrap();
        app.items.clear();
        let actual = app
            .image_combine_test_item_for_source(&source, &prepared, recorded)
            .unwrap();
        assert_snapshot_eq(&expected, &actual);
        assert_eq!(
            expected.edits.comic.as_ref().unwrap().objects,
            actual.edits.comic.as_ref().unwrap().objects
        );
        assert!(Arc::ptr_eq(
            &expected.edits.comic.as_ref().unwrap().fonts,
            &actual.edits.comic.as_ref().unwrap().fonts
        ));
        // Missing record is left unresolved for the worker's typed PDF analysis,
        // which rejects vectors instead of inventing a coordinate canvas.
        let missing = app
            .image_combine_test_item_for_source(&source, &prepared, None)
            .unwrap();
        assert!(missing.edits.comic.is_some());
        assert!(missing.edits.comic_source_dims.is_none());
    }
}

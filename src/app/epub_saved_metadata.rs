//! Adopt the worker's saved-PDF metadata before the dialog continues to open/reload.

use std::collections::BTreeSet;

use super::App;

fn in_saved_scope(root: &str, key: &str) -> bool {
    key == root
        || key
            .strip_prefix(root)
            .is_some_and(|tail| tail.starts_with("::"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rotation_db::Rotation;
    use std::path::Path;

    fn seed_families(data: &Path, key: &str, angle: i32, marker: &str) {
        drop(crate::adjustment_db::AdjustmentDb::open_at(&data.join("adjustment.db")).unwrap());
        drop(crate::mask_db::MaskDb::open_at(&data.join("mask.db")).unwrap());
        drop(crate::conceal_db::ConcealDb::open_at(&data.join("conceal.db")).unwrap());
        drop(
            crate::local_adjust_db::LocalAdjustDb::open_at(&data.join("local_adjust.db")).unwrap(),
        );
        drop(crate::comic_db::ComicDb::open_at(&data.join("comic.db")).unwrap());
        drop(crate::export_crop::CropDb::open_at(&data.join("export_crop.db")).unwrap());
        drop(crate::rotation_db::RotationDb::open_at(&data.join("rotation.db")).unwrap());
        for (file, sql) in [
            (
                "adjustment.db",
                "INSERT OR REPLACE INTO page_params (page_path,params_json) VALUES (?1,?2)",
            ),
            (
                "mask.db",
                "INSERT OR REPLACE INTO masks (path,mask_data,width,height) VALUES (?1,?2,1,1)",
            ),
            (
                "conceal.db",
                "INSERT OR REPLACE INTO conceal_entries (page_path,bitmap_data,bitmap_w,bitmap_h) VALUES (?1,?2,1,1)",
            ),
            (
                "local_adjust.db",
                "INSERT OR REPLACE INTO local_adjust_pages (page_path,layers_json) VALUES (?1,?2)",
            ),
            (
                "comic.db",
                "INSERT OR REPLACE INTO comic_entries (page_path,doc_version,doc_json) VALUES (?1,1,?2)",
            ),
            (
                "export_crop.db",
                "INSERT OR REPLACE INTO export_crop_pages (page_path,min_x,min_y,max_x,max_y,aspect_mode) VALUES (?1,0,0,1,1,?2)",
            ),
        ] {
            rusqlite::Connection::open(data.join(file))
                .unwrap()
                .execute(sql, rusqlite::params![key, marker])
                .unwrap();
        }
        rusqlite::Connection::open(data.join("rotation.db"))
            .unwrap()
            .execute(
                "INSERT OR REPLACE INTO rotations VALUES (?1,?2)",
                rusqlite::params![key, angle],
            )
            .unwrap();
    }

    fn assert_families(app: &App, key: &str, expected: bool) {
        for owner in [
            &app.adjusted_page_keys,
            &app.mask_page_keys,
            &app.conceal_page_keys,
            &app.local_adjust_page_keys,
            &app.comic_page_keys,
            &app.export_crop_page_keys,
        ] {
            assert_eq!(owner.contains(key), expected, "{key}");
        }
    }

    fn seed_owner(app: &mut App, key: &str) {
        for owner in [
            &mut app.adjusted_page_keys,
            &mut app.mask_page_keys,
            &mut app.conceal_page_keys,
            &mut app.local_adjust_page_keys,
            &mut app.comic_page_keys,
            &mut app.export_crop_page_keys,
        ] {
            owner.insert(key.to_string());
        }
        app.update_rotation_page_owner([(key.to_string(), Rotation::Cw90)]);
    }

    #[test]
    fn section1761_seven_families_copy_readback_adopts_empty_and_existing_destination() {
        let mut app = crate::app::setup_app_for_test();
        let data = app.tmp.path().to_path_buf();
        let source = data.join("seven.epub");
        let destination = source.with_extension("pdf");
        let root = crate::path_key::normalize_keep_drive(&destination);
        let source_root = crate::path_key::normalize_keep_drive(&source);
        // Both the exact container key and its virtual page are copied and read back.
        for suffix in ["", "::page_1"] {
            seed_families(&data, &format!("{source_root}{suffix}"), 90, "source");
        }
        let saved = crate::epub_convert::copy_saved_pdf_metadata(
            &data,
            &source,
            destination.clone(),
            false,
        );
        assert!(
            saved.user_data_errors.is_empty(),
            "{:?}",
            saved.user_data_errors
        );
        app.apply_epub_saved_metadata(saved.metadata);
        for suffix in ["", "::page_1"] {
            let key = format!("{root}{suffix}");
            assert_families(&app, &key, true);
            assert_eq!(app.rotation_for_page_key(&key), Rotation::Cw90);
            seed_families(&data, &key, 270, "destination");
        }
        let saved =
            crate::epub_convert::copy_saved_pdf_metadata(&data, &source, destination, false);
        assert!(
            saved.user_data_errors.is_empty(),
            "{:?}",
            saved.user_data_errors
        );
        app.apply_epub_saved_metadata(saved.metadata);
        for suffix in ["", "::page_1"] {
            let key = format!("{root}{suffix}");
            assert_families(&app, &key, true);
            assert_eq!(app.rotation_for_page_key(&key), Rotation::Cw270);
            // The existing destination payload, rather than the source value, wins.
            for (file, table, column, value) in [
                ("adjustment.db", "page_params", "page_path", "params_json"),
                ("mask.db", "masks", "path", "mask_data"),
                ("conceal.db", "conceal_entries", "page_path", "bitmap_data"),
                (
                    "local_adjust.db",
                    "local_adjust_pages",
                    "page_path",
                    "layers_json",
                ),
                ("comic.db", "comic_entries", "page_path", "doc_json"),
                (
                    "export_crop.db",
                    "export_crop_pages",
                    "page_path",
                    "aspect_mode",
                ),
            ] {
                let sql = format!("SELECT {value} FROM {table} WHERE {column}=?1");
                let actual: String = rusqlite::Connection::open(data.join(file))
                    .unwrap()
                    .query_row(&sql, [&key], |row| row.get(0))
                    .unwrap();
                assert_eq!(actual, "destination");
            }
        }
    }

    #[test]
    fn section1761_scope_replacement_removes_absent_and_zero_preserves_siblings() {
        let mut app = crate::app::setup_app_for_test();
        let destination = app.tmp.path().join("replace.pdf");
        let root = crate::path_key::normalize_keep_drive(&destination);
        let old_page = format!("{root}::page_1");
        let physical = format!("{root}/unrelated.png");
        let sibling = format!("{root}2::page_1");
        for key in [&root, &old_page, &physical, &sibling] {
            seed_owner(&mut app, key);
            app.comic_docs.insert(key.clone(), Vec::new());
        }
        let data = app.tmp.path().to_path_buf();
        seed_families(&data, &root, 0, "kept");
        let presence =
            crate::content_identity::load_container_restore_presence(&data, &destination).unwrap();
        app.apply_epub_saved_metadata(crate::epub_convert::SavedPdfMetadata {
            destination_root: root.clone(),
            presence: Ok(presence),
            store_mutations: Default::default(),
        });
        assert_families(&app, &root, true);
        assert_families(&app, &old_page, false);
        for key in [&root, &old_page] {
            assert_eq!(app.rotation_for_page_key(key), Rotation::None);
            assert!(!app.comic_docs.contains_key(key));
        }
        for key in [&physical, &sibling] {
            assert_families(&app, key, true);
            assert_eq!(app.rotation_for_page_key(key), Rotation::Cw90);
            assert!(app.comic_docs.contains_key(key));
        }
    }

    #[test]
    fn section1761_empty_scope_invalidates_pinned_preview_only_in_destination() {
        use crate::grid_item::{GridItem, ThumbnailState};

        let mut app = crate::app::setup_app_for_test();
        let root = crate::path_key::normalize_keep_drive(&app.tmp.path().join("preview.pdf"));
        let keys = [
            format!("{root}::page_1"),
            format!("{root}/physical"),
            format!("{root}2::page_1"),
        ];
        app.items = ["destination-pin", "physical-pin", "sibling-pin"]
            .into_iter()
            .map(|name| GridItem::Folder(app.tmp.path().join(name)))
            .collect();
        app.image_metas = vec![None; 3];
        assert!((0..3).all(|idx| app.page_path_key(idx).is_none()));
        let ctx = egui::Context::default();
        let tex = ctx.load_texture(
            "saved-pdf-pinned-preview",
            egui::ColorImage::filled([2, 1], egui::Color32::WHITE),
            Default::default(),
        );
        app.thumbnails = (0..3)
            .map(|_| {
                ThumbnailState::loaded_raster(
                    tex.clone(),
                    crate::thumb_loader::ThumbLoadOrigin::FinalCache,
                    true,
                    2,
                    crate::source_coverage::SourceCoverage::FullSource,
                    Some((2, 1)),
                    None,
                )
            })
            .collect();
        for (idx, key) in keys.iter().enumerate() {
            app.thumb_edit_preview_keys.insert(idx, key.clone());
            app.requested.insert(idx, false);
        }
        for owner in [
            &app.adjusted_page_keys,
            &app.mask_page_keys,
            &app.conceal_page_keys,
            &app.local_adjust_page_keys,
            &app.comic_page_keys,
            &app.export_crop_page_keys,
        ] {
            assert!(owner.is_empty());
        }
        assert!(app.rotation_page_keys.is_empty());
        app.apply_epub_saved_metadata(crate::epub_convert::SavedPdfMetadata {
            destination_root: root,
            presence: Ok(Default::default()),
            store_mutations: Default::default(),
        });
        assert!(matches!(&app.thumbnails[0], ThumbnailState::Evicted));
        assert!(!app.requested.contains_key(&0));
        for idx in [1, 2] {
            assert!(matches!(
                &app.thumbnails[idx],
                ThumbnailState::Loaded {
                    tex: retained,
                    from_edit_preview: true,
                    ..
                } if retained.id() == tex.id()
            ));
            assert!(app.requested.contains_key(&idx));
            assert_eq!(app.thumb_edit_preview_keys.get(&idx), Some(&keys[idx]));
        }
        for owner in [
            &app.adjusted_page_keys,
            &app.mask_page_keys,
            &app.conceal_page_keys,
            &app.local_adjust_page_keys,
            &app.comic_page_keys,
            &app.export_crop_page_keys,
        ] {
            assert!(owner.is_empty());
        }
        assert!(app.rotation_page_keys.is_empty());
    }

    #[test]
    fn section1761_readback_failure_keeps_presence_and_rotation_owners() {
        let mut app = crate::app::setup_app_for_test();
        let data = app.tmp.path().to_path_buf();
        let source = data.join("read_failure.epub");
        let destination = source.with_extension("pdf");
        let root = crate::path_key::normalize_keep_drive(&destination);
        seed_owner(&mut app, &root);
        // A valid SQLite file with no expected table makes the readonly query fail.
        let broken = rusqlite::Connection::open(data.join("rotation.db")).unwrap();
        broken.execute_batch("DROP TABLE rotations").unwrap();
        drop(broken);
        drop(crate::ai_caption_db::CaptionDb::open_at(data.join("ai_caption.db")).unwrap());
        rusqlite::Connection::open(data.join("ai_caption.db")).unwrap().execute(
            "INSERT INTO captions (item_key,mtime_ms,size,model_id,prompt_version,lang,caption,visible_text,cover_ref,keywords,status,created_at)
             VALUES (?1,123,456,'test',1,'ja','caption','','','[]','ok',789)",
            [crate::path_key::normalize_keep_drive(&source)],
        ).unwrap();
        app.caption_display.seed_for_test(&root);
        let epoch = app.derived_text_epoch_for_test(&data);
        let before = epoch.load(std::sync::atomic::Ordering::Acquire);
        let saved =
            crate::epub_convert::copy_saved_pdf_metadata(&data, &source, destination, false);
        assert!(saved.metadata.presence.is_err());
        assert!(!saved.user_data_errors.is_empty());
        app.apply_epub_saved_metadata(saved.metadata);
        assert_families(&app, &root, true);
        assert_eq!(app.rotation_for_page_key(&root), Rotation::Cw90);
        assert_eq!(epoch.load(std::sync::atomic::Ordering::Acquire), before + 1);
        assert!(!app.caption_display.contains_for_test(&root));
    }

    #[test]
    fn section1761_copy_report_notifies_caption_and_invalidates_negative_caches_without_ui_db_reads()
     {
        let mut app = crate::app::setup_app_for_test();
        let data = app.tmp.path().to_path_buf();
        let source = data.join("caption.epub");
        let destination = source.with_extension("pdf");
        let source_key = format!("{}::page_1", crate::path_key::normalize_keep_drive(&source));
        let destination_key = format!(
            "{}::page_1",
            crate::path_key::normalize_keep_drive(&destination)
        );
        seed_families(&data, &source_key, 90, "source");
        drop(crate::ai_caption_db::CaptionDb::open_at(data.join("ai_caption.db")).unwrap());
        let db = rusqlite::Connection::open(data.join("ai_caption.db")).unwrap();
        db.execute(
            "INSERT INTO captions (item_key,mtime_ms,size,model_id,prompt_version,lang,caption,visible_text,cover_ref,keywords,status,created_at)
             VALUES (?1,123,456,'test',1,'ja','caption','','','[]','ok',789)",
            [&source_key],
        ).unwrap();
        drop(db);
        app.items = vec![crate::grid_item::GridItem::PdfPage {
            pdf_path: destination.clone(),
            page_num: 1,
            content_type: None,
        }];
        // Materialize the existing no-rotation and no-comic sentinels before the copy.
        assert_eq!(app.get_rotation(0), Rotation::None);
        app.comic_docs.insert(destination_key.clone(), Vec::new());
        app.caption_display.seed_for_test(&destination_key);
        let epoch = app.derived_text_epoch_for_test(&data);
        let before = epoch.load(std::sync::atomic::Ordering::Acquire);
        let saved =
            crate::epub_convert::copy_saved_pdf_metadata(&data, &source, destination, false);
        assert!(
            saved.user_data_errors.is_empty(),
            "{:?}",
            saved.user_data_errors
        );
        assert!(saved.metadata.store_mutations.derived_text_changed());
        assert!(saved.metadata.store_mutations.caption_display_stale());
        let reads = crate::rotation_db::read_operations_for_test();
        app.apply_epub_saved_metadata(saved.metadata);
        assert_eq!(crate::rotation_db::read_operations_for_test(), reads);
        assert_eq!(epoch.load(std::sync::atomic::Ordering::Acquire), before + 1);
        assert!(!app.caption_display.contains_for_test(&destination_key));
        assert!(!app.comic_docs.contains_key(&destination_key));
        assert!(!app.rotation_cache.contains_key(&0));
        assert_eq!(app.rotation_for_page_key(&destination_key), Rotation::Cw90);
    }
}

fn replace_scope(owner: &mut BTreeSet<String>, root: &str, replacement: BTreeSet<String>) {
    for key in scope_keys(owner, root) {
        owner.remove(&key);
    }
    owner.extend(replacement);
}

fn scope_keys(owner: &impl crate::rotation_db::PageEditKeys, root: &str) -> Vec<String> {
    let prefix = format!("{root}::");
    let mut keys = owner
        .page_keys_from(prefix.clone())
        .take_while(|key| key.starts_with(&prefix))
        .cloned()
        .collect::<Vec<_>>();
    if owner.contains_page_key(root) {
        keys.push(root.to_string());
    }
    keys
}

impl App {
    pub(crate) fn apply_epub_saved_metadata(
        &mut self,
        report: crate::epub_convert::SavedPdfMetadata,
    ) {
        // Caption mutations are committed independently of the presence readback.
        self.apply_content_identity_store_mutations(report.store_mutations);
        let Ok(presence) = report.presence else {
            // Worker logged the read error and attached the existing partial-transfer warning.
            return;
        };
        let root = report.destination_root;
        let mut invalidated = presence.clone();
        invalidated.page_edits.extend(
            self.thumb_edit_preview_keys
                .values()
                .filter(|key| in_saved_scope(&root, key))
                .cloned(),
        );
        // Include removed edits and negative cache entries. The new DB state replaces the
        // entire destination scope; sibling containers and physical root/ descendants stay put.
        for owner in [
            &self.adjusted_page_keys,
            &self.mask_page_keys,
            &self.conceal_page_keys,
            &self.local_adjust_page_keys,
            &self.comic_page_keys,
            &self.export_crop_page_keys,
        ] {
            invalidated.page_edits.extend(scope_keys(owner, &root));
        }
        invalidated.comics.extend(
            self.comic_docs
                .keys()
                .filter(|key| in_saved_scope(&root, key))
                .cloned(),
        );
        for idx in 0..self.items.len() {
            if let Some(key) = self.rotation_key_for_idx(idx)
                && in_saved_scope(&root, &key)
            {
                invalidated.rotations.entry(key).or_default();
            }
            if let Some(key) = self.page_path_key(idx)
                && in_saved_scope(&root, &key)
            {
                invalidated.page_edits.insert(key);
            }
        }
        self.invalidate_content_restore_destination_caches(&invalidated);
        replace_scope(&mut self.adjusted_page_keys, &root, presence.adjusted);
        replace_scope(&mut self.mask_page_keys, &root, presence.masks);
        replace_scope(&mut self.conceal_page_keys, &root, presence.conceals);
        replace_scope(
            &mut self.local_adjust_page_keys,
            &root,
            presence.local_adjustments,
        );
        replace_scope(&mut self.comic_page_keys, &root, presence.comics);
        replace_scope(&mut self.export_crop_page_keys, &root, presence.crops);
        let mut rotations = scope_keys(&self.rotation_page_keys, &root)
            .into_iter()
            .map(|key| (key, crate::rotation_db::Rotation::None))
            .collect::<std::collections::BTreeMap<_, _>>();
        rotations.extend(presence.rotations);
        self.update_rotation_page_owner(rotations);
    }
}

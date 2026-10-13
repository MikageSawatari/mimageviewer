//! Everything sidecars prepared on the shared Search worker. UI capture and
//! freshness checks only clone initialized resources or read publication stamps.

use crate::app::ConvertedArchiveSourceState;
use crate::folder_thumb_pins::{FolderPinMutationStamp, FolderPinSource, FolderThumbPinDb};
use crate::grid_item::GridItem;
use crate::page_edit_write_epoch::{EditWriteEpoch, WriteStamp};
use crate::tags_db::{TAG_WRITES, TagsDb};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Clone, Default)]
pub(crate) struct SearchMetadataResources {
    pub(crate) pin_db: Option<Arc<FolderThumbPinDb>>,
    pub(crate) tags_db_path: Option<PathBuf>,
    pub(crate) video_pins: Option<crate::video_pins::VideoPinReadSource>,
}

pub(crate) enum SearchMetadataInstall {
    /// Preserve the Metadata search producer's existing install helpers.
    Existing,
    Prepared(EverythingMetadata),
}

pub(crate) struct EverythingMetadata {
    pub(crate) pins: HashMap<String, FolderPinSource>,
    pub(crate) converted: HashMap<String, ConvertedArchiveSourceState>,
    pub(crate) tags: HashMap<String, Vec<String>>,
    pub(crate) video_pin_blobs: Arc<HashMap<PathBuf, Vec<u8>>>,
    pub(crate) tag_stamp: WriteStamp,
    pin_stamp: Option<FolderPinMutationStamp>,
    tags_db_path: Option<PathBuf>,
    video_pin_stamp: Option<crate::video_pins::VideoPinMutationStamp>,
}

impl SearchMetadataInstall {
    /// A stale ordinary write asks the caller to prepare again before adoption.
    /// This method does no SQLite, filesystem, or archive work on the UI thread.
    pub(crate) fn is_current(&self, resources: &SearchMetadataResources) -> bool {
        match self {
            Self::Existing => true,
            Self::Prepared(prepared) => prepared.is_current(resources),
        }
    }
}

impl EverythingMetadata {
    pub(crate) fn is_current(&self, resources: &SearchMetadataResources) -> bool {
        self.tags_db_path == resources.tags_db_path
            && self.pin_stamp == resources.pin_db.as_ref().map(|db| db.mutation_stamp())
            && TAG_WRITES.accepts(self.tag_stamp)
            && self.video_pin_stamp
                == resources
                    .video_pins
                    .as_ref()
                    .map(|source| source.mutation_stamp())
    }
}

/// Worker-only. Never create/migrate a DB, inspect an archive, or discover a
/// default profile path. Cancellation remains owned by the surrounding worker.
pub(crate) fn prepare(
    items: &[GridItem],
    metas: &[Option<(i64, i64)>],
    resources: &SearchMetadataResources,
) -> Result<SearchMetadataInstall, String> {
    if items.len() != metas.len() {
        return Err("検索結果のメタデータ件数が一致しません。".to_owned());
    }
    let mut tag_stamp = TAG_WRITES.sample();
    let pin_stamp = resources.pin_db.as_ref().map(|db| db.mutation_stamp());
    let video_pin_stamp = resources
        .video_pins
        .as_ref()
        .map(|source| source.mutation_stamp());
    let video_paths = items
        .iter()
        .filter_map(|item| match item {
            GridItem::Video(path) => Some(path),
            _ => None,
        })
        .collect::<Vec<_>>();
    let video_pin_blobs = if !video_paths.is_empty() {
        if let Some(source) = &resources.video_pins {
            let db = crate::video_pins::VideoPinDb::open_readonly(&source.path)
                .map_err(|_| "検索結果の動画ピンを読み取れませんでした。".to_owned())?;
            db.lookup_webps_many(video_paths)
        } else {
            HashMap::new()
        }
    } else {
        HashMap::new()
    };
    let pins = resources.pin_db.as_ref().map_or_else(HashMap::new, |db| {
        db.lookup_many(items.iter().filter_map(GridItem::container_path))
    });
    let mut converted = HashMap::new();
    for (item, meta) in items.iter().zip(metas) {
        if let GridItem::ConvertibleArchive { path, .. } = item {
            let state = if meta.is_some() {
                ConvertedArchiveSourceState::Pending
            } else {
                ConvertedArchiveSourceState::Unavailable {
                    logical_source: None,
                }
            };
            converted
                .entry(crate::path_key::normalize_keep_drive(path))
                .and_modify(|current| {
                    if matches!(state, ConvertedArchiveSourceState::Pending) {
                        *current = ConvertedArchiveSourceState::Pending;
                    }
                })
                .or_insert(state);
        }
    }
    let mut keys = items
        .iter()
        .filter_map(crate::app::tag_item_path)
        .map(crate::tags_db::item_key_for_path)
        .collect::<Vec<_>>();
    keys.sort_unstable();
    keys.dedup();
    let mut tags = if let Some(path) = &resources.tags_db_path {
        let db = TagsDb::open_readonly(path)
            .map_err(|_| "検索結果のタグを読み取れませんでした。".to_owned())?;
        db.get_many_display_tags(&keys)
    } else {
        HashMap::new()
    };
    // An explicit empty value means this row was prepared, not lazily unread.
    for key in keys {
        tags.entry(key).or_default();
    }
    let after = TAG_WRITES.sample();
    if !EditWriteEpoch::read_is_stable(tag_stamp, after) {
        // Keep a permanently invalid immutable reply when a write overlapped.
        // The caller's ordinary freshness branch retries; no recovery state here.
        tag_stamp.active_writers = 1;
    }
    Ok(SearchMetadataInstall::Prepared(EverythingMetadata {
        pins,
        converted,
        tags,
        video_pin_blobs: Arc::new(video_pin_blobs),
        tag_stamp,
        pin_stamp,
        tags_db_path: resources.tags_db_path.clone(),
        video_pin_stamp,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::archive_converter::ArchiveFormat;
    use crate::page_edit_write_epoch::TestEpochScope;

    fn unpack(install: SearchMetadataInstall) -> EverythingMetadata {
        match install {
            SearchMetadataInstall::Prepared(metadata) => metadata,
            SearchMetadataInstall::Existing => panic!("Everything metadata must be prepared"),
        }
    }

    #[test]
    fn video_pin_payload_uses_readonly_worker_connection_and_rejects_changed_source() {
        let _epochs = TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("pins.db");
        let db = crate::video_pins::VideoPinDb::open_at(&path).unwrap();
        let video = temp.path().join("movie.mp4");
        db.set_pin(&video, 1.0, &[1, 2, 3]).unwrap();
        let resources = SearchMetadataResources {
            video_pins: db.read_source(),
            ..Default::default()
        };
        let items = [GridItem::Video(video.clone())];
        let payload = unpack(prepare(&items, &[Some((0, 1))], &resources).unwrap());
        assert_eq!(payload.video_pin_blobs[&video], [1, 2, 3]);
        assert_eq!(db.bulk_lookup_calls_for_test(), 0);
        assert!(payload.is_current(&resources));
        db.set_pin(&video, 2.0, &[4, 5]).unwrap();
        assert!(!payload.is_current(&resources));
        let updated = unpack(prepare(&items, &[Some((0, 1))], &resources).unwrap());
        assert_eq!(updated.video_pin_blobs[&video], [4, 5]);
        assert!(updated.is_current(&resources));
        let replacement = crate::video_pins::VideoPinDb::open_at(&path).unwrap();
        assert!(!updated.is_current(&SearchMetadataResources {
            video_pins: replacement.read_source(),
            ..Default::default()
        }));
        db.remove(&video).unwrap();
        assert!(!updated.is_current(&resources));
        let removed = unpack(prepare(&items, &[Some((0, 1))], &resources).unwrap());
        assert!(removed.video_pin_blobs.is_empty());
        assert!(removed.is_current(&resources));
        assert!(
            unpack(prepare(&items, &[Some((0, 1))], &Default::default()).unwrap())
                .video_pin_blobs
                .is_empty()
        );
    }

    #[test]
    fn prepared_without_initialized_resources_is_pure_and_registers_real_stamps_only() {
        let _epochs = TestEpochScope::fresh();
        let resources = SearchMetadataResources::default();
        let items = vec![
            GridItem::Image("C:/missing/image.jpg".into()),
            GridItem::ConvertibleArchive {
                path: "C:/missing/real.rar".into(),
                format: ArchiveFormat::Rar,
            },
            GridItem::ConvertibleArchive {
                path: "C:/missing/unknown.rar".into(),
                format: ArchiveFormat::Rar,
            },
        ];
        let payload =
            unpack(prepare(&items, &[Some((0, 7)), Some((0, 8)), None], &resources).unwrap());
        assert!(payload.is_current(&resources));
        assert!(payload.pins.is_empty());
        assert_eq!(payload.tags.len(), 3);
        assert!(payload.tags.values().all(Vec::is_empty));
        assert_eq!(
            payload.converted[&crate::path_key::normalize_keep_drive(std::path::Path::new(
                "C:/missing/real.rar"
            ))],
            ConvertedArchiveSourceState::Pending
        );
        assert_eq!(
            payload.converted[&crate::path_key::normalize_keep_drive(std::path::Path::new(
                "C:/missing/unknown.rar"
            ))],
            ConvertedArchiveSourceState::Unavailable {
                logical_source: None
            }
        );
        assert!(prepare(&items, &[], &resources).is_err());
    }

    #[test]
    fn worker_reads_only_supplied_initialized_stores_and_tracks_edits() {
        let _epochs = TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let pin_db = Arc::new(FolderThumbPinDb::open_at(&temp.path().join("pins.db")).unwrap());
        let tag_path = temp.path().join("tags.db");
        let mut tag_db = TagsDb::open_at(&tag_path).unwrap();
        let zip = PathBuf::from("C:/books/book.zip");
        let image = PathBuf::from("C:/images/photo.jpg");
        let pin = FolderPinSource::ZipEntry {
            zip_rel: String::new(),
            entry: "cover.jpg".to_owned(),
        };
        pin_db.set(&zip, &pin).unwrap();
        let key = crate::tags_db::item_key_for_path(&image);
        tag_db
            .set_item_tags(&key, ["日本語"], crate::tags_db::source::EDIT)
            .unwrap();
        let resources = SearchMetadataResources {
            pin_db: Some(Arc::clone(&pin_db)),
            tags_db_path: Some(tag_path.clone()),
            ..Default::default()
        };
        let items = [GridItem::ZipFile(zip.clone()), GridItem::Image(image)];
        let payload = unpack(prepare(&items, &[Some((0, 1)); 2], &resources).unwrap());
        assert!(payload.is_current(&resources));
        assert_eq!(payload.pins.len(), 1);
        assert_eq!(
            payload.pins[&crate::path_key::normalize_keep_drive(&zip)],
            pin
        );
        assert_eq!(payload.tags[&key], vec!["#日本語"]);
        assert_eq!(payload.tags.len(), 2);
        tag_db
            .set_item_tags(&key, ["changed"], crate::tags_db::source::EDIT)
            .unwrap();
        assert!(!payload.is_current(&resources));
        let refreshed = unpack(prepare(&items, &[Some((0, 1)); 2], &resources).unwrap());
        assert!(refreshed.is_current(&resources));
        pin_db.remove(&zip).unwrap();
        assert!(!refreshed.is_current(&resources));
        let final_payload = unpack(prepare(&items, &[Some((0, 1)); 2], &resources).unwrap());
        assert!(final_payload.pins.is_empty());
        let replacement = SearchMetadataResources {
            pin_db: Some(Arc::new(
                FolderThumbPinDb::open_at(&temp.path().join("other-pins.db")).unwrap(),
            )),
            tags_db_path: Some(tag_path),
            ..Default::default()
        };
        assert!(!final_payload.is_current(&replacement));
    }

    #[test]
    fn missing_or_uninitialized_tag_store_is_never_created_or_migrated() {
        let _epochs = TestEpochScope::fresh();
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("does-not-exist/tags.db");
        let resources = SearchMetadataResources {
            pin_db: None,
            tags_db_path: Some(missing.clone()),
            ..Default::default()
        };
        let items = [GridItem::Image("C:/photo.jpg".into())];
        assert!(prepare(&items, &[Some((0, 1))], &resources).is_err());
        assert!(!missing.exists());
        assert!(!missing.parent().unwrap().exists());
        let empty = temp.path().join("empty.db");
        let conn = rusqlite::Connection::open(&empty).unwrap();
        let resources = SearchMetadataResources {
            pin_db: None,
            tags_db_path: Some(empty),
            ..Default::default()
        };
        let _ = prepare(&items, &[Some((0, 1))], &resources);
        assert_eq!(
            conn.query_row("SELECT COUNT(*) FROM sqlite_master", [], |row| row
                .get::<_, i64>(0))
                .unwrap(),
            0
        );
    }

    #[test]
    fn in_flight_tag_write_rejects_payload_even_after_writer_finishes() {
        let _epochs = TestEpochScope::fresh();
        let resources = SearchMetadataResources::default();
        let writer = TAG_WRITES.begin();
        let payload = unpack(prepare(&[], &[], &resources).unwrap());
        assert!(!payload.is_current(&resources));
        drop(writer);
        assert!(!payload.is_current(&resources));
        assert!(
            prepare(&[], &[], &resources)
                .unwrap()
                .is_current(&resources)
        );
        assert!(SearchMetadataInstall::Existing.is_current(&resources));
    }
}

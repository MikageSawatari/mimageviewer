//! 旧 v1.0 XMP `dc:subject` タグを `tags.db` へ一度だけ seed する worker。
//!
//! Tantivy STORED tags からの一括移行に乗らなかった、お気に入り外・未索引ファイル向けの
//! 保険経路。UI スレッドではファイルを読まず、`tag_item_state` がある item は読み込み前に
//! skip してタグ削除済み item の復活を防ぐ。

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LegacySeedReport {
    pub candidate_items: usize,
    pub skipped_decided_items: usize,
    pub read_items: usize,
    pub imported_items: usize,
    pub inserted_tags: usize,
    pub marked_empty_items: usize,
    pub read_errors: usize,
    pub db_errors: usize,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LegacySeedResult {
    pub report: LegacySeedReport,
    pub cache_updates: Vec<(PathBuf, Vec<String>)>,
}

pub(crate) struct LegacySeedPending {
    cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<Result<LegacySeedResult, String>>,
}

impl LegacySeedPending {
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn spawn(data_dir: PathBuf, paths: Vec<PathBuf>) -> LegacySeedPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_w = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("tag-legacy-seed".into())
        .spawn(move || {
            let result = run_seed(data_dir, paths, &cancel_w);
            let _ = tx.send(result);
        })
        .ok();
    LegacySeedPending { cancel, rx }
}

fn run_seed(
    data_dir: PathBuf,
    paths: Vec<PathBuf>,
    cancel: &AtomicBool,
) -> Result<LegacySeedResult, String> {
    let mut db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db"))
        .map_err(|e| format!("tags.db を開けませんでした: {e}"))?;
    let mut result = LegacySeedResult {
        report: LegacySeedReport {
            candidate_items: paths.len(),
            ..LegacySeedReport::default()
        },
        cache_updates: Vec::new(),
    };

    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let item_key = crate::tags_db::item_key_for_path(&path);
        if db.has_item_state(&item_key) {
            result.report.skipped_decided_items += 1;
            continue;
        }

        let subjects = match crate::xmp_reader::try_read_dc_subject(&path) {
            Ok(subjects) => {
                result.report.read_items += 1;
                subjects
            }
            Err(e) => {
                result.report.read_errors += 1;
                crate::logger::log(format!(
                    "[TAG] legacy seed read failed: {} ({e})",
                    path.display()
                ));
                continue;
            }
        };

        let legacy_tags = miv_legacy_tags(subjects);
        if legacy_tags.is_empty() {
            match db.upsert_item_state(&item_key, crate::tags_db::source::XMP_LEGACY) {
                Ok(()) => result.report.marked_empty_items += 1,
                Err(e) => {
                    result.report.db_errors += 1;
                    crate::logger::log(format!(
                        "[TAG] legacy seed state mark failed: {} ({e})",
                        path.display()
                    ));
                }
            }
            continue;
        }

        let before = db.display_tags_for_item(&item_key);
        let before_keys = tag_key_set(&before);
        let mut union = before.clone();
        union.extend(legacy_tags.iter().cloned());
        match db.set_item_tags(
            &item_key,
            union
                .iter()
                .map(|tag| crate::tags_db::strip_display_hash(tag)),
            crate::tags_db::source::XMP_LEGACY,
        ) {
            Ok(after) => {
                let after_keys = tag_key_set(&after);
                result.report.inserted_tags += after_keys.difference(&before_keys).count();
                result.report.imported_items += 1;
                if after != before {
                    result.cache_updates.push((path, after));
                }
            }
            Err(e) => {
                result.report.db_errors += 1;
                crate::logger::log(format!(
                    "[TAG] legacy seed DB write failed: {} ({e})",
                    path.display()
                ));
            }
        }
    }

    Ok(result)
}

fn miv_legacy_tags(subjects: Vec<String>) -> Vec<String> {
    crate::tags_db::collapse_tags(
        subjects
            .into_iter()
            .filter(|tag| tag.trim_start().starts_with('#')),
        0,
    )
    .into_iter()
    .map(|tag| crate::tags_db::format_display_tag(&tag.tag))
    .collect()
}

fn tag_key_set(tags: &[String]) -> HashSet<String> {
    tags.iter()
        .map(|tag| crate::tags_db::normalize_tag_key(tag))
        .filter(|key| !key.is_empty())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    const SAMPLE_XMP_WITH_TAGS: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description xmlns:dc="http://purl.org/dc/elements/1.1/">
      <dc:subject>
        <rdf:Bag>
          <rdf:li>#Old</rdf:li>
          <rdf:li>external</rdf:li>
        </rdf:Bag>
      </dc:subject>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    #[test]
    fn miv_legacy_tags_filters_hash_tags_only() {
        let tags = miv_legacy_tags(vec![
            "#Cat".to_string(),
            "external".to_string(),
            " #ＤＯＧ ".to_string(),
            "#".to_string(),
        ]);
        assert_eq!(tags, vec!["#Cat".to_string(), "#DOG".to_string()]);
    }

    #[test]
    fn sidecar_target_for_real_file_uses_lowercase_filename() {
        let path = PathBuf::from("C:/Pics/Cat.JPG");
        let target = crate::tag_write_worker::sidecar_target_for_real_file(&path).unwrap();
        assert_eq!(target.folder, PathBuf::from("C:/Pics"));
        assert_eq!(target.rel_key, "cat.jpg");
    }

    #[test]
    fn run_seed_imports_video_xmp_once_and_skips_decided_items() {
        let media_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let video = media_dir.path().join("clip.mp4");
        let sidecar = crate::xmp_writer::sidecar_path_for(&video);
        std::fs::write(&sidecar, SAMPLE_XMP_WITH_TAGS).unwrap();

        let cancel = AtomicBool::new(false);
        let first = run_seed(data_dir.path().to_path_buf(), vec![video.clone()], &cancel).unwrap();
        assert_eq!(first.report.imported_items, 1);
        assert_eq!(first.report.inserted_tags, 1);
        assert_eq!(first.cache_updates.len(), 1);

        let db = crate::tags_db::TagsDb::open_at(&data_dir.path().join("tags.db")).unwrap();
        let key = crate::tags_db::item_key_for_path(&video);
        assert_eq!(db.display_tags_for_item(&key), vec!["#Old".to_string()]);

        std::fs::write(
            &sidecar,
            SAMPLE_XMP_WITH_TAGS.replace("#Old", "#New").as_bytes(),
        )
        .unwrap();
        let second = run_seed(data_dir.path().to_path_buf(), vec![video.clone()], &cancel).unwrap();
        assert_eq!(second.report.skipped_decided_items, 1);
        assert!(second.cache_updates.is_empty());
        assert_eq!(db.display_tags_for_item(&key), vec!["#Old".to_string()]);
    }
}

//! 旧 v1.0 XMP `dc:subject` タグの明示取り込み worker。
//!
//! §7.2 の自動 seed と違い、ユーザーが明示的に選んだ対象は `tag_item_state`
//! の有無に関係なく読み直す。取り込みは `tags.db` へ union し、削除モードでは
//! DB 反映が成功した item だけファイル側の `#` タグを除去する。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LegacyXmpImportMode {
    ImportOnly,
    ImportAndRemove,
}

impl LegacyXmpImportMode {
    pub fn removes_from_file(self) -> bool {
        matches!(self, Self::ImportAndRemove)
    }

    pub fn progress_label(self) -> &'static str {
        match self {
            Self::ImportOnly => "旧XMPタグを取り込み中",
            Self::ImportAndRemove => "旧XMPタグを取り込み、ファイルから削除中",
        }
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LegacyXmpImportReport {
    pub candidate_items: usize,
    pub read_items: usize,
    pub imported_items: usize,
    pub inserted_tags: usize,
    pub marked_empty_items: usize,
    pub cleaned_files: usize,
    pub deleted_video_sidecars: usize,
    pub read_errors: usize,
    pub db_errors: usize,
    pub write_errors: usize,
    /// ユーザー操作 (再実行メニュー) またはアプリ終了で中止された。
    /// 処理済み分は反映済み — トーストでその旨を伝える。
    pub cancelled: bool,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct LegacyXmpImportResult {
    pub report: LegacyXmpImportReport,
    pub cache_updates: Vec<(PathBuf, Vec<String>)>,
    pub errors: Vec<(PathBuf, String)>,
}

pub(crate) struct LegacyXmpImportPending {
    pub mode: LegacyXmpImportMode,
    // ImportAndRemove はユーザーファイルを書き換える破壊的バッチなので、
    // 必ず中止手段を持つ (再実行メニュー / アプリ終了時にキャンセル)。
    cancel: Arc<AtomicBool>,
    pub rx: mpsc::Receiver<Result<LegacyXmpImportResult, String>>,
}

impl LegacyXmpImportPending {
    pub(crate) fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

pub(crate) fn spawn(
    data_dir: PathBuf,
    paths: Vec<PathBuf>,
    mode: LegacyXmpImportMode,
) -> LegacyXmpImportPending {
    let cancel = Arc::new(AtomicBool::new(false));
    let cancel_w = Arc::clone(&cancel);
    let (tx, rx) = mpsc::channel();
    std::thread::Builder::new()
        .name("tag-legacy-xmp".into())
        .spawn(move || {
            let result = run_import(data_dir, paths, mode, &cancel_w);
            let _ = tx.send(result);
        })
        .ok();
    LegacyXmpImportPending { mode, cancel, rx }
}

fn run_import(
    data_dir: PathBuf,
    paths: Vec<PathBuf>,
    mode: LegacyXmpImportMode,
    cancel: &AtomicBool,
) -> Result<LegacyXmpImportResult, String> {
    let mut db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db"))
        .map_err(|e| format!("tags.db を開けませんでした: {e}"))?;
    let mut result = LegacyXmpImportResult {
        report: LegacyXmpImportReport {
            candidate_items: paths.len(),
            ..LegacyXmpImportReport::default()
        },
        cache_updates: Vec::new(),
        errors: Vec::new(),
    };

    for path in paths {
        if cancel.load(Ordering::Relaxed) {
            result.report.cancelled = true;
            break;
        }
        let item_key = crate::tags_db::item_key_for_path(&path);
        let subjects = match crate::xmp_reader::try_read_dc_subject(&path) {
            Ok(subjects) => {
                result.report.read_items += 1;
                subjects
            }
            Err(e) => {
                result.report.read_errors += 1;
                push_error(&mut result, &path, format!("読み取り失敗: {e}"));
                continue;
            }
        };

        let legacy_tags = miv_legacy_tags(subjects);
        let before = db.display_tags_for_item(&item_key);
        let (after, inserted) = match db.union_item_tags(
            &item_key,
            legacy_tags
                .iter()
                .map(|tag| crate::tags_db::strip_display_hash(tag)),
            crate::tags_db::source::XMP_LEGACY,
        ) {
            Ok((after, inserted)) => (after, inserted),
            Err(e) => {
                result.report.db_errors += 1;
                push_error(&mut result, &path, format!("DB更新失敗: {e}"));
                continue;
            }
        };

        if legacy_tags.is_empty() {
            result.report.marked_empty_items += 1;
        } else {
            result.report.imported_items += 1;
            result.report.inserted_tags += inserted;
        }
        if after != before {
            result.cache_updates.push((path.clone(), after));
        }

        if mode.removes_from_file() && !legacy_tags.is_empty() {
            match crate::xmp_writer::apply_tag_op(&path, &crate::xmp_writer::TagOp::ClearMiv) {
                Ok(_) => {
                    result.report.cleaned_files += 1;
                    match crate::xmp_writer::cleanup_empty_video_sidecar_after_clear(&path) {
                        Ok(true) => result.report.deleted_video_sidecars += 1,
                        Ok(false) => {}
                        Err(e) => {
                            result.report.write_errors += 1;
                            push_error(&mut result, &path, format!("sidecar削除失敗: {e}"));
                        }
                    }
                }
                Err(e) => {
                    result.report.write_errors += 1;
                    push_error(&mut result, &path, format!("ファイル更新失敗: {e}"));
                }
            }
        }
    }

    Ok(result)
}

fn push_error(result: &mut LegacyXmpImportResult, path: &std::path::Path, msg: String) {
    crate::logger::log(format!(
        "[TAG] legacy XMP import: {} ({msg})",
        path.display()
    ));
    result.errors.push((path.to_path_buf(), msg));
}

use crate::tags_db::miv_legacy_tags;

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicBool;

    const SAMPLE_ONLY_MIV_TAGS: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="mimageviewer">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
      xmlns:dc="http://purl.org/dc/elements/1.1/"
      xmlns:xmp="http://ns.adobe.com/xap/1.0/">
      <dc:subject><rdf:Bag><rdf:li>#Old</rdf:li></rdf:Bag></dc:subject>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    const SAMPLE_MIXED_TAGS: &str = r#"<?xpacket begin="" id="W5M0MpCehiHzreSzNTczkc9d"?>
<x:xmpmeta xmlns:x="adobe:ns:meta/" x:xmptk="external">
  <rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
    <rdf:Description rdf:about=""
      xmlns:dc="http://purl.org/dc/elements/1.1/"
      xmlns:xmp="http://ns.adobe.com/xap/1.0/">
      <dc:subject>
        <rdf:Bag>
          <rdf:li>#Old</rdf:li>
          <rdf:li>Photographer</rdf:li>
        </rdf:Bag>
      </dc:subject>
    </rdf:Description>
  </rdf:RDF>
</x:xmpmeta>
<?xpacket end="w"?>"#;

    #[test]
    fn import_and_remove_deletes_video_sidecar_when_only_miv_tags_remain() {
        let media_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let video = media_dir.path().join("clip.mp4");
        let sidecar = crate::xmp_writer::sidecar_path_for(&video);
        std::fs::write(&sidecar, SAMPLE_ONLY_MIV_TAGS).unwrap();

        let cancel = AtomicBool::new(false);
        let result = run_import(
            data_dir.path().to_path_buf(),
            vec![video.clone()],
            LegacyXmpImportMode::ImportAndRemove,
            &cancel,
        )
        .unwrap();
        assert_eq!(result.report.imported_items, 1);
        assert_eq!(result.report.inserted_tags, 1);
        assert_eq!(result.report.cleaned_files, 1);
        assert_eq!(result.report.deleted_video_sidecars, 1);
        assert!(!sidecar.exists());

        let db = crate::tags_db::TagsDb::open_at(&data_dir.path().join("tags.db")).unwrap();
        let key = crate::tags_db::item_key_for_path(&video);
        assert_eq!(db.display_tags_for_item(&key), vec!["#Old".to_string()]);
    }

    #[test]
    fn import_and_remove_preserves_video_sidecar_with_external_subject() {
        let media_dir = tempfile::tempdir().unwrap();
        let data_dir = tempfile::tempdir().unwrap();
        let video = media_dir.path().join("clip.mp4");
        let sidecar = crate::xmp_writer::sidecar_path_for(&video);
        std::fs::write(&sidecar, SAMPLE_MIXED_TAGS).unwrap();

        let cancel = AtomicBool::new(false);
        let result = run_import(
            data_dir.path().to_path_buf(),
            vec![video.clone()],
            LegacyXmpImportMode::ImportAndRemove,
            &cancel,
        )
        .unwrap();
        assert_eq!(result.report.imported_items, 1);
        assert_eq!(result.report.cleaned_files, 1);
        assert_eq!(result.report.deleted_video_sidecars, 0);
        assert!(sidecar.exists());
        let subjects = crate::xmp_reader::try_read_dc_subject(&video).unwrap();
        assert_eq!(subjects, vec!["Photographer".to_string()]);
    }
}

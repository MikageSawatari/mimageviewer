//! 旧 v1.0 XMP `dc:subject` タグを `tags.db` へ一度だけ seed する worker。
//!
//! Tantivy STORED tags からの一括移行に乗らなかった、お気に入り外・未索引ファイル向けの
//! 保険経路。UI スレッドではファイルを読まず、`tag_item_state` がある item は読み込み前に
//! skip してタグ削除済み item の復活を防ぐ。

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
    // バックグラウンドモード: CPU と **ディスク I/O の優先度**を両方下げる。
    // 初訪フォルダでは未決定ファイル全件の prefix 読みが走るため、サムネイル生成と
    // 同じディスクを取り合わないようにする (移行はレイテンシ非依存の安全網なので
    // 遅くなって構わない。件数 cap は付けない — correctness > latency)。
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{
            GetCurrentThread, SetThreadPriority, THREAD_MODE_BACKGROUND_BEGIN,
        };
        if SetThreadPriority(GetCurrentThread(), THREAD_MODE_BACKGROUND_BEGIN).is_err() {
            crate::logger::log("tag-legacy-seed: SetThreadPriority(background) failed");
        }
    }

    let mut db = crate::tags_db::TagsDb::open_at(&data_dir.join("tags.db"))
        .map_err(|e| format!("tags.db を開けませんでした: {e}"))?;
    let mut result = LegacySeedResult {
        report: LegacySeedReport {
            candidate_items: paths.len(),
            ..LegacySeedReport::default()
        },
        cache_updates: Vec::new(),
    };

    // 決定済み (tag_item_state あり) の事前フィルタは bulk IN クエリ 1 パスで行う。
    // フォルダ再訪 (全件決定済み) を per-file 点クエリ × N にしないため。
    let item_keys: Vec<String> = paths
        .iter()
        .map(|path| crate::tags_db::item_key_for_path(path))
        .collect();
    let decided = db.keys_with_item_state(&item_keys);

    for (path, item_key) in paths.into_iter().zip(item_keys) {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        if decided.contains(&item_key) {
            result.report.skipped_decided_items += 1;
            continue;
        }

        let subjects = match crate::xmp_reader::try_read_dc_subject_for_legacy_seed(&path) {
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
        // XMP 読み取り (遅い I/O) 中に通常編集が同じ item を確定させた可能性があるため、
        // 反映は tag_item_state を**同一トランザクション内で再チェック**する原子的 union
        // (`seed_legacy_tags_if_undecided`) で行う。read→全置換だと編集を巻き戻す
        // (削除タグの復活) TOCTOU がある。
        match db.seed_legacy_tags_if_undecided(
            &item_key,
            legacy_tags
                .iter()
                .map(|tag| crate::tags_db::strip_display_hash(tag)),
        ) {
            Ok(None) => result.report.skipped_decided_items += 1,
            Ok(Some((after, inserted))) => {
                if legacy_tags.is_empty() {
                    result.report.marked_empty_items += 1;
                } else {
                    result.report.imported_items += 1;
                    result.report.inserted_tags += inserted;
                    if inserted > 0 {
                        result.cache_updates.push((path, after));
                    }
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

use crate::tags_db::miv_legacy_tags;

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

    /// TOCTOU 回帰: seed の事前チェック後〜DB 反映前に通常編集が確定した場合、
    /// seed は何も書かず編集結果を保持する (削除タグの復活・追加タグの巻き戻しを
    /// 起こさない)。`seed_legacy_tags_if_undecided` がトランザクション内で
    /// tag_item_state を再チェックすることを直接検証する。
    #[test]
    fn seed_does_not_clobber_concurrent_edit() {
        let data_dir = tempfile::tempdir().unwrap();
        let mut db = crate::tags_db::TagsDb::open_at(&data_dir.path().join("tags.db")).unwrap();
        let item_key = "c:/pics/racy.jpg";

        // seed worker が has_item_state(=false) を確認した直後の想定で、
        // ユーザー編集が先に確定する (state 行 = edit が立つ)。
        db.set_item_tags(item_key, ["user-tag"], crate::tags_db::source::EDIT)
            .unwrap();

        // 遅れて到着した seed の反映は no-op になる。
        let outcome = db
            .seed_legacy_tags_if_undecided(item_key, ["Old", "Stale"])
            .unwrap();
        assert!(outcome.is_none(), "decided item must be skipped");
        assert_eq!(
            db.display_tags_for_item(item_key),
            vec!["#user-tag".to_string()]
        );

        // 未決定 item への seed は union として入る (既存タグを消さない)。
        let undecided = "c:/pics/fresh.jpg";
        let (after, inserted) = db
            .seed_legacy_tags_if_undecided(undecided, ["Old"])
            .unwrap()
            .expect("undecided item must be seeded");
        assert_eq!(after, vec!["#Old".to_string()]);
        assert_eq!(inserted, 1);
    }
}

//! グリッド選択の一括エクスポート (Ctrl+E)。
//!
//! 合成は製本と同じ [`crate::books::write_composited_page`] を使う。この module が
//! 持つのは「出力先 / ファイル名 / 縮小 / 進捗とキャンセル」だけで、デコード → 合成 →
//! エンコードのパイプラインを二重に持たない。
//!
//! 進捗は Ctrl+E 単ページと同じ [`crate::export_dialog::ExportEvent`] /
//! [`crate::export_dialog::ExportPending`] で報告するので、進捗ダイアログと
//! `poll_export_pending` はそのまま動く。

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

use crate::export_dialog::{ExportEvent, ExportFailure, ExportPending, ExportScale, ExportSuccess};

/// テンプレートに使える置換子。ダイアログのヒントと [`expand_filename_template`] の
/// 唯一の定義元。
pub const TEMPLATE_PLACEHOLDER_HINT: &str = "<filename> / <dirname> / <num> が使えます";

/// 書き出し 1 件ぶん。
pub struct BatchExportItem {
    /// `<filename>` に入る名前 (拡張子なし)。
    pub filename: String,
    /// `<dirname>` に入る名前 (実フォルダ名 / ZIP・PDF のファイル名)。
    pub dirname: String,
    pub source: crate::books::CompositeSource,
    pub edits: crate::books::BakedEditSnapshot,
}

pub struct BatchExportRequest {
    pub output_dir: PathBuf,
    pub template: String,
    pub scale: ExportScale,
    pub items: Vec<BatchExportItem>,
}

/// `<filename>` / `<dirname>` / `<num>` を展開する。未知の `<...>` は展開せずそのまま
/// 残す (テンプレートの打ち間違いを黙って消さない)。`index` は 1 origin。
pub fn expand_filename_template(
    template: &str,
    filename: &str,
    dirname: &str,
    index: usize,
) -> String {
    template
        .replace("<filename>", filename)
        .replace("<dirname>", dirname)
        .replace("<num>", &format!("{index:04}"))
}

/// テンプレート展開 → ファイル名として使える文字へ正規化。
///
/// `basename_from_text` は禁止文字を `_` へ置き換え、空にはならない (空入力は
/// `capture` になる) ので、ここに追加の空文字フォールバックは置かない。
pub fn resolve_item_stem(template: &str, item: &BatchExportItem, index: usize) -> String {
    let expanded = expand_filename_template(template, &item.filename, &item.dirname, index);
    crate::capture::basename_from_text(&expanded)
}

/// まだ使われていない出力パスを決める。既存ファイルも、この実行で既に書いた名前も
/// 避ける (テンプレート次第で複数の元画像が同じ名前へ落ちる)。上書きはしない。
fn unique_target_path(
    output_dir: &Path,
    stem: &str,
    extension: &str,
    claimed: &mut HashSet<PathBuf>,
) -> Result<PathBuf, String> {
    for seq in 0..=9999u32 {
        let candidate = if seq == 0 {
            output_dir.join(format!("{stem}.{extension}"))
        } else {
            output_dir.join(format!("{stem}_{seq}.{extension}"))
        };
        if !claimed.contains(&candidate) && !candidate.exists() {
            claimed.insert(candidate.clone());
            return Ok(candidate);
        }
    }
    Err(format!("同名ファイルが多すぎます: {stem}.{extension}"))
}

pub fn spawn_batch_export_worker(request: BatchExportRequest) -> Result<ExportPending, String> {
    let total = request.items.len();
    if total == 0 {
        return Err("エクスポートする画像がありません".to_string());
    }
    let (tx, rx) = mpsc::channel();
    let cancel = Arc::new(AtomicBool::new(false));
    let worker_cancel = Arc::clone(&cancel);
    std::thread::Builder::new()
        .name("ctrl-e-batch-export".into())
        .spawn(move || run_batch_export(request, worker_cancel, tx))
        .map_err(|e| format!("一括エクスポート worker を開始できません: {e}"))?;
    Ok(ExportPending {
        cancel,
        rx,
        total,
        done: 0,
        last_message: "準備中".to_string(),
        successes: Vec::new(),
        errors: Vec::new(),
        finished: false,
        cancel_requested: false,
    })
}

fn run_batch_export(
    request: BatchExportRequest,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<ExportEvent>,
) {
    if let Err(error) = std::fs::create_dir_all(&request.output_dir) {
        let message = format!(
            "保存先フォルダを作成できません: {}: {error}",
            request.output_dir.display()
        );
        for item in &request.items {
            let _ = tx.send(ExportEvent::Failed(ExportFailure {
                label: item.filename.clone(),
                message: message.clone(),
            }));
        }
        let _ = tx.send(ExportEvent::AllDone);
        return;
    }

    let mut claimed: HashSet<PathBuf> = HashSet::new();
    for (index, item) in request.items.iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            let _ = tx.send(ExportEvent::Cancelled);
            return;
        }
        let label = item.filename.clone();
        let _ = tx.send(ExportEvent::Started {
            label: label.clone(),
        });
        let stem = resolve_item_stem(&request.template, item, index + 1);
        let path = match unique_target_path(
            &request.output_dir,
            &stem,
            item.edits.format.extension(),
            &mut claimed,
        ) {
            Ok(path) => path,
            Err(message) => {
                let _ = tx.send(ExportEvent::Failed(ExportFailure { label, message }));
                continue;
            }
        };
        match crate::books::write_composited_page(&item.source, &item.edits, &path, request.scale) {
            Ok(_) => {
                let _ = tx.send(ExportEvent::Completed(ExportSuccess { label, path }));
            }
            Err(message) => {
                let _ = tx.send(ExportEvent::Failed(ExportFailure { label, message }));
            }
        }
    }
    let _ = tx.send(ExportEvent::AllDone);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn item(filename: &str, dirname: &str) -> BatchExportItem {
        BatchExportItem {
            filename: filename.to_string(),
            dirname: dirname.to_string(),
            source: crate::books::CompositeSource::File {
                path: PathBuf::from("unused"),
            },
            edits: crate::books::BakedEditSnapshot {
                params: crate::adjustment::AdjustParams::default(),
                rotation: crate::rotation_db::Rotation::None,
                conceal: None,
                erase: None,
                local_adjust: None,
                comic: None,
                comic_source_dims: None,
                export_crop: None,
                crop_legacy_writeback: None,
                format: crate::capture::CaptureFormat::Png,
                jpeg_matte: crate::capture::JpegMatte::Black,
            },
        }
    }

    #[test]
    fn template_expands_every_placeholder() {
        let got = expand_filename_template("<dirname>/<filename>_<num>", "shot", "trip", 7);

        assert_eq!(got, "trip/shot_0007");
    }

    #[test]
    fn template_keeps_unknown_placeholders_instead_of_dropping_them() {
        let got = expand_filename_template("<filename>_<unknown>", "shot", "trip", 1);

        assert_eq!(got, "shot_<unknown>");
    }

    #[test]
    fn stem_replaces_path_separators_so_the_output_stays_in_the_chosen_folder() {
        let got = resolve_item_stem("<dirname>/<filename>", &item("shot", "trip"), 1);

        assert_eq!(got, "trip_shot");
    }

    #[test]
    fn unique_target_path_avoids_existing_files_and_names_claimed_in_this_run() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("out.png"), b"x").unwrap();
        let mut claimed = HashSet::new();

        let first = unique_target_path(temp.path(), "out", "png", &mut claimed).unwrap();
        let second = unique_target_path(temp.path(), "out", "png", &mut claimed).unwrap();

        assert_eq!(first, temp.path().join("out_1.png"));
        assert_eq!(second, temp.path().join("out_2.png"));
    }

    #[test]
    fn two_items_that_expand_to_the_same_name_both_get_written() {
        let temp = tempfile::tempdir().unwrap();
        let mut claimed = HashSet::new();

        let a = resolve_item_stem("<dirname>", &item("one", "same"), 1);
        let b = resolve_item_stem("<dirname>", &item("two", "same"), 2);
        let a = unique_target_path(temp.path(), &a, "png", &mut claimed).unwrap();
        let b = unique_target_path(temp.path(), &b, "png", &mut claimed).unwrap();

        assert_ne!(a, b);
    }
}

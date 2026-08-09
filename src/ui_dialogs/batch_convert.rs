//! 複数アーカイブの明示 ZIP 変換 (バッチ)。
//!
//! スペースキー選択 (`checked`) があればその集合、無ければカーソル位置の
//! `ConvertibleArchive` (RAR/CBR/7z/CB7/LZH/LHA) を、同フォルダの同名 `.zip` へ
//! まとめて変換する。対象解決はレーティング / タグと同じ `selection_target_indices`。
//!
//! ## 方針
//! - **同名 `.zip` が既に存在するファイルは変換しない** (上書きのリスクを避ける)。
//!   スキップした件数は完了トーストで通知する。
//! - 進捗は削除と同じく `egui::Modal` で表示する (動画アップスケールのような
//!   キュー管理は持たない)。
//! - 単一ファイルで同名 zip も無い通常ケースは、既存の対話フロー
//!   (`request_explicit_zip_convert`、確認 + パスワード入力対応) をそのまま使う。
//!   バッチ経路はパスワード入力を持たないため、パスワード保護アーカイブは
//!   「変換できませんでした」として集計する。

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;

use crate::app::App;
use crate::archive_converter::{
    ArchiveFormat, ConvertError, ConvertOptions, convert_to_zip_with_password,
};

pub(crate) struct BatchConvertJob {
    pub src: PathBuf,
    pub dst: PathBuf,
    pub format: ArchiveFormat,
}

pub(crate) enum BatchConvertMsg {
    /// index 番目 (0-based) の変換を開始した。
    Start { name: String },
    /// index 番目の結果。Ok(出力 zip) / Err(理由ラベル)。
    Done {
        src: PathBuf,
        result: Result<PathBuf, String>,
    },
    /// 全件終了。`canceled` はキャンセルで打ち切ったか。
    Finished { canceled: bool },
}

pub(crate) struct BatchConvertPending {
    pub rx: Receiver<BatchConvertMsg>,
    pub cancel: Arc<AtomicBool>,
    pub total: usize,
    /// 現在処理中のファイル名 (`Start` 受信で更新)。
    pub current_name: String,
    pub converted: Vec<PathBuf>,
    pub failed: Vec<(PathBuf, String)>,
    /// 同名 zip 既存でスキップした元アーカイブ (開始時点で確定)。
    pub skipped_existing: Vec<PathBuf>,
    /// 出力名衝突でスキップした元アーカイブ (開始時点で確定)。
    pub skipped_collision: Vec<PathBuf>,
}

impl BatchConvertPending {
    /// 完了 (成功 + 失敗) 件数。進捗バー用。
    pub fn finished_count(&self) -> usize {
        self.converted.len() + self.failed.len()
    }
}

/// 完了トーストの本文を組み立てる (件数ゼロの区分は出さない)。全区分ゼロなら `None`。
fn batch_convert_summary(
    canceled: bool,
    converted: usize,
    skipped_existing: usize,
    skipped_collision: usize,
    failed: usize,
) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    if canceled {
        parts.push("キャンセルしました".to_string());
    }
    if converted > 0 {
        parts.push(format!("{converted} 件を ZIP に変換しました"));
    }
    if skipped_existing > 0 {
        parts.push(format!(
            "{skipped_existing} 件は同名の ZIP があるため変換しませんでした"
        ));
    }
    if skipped_collision > 0 {
        parts.push(format!(
            "{skipped_collision} 件は出力名が重複するため変換しませんでした"
        ));
    }
    if failed > 0 {
        parts.push(format!("{failed} 件は変換できませんでした"));
    }
    if parts.is_empty() {
        None
    } else {
        Some(parts.join(" / "))
    }
}

/// `partition_targets` の結果。スキップは理由別に分けて正確に通知する。
struct Partition {
    jobs: Vec<BatchConvertJob>,
    /// 同名 `.zip` が既にディスク上にあってスキップした元アーカイブ。
    skipped_existing: Vec<PathBuf>,
    /// 同じ出力名を選択内の先行アーカイブが確保済みでスキップした元アーカイブ。
    skipped_collision: Vec<PathBuf>,
}

/// 対象を「変換する job」と「スキップする元アーカイブ (理由別)」に分ける純ロジック。
///
/// スキップ条件:
///  1. 同名 `.zip` が既にディスク上にある (`dst_exists` が true) — ユーザーの ZIP を
///     上書きしない → `skipped_existing`。
///  2. 同じ出力名 (`<basename>.zip`) を、この選択内の先行アーカイブが既に確保している
///     (例: `book.rar` と `book.7z` がどちらも `book.zip` を狙う) — 先頭だけ変換し、
///     残りは上書き衝突になるのでスキップ → `skipped_collision`。比較は Windows 前提で
///     大小無視。
///
/// `dst_exists` を注入するのはユニットテスト容易化のため (実処理は `|dst| dst.exists()`)。
fn partition_targets(
    targets: Vec<(PathBuf, ArchiveFormat)>,
    dst_exists: impl Fn(&std::path::Path) -> bool,
) -> Partition {
    let mut jobs: Vec<BatchConvertJob> = Vec::new();
    let mut skipped_existing: Vec<PathBuf> = Vec::new();
    let mut skipped_collision: Vec<PathBuf> = Vec::new();
    let mut claimed_dst: std::collections::HashSet<String> = std::collections::HashSet::new();
    for (path, format) in targets {
        let dst = path.with_extension("zip");
        let dst_key = dst.to_string_lossy().to_lowercase();
        if dst_exists(&dst) {
            skipped_existing.push(path);
        } else if !claimed_dst.insert(dst_key) {
            skipped_collision.push(path);
        } else {
            jobs.push(BatchConvertJob {
                src: path,
                dst,
                format,
            });
        }
    }
    Partition {
        jobs,
        skipped_existing,
        skipped_collision,
    }
}

fn convert_error_label(e: &ConvertError) -> String {
    match e {
        ConvertError::PasswordRequired | ConvertError::BadPassword => "パスワード保護".to_string(),
        ConvertError::PasswordUnsupported => "パスワード付き形式に非対応".to_string(),
        ConvertError::NoImages => "画像なし".to_string(),
        ConvertError::TooLarge => "サイズ超過".to_string(),
        ConvertError::Io(err) => format!("I/O エラー: {err}"),
        ConvertError::Archive(s) => s.clone(),
        // Cancelled はワーカー側で握り (Done を送らない) ので通常ここには来ない。
        ConvertError::Cancelled => "キャンセル".to_string(),
    }
}

fn spawn(jobs: Vec<BatchConvertJob>, cancel: Arc<AtomicBool>) -> Receiver<BatchConvertMsg> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        for job in &jobs {
            if cancel.load(Ordering::Relaxed) {
                break;
            }
            let name = job
                .src
                .file_name()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| job.src.to_string_lossy().into_owned());
            let _ = tx.send(BatchConvertMsg::Start { name });
            let result = convert_to_zip_with_password(
                &job.src,
                &job.dst,
                job.format,
                None,
                &cancel,
                None,
                ConvertOptions {
                    no_clobber: true,
                    verify: true,
                },
            );
            match result {
                Ok(_) => {
                    let _ = tx.send(BatchConvertMsg::Done {
                        src: job.src.clone(),
                        result: Ok(job.dst.clone()),
                    });
                }
                // キャンセル中断は失敗に数えない (次ループ頭の break で抜ける)。
                Err(ConvertError::Cancelled) => break,
                Err(e) => {
                    let _ = tx.send(BatchConvertMsg::Done {
                        src: job.src.clone(),
                        result: Err(convert_error_label(&e)),
                    });
                }
            }
            if cancel.load(Ordering::Relaxed) {
                break;
            }
        }
        let _ = tx.send(BatchConvertMsg::Finished {
            canceled: cancel.load(Ordering::Relaxed),
        });
    });
    rx
}

impl App {
    /// 「変換 > ZIP ファイルに変換」の実処理。選択 (またはカーソル) の
    /// `ConvertibleArchive` を同名 zip へ変換する。
    pub(crate) fn start_batch_convert_to_zip(&mut self) {
        if self.batch_convert.is_some() || self.archive_convert.is_some() {
            return;
        }

        // 対象解決 (スペース選択があればその集合、無ければカーソル)。同一 path は 1 回だけ。
        let mut seen = std::collections::HashSet::new();
        let mut targets: Vec<(PathBuf, ArchiveFormat)> = Vec::new();
        for idx in self.selection_target_indices(crate::app::ActionSurface::MainWindow) {
            if let Some(crate::grid_item::GridItem::ConvertibleArchive { path, format }) =
                self.items.get(idx)
            {
                if seen.insert(path.clone()) {
                    targets.push((path.clone(), *format));
                }
            }
        }
        if targets.is_empty() {
            return;
        }

        let Partition {
            jobs,
            skipped_existing,
            skipped_collision,
        } = partition_targets(targets, |dst| dst.exists());

        if jobs.is_empty() {
            // 全件スキップ。この分岐では collision は必ず空 (name を確保する job が
            // 無いため) だが、念のため両方を合算して通知する。
            if let Some(msg) =
                batch_convert_summary(false, 0, skipped_existing.len(), skipped_collision.len(), 0)
            {
                self.show_feedback_toast(msg);
            }
            return;
        }

        // 単一ファイルで同名 zip も出力名衝突も無い通常ケースは、既存の対話フロー
        // (確認 + パスワード入力対応) に委ねる。それ以外はバッチモーダルで一括処理。
        if jobs.len() == 1 && skipped_existing.is_empty() && skipped_collision.is_empty() {
            let job = jobs.into_iter().next().expect("len == 1");
            let _ = self.request_explicit_zip_convert(job.src, job.format);
            return;
        }

        let total = jobs.len();
        let cancel = Arc::new(AtomicBool::new(false));
        let rx = spawn(jobs, Arc::clone(&cancel));
        self.batch_convert = Some(BatchConvertPending {
            rx,
            cancel,
            total,
            current_name: String::new(),
            converted: Vec::new(),
            failed: Vec::new(),
            skipped_existing,
            skipped_collision,
        });
    }

    /// 毎フレーム `batch_convert` の進捗メッセージを受信する。完了時に一覧を
    /// 再読み込み (新しい zip の反映 + 同名 zip 優先の dedup) し、結果をトーストで通知する。
    pub(crate) fn poll_batch_convert(&mut self) {
        let Some(pending) = self.batch_convert.as_mut() else {
            return;
        };
        let mut finished = false;
        let mut canceled = false;
        loop {
            match pending.rx.try_recv() {
                Ok(BatchConvertMsg::Start { name }) => {
                    pending.current_name = name;
                }
                Ok(BatchConvertMsg::Done { src, result }) => match result {
                    Ok(dst) => pending.converted.push(dst),
                    Err(label) => pending.failed.push((src, label)),
                },
                Ok(BatchConvertMsg::Finished { canceled: c }) => {
                    finished = true;
                    canceled = c;
                    break;
                }
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    // `Finished` を受ける前にチャネルが切れた = worker が異常終了
                    // (panic 等、通常起きない稀な経路)。未処理分を「失敗」として計上し、
                    // トーストで必ずユーザーに知らせる (黙って成功扱いにしない)。
                    let accounted = pending.converted.len() + pending.failed.len();
                    if accounted < pending.total {
                        crate::logger::log(format!(
                            "[batch-convert] worker disconnected early: {accounted}/{} accounted",
                            pending.total
                        ));
                        for _ in accounted..pending.total {
                            pending
                                .failed
                                .push((PathBuf::new(), "worker 異常終了".to_string()));
                        }
                    }
                    finished = true;
                    break;
                }
            }
        }

        if !finished {
            return;
        }

        let pending = self.batch_convert.take().expect("guarded above");
        let converted = pending.converted.len();
        let failed = pending.failed.len();
        let skipped_existing = pending.skipped_existing.len();
        let skipped_collision = pending.skipped_collision.len();

        crate::logger::log(format!(
            "[batch-convert] done: canceled={canceled} converted={converted} \
             failed={failed} skipped_existing={skipped_existing} \
             skipped_collision={skipped_collision}"
        ));
        for (path, reason) in &pending.failed {
            crate::logger::log(format!(
                "[batch-convert] failed: {} ({reason})",
                path.display()
            ));
        }

        if let Some(msg) = batch_convert_summary(
            canceled,
            converted,
            skipped_existing,
            skipped_collision,
            failed,
        ) {
            self.show_feedback_toast(msg);
        }

        // スペース選択を解除し、一覧を再構築して新しい zip / 同名 zip 優先の dedup を
        // 反映する。`apply_sort_change_reload` は現在のビュー種別 (通常フォルダ /
        // サブフォルダ展開 / 検索 / タグ / レーティング) を保ったまま並べ直すので、
        // 検索ビュー等から変換しても現フォルダへ強制復帰しない。閲覧履歴ビューは
        // `apply_sort_change_reload` が扱わない (合成パスへ load_folder してしまう) ので
        // 明示的に開き直す。
        self.checked.clear();
        if converted > 0 {
            if self.items_are_reading_history_view {
                self.enter_reading_history();
            } else {
                self.apply_sort_change_reload();
            }
        }
    }

    /// バッチ変換中のモーダル進捗ダイアログ (削除の進捗ダイアログと同じ体裁)。
    pub(crate) fn show_batch_convert_progress_dialog(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.batch_convert.as_ref() else {
            return;
        };
        // worker 動作中は毎フレーム再描画して進捗が止まって見えないようにする。
        ctx.request_repaint();

        let total = pending.total;
        let finished = pending.finished_count();
        let current = pending.current_name.clone();
        let canceling = pending.cancel.load(Ordering::Relaxed);

        let mut cancel_requested = false;
        egui::Modal::new(egui::Id::new("batch_convert_progress_modal")).show(ctx, |ui| {
            ui.set_min_width(320.0);
            ui.heading("ZIP に変換中");
            ui.add_space(4.0);
            ui.label(format!("{finished} / {total}"));
            if !current.is_empty() {
                ui.label(
                    egui::RichText::new(current)
                        .size(12.0)
                        .color(ui.visuals().weak_text_color()),
                );
            }
            let ratio = if total > 0 {
                finished as f32 / total as f32
            } else {
                0.0
            };
            ui.add(egui::ProgressBar::new(ratio).show_percentage());
            ui.add_space(6.0);
            if canceling {
                ui.label("キャンセル中…");
            } else if ui.button("キャンセル").clicked() {
                cancel_requested = true;
            }
        });
        if cancel_requested {
            if let Some(p) = self.batch_convert.as_ref() {
                p.cancel.store(true, Ordering::Relaxed);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summary_reports_each_nonzero_bucket() {
        // 変換のみ。
        assert_eq!(
            batch_convert_summary(false, 3, 0, 0, 0).as_deref(),
            Some("3 件を ZIP に変換しました")
        );
        // スキップ (同名 zip 既存) は「変換しませんでした」文言。
        assert_eq!(
            batch_convert_summary(false, 0, 2, 0, 0).as_deref(),
            Some("2 件は同名の ZIP があるため変換しませんでした")
        );
        // 出力名衝突は別文言。
        assert_eq!(
            batch_convert_summary(false, 0, 0, 2, 0).as_deref(),
            Some("2 件は出力名が重複するため変換しませんでした")
        );
        // 失敗 (パスワード保護など) は「変換できませんでした」文言。
        assert_eq!(
            batch_convert_summary(false, 0, 0, 0, 1).as_deref(),
            Some("1 件は変換できませんでした")
        );
    }

    #[test]
    fn summary_joins_mixed_buckets_in_order() {
        assert_eq!(
            batch_convert_summary(false, 3, 2, 1, 1).as_deref(),
            Some(
                "3 件を ZIP に変換しました / 2 件は同名の ZIP があるため変換しませんでした / \
                 1 件は出力名が重複するため変換しませんでした / 1 件は変換できませんでした"
            )
        );
    }

    #[test]
    fn summary_prefixes_cancel_notice() {
        let msg = batch_convert_summary(true, 1, 0, 0, 0).unwrap();
        assert!(msg.starts_with("キャンセルしました"));
        assert!(msg.contains("1 件を ZIP に変換しました"));
    }

    #[test]
    fn summary_is_none_when_nothing_happened() {
        assert_eq!(batch_convert_summary(false, 0, 0, 0, 0), None);
    }

    #[test]
    fn partition_skips_existing_zip_and_output_name_collisions() {
        let targets = vec![
            (PathBuf::from(r"C:\a\book.rar"), ArchiveFormat::Rar),
            // book.7z も book.zip を狙う → 先の book.rar が確保済みなので衝突スキップ。
            (PathBuf::from(r"C:\a\book.7z"), ArchiveFormat::SevenZ),
            // vol2.zip はディスク上に既存 → 既存スキップ。
            (PathBuf::from(r"C:\a\vol2.rar"), ArchiveFormat::Rar),
            // vol3 は衝突も既存も無い → 変換対象。
            (PathBuf::from(r"C:\a\vol3.rar"), ArchiveFormat::Rar),
        ];
        let existing: std::collections::HashSet<String> =
            [r"c:\a\vol2.zip".to_string()].into_iter().collect();
        let part = partition_targets(targets, |dst| {
            existing.contains(&dst.to_string_lossy().to_lowercase())
        });

        assert_eq!(part.jobs.len(), 2, "book.rar と vol3.rar のみ変換対象");
        assert_eq!(part.jobs[0].src, PathBuf::from(r"C:\a\book.rar"));
        assert_eq!(part.jobs[0].dst, PathBuf::from(r"C:\a\book.zip"));
        assert_eq!(part.jobs[1].src, PathBuf::from(r"C:\a\vol3.rar"));
        assert_eq!(
            part.skipped_existing,
            vec![PathBuf::from(r"C:\a\vol2.rar")],
            "vol2 は既存 zip でスキップ"
        );
        assert_eq!(
            part.skipped_collision,
            vec![PathBuf::from(r"C:\a\book.7z")],
            "book.7z は出力名衝突でスキップ"
        );
    }

    #[test]
    fn password_protected_maps_to_dedicated_label() {
        assert_eq!(
            convert_error_label(&ConvertError::PasswordRequired),
            "パスワード保護"
        );
        assert_eq!(
            convert_error_label(&ConvertError::BadPassword),
            "パスワード保護"
        );
        assert_eq!(
            convert_error_label(&ConvertError::PasswordUnsupported),
            "パスワード付き形式に非対応"
        );
    }
}

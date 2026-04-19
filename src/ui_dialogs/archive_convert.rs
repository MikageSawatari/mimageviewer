//! 7z / LZH → ZIP 変換の確認・進捗ダイアログ (v0.7.0 Task 15)。
//!
//! フロー:
//!   1. グリッドで 7z / LZH をクリック → `App::request_archive_convert` が
//!      `ArchiveConvertState::Scanning` に遷移し、バックグラウンドで画像エントリを数える。
//!   2. スキャン完了 → `Confirm` フェーズに遷移し、画像数・サイズ見積もりを表示。
//!   3. [ 変換して開く ] → `Converting` フェーズに遷移、変換ワーカーを spawn。
//!   4. 完了 → キャッシュ DB に記録し、`pending_post_convert_nav` にキャッシュ ZIP パスを
//!      セット → 次フレームで通常の ZIP として開く。
//!
//! キャンセルは `Arc<AtomicBool>` を立ててワーカーにシグナルする。ワーカーは
//! 各エントリ境界で検査する。

#![allow(unused_imports)]

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc};
use std::thread;

use eframe::egui;

use crate::app::App;
use crate::archive_cache::ArchiveCacheDb;
use crate::archive_converter::{
    convert_to_zip, scan_summary, ArchiveFormat, ArchiveImageSummary, ConvertError,
    ConvertProgress,
};

// ──────────────────────────────────────────────────────────────────────
// ステート型
// ──────────────────────────────────────────────────────────────────────

/// スキャン完了 / 変換完了通知用メッセージ。
pub(crate) enum ArchiveConvertMsg {
    ScanDone(Result<ArchiveImageSummary, ConvertError>),
    /// 変換完了。Ok なら (summary, cached_zip_path, cached_zip_size)
    ConvertDone(Result<(ArchiveImageSummary, PathBuf, i64), ConvertError>),
}

/// 進捗の共有ハンドル。変換ワーカーが書き、UI スレッドが読む。
pub(crate) struct ArchiveConvertProgressShared {
    pub files_done: AtomicU64,
    pub files_total: AtomicU64,
    pub bytes_written: AtomicU64,
}

impl ArchiveConvertProgressShared {
    pub fn new() -> Self {
        Self {
            files_done: AtomicU64::new(0),
            files_total: AtomicU64::new(0),
            bytes_written: AtomicU64::new(0),
        }
    }
}

/// 変換ダイアログのフェーズ。
pub(crate) enum ArchiveConvertPhase {
    /// 事前スキャン中 (画像数カウント)
    Scanning,
    /// スキャン完了、ユーザーの確認待ち
    Confirm { summary: ArchiveImageSummary },
    /// 変換実行中
    Converting {
        progress: Arc<ArchiveConvertProgressShared>,
        cancel: Arc<AtomicBool>,
    },
    /// エラー (ユーザーが閉じるまで表示)
    Error { message: String },
}

pub(crate) struct ArchiveConvertState {
    pub src_path: PathBuf,
    pub format: ArchiveFormat,
    pub phase: ArchiveConvertPhase,
    pub rx: mpsc::Receiver<ArchiveConvertMsg>,
    /// 変換完了後にメイン UI がナビゲーションに使うキャッシュ ZIP パス。
    /// `update()` が毎フレーム見に行き、Some なら `load_folder` を呼んでクリアする。
    pub pending_nav: Option<PathBuf>,
}

// ──────────────────────────────────────────────────────────────────────
// App 側 API
// ──────────────────────────────────────────────────────────────────────

impl App {
    /// 有効なキャッシュがあれば ZIP パスを返す。無効 / 未変換なら None。
    pub(crate) fn try_archive_cache_lookup(&self, src: &std::path::Path) -> Option<PathBuf> {
        let db = self.archive_cache_db.as_ref()?;
        let meta = std::fs::metadata(src).ok()?;
        let mtime = crate::ui_helpers::mtime_secs(&meta);
        let size = meta.len() as i64;
        db.lookup(src, mtime, size)
    }

    /// 変換済みアーカイブを開く。`src` は元 (7z/LZH)、`cached_zip` は
    /// 変換済み ZIP のパス。キャッシュ ZIP を load_folder し、その後
    /// `archive_source_override` / `address` を元パスに書き戻す。
    ///
    /// Enter / ダブルクリックのキャッシュヒット経路で使う。変換直後の
    /// pending_nav 経路は `show_archive_convert_dialog` 内で直接処理する
    /// (そちらは `archive_convert` のライフサイクルと絡むため)。
    pub(crate) fn open_archive_via_cache(&mut self, src: PathBuf, cached_zip: PathBuf) {
        self.load_folder(cached_zip);
        self.address = src.to_string_lossy().to_string();
        self.archive_source_override = Some(src);
    }

    /// 変換ダイアログを開始する (スキャン fase から)。
    /// 既に別のダイアログが動作中なら無視 (二重起動防止)。
    pub(crate) fn request_archive_convert(
        &mut self,
        src: PathBuf,
        format: ArchiveFormat,
    ) {
        if self.archive_convert.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        let src_for_scan = src.clone();
        thread::spawn(move || {
            let result = scan_summary(&src_for_scan, format);
            let _ = tx.send(ArchiveConvertMsg::ScanDone(result));
        });
        self.archive_convert = Some(ArchiveConvertState {
            src_path: src,
            format,
            phase: ArchiveConvertPhase::Scanning,
            rx,
            pending_nav: None,
        });
    }

    /// 毎フレーム呼ばれるダイアログ描画・メッセージ処理のエントリポイント。
    pub(crate) fn show_archive_convert_dialog(&mut self, ctx: &egui::Context) {
        // 先にメッセージ処理 (ステート遷移)
        self.poll_archive_convert_messages();

        // 変換完了後のナビゲーション処理 (別フィールドに移動して state を Drop)
        if let Some(nav) = self
            .archive_convert
            .as_mut()
            .and_then(|s| s.pending_nav.take())
        {
            // 元 (7z/LZH) のパスを退避してから load_folder (キャッシュ ZIP) を実行、
            // その後 override に元パスを書き戻すことで、UI 表示は元ファイルの場所のままに保つ。
            let src = self.archive_convert.as_ref().map(|s| s.src_path.clone());
            self.archive_convert = None;
            self.load_folder(nav);
            if let Some(src) = src {
                self.address = src.to_string_lossy().to_string();
                self.archive_source_override = Some(src);
            }
            return;
        }

        let Some(state) = self.archive_convert.as_ref() else {
            return;
        };
        let src_name = state
            .src_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("")
            .to_string();
        let fmt_label = state.format.label();
        let dialog_pos = ctx.content_rect().min + egui::vec2(60.0, 40.0);
        let mut should_close = false;
        let mut start_convert = false;
        let mut cancel_convert = false;
        let escape_pressed = self.dialog_escape_pressed(ctx);

        let title = match &state.phase {
            ArchiveConvertPhase::Scanning => format!("{fmt_label} を読み込み中..."),
            ArchiveConvertPhase::Confirm { .. } => {
                format!("{fmt_label} を ZIP に変換")
            }
            ArchiveConvertPhase::Converting { .. } => {
                format!("{fmt_label} を ZIP に変換中")
            }
            ArchiveConvertPhase::Error { .. } => "変換エラー".to_string(),
        };

        let mut open = true;
        egui::Window::new(title)
            .id(egui::Id::new("archive_convert_dialog"))
            .open(&mut open)
            .resizable(false)
            .collapsible(false)
            .default_pos(dialog_pos)
            .show(ctx, |ui| {
                ui.set_min_width(420.0);

                match &state.phase {
                    ArchiveConvertPhase::Scanning => {
                        ui.label(format!("入力: {src_name}"));
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            ui.spinner();
                            ui.label("画像エントリを列挙しています…");
                        });
                        ui.add_space(6.0);
                        if ui.button("キャンセル").clicked() {
                            should_close = true;
                        }
                        ctx.request_repaint_after(std::time::Duration::from_millis(100));
                    }
                    ArchiveConvertPhase::Confirm { summary } => {
                        ui.label(format!(
                            "{fmt_label} を ZIP に変換して閲覧できるようにします。"
                        ));
                        ui.label(
                            "元ファイルはそのまま残り、変換したファイルが\
                             キャッシュとして作成されます。",
                        );
                        ui.label(
                            "キャッシュ管理メニューから削除することができます。",
                        );
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(format!("ファイル: {src_name}"))
                                .size(12.0)
                                .color(egui::Color32::from_gray(160)),
                        );
                        ui.label(
                            egui::RichText::new(format!(
                                "画像ファイル数: {} / 変換後 ZIP の目安: 約 {}",
                                summary.image_count,
                                crate::ui_helpers::format_bytes(
                                    summary.total_uncompressed_bytes
                                )
                            ))
                            .size(12.0)
                            .color(egui::Color32::from_gray(160)),
                        );
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    summary.image_count > 0,
                                    egui::Button::new("変換して開く"),
                                )
                                .clicked()
                            {
                                start_convert = true;
                            }
                            if ui.button("キャンセル").clicked() {
                                should_close = true;
                            }
                        });
                        if summary.image_count == 0 {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(
                                    "このアーカイブには画像ファイルが含まれていません。",
                                )
                                .color(egui::Color32::from_rgb(180, 60, 60)),
                            );
                        }
                    }
                    ArchiveConvertPhase::Converting { progress, .. } => {
                        let done = progress.files_done.load(Ordering::Relaxed);
                        let total = progress.files_total.load(Ordering::Relaxed).max(1);
                        let bytes = progress.bytes_written.load(Ordering::Relaxed);
                        let frac = (done as f32 / total as f32).clamp(0.0, 1.0);
                        ui.label(format!("入力: {src_name}"));
                        ui.add_space(6.0);
                        ui.add(egui::ProgressBar::new(frac).show_percentage());
                        ui.add_space(4.0);
                        ui.label(format!(
                            "{} / {} ファイル ({})",
                            done,
                            total,
                            crate::ui_helpers::format_bytes(bytes)
                        ));
                        ui.add_space(6.0);
                        if ui.button("キャンセル").clicked() {
                            cancel_convert = true;
                        }
                        ctx.request_repaint_after(std::time::Duration::from_millis(80));
                    }
                    ArchiveConvertPhase::Error { message } => {
                        ui.label(format!("入力: {src_name}"));
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(message.as_str())
                                .color(egui::Color32::from_rgb(180, 60, 60)),
                        );
                        ui.add_space(6.0);
                        if ui.button("閉じる").clicked() {
                            should_close = true;
                        }
                    }
                }
            });

        if !open || escape_pressed {
            should_close = true;
        }

        if cancel_convert {
            if let Some(state) = self.archive_convert.as_ref() {
                if let ArchiveConvertPhase::Converting { cancel, .. } = &state.phase {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
        }
        if start_convert {
            self.start_archive_convert();
        }
        if should_close {
            // 変換中ならキャンセル信号も立てておく (ワーカーは後で気付いて停止)
            if let Some(state) = self.archive_convert.as_ref() {
                if let ArchiveConvertPhase::Converting { cancel, .. } = &state.phase {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            self.archive_convert = None;
        }
    }

    /// バックグラウンドメッセージを取り込んでフェーズ遷移させる。
    fn poll_archive_convert_messages(&mut self) {
        let Some(state) = self.archive_convert.as_mut() else {
            return;
        };
        while let Ok(msg) = state.rx.try_recv() {
            match msg {
                ArchiveConvertMsg::ScanDone(Ok(summary)) => {
                    if summary.image_count == 0 {
                        state.phase = ArchiveConvertPhase::Error {
                            message: "このアーカイブには画像ファイルが含まれていません。"
                                .to_string(),
                        };
                    } else {
                        state.phase = ArchiveConvertPhase::Confirm { summary };
                    }
                }
                ArchiveConvertMsg::ScanDone(Err(e)) => {
                    state.phase = ArchiveConvertPhase::Error {
                        message: format!("スキャン失敗: {e}"),
                    };
                }
                ArchiveConvertMsg::ConvertDone(Ok((
                    summary,
                    cached_zip,
                    cached_size,
                ))) => {
                    // キャッシュ DB に記録
                    if let Some(db) = self.archive_cache_db.as_ref() {
                        if let Ok(meta) = std::fs::metadata(&state.src_path) {
                            let src_mtime = crate::ui_helpers::mtime_secs(&meta);
                            let src_size = meta.len() as i64;
                            let _ = db.record(
                                &state.src_path,
                                src_mtime,
                                src_size,
                                state.format,
                                &cached_zip,
                                cached_size,
                                summary.image_count,
                            );
                        }
                    }
                    state.pending_nav = Some(cached_zip);
                }
                ArchiveConvertMsg::ConvertDone(Err(ConvertError::Cancelled)) => {
                    // ユーザーキャンセルならダイアログを即閉じる
                    self.archive_convert = None;
                    return;
                }
                ArchiveConvertMsg::ConvertDone(Err(e)) => {
                    state.phase = ArchiveConvertPhase::Error {
                        message: format!("変換失敗: {e}"),
                    };
                }
            }
        }
    }

    /// Confirm 段階で「変換して開く」が押されたときの遷移。
    fn start_archive_convert(&mut self) {
        let Some(state) = self.archive_convert.as_mut() else {
            return;
        };
        // キャッシュ DB が初期化できていないと書き込み先を確定できない
        let Some(db) = self.archive_cache_db.clone() else {
            state.phase = ArchiveConvertPhase::Error {
                message: "キャッシュ DB の初期化に失敗しています。".to_string(),
            };
            return;
        };
        let dst = match db.reserve_cache_zip_path(&state.src_path) {
            Ok(p) => p,
            Err(e) => {
                state.phase = ArchiveConvertPhase::Error {
                    message: format!("出力先の作成に失敗: {e}"),
                };
                return;
            }
        };
        let cancel = Arc::new(AtomicBool::new(false));
        let progress = Arc::new(ArchiveConvertProgressShared::new());
        let (tx, rx) = mpsc::channel();
        let src = state.src_path.clone();
        let format = state.format;
        let cancel_worker = cancel.clone();
        let progress_worker = progress.clone();
        thread::spawn(move || {
            let cb = |p: ConvertProgress| {
                progress_worker
                    .files_done
                    .store(p.files_done as u64, Ordering::Relaxed);
                progress_worker
                    .files_total
                    .store(p.files_total as u64, Ordering::Relaxed);
                progress_worker
                    .bytes_written
                    .store(p.bytes_written, Ordering::Relaxed);
            };
            let result = convert_to_zip(&src, &dst, format, &cancel_worker, Some(&cb));
            let msg = match result {
                Ok(summary) => {
                    let cached_size =
                        std::fs::metadata(&dst).map(|m| m.len() as i64).unwrap_or(0);
                    ArchiveConvertMsg::ConvertDone(Ok((summary, dst, cached_size)))
                }
                Err(e) => ArchiveConvertMsg::ConvertDone(Err(e)),
            };
            let _ = tx.send(msg);
        });
        state.phase = ArchiveConvertPhase::Converting { progress, cancel };
        state.rx = rx;
    }
}

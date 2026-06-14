//! RAR / 7z / LZH → ZIP 変換の確認・進捗ダイアログ。
//!
//! フロー:
//!   1. グリッドで RAR / 7z / LZH をクリック → `App::request_archive_convert` が
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
use std::sync::{Arc, mpsc};
use std::thread;

use eframe::egui;

use crate::app::App;
use crate::archive_cache::ArchiveCacheDb;
use crate::archive_converter::{
    ArchiveFormat, ArchiveImageSummary, ConvertError, ConvertProgress,
    convert_to_zip_with_password, scan_summary_with_password,
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
    /// RAR パスワード入力待ち
    PasswordRequired {
        message: Option<String>,
        resume: ArchivePasswordResume,
    },
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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ArchivePasswordResume {
    Scan,
    Convert,
}

pub(crate) struct ArchiveConvertState {
    pub src_path: PathBuf,
    pub format: ArchiveFormat,
    pub password: Option<String>,
    pub password_input: String,
    pub phase: ArchiveConvertPhase,
    pub rx: mpsc::Receiver<ArchiveConvertMsg>,
    /// 変換完了後にメイン UI がナビゲーションに使うキャッシュ ZIP パス。
    /// `update()` が毎フレーム見に行き、Some なら `load_folder` を呼んでクリアする。
    pub pending_nav: Option<PathBuf>,
    /// 履歴の戻る/進むから未変換アーカイブに入ろうとしてダイアログが出た場合、
    /// キャンセル時に戻る/進むスタックをクリック前へ戻すためのスナップショット。
    pub nav_history_rollback: Option<crate::app::FolderNavHistorySnapshot>,
    /// この変換完了後に 1 ページ目を自動フルスクリーン表示するか。明示的なオープン
    /// (グリッド Enter / ダブルクリック / ゲームパッド × 設定 ON) のときだけ true。
    /// キャンセル時は state ごと drop されるので stale フラグが残らない。
    pub auto_fullscreen: bool,
    /// true の場合、Scanning のウィンドウを出さず、Confirm を自動通過する。
    /// 変換中の進捗、パスワード入力、エラーは表示する。
    pub suppress_confirm: bool,
    /// Confirm 画面の「次回から表示しない」。変換開始時に設定へ反映する。
    pub suppress_confirm_next_time: bool,
}

fn spawn_archive_scan(
    src: PathBuf,
    format: ArchiveFormat,
    password: Option<String>,
) -> mpsc::Receiver<ArchiveConvertMsg> {
    let (tx, rx) = mpsc::channel();
    thread::spawn(move || {
        let result = scan_summary_with_password(&src, format, password.as_deref());
        let _ = tx.send(ArchiveConvertMsg::ScanDone(result));
    });
    rx
}

fn prepare_archive_password_retry(
    state: &mut ArchiveConvertState,
) -> Option<(ArchivePasswordResume, String)> {
    if state.format != ArchiveFormat::Rar {
        return None;
    }
    let password = state.password_input.trim().to_string();
    if password.is_empty() {
        return None;
    }
    let resume = match &state.phase {
        ArchiveConvertPhase::PasswordRequired { resume, .. } => *resume,
        _ => ArchivePasswordResume::Scan,
    };
    state.password = Some(password.clone());
    state.password_input.clear();
    Some((resume, password))
}

fn archive_convert_window_suppressed(phase: &ArchiveConvertPhase, suppress_confirm: bool) -> bool {
    suppress_confirm && matches!(phase, ArchiveConvertPhase::Scanning)
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

    /// 変換済みアーカイブを開く。`src` は元 (RAR/7z/LZH)、`cached_zip` は
    /// 変換済み ZIP のパス。キャッシュ ZIP を load_folder し、その後
    /// `archive_source_override` / `address` を元パスに書き戻す。
    ///
    /// Enter / ダブルクリックのキャッシュヒット経路で使う。変換直後の
    /// pending_nav 経路は `show_archive_convert_dialog` 内で直接処理する
    /// (そちらは `archive_convert` のライフサイクルと絡むため)。
    /// `auto_fullscreen` は **明示的なオープン** (グリッド Enter / ダブルクリック /
    /// ゲームパッド / 起動引数・SendTo × 設定 ON) のときだけ true。履歴の戻る/進む・アドレスバー経由の
    /// `load_folder_or_convert_archive` からは false で呼び、ZIP/PDF と挙動を揃える
    /// (ZIP/PDF も明示オープン時のみ自動フルスクリーン)。`load_folder(cache_zip)` →
    /// `load_zip_as_folder` が同フレームで `pending_auto_fs_open` を消化するので stale 化しない。
    pub(crate) fn open_archive_via_cache(
        &mut self,
        src: PathBuf,
        cached_zip: PathBuf,
        auto_fullscreen: bool,
    ) -> bool {
        if auto_fullscreen {
            self.pending_auto_fs_open = true;
        }
        self.load_folder(cached_zip.clone());
        // load が ★固定 (snapshot lock) の範囲外ガード等でブロックされると current_folder は
        // 変わらない (load_zip_as_folder が current_folder = cache_zip を同期セットする前に
        // return するため)。その場合は override / address / recent を更新しない
        // (current_folder は元の場所のまま override だけ範囲外アーカイブを指す不整合を防ぐ、
        // Codex P1)。戻り値でブロックを呼び出し側にも伝える。
        if !self
            .current_folder
            .as_ref()
            .is_some_and(|cur| crate::folder_tree::path_eq(cur, &cached_zip))
        {
            return false;
        }
        self.address = src.to_string_lossy().to_string();
        // 検索 (Ctrl+G / Ctrl+S) 中は recent_folders を一切変更しない
        // (remember_recent_folder 自体もガード済みだが、retain も検索中は走らせない)。
        if !(self.global_search.active || self.favsearch.active) {
            self.recent_folders
                .retain(|p| !crate::folder_tree::path_eq(p, &cached_zip));
            self.remember_recent_folder(&src);
        }
        self.update_active_quick_folder_target(&src);
        self.archive_source_override = Some(src);
        true
    }

    /// 変換ダイアログを開始する (スキャン fase から)。
    /// 既に別のダイアログが動作中なら無視 (二重起動防止)。
    pub(crate) fn request_archive_convert(
        &mut self,
        src: PathBuf,
        format: ArchiveFormat,
        auto_fullscreen: bool,
    ) -> bool {
        if self.archive_convert.is_some() {
            return false;
        }
        let rx = spawn_archive_scan(src.clone(), format, None);
        let suppress_confirm = self.settings.archive_convert_without_dialog;
        self.archive_convert = Some(ArchiveConvertState {
            src_path: src,
            format,
            password: None,
            password_input: String::new(),
            phase: ArchiveConvertPhase::Scanning,
            rx,
            pending_nav: None,
            nav_history_rollback: None,
            auto_fullscreen,
            suppress_confirm,
            suppress_confirm_next_time: false,
        });
        true
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
            // ConvertDone 受信時に `exists()` は通過しているが、pending_nav 消費までの
            // 短い間隔で並行 maintenance (clear_all/delete_entry) が先に削除する順序レースが
            // 残るため、navigate 直前にもう一度確認する。消えていたらエラー表示に戻す。
            if !nav.exists() {
                if let Some(s) = self.archive_convert.as_mut() {
                    s.phase = ArchiveConvertPhase::Error {
                        message: "変換直後にキャッシュが削除されました。再度お試しください。"
                            .to_string(),
                    };
                }
            } else {
                // 元 (RAR/7z/LZH) のパスを退避してから load_folder (キャッシュ ZIP) を実行、
                // その後 override に元パスを書き戻すことで、UI 表示は元ファイルの場所のままに保つ。
                let src = self.archive_convert.as_ref().map(|s| s.src_path.clone());
                // 明示オープンからの変換 (state.auto_fullscreen=true) のときだけ、変換成功
                // 直後に 1 ページ目を自動フルスクリーン表示する (履歴/アドレスバー経由の
                // 変換は false なので発火しない)。
                let auto_fs = self
                    .archive_convert
                    .as_ref()
                    .map(|s| s.auto_fullscreen)
                    .unwrap_or(false);
                // ブロック時に履歴スタックを巻き戻せるよう、state を drop する前に退避する。
                let nav_history_rollback = self
                    .archive_convert
                    .as_ref()
                    .and_then(|s| s.nav_history_rollback.clone());
                self.archive_convert = None;
                if auto_fs {
                    self.pending_auto_fs_open = true;
                }
                self.load_folder(nav.clone());
                // load が ★固定 (snapshot lock) の範囲外ガード等でブロックされると
                // current_folder は変わらない。その場合は override / address / recent を
                // 更新せず、変換ダイアログを開いたときに変えた履歴スタックも巻き戻す
                // (override と current_folder の不整合・nav スタック残りを防ぐ、Codex P1/P2)。
                let loaded = self
                    .current_folder
                    .as_ref()
                    .is_some_and(|cur| crate::folder_tree::path_eq(cur, &nav));
                if !loaded {
                    if let Some(snapshot) = nav_history_rollback {
                        self.restore_folder_nav_history(snapshot);
                    }
                    return;
                }
                if let Some(src) = src {
                    self.address = src.to_string_lossy().to_string();
                    // 検索 (Ctrl+G / Ctrl+S) 中は recent_folders を一切変更しない。
                    if !(self.global_search.active || self.favsearch.active) {
                        self.recent_folders
                            .retain(|p| !crate::folder_tree::path_eq(p, &nav));
                        self.remember_recent_folder(&src);
                    }
                    self.update_active_quick_folder_target(&src);
                    self.archive_source_override = Some(src);
                }
                if self.favsearch.active {
                    self.update_favsearch_address();
                }
                return;
            }
        }

        if self
            .archive_convert
            .as_ref()
            .is_some_and(|s| archive_convert_window_suppressed(&s.phase, s.suppress_confirm))
        {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }

        let enter_pressed = self.dialog_enter_pressed(ctx);
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let Some(state) = self.archive_convert.as_mut() else {
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
        let mut apply_password = false;

        // ZIP (入れ子アーカイブ展開、v1.3.0) は「ZIP を ZIP に変換」だと意味が通らない
        // ので展開系の文言にする。
        let is_zip_expand = state.format == ArchiveFormat::Zip;
        let title = match &state.phase {
            ArchiveConvertPhase::Scanning => format!("{fmt_label} を読み込み中..."),
            ArchiveConvertPhase::PasswordRequired { .. } => {
                format!("{fmt_label} パスワード入力")
            }
            ArchiveConvertPhase::Confirm { .. } if is_zip_expand => {
                "ZIP 内のアーカイブを展開".to_string()
            }
            ArchiveConvertPhase::Confirm { .. } => {
                format!("{fmt_label} を ZIP に変換")
            }
            ArchiveConvertPhase::Converting { .. } if is_zip_expand => {
                "ZIP 内のアーカイブを展開中".to_string()
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
                    ArchiveConvertPhase::PasswordRequired { message, .. } => {
                        ui.label(format!(
                            "この{fmt_label}ファイルを開くにはパスワードが必要です:"
                        ));
                        ui.label(
                            egui::RichText::new(src_name.as_str())
                                .size(12.0)
                                .color(egui::Color32::from_gray(120)),
                        );
                        ui.add_space(4.0);
                        let resp = ui.add(
                            egui::TextEdit::singleline(&mut state.password_input)
                                .password(true)
                                .desired_width(f32::INFINITY)
                                .hint_text("パスワード"),
                        );
                        if !resp.has_focus() && !ui.memory(|m| m.focused().is_some()) {
                            resp.request_focus();
                        }
                        if enter_pressed && (resp.has_focus() || resp.lost_focus()) {
                            apply_password = true;
                        }
                        if let Some(message) = message {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(message.as_str())
                                    .color(crate::ui_helpers::ERROR_TEXT_COLOR)
                                    .size(crate::ui_helpers::ERROR_TEXT_SIZE),
                            );
                        }
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(
                                "パスワードは保存しません。変換後の ZIP キャッシュはパスワードなしで保存され、キャッシュが残っている間は次回以降そのまま開けます。",
                            )
                            .small()
                            .weak(),
                        );
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(4.0);
                        ui.horizontal(|ui| {
                            let can_apply = !state.password_input.trim().is_empty();
                            if ui
                                .add_enabled(can_apply, egui::Button::new("  OK  "))
                                .clicked()
                            {
                                apply_password = true;
                            }
                            if ui.button("キャンセル").clicked() {
                                should_close = true;
                            }
                        });
                    }
                    ArchiveConvertPhase::Confirm { summary } => {
                        if is_zip_expand {
                            ui.label(
                                "この ZIP には RAR / 7z / LZH などのアーカイブが\
                                 入れ子になっています。",
                            );
                            ui.label(
                                "中身の画像も表示できるように、入れ子を展開した\
                                 閲覧用キャッシュを作成します。",
                            );
                        } else {
                            ui.label(format!(
                                "{fmt_label} を ZIP に変換して閲覧できるようにします。"
                            ));
                        }
                        ui.label(
                            "元ファイルはそのまま残り、変換したファイルが\
                             キャッシュとして作成されます。",
                        );
                        ui.label("キャッシュ管理メニューから削除することができます。");
                        if state.format == ArchiveFormat::Rar && state.password.is_some() {
                            ui.add_space(4.0);
                            ui.label(
                                egui::RichText::new(
                                    "この変換キャッシュはパスワードなしの ZIP として保存されます。",
                                )
                                .color(egui::Color32::from_rgb(170, 120, 40)),
                            );
                        }
                        ui.add_space(10.0);
                        ui.separator();
                        ui.add_space(6.0);
                        ui.label(
                            egui::RichText::new(format!("ファイル: {src_name}"))
                                .size(12.0)
                                .color(egui::Color32::from_gray(160)),
                        );
                        let mut info = format!(
                            "画像ファイル数: {} / 変換後 ZIP の目安: 約 {}",
                            summary.image_count,
                            crate::ui_helpers::format_bytes(summary.total_uncompressed_bytes)
                        );
                        if summary.nested_archive_count > 0 {
                            info.push_str(&format!(
                                " / 入れ子アーカイブ: {} 個 (変換時に展開され、画像数が増えます)",
                                summary.nested_archive_count
                            ));
                        }
                        ui.label(
                            egui::RichText::new(info)
                                .size(12.0)
                                .color(egui::Color32::from_gray(160)),
                        );
                        ui.add_space(10.0);
                        ui.checkbox(&mut state.suppress_confirm_next_time, "次回から表示しない");
                        ui.add_space(6.0);
                        // 直下画像が 0 でも入れ子アーカイブがあれば変換する価値がある
                        // (中身の画像は変換時に展開されて初めて数えられる)。
                        let convertible =
                            summary.image_count > 0 || summary.nested_archive_count > 0;
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(convertible, egui::Button::new("変換して開く"))
                                .clicked()
                            {
                                start_convert = true;
                            }
                            if ui.button("キャンセル").clicked() {
                                should_close = true;
                            }
                        });
                        if !convertible {
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
            let suppress_next_time = self
                .archive_convert
                .as_ref()
                .is_some_and(|state| state.suppress_confirm_next_time);
            if suppress_next_time && !self.settings.archive_convert_without_dialog {
                self.settings.archive_convert_without_dialog = true;
                self.settings.save();
            }
            self.start_archive_convert();
        }
        if apply_password {
            self.apply_archive_password();
        }
        if should_close {
            // 変換中ならキャンセル信号も立てておく (ワーカーは後で気付いて停止)
            if let Some(state) = self.archive_convert.as_ref() {
                if let ArchiveConvertPhase::Converting { cancel, .. } = &state.phase {
                    cancel.store(true, Ordering::Relaxed);
                }
            }
            let nav_history_rollback = self
                .archive_convert
                .as_ref()
                .and_then(|state| state.nav_history_rollback.clone());
            self.archive_convert = None;
            if let Some(snapshot) = nav_history_rollback {
                self.restore_folder_nav_history(snapshot);
            }
        }
    }

    /// バックグラウンドメッセージを取り込んでフェーズ遷移させる。
    fn poll_archive_convert_messages(&mut self) {
        let Some(state) = self.archive_convert.as_mut() else {
            return;
        };
        let mut start_convert_after_poll = false;
        while let Ok(msg) = state.rx.try_recv() {
            match msg {
                ArchiveConvertMsg::ScanDone(Ok(summary)) => {
                    // 直下画像 0 でも入れ子アーカイブがあれば変換対象 (展開で画像が出る)。
                    if summary.image_count == 0 && summary.nested_archive_count == 0 {
                        state.phase = ArchiveConvertPhase::Error {
                            message: "このアーカイブには画像ファイルが含まれていません。"
                                .to_string(),
                        };
                    } else if state.suppress_confirm {
                        state.phase = ArchiveConvertPhase::Confirm { summary };
                        start_convert_after_poll = true;
                    } else {
                        state.phase = ArchiveConvertPhase::Confirm { summary };
                    }
                }
                ArchiveConvertMsg::ScanDone(Err(ConvertError::PasswordRequired)) => {
                    state.password = None;
                    state.password_input.clear();
                    state.phase = ArchiveConvertPhase::PasswordRequired {
                        message: None,
                        resume: ArchivePasswordResume::Scan,
                    };
                }
                ArchiveConvertMsg::ScanDone(Err(ConvertError::BadPassword)) => {
                    state.password = None;
                    state.password_input.clear();
                    state.phase = ArchiveConvertPhase::PasswordRequired {
                        message: Some("パスワードが正しくありません".to_string()),
                        resume: ArchivePasswordResume::Scan,
                    };
                }
                ArchiveConvertMsg::ScanDone(Err(e)) => {
                    state.phase = ArchiveConvertPhase::Error {
                        message: format!("スキャン失敗: {e}"),
                    };
                }
                ArchiveConvertMsg::ConvertDone(Ok((_summary, cached_zip, _cached_size))) => {
                    // DB への record は worker 側で convert_lock を握ったまま済ませている
                    // (docs/async-architecture.md: maintenance と convert の直列化)。
                    // ただし convert worker が `ConvertDone` を送信してから guard を drop する
                    // までの間に、待機していた clear_all / delete_entry が動き出して、
                    // 今 record したばかりのエントリごと消す余地がある (convert_lock は
                    // 「変換と保守が同時に走らない」を保証するが、「変換完了 → 保守開始 →
                    // 保守完了 → UI 受信」の順序は許容される)。
                    // ここで navigation 直前に存在確認し、削除済みなら再変換を促す。
                    if cached_zip.exists() {
                        state.pending_nav = Some(cached_zip);
                    } else {
                        state.phase = ArchiveConvertPhase::Error {
                            message: "変換直後にキャッシュが削除されました。再度お試しください。"
                                .to_string(),
                        };
                    }
                }
                ArchiveConvertMsg::ConvertDone(Err(ConvertError::Cancelled)) => {
                    // ユーザーキャンセルならダイアログを即閉じる
                    let nav_history_rollback = state.nav_history_rollback.clone();
                    self.archive_convert = None;
                    if let Some(snapshot) = nav_history_rollback {
                        self.restore_folder_nav_history(snapshot);
                    }
                    return;
                }
                ArchiveConvertMsg::ConvertDone(Err(ConvertError::PasswordRequired)) => {
                    state.password = None;
                    state.password_input.clear();
                    state.phase = ArchiveConvertPhase::PasswordRequired {
                        message: None,
                        resume: ArchivePasswordResume::Convert,
                    };
                }
                ArchiveConvertMsg::ConvertDone(Err(ConvertError::BadPassword)) => {
                    state.password = None;
                    state.password_input.clear();
                    state.phase = ArchiveConvertPhase::PasswordRequired {
                        message: Some("パスワードが正しくありません".to_string()),
                        resume: ArchivePasswordResume::Convert,
                    };
                }
                ArchiveConvertMsg::ConvertDone(Err(e)) => {
                    state.phase = ArchiveConvertPhase::Error {
                        message: format!("変換失敗: {e}"),
                    };
                }
            }
        }
        if start_convert_after_poll && self.archive_convert.is_some() {
            self.start_archive_convert();
        }
    }

    fn apply_archive_password(&mut self) {
        let Some((resume, password, src, format)) =
            self.archive_convert.as_mut().and_then(|state| {
                let (resume, password) = prepare_archive_password_retry(state)?;
                Some((resume, password, state.src_path.clone(), state.format))
            })
        else {
            return;
        };

        match resume {
            ArchivePasswordResume::Scan => {
                if let Some(state) = self.archive_convert.as_mut() {
                    state.rx = spawn_archive_scan(src, format, Some(password));
                    state.phase = ArchiveConvertPhase::Scanning;
                }
            }
            ArchivePasswordResume::Convert => {
                self.start_archive_convert();
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
        let password = state.password.clone();
        let cancel_worker = cancel.clone();
        let progress_worker = progress.clone();
        let db_worker = Arc::clone(&db);
        let archive_cache_max_bytes = self.settings.archive_cache_max_bytes;
        thread::spawn(move || {
            // 変換と保守 (clear_all / delete_entry) を直列化する。guard は worker thread
            // スコープを抜けるまで保持され、その間は maintenance がブロックされる。
            // MutexGuard は !Send なのでここで取り、同 thread 内の record() まで持ち越す。
            let _convert_guard = db_worker.begin_convert();

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
            let result = convert_to_zip_with_password(
                &src,
                &dst,
                format,
                password.as_deref(),
                &cancel_worker,
                Some(&cb),
            );
            let msg = match result {
                Ok(summary) => {
                    let cached_size = std::fs::metadata(&dst).map(|m| m.len() as i64).unwrap_or(0);
                    // ここで record。convert_guard 保持中なので maintenance と排他。
                    if let Ok(meta) = std::fs::metadata(&src) {
                        let src_mtime = crate::ui_helpers::mtime_secs(&meta);
                        let src_size = meta.len() as i64;
                        let record_result = db_worker.record(
                            &src,
                            src_mtime,
                            src_size,
                            format,
                            &dst,
                            cached_size,
                            summary.image_count,
                            format == ArchiveFormat::Rar && password.is_some(),
                        );
                        match record_result {
                            Ok(()) => {
                                if archive_cache_max_bytes > 0 {
                                    match db_worker
                                        .prune_to_size_limit_locked(archive_cache_max_bytes, &src)
                                    {
                                        Ok(removed) if removed > 0 => {
                                            crate::logger::log(format!(
                                                "archive_cache: pruned {removed} entries to stay under {} bytes",
                                                archive_cache_max_bytes
                                            ));
                                        }
                                        Ok(_) => {}
                                        Err(e) => crate::logger::log(format!(
                                            "archive_cache: prune_to_size_limit failed: {e}"
                                        )),
                                    }
                                }
                            }
                            Err(e) => crate::logger::log(format!(
                                "archive_cache: record failed after convert: {e}"
                            )),
                        }
                    }
                    ArchiveConvertMsg::ConvertDone(Ok((summary, dst, cached_size)))
                }
                Err(e) => ArchiveConvertMsg::ConvertDone(Err(e)),
            };
            // guard を先に drop する: 待機中の maintenance を走らせてから `ConvertDone` を
            // 送るため、UI 側の `exists()` チェックは「maintenance 完了後」の状態を見ることに
            // なる。guard 保持のまま send してしまうと、UI が先に `exists()` を評価して
            // pending_nav を立て、その後で maintenance が走って同 ZIP を削除する race が残る。
            drop(_convert_guard);
            let _ = tx.send(msg);
        });
        state.phase = ArchiveConvertPhase::Converting { progress, cancel };
        state.rx = rx;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_for_password_resume(resume: ArchivePasswordResume) -> ArchiveConvertState {
        let (_tx, rx) = mpsc::channel();
        ArchiveConvertState {
            src_path: PathBuf::from(r"C:\tmp\locked.rar"),
            format: ArchiveFormat::Rar,
            password: None,
            password_input: "  mivtest2026  ".to_string(),
            phase: ArchiveConvertPhase::PasswordRequired {
                message: None,
                resume,
            },
            rx,
            pending_nav: None,
            nav_history_rollback: None,
            auto_fullscreen: false,
            suppress_confirm: false,
            suppress_confirm_next_time: false,
        }
    }

    #[test]
    fn prepare_password_retry_keeps_scan_resume() {
        let mut state = state_for_password_resume(ArchivePasswordResume::Scan);
        let (resume, password) = prepare_archive_password_retry(&mut state).unwrap();

        assert_eq!(resume, ArchivePasswordResume::Scan);
        assert_eq!(password, "mivtest2026");
        assert_eq!(state.password.as_deref(), Some("mivtest2026"));
        assert!(state.password_input.is_empty());
    }

    #[test]
    fn prepare_password_retry_keeps_convert_resume() {
        let mut state = state_for_password_resume(ArchivePasswordResume::Convert);
        let (resume, password) = prepare_archive_password_retry(&mut state).unwrap();

        assert_eq!(resume, ArchivePasswordResume::Convert);
        assert_eq!(password, "mivtest2026");
        assert_eq!(state.password.as_deref(), Some("mivtest2026"));
        assert!(state.password_input.is_empty());
    }

    #[test]
    fn suppress_confirm_hides_only_scanning_phase() {
        assert!(archive_convert_window_suppressed(
            &ArchiveConvertPhase::Scanning,
            true
        ));
        assert!(!archive_convert_window_suppressed(
            &ArchiveConvertPhase::Converting {
                progress: Arc::new(ArchiveConvertProgressShared::new()),
                cancel: Arc::new(AtomicBool::new(false)),
            },
            true
        ));
        assert!(!archive_convert_window_suppressed(
            &ArchiveConvertPhase::PasswordRequired {
                message: None,
                resume: ArchivePasswordResume::Scan,
            },
            true
        ));
        assert!(!archive_convert_window_suppressed(
            &ArchiveConvertPhase::Error {
                message: "failed".to_string(),
            },
            true
        ));
        assert!(!archive_convert_window_suppressed(
            &ArchiveConvertPhase::Scanning,
            false
        ));
    }
}

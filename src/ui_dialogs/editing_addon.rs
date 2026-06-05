//! 編集用追加パック (オノマトペ向けフォント + 被写体分離モデル) の
//! オンライン取得ダイアログ。
//!
//! 補正レイヤー / 吹き出し / テキスト / オノマトペ など追加ファイルを要する機能へ
//! 初めて入ったとき、未導入なら確認モーダルを出す ([`App::maybe_prompt_editing_addon`])。
//! TensorRT pack の [`crate::ui_dialogs::trt_install`] と同じ造り:
//!
//! 1. **Confirm**: 追加内容と容量目安 → [ダウンロード] で Running、[今はしない] で
//!    `declined_this_session` を立てて閉じる (= 同一セッションで再表示しない)。
//! 2. **Running**: 配布一覧取得 → zip DL → 検証 → 展開 → ファイル検証 → 配置 を
//!    プログレスバー + フェーズラベルで表示。途中 [キャンセル] でアボート。
//! 3. **Done**: 「追加されました」。フォント / モデルが利用可能になる。
//! 4. **Error**: 失敗理由 + [再試行] [閉じる]。
//!
//! ## アーキテクチャ
//!
//! - DL/展開本体は [`crate::editing_addon_download`] の worker thread。
//! - `App::editing_addon_install_state` が `Some` の間ダイアログ表示 + worker 動作。
//!   閉じる (Drop) と worker は cancel される。

use eframe::egui;

use crate::app::App;
use crate::editing_addon_download::{InstallHandle, InstallProgress, start_install};

/// 確認モーダルに出すダウンロード容量の目安 (MB)。実値は IndexFetched で上書きされる。
/// フォント ~62 MiB + BiRefNet fp16 ~490 MiB ≒ 550 MB。
const APPROX_SIZE_MB: u64 = 550;

/// インストールダイアログのフェーズ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EditingAddonPhase {
    /// 確認画面 (まだ DL 開始していない)。
    Confirm,
    /// worker thread 動作中。
    Running,
    /// 全完了 (active.json 更新済み)。
    Done,
    /// 致命的エラー。
    Error(String),
}

/// インストール状態。`App::editing_addon_install_state` に保持される。
pub struct EditingAddonInstallState {
    phase: EditingAddonPhase,
    handle: Option<InstallHandle>,
    /// 選定された pack version (IndexFetched で更新)。
    version: String,
    /// zip 合計バイト (進捗バー分母)。0 = 未取得。
    zip_bytes: u64,
    /// DL 済みバイト (Downloading の絶対値)。
    bytes_done: u64,
    /// 同梱フォント数 (IndexFetched、UI 表示)。
    font_count: u32,
    /// 被写体分離モデル名 (IndexFetched、UI 表示)。
    subject_model: String,
    /// 現在のサブフェーズ表示文。
    phase_label: String,
    /// 展開進捗。
    extract_done: usize,
    extract_total: usize,
    /// ファイル検証進捗。
    verify_done: usize,
    verify_total: usize,
    /// Done 遷移後、キャッシュ無効化 (フォント再読込) をまだ行っていなければ true。
    /// `take_reload` で 1 回だけ消費される。
    needs_reload: bool,
}

impl EditingAddonInstallState {
    /// 新規インストールセッションを Confirm フェーズで開始。
    pub fn new() -> Self {
        Self {
            phase: EditingAddonPhase::Confirm,
            handle: None,
            version: String::new(),
            zip_bytes: 0,
            bytes_done: 0,
            font_count: 0,
            subject_model: String::new(),
            phase_label: String::new(),
            extract_done: 0,
            extract_total: 0,
            verify_done: 0,
            verify_total: 0,
            needs_reload: false,
        }
    }

    /// Done 遷移後に呼び、フォント再読込フラグを 1 回消費する。
    fn take_reload(&mut self) -> bool {
        let v = self.needs_reload;
        self.needs_reload = false;
        v
    }

    /// Confirm 画面で [ダウンロード] が押された時に呼ぶ。worker を spawn して Running へ。
    fn start(&mut self) {
        self.handle = Some(start_install());
        self.phase = EditingAddonPhase::Running;
        self.phase_label = "配布一覧を取得中...".to_string();
    }

    /// Running 中に [キャンセル] が押された時に worker へ通知する。
    /// dialog 自体は呼び出し側が state を drop して閉じる (Drop で worker 停止)。
    fn cancel(&self) {
        if let Some(h) = &self.handle {
            h.cancel();
        }
    }

    /// 1 フレーム分の進捗を吸い上げる。Running 中のみ呼ぶ。
    /// Returns: フェーズ遷移したか (Done / Error に至ったか)。
    fn pump_progress(&mut self) -> bool {
        let Some(handle) = self.handle.as_mut() else {
            return false;
        };
        let mut transitioned = false;
        for _ in 0..64 {
            match handle.poll() {
                Some(InstallProgress::FetchingIndex) => {
                    self.phase_label = "配布一覧を取得中...".to_string();
                }
                Some(InstallProgress::IndexFetched {
                    version,
                    zip_bytes,
                    font_count,
                    subject_model,
                }) => {
                    self.version = version;
                    self.zip_bytes = zip_bytes;
                    self.font_count = font_count;
                    self.subject_model = subject_model;
                    self.phase_label = "ダウンロード準備中...".to_string();
                }
                Some(InstallProgress::Downloading {
                    bytes_done,
                    bytes_total,
                }) => {
                    self.bytes_done = bytes_done;
                    if bytes_total > 0 {
                        self.zip_bytes = bytes_total;
                    }
                    self.phase_label = "ダウンロード中".to_string();
                }
                Some(InstallProgress::VerifyingZip) => {
                    self.bytes_done = self.zip_bytes;
                    self.phase_label = "ダウンロードファイルを検証中...".to_string();
                }
                Some(InstallProgress::Extracting { entry_index, total }) => {
                    self.extract_done = entry_index;
                    self.extract_total = total;
                    self.phase_label = "展開中...".to_string();
                }
                Some(InstallProgress::VerifyingFiles { file_index, total }) => {
                    self.verify_done = file_index;
                    self.verify_total = total;
                    self.phase_label = "ファイルを検証中...".to_string();
                }
                Some(InstallProgress::Installing) => {
                    self.phase_label = "配置中...".to_string();
                }
                Some(InstallProgress::Done { version }) => {
                    self.version = version;
                    self.phase = EditingAddonPhase::Done;
                    self.needs_reload = true;
                    transitioned = true;
                    break;
                }
                Some(InstallProgress::Cancelled) => {
                    // ユーザーキャンセル。worker は片付け済み。dialog を閉じるため
                    // Error 扱いにはせず、呼び出し側でクローズさせる。
                    self.phase = EditingAddonPhase::Error("キャンセルされました".to_string());
                    transitioned = true;
                    break;
                }
                Some(InstallProgress::Error { message }) => {
                    self.phase = EditingAddonPhase::Error(message);
                    transitioned = true;
                    break;
                }
                None => break,
            }
        }
        if !transitioned && handle.is_finished() && self.phase == EditingAddonPhase::Running {
            self.phase = EditingAddonPhase::Error(
                "ワーカーが想定外に終了しました。ログを確認してください。".to_string(),
            );
            transitioned = true;
        }
        transitioned
    }

    /// Confirm へ戻して再試行できる状態にリセットする。
    fn reset_to_confirm(&mut self) {
        self.phase = EditingAddonPhase::Confirm;
        self.handle = None;
        self.bytes_done = 0;
        self.zip_bytes = 0;
        self.extract_done = 0;
        self.extract_total = 0;
        self.verify_done = 0;
        self.verify_total = 0;
        self.phase_label.clear();
    }
}

impl Default for EditingAddonInstallState {
    fn default() -> Self {
        Self::new()
    }
}

impl App {
    /// 編集系機能の入口で呼ぶ。追加パックが未導入かつ未辞退 (このセッション) なら
    /// 確認ダイアログを開く。導入済み / 既に表示中 / 辞退済みなら何もしない。
    ///
    /// 追加ファイルが無くても基本 UI は使える (テキストはシステムフォントで動く) ので、
    /// この prompt は機能の利用を阻害しない。
    pub(crate) fn maybe_prompt_editing_addon(&mut self) {
        if self.editing_addon_install_state.is_some() {
            return; // 既に表示中
        }
        if self.editing_addon_declined_session {
            return; // このセッションで辞退済み
        }
        if crate::editing_addon::is_installed() {
            return; // 導入済み
        }
        self.editing_addon_install_state = Some(EditingAddonInstallState::new());
    }

    /// 編集用追加パックの導入完了直後に呼ばれ、フォント等のキャッシュを無効化する。
    /// 次回ベイク時に追加パックのフォントを含む FontSet が読み直される。
    fn on_editing_addon_installed(&mut self) {
        self.comic_fonts = None;
        self.comic_fonts_loaded = false;
        self.refresh_subject_matte_path();
        crate::logger::log(
            "[editing pack] install 完了、フォント / 被写体マットキャッシュを無効化".to_string(),
        );
    }

    /// インストールダイアログ本体。`update()` から毎フレーム呼ぶ。
    /// state が None のときは即 return。
    pub(crate) fn show_editing_addon_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut state) = self.editing_addon_install_state.take() else {
            return;
        };

        if state.phase == EditingAddonPhase::Running {
            state.pump_progress();
        }

        if matches!(state.phase, EditingAddonPhase::Done) && state.take_reload() {
            self.on_editing_addon_installed();
        }

        let mut close_dialog = false;
        let mut decline = false;
        let mut retry_clicked = false;

        let content = ctx.content_rect();
        let default_pos = content.min + egui::vec2(80.0, 60.0);

        egui::Window::new("編集用追加ファイルのダウンロード")
            .id(egui::Id::new("editing_addon_dialog"))
            .default_pos(default_pos)
            .default_width(560.0)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 10.0;
                match &state.phase {
                    EditingAddonPhase::Confirm => {
                        draw_confirm_phase(ui, &mut state, &mut decline);
                    }
                    EditingAddonPhase::Running => {
                        draw_running_phase(ui, &state, &mut close_dialog);
                    }
                    EditingAddonPhase::Done => {
                        draw_done_phase(ui, &state, &mut close_dialog);
                    }
                    EditingAddonPhase::Error(msg) => {
                        let msg = msg.clone();
                        draw_error_phase(ui, &msg, &mut close_dialog, &mut retry_clicked);
                    }
                }
            });

        if retry_clicked {
            state.reset_to_confirm();
        }

        if decline {
            // 「今はしない」: このセッションでは二度と prompt しない。
            self.editing_addon_declined_session = true;
            // state を保持し直さない = drop = dialog 閉じる (worker は未起動)。
            return;
        }

        if close_dialog {
            state.cancel(); // worker へ通知 (Drop でも止まる)
        // state を drop = 閉じる
        } else {
            self.editing_addon_install_state = Some(state);
            ctx.request_repaint();
        }
    }
}

fn draw_confirm_phase(ui: &mut egui::Ui, state: &mut EditingAddonInstallState, decline: &mut bool) {
    ui.label("追加の編集用フォント・AI モデルをダウンロードしますか？");
    ui.add_space(2.0);
    ui.indent("editing_addon_summary", |ui| {
        ui.label("• オノマトペ / テキスト向けの日本語フォント");
        ui.label("• 被写体分離 (人物などの切り抜き) 用 AI モデル");
        ui.label(format!("• ダウンロードサイズ: 約 {APPROX_SIZE_MB} MB"));
    });
    ui.separator();
    ui.label(
        "追加ファイルが無くても基本的なテキスト編集はシステムフォントで利用できます。\
         ダウンロードは中断・再開でき、後で環境設定からも導入できます。",
    );
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("ダウンロード").clicked() {
            state.start();
        }
        if ui.button("今はしない").clicked() {
            *decline = true;
        }
    });
}

fn draw_running_phase(ui: &mut egui::Ui, state: &EditingAddonInstallState, close: &mut bool) {
    ui.label("編集用追加ファイルをダウンロードしています");

    let ratio = if state.zip_bytes > 0 {
        (state.bytes_done as f32 / state.zip_bytes as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let text = if state.zip_bytes > 0 {
        format!(
            "{:.1} / {:.1} MB ({:.0}%)",
            state.bytes_done as f64 / 1_000_000.0,
            state.zip_bytes as f64 / 1_000_000.0,
            ratio * 100.0
        )
    } else {
        state.phase_label.clone()
    };
    ui.add(
        egui::ProgressBar::new(ratio)
            .text(text)
            .desired_width(520.0),
    );

    ui.add_space(4.0);
    let mut detail = state.phase_label.clone();
    if state.extract_total > 0 && state.phase_label.starts_with("展開") {
        detail = format!(
            "展開中... ({} / {})",
            state.extract_done + 1,
            state.extract_total
        );
    } else if state.verify_total > 0 && state.phase_label.starts_with("ファイル") {
        detail = format!(
            "ファイルを検証中... ({} / {})",
            state.verify_done + 1,
            state.verify_total
        );
    }
    if !detail.is_empty() {
        ui.label(detail);
    }

    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("キャンセル").clicked() {
            *close = true;
        }
    });
}

fn draw_done_phase(ui: &mut egui::Ui, state: &EditingAddonInstallState, close: &mut bool) {
    ui.colored_label(
        egui::Color32::from_rgb(120, 200, 120),
        "✓ 編集用追加ファイルを導入しました",
    );
    ui.add_space(4.0);
    if state.font_count > 0 {
        ui.label(format!(
            "フォント {} 書体が利用可能になりました。",
            state.font_count
        ));
    } else {
        ui.label("フォントと被写体分離モデルが利用可能になりました。");
    }
    if !state.subject_model.is_empty() {
        ui.label(format!("被写体分離モデル: {}", state.subject_model));
    }
    ui.separator();
    if ui.button("閉じる").clicked() {
        *close = true;
    }
}

fn draw_error_phase(ui: &mut egui::Ui, message: &str, close: &mut bool, retry: &mut bool) {
    ui.colored_label(
        egui::Color32::from_rgb(220, 100, 100),
        "✗ ダウンロードに失敗しました",
    );
    ui.add_space(4.0);
    ui.label(message);
    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("再試行").clicked() {
            *retry = true;
        }
        if ui.button("閉じる").clicked() {
            *close = true;
        }
    });
}

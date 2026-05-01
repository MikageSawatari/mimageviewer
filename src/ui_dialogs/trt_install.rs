//! TensorRT pack インストール用のオンライン取得ダイアログ。
//!
//! 環境設定の AI バックエンドページから「TensorRT パックをダウンロードする」を
//! 押すと開く。この 1 つのダイアログで以下のフェーズを段階的に表示する:
//!
//! 1. **Confirm**: 容量・所要時間・対応 GPU の確認 → ユーザーが [開始] を押すと
//!    Running へ
//! 2. **Running**: manifest fetch → notices DL → common DLL DL × N → engine zip
//!    DL → engine zip 展開 をプログレスバーで表示。途中 [キャンセル] でアボート
//! 3. **Done**: 「次回起動時から TensorRT が有効になります」のメッセージ。
//!    [閉じる] のみ (再起動は手動 or アプリ再起動時に AiRuntime が拾う)
//! 4. **Error**: 失敗理由 + [再試行] [閉じる]
//!
//! ## アーキテクチャ
//!
//! - インストール処理本体は [`crate::ai::tensorrt_installer`] の worker thread。
//!   このダイアログは UI からハンドルを保持して毎フレーム `poll` する役回り
//! - `App` には `Option<TrtInstallState>` を持たせ、ダイアログを開いている間だけ
//!   Some。閉じたら drop されて worker も自動停止 (Drop で cancel)

use eframe::egui;

use crate::ai::tensorrt_installer::{InstallHandle, InstallProgress, start_install};
use crate::app::App;

/// インストールダイアログのフェーズ。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrtInstallPhase {
    /// 確認画面 (まだ DL 開始していない)。
    Confirm,
    /// worker thread 動作中。
    Running,
    /// 全完了 (INSTALL_OK 書き込み済み)。
    Done,
    /// 致命的エラー。
    Error(String),
}

/// インストール状態。`App::trt_install_state` に保持される。
pub struct TrtInstallState {
    /// 現在のフェーズ。
    phase: TrtInstallPhase,
    /// worker handle。Confirm/Done/Error フェーズでは None。
    handle: Option<InstallHandle>,
    /// 検出済みユーザー GPU の SM × 10。
    /// インストールダイアログを開くタイミングで gpu_info から取得。
    target_sm_x10: Option<u32>,
    /// 全 DL 量 (bytes)、ManifestFetched で更新。0 = 未取得。
    total_bytes: u64,
    /// 累積 DL 完了量 (bytes)。FileProgress で running 加算、完了ファイルは bytes_total を加算。
    bytes_done: u64,
    /// 直近進捗ファイル名 (UI 表示用)。
    current_file: String,
    /// 完了済みファイル数 / 総ファイル数。
    files_done: usize,
    files_total: usize,
    /// engine zip 展開中の進捗。
    extract_done: usize,
    extract_total: usize,
    /// 直前のファイル名と bytes_done を覚えておいて、次のファイルに移ったタイミングで
    /// 「直前ファイルの完了分」を total に加算する (= sender 側で 50ms 間引きしている
    /// ので最後の FileProgress が来ない可能性があるため)。
    last_file_bytes_done: u64,
    last_file_name: String,
    /// Done フェーズに遷移後、まだ TensorRT バックエンドの自動有効化を行って
    /// いないなら true。`take_backend_activation` で 1 回だけ消費される。
    /// (App 側がこれを見て settings.ai_backend = "tensorrt" を書き、worker pool を
    /// 起動する)。
    needs_backend_activation: bool,
}

impl TrtInstallState {
    /// 新規インストールセッションを開く。`Confirm` フェーズで開始。
    pub fn new(target_sm_x10: Option<u32>) -> Self {
        Self {
            phase: TrtInstallPhase::Confirm,
            handle: None,
            target_sm_x10,
            total_bytes: 0,
            bytes_done: 0,
            current_file: String::new(),
            files_done: 0,
            files_total: 0,
            extract_done: 0,
            extract_total: 0,
            last_file_bytes_done: 0,
            last_file_name: String::new(),
            needs_backend_activation: false,
        }
    }

    /// Done 遷移後に呼び、TensorRT バックエンドの自動有効化フラグを 1 回消費する。
    /// True を返した場合、App 側で settings.ai_backend = "tensorrt" + save +
    /// apply_ai_backend_change を実行すべき。
    pub fn take_backend_activation(&mut self) -> bool {
        let v = self.needs_backend_activation;
        self.needs_backend_activation = false;
        v
    }

    /// Confirm 画面で [開始] が押された時に呼ぶ。worker を spawn して Running へ。
    fn start(&mut self) {
        self.handle = Some(start_install(self.target_sm_x10));
        self.phase = TrtInstallPhase::Running;
    }

    /// Running 中に [キャンセル] が押された時に呼ぶ。worker に通知 + dialog は閉じる。
    fn cancel_and_close(&mut self) {
        if let Some(h) = &self.handle {
            h.cancel();
        }
        // ダイアログ閉じるのは呼び出し側が handle.take() + dialog 閉じ
    }

    /// 1 フレーム分の進捗を吸い上げる。Running 中のみ呼ぶ。
    /// Returns: フェーズ遷移したか (Done / Error に至ったか)。
    fn pump_progress(&mut self) -> bool {
        let Some(handle) = self.handle.as_mut() else {
            return false;
        };
        let mut transitioned = false;
        // 1 フレームに最大 64 イベント処理 (バーストでも UI が固まらない上限)
        for _ in 0..64 {
            match handle.poll() {
                Some(InstallProgress::FetchingManifest) => {
                    self.current_file = "manifest.json".to_string();
                }
                Some(InstallProgress::ManifestFetched {
                    total_files,
                    total_bytes,
                    ..
                }) => {
                    self.total_bytes = total_bytes;
                    self.files_total = total_files;
                }
                Some(InstallProgress::StartingFile {
                    name,
                    file_index,
                    total_files,
                    bytes_total: _,
                }) => {
                    // 前のファイル分を累積に確定する (途中で 50ms 間引きで取りこぼした
                    // 最終バイト数を補正)。
                    if !self.last_file_name.is_empty() {
                        // last_file_bytes_done は last_file_name の進捗値そのもの。
                        // 完了したと仮定して total に加算済みのはずなので、ここでは差分のみ補正。
                        // → シンプルに「ファイル境界で last をリセット」だけにする
                    }
                    self.current_file = name;
                    self.last_file_name = self.current_file.clone();
                    self.last_file_bytes_done = 0;
                    self.files_done = file_index;
                    self.files_total = total_files;
                }
                Some(InstallProgress::FileProgress {
                    name,
                    bytes_done,
                    bytes_total,
                }) => {
                    if name == self.last_file_name {
                        // 同一ファイル内の差分だけ加算
                        let delta = bytes_done.saturating_sub(self.last_file_bytes_done);
                        self.bytes_done = self.bytes_done.saturating_add(delta);
                        self.last_file_bytes_done = bytes_done;
                        // ファイル完了 (bytes_done == bytes_total) なら files_done を増やす。
                        // ただし StartingFile が次に来ると files_done は上書きされるので
                        // ここで増やす必要はない (= UI の "X / N 個目" 表示は StartingFile 起点)。
                        let _ = bytes_total;
                    } else {
                        // ファイル切替、再同期 (StartingFile が来る前にこの分岐に入る可能性は低い)
                        self.current_file = name.clone();
                        self.last_file_name = name;
                        self.last_file_bytes_done = bytes_done;
                        self.bytes_done = self.bytes_done.saturating_add(bytes_done);
                    }
                }
                Some(InstallProgress::VerifyingFile { name }) => {
                    self.current_file = format!("{} (検証中)", name);
                }
                Some(InstallProgress::ExtractingEngine { entry_index, total }) => {
                    self.extract_done = entry_index;
                    self.extract_total = total;
                    self.current_file = format!("エンジン展開中 ({}/{})", entry_index + 1, total);
                }
                Some(InstallProgress::Done) => {
                    self.phase = TrtInstallPhase::Done;
                    // App 側で 1 回だけ TensorRT 自動有効化を実行するためのフラグ。
                    // Done 描画される最初のフレームで App が消費する。
                    self.needs_backend_activation = true;
                    transitioned = true;
                    break;
                }
                Some(InstallProgress::Error { message }) => {
                    self.phase = TrtInstallPhase::Error(message);
                    transitioned = true;
                    break;
                }
                None => break,
            }
        }
        // worker thread が外から見て finished かどうかも確認する (チャネル空 + thread 死亡時)
        if !transitioned && handle.is_finished() && self.phase == TrtInstallPhase::Running {
            // チャネルに最終 progress が無いまま thread が exit していれば、原因不明エラー扱い。
            self.phase = TrtInstallPhase::Error(
                "ワーカーが想定外に終了しました。ログを確認してください。".to_string(),
            );
            transitioned = true;
        }
        transitioned
    }
}

impl App {
    /// install 完了直後に呼ばれ、TensorRT バックエンドを「保存 + ホットスイッチ」する。
    ///
    /// `apply_ai_backend_change` は worker pool の起動/停止を担当するメソッドで、
    /// 環境設定ダイアログの Apply パスからも呼ばれる。設定値変更を伴うので
    /// settings 永続化もここでやる。
    ///
    /// 起動失敗時は `apply_ai_backend_change` 内で `report_worker_spawn_failed`
    /// が呼ばれ、UI バナー (trt_worker_notice) で通知される。
    fn activate_tensorrt_after_install(&mut self) {
        let prev = self.settings.ai_backend.clone();
        self.settings.ai_backend = Some("tensorrt".to_string());
        self.settings.save();
        crate::logger::log(format!(
            "[AI] install 完了後、ai_backend を自動切替: {:?} -> tensorrt",
            prev
        ));
        // ホットスイッチ (= worker pool を起動)。再起動不要。
        self.apply_ai_backend_change(Some("tensorrt"));
    }

    /// インストールダイアログ本体。`update()` から毎フレーム呼ぶ。
    /// ダイアログを開いていない (= state が None) ときは即 return。
    pub(crate) fn show_trt_install_dialog(&mut self, ctx: &egui::Context) {
        // pump_progress と UI の借用が衝突するので state を一度 take する。
        let Some(mut state) = self.trt_install_state.take() else {
            return;
        };

        // Running なら 1 フレーム分の進捗を吸う。
        if state.phase == TrtInstallPhase::Running {
            state.pump_progress();
        }

        // Done に遷移したフレームで TensorRT を自動有効化 (= ホットスイッチ)。
        // worker 分離アーキテクチャなのでアプリ再起動は不要。take_backend_activation
        // は 1 回だけ true を返すので、Done 描画中の毎フレーム実行にはならない。
        if matches!(state.phase, TrtInstallPhase::Done) && state.take_backend_activation() {
            self.activate_tensorrt_after_install();
        }

        let mut close_dialog = false;
        let mut retry_clicked = false;

        let content = ctx.content_rect();
        let default_pos = content.min + egui::vec2(80.0, 60.0);

        egui::Window::new("TensorRT 高速化パックのインストール")
            .id(egui::Id::new("trt_install_dialog"))
            .default_pos(default_pos)
            .default_width(560.0)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 10.0;

                match &state.phase {
                    TrtInstallPhase::Confirm => {
                        draw_confirm_phase(ui, &mut state, &mut close_dialog);
                    }
                    TrtInstallPhase::Running => {
                        draw_running_phase(ui, &mut state, &mut close_dialog);
                    }
                    TrtInstallPhase::Done => {
                        draw_done_phase(ui, &mut close_dialog);
                    }
                    TrtInstallPhase::Error(msg) => {
                        let msg = msg.clone();
                        draw_error_phase(ui, &msg, &mut close_dialog, &mut retry_clicked);
                    }
                }
            });

        if retry_clicked {
            state.phase = TrtInstallPhase::Confirm;
            state.handle = None;
            state.total_bytes = 0;
            state.bytes_done = 0;
            state.files_done = 0;
            state.files_total = 0;
            state.current_file.clear();
            state.last_file_name.clear();
            state.last_file_bytes_done = 0;
            state.extract_done = 0;
            state.extract_total = 0;
        }

        if close_dialog {
            // ハンドルが残っていれば cancel 経由で worker を片付ける (Drop でも止まる)
            state.cancel_and_close();
            // state を保持し直さない = drop = worker 停止 + dialog 閉じる
        } else {
            // 表示続行
            self.trt_install_state = Some(state);
            // Running 中は進捗が頻繁に更新されるのでフレームを進める。
            // egui は requesting_repaint しない限り入力なしでフレームを止めるため。
            ctx.request_repaint();
        }
    }
}

fn draw_confirm_phase(ui: &mut egui::Ui, state: &mut TrtInstallState, close: &mut bool) {
    ui.label("TensorRT 高速化パックを GitHub からダウンロードします。");
    ui.add_space(2.0);
    ui.indent("trt_install_summary", |ui| {
        ui.label("• ダウンロード容量: 約 1.97 GB");
        ui.label("• 所要時間: 5〜15 分 (ネットワーク速度による)");
        ui.label("• 対応 GPU: RTX 30 / 40 / 50 シリーズ (compute capability 8.0+)");
        match state.target_sm_x10 {
            Some(sm) if sm >= 80 => {
                ui.colored_label(
                    egui::Color32::from_rgb(120, 200, 120),
                    format!(
                        "✓ 検出された GPU は対応しています (compute capability {}.{})",
                        sm / 10,
                        sm % 10
                    ),
                );
            }
            Some(sm) => {
                ui.colored_label(
                    egui::Color32::from_rgb(220, 100, 100),
                    format!(
                        "✗ 検出された GPU は対応していません (compute capability {}.{})。\n\
                         RTX 20 シリーズ以前は DirectML をご利用ください。",
                        sm / 10,
                        sm % 10
                    ),
                );
            }
            None => {
                ui.colored_label(
                    egui::Color32::from_rgb(220, 180, 100),
                    "⚠ GPU の compute capability を判定できませんでした。\
                     インストールを試行することはできますが、対応外 GPU の場合は失敗します。",
                );
            }
        }
    });

    ui.separator();
    ui.label(
        "ダウンロードは中断・再開できます。インストール後、TensorRT が次回起動時から \
         自動的に有効になります。",
    );

    ui.separator();
    ui.horizontal(|ui| {
        let can_start =
            matches!(state.target_sm_x10, Some(sm) if sm >= 80) || state.target_sm_x10.is_none();
        if ui
            .add_enabled(can_start, egui::Button::new("開始"))
            .clicked()
        {
            state.start();
        }
        if ui.button("キャンセル").clicked() {
            *close = true;
        }
    });
}

fn draw_running_phase(ui: &mut egui::Ui, state: &mut TrtInstallState, close: &mut bool) {
    ui.label("ダウンロード中...");

    // 全体進捗 (bytes ベース)
    let overall_ratio = if state.total_bytes > 0 {
        (state.bytes_done as f32 / state.total_bytes as f32).clamp(0.0, 1.0)
    } else {
        0.0
    };
    let overall_text = if state.total_bytes > 0 {
        format!(
            "{:.1} / {:.1} MiB ({:.0}%)",
            state.bytes_done as f64 / 1024.0 / 1024.0,
            state.total_bytes as f64 / 1024.0 / 1024.0,
            overall_ratio * 100.0
        )
    } else {
        "manifest を取得中...".to_string()
    };
    ui.add(
        egui::ProgressBar::new(overall_ratio)
            .text(overall_text)
            .desired_width(520.0),
    );

    ui.add_space(4.0);

    // ファイル単位進捗
    if state.files_total > 0 {
        ui.label(format!(
            "ファイル: {} ({} / {} 個目)",
            state.current_file,
            state.files_done + 1,
            state.files_total
        ));
    } else {
        ui.label(state.current_file.clone());
    }

    if state.extract_total > 0 {
        ui.label(format!(
            "エンジン展開: {} / {}",
            state.extract_done + 1,
            state.extract_total
        ));
    }

    ui.separator();
    ui.horizontal(|ui| {
        if ui.button("キャンセル").clicked() {
            *close = true;
        }
    });
}

fn draw_done_phase(ui: &mut egui::Ui, close: &mut bool) {
    ui.colored_label(
        egui::Color32::from_rgb(120, 200, 120),
        "✓ TensorRT パックのインストールが完了しました",
    );
    ui.add_space(4.0);
    ui.label(
        "TensorRT が有効になりました。アップスケール / デノイズはこれより \
         TensorRT 経由で動作します (アプリ再起動は不要)。",
    );
    ui.add_space(2.0);
    ui.label(
        egui::RichText::new("※ ワーカープロセスの起動でエラーが出た場合は画面右上に通知が出ます。")
            .small(),
    );
    ui.separator();
    if ui.button("閉じる").clicked() {
        *close = true;
    }
}

fn draw_error_phase(ui: &mut egui::Ui, message: &str, close: &mut bool, retry: &mut bool) {
    ui.colored_label(
        egui::Color32::from_rgb(220, 100, 100),
        "✗ インストールに失敗しました",
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

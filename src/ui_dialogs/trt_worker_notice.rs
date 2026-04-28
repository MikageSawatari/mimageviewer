//! TensorRT ワーカー関連の通知バナー (Phase 3 Step 5)。
//!
//! 表示条件:
//! - **起動失敗**: 設定で TensorRT ON にしたが pack 不在 / engine 不整合 / DLL
//!   ロード失敗等で `TrtWorkerPool::start()` が Err を返した。AI は DirectML で
//!   動作しているがユーザーは「TRT になっていない」ことを知らないので通知する。
//! - **推論中の死亡**: 一度起動成功した後の `infer` / `load_model` 中に
//!   stdin/stdout が破断した (子プロセスが crash 等で死亡)。`infer_via_worker` 内で
//!   pool が detach 済みなので以降の推論は DirectML にフォールバック。バナーで
//!   ユーザーに知らせ、必要なら再起動できるようにする。
//!
//! UI:
//! - 右上の小さい floating Window (`anchor` ではなく `default_pos` でドラッグ可)
//! - メッセージ + [ワーカー再起動] [閉じる] の 2 ボタン
//! - 時間で自動消滅しない (ユーザー認知が必要なため)
//! - `TrtBuildPhase::Building` 中は出さない (ビルダー dialog と被るため)

use eframe::egui;

use crate::ai::runtime::{WorkerNotice, WorkerNoticeKind};
use crate::app::App;

impl App {
    /// `AiRuntime::take_worker_notice()` をポーリングし、新しい通知があれば
    /// `App::trt_worker_notice` に転写する。バナー本体の描画は別メソッド。
    pub(crate) fn poll_trt_worker_notice(&mut self) {
        let Some(rt) = self.ai_runtime.as_ref() else {
            return;
        };
        if let Some(notice) = rt.take_worker_notice() {
            // 既存の通知が表示中ならそれを上書き (新しい状況のほうが重要)。
            self.trt_worker_notice = Some(notice);
        }
    }

    /// 通知バナーを 1 個描画する。`App::update()` から毎フレーム呼ぶ。
    pub(crate) fn show_trt_worker_notice_dialog(&mut self, ctx: &egui::Context) {
        let Some(notice) = self.trt_worker_notice.clone() else {
            return;
        };
        // ビルド進捗ダイアログ表示中は被るので隠す。完了/キャンセル後に
        // 再表示される (バナーはフラグ自体を消さないため)。
        if self.trt_build_state.is_some() {
            return;
        }

        let (title, body, can_retry) = notice_text(&notice);

        // 右上に小さく出す (`anchor` だとドラッグできなくなるので default_pos)。
        let content = ctx.content_rect();
        let default_pos = egui::pos2(content.max.x - 380.0 - 16.0, content.min.y + 56.0);

        let mut close_clicked = false;
        let mut retry_clicked = false;

        egui::Window::new(title)
            .id(egui::Id::new("trt_worker_notice"))
            .default_pos(default_pos)
            .default_width(360.0)
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = 8.0;
                ui.label(body);
                ui.separator();
                ui.horizontal(|ui| {
                    if can_retry {
                        if ui.button("ワーカーを再起動").clicked() {
                            retry_clicked = true;
                        }
                    }
                    if ui.button("閉じる").clicked() {
                        close_clicked = true;
                    }
                });
            });

        if retry_clicked {
            // 通知を消してから再起動 (失敗したらまた通知される)
            self.trt_worker_notice = None;
            if let Some(rt) = self.ai_runtime.as_ref() {
                let runtime_arc = rt.clone();
                Self::spawn_trt_worker_pool(&runtime_arc);
            }
        } else if close_clicked {
            self.trt_worker_notice = None;
        }
    }
}

/// 通知種別ごとの (タイトル, 本文, 再起動ボタンを出すか) を返す。
fn notice_text(notice: &WorkerNotice) -> (&'static str, String, bool) {
    match notice.kind {
        WorkerNoticeKind::SpawnFailed => (
            "TensorRT 起動失敗",
            format!(
                "TensorRT ワーカープロセスを起動できませんでした。\n\
                 AI は DirectML で動作しています。\n\n\
                 詳細: {}",
                notice.detail
            ),
            true,
        ),
        WorkerNoticeKind::DiedDuringInfer => (
            "TensorRT ワーカーが停止",
            format!(
                "TensorRT ワーカーが停止したため、AI は DirectML にフォールバックしました。\n\n\
                 詳細: {}",
                notice.detail
            ),
            true,
        ),
    }
}

//! TensorRT ワーカー関連の通知バナー (Phase 3 Step 5)。
//!
//! 表示条件:
//! - **起動失敗**: 設定で TensorRT ON にしたが pack 不在 / engine 不整合 / DLL
//!   ロード失敗等で `TrtWorkerPool::start()` が Err を返した。transient timeout 系は
//!   1 回だけ silent retry し、それでも駄目ならユーザーに通知する。
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

use crate::ai::runtime::{WorkerNotice, WorkerNoticeKind};
use crate::app::App;
use eframe::egui;

/// セッション中の worker 自動再起動の上限。
///
/// この回数を超えて死亡が続いたら、自動再起動を諦めてバナーで通知する
/// (= TRT pack の異常 / モデル特定の crash 等が疑われ、無限ループにしないため)。
/// 1 回の死亡で 1 つ消費する。手動再起動 (バナーボタン) でカウンタを 0 にリセット。
const MAX_TRT_AUTO_RESTART_ATTEMPTS: u32 = 3;
/// 起動段階の transient 失敗は 1 回だけ自動再試行する。
const MAX_TRT_SPAWN_RESTART_ATTEMPTS: u32 = 1;
/// 起動失敗直後に同じ provider 初期化へ突っ込まないための短い猶予。
const TRT_SPAWN_RETRY_BACKOFF: std::time::Duration = std::time::Duration::from_secs(2);

impl App {
    /// `AiRuntime::take_worker_notice()` をポーリングし、新しい通知があれば
    /// 処理する。バナー本体の描画は別メソッド。
    ///
    /// **DiedDuringInfer の場合は自動再起動を試みる** (`MAX_TRT_AUTO_RESTART_ATTEMPTS`
    /// 回まで silent)。起動時の transient failure も 1 回だけ silent retry する。
    /// それを超えたらバナーを出してユーザーに手動再起動を促す。
    pub(crate) fn poll_trt_worker_notice(&mut self) {
        let Some(rt) = self.ai_runtime.as_ref() else {
            return;
        };
        if rt.has_worker_pool() {
            if self.trt_auto_restart_attempts != 0 || self.trt_spawn_restart_attempts != 0 {
                crate::logger::log(format!(
                    "[AI] TRT worker pool 稼働を確認、自動再起動カウンタをリセット \
                     (died={}, spawn={})",
                    self.trt_auto_restart_attempts, self.trt_spawn_restart_attempts
                ));
            }
            self.trt_auto_restart_attempts = 0;
            self.trt_spawn_restart_attempts = 0;
        }
        let Some(notice) = rt.take_worker_notice() else {
            return;
        };
        match notice.kind {
            crate::ai::runtime::WorkerNoticeKind::SpawnFailed
                if self.trt_spawn_restart_attempts < MAX_TRT_SPAWN_RESTART_ATTEMPTS
                    && is_transient_spawn_failure(&notice.detail) =>
            {
                self.trt_spawn_restart_attempts += 1;
                crate::logger::log(format!(
                    "[AI] TRT worker 起動失敗は一時的な可能性があるため自動再試行 \
                     (#{} / {}): {}",
                    self.trt_spawn_restart_attempts, MAX_TRT_SPAWN_RESTART_ATTEMPTS, notice.detail
                ));
                Self::spawn_trt_worker_pool_guarded_after(
                    rt,
                    self.trt_restart_in_flight.clone(),
                    TRT_SPAWN_RETRY_BACKOFF,
                );
            }
            crate::ai::runtime::WorkerNoticeKind::DiedDuringInfer
                if self.trt_auto_restart_attempts < MAX_TRT_AUTO_RESTART_ATTEMPTS =>
            {
                // 自動再起動: pool は既に detach 済みなので、新しい pool を spawn する。
                // バナーは出さない (silent recovery)。次回推論からは自動的に新 pool 経由。
                self.trt_auto_restart_attempts += 1;
                crate::logger::log(format!(
                    "[AI] worker 死亡を検出、自動再起動 (#{} / {}): {}",
                    self.trt_auto_restart_attempts, MAX_TRT_AUTO_RESTART_ATTEMPTS, notice.detail
                ));
                // 多重 spawn ガード付き: 並行する複数 AI 推論が同じ死亡通知を
                // 観測しても、1 個の spawn 試行しか走らない (Codex P2)。
                Self::spawn_trt_worker_pool_guarded(rt, self.trt_restart_in_flight.clone());
            }
            _ => {
                // 自動再起動できない (SpawnFailed か、retry 上限到達): バナーで通知。
                if matches!(
                    notice.kind,
                    crate::ai::runtime::WorkerNoticeKind::DiedDuringInfer
                ) {
                    crate::logger::log(format!(
                        "[AI] worker 死亡を検出、自動再起動の上限 ({}) に到達。\
                         バナーで手動再起動を促す: {}",
                        MAX_TRT_AUTO_RESTART_ATTEMPTS, notice.detail
                    ));
                }
                self.trt_worker_notice = Some(notice);
            }
        }
    }

    /// 通知バナーを 1 個描画する。`App::update()` から毎フレーム呼ぶ。
    pub(crate) fn show_trt_worker_notice_dialog(&mut self, ctx: &egui::Context) {
        let Some(notice) = self.trt_worker_notice.clone() else {
            return;
        };

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
            // ユーザーが明示的に再起動を選んだ → 自動再起動カウンタも 0 に戻す。
            // (= ユーザー的には「リセット」操作なので、また 3 回 silent recovery が
            // 利く状態に戻す)
            self.trt_auto_restart_attempts = 0;
            self.trt_spawn_restart_attempts = 0;
            // 通知を消してから再起動 (失敗したらまた通知される)
            self.trt_worker_notice = None;
            if let Some(rt) = self.ai_runtime.as_ref() {
                let runtime_arc = rt.clone();
                Self::spawn_trt_worker_pool(&runtime_arc, self.local_ai_activity_lease());
            }
        } else if close_clicked {
            self.trt_worker_notice = None;
        }
    }
}

fn is_transient_spawn_failure(detail: &str) -> bool {
    let detail_lower = detail.to_ascii_lowercase();
    detail.contains("worker 応答 timeout")
        || detail.contains("worker stdout が EOF")
        || detail.contains("worker stdout reader thread が終了している")
        || detail.contains("通信失敗")
        || detail_lower.contains("0x8007045a")
        || detail.contains("DLL 初期化")
}

#[cfg(test)]
mod tests {
    use super::is_transient_spawn_failure;

    #[test]
    fn classifies_transient_spawn_failures() {
        assert!(is_transient_spawn_failure(
            "ワーカー起動 timeout / 通信失敗: worker 応答 timeout (45 秒)"
        ));
        assert!(is_transient_spawn_failure(
            "provider preload failed: HRESULT(0x8007045A)"
        ));
        assert!(is_transient_spawn_failure(
            "provider preload failed: HRESULT(0x8007045a)"
        ));
    }

    #[test]
    fn leaves_non_transient_spawn_failures_for_user_notice() {
        assert!(!is_transient_spawn_failure(
            "TensorRT pack が見つかりません: tensorrt_pack"
        ));
        assert!(!is_transient_spawn_failure(
            "engine metadata mismatch: expected fp16"
        ));
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

//! TensorRT エンジンビルド進捗ダイアログ。
//!
//! ユーザーが環境設定 → AI バックエンドページで「全エンジンビルド」を押すと、
//! 全 ModelKind について順次 `tensorrt_builder::build_engine_for` を子プロセスで
//! 実行し、進捗をモーダルダイアログに表示する。
//!
//! フロー:
//!   1. App::start_trt_build() でワーカースレッドを spawn、`trt_build_state` をセット
//!   2. App::update() が毎フレーム show_trt_build_dialog() を呼ぶ
//!   3. mpsc 受信ですべてのイベントをポーリング、ダイアログを更新
//!   4. 完了/エラー時はユーザーが閉じるまで結果を表示
//!
//! キャンセルは `Arc<AtomicBool>` をワーカーに伝えて、次のモデル境界で停止。
//! 走行中の子プロセスは親で kill しないので最大 1 モデル分の待ちが発生する
//! (~30 秒〜数分) が、UI スレッドはブロックされない。

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use eframe::egui;

use crate::ai::ModelKind;
use crate::ai::tensorrt_builder::{TrtBuildEvent, build_engine_for};
use crate::app::App;

/// 1 モデルのビルド状態。
#[derive(Debug, Clone)]
pub(crate) enum TrtModelStatus {
    /// 未着手
    Pending,
    /// 子プロセスがモデルをロード中
    Loading,
    /// 子プロセスが engine compile 中 (一番長いフェーズ)
    Compiling,
    /// 完了 (所要時間 ms)
    Done(u64),
    /// 失敗
    Error(String),
}

/// ダイアログ全体のフェーズ。
#[derive(Debug)]
pub(crate) enum TrtBuildPhase {
    /// ビルド進行中
    Building,
    /// 全モデル成功
    AllDone { total_elapsed: Duration },
    /// 途中で失敗 (失敗したモデル以降は中断)
    Aborted {
        failed_kind: ModelKind,
        message: String,
    },
    /// ユーザーキャンセル
    Cancelled,
}

/// ワーカー → UI のメッセージ。
#[derive(Debug)]
pub(crate) enum TrtBuildMsg {
    /// モデル `i` のビルドを開始
    KindStart(usize),
    /// 子プロセスから受信したイベント
    KindEvent(usize, TrtBuildEvent),
    /// モデル `i` のビルドが終了 (Ok=elapsed_ms, Err=message)
    KindDone(usize, Result<u64, String>),
    /// 全モデル走破 (成功・失敗・キャンセル問わずワーカー終了)
    WorkerExited,
}

pub(crate) struct TrtBuildState {
    pub phase: TrtBuildPhase,
    pub rx: mpsc::Receiver<TrtBuildMsg>,
    pub cancel: Arc<AtomicBool>,
    pub start_time: Instant,
    pub kinds: Vec<ModelKind>,
    pub statuses: Vec<TrtModelStatus>,
    pub current_idx: usize,
}

/// 全 8 ModelKind を返す (順序はビルドされる順)。
fn all_model_kinds() -> Vec<ModelKind> {
    vec![
        ModelKind::ClassifierMobileNet,
        ModelKind::UpscaleRealEsrganX4Plus,
        ModelKind::UpscaleRealEsrganAnime6B,
        ModelKind::UpscaleRealEsrGeneralV3,
        ModelKind::UpscaleRealCugan4x,
        ModelKind::UpscaleNmkdSiax4x,
        ModelKind::DenoiseRealplksr,
        ModelKind::InpaintMiGan,
    ]
}

impl App {
    /// TRT 全エンジンのビルドを開始する (preferences の「全エンジンビルド」ボタンから呼ばれる)。
    /// 既にビルド中なら何もしない。
    pub(crate) fn start_trt_build(&mut self) {
        if self.trt_build_state.is_some() {
            return;
        }
        let kinds = all_model_kinds();
        let cancel = Arc::new(AtomicBool::new(false));
        let (tx, rx) = mpsc::channel();
        let kinds_for_worker = kinds.clone();
        let cancel_for_worker = cancel.clone();

        thread::spawn(move || {
            for (i, &kind) in kinds_for_worker.iter().enumerate() {
                if cancel_for_worker.load(Ordering::Relaxed) {
                    break;
                }
                let _ = tx.send(TrtBuildMsg::KindStart(i));
                let tx_for_event = tx.clone();
                let r = build_engine_for(kind, |ev| {
                    let _ = tx_for_event.send(TrtBuildMsg::KindEvent(i, ev.clone()));
                });
                let send_result = tx.send(TrtBuildMsg::KindDone(i, r.clone()));
                if send_result.is_err() {
                    break;
                }
                if r.is_err() {
                    break; // 失敗したら以降のモデルは打ち切り
                }
            }
            let _ = tx.send(TrtBuildMsg::WorkerExited);
        });

        let n = kinds.len();
        self.trt_build_state = Some(TrtBuildState {
            phase: TrtBuildPhase::Building,
            rx,
            cancel,
            start_time: Instant::now(),
            kinds,
            statuses: vec![TrtModelStatus::Pending; n],
            current_idx: 0,
        });
        crate::logger::log("[TRT-build] スタート (全モデル一括ビルド)".to_string());
    }

    /// TRT ビルドダイアログを描画する (毎フレーム呼ばれる)。
    /// `trt_build_state` が None のときは何もしない。
    pub(crate) fn show_trt_build_dialog(&mut self, ctx: &egui::Context) {
        let Some(state) = self.trt_build_state.as_mut() else {
            return;
        };

        // メッセージをポーリング
        while let Ok(msg) = state.rx.try_recv() {
            match msg {
                TrtBuildMsg::KindStart(i) => {
                    state.current_idx = i;
                    state.statuses[i] = TrtModelStatus::Loading;
                }
                TrtBuildMsg::KindEvent(i, ev) => match ev {
                    TrtBuildEvent::Loading { .. } => {
                        state.statuses[i] = TrtModelStatus::Loading;
                    }
                    TrtBuildEvent::Compiling { .. } => {
                        state.statuses[i] = TrtModelStatus::Compiling;
                    }
                    TrtBuildEvent::Done { .. } | TrtBuildEvent::Error { .. } => {
                        // KindDone で確定するのでここでは何もしない
                    }
                },
                TrtBuildMsg::KindDone(i, Ok(ms)) => {
                    state.statuses[i] = TrtModelStatus::Done(ms);
                }
                TrtBuildMsg::KindDone(i, Err(msg)) => {
                    state.statuses[i] = TrtModelStatus::Error(msg.clone());
                    state.phase = TrtBuildPhase::Aborted {
                        failed_kind: state.kinds[i],
                        message: msg,
                    };
                }
                TrtBuildMsg::WorkerExited => {
                    // 全モデル走破。phase が Building のままなら、それは成功 or キャンセル。
                    state.phase = match state.phase {
                        TrtBuildPhase::Building => {
                            if state.cancel.load(Ordering::Relaxed) {
                                TrtBuildPhase::Cancelled
                            } else if state
                                .statuses
                                .iter()
                                .all(|s| matches!(s, TrtModelStatus::Done(_)))
                            {
                                TrtBuildPhase::AllDone {
                                    total_elapsed: state.start_time.elapsed(),
                                }
                            } else {
                                // KindDone(Err) で Aborted を設定済み、ここに来るパスはほぼ無い。
                                // 念のため AllDone とする (phase は KindDone 時点で更新されている前提)
                                TrtBuildPhase::AllDone {
                                    total_elapsed: state.start_time.elapsed(),
                                }
                            }
                        }
                        // Aborted / Cancelled / AllDone はそのまま維持
                        _ => std::mem::replace(&mut state.phase, TrtBuildPhase::Building),
                    };
                    crate::logger::log(format!(
                        "[TRT-build] ワーカー終了 (phase = {:?})",
                        state.phase
                    ));
                }
            }
        }

        // 進行中は毎フレーム再描画 (進捗表示)
        if matches!(state.phase, TrtBuildPhase::Building) {
            ctx.request_repaint_after(std::time::Duration::from_millis(200));
        }

        // 描画
        let dialog_pos = ctx.content_rect().min + egui::vec2(80.0, 60.0);
        let mut should_close = false;
        let phase_is_terminal = !matches!(state.phase, TrtBuildPhase::Building);

        egui::Window::new("TensorRT エンジンビルド")
            .default_pos(dialog_pos)
            .default_size([520.0, 400.0])
            .resizable(false)
            .collapsible(false)
            .show(ctx, |ui| {
                ui.label(egui::RichText::new("TensorRT エンジンを準備しています").strong());
                ui.add_space(4.0);
                ui.label(
                    "各モデルを GPU 専用に最適化したエンジンとしてコンパイルし、\n\
                     %APPDATA%/mimageviewer/tensorrt-engines/ にキャッシュします。\n\
                     初回のみ各モデル 30 秒〜数分かかります (キャッシュ後は瞬時)。",
                );
                ui.add_space(8.0);

                // 全体進捗
                let done = state
                    .statuses
                    .iter()
                    .filter(|s| matches!(s, TrtModelStatus::Done(_)))
                    .count();
                let elapsed = state.start_time.elapsed();
                ui.label(format!(
                    "進捗: {} / {} モデル完了  経過時間: {:.0} 秒",
                    done,
                    state.kinds.len(),
                    elapsed.as_secs_f64()
                ));
                let progress = done as f32 / state.kinds.len() as f32;
                ui.add(egui::ProgressBar::new(progress).show_percentage());
                ui.add_space(8.0);

                // モデル別ステータス
                egui::ScrollArea::vertical()
                    .id_salt("trt_build_status_list")
                    .max_height(200.0)
                    .show(ui, |ui| {
                        for (kind, status) in state.kinds.iter().zip(state.statuses.iter()) {
                            ui.horizontal(|ui| {
                                let (icon, color, text) = match status {
                                    TrtModelStatus::Pending => (
                                        "⏸",
                                        egui::Color32::GRAY,
                                        "待機中".to_string(),
                                    ),
                                    TrtModelStatus::Loading => (
                                        "📂",
                                        egui::Color32::from_rgb(100, 150, 220),
                                        "ロード中".to_string(),
                                    ),
                                    TrtModelStatus::Compiling => (
                                        "⚙",
                                        egui::Color32::from_rgb(220, 160, 50),
                                        "コンパイル中...".to_string(),
                                    ),
                                    TrtModelStatus::Done(ms) => (
                                        "✓",
                                        egui::Color32::from_rgb(100, 200, 100),
                                        format!("完了 ({:.1} 秒)", *ms as f64 / 1000.0),
                                    ),
                                    TrtModelStatus::Error(msg) => {
                                        // 長いエラーメッセージは省略 (詳細は phase ブロックに別途表示)。
                                        // msg は UTF-8 で日本語が混じる可能性があるので char 単位で切る。
                                        let short = if msg.chars().count() > 60 {
                                            let truncated: String = msg.chars().take(60).collect();
                                            format!("失敗: {truncated}…")
                                        } else {
                                            format!("失敗: {msg}")
                                        };
                                        (
                                            "❌",
                                            egui::Color32::from_rgb(220, 100, 100),
                                            short,
                                        )
                                    }
                                };
                                ui.colored_label(color, icon);
                                ui.label(format!("{}: ", kind.display_label()));
                                ui.colored_label(color, text);
                            });
                        }
                    });
                ui.add_space(8.0);

                // フェーズ別メッセージ
                match &state.phase {
                    TrtBuildPhase::Building => {
                        ui.label(
                            egui::RichText::new(
                                "中断する場合は「キャンセル」を押してください。\n\
                                 (現在ビルド中のモデルは完了するまで待機します)",
                            )
                            .small(),
                        );
                    }
                    TrtBuildPhase::AllDone { total_elapsed } => {
                        ui.colored_label(
                            egui::Color32::from_rgb(100, 200, 100),
                            format!(
                                "✓ 全エンジンのビルドが完了しました ({:.1} 秒)",
                                total_elapsed.as_secs_f64()
                            ),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            "「TensorRT を有効にする」が設定済みであれば、\n\
                             アプリを再起動すると TensorRT で動作します。",
                        );
                    }
                    TrtBuildPhase::Aborted { failed_kind, message } => {
                        ui.colored_label(
                            egui::Color32::from_rgb(220, 100, 100),
                            format!("❌ {} のビルドに失敗", failed_kind.display_label()),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(format!("エラー: {}", message)).small(),
                        );
                        ui.add_space(4.0);
                        ui.label(
                            egui::RichText::new(
                                "TensorRT パックが破損している可能性があります。\n\
                                 setup-tensorrt-pack.ps1 を再実行するか、\n\
                                 環境設定からパックを削除してください。",
                            )
                            .small(),
                        );
                    }
                    TrtBuildPhase::Cancelled => {
                        ui.colored_label(
                            egui::Color32::GRAY,
                            "ユーザーによりキャンセルされました",
                        );
                    }
                }
                ui.add_space(8.0);

                // ボタン
                ui.horizontal(|ui| {
                    if !phase_is_terminal {
                        if ui.button("キャンセル").clicked() {
                            state.cancel.store(true, Ordering::Relaxed);
                            crate::logger::log(
                                "[TRT-build] ユーザーキャンセル要求 (走行中モデルの完了を待つ)"
                                    .to_string(),
                            );
                        }
                    } else if ui.button("閉じる").clicked() {
                        should_close = true;
                    }
                });
            });

        if should_close {
            self.trt_build_state = None;
        }
    }
}

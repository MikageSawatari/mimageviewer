//! v0.7.0 データ保存場所変更機能の進捗ダイアログ。
//!
//! 起動時に `settings.pending_move` が設定されている場合、通常 UI の代わりに
//! このダイアログだけを描画する。移動が完了したら結果メッセージを表示し、
//! 「終了」ボタンでアプリを閉じる (再起動は手動)。

use std::sync::atomic::Ordering;

use eframe::egui;

use crate::app::App;

impl App {
    pub(crate) fn show_data_move_progress_window(&mut self, ctx: &egui::Context) {
        let Some(state) = self.data_move_state.clone() else {
            return;
        };

        // 完了直後に結果メッセージを一度だけ整形する
        if state.is_finished() && self.data_move_result_message.is_none() {
            let res = state.result.lock().unwrap().clone();
            self.data_move_result_message = Some(match res {
                Some(Ok(())) => format!(
                    "データを新しい場所に移動しました。\n\n移動先: {}\n\n「終了」でアプリを閉じ、もう一度起動してください。",
                    state.to.display()
                ),
                Some(Err(msg)) => format!(
                    "データ移動に失敗しました:\n\n{msg}\n\nデータは元の場所のままです。「終了」でアプリを閉じてください。"
                ),
                None => "データ移動が予期せず終了しました。".to_string(),
            });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            let avail = ui.available_size();
            let dialog_w = avail.x.min(560.0).max(400.0);
            let x_off = ((avail.x - dialog_w) * 0.5).max(0.0);
            let y_off = (avail.y * 0.2).max(20.0);
            ui.add_space(y_off);
            ui.horizontal(|ui| {
                ui.add_space(x_off);
                ui.allocate_ui_with_layout(
                    egui::vec2(dialog_w, avail.y - y_off - 20.0),
                    egui::Layout::top_down(egui::Align::Min),
                    |ui| {
                        ui.heading("データ保存場所の移動");
                        ui.add_space(8.0);
                        ui.separator();
                        ui.add_space(8.0);

                        ui.label(format!("移動元: {}", state.from.display()));
                        ui.label(format!("移動先: {}", state.to.display()));
                        ui.add_space(8.0);

                        let total_bytes = state.total_bytes.load(Ordering::Relaxed);
                        let bytes_done = state.bytes_done.load(Ordering::Relaxed);
                        let total_files = state.total_files.load(Ordering::Relaxed);
                        let files_done = state.files_done.load(Ordering::Relaxed);
                        let current = state.current.lock().unwrap().clone();

                        if !state.is_finished() {
                            let ratio = if total_bytes > 0 {
                                (bytes_done as f32 / total_bytes as f32).clamp(0.0, 1.0)
                            } else {
                                0.0
                            };
                            ui.add(
                                egui::ProgressBar::new(ratio)
                                    .show_percentage()
                                    .desired_width(dialog_w - 40.0),
                            );
                            ui.add_space(6.0);
                            ui.label(format!(
                                "{}/{} ファイル ({}/{})",
                                files_done,
                                total_files,
                                format_bytes(bytes_done),
                                format_bytes(total_bytes),
                            ));
                            if !current.is_empty() {
                                ui.label(
                                    egui::RichText::new(format!("コピー中: {current}"))
                                        .weak(),
                                );
                            }
                            ui.add_space(12.0);
                            ui.separator();
                            ui.add_space(8.0);
                            if ui.button("キャンセル").clicked() {
                                state.request_cancel();
                            }
                        } else {
                            if let Some(msg) = &self.data_move_result_message {
                                ui.label(msg);
                            }
                            ui.add_space(12.0);
                            ui.separator();
                            ui.add_space(8.0);
                            if ui.button("終了").clicked() {
                                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                            }
                        }
                    },
                );
            });
        });
    }
}

fn format_bytes(n: u64) -> String {
    const KB: u64 = 1024;
    const MB: u64 = 1024 * 1024;
    const GB: u64 = 1024 * 1024 * 1024;
    if n >= GB {
        format!("{:.2} GB", n as f64 / GB as f64)
    } else if n >= MB {
        format!("{:.1} MB", n as f64 / MB as f64)
    } else if n >= KB {
        format!("{:.0} KB", n as f64 / KB as f64)
    } else {
        format!("{n} B")
    }
}

//! 明示操作の孤児メタデータ整理ダイアログ。

use std::collections::HashSet;
use std::sync::atomic::Ordering;

use eframe::egui;

use crate::app::App;
use crate::metadata_cleanup::{CleanupTask, WorkerResult};

impl App {
    pub(crate) fn poll_metadata_cleanup(&mut self, ctx: &egui::Context) {
        let message = self.metadata_cleanup_pending.as_ref().and_then(|pending| {
            match pending.rx.try_recv() {
                Ok(message) => Some(message),
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
                Err(std::sync::mpsc::TryRecvError::Disconnected) => Some(match pending.task {
                    CleanupTask::Scan => WorkerResult::Scan(crate::metadata_cleanup::ScanReport {
                        errors: vec!["整理スキャンが予期せず終了しました".into()],
                        ..Default::default()
                    }),
                    CleanupTask::Delete => {
                        WorkerResult::Delete(crate::metadata_cleanup::DeleteReport {
                            errors: vec!["整理処理が予期せず終了しました".into()],
                            store_mutations:
                                crate::rename_key_migration::StoreMutationEffects::
                                    for_content_identity_index_stale(),
                            ..Default::default()
                        })
                    }
                }),
            }
        });
        let Some(message) = message else {
            if self.metadata_cleanup_pending.is_some() {
                ctx.request_repaint_after(std::time::Duration::from_millis(50));
            }
            return;
        };
        self.metadata_cleanup_pending = None;
        match message {
            WorkerResult::Scan(report) => {
                self.metadata_cleanup_result = None;
                self.metadata_cleanup_scan = Some(report);
            }
            WorkerResult::ScanCanceled => {
                self.metadata_cleanup_scan = None;
                self.show_feedback_toast("メタデータ整理のスキャンを中止しました".into());
            }
            WorkerResult::Delete(report) => {
                self.apply_metadata_cleanup_result(&report);
                self.metadata_cleanup_scan = None;
                self.metadata_cleanup_result = Some(report);
                self.try_start_next_rename_migration();
            }
        }
    }

    pub(crate) fn apply_metadata_cleanup_result(
        &mut self,
        report: &crate::metadata_cleanup::DeleteReport,
    ) {
        self.apply_content_identity_store_mutations(report.store_mutations);
        if report.deleted_keys.is_empty() {
            return;
        }
        let deleted = report
            .deleted_keys
            .iter()
            .map(String::as_str)
            .collect::<HashSet<_>>();
        self.rating_cache.clear();
        self.current_folder_rating_cache = None;
        self.invalidate_rating_counts_cache();
        self.folder_rating_counts.clear();
        self.folder_rating_counts_loaded = false;
        self.tags_cache
            .retain(|key, _| !deleted.contains(key.as_str()));
        self.rating_session_writes
            .retain(|key, _| !deleted.contains(key.as_str()));
        self.folder_pin_map
            .retain(|key, _| !deleted.contains(key.as_str()));
        self.rating_view_rows
            .retain(|row| !deleted.contains(row.key.as_str()));
        for keys in [
            &mut self.adjusted_page_keys,
            &mut self.local_adjust_page_keys,
            &mut self.mask_page_keys,
            &mut self.conceal_page_keys,
            &mut self.comic_page_keys,
            &mut self.rotation_page_keys,
        ] {
            keys.retain(|key| !deleted.contains(key.as_str()));
        }
        self.invalidate_tag_apply_suggestions();
        self.reload_open_metadata_views_after_cleanup();
    }

    fn metadata_cleanup_can_start(&self) -> bool {
        self.metadata_cleanup_pending.is_none()
            && self.delete_pending.is_none()
            && self.delete_purge_retry_pending.is_none()
            && self.rename_pending.is_none()
            && self.rename_migration_in_flight.is_none()
            && self.rename_migration_queue.is_empty()
            && self.rename_migration_boot_retry.is_empty()
            && !self.rename_migration_writers_busy()
    }

    fn start_metadata_cleanup_scan(&mut self) {
        if !self.metadata_cleanup_can_start() {
            self.show_feedback_toast("ファイル操作の完了後にもう一度実行してください".into());
            return;
        }
        self.metadata_cleanup_scan = None;
        self.metadata_cleanup_result = None;
        self.metadata_cleanup_pending = Some(crate::metadata_cleanup::spawn_scan(
            crate::data_dir::get(),
            self.book_root_path(),
        ));
    }

    fn start_metadata_cleanup_delete(&mut self) {
        if !self.metadata_cleanup_can_start() {
            self.show_feedback_toast("メタデータ書き込みの完了後にもう一度実行してください".into());
            return;
        }
        let Some(scan) = self.metadata_cleanup_scan.clone() else {
            return;
        };
        if scan.orphan_total() == 0 {
            return;
        }
        self.metadata_cleanup_result = None;
        self.metadata_cleanup_pending = Some(crate::metadata_cleanup::spawn_delete(
            crate::data_dir::get(),
            self.book_root_path(),
            scan,
        ));
    }

    pub(crate) fn show_metadata_cleanup_dialog(&mut self, ctx: &egui::Context) {
        if !self.show_metadata_cleanup {
            return;
        }
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let mut open = true;
        let mut start_scan = false;
        let mut start_delete = false;
        let mut cancel = false;
        let mut close = false;
        egui::Window::new("メタデータを整理")
            .open(&mut open)
            .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
            .default_width(520.0)
            .resizable(true)
            .show(ctx, |ui| {
                ui.label("存在しないファイルに残った評価・補正・タグなどを整理します。");
                ui.label("親フォルダに到達できない外付けドライブや NAS の行は削除しません。");
                ui.label("スキャン結果を確認してから、整理を実行できます。");
                ui.add_space(8.0);
                if let Some(pending) = self.metadata_cleanup_pending.as_ref() {
                    let (processed, total, _, deleting) = pending.progress.snapshot();
                    ui.heading(if deleting {
                        "整理中…"
                    } else {
                        "スキャン中…"
                    });
                    ui.label(format!("{processed} / {total} 行"));
                    let ratio = if total == 0 {
                        0.0
                    } else {
                        (processed as f32 / total as f32).clamp(0.0, 1.0)
                    };
                    ui.add(egui::ProgressBar::new(ratio).show_percentage());
                    if pending.cancel.load(Ordering::Relaxed) {
                        ui.label("キャンセル中…");
                    } else if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                    return;
                }

                if let Some(report) = self.metadata_cleanup_result.as_ref() {
                    if report.canceled {
                        ui.heading("整理を中止しました");
                    } else {
                        ui.heading(format!("{} 行を整理しました", report.deleted_total()));
                    }
                    for row in &report.deleted_by_store {
                        ui.label(format!("{}: {}", row.store, row.rows));
                    }
                    if !report.protected_after_scan.is_empty() {
                        ui.label("確認後に状態が変わった行は保護して残しました。");
                    }
                    for error in &report.errors {
                        ui.colored_label(egui::Color32::LIGHT_RED, error);
                    }
                    ui.add_space(8.0);
                    if ui.button("閉じる").clicked() {
                        close = true;
                    }
                    return;
                }

                if let Some(report) = self.metadata_cleanup_scan.as_ref() {
                    let total = report.orphan_total();
                    if total == 0 {
                        ui.heading("整理対象はありません");
                    } else {
                        ui.heading(format!("整理対象: 合計 {total} 行"));
                        for row in &report.orphan_by_store {
                            ui.label(format!("{}: {}", row.store, row.rows));
                        }
                    }
                    if report.protected_total() > 0 {
                        ui.add_space(6.0);
                        ui.label(format!(
                            "オフライン保護で残す行: {}",
                            report.protected_total()
                        ));
                    }
                    if !report.excluded.is_empty() {
                        ui.collapsing("整理対象外のストアと理由", |ui| {
                            for entry in &report.excluded {
                                ui.label(format!(
                                    "{} ({} 行): {}",
                                    entry.store, entry.rows, entry.reason
                                ));
                            }
                        });
                    }
                    for error in &report.errors {
                        ui.colored_label(egui::Color32::LIGHT_RED, error);
                    }
                    ui.add_space(8.0);
                    ui.horizontal(|ui| {
                        if ui
                            .add_enabled(total > 0, egui::Button::new("整理する"))
                            .clicked()
                        {
                            start_delete = true;
                        }
                        if ui.button("再スキャン").clicked() {
                            start_scan = true;
                        }
                        if ui.button("閉じる").clicked() {
                            close = true;
                        }
                    });
                } else if ui.button("スキャンする").clicked() {
                    start_scan = true;
                }
            });

        if cancel {
            if let Some(pending) = self.metadata_cleanup_pending.as_ref() {
                pending.cancel();
            }
        }
        if start_scan {
            self.start_metadata_cleanup_scan();
        }
        if start_delete {
            self.start_metadata_cleanup_delete();
        }
        if close || !open || escape_pressed {
            if let Some(pending) = self.metadata_cleanup_pending.as_ref() {
                pending.cancel();
            }
            self.show_metadata_cleanup = false;
        }
    }
}

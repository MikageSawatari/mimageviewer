//! App が削除の全 phase を所有し、SQLite の処理は短命 worker で行う。
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use eframe::egui;

use crate::app::App;
use crate::feedback::FeedbackKind;
use crate::transcript_db::{TranscriptDb, TranscriptDeletionCandidates};

pub(crate) enum TranscriptDeleteState {
    Counting {
        rx: Receiver<Result<TranscriptDeletionCandidates, String>>,
    },
    Confirm {
        keys: Vec<String>,
        ok: usize,
        failed: usize,
    },
    Deleting {
        rx: Receiver<Result<usize, String>>,
    },
}

fn count_at(path: &Path, keys: &[String]) -> rusqlite::Result<TranscriptDeletionCandidates> {
    match TranscriptDb::open_read_only(path)? {
        Some(db) => db.count_existing(keys),
        None => Ok(TranscriptDeletionCandidates::default()),
    }
}

fn spawn<T: Send + 'static>(
    ctx: &egui::Context,
    work: impl FnOnce() -> rusqlite::Result<T> + Send + 'static,
) -> Result<Receiver<Result<T, String>>, String> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("transcript-delete".into())
        .spawn(move || {
            let result = work().map_err(|error| error.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        })
        .map_err(|error| error.to_string())?;
    // UI は JoinHandle を持たず、終了時に transaction を待たない。
    Ok(rx)
}

impl App {
    fn transcript_deletion_operation_disabled_reason(&self) -> Option<&'static str> {
        if self.remote_session_blocks_local_control() {
            return Some("リモート閲覧中は文字起こしを削除できません");
        }
        if self.asr_setup.blocks_transcript_deletion() {
            return Some(
                "文字起こしのモデルの準備中や、モデルの窓を開いている間は削除できません。準備が終わり窓を閉じてから選んでください",
            );
        }
        if self.main_chrome_pointer_blocked() {
            return Some("ほかの操作を終えてから文字起こしを削除してください");
        }
        if self.transcript_job.is_some() {
            return Some("文字起こしの作成中は削除できません。作成が終わってから選んでください");
        }
        None
    }

    pub(crate) fn transcript_deletion_disabled_reason(
        &self,
        background: bool,
    ) -> Option<&'static str> {
        // 全 phase が modal なので、一般的な modal 判定より削除を先に示す。
        if self.remote_session_blocks_local_control() {
            return Some("リモート閲覧中は文字起こしを削除できません");
        }
        if self.transcript_delete.is_some() {
            return Some("文字起こしの削除を処理しています");
        }
        if let Some(reason) = self.transcript_deletion_operation_disabled_reason() {
            return Some(reason);
        }
        if self.transcript_targets(background).paths.is_empty() {
            return Some(crate::context_menu_model::TRANSCRIPT_NO_TARGET_REASON);
        }
        None
    }

    pub(crate) fn request_delete_transcripts(&mut self, ctx: &egui::Context, background: bool) {
        if let Some(reason) = self.transcript_deletion_disabled_reason(background) {
            self.show_feedback_result(reason.into(), FeedbackKind::Info);
            return;
        }
        let keys = self
            .transcript_targets(background)
            .paths
            .iter()
            .map(|target| TranscriptDb::normalize_path(&target.path))
            .collect::<Vec<_>>();
        let path = TranscriptDb::db_path();
        match spawn(ctx, move || count_at(&path, &keys)) {
            Ok(rx) => self.transcript_delete = Some(TranscriptDeleteState::Counting { rx }),
            Err(error) => self.transcript_deletion_failed(error),
        }
    }

    pub(crate) fn poll_transcript_delete(&mut self) {
        let Some(state) = self.transcript_delete.take() else {
            return;
        };
        match state {
            TranscriptDeleteState::Counting { rx } => match rx.try_recv() {
                Ok(Ok(candidates)) if candidates.keys.is_empty() => {
                    self.show_feedback_result(
                        "選んだ項目に 文字起こしはありません".into(),
                        FeedbackKind::Info,
                    );
                }
                Ok(Ok(TranscriptDeletionCandidates { keys, ok, failed })) => {
                    self.transcript_delete =
                        Some(TranscriptDeleteState::Confirm { keys, ok, failed });
                }
                Ok(Err(error)) => self.transcript_deletion_failed(error),
                Err(TryRecvError::Empty) => {
                    self.transcript_delete = Some(TranscriptDeleteState::Counting { rx })
                }
                Err(TryRecvError::Disconnected) => {
                    self.transcript_deletion_failed("count worker disconnected".into())
                }
            },
            TranscriptDeleteState::Deleting { rx } => match rx.try_recv() {
                Ok(Ok(deleted)) => {
                    let message = if deleted > 0 {
                        self.record_transcript_store_change();
                        format!("文字起こしを {deleted} 件削除しました")
                    } else {
                        "削除する文字起こしはありませんでした".into()
                    };
                    self.show_feedback_result(message, FeedbackKind::Info);
                }
                Ok(Err(error)) => self.transcript_deletion_failed(error),
                Err(TryRecvError::Empty) => {
                    self.transcript_delete = Some(TranscriptDeleteState::Deleting { rx })
                }
                Err(TryRecvError::Disconnected) => {
                    self.transcript_deletion_failed("delete worker disconnected".into())
                }
            },
            confirm @ TranscriptDeleteState::Confirm { .. } => {
                self.transcript_delete = Some(confirm)
            }
        }
    }

    fn transcript_deletion_failed(&mut self, error: String) {
        crate::logger::log(format!("[ASR] transcript deletion: {error}"));
        self.show_feedback_result(
            "文字起こしを削除できませんでした".into(),
            FeedbackKind::Error,
        );
    }

    pub(crate) fn transcript_delete_dialog_visible(&self) -> bool {
        self.transcript_delete.is_some()
    }

    fn confirm_transcript_deletion(&mut self, ctx: &egui::Context) {
        if !matches!(
            self.transcript_delete,
            Some(TranscriptDeleteState::Confirm { .. })
        ) {
            return;
        }
        let Some(TranscriptDeleteState::Confirm { keys, .. }) = self.transcript_delete.take()
        else {
            unreachable!()
        };
        // 自身の modal を外してから共通 gate を再確認。対象は捕捉済み key のまま。
        if let Some(reason) = self.transcript_deletion_operation_disabled_reason() {
            self.show_feedback_result(reason.into(), FeedbackKind::Info);
            return;
        }
        if !self.admit_book_append_followup_mutation() {
            return;
        }
        let path = TranscriptDb::db_path();
        match spawn(ctx, move || {
            TranscriptDb::open_existing_for_write(path)?.delete_keys(&keys)
        }) {
            Ok(rx) => self.transcript_delete = Some(TranscriptDeleteState::Deleting { rx }),
            Err(error) => self.transcript_deletion_failed(error),
        }
    }

    pub(crate) fn show_transcript_delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(state) = &self.transcript_delete else {
            return;
        };
        let escape = !matches!(state, TranscriptDeleteState::Deleting { .. })
            && self.dialog_escape_pressed(ctx);
        match draw_transcript_delete_dialog(ctx, state, escape) {
            ConfirmationAction::Delete => self.confirm_transcript_deletion(ctx),
            ConfirmationAction::Cancel => self.transcript_delete = None,
            ConfirmationAction::None => {}
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ConfirmationAction {
    None,
    Delete,
    Cancel,
}

fn draw_transcript_delete_dialog(
    ctx: &egui::Context,
    state: &TranscriptDeleteState,
    escape: bool,
) -> ConfirmationAction {
    let mut action = ConfirmationAction::None;
    let mut open = true;
    let window = egui::Window::new("文字起こしの削除")
        .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
        .default_width(380.0_f32.min((ctx.content_rect().width() - 100.0).max(180.0)))
        .collapsible(false)
        .resizable(false);
    let window = if matches!(state, TranscriptDeleteState::Deleting { .. }) {
        window
    } else {
        window.open(&mut open)
    };
    window.show(ctx, |ui| match state {
        TranscriptDeleteState::Counting { .. } => {
            ui.label("文字起こしを数えています…");
            ui.add_space(8.0);
            if ui.button("キャンセル").clicked() || escape {
                action = ConfirmationAction::Cancel;
            }
        }
        TranscriptDeleteState::Confirm { ok, failed, .. } => {
            let detail = if *failed == 0 {
                String::new()
            } else {
                format!(" (うち作成に失敗した記録 {failed} 件)")
            };
            ui.label(format!(
                "文字起こしを {} 件削除します{detail}。この操作は元に戻せません。",
                ok + failed
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                // 不可逆な操作は Enter/Space ではなく pointer で確定する。
                if ui.button("削除").clicked_by(egui::PointerButton::Primary) {
                    action = ConfirmationAction::Delete;
                }
                if ui.button("キャンセル").clicked() || escape {
                    action = ConfirmationAction::Cancel;
                }
            });
        }
        TranscriptDeleteState::Deleting { .. } => {
            ui.label("文字起こしを削除しています…");
        }
    });
    if !open {
        action = ConfirmationAction::Cancel;
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::{NodeT, Queryable};

    #[test]
    fn transcript_delete_all_phases_are_modal_and_block_creation() {
        let mut app = crate::app::setup_app_for_test();
        for phase in 0..3 {
            app.transcript_delete = Some(match phase {
                0 => TranscriptDeleteState::Counting {
                    rx: mpsc::channel().1,
                },
                1 => TranscriptDeleteState::Confirm {
                    keys: vec!["item".into()],
                    ok: 1,
                    failed: 1,
                },
                _ => TranscriptDeleteState::Deleting {
                    rx: mpsc::channel().1,
                },
            });
            assert!(app.common_modal_dialog_open());
            assert!(app.transcript_creation_disabled_reason(false).is_some());
        }
    }

    #[test]
    fn transcript_delete_small_window_confirmation_requires_pointer() {
        for size in [egui::vec2(1093.0, 614.0), egui::vec2(546.5, 307.0)] {
            let mut harness = crate::test_ui::builder()
                .with_context_setup(crate::ui_fonts::configure_fonts)
                .with_size(size)
                .build_ui_state(
                    |ui, action| {
                        let result = draw_transcript_delete_dialog(
                            ui.ctx(),
                            &TranscriptDeleteState::Confirm {
                                keys: vec!["item".into()],
                                ok: 10,
                                failed: 2,
                            },
                            false,
                        );
                        if result != ConfirmationAction::None {
                            *action = result;
                        }
                    },
                    ConfirmationAction::None,
                );
            harness.run_steps(4);
            for button in harness.get_all_by_role(egui::accesskit::Role::Button) {
                assert!(
                    egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(button.rect())
                );
            }
            harness.get_by_label("削除").focus();
            harness.key_press(egui::Key::Enter);
            harness.run_steps(2);
            assert_eq!(*harness.state(), ConfirmationAction::None);
            harness.get_by_label("削除").click();
            harness.run_steps(2);
            assert_eq!(*harness.state(), ConfirmationAction::Delete);
        }
    }

    #[test]
    fn transcript_delete_confirm_snapshot() {
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(560.0, 260.0))
            .build_ui(|ui| {
                crate::os_theme::apply_resolved(ui.ctx(), crate::os_theme::ResolvedTheme::Dark);
                draw_transcript_delete_dialog(
                    ui.ctx(),
                    &TranscriptDeleteState::Confirm {
                        keys: vec!["item".into()],
                        ok: 10,
                        failed: 2,
                    },
                    false,
                );
            });
        harness.run_steps(4);
        harness.snapshot("transcript_delete_confirm_dark");
    }
}

//! One App-owned deletion flow. All SQLite work stays on short-lived workers.
use std::path::Path;
use std::sync::mpsc::{self, Receiver, TryRecvError};

use eframe::egui;

use crate::ai_caption_db::{CaptionDb, CaptionDeletionCandidates};
use crate::app::App;
use crate::feedback::FeedbackKind;

pub(crate) enum CaptionDeleteState {
    Counting {
        rx: Receiver<Result<CaptionDeletionCandidates, String>>,
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

fn count_at(path: &Path, keys: &[String]) -> rusqlite::Result<CaptionDeletionCandidates> {
    match CaptionDb::open_read_only(path)? {
        Some(db) => db.count_existing(keys),
        None => Ok(CaptionDeletionCandidates::default()),
    }
}

fn spawn<T: Send + 'static>(
    ctx: &egui::Context,
    work: impl FnOnce() -> rusqlite::Result<T> + Send + 'static,
) -> Result<Receiver<Result<T, String>>, String> {
    let (tx, rx) = mpsc::channel();
    let ctx = ctx.clone();
    std::thread::Builder::new()
        .name("caption-delete".into())
        .spawn(move || {
            let result = work().map_err(|error| error.to_string());
            let _ = tx.send(result);
            ctx.request_repaint();
        })
        .map_err(|error| error.to_string())?;
    // No JoinHandle is retained: shutdown never waits for the transaction.
    Ok(rx)
}

impl App {
    fn caption_deletion_operation_disabled_reason(&self) -> Option<&'static str> {
        if self.remote_session_blocks_local_control() {
            return Some("リモート閲覧中は AI 説明文を削除できません");
        }
        if self.vlm_setup.blocks_caption_deletion() {
            return Some("AI 説明文のモデルを準備中は削除できません。終わってから選んでください");
        }
        if self.main_chrome_pointer_blocked() {
            return Some("ほかの操作を終えてから AI 説明文を削除してください");
        }
        if self.caption_job.is_some() {
            return Some("AI 説明文の作成中は削除できません。作成が終わってから選んでください");
        }
        None
    }

    pub(crate) fn caption_deletion_disabled_reason(
        &self,
        background: bool,
    ) -> Option<&'static str> {
        // The entire deletion flow is modal, so name it before the
        // generic modal check. Remote still takes precedence.
        if self.remote_session_blocks_local_control() {
            return Some("リモート閲覧中は AI 説明文を削除できません");
        }
        if self.caption_delete.is_some() {
            return Some("AI 説明文の削除を処理しています");
        }
        if let Some(reason) = self.caption_deletion_operation_disabled_reason() {
            return Some(reason);
        }
        if self.caption_targets(background).paths.is_empty() {
            return Some(crate::context_menu_model::CAPTION_DELETE_NO_TARGET_REASON);
        }
        None
    }

    pub(crate) fn request_delete_ai_captions(&mut self, ctx: &egui::Context, background: bool) {
        if let Some(reason) = self.caption_deletion_disabled_reason(background) {
            self.show_feedback_result(reason.into(), FeedbackKind::Info);
            return;
        }
        let keys = self
            .caption_targets(background)
            .paths
            .iter()
            .map(|target| CaptionDb::normalize_path(target.path()))
            .collect::<Vec<_>>();
        let path = CaptionDb::db_path();
        match spawn(ctx, move || count_at(&path, &keys)) {
            Ok(rx) => self.caption_delete = Some(CaptionDeleteState::Counting { rx }),
            Err(error) => self.caption_deletion_failed(error),
        }
    }

    pub(crate) fn poll_caption_delete(&mut self) {
        let Some(state) = self.caption_delete.take() else {
            return;
        };
        match state {
            CaptionDeleteState::Counting { rx } => match rx.try_recv() {
                Ok(Ok(candidates)) if candidates.keys.is_empty() => {
                    self.show_feedback_result(
                        "選んだ項目に AI 説明文はありません".into(),
                        FeedbackKind::Info,
                    );
                }
                Ok(Ok(CaptionDeletionCandidates { keys, ok, failed })) => {
                    self.caption_delete = Some(CaptionDeleteState::Confirm { keys, ok, failed });
                }
                Ok(Err(error)) => self.caption_deletion_failed(error),
                Err(TryRecvError::Empty) => {
                    self.caption_delete = Some(CaptionDeleteState::Counting { rx })
                }
                Err(TryRecvError::Disconnected) => {
                    self.caption_deletion_failed("count worker disconnected".into())
                }
            },
            CaptionDeleteState::Deleting { rx } => match rx.try_recv() {
                Ok(Ok(deleted)) => {
                    let message = if deleted > 0 {
                        self.notify_caption_changed();
                        self.caption_display.clear();
                        format!("AI 説明文を {deleted} 件削除しました")
                    } else {
                        "削除する AI 説明文はありませんでした".into()
                    };
                    self.show_feedback_result(message, FeedbackKind::Info);
                }
                Ok(Err(error)) => self.caption_deletion_failed(error),
                Err(TryRecvError::Empty) => {
                    self.caption_delete = Some(CaptionDeleteState::Deleting { rx })
                }
                Err(TryRecvError::Disconnected) => {
                    self.caption_deletion_failed("delete worker disconnected".into())
                }
            },
            confirm @ CaptionDeleteState::Confirm { .. } => self.caption_delete = Some(confirm),
        }
    }

    fn caption_deletion_failed(&mut self, error: String) {
        crate::logger::log(format!("[VLM] caption deletion: {error}"));
        self.show_feedback_result(
            "AI 説明文を削除できませんでした".into(),
            FeedbackKind::Error,
        );
    }

    pub(crate) fn caption_delete_dialog_visible(&self) -> bool {
        self.caption_delete.is_some()
    }

    fn confirm_caption_deletion(&mut self, ctx: &egui::Context) {
        if !matches!(
            self.caption_delete,
            Some(CaptionDeleteState::Confirm { .. })
        ) {
            return;
        }
        let Some(CaptionDeleteState::Confirm { keys, .. }) = self.caption_delete.take() else {
            unreachable!()
        };
        // Remove our own modal before rechecking the remaining operation gates.
        // The captured keys remain the target throughout the modal flow.
        if let Some(reason) = self.caption_deletion_operation_disabled_reason() {
            self.show_feedback_result(reason.into(), FeedbackKind::Info);
            return;
        }
        if !self.admit_book_append_followup_mutation() {
            return;
        }
        let path = CaptionDb::db_path();
        match spawn(ctx, move || {
            CaptionDb::open_existing_for_write(path)?.delete_keys(&keys)
        }) {
            Ok(rx) => self.caption_delete = Some(CaptionDeleteState::Deleting { rx }),
            Err(error) => self.caption_deletion_failed(error),
        }
    }

    pub(crate) fn show_caption_delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(state) = &self.caption_delete else {
            return;
        };
        let escape = !matches!(state, CaptionDeleteState::Deleting { .. })
            && self.dialog_escape_pressed(ctx);
        match draw_caption_delete_dialog(ctx, state, escape) {
            ConfirmationAction::Delete => self.confirm_caption_deletion(ctx),
            ConfirmationAction::Cancel => self.caption_delete = None,
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

fn draw_caption_delete_dialog(
    ctx: &egui::Context,
    state: &CaptionDeleteState,
    escape: bool,
) -> ConfirmationAction {
    let mut action = ConfirmationAction::None;
    let mut open = true;
    let window = egui::Window::new("AI 説明文の削除")
        .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
        .default_width(380.0_f32.min((ctx.content_rect().width() - 100.0).max(180.0)))
        .collapsible(false)
        .resizable(false);
    let window = if matches!(state, CaptionDeleteState::Deleting { .. }) {
        window
    } else {
        window.open(&mut open)
    };
    window.show(ctx, |ui| match state {
        CaptionDeleteState::Counting { .. } => {
            ui.label("AI 説明文を数えています…");
            ui.add_space(8.0);
            if ui.button("キャンセル").clicked() || escape {
                action = ConfirmationAction::Cancel;
            }
        }
        CaptionDeleteState::Confirm { ok, failed, .. } => {
            let detail = if *failed == 0 {
                String::new()
            } else {
                format!(" (うち作成に失敗した記録 {failed} 件)")
            };
            ui.label(format!(
                "AI 説明文を {} 件削除します{detail}。この操作は元に戻せません。",
                ok + failed
            ));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                // A focused egui button's clicked() also accepts Enter/Space.
                // This irreversible operation requires a pointer click.
                if ui.button("削除").clicked_by(egui::PointerButton::Primary) {
                    action = ConfirmationAction::Delete;
                }
                if ui.button("キャンセル").clicked() || escape {
                    action = ConfirmationAction::Cancel;
                }
            });
        }
        CaptionDeleteState::Deleting { .. } => {
            ui.label("AI 説明文を削除しています…");
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
    use crate::app::{AppTestEnvForTest, setup_app_for_test};
    use crate::grid_item::GridItem;
    use egui_kittest::kittest::Queryable;
    use std::sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    };

    fn fixture() -> AppTestEnvForTest {
        let mut app = setup_app_for_test();
        let folder = app.tmp.path().to_owned();
        app.current_folder = Some(folder.clone());
        app.items = vec![
            GridItem::Image(folder.join("ok.png")),
            GridItem::ZipFile(folder.join("failed.zip")),
        ];
        app.checked = [0, 1].into_iter().collect();
        let db = CaptionDb::open().unwrap();
        drop(db);
        let conn = rusqlite::Connection::open(CaptionDb::db_path()).unwrap();
        for (path, status) in [
            (folder.join("ok.png"), "ok"),
            (folder.join("failed.zip"), "failed"),
        ] {
            conn.execute("INSERT INTO captions (item_key, mtime_ms, size, model_id, prompt_version, lang, caption, visible_text, keywords, cover_ref, status, created_at) VALUES (?1, 0, 0, 'fixture', 1, 'ja', 'caption', '', '[]', '', ?2, 0)", rusqlite::params![CaptionDb::normalize_path(&path), status]).unwrap();
        }
        app
    }

    fn wait(app: &mut App, counting: bool) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(20);
        loop {
            app.poll_caption_delete();
            let busy = if counting {
                matches!(
                    app.caption_delete,
                    Some(CaptionDeleteState::Counting { .. })
                )
            } else {
                matches!(
                    app.caption_delete,
                    Some(CaptionDeleteState::Deleting { .. })
                )
            };
            if !busy {
                break;
            }
            assert!(
                std::time::Instant::now() < deadline,
                "caption deletion worker timeout"
            );
            std::thread::yield_now();
        }
    }

    #[test]
    fn caption_delete_count_confirm_delete_clears_display_and_notifies_epoch() {
        let mut app = fixture();
        let ctx = egui::Context::default();
        let data = app.tmp.path().join("index");
        let epoch = app.caption_epoch_for_test(&data);
        let before = epoch.load(Ordering::Acquire);
        app.caption_display.seed_for_test("cached");
        app.request_delete_ai_captions(&ctx, false);
        assert!(matches!(
            app.caption_delete,
            Some(CaptionDeleteState::Counting { .. })
        ));
        assert!(app.caption_delete_dialog_visible());
        assert!(app.common_modal_dialog_open());
        assert!(!app.grid_open_from_click_allowed());
        wait(&mut app, true);
        let Some(CaptionDeleteState::Confirm { keys, ok, failed }) = &app.caption_delete else {
            panic!("missing confirmation")
        };
        assert_eq!((*ok, *failed, keys.len()), (1, 1, 2));
        assert_eq!(app.modal_dialog_block_reason(), Some("caption_delete"));
        let revision = app.book_append_undo_revision;
        app.confirm_caption_deletion(&ctx);
        assert!(app.book_append_undo_revision > revision);
        assert!(matches!(
            app.caption_delete,
            Some(CaptionDeleteState::Deleting { .. })
        ));
        assert!(app.caption_delete_dialog_visible());
        assert!(app.common_modal_dialog_open());
        assert!(!app.grid_open_from_click_allowed());
        wait(&mut app, false);
        assert!(app.caption_delete.is_none());
        assert!(!app.caption_display.contains_for_test("cached"));
        assert!(epoch.load(Ordering::Acquire) > before);
        assert_eq!(
            app.fs_feedback_toast.as_ref().unwrap().0,
            "AI 説明文を 2 件削除しました"
        );
        let db = CaptionDb::open_read_only(CaptionDb::db_path())
            .unwrap()
            .unwrap();
        assert!(
            db.count_existing(
                &app.caption_targets(false)
                    .paths
                    .iter()
                    .map(|t| CaptionDb::normalize_path(t.path()))
                    .collect::<Vec<_>>()
            )
            .unwrap()
            .keys
            .is_empty()
        );
    }

    #[test]
    fn caption_delete_zero_and_missing_db_skip_confirmation_without_creation() {
        let temp = tempfile::tempdir().unwrap();
        let missing = temp.path().join("absent.db");
        assert_eq!(
            count_at(&missing, &["absent".into()]).unwrap(),
            CaptionDeletionCandidates::default()
        );
        assert!(!missing.exists());
        let mut app = setup_app_for_test();
        app.items = vec![GridItem::Image(app.tmp.path().join("absent.png"))];
        app.selected = Some(0);
        app.request_delete_ai_captions(&egui::Context::default(), false);
        wait(&mut app, true);
        assert!(app.caption_delete.is_none());
        assert_eq!(
            app.fs_feedback_toast.as_ref().unwrap().0,
            "選んだ項目に AI 説明文はありません"
        );
        assert!(!CaptionDb::db_path().exists());
    }

    fn remote(app: &mut App) {
        let handle = crate::remote_ipc::session::SessionHandle::new();
        app.set_remote_session_handle(handle.clone());
        handle.acquire(mimageviewer_ipc::SessionAcquireRequest {
            client_id: "caption-delete-test".into(),
            peer: mimageviewer_ipc::SessionPeerInfo {
                connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                device_name: None,
            },
        });
    }

    #[test]
    fn caption_delete_disabled_reasons_cover_remote_modal_jobs_all_delete_phases_and_empty() {
        let mut app = setup_app_for_test();
        app.items = vec![GridItem::Image("C:/image.png".into())];
        app.selected = Some(0);
        assert_eq!(app.caption_deletion_disabled_reason(false), None);
        app.show_about_dialog = true;
        assert_eq!(
            app.caption_deletion_disabled_reason(false),
            Some("ほかの操作を終えてから AI 説明文を削除してください")
        );
        app.show_about_dialog = false;
        for stopping in [false, true] {
            let mut job = super::super::caption_job::CaptionJob::for_progress_test(Arc::new(
                AtomicBool::new(false),
            ));
            if stopping {
                job.stop();
            }
            app.caption_job = Some(job);
            assert_eq!(
                app.caption_deletion_disabled_reason(false),
                Some("AI 説明文の作成中は削除できません。作成が終わってから選んでください")
            );
        }
        app.caption_job = None;
        for state in [
            CaptionDeleteState::Counting {
                rx: mpsc::channel().1,
            },
            CaptionDeleteState::Confirm {
                keys: vec!["key".into()],
                ok: 1,
                failed: 0,
            },
            CaptionDeleteState::Deleting {
                rx: mpsc::channel().1,
            },
        ] {
            app.caption_delete = Some(state);
            assert_eq!(
                app.caption_deletion_disabled_reason(false),
                Some("AI 説明文の削除を処理しています")
            );
            assert_eq!(
                app.caption_creation_disabled_reason(false),
                Some("AI 説明文の削除を処理しています。終わってから選んでください")
            );
        }
        app.caption_delete = None;
        app.items.clear();
        assert_eq!(
            app.caption_deletion_disabled_reason(false),
            Some(crate::context_menu_model::CAPTION_DELETE_NO_TARGET_REASON)
        );
        remote(&mut app);
        assert_eq!(
            app.caption_deletion_disabled_reason(false),
            Some("リモート閲覧中は AI 説明文を削除できません")
        );
    }

    #[test]
    fn caption_delete_confirm_rechecks_other_owners_without_deleting() {
        for owner in ["modal", "remote", "job"] {
            let mut app = fixture();
            let ctx = egui::Context::default();
            app.request_delete_ai_captions(&ctx, false);
            wait(&mut app, true);
            let keys = match &app.caption_delete {
                Some(CaptionDeleteState::Confirm { keys, .. }) => keys.clone(),
                _ => panic!(),
            };
            match owner {
                "modal" => app.show_about_dialog = true,
                "remote" => remote(&mut app),
                _ => {
                    app.caption_job =
                        Some(super::super::caption_job::CaptionJob::for_progress_test(
                            Arc::new(AtomicBool::new(false)),
                        ))
                }
            }
            app.confirm_caption_deletion(&ctx);
            assert!(app.caption_delete.is_none(), "{owner}");
            assert_eq!(
                CaptionDb::open_read_only(CaptionDb::db_path())
                    .unwrap()
                    .unwrap()
                    .count_existing(&keys)
                    .unwrap()
                    .keys
                    .len(),
                2
            );
            assert!(app.fs_feedback_toast.is_some());
        }
    }

    #[test]
    fn caption_delete_cancel_escape_and_small_window_buttons_keep_rows() {
        for cancel in ["Escape", "キャンセル", "Close window"] {
            let mut app = fixture();
            app.request_delete_ai_captions(&egui::Context::default(), false);
            wait(&mut app, true);
            let mut harness = crate::test_ui::builder()
                .with_context_setup(crate::ui_fonts::configure_fonts)
                .with_size(egui::vec2(1093.0, 614.0))
                .build_ui_state(
                    |ui, app: &mut AppTestEnvForTest| app.show_caption_delete_dialog(ui.ctx()),
                    app,
                );
            harness.run_steps(4);
            let screen = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1093.0, 614.0));
            for label in ["削除", "キャンセル", "Close window"] {
                assert!(screen.contains_rect(harness.get_by_label(label).rect()));
            }
            if cancel == "Escape" {
                harness.key_press(egui::Key::Escape);
            } else {
                harness.get_by_label(cancel).click();
            }
            harness.run_steps(2);
            assert!(harness.state().caption_delete.is_none());
            let db = CaptionDb::open_read_only(CaptionDb::db_path())
                .unwrap()
                .unwrap();
            assert_eq!(
                db.count_existing(
                    &harness
                        .state()
                        .caption_targets(false)
                        .paths
                        .iter()
                        .map(|t| CaptionDb::normalize_path(t.path()))
                        .collect::<Vec<_>>()
                )
                .unwrap()
                .keys
                .len(),
                2
            );
        }
    }

    #[test]
    fn caption_delete_confirmation_ime_preserves_escape_and_only_click_deletes() {
        for key in [egui::Key::Enter, egui::Key::Escape] {
            let mut app = fixture();
            app.request_delete_ai_captions(&egui::Context::default(), false);
            wait(&mut app, true);
            let mut harness = crate::test_ui::builder()
                .with_context_setup(|ctx| {
                    crate::ui_fonts::configure_fonts(ctx);
                    crate::ime_focus::install_ime_input_policy(ctx);
                })
                .with_size(egui::vec2(1093.0, 614.0))
                .build_ui_state(
                    |ui, app: &mut AppTestEnvForTest| app.show_caption_delete_dialog(ui.ctx()),
                    app,
                );
            harness.run_steps(4);
            harness.event(egui::Event::Ime(egui::ImeEvent::Preedit {
                text: "変換中".into(),
                active_range_chars: None,
            }));
            harness.run_steps(1);
            harness.key_press(key);
            harness.run_steps(2);
            assert!(matches!(
                harness.state().caption_delete,
                Some(CaptionDeleteState::Confirm { .. })
            ));
            harness.event(egui::Event::Ime(egui::ImeEvent::Commit("変換".into())));
            harness.run_steps(1);
            harness.key_press(egui::Key::Enter);
            harness.run_steps(2);
            assert!(matches!(
                harness.state().caption_delete,
                Some(CaptionDeleteState::Confirm { .. })
            ));
            harness.get_by_label("削除").click();
            harness.run_steps(2);
            assert!(matches!(
                harness.state().caption_delete,
                Some(CaptionDeleteState::Deleting { .. })
            ));
            wait(harness.state_mut(), false);
            assert!(harness.state().caption_delete.is_none());
        }
    }

    #[test]
    fn caption_delete_counting_is_visible_modal_and_cancel_discards_late_result() {
        for cancel in ["キャンセル", "Escape", "Close window"] {
            let mut app = fixture();
            let keys = app
                .caption_targets(false)
                .paths
                .iter()
                .map(|target| CaptionDb::normalize_path(target.path()))
                .collect::<Vec<_>>();
            let worker_keys = keys.clone();
            let path = CaptionDb::db_path();
            let (tx, rx) = mpsc::channel();
            let (finish, release) = mpsc::channel();
            let (reported, report) = mpsc::channel();
            std::thread::spawn(move || {
                release.recv().unwrap();
                let result = count_at(&path, &worker_keys).map_err(|error| error.to_string());
                assert_eq!(result.as_ref().unwrap().keys.len(), 2);
                reported.send(tx.send(result).is_err()).unwrap();
            });
            app.caption_delete = Some(CaptionDeleteState::Counting { rx });
            assert!(app.common_modal_dialog_open());
            assert!(!app.grid_open_from_click_allowed());
            let mut harness = crate::test_ui::builder()
                .with_context_setup(crate::ui_fonts::configure_fonts)
                .with_size(egui::vec2(1093.0, 614.0))
                .build_ui_state(
                    |ui, app: &mut AppTestEnvForTest| app.show_caption_delete_dialog(ui.ctx()),
                    app,
                );
            harness.run_steps(4);
            harness.get_by_label("AI 説明文を数えています…");
            if cancel == "Escape" {
                harness.key_press(egui::Key::Escape);
            } else {
                harness.get_by_label(cancel).click();
            }
            harness.run_steps(2);
            assert!(harness.state().caption_delete.is_none());
            assert!(!harness.state().common_modal_dialog_open());
            finish.send(()).unwrap();
            assert!(
                report
                    .recv_timeout(std::time::Duration::from_secs(20))
                    .unwrap()
            );
            harness.state_mut().poll_caption_delete();
            harness.run_steps(2);
            assert!(harness.state().caption_delete.is_none());
            assert!(harness.query_by_label("削除").is_none());
            assert_eq!(
                CaptionDb::open_read_only(CaptionDb::db_path())
                    .unwrap()
                    .unwrap()
                    .count_existing(&keys)
                    .unwrap()
                    .keys
                    .len(),
                2
            );
        }
    }

    #[test]
    fn caption_delete_confirmation_enter_never_confirms_including_first_display_and_focus() {
        let mut app = fixture();
        let keys = app
            .caption_targets(false)
            .paths
            .iter()
            .map(|target| CaptionDb::normalize_path(target.path()))
            .collect::<Vec<_>>();
        let (counted, rx) = mpsc::channel();
        app.caption_delete = Some(CaptionDeleteState::Counting { rx });
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(1093.0, 614.0))
            .build_ui_state(
                |ui, app: &mut AppTestEnvForTest| {
                    app.poll_caption_delete();
                    app.show_caption_delete_dialog(ui.ctx());
                },
                app,
            );
        harness.run_steps(4);
        harness.get_by_label("AI 説明文を数えています…");
        // Adopt the count in the same frame as an Enter intended for the old UI.
        counted
            .send(Ok(CaptionDeletionCandidates {
                keys: keys.clone(),
                ok: 1,
                failed: 1,
            }))
            .unwrap();
        harness.key_press(egui::Key::Enter);
        harness.run_steps(4);
        assert!(matches!(
            harness.state().caption_delete,
            Some(CaptionDeleteState::Confirm { .. })
        ));
        harness.get_by_label("削除").focus();
        harness.run_steps(1);
        harness.key_press(egui::Key::Enter);
        harness.run_steps(2);
        assert!(matches!(
            harness.state().caption_delete,
            Some(CaptionDeleteState::Confirm { .. })
        ));
        assert_eq!(
            CaptionDb::open_read_only(CaptionDb::db_path())
                .unwrap()
                .unwrap()
                .count_existing(&keys)
                .unwrap()
                .keys
                .len(),
            2
        );
        harness.key_press(egui::Key::Escape);
        harness.run_steps(2);
        assert!(harness.state().caption_delete.is_none());
    }

    #[test]
    fn caption_delete_deleting_is_visible_modal_and_escape_cannot_close_it() {
        let mut app = setup_app_for_test();
        let (_tx, rx) = mpsc::channel();
        app.caption_delete = Some(CaptionDeleteState::Deleting { rx });
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(1093.0, 614.0))
            .build_ui_state(
                |ui, app: &mut AppTestEnvForTest| app.show_caption_delete_dialog(ui.ctx()),
                app,
            );
        harness.run_steps(4);
        harness.get_by_label("AI 説明文を削除しています…");
        assert!(harness.query_by_label("Close window").is_none());
        assert!(harness.query_by_label("キャンセル").is_none());
        assert!(harness.query_by_label("削除").is_none());
        harness.key_press(egui::Key::Escape);
        harness.run_steps(2);
        assert!(matches!(
            harness.state().caption_delete,
            Some(CaptionDeleteState::Deleting { .. })
        ));
        assert!(harness.state().common_modal_dialog_open());
        assert!(!harness.state().grid_open_from_click_allowed());
    }

    #[test]
    fn caption_delete_failures_disconnects_and_zero_mutation_finish_once() {
        let mut app = setup_app_for_test();
        for counting in [false, true] {
            if counting {
                app.caption_delete = Some(CaptionDeleteState::Counting {
                    rx: mpsc::channel().1,
                });
            } else {
                app.caption_delete = Some(CaptionDeleteState::Deleting {
                    rx: mpsc::channel().1,
                });
            }
            app.poll_caption_delete();
            assert!(app.caption_delete.is_none());
            assert_eq!(
                app.fs_feedback_toast.as_ref().unwrap().0,
                "AI 説明文を削除できませんでした"
            );
        }
        let (tx, rx) = mpsc::channel();
        tx.send(Err("fixture DB failure".into())).unwrap();
        app.caption_delete = Some(CaptionDeleteState::Deleting { rx });
        app.poll_caption_delete();
        assert!(app.caption_delete.is_none());
        app.caption_display.seed_for_test("unchanged");
        let (tx, rx) = mpsc::channel();
        tx.send(Ok(0)).unwrap();
        app.caption_delete = Some(CaptionDeleteState::Deleting { rx });
        app.poll_caption_delete();
        assert!(app.caption_delete.is_none());
        assert!(app.caption_display.contains_for_test("unchanged"));
        assert_eq!(
            app.fs_feedback_toast.as_ref().unwrap().0,
            "削除する AI 説明文はありませんでした"
        );
    }

    #[test]
    fn caption_delete_confirmation_snapshot() {
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(520.0, 250.0))
            .build_ui(|ui| {
                crate::os_theme::apply_resolved(ui.ctx(), crate::os_theme::ResolvedTheme::Dark);
                draw_caption_delete_dialog(
                    ui.ctx(),
                    &CaptionDeleteState::Confirm {
                        keys: Vec::new(),
                        ok: 12,
                        failed: 3,
                    },
                    false,
                );
            });
        harness.run_steps(4);
        harness.snapshot("caption_delete_confirmation_dark");
    }
}

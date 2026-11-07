//! App-owned admission and progress for a single local caption job.
//! Selection is converted to paths without I/O; only the worker touches files.
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use eframe::egui;

use crate::ai::caption_job_worker::{
    CaptionItemOutcome, CaptionTarget, CaptionWorker, CaptionWorkerCommand, CaptionWorkerConfig,
    CaptionWorkerEvent, spawn_caption_worker,
};
use crate::app::book_cover::BookKind;
use crate::app::{ActionSurface, App};
use crate::feedback::FeedbackKind;
use crate::grid_item::GridItem;

const STOPPING_MESSAGE: &str = "前のジョブを終えています。終わってから選び直してください";

/// Captured at the menu boundary and retained as one value through model setup.
/// Unsupported items need no file reads, but remain in the final/progress counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CaptionTargets {
    pub paths: Vec<CaptionTarget>,
    pub unsupported: usize,
}

impl From<Vec<PathBuf>> for CaptionTargets {
    fn from(paths: Vec<PathBuf>) -> Self {
        Self {
            paths: paths.into_iter().map(CaptionTarget::Image).collect(),
            unsupported: 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobState {
    Running,
    Stopping,
}

#[derive(Clone, Debug, Default)]
struct CaptionProgress {
    total: usize,
    done: usize,
    created: usize,
    already_created: usize,
    unsupported: usize,
    missing: usize,
    unconverted: usize,
    password_required: usize,
    video_cover: usize,
    failed: usize,
    discarded: usize,
}

pub(crate) struct CaptionJob {
    state: JobState,
    queue: VecDeque<CaptionTarget>,
    current: Option<CaptionTarget>,
    progress: Arc<CaptionProgress>,
    cancel: Arc<AtomicBool>,
    worker: CaptionWorker,
}

impl CaptionJob {
    #[cfg(test)]
    pub(crate) fn for_progress_test(cancel: Arc<AtomicBool>) -> Self {
        let (worker, _commands, _events) = CaptionWorker::test_channels(cancel.clone());
        Self::new(worker, cancel)
    }

    #[cfg(test)]
    pub(super) fn queued_paths_for_test(&self) -> Vec<PathBuf> {
        self.queue
            .iter()
            .map(|target| target.path().to_owned())
            .collect()
    }

    pub(super) fn new(worker: CaptionWorker, cancel: Arc<AtomicBool>) -> Self {
        Self {
            state: JobState::Running,
            queue: VecDeque::new(),
            current: None,
            progress: Arc::default(),
            cancel,
            worker,
        }
    }

    fn append(&mut self, targets: CaptionTargets) -> bool {
        if self.state == JobState::Stopping {
            return false;
        }
        let progress = Arc::make_mut(&mut self.progress);
        progress.unsupported += targets.unsupported;
        progress.done += targets.unsupported;
        progress.total += targets.unsupported;
        let mut seen: HashSet<_> = self
            .queue
            .iter()
            .map(|target| crate::path_key::normalize_keep_drive(target.path()))
            .collect();
        for target in targets.paths {
            if !seen.insert(crate::path_key::normalize_keep_drive(target.path())) {
                continue;
            }
            self.queue.push_back(target);
            progress.total += 1;
        }
        true
    }

    /// Called only at initial admission and after one item result. No worker
    /// observes the queue or decides when admission ends. Finish is sent once.
    fn advance(&mut self) -> Result<(), String> {
        debug_assert!(self.current.is_none());
        let command = if self.state == JobState::Running {
            if let Some(path) = self.queue.pop_front() {
                self.current = Some(path.clone());
                CaptionWorkerCommand::Item(path)
            } else {
                self.state = JobState::Stopping;
                CaptionWorkerCommand::Finish
            }
        } else {
            CaptionWorkerCommand::Finish
        };
        self.worker
            .commands
            .send(command)
            .map_err(|_| "説明文の作成処理が終了しました".into())
    }

    pub(super) fn stop(&mut self) {
        // Close admission immediately, before the current inference returns.
        self.state = JobState::Stopping;
        self.cancel.store(true, Ordering::Release);
        self.queue.clear();
    }

    fn item_done(
        &mut self,
        outcome: CaptionItemOutcome,
        display: &mut crate::caption_display_cache::CaptionDisplayCache,
    ) -> Result<(), String> {
        // Every outcome rechecks this path. Failed inference may retain an older ok row.
        if let Some(path) = self.current.take() {
            display.invalidate(&crate::ai_caption_db::CaptionDb::normalize_path(
                path.path(),
            ));
        }
        let progress = Arc::make_mut(&mut self.progress);
        progress.done += 1;
        match outcome {
            CaptionItemOutcome::Created => progress.created += 1,
            CaptionItemOutcome::AlreadyCreated => progress.already_created += 1,
            CaptionItemOutcome::Missing => progress.missing += 1,
            CaptionItemOutcome::Unconverted => progress.unconverted += 1,
            CaptionItemOutcome::PasswordRequired => progress.password_required += 1,
            CaptionItemOutcome::VideoCover => {
                progress.unsupported += 1;
                progress.video_cover += 1;
            }
            CaptionItemOutcome::Failed | CaptionItemOutcome::FailedWritten => progress.failed += 1,
            CaptionItemOutcome::Discarded => progress.discarded += 1,
            CaptionItemOutcome::Cancelled => {}
        }
        self.advance()
    }

    fn summary(&self) -> String {
        let p = &self.progress;
        let completion = if self.cancel.load(Ordering::Acquire) {
            "作成を中止しました"
        } else {
            "作成が終わりました"
        };
        let mut summary = format!(
            "AI 説明文の{completion} (作成 {}、既にあり {}、対象外 {} (動画の表紙 {})、ファイルなし {}、未変換 {}、パスワードが必要 {}、読めなかった・失敗 {}、変更により破棄 {})",
            p.created,
            p.already_created,
            p.unsupported,
            p.video_cover,
            p.missing,
            p.unconverted,
            p.password_required,
            p.failed,
            p.discarded,
        );
        if p.unconverted != 0 {
            summary.push_str("。未変換の本は一度開いて変換すると作れます");
        }
        summary
    }
}

fn same_path(a: &Path, b: &Path) -> bool {
    crate::folder_tree::path_eq(a, b)
}

fn collect_caption_targets(
    items: &[GridItem],
    indices: impl IntoIterator<Item = usize>,
    direct_folder: Option<&Path>,
) -> CaptionTargets {
    let mut targets = CaptionTargets::default();
    let mut seen = HashSet::new();
    for index in indices {
        let Some(item) = items.get(index) else {
            continue;
        };
        // A background request never recurses, including flattened result lists.
        if let Some(folder) = direct_folder {
            let path = match item {
                GridItem::Image(path)
                | GridItem::ZipFile(path)
                | GridItem::PdfFile(path)
                | GridItem::ConvertibleArchive { path, .. } => path,
                // Subfolders are not included in a current-folder request.
                _ => continue,
            };
            if !path
                .parent()
                .is_some_and(|parent| same_path(parent, folder))
            {
                continue;
            }
        }
        let target = match item {
            GridItem::Image(path) => CaptionTarget::Image(path.clone()),
            GridItem::ZipFile(path) => CaptionTarget::Book {
                path: path.clone(),
                kind: BookKind::Zip,
            },
            GridItem::PdfFile(path) => CaptionTarget::Book {
                path: path.clone(),
                kind: BookKind::Pdf,
            },
            GridItem::ConvertibleArchive { path, format } => CaptionTarget::Book {
                path: path.clone(),
                kind: BookKind::Archive(*format),
            },
            GridItem::Folder(path) => CaptionTarget::Book {
                path: path.clone(),
                kind: BookKind::Folder,
            },
            _ => {
                targets.unsupported += 1;
                continue;
            }
        };
        if seen.insert(crate::path_key::normalize_keep_drive(target.path())) {
            targets.paths.push(target);
        }
    }
    targets
}

/// Folder-wide actions use the context's retained source members, not only
/// visible aggregate cells. This does not materialize/sort a second grid or scan.
fn collect_folder_caption_targets(
    items: &[GridItem],
    stack: Option<&crate::filename_stack::StackView>,
    folder: &Path,
) -> CaptionTargets {
    let Some(stack) = stack else {
        return collect_caption_targets(items, 0..items.len(), Some(folder));
    };
    let mut targets =
        collect_caption_targets(&stack.passthrough, 0..stack.passthrough.len(), Some(folder));
    let mut seen: HashSet<_> = targets
        .paths
        .iter()
        .map(|target| crate::path_key::normalize_keep_drive(target.path()))
        .collect();
    for member in stack.groups.iter().flat_map(|group| &group.members) {
        if !member.is_video
            && member
                .path
                .parent()
                .is_some_and(|parent| same_path(parent, folder))
            && seen.insert(crate::path_key::normalize_keep_drive(&member.path))
        {
            targets
                .paths
                .push(CaptionTarget::Image(member.path.clone()));
        }
    }
    targets
}

impl App {
    pub(super) fn caption_targets(&self, background: bool) -> CaptionTargets {
        let indices = self.selection_target_indices(ActionSurface::MainWindow);
        if background || indices.is_empty() {
            let Some(folder) = self.current_folder.as_deref() else {
                return CaptionTargets::default();
            };
            collect_folder_caption_targets(&self.items, self.stack_view.as_deref(), folder)
        } else {
            collect_caption_targets(&self.items, indices, None)
        }
    }

    pub(crate) fn caption_creation_disabled_reason(
        &self,
        background: bool,
    ) -> Option<&'static str> {
        if self.remote_session_blocks_local_control() {
            return Some("リモート閲覧中は AI 説明文を作れません");
        }
        if self.caption_delete.is_some() {
            return Some("AI 説明文の削除を処理しています。終わってから選んでください");
        }
        // A native/egui menu still owns its command while this is queried.
        if self.main_chrome_pointer_blocked() {
            return Some("ほかの操作を終えてから AI 説明文を作ってください");
        }
        if self
            .caption_job
            .as_ref()
            .is_some_and(|job| job.state == JobState::Stopping)
        {
            return Some(STOPPING_MESSAGE);
        }
        if self.caption_targets(background).paths.is_empty() {
            return Some(crate::context_menu_model::CAPTION_NO_TARGET_REASON);
        }
        None
    }

    pub(crate) fn request_create_ai_captions(&mut self, ctx: &egui::Context, background: bool) {
        if let Some(reason) = self.caption_creation_disabled_reason(background) {
            self.show_feedback_result(reason.into(), FeedbackKind::Info);
            return;
        }
        let targets = self.caption_targets(background);
        if self.caption_job.is_some() {
            self.append_caption_job_targets(targets);
            return;
        }
        if targets.unsupported != 0 {
            self.show_feedback_result(
                format!("AI 説明文を作成します (対象外 {})", targets.unsupported),
                FeedbackKind::Info,
            );
        }
        self.request_vlm_caption_setup_targets(ctx, targets);
    }

    fn append_caption_job_targets(&mut self, targets: CaptionTargets) {
        let job = self.caption_job.as_mut().expect("existing caption job");
        let before = job.queue.len();
        let unsupported = targets.unsupported;
        let message = if job.append(targets) {
            self.show_caption_progress = true;
            format!(
                "AI 説明文の作成に追加しました (追加 {}、対象外 {unsupported})",
                job.queue.len() - before
            )
        } else {
            STOPPING_MESSAGE.into()
        };
        self.show_feedback_result(message, FeedbackKind::Info);
    }

    pub(crate) fn start_caption_job(
        &mut self,
        ctx: &egui::Context,
        targets: CaptionTargets,
        config: CaptionWorkerConfig,
    ) {
        if self.caption_delete.is_some() {
            self.show_feedback_result(
                "AI 説明文の削除を処理しています。終わってから選んでください".into(),
                FeedbackKind::Info,
            );
            return;
        }
        if self.caption_job.is_some() {
            self.append_caption_job_targets(targets);
            return;
        }
        if targets.paths.is_empty() {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker = match spawn_caption_worker(config, cancel.clone(), ctx.clone()) {
            Ok(worker) => worker,
            Err(error) => {
                crate::logger::log(format!("[VLM] caption thread: {error}"));
                self.show_feedback_result(
                    "AI 説明文の作成を始められませんでした".into(),
                    FeedbackKind::Error,
                );
                return;
            }
        };
        let mut job = CaptionJob::new(worker, cancel);
        job.append(targets);
        if let Err(error) = job.advance() {
            self.show_feedback_result(error, FeedbackKind::Error);
            return;
        }
        self.caption_job = Some(job);
        self.show_caption_progress = true;
    }

    /// Root-frame polling is independent of viewer lifetime and Remote input ownership.
    pub(crate) fn poll_caption_job(&mut self) {
        for _ in 0..64 {
            let Some(job) = self.caption_job.as_mut() else {
                break;
            };
            let result = match job.worker.events.try_recv() {
                Ok(CaptionWorkerEvent::Started(_kill)) => continue,
                Ok(CaptionWorkerEvent::ItemDone(outcome)) => {
                    if matches!(
                        outcome,
                        CaptionItemOutcome::Created | CaptionItemOutcome::FailedWritten
                    ) {
                        // These outcomes alone carry SaveOutcome::Written.
                        if let Some(manager) = self.indexer_init.as_ref() {
                            manager.notify_caption_changed();
                        }
                    }
                    if let Err(error) = job.item_done(outcome, &mut self.caption_display) {
                        Some(Err(error))
                    } else {
                        None
                    }
                }
                Ok(CaptionWorkerEvent::Finished(result)) => Some(result),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("説明文の作成処理との接続が切れました".into()))
                }
            };
            if let Some(result) = result {
                let job = self.caption_job.take().expect("polled job");
                self.show_caption_progress = false;
                match result {
                    Ok(()) => self.show_feedback_result(job.summary(), FeedbackKind::Info),
                    Err(error) => {
                        crate::logger::log(format!("[VLM] caption job: {error}"));
                        self.show_feedback_result(
                            format!("AI 説明文の作成を停止しました: {error}"),
                            FeedbackKind::Error,
                        );
                    }
                }
                break;
            }
        }
    }

    pub(crate) fn stop_caption_job_for_exit(&mut self) {
        self.show_caption_progress = false;
        if let Some(job) = self.caption_job.take() {
            job.cancel.store(true, Ordering::Release);
            job.worker.stop_for_exit();
            // No JoinHandle is retained or joined by the UI owner.
        }
    }

    pub(crate) fn show_caption_job_progress(&mut self, ctx: &egui::Context) {
        if !self.show_caption_progress {
            return;
        }
        // This modeless window must not take a text field's Escape, close under
        // another dialog/popup, or consume a key belonging to another viewport.
        let escape = self.dialog_escape_pressed(ctx)
            && ctx.viewport_id() == egui::ViewportId::ROOT
            && ctx.input(|input| input.viewport().focused).unwrap_or(true)
            && !self.common_modal_dialog_open()
            // Memory owns open menus/ComboBoxes even before their widget is drawn.
            // The current-pass check also protects a popup drawn earlier that has
            // already closed its memory state on this same Escape.
            && !egui::Popup::is_any_open(ctx)
            && !ctx.any_popup_open()
            && ctx.memory(|memory| {
                memory
                    .layer_ids()
                    .filter(|layer| {
                        layer.order == egui::Order::Middle
                            && memory.areas().is_visible(layer)
                            && memory.areas().parent_layer(*layer).is_none()
                    })
                    .last()
            }) == Some(egui::LayerId::new(
                egui::Order::Middle,
                egui::Id::unique("caption_job_progress"),
            ))
            && matches!(
                self.keyboard_owner_for_pass(ctx),
                crate::keyboard_input::KeyboardOwner::ApplicationShortcut { .. }
                    | crate::keyboard_input::KeyboardOwner::FocusedUi { .. }
            );
        if escape {
            ctx.input_mut(|input| {
                input.consume_key(input.modifiers, egui::Key::Escape);
            });
            self.show_caption_progress = false;
            return;
        }
        let Some(job) = self.caption_job.as_mut() else {
            self.show_caption_progress = false;
            return;
        };
        if draw_caption_progress(
            ctx,
            &job.progress,
            job.current.as_ref().map(CaptionTarget::path),
            job.state,
            &mut self.show_caption_progress,
        ) {
            job.stop();
        }
    }
}

fn draw_caption_progress(
    ctx: &egui::Context,
    progress: &CaptionProgress,
    current: Option<&Path>,
    state: JobState,
    open: &mut bool,
) -> bool {
    let mut stop = false;
    egui::Window::new("AI 説明文を作成")
        .id(egui::Id::unique("caption_job_progress"))
        .open(open)
        .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
        .default_width(360.0)
        .max_width((ctx.content_rect().width() - 32.0).max(180.0))
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.label(format!("{} / {} 件", progress.done, progress.total));
            // One elided line keeps the fixed action visible even for long filenames.
            ui.add(
                egui::Label::new(
                    current
                        .and_then(Path::file_name)
                        .unwrap_or_default()
                        .to_string_lossy(),
                )
                .truncate(),
            );
            ui.label(format!(
                "作成 {} / 既にあり {}",
                progress.created, progress.already_created
            ));
            ui.label(format!(
                "対象外 {} (動画の表紙 {})",
                progress.unsupported, progress.video_cover
            ));
            ui.label(format!(
                "ファイルなし {} / 未変換 {} / パスワードが必要 {}",
                progress.missing, progress.unconverted, progress.password_required
            ));
            ui.label(format!(
                "読めなかった・失敗 {} / 変更により破棄 {}",
                progress.failed, progress.discarded
            ));
            ui.horizontal(|ui| {
                stop = ui
                    .add_enabled(state == JobState::Running, egui::Button::new("中止"))
                    .clicked();
                if state == JobState::Stopping {
                    ui.label("終了しています...");
                }
            });
        });
    stop
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::kittest::{NodeT, Queryable};

    fn job() -> (
        CaptionJob,
        mpsc::Receiver<CaptionWorkerCommand>,
        mpsc::Sender<CaptionWorkerEvent>,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (worker, commands, events) = CaptionWorker::test_channels(cancel.clone());
        (CaptionJob::new(worker, cancel), commands, events)
    }

    fn assert_item(commands: &mpsc::Receiver<CaptionWorkerCommand>, expected: &str) {
        match commands.try_recv().unwrap() {
            CaptionWorkerCommand::Item(path) => {
                assert!(same_path(path.path(), Path::new(expected)))
            }
            CaptionWorkerCommand::Finish => panic!("premature Finish"),
        }
    }

    #[test]
    fn acceptance_c_appending_reports_added_and_unsupported_counts() {
        let mut app = crate::app::setup_app_for_test();
        let (job, _commands, _events) = job();
        app.caption_job = Some(job);
        app.items = vec![
            GridItem::Image("C:/Photo.jpg".into()),
            GridItem::PdfPage {
                pdf_path: "C:/Book.pdf".into(),
                page_num: 0,
                content_type: None,
            },
        ];
        app.selected = Some(0);
        app.checked = [0, 1].into_iter().collect();
        app.request_create_ai_captions(&egui::Context::default(), false);
        let text = &app.fs_feedback_toast.as_ref().expect("append feedback").0;
        assert!(text.contains("追加 1"), "{text}");
        assert!(text.contains("対象外 1"), "{text}");
        app.request_create_ai_captions(&egui::Context::default(), false);
        let text = &app
            .fs_feedback_toast
            .as_ref()
            .expect("duplicate append feedback")
            .0;
        assert!(text.contains("追加 0"), "{text}");
        assert!(text.contains("対象外 1"), "{text}");
    }

    fn tools_menu_fixture(append: bool, selected: bool) -> crate::app::AppTestEnvForTest {
        let mut app = crate::app::setup_app_for_test();
        app.current_folder = Some("C:/caption-tools".into());
        app.items = vec![
            GridItem::Image("C:/caption-tools/a.png".into()),
            GridItem::Image("C:/caption-tools/b.png".into()),
            GridItem::Image("C:/caption-tools/sub/child.png".into()),
            GridItem::Video("C:/caption-tools/movie.mp4".into()),
        ];
        if selected {
            app.selected = Some(0);
            app.checked = [0, 3].into_iter().collect();
        }
        app.main_hwnd = None;
        app.vlm_setup =
            super::super::vlm_addon::VlmSetup::for_caption_menu_test(app.tmp.path().join("vlm"));
        if append {
            let (job, _commands, _events) = job();
            app.caption_job = Some(job);
        }
        app
    }

    fn tools_menubar_harness(
        app: crate::app::AppTestEnvForTest,
    ) -> egui_kittest::Harness<'static, crate::app::AppTestEnvForTest> {
        crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(1600.0, 700.0))
            .build_ui_state(
                |ui, app: &mut crate::app::AppTestEnvForTest| {
                    app.note_main_chrome_pointer_block_transition(ui.ctx());
                    app.render_menubar(ui);
                },
                app,
            )
    }

    #[test]
    fn caption_job_tools_menu_creates_and_appends_the_same_selection_targets() {
        for append in [false, true] {
            for selected in [false, true] {
                let app = tools_menu_fixture(append, selected);
                let expected = app.caption_targets(false);
                assert_eq!(app.caption_creation_disabled_reason(false), None);
                assert_eq!(expected.paths.len(), if selected { 1 } else { 2 });
                assert_eq!(expected.unsupported, usize::from(selected));
                let label = app
                    .keymap
                    .menu_command_label(crate::keymap::MenuCommandId::ToolsCreateAiCaptions);
                let mut harness = tools_menubar_harness(app);
                harness.run_steps(4);
                harness
                    .get_by_label(crate::keymap::TopMenuId::Tools.label())
                    .click();
                harness.run_steps(2);
                harness.get_by_label(&label).click();
                harness.run_steps(2);
                let app = harness.state();
                if append {
                    let job = app.caption_job.as_ref().unwrap();
                    assert_eq!(
                        job.queue.iter().cloned().collect::<Vec<_>>(),
                        expected.paths
                    );
                    assert_eq!(job.progress.unsupported, expected.unsupported);
                    assert!(app.show_caption_progress);
                    assert!(app.vlm_setup.captured_caption_targets_for_test().is_none());
                } else {
                    assert_eq!(
                        app.vlm_setup.captured_caption_targets_for_test(),
                        Some(&expected)
                    );
                    assert!(app.caption_job.is_none());
                }
            }
        }
    }

    #[test]
    fn caption_job_tools_menu_progress_click_reopens_the_existing_job() {
        let app = tools_menu_fixture(true, true);
        assert!(!app.show_caption_progress);
        let cancel = app.caption_job.as_ref().unwrap().cancel.clone();
        let label = app
            .keymap
            .menu_command_label(crate::keymap::MenuCommandId::ToolsShowCaptionProgress);
        let mut harness = tools_menubar_harness(app);
        harness.run_steps(4);
        harness
            .get_by_label(crate::keymap::TopMenuId::Tools.label())
            .click();
        harness.run_steps(2);
        harness.get_by_label(&label).click();
        harness.run_steps(2);
        assert!(harness.state().show_caption_progress);
        assert_eq!(
            harness.state().caption_job.as_ref().unwrap().state,
            JobState::Running
        );
        assert!(!cancel.load(Ordering::Acquire));
    }

    #[test]
    fn caption_job_tools_menu_is_disabled_during_caption_deletion() {
        let mut app = tools_menu_fixture(false, true);
        app.caption_delete = Some(super::super::caption_delete::CaptionDeleteState::Counting {
            rx: mpsc::channel().1,
        });
        assert_eq!(
            app.caption_creation_disabled_reason(false),
            Some("AI 説明文の削除を処理しています。終わってから選んでください")
        );
        let mut harness = tools_menubar_harness(app);
        harness.run_steps(4);
        assert!(
            harness
                .get_by_label(crate::keymap::TopMenuId::Tools.label())
                .accesskit_node()
                .is_disabled()
        );
        assert!(harness.state().caption_job.is_none());
    }

    #[test]
    fn caption_job_creation_availability_shares_selected_and_folder_target_rules() {
        let mut app = tools_menu_fixture(false, true);
        assert_eq!(app.caption_creation_disabled_reason(false), None);
        app.selected = Some(3);
        app.checked.clear();
        assert!(app.caption_targets(false).paths.is_empty());
        assert_eq!(
            app.caption_creation_disabled_reason(false),
            Some(crate::context_menu_model::CAPTION_NO_TARGET_REASON)
        );
        // Background and unselected requests use only the current folder's direct members.
        assert_eq!(app.caption_creation_disabled_reason(true), None);
        assert_eq!(app.caption_targets(true).paths.len(), 2);
        app.selected = None;
        assert_eq!(app.caption_creation_disabled_reason(false), None);
        assert_eq!(app.caption_targets(false), app.caption_targets(true));
        app.current_folder = None;
        assert_eq!(
            app.caption_creation_disabled_reason(false),
            Some(crate::context_menu_model::CAPTION_NO_TARGET_REASON)
        );
        // An explicit valid selection remains usable without a current folder.
        app.selected = Some(0);
        assert_eq!(app.caption_creation_disabled_reason(false), None);
    }

    #[test]
    fn caption_job_item_done_invalidates_only_current_key_for_every_outcome() {
        for outcome in [
            CaptionItemOutcome::Created,
            CaptionItemOutcome::AlreadyCreated,
            CaptionItemOutcome::Missing,
            CaptionItemOutcome::Unconverted,
            CaptionItemOutcome::PasswordRequired,
            CaptionItemOutcome::VideoCover,
            CaptionItemOutcome::Failed,
            CaptionItemOutcome::FailedWritten,
            CaptionItemOutcome::Discarded,
            CaptionItemOutcome::Cancelled,
        ] {
            let mut app = crate::app::setup_app_for_test();
            let index_data = app.tmp.path().join("caption-index");
            let caption_epoch = app.caption_epoch_for_test(&index_data);
            let before = caption_epoch.load(Ordering::Acquire);
            let (mut job, commands, events) = job();
            job.append(vec!["C:/images/a.png".into(), "C:/images/b.png".into()].into());
            job.advance().unwrap();
            assert_item(&commands, "C:/images/a.png");
            let key = crate::ai_caption_db::CaptionDb::normalize_path(Path::new("C:/images/a.png"));
            let sibling =
                crate::ai_caption_db::CaptionDb::normalize_path(Path::new("C:/images/b.png"));
            app.caption_display.seed_for_test(&key);
            app.caption_display.seed_for_test(&sibling);
            let epoch = app.caption_display.epoch_for_test();
            app.caption_job = Some(job);
            events.send(CaptionWorkerEvent::ItemDone(outcome)).unwrap();
            app.poll_caption_job();
            assert!(!app.caption_display.contains_for_test(&key), "{outcome:?}");
            assert!(
                app.caption_display.contains_for_test(&sibling),
                "{outcome:?}"
            );
            assert_eq!(app.caption_display.epoch_for_test(), epoch);
            let committed = matches!(
                outcome,
                CaptionItemOutcome::Created | CaptionItemOutcome::FailedWritten
            );
            assert_eq!(
                caption_epoch.load(Ordering::Acquire),
                before + u64::from(committed),
                "{outcome:?}"
            );
            assert_eq!(
                app.caption_job.as_ref().unwrap().progress.failed,
                usize::from(matches!(
                    outcome,
                    CaptionItemOutcome::Failed | CaptionItemOutcome::FailedWritten
                ))
            );
            assert_item(&commands, "C:/images/b.png");
        }
    }

    #[test]
    fn caption_job_additions_before_and_after_result_keep_fifo_without_duplicates() {
        let (mut job, commands, _) = job();
        assert!(
            job.append(
                vec![
                    PathBuf::from("C:/images/a.png"),
                    PathBuf::from("C:/images/b.png")
                ]
                .into()
            )
        );
        job.advance().unwrap();
        assert_item(&commands, "C:/images/a.png");
        assert!(job.append(vec!["c:/images/B.png".into(), "C:/images/c.png".into()].into()));
        job.item_done(CaptionItemOutcome::Created, &mut Default::default())
            .unwrap();
        assert_item(&commands, "C:/images/b.png");
        assert!(job.append(vec!["C:/images/d.png".into()].into()));
        job.item_done(CaptionItemOutcome::AlreadyCreated, &mut Default::default())
            .unwrap();
        assert_item(&commands, "C:/images/c.png");
        job.item_done(CaptionItemOutcome::Failed, &mut Default::default())
            .unwrap();
        assert_item(&commands, "C:/images/d.png");
        job.item_done(CaptionItemOutcome::Missing, &mut Default::default())
            .unwrap();
        assert!(matches!(
            commands.try_recv(),
            Ok(CaptionWorkerCommand::Finish)
        ));
        assert_eq!(job.state, JobState::Stopping);
        assert_eq!(job.progress.total, 4);
        assert_eq!(job.progress.done, 4);
        assert!(matches!(
            commands.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
    }

    #[test]
    fn caption_job_finish_closes_admission_until_completion_is_polled() {
        let (mut job, commands, events) = job();
        job.append(vec!["a.png".into()].into());
        job.advance().unwrap();
        assert_item(&commands, "a.png");
        job.item_done(CaptionItemOutcome::Created, &mut Default::default())
            .unwrap();
        assert!(matches!(
            commands.try_recv(),
            Ok(CaptionWorkerCommand::Finish)
        ));
        assert!(!job.append(vec!["late.png".into()].into()));
        let mut app = crate::app::setup_app_for_test();
        app.caption_job = Some(job);
        app.request_create_ai_captions(&egui::Context::default(), false);
        assert_eq!(app.caption_job.as_ref().unwrap().progress.total, 1);
        assert!(app.vlm_caption_job_running());
        app.poll_caption_job();
        assert!(app.caption_job.is_some());
        events.send(CaptionWorkerEvent::Finished(Ok(()))).unwrap();
        app.poll_caption_job();
        assert!(app.caption_job.is_none());
        assert!(!app.vlm_caption_job_running());
    }

    #[test]
    fn caption_job_inflight_same_path_is_queued_once_for_recheck_after_result() {
        let (mut job, commands, _) = job();
        job.append(vec!["C:/a.png".into()].into());
        job.advance().unwrap();
        assert_item(&commands, "C:/a.png");
        job.append(vec!["c:/A.png".into(), "C:/a.png".into()].into());
        assert_eq!(job.queue.len(), 1);
        job.item_done(CaptionItemOutcome::Failed, &mut Default::default())
            .unwrap();
        assert_item(&commands, "C:/a.png");
        assert_eq!(job.state, JobState::Running);
    }

    #[test]
    fn caption_job_cancel_closes_admission_before_inflight_result() {
        let (mut job, commands, _) = job();
        job.append(vec!["a.png".into(), "queued.png".into()].into());
        job.advance().unwrap();
        assert_item(&commands, "a.png");
        job.stop();
        assert!(job.cancel.load(Ordering::Acquire));
        assert_eq!(job.state, JobState::Stopping);
        assert!(job.current.is_some());
        assert!(job.queue.is_empty());
        assert!(!job.append(vec!["after-cancel.png".into()].into()));
        assert!(matches!(
            commands.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        job.item_done(CaptionItemOutcome::Cancelled, &mut Default::default())
            .unwrap();
        assert!(matches!(
            commands.try_recv(),
            Ok(CaptionWorkerCommand::Finish)
        ));
        assert_eq!(job.progress.created, 0);
    }

    #[test]
    fn caption_job_targets_share_selection_and_ignore_viewer_cursor() {
        let mut app = crate::app::setup_app_for_test();
        app.current_folder = Some("C:/images".into());
        app.items = vec![
            GridItem::Image("C:/images/a.png".into()),
            GridItem::Image("C:/images/b.png".into()),
            GridItem::Image("C:/images/c.png".into()),
        ];
        app.selected = Some(1);
        app.fullscreen_idx = Some(2);
        app.checked = [0, 1].into_iter().collect();
        assert_eq!(
            app.caption_targets(false).paths,
            vec![
                CaptionTarget::Image("C:/images/a.png".into()),
                CaptionTarget::Image("C:/images/b.png".into())
            ]
        );
        app.checked = [0, 2].into_iter().collect();
        assert_eq!(
            app.caption_targets(false).paths,
            vec![CaptionTarget::Image("C:/images/b.png".into())]
        );
        app.selected = None;
        app.checked.clear();
        assert_eq!(app.caption_targets(false).paths.len(), 3);
        app.selected = Some(1);
        assert_eq!(app.caption_targets(true).paths.len(), 3);
    }

    #[test]
    fn caption_job_targets_images_selected_books_and_direct_file_books_without_io() {
        let items = vec![
            GridItem::Image("C:/images/a.png".into()),
            GridItem::Image("C:/images/sub/child.png".into()),
            GridItem::Folder("C:/images/sub".into()),
            GridItem::ZipFile("C:/images/book.zip".into()),
            GridItem::PdfFile("C:/images/book.pdf".into()),
            GridItem::PdfFile("C:/images/book.epub".into()),
            GridItem::ConvertibleArchive {
                path: "C:/images/book.7z".into(),
                format: crate::archive_converter::ArchiveFormat::SevenZ,
            },
            GridItem::ZipDir {
                zip_path: "C:/images/book.zip".into(),
                dir_prefix: "sub/".into(),
                is_archive: false,
                representative: None,
            },
            GridItem::ZipImage {
                zip_path: "C:/images/book.zip".into(),
                entry_name: "p.png".into(),
            },
            GridItem::PdfPage {
                pdf_path: "C:/images/book.pdf".into(),
                page_num: 0,
                content_type: None,
            },
            GridItem::Video("C:/images/video.mp4".into()),
            GridItem::Audio("C:/images/audio.mp3".into()),
        ];
        let selected = collect_caption_targets(&items, 0..items.len(), None);
        assert_eq!(selected.paths.len(), 7);
        assert_eq!(selected.unsupported, 5);
        assert_eq!(
            selected.paths[2],
            CaptionTarget::Book {
                path: "C:/images/sub".into(),
                kind: BookKind::Folder
            }
        );
        assert_eq!(
            selected.paths[6],
            CaptionTarget::Book {
                path: "C:/images/book.7z".into(),
                kind: BookKind::Archive(crate::archive_converter::ArchiveFormat::SevenZ)
            }
        );
        let background =
            collect_caption_targets(&items, 0..items.len(), Some(Path::new("C:/images")));
        assert_eq!(
            background.paths,
            vec![
                CaptionTarget::Image("C:/images/a.png".into()),
                CaptionTarget::Book {
                    path: "C:/images/book.zip".into(),
                    kind: BookKind::Zip
                },
                CaptionTarget::Book {
                    path: "C:/images/book.pdf".into(),
                    kind: BookKind::Pdf
                },
                CaptionTarget::Book {
                    path: "C:/images/book.epub".into(),
                    kind: BookKind::Pdf
                },
                CaptionTarget::Book {
                    path: "C:/images/book.7z".into(),
                    kind: BookKind::Archive(crate::archive_converter::ArchiveFormat::SevenZ)
                },
            ]
        );
        assert_eq!(background.unsupported, 0);
    }

    fn stack_fixture() -> crate::filename_stack::StackView {
        use crate::filename_stack::{StackMember, StackView};
        let media = [
            ("C:/images/post_0.png", false),
            ("C:/images/post_1.png", false),
            ("C:/images/single.png", false),
            ("C:/images/sub/child.png", false),
            ("C:/images/video.mp4", true),
        ]
        .into_iter()
        .map(|(path, is_video)| StackMember {
            path: path.into(),
            mtime: 0,
            size: Some(1),
            is_video,
        })
        .collect();
        StackView::build(
            "C:/images".into(),
            vec![
                GridItem::ZipFile("C:/images/book.zip".into()),
                GridItem::Folder("C:/images/sub".into()),
                GridItem::Audio("C:/images/song.mp3".into()),
            ],
            vec![None; 3],
            media,
            '_',
            crate::settings::SortOrder::FileName,
        )
    }

    #[test]
    fn caption_job_folder_targets_are_identical_with_stack_on_off_or_flat() {
        let mut app = crate::app::setup_app_for_test();
        let view = stack_fixture();
        app.current_folder = Some(view.folder.clone());
        app.items = view.materialize_flat().0;
        app.selected = None;
        let plain = app.caption_targets(false);
        let keys = |targets: &CaptionTargets| -> std::collections::BTreeSet<_> {
            targets
                .paths
                .iter()
                .map(|target| crate::path_key::normalize_keep_drive(target.path()))
                .collect()
        };
        assert_eq!(plain.paths.len(), 4);
        assert_eq!(plain.unsupported, 0);
        app.items = view.materialize_aggregated().0;
        app.stack_view = Some(Arc::new(view));
        for background in [false, true] {
            let aggregated = app.caption_targets(background);
            assert_eq!(keys(&aggregated), keys(&plain));
            assert_eq!(aggregated.unsupported, plain.unsupported);
        }
        // A background request ignores the selected collapsed stack too.
        app.selected = app
            .items
            .iter()
            .position(|i| matches!(i, GridItem::Stack { .. }));
        assert!(app.selected.is_some());
        assert_eq!(keys(&app.caption_targets(true)), keys(&plain));
        app.items = app.stack_view.as_ref().unwrap().materialize_flat().0;
        app.stack_showing_flat = true;
        app.selected = None;
        assert_eq!(keys(&app.caption_targets(false)), keys(&plain));
    }

    #[test]
    fn caption_job_selected_stack_is_unsupported_like_existing_grid_tag_operation() {
        let mut app = crate::app::setup_app_for_test();
        let view = stack_fixture();
        app.current_folder = Some(view.folder.clone());
        app.items = view.materialize_aggregated().0;
        app.stack_view = Some(Arc::new(view));
        app.selected = app
            .items
            .iter()
            .position(|i| matches!(i, GridItem::Stack { .. }));
        assert!(app.selected.is_some());
        assert!(app.tag_target_paths(ActionSurface::MainWindow).is_empty());
        let targets = app.caption_targets(false);
        assert!(targets.paths.is_empty());
        assert_eq!(targets.unsupported, 1);
        app.selected = app
            .items
            .iter()
            .position(|i| matches!(i, GridItem::Image(_)));
        assert!(app.selected.is_some());
        assert_eq!(app.caption_targets(false).paths.len(), 1);
    }

    #[test]
    fn caption_job_exit_drops_without_waiting_for_worker_completion() {
        let (job, commands, _events) = job();
        let cancel = job.cancel.clone();
        let mut app = crate::app::setup_app_for_test();
        app.caption_job = Some(job);
        app.stop_caption_job_for_exit();
        assert!(app.caption_job.is_none());
        assert!(cancel.load(Ordering::Acquire));
        assert!(matches!(
            commands.try_recv(),
            Ok(CaptionWorkerCommand::Finish)
        ));
    }

    #[test]
    fn caption_job_book_dedup_uses_path_across_target_kinds_and_cancel_retains_lifecycle() {
        let (mut job, commands, _) = job();
        let book = CaptionTarget::Book {
            path: "C:/books/a.zip".into(),
            kind: BookKind::Zip,
        };
        job.append(CaptionTargets {
            paths: vec![book.clone(), CaptionTarget::Image("c:/books/A.zip".into())],
            unsupported: 0,
        });
        assert_eq!(job.queue.len(), 1);
        job.advance().unwrap();
        assert!(
            matches!(commands.try_recv().unwrap(), CaptionWorkerCommand::Item(target) if target == book)
        );
        job.append(CaptionTargets {
            paths: vec![book.clone(), book],
            unsupported: 0,
        });
        assert_eq!(job.queue.len(), 1);
        assert_eq!(job.progress.total, 2);
        job.stop();
        assert!(job.current.is_some());
        assert!(job.queue.is_empty());
        assert!(!job.append(vec!["late.png".into()].into()));
        assert!(matches!(
            commands.try_recv(),
            Err(mpsc::TryRecvError::Empty)
        ));
        job.item_done(CaptionItemOutcome::Cancelled, &mut Default::default())
            .unwrap();
        assert!(matches!(
            commands.try_recv(),
            Ok(CaptionWorkerCommand::Finish)
        ));
    }

    #[test]
    fn caption_job_book_outcomes_have_separate_counts_and_conversion_guidance() {
        let (mut job, commands, _) = job();
        job.append(vec!["one.7z".into(), "two.pdf".into(), "three.zip".into()].into());
        job.advance().unwrap();
        for outcome in [
            CaptionItemOutcome::Unconverted,
            CaptionItemOutcome::PasswordRequired,
            CaptionItemOutcome::VideoCover,
        ] {
            assert!(matches!(
                commands.try_recv(),
                Ok(CaptionWorkerCommand::Item(_))
            ));
            job.item_done(outcome, &mut Default::default()).unwrap();
        }
        assert_eq!(job.progress.unconverted, 1);
        assert_eq!(job.progress.password_required, 1);
        assert_eq!(job.progress.unsupported, 1);
        assert_eq!(job.progress.video_cover, 1);
        assert_eq!(job.progress.done, 3);
        assert!(job.summary().contains("一度開いて変換すると作れます"));
        assert!(job.summary().contains("動画の表紙 1"));
        assert!(job.summary().contains("パスワードが必要 1"));
        let (job, _, _) = self::job();
        assert!(!job.summary().contains("一度開いて変換すると作れます"));
    }

    #[test]
    fn caption_job_hidden_progress_keeps_polling_and_addition_reopens_it() {
        let mut app = crate::app::setup_app_for_test();
        let (mut job, commands, events) = job();
        job.append(vec!["a.png".into(), "b.png".into()].into());
        job.advance().unwrap();
        assert_item(&commands, "a.png");
        let cancel = job.cancel.clone();
        app.caption_job = Some(job);
        app.show_caption_progress = false;
        app.show_caption_job_progress(&egui::Context::default());
        events
            .send(CaptionWorkerEvent::ItemDone(CaptionItemOutcome::Created))
            .unwrap();
        app.poll_caption_job();
        assert_item(&commands, "b.png");
        assert!(!app.show_caption_progress);
        assert!(!cancel.load(Ordering::Acquire));
        app.append_caption_job_targets(vec!["c.png".into()].into());
        assert!(app.show_caption_progress);
        app.show_caption_progress = false;
        events
            .send(CaptionWorkerEvent::ItemDone(
                CaptionItemOutcome::AlreadyCreated,
            ))
            .unwrap();
        app.poll_caption_job();
        assert_item(&commands, "c.png");
        events
            .send(CaptionWorkerEvent::ItemDone(CaptionItemOutcome::Created))
            .unwrap();
        app.poll_caption_job();
        assert!(matches!(
            commands.try_recv(),
            Ok(CaptionWorkerCommand::Finish)
        ));
        // Finishing jobs remain inspectable, while additions cannot reopen them.
        app.show_caption_progress = true;
        app.append_caption_job_targets(vec!["late.png".into()].into());
        assert_eq!(app.caption_job.as_ref().unwrap().progress.total, 3);
        events.send(CaptionWorkerEvent::Finished(Ok(()))).unwrap();
        app.poll_caption_job();
        assert!(app.caption_job.is_none());
        assert!(!app.show_caption_progress);
        let summary = &app.fs_feedback_toast.as_ref().unwrap().0;
        assert!(summary.contains("作成 2、既にあり 1"), "{summary}");
    }

    #[test]
    fn caption_job_progress_close_and_escape_leave_job_running() {
        for close in ["Close window", "Escape"] {
            let mut app = crate::app::setup_app_for_test();
            let (mut job, _commands, _events) = job();
            job.append(vec!["a.png".into()].into());
            job.advance().unwrap();
            let cancel = job.cancel.clone();
            app.caption_job = Some(job);
            app.show_caption_progress = true;
            let mut harness = crate::test_ui::builder()
                .with_context_setup(crate::ui_fonts::configure_fonts)
                .with_size(egui::vec2(900.0, 600.0))
                .build_ui_state(
                    |ui, app: &mut crate::app::AppTestEnvForTest| {
                        app.show_caption_job_progress(ui.ctx());
                    },
                    app,
                );
            harness.run_steps(4);
            if close == "Escape" {
                harness.key_press(egui::Key::Escape);
            } else {
                harness.get_by_label(close).click();
            }
            harness.run_steps(2);
            assert!(!harness.state().show_caption_progress, "{close}");
            let job = harness.state().caption_job.as_ref().unwrap();
            assert_eq!(job.state, JobState::Running, "{close}");
            assert!(job.current.is_some(), "{close}");
            assert!(!cancel.load(Ordering::Acquire), "{close}");
        }
    }

    #[test]
    fn caption_job_progress_escape_preserves_text_ime_and_modal_owners() {
        for owner in ["text", "ime", "modal", "covered"] {
            let mut app = crate::app::setup_app_for_test();
            let (mut job, _commands, _events) = job();
            job.append(vec!["a.png".into()].into());
            job.advance().unwrap();
            app.caption_job = Some(job);
            app.show_caption_progress = true;
            if owner == "text" {
                app.search_has_focus = true;
            } else if owner == "modal" {
                app.show_open_folder_dialog = true;
            }
            let mut harness = crate::test_ui::builder()
                .with_context_setup(|ctx| {
                    crate::ui_fonts::configure_fonts(ctx);
                    crate::ime_focus::install_ime_input_policy(ctx);
                })
                .with_size(egui::vec2(900.0, 600.0))
                .build_ui_state(
                    move |ui, app: &mut crate::app::AppTestEnvForTest| {
                        app.show_caption_job_progress(ui.ctx());
                        if owner == "covered" {
                            egui::Window::new("Another tool").show(ui.ctx(), |ui| {
                                ui.label("Another window owns Escape");
                            });
                        }
                    },
                    app,
                );
            harness.run_steps(4);
            if owner == "ime" {
                harness.event(egui::Event::Ime(egui::ImeEvent::Preedit {
                    text: "変換中".into(),
                    active_range_chars: None,
                }));
                harness.run_steps(1);
            }
            harness.key_press(egui::Key::Escape);
            harness.run_steps(2);
            assert!(harness.state().show_caption_progress, "{owner}");
            let job = harness.state().caption_job.as_ref().unwrap();
            assert_eq!(job.state, JobState::Running, "{owner}");
            assert!(!job.cancel.load(Ordering::Acquire), "{owner}");
        }
    }

    #[test]
    fn caption_job_progress_escape_after_covering_window_closes() {
        let mut app = crate::app::setup_app_for_test();
        let (job, _commands, _events) = job();
        app.caption_job = Some(job);
        app.show_caption_progress = true;
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(900.0, 600.0))
            .build_ui_state(
                |ui, (app, covering): &mut (crate::app::AppTestEnvForTest, bool)| {
                    app.show_caption_job_progress(ui.ctx());
                    if *covering {
                        egui::Window::new("Covering tool").show(ui.ctx(), |ui| {
                            ui.label("Another window owns Escape");
                        });
                    }
                },
                (app, true),
            );
        harness.run_steps(4);
        harness.key_press(egui::Key::Escape);
        harness.run_steps(2);
        assert!(harness.state().0.show_caption_progress);
        harness.state_mut().1 = false;
        harness.run_steps(2);
        harness.key_press(egui::Key::Escape);
        harness.run_steps(2);
        assert!(!harness.state().0.show_caption_progress);
        assert!(harness.state().0.caption_job.is_some());
    }

    #[test]
    fn caption_job_progress_small_window_cancel_visible_and_clickable() {
        for size in [egui::vec2(1093.0, 614.0), egui::vec2(546.5, 307.0)] {
            let mut harness = crate::test_ui::builder()
                .with_context_setup(crate::ui_fonts::configure_fonts)
                .with_size(size)
                .build_ui_state(|ui, stopped: &mut bool| {
                    *stopped |= draw_caption_progress(ui.ctx(), &CaptionProgress { total: 100, done: 10, unsupported: 2, failed: 3, ..Default::default() }, Some(Path::new("とても長い画像の名前ですとても長い画像の名前ですとても長い画像の名前です.png")), JobState::Running, &mut true);
                }, false);
            harness.run_steps(4);
            let cancel = harness.get_by_role_and_label(egui::accesskit::Role::Button, "中止");
            assert!(egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(cancel.rect()));
            let close = harness.get_by_label("Close window");
            assert!(egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(close.rect()));
            cancel.click();
            harness.run_steps(2);
            assert!(*harness.state());
        }
    }

    #[test]
    fn caption_job_progress_snapshot() {
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(480.0, 290.0))
            .build_ui(|ui| {
                let ctx = ui.ctx();
                crate::os_theme::apply_resolved(ctx, crate::os_theme::ResolvedTheme::Dark);
                draw_caption_progress(
                    ctx,
                    &CaptionProgress {
                        total: 27,
                        done: 12,
                        created: 5,
                        already_created: 3,
                        unsupported: 2,
                        missing: 1,
                        failed: 1,
                        ..Default::default()
                    },
                    Some(Path::new("sample.png")),
                    JobState::Running,
                    &mut true,
                );
            });
        harness.run_steps(4);
        harness.snapshot("caption_job_progress");
    }
}

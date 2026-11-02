//! App-owned admission and progress for a single local caption job.
//! Selection is converted to paths without I/O; only the worker touches files.
use std::collections::{HashSet, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use eframe::egui;

use crate::ai::caption_job_worker::{
    CaptionItemOutcome, CaptionWorker, CaptionWorkerCommand, CaptionWorkerConfig,
    CaptionWorkerEvent, spawn_caption_worker,
};
use crate::app::{ActionSurface, App};
use crate::feedback::FeedbackKind;
use crate::grid_item::GridItem;

const STOPPING_MESSAGE: &str = "前のジョブを終えています。終わってから選び直してください";

/// Captured at the menu boundary and retained as one value through model setup.
/// Unsupported items need no file reads, but remain in the final/progress counts.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct CaptionTargets {
    pub paths: Vec<PathBuf>,
    pub unsupported: usize,
}

impl From<Vec<PathBuf>> for CaptionTargets {
    fn from(paths: Vec<PathBuf>) -> Self {
        Self {
            paths,
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
    failed: usize,
    discarded: usize,
}

pub(crate) struct CaptionJob {
    state: JobState,
    queue: VecDeque<PathBuf>,
    current: Option<PathBuf>,
    progress: Arc<CaptionProgress>,
    cancel: Arc<AtomicBool>,
    worker: CaptionWorker,
}

impl CaptionJob {
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
            .map(|path| crate::path_key::normalize_keep_drive(path))
            .collect();
        for path in targets.paths {
            if !seen.insert(crate::path_key::normalize_keep_drive(&path)) {
                continue;
            }
            self.queue.push_back(path);
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

    fn item_done(&mut self, outcome: CaptionItemOutcome) -> Result<(), String> {
        self.current = None;
        let progress = Arc::make_mut(&mut self.progress);
        progress.done += 1;
        match outcome {
            CaptionItemOutcome::Created => progress.created += 1,
            CaptionItemOutcome::AlreadyCreated => progress.already_created += 1,
            CaptionItemOutcome::Missing => progress.missing += 1,
            CaptionItemOutcome::Failed => progress.failed += 1,
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
        format!(
            "AI 説明文の{completion} (作成 {}、作成済み {}、対象外 {}、ファイルなし {}、失敗 {}、変更により破棄 {})",
            p.created, p.already_created, p.unsupported, p.missing, p.failed, p.discarded,
        )
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
        match item {
            GridItem::Image(path) => {
                if seen.insert(crate::path_key::normalize_keep_drive(path)) {
                    targets.paths.push(path.clone());
                }
            }
            _ => targets.unsupported += 1,
        }
    }
    targets
}

impl App {
    fn caption_targets(&self, background: bool) -> CaptionTargets {
        let indices = self.selection_target_indices(ActionSurface::MainWindow);
        if background || indices.is_empty() {
            let Some(folder) = self.current_folder.as_deref() else {
                return CaptionTargets::default();
            };
            collect_caption_targets(&self.items, 0..self.items.len(), Some(folder))
        } else {
            collect_caption_targets(&self.items, indices, None)
        }
    }

    pub(crate) fn request_create_ai_captions(&mut self, ctx: &egui::Context, background: bool) {
        if self.common_modal_dialog_open() || self.remote_session_blocks_local_control() {
            return;
        }
        if self
            .caption_job
            .as_ref()
            .is_some_and(|job| job.state == JobState::Stopping)
        {
            self.show_feedback_result(STOPPING_MESSAGE.into(), FeedbackKind::Info);
            return;
        }
        let targets = self.caption_targets(background);
        if let Some(job) = self.caption_job.as_mut() {
            job.append(targets);
            return;
        }
        if targets.paths.is_empty() {
            self.show_feedback_result(
                format!(
                    "AI 説明文を作れる画像がありません (対象外 {})",
                    targets.unsupported
                ),
                FeedbackKind::Info,
            );
            return;
        }
        self.request_vlm_caption_setup_targets(ctx, targets);
    }

    pub(crate) fn start_caption_job(
        &mut self,
        ctx: &egui::Context,
        targets: CaptionTargets,
        config: CaptionWorkerConfig,
    ) {
        if let Some(job) = self.caption_job.as_mut() {
            if !job.append(targets) {
                self.show_feedback_result(STOPPING_MESSAGE.into(), FeedbackKind::Info);
            }
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
                    if let Err(error) = job.item_done(outcome) {
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
        if let Some(job) = self.caption_job.take() {
            job.cancel.store(true, Ordering::Release);
            job.worker.stop_for_exit();
            // No JoinHandle is retained or joined by the UI owner.
        }
    }

    pub(crate) fn show_caption_job_progress(&mut self, ctx: &egui::Context) {
        let Some(job) = self.caption_job.as_mut() else {
            return;
        };
        if draw_caption_progress(ctx, &job.progress, job.current.as_deref(), job.state) {
            job.stop();
        }
    }
}

fn draw_caption_progress(
    ctx: &egui::Context,
    progress: &CaptionProgress,
    current: Option<&Path>,
    state: JobState,
) -> bool {
    let mut stop = false;
    egui::Window::new("AI 説明文を作成")
        .id(egui::Id::unique("caption_job_progress"))
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
                "飛ばした数: 作成済み {} / 対象外 {} / ファイルなし {}",
                progress.already_created, progress.unsupported, progress.missing
            ));
            ui.label(format!(
                "失敗 {} / 変更により破棄 {}",
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
            CaptionWorkerCommand::Item(path) => assert!(same_path(&path, Path::new(expected))),
            CaptionWorkerCommand::Finish => panic!("premature Finish"),
        }
    }

    #[test]
    fn caption_job_additions_before_and_after_result_keep_fifo_without_duplicates() {
        let (mut job, commands, _) = job();
        assert!(
            job.append(vec![PathBuf::from("C:/images/a.png"), "C:/images/b.png".into()].into())
        );
        job.advance().unwrap();
        assert_item(&commands, "C:/images/a.png");
        assert!(job.append(vec!["c:/images/B.png".into(), "C:/images/c.png".into()].into()));
        job.item_done(CaptionItemOutcome::Created).unwrap();
        assert_item(&commands, "C:/images/b.png");
        assert!(job.append(vec!["C:/images/d.png".into()].into()));
        job.item_done(CaptionItemOutcome::AlreadyCreated).unwrap();
        assert_item(&commands, "C:/images/c.png");
        job.item_done(CaptionItemOutcome::Failed).unwrap();
        assert_item(&commands, "C:/images/d.png");
        job.item_done(CaptionItemOutcome::Missing).unwrap();
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
        job.item_done(CaptionItemOutcome::Created).unwrap();
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
        job.item_done(CaptionItemOutcome::Failed).unwrap();
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
        job.item_done(CaptionItemOutcome::Cancelled).unwrap();
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
            vec![PathBuf::from("C:/images/a.png"), "C:/images/b.png".into()]
        );
        app.checked = [0, 2].into_iter().collect();
        assert_eq!(
            app.caption_targets(false).paths,
            vec![PathBuf::from("C:/images/b.png")]
        );
        app.selected = None;
        app.checked.clear();
        assert_eq!(app.caption_targets(false).paths.len(), 3);
        app.selected = Some(1);
        assert_eq!(app.caption_targets(true).paths.len(), 3);
    }

    #[test]
    fn caption_job_targets_only_real_images_and_direct_children_without_io() {
        let items = vec![
            GridItem::Image("C:/images/a.png".into()),
            GridItem::Image("C:/images/sub/child.png".into()),
            GridItem::Folder("C:/images/sub".into()),
            GridItem::ZipFile("C:/images/book.zip".into()),
            GridItem::PdfFile("C:/images/book.pdf".into()),
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
        assert_eq!(selected.paths.len(), 2);
        assert_eq!(selected.unsupported, 7);
        let background =
            collect_caption_targets(&items, 0..items.len(), Some(Path::new("C:/images")));
        assert_eq!(background.paths, vec![PathBuf::from("C:/images/a.png")]);
        assert_eq!(background.unsupported, 2);
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
    fn caption_job_progress_small_window_cancel_visible_and_clickable() {
        for size in [egui::vec2(1093.0, 614.0), egui::vec2(546.5, 307.0)] {
            let mut harness = crate::test_ui::builder()
                .with_context_setup(crate::ui_fonts::configure_fonts)
                .with_size(size)
                .build_ui_state(|ui, stopped: &mut bool| {
                    *stopped |= draw_caption_progress(ui.ctx(), &CaptionProgress { total: 100, done: 10, unsupported: 2, failed: 3, ..Default::default() }, Some(Path::new("とても長い画像の名前ですとても長い画像の名前ですとても長い画像の名前です.png")), JobState::Running);
                }, false);
            harness.run_steps(4);
            let cancel = harness.get_by_role_and_label(egui::accesskit::Role::Button, "中止");
            assert!(egui::Rect::from_min_size(egui::Pos2::ZERO, size).contains_rect(cancel.rect()));
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
                        already_created: 3,
                        unsupported: 2,
                        missing: 1,
                        failed: 1,
                        ..Default::default()
                    },
                    Some(Path::new("sample.png")),
                    JobState::Running,
                );
            });
        harness.run_steps(4);
        harness.snapshot("caption_job_progress");
    }
}

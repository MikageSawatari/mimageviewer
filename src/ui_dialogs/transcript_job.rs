//! 文字起こしの受付・待ち行列・進捗を所有する。実ファイルには worker だけが触れる。
use crate::ai::transcript_job_worker::{
    TranscriptItemOutcome, TranscriptTarget, TranscriptWorker, TranscriptWorkerCommand,
    TranscriptWorkerConfig, TranscriptWorkerEvent, spawn_transcript_worker,
};
use crate::app::{ActionSurface, App};
use crate::feedback::FeedbackKind;
use crate::grid_item::GridItem;
use eframe::egui;
use std::collections::{HashSet, VecDeque};
use std::path::Path;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
    mpsc,
};

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct TranscriptTargets {
    pub paths: Vec<TranscriptTarget>,
    pub unsupported: usize,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum JobState {
    Running,
    Stopping,
}

#[derive(Default)]
struct Progress {
    total: usize,
    done: usize,
    created: usize,
    already: usize,
    no_audio: usize,
    unsupported: usize,
    failed: usize,
    discarded: usize,
    processed_ms: u64,
    duration_ms: u64,
}

pub(crate) struct TranscriptJob {
    state: JobState,
    queue: VecDeque<TranscriptTarget>,
    current: Option<TranscriptTarget>,
    progress: Progress,
    worker: TranscriptWorker,
    cancel: Arc<AtomicBool>,
}

impl TranscriptJob {
    fn new(worker: TranscriptWorker, cancel: Arc<AtomicBool>) -> Self {
        Self {
            state: JobState::Running,
            queue: VecDeque::new(),
            current: None,
            progress: Progress::default(),
            worker,
            cancel,
        }
    }
    fn append(&mut self, targets: TranscriptTargets) -> usize {
        if self.state != JobState::Running {
            return 0;
        }
        let mut seen: HashSet<_> = self
            .queue
            .iter()
            .map(|p| crate::path_key::normalize_keep_drive(&p.path))
            .collect();
        let mut added = 0;
        for path in targets.paths {
            if seen.insert(crate::path_key::normalize_keep_drive(&path.path)) {
                self.queue.push_back(path);
                added += 1;
            }
        }
        self.progress.total += added + targets.unsupported;
        self.progress.done += targets.unsupported;
        self.progress.unsupported += targets.unsupported;
        added
    }
    fn advance(&mut self) -> Result<(), String> {
        // 中止後の保存済み通知は採用するが、新しい項目は送らない。
        if self.state == JobState::Stopping {
            return Ok(());
        }
        let command = if let Some(path) = self.queue.pop_front() {
            self.current = Some(path.clone());
            self.progress.processed_ms = 0;
            self.progress.duration_ms = 0;
            TranscriptWorkerCommand::Item(path)
        } else {
            self.state = JobState::Stopping;
            TranscriptWorkerCommand::Finish
        };
        self.worker
            .commands
            .send(command)
            .map_err(|_| "文字起こしの作成処理が終了しました".into())
    }
    pub(super) fn stop(&mut self) {
        self.state = JobState::Stopping;
        self.queue.clear();
        self.worker.stop();
    }
    fn item_done(&mut self, outcome: TranscriptItemOutcome) -> Result<(), String> {
        self.current = None;
        self.progress.done += 1;
        match outcome {
            TranscriptItemOutcome::Created => self.progress.created += 1,
            TranscriptItemOutcome::AlreadyCreated => self.progress.already += 1,
            TranscriptItemOutcome::NoAudio => self.progress.no_audio += 1,
            TranscriptItemOutcome::Failed | TranscriptItemOutcome::FailedWritten => {
                self.progress.failed += 1
            }
            TranscriptItemOutcome::Discarded => self.progress.discarded += 1,
            TranscriptItemOutcome::Cancelled => {}
        }
        self.advance()
    }
    fn summary(&self) -> String {
        format!(
            "文字起こしの作成{} (作成 {}、既にあり {}、音声なし {}、対象外 {}、失敗 {}、変更により破棄 {})",
            if self.cancel.load(Ordering::Acquire) {
                "を中止しました"
            } else {
                "が終わりました"
            },
            self.progress.created,
            self.progress.already,
            self.progress.no_audio,
            self.progress.unsupported,
            self.progress.failed,
            self.progress.discarded
        )
    }
}

fn collect_targets(
    items: &[GridItem],
    indices: impl IntoIterator<Item = usize>,
    folder: Option<&Path>,
) -> TranscriptTargets {
    let mut result = TranscriptTargets::default();
    let mut seen = HashSet::new();
    for index in indices {
        let Some(item) = items.get(index) else {
            continue;
        };
        if let Some(folder) = folder {
            // 仮想項目・サブフォルダを走査しない。保持済みの一覧だけを使う。
            let path = match item {
                GridItem::Video(p)
                | GridItem::Audio(p)
                | GridItem::Image(p)
                | GridItem::ZipFile(p)
                | GridItem::PdfFile(p) => p,
                _ => continue,
            };
            if !path
                .parent()
                .is_some_and(|p| crate::folder_tree::path_eq(p, folder))
            {
                continue;
            }
        }
        let path = match item {
            GridItem::Video(p) | GridItem::Audio(p) => p,
            _ => {
                result.unsupported += 1;
                continue;
            }
        };
        if seen.insert(crate::path_key::normalize_keep_drive(path)) {
            result.paths.push(path.clone().into());
        }
    }
    result
}

impl App {
    pub(super) fn transcript_targets(&self, background: bool) -> TranscriptTargets {
        let indices = self.selection_target_indices(ActionSurface::MainWindow);
        let mut targets = if background || indices.is_empty() {
            let Some(folder) = self.current_folder.as_deref() else {
                return TranscriptTargets::default();
            };
            if let Some(stack) = self.stack_view.as_deref() {
                let mut targets =
                    collect_targets(&stack.passthrough, 0..stack.passthrough.len(), Some(folder));
                let mut seen: HashSet<_> = targets
                    .paths
                    .iter()
                    .map(|p| crate::path_key::normalize_keep_drive(&p.path))
                    .collect();
                for member in stack.groups.iter().flat_map(|g| &g.members) {
                    if member
                        .path
                        .parent()
                        .is_some_and(|p| crate::folder_tree::path_eq(p, folder))
                    {
                        if member.is_video
                            && seen.insert(crate::path_key::normalize_keep_drive(&member.path))
                        {
                            targets.paths.push(member.path.clone().into());
                        } else if !member.is_video {
                            targets.unsupported += 1;
                        }
                    }
                }
                targets
            } else {
                collect_targets(&self.items, 0..self.items.len(), Some(folder))
            }
        } else {
            collect_targets(&self.items, indices, None)
        };
        // 再生と同じメモリ上の選択を受付時に捕捉。設定の非同期保存や後の切替に影響されない。
        for target in &mut targets.paths {
            target.saved_audio_track = self.saved_audio_track_choice_for_path(&target.path);
        }
        targets
    }
    pub(crate) fn transcript_creation_disabled_reason(
        &self,
        background: bool,
    ) -> Option<&'static str> {
        if self.remote_session_blocks_local_control() {
            return Some("リモート閲覧中は文字起こしを作れません");
        }
        if self.transcript_delete.is_some() {
            return Some("文字起こしの削除を処理しています");
        }
        if self.main_chrome_pointer_blocked() {
            return Some("ほかの操作を終えてから文字起こしを作ってください");
        }
        if self
            .transcript_job
            .as_ref()
            .is_some_and(|j| j.state == JobState::Stopping)
        {
            return Some("前のジョブを終えています。終わってから選び直してください");
        }
        if self.transcript_targets(background).paths.is_empty() {
            return Some(crate::context_menu_model::TRANSCRIPT_NO_TARGET_REASON);
        }
        None
    }
    pub(crate) fn request_create_transcripts(&mut self, ctx: &egui::Context, background: bool) {
        if let Some(reason) = self.transcript_creation_disabled_reason(background) {
            self.show_feedback_result(reason.into(), FeedbackKind::Info);
            return;
        }
        let targets = self.transcript_targets(background);
        if let Some(job) = self.transcript_job.as_mut() {
            let unsupported = targets.unsupported;
            let added = job.append(targets);
            self.show_transcript_progress = true;
            self.show_feedback_result(
                format!("文字起こしの作成に追加しました (追加 {added}、対象外 {unsupported})"),
                FeedbackKind::Info,
            );
        } else {
            if targets.unsupported != 0 {
                self.show_feedback_result(
                    format!("文字起こしを作成します (対象外 {})", targets.unsupported),
                    FeedbackKind::Info,
                );
            }
            self.request_asr_setup(ctx, targets);
        }
    }
    pub(crate) fn start_transcript_job(
        &mut self,
        ctx: &egui::Context,
        targets: TranscriptTargets,
        config: TranscriptWorkerConfig,
    ) {
        if self.transcript_delete.is_some()
            || self.transcript_job.is_some()
            || targets.paths.is_empty()
        {
            return;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        match spawn_transcript_worker(config, cancel.clone(), ctx.clone()) {
            Ok(worker) => {
                let mut job = TranscriptJob::new(worker, cancel);
                job.append(targets);
                if let Err(error) = job.advance() {
                    self.show_feedback_result(error, FeedbackKind::Error);
                    return;
                }
                self.transcript_job = Some(Box::new(job));
                self.show_transcript_progress = true;
            }
            Err(error) => self.show_feedback_result(
                format!("文字起こしの作成を始められませんでした: {error}"),
                FeedbackKind::Error,
            ),
        }
    }
    /// 保存済み通知は中止後も採用する。終端まで owner を残して次の受付と重ねない。
    pub(crate) fn poll_transcript_job(&mut self, ctx: &egui::Context) {
        let mut consumed = 0;
        for _ in 0..64 {
            let Some(job) = self.transcript_job.as_mut() else {
                break;
            };
            let mut saved = false;
            let event = job.worker.events.try_recv();
            if event.is_ok() {
                consumed += 1;
            }
            let terminal = match event {
                Ok(TranscriptWorkerEvent::Started(_)) => None,
                Ok(TranscriptWorkerEvent::Progress {
                    processed_ms,
                    duration_ms,
                }) => {
                    if job.state == JobState::Running {
                        job.progress.processed_ms = processed_ms;
                        job.progress.duration_ms = duration_ms;
                    }
                    None
                }
                Ok(TranscriptWorkerEvent::ItemDone(outcome)) => {
                    // T1 では DB の indexed 印が正本。表示 cache と indexer は後段で接続する。
                    if matches!(
                        outcome,
                        TranscriptItemOutcome::Created | TranscriptItemOutcome::FailedWritten
                    ) {
                        saved = true;
                    }
                    job.item_done(outcome).err().map(Err)
                }
                Ok(TranscriptWorkerEvent::Finished(result)) => Some(result),
                Err(mpsc::TryRecvError::Empty) => break,
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("文字起こしの作成処理との接続が切れました".into()))
                }
            };
            if saved {
                self.record_transcript_store_change();
            }
            if let Some(result) = terminal {
                let job = self.transcript_job.take().expect("文字起こしの owner");
                self.show_transcript_progress = false;
                match result {
                    Ok(()) => self.show_feedback_result(job.summary(), FeedbackKind::Info),
                    Err(error) => {
                        crate::logger::log(format!("[ASR] job: {error}"));
                        self.show_feedback_result(
                            format!("文字起こしの作成を停止しました: {error}"),
                            FeedbackKind::Error,
                        );
                    }
                }
                break;
            }
        }
        // worker の repaint はまとめられる。上限で残した通知の続きを root が引き受ける。
        // 空の queue を周期的に監視せず、上限まで読んで owner が残る場合だけ一度予約する。
        if consumed == 64 && self.transcript_job.is_some() {
            ctx.request_repaint();
        }
    }
    pub(crate) fn record_transcript_store_change(&mut self) {
        let mut effects = crate::rename_key_migration::StoreMutationEffects::default();
        effects.record_transcripts_changed();
        self.apply_content_identity_store_mutations(effects);
    }
    pub(crate) fn stop_transcript_job_for_exit(&mut self) {
        self.show_transcript_progress = false;
        if let Some(job) = self.transcript_job.take() {
            job.worker.stop_for_exit();
        }
    }
    pub(crate) fn show_transcript_job_progress(&mut self, ctx: &egui::Context) {
        if !self.show_transcript_progress {
            return;
        }
        let Some(job) = self.transcript_job.as_mut() else {
            self.show_transcript_progress = false;
            return;
        };
        if draw_progress(
            ctx,
            &job.progress,
            job.current.as_ref().map(|target| target.path.as_path()),
            job.state,
            &mut self.show_transcript_progress,
        ) {
            job.stop();
        }
    }
}

fn draw_progress(
    ctx: &egui::Context,
    progress: &Progress,
    current: Option<&Path>,
    state: JobState,
    open: &mut bool,
) -> bool {
    let mut stop = false;
    egui::Window::new("文字起こしを作成")
        .id(egui::Id::unique("transcript_job_progress"))
        .default_pos(ctx.content_rect().min + egui::vec2(60.0, 40.0))
        .default_width(380.0)
        .max_width((ctx.content_rect().width() - 32.0).max(180.0))
        .open(open)
        .collapsible(false)
        .resizable(false)
        .show(ctx, |ui| {
            ui.label(format!("{} / {} 件", progress.done, progress.total));
            ui.add(
                egui::Label::new(
                    current
                        .and_then(Path::file_name)
                        .unwrap_or_default()
                        .to_string_lossy(),
                )
                .truncate(),
            );
            let ratio = if progress.duration_ms == 0 {
                0.0
            } else {
                progress.processed_ms as f32 / progress.duration_ms as f32
            };
            ui.add(egui::ProgressBar::new(ratio.clamp(0.0, 1.0)).show_percentage());
            ui.label(format!(
                "音声 {} / {}",
                crate::ui_helpers::format_hms(progress.processed_ms as f64 / 1000.0),
                if progress.duration_ms == 0 {
                    "不明".into()
                } else {
                    crate::ui_helpers::format_hms(progress.duration_ms as f64 / 1000.0)
                },
            ));
            ui.label(format!(
                "作成 {} / 既にあり {} / 音声なし {}",
                progress.created, progress.already, progress.no_audio
            ));
            ui.label(format!(
                "対象外 {} / 失敗 {} / 変更により破棄 {}",
                progress.unsupported, progress.failed, progress.discarded
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
        TranscriptJob,
        mpsc::Receiver<TranscriptWorkerCommand>,
        mpsc::Sender<TranscriptWorkerEvent>,
    ) {
        let cancel = Arc::new(AtomicBool::new(false));
        let (worker, commands, events) = TranscriptWorker::test_channels(cancel.clone());
        (TranscriptJob::new(worker, cancel), commands, events)
    }

    #[test]
    fn transcript_targets_only_real_media_and_current_folder_children() {
        let items = vec![
            GridItem::Video("C:/media/a.mp4".into()),
            GridItem::Audio("C:/media/b.flac".into()),
            GridItem::Image("C:/media/c.png".into()),
            GridItem::ZipImage {
                zip_path: "C:/media/d.zip".into(),
                entry_name: "voice.mp3".into(),
            },
            GridItem::Video("C:/media/sub/e.mp4".into()),
        ];
        let selected = collect_targets(&items, 0..items.len(), None);
        assert_eq!(selected.paths.len(), 3);
        assert_eq!(selected.unsupported, 2);
        let folder = collect_targets(&items, 0..items.len(), Some(Path::new("C:/media")));
        assert_eq!(
            folder.paths,
            vec![
                TranscriptTarget::from("C:/media/a.mp4"),
                TranscriptTarget::from("C:/media/b.flac")
            ]
        );
        assert_eq!(folder.unsupported, 1);
    }

    #[test]
    fn transcript_targets_without_selection_use_only_current_folder_children() {
        let mut app = crate::app::setup_app_for_test();
        app.current_folder = Some("C:/media".into());
        app.items = vec![
            GridItem::Video("C:/media/a.mp4".into()),
            GridItem::Audio("C:/media/b.flac".into()),
            GridItem::Video("C:/media/sub/c.mp4".into()),
        ];
        app.selected = None;
        assert_eq!(app.transcript_targets(false).paths.len(), 2);
        app.selected = Some(2);
        assert_eq!(
            app.transcript_targets(false).paths,
            vec![TranscriptTarget::from("C:/media/sub/c.mp4")]
        );
        assert_eq!(app.transcript_targets(true).paths.len(), 2);
    }

    #[test]
    fn transcript_queue_append_deduplicates_waiting_and_keeps_caption_independent() {
        let mut app = crate::app::setup_app_for_test();
        let (mut job, commands, _events) = job();
        assert_eq!(
            job.append(TranscriptTargets {
                paths: vec!["a.mp4".into(), "b.mp4".into()],
                unsupported: 1
            }),
            2
        );
        job.advance().unwrap();
        assert!(
            matches!(commands.try_recv().unwrap(),TranscriptWorkerCommand::Item(target) if target.path == Path::new("a.mp4"))
        );
        assert_eq!(
            job.append(TranscriptTargets {
                paths: vec!["b.mp4".into(), "c.wav".into()],
                unsupported: 0
            }),
            1
        );
        app.transcript_job = Some(Box::new(job));
        let caption_cancel = Arc::new(AtomicBool::new(false));
        app.caption_job = Some(super::super::caption_job::CaptionJob::for_progress_test(
            caption_cancel.clone(),
        ));
        app.transcript_job.as_mut().unwrap().stop();
        assert!(!caption_cancel.load(Ordering::Acquire));
        let job = app.transcript_job.as_mut().unwrap();
        assert_eq!(
            job.append(TranscriptTargets {
                paths: vec!["d.wav".into()],
                unsupported: 0
            }),
            0
        );
        assert!(job.queue.is_empty());
        assert!(app.caption_job.is_some());
    }

    #[test]
    fn transcript_targets_capture_playback_memory_choice_before_settings_save() {
        let mut app = crate::app::setup_app_for_test();
        let paths = [
            app.tmp.path().join("movie.mp4"),
            app.tmp.path().join("audio.flac"),
        ];
        app.items = vec![
            GridItem::Video(paths[0].clone()),
            GridItem::Audio(paths[1].clone()),
        ];
        app.checked = [0, 1].into_iter().collect();
        let choice = |stream_index| crate::video::SavedAudioTrackChoice {
            stream_index,
            codec: "aac".into(),
            language: Some("jpn".into()),
            channels: Some(2),
            title: None,
        };
        let old = choice(1);
        let current = choice(2);
        let later = choice(3);
        let db = crate::settings_db::SettingsDb::open(app.tmp.path()).unwrap();
        let mut disk_settings = crate::settings::Settings::default();
        for path in &paths {
            let key = crate::adjustment_db::normalize_path(path);
            disk_settings
                .video_audio_track_choices
                .insert(key.clone(), old.clone());
            app.settings
                .video_audio_track_choices
                .insert(key, current.clone());
        }
        db.save_full(&disk_settings).unwrap();
        let targets = app.transcript_targets(false);
        let (mut job, commands, _events) = job();
        job.append(targets);
        for path in &paths {
            let key = crate::adjustment_db::normalize_path(path);
            assert_eq!(
                db.load_into_settings().unwrap().video_audio_track_choices[&key],
                old
            );
            app.settings
                .video_audio_track_choices
                .insert(key, later.clone());
        }
        job.advance().unwrap();
        let TranscriptWorkerCommand::Item(target) = commands.try_recv().unwrap() else {
            panic!("最初の対象が必要")
        };
        assert_eq!(target.path, paths[0]);
        assert_eq!(target.saved_audio_track, Some(current.clone()));
        assert_eq!(job.queue.front().unwrap().path, paths[1]);
        assert_eq!(job.queue.front().unwrap().saved_audio_track, Some(current));
        assert_eq!(
            app.saved_audio_track_choice_for_path(&paths[0]),
            Some(later)
        );
    }

    #[test]
    fn transcript_stopping_adopts_saved_result_and_finished_but_ignores_progress() {
        let mut app = crate::app::setup_app_for_test();
        let (mut job, _commands, events) = job();
        job.current = Some("a.mp4".into());
        job.stop();
        app.transcript_job = Some(Box::new(job));
        assert!(
            events
                .send(TranscriptWorkerEvent::Progress {
                    processed_ms: 500,
                    duration_ms: 1000,
                })
                .is_ok()
        );
        assert!(
            events
                .send(TranscriptWorkerEvent::ItemDone(
                    TranscriptItemOutcome::Created,
                ))
                .is_ok()
        );
        app.poll_transcript_job(&egui::Context::default());
        let job = app.transcript_job.as_ref().unwrap();
        assert_eq!(job.state, JobState::Stopping);
        assert_eq!(job.progress.processed_ms, 0);
        assert_eq!(job.progress.created, 1);
        assert_eq!(job.progress.done, 1);
        assert!(events.send(TranscriptWorkerEvent::Finished(Ok(()))).is_ok());
        app.poll_transcript_job(&egui::Context::default());
        assert!(app.transcript_job.is_none());
        let feedback = &app.fs_feedback_toast.as_ref().unwrap().0;
        assert!(feedback.contains("中止"));
        assert!(feedback.contains("作成 1"));
    }

    fn root_repaint_delay(output: &egui::FullOutput) -> std::time::Duration {
        output.viewport_output[&egui::ViewportId::ROOT].repaint_delay
    }

    fn idle_repaint_context() -> egui::Context {
        let ctx = egui::Context::default();
        // egui 自身の初期 repaint を固定回数の空 pass で消費する。worker の待機ではない。
        ctx.run_pass(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        ctx.run_pass(egui::RawInput::default(), |_| {})
            .drop_without_applying_deltas();
        let output = ctx.run_pass(egui::RawInput::default(), |_| {});
        assert_eq!(root_repaint_delay(&output), std::time::Duration::MAX);
        output.drop_without_applying_deltas();
        ctx
    }

    #[test]
    fn transcript_bounded_drain_reserves_repaint_and_adopts_saved_terminal_while_stopping() {
        let mut app = crate::app::setup_app_for_test();
        let (mut job, _commands, events) = job();
        job.current = Some("a.mp4".into());
        job.stop();
        app.transcript_job = Some(Box::new(job));
        for processed_ms in 0..200 {
            assert!(
                events
                    .send(TranscriptWorkerEvent::Progress {
                        processed_ms,
                        duration_ms: 1000
                    })
                    .is_ok()
            );
        }
        assert!(
            events
                .send(TranscriptWorkerEvent::ItemDone(
                    TranscriptItemOutcome::Created
                ))
                .is_ok()
        );
        assert!(events.send(TranscriptWorkerEvent::Finished(Ok(()))).is_ok());
        let ctx = idle_repaint_context();
        for _ in 0..3 {
            let output = ctx.run_pass(egui::RawInput::default(), |ctx| {
                app.poll_transcript_job(ctx)
            });
            assert_eq!(root_repaint_delay(&output), std::time::Duration::ZERO);
            output.drop_without_applying_deltas();
            let job = app
                .transcript_job
                .as_ref()
                .expect("上限で残した通知の owner を維持する");
            assert_eq!(job.state, JobState::Stopping);
            assert_eq!(job.progress.processed_ms, 0);
            assert_eq!(job.progress.created, 0);
        }
        ctx.run_pass(egui::RawInput::default(), |ctx| {
            app.poll_transcript_job(ctx)
        })
        .drop_without_applying_deltas();
        assert!(app.transcript_job.is_none());
        let feedback = &app.fs_feedback_toast.as_ref().unwrap().0;
        assert!(feedback.contains("中止"));
        assert!(feedback.contains("作成 1"));
    }

    #[test]
    fn transcript_empty_active_event_queue_does_not_reserve_repaint() {
        let mut app = crate::app::setup_app_for_test();
        let (job, _commands, _events) = job();
        let ctx = idle_repaint_context();
        app.transcript_job = Some(Box::new(job));
        let output = ctx.run_pass(egui::RawInput::default(), |ctx| {
            app.poll_transcript_job(ctx)
        });
        assert_eq!(root_repaint_delay(&output), std::time::Duration::MAX);
        output.drop_without_applying_deltas();
        app.transcript_job.as_mut().unwrap().stop();
        let output = ctx.run_pass(egui::RawInput::default(), |ctx| {
            app.poll_transcript_job(ctx)
        });
        assert_eq!(root_repaint_delay(&output), std::time::Duration::MAX);
        output.drop_without_applying_deltas();
        app.transcript_job = None;
        let output = ctx.run_pass(egui::RawInput::default(), |ctx| {
            app.poll_transcript_job(ctx)
        });
        assert_eq!(root_repaint_delay(&output), std::time::Duration::MAX);
        output.drop_without_applying_deltas();
    }

    #[test]
    fn transcript_terminal_at_drain_limit_does_not_reserve_repaint() {
        let mut app = crate::app::setup_app_for_test();
        let (mut job, _commands, events) = job();
        job.stop();
        app.transcript_job = Some(Box::new(job));
        for processed_ms in 0..63 {
            assert!(
                events
                    .send(TranscriptWorkerEvent::Progress {
                        processed_ms,
                        duration_ms: 1000
                    })
                    .is_ok()
            );
        }
        assert!(events.send(TranscriptWorkerEvent::Finished(Ok(()))).is_ok());
        let ctx = idle_repaint_context();
        let output = ctx.run_pass(egui::RawInput::default(), |ctx| {
            app.poll_transcript_job(ctx)
        });
        assert!(app.transcript_job.is_none());
        assert_eq!(root_repaint_delay(&output), std::time::Duration::MAX);
        output.drop_without_applying_deltas();
    }

    #[test]
    fn transcript_modeless_close_keeps_worker_and_reopen_keeps_progress() {
        let mut app = crate::app::setup_app_for_test();
        let (mut job, _commands, _events) = job();
        job.progress.created = 3;
        app.transcript_job = Some(Box::new(job));
        app.show_transcript_progress = false;
        app.show_transcript_job_progress(&egui::Context::default());
        assert_eq!(app.transcript_job.as_ref().unwrap().progress.created, 3);
        app.show_transcript_progress = true;
        assert!(!app.common_modal_dialog_open());
    }

    #[test]
    fn transcript_progress_small_window_stop_is_clickable() {
        for size in [egui::vec2(1093.0, 614.0), egui::vec2(546.5, 307.0)] {
            for duration_ms in [0, 100000] {
                let mut harness = crate::test_ui::builder().with_context_setup(crate::ui_fonts::configure_fonts).with_size(size).build_ui_state(move |ui, stopped| {*stopped |= draw_progress(ui.ctx(),&Progress {total:10,done:3,created:2,processed_ms:27000,duration_ms,..Default::default()},Some(Path::new("長い名前の動画ファイルです長い名前の動画ファイルです長い名前の動画ファイルです.mp4")),JobState::Running,&mut true);},false);
                harness.run_steps(4);
                let bounds = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
                for button in harness.get_all_by_role(egui::accesskit::Role::Button) {
                    assert!(bounds.contains_rect(button.rect()));
                }
                let time_label = if duration_ms == 0 {
                    "音声 00:27 / 不明"
                } else {
                    "音声 00:27 / 01:40"
                };
                assert!(bounds.contains_rect(harness.get_by_label(time_label).rect()));
                harness.get_by_label("中止").click();
                harness.run_steps(2);
                assert!(*harness.state());
            }
        }
    }

    #[test]
    fn transcript_progress_snapshot() {
        let mut harness = crate::test_ui::builder()
            .with_context_setup(crate::ui_fonts::configure_fonts)
            .with_size(egui::vec2(480.0, 290.0))
            .build_ui(|ui| {
                crate::os_theme::apply_resolved(ui.ctx(), crate::os_theme::ResolvedTheme::Dark);
                draw_progress(
                    ui.ctx(),
                    &Progress {
                        total: 27,
                        done: 12,
                        created: 5,
                        already: 3,
                        no_audio: 1,
                        unsupported: 2,
                        failed: 1,
                        processed_ms: 27000,
                        duration_ms: 100000,
                        ..Default::default()
                    },
                    Some(Path::new("sample.mp4")),
                    JobState::Running,
                    &mut true,
                );
            });
        harness.run_steps(4);
        harness.snapshot("transcript_job_progress");
    }
}

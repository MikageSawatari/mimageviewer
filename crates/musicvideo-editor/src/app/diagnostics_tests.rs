//! Real stdout/stderr locks in an isolated test process simulate a writer stuck
//! on its output. No native window, audio device or normal data directory.
use super::*;
use crate::persistence::{Command, Persistence};

fn stalled_output(
    name: &str,
    stderr_markers: &[&str],
    stdout_marker: Option<&str>,
    exercise: impl FnOnce(Persistence) + Send + 'static,
) {
    const CHILD: &str = "MIV_P4C_STALLED_OUTPUT_TEST";
    if std::env::var(CHILD).as_deref() == Ok(name) {
        let (locked_tx, locked_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let output_owner = std::thread::spawn(move || {
            let stderr = std::io::stderr();
            let stdout = std::io::stdout();
            let _stderr = stderr.lock();
            let _stdout = stdout.lock();
            locked_tx.send(()).unwrap();
            // Bounded even if the test's handler panics. This is only a test sink.
            let _ = release_rx.recv_timeout(Duration::from_secs(15));
        });
        locked_rx.recv_timeout(Duration::from_secs(5)).unwrap();
        let (services, guard) = Persistence::start(None).unwrap();
        services
            .tx
            .send(Command::Log("stalled writer primer".into()))
            .unwrap();
        let (done_tx, done_rx) = mpsc::channel();
        let handler = std::thread::spawn(move || {
            exercise(services);
            done_tx.send(()).unwrap();
        });
        let completed = done_rx.recv_timeout(Duration::from_secs(3));
        // Unblock before assertions/joins, so a broken direct print is a failed
        // test rather than a stuck test harness.
        let _ = release_tx.send(());
        output_owner.join().unwrap();
        handler.join().unwrap();
        drop(guard); // Normal post-GUI drain, after the test output is released.
        assert!(
            completed.is_ok(),
            "GUI handler waited for stdout/stderr: {completed:?}"
        );
        return;
    }
    // --nocapture is essential: direct eprintln must hit the real stream lock,
    // not the test harness's thread-local output buffer.
    let output = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", name, "--nocapture"])
        .env(CHILD, name)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "child failed:\n{}\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    for marker in stderr_markers {
        assert!(
            stderr.contains(marker),
            "missing worker output: {marker}\n{stderr}"
        );
    }
    if let Some(marker) = stdout_marker {
        assert!(String::from_utf8_lossy(&output.stdout).contains(marker));
    }
}

fn app(services: Persistence) -> MusicVideoLabApp {
    let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
    app.configure_startup(
        crate::startup::StartupArgs::default(),
        Arc::new(crate::startup::RuntimeAdmission::Ready { _lease: None }),
        Some(services),
        None,
        &crate::project::test_context(),
    );
    app
}

#[test]
fn p4c_review_save_failure_returns_with_stalled_output() {
    stalled_output(
        "app::diagnostics_tests::p4c_review_save_failure_returns_with_stalled_output",
        &["stalled save failure"],
        None,
        |services| {
            let temp = tempfile::tempdir().unwrap();
            let ctx = crate::project::test_context();
            let mut app = app(services);
            app.accept_document_intent(DocumentIntent::Close);
            let (tx, rx) = mpsc::channel();
            app.project_save_pending = Some(ProjectSavePending::Writing(ProjectSaveWrite {
                checkpoint: app
                    .document
                    .begin_save(temp.path().join("saved.json"), &app.project),
                rx,
            }));
            tx.send(Err("stalled save failure".into())).unwrap();
            app.poll_projects(&ctx);
            assert!(app.project_save_pending.is_none());
            assert!(!app.intent_pending());
            assert_eq!(app.status, "stalled save failure");
        },
    );
}

#[test]
fn p4c_review_mp4_failure_returns_with_stalled_output() {
    stalled_output(
        "app::diagnostics_tests::p4c_review_mp4_failure_returns_with_stalled_output",
        &["stalled MP4 failure"],
        None,
        |services| {
            let temp = tempfile::tempdir().unwrap();
            let mut app = app(services);
            let (_tx, rx) = mpsc::channel();
            app.export_state = ExportState::Running(ExportPending {
                kind: ExportKind::Mp4 {
                    stage: Arc::new(AtomicUsize::new(3)),
                    output_path: temp.path().join("output.mp4"),
                },
                cancel: Arc::new(AtomicBool::new(false)),
                progress: Arc::new(AtomicUsize::new(0)),
                total_frames: 1,
                rx,
            });
            app.complete_mp4_export(Err(mimageviewer::video::mvexport::ExportError::Io(
                "stalled MP4 failure".into(),
            )));
            assert!(matches!(app.export_state, ExportState::Idle));
            assert!(app.preview_dirty);
            assert!(app.status.contains("stalled MP4 failure"));
        },
    );
}

#[test]
fn p4c_review_mp4_choice_disconnect_returns_with_stalled_output() {
    stalled_output(
        "app::diagnostics_tests::p4c_review_mp4_choice_disconnect_returns_with_stalled_output",
        &["保存先ダイアログが終了しました"],
        None,
        |services| {
            let mut app = app(services);
            let (tx, rx) = mpsc::channel();
            app.export_state = ExportState::Choosing {
                settings: crate::mp4::ExportSettings::default(),
                rx,
            };
            drop(tx);
            app.poll_mp4_choice(&crate::project::test_context());
            assert!(matches!(app.export_state, ExportState::Idle));
            assert_eq!(app.status, "保存先ダイアログが終了しました");
        },
    );
}

#[test]
fn p4c_review_audio_command_disconnect_returns_with_stalled_output() {
    stalled_output(
        "app::diagnostics_tests::p4c_review_audio_command_disconnect_returns_with_stalled_output",
        &["音楽ビデオの音声出力エラー: 音声スレッドが終了しました"],
        None,
        |services| {
            let mut app = MusicVideoLabApp::new(Arc::new(Assets::default()));
            app.audio_player = Some(AudioPlayer::new_disconnected_headless());
            let ctx = crate::project::test_context();
            app.configure_startup(
                crate::startup::StartupArgs::default(),
                Arc::new(crate::startup::RuntimeAdmission::Ready { _lease: None }),
                Some(services),
                None,
                &ctx,
            );
            app.apply_chrome_actions(&ctx, vec![ChromeAction::TogglePlayback]);
            app.poll_playback_error(&ctx);
            assert_eq!(
                app.playback_error.as_deref(),
                Some("音声スレッドが終了しました")
            );
            assert!(app.status.contains("音声スレッドが終了しました"));
        },
    );
}

#[test]
fn p4c_review_preview_benchmark_returns_with_stalled_stdout() {
    stalled_output(
        "app::diagnostics_tests::p4c_review_preview_benchmark_returns_with_stalled_stdout",
        &[],
        Some("musicvideo preview benchmark: frames=1"),
        |services| {
            let mut app = app(services);
            app.preview_benchmark_target = Some(1);
            app.preview_perf.push(PreviewPerfSample {
                display_interval_ms: 16.0,
                ..Default::default()
            });
            app.finish_preview_benchmark(&crate::project::test_context());
            assert!(app.preview_benchmark_target.is_none());
        },
    );
}

#[test]
fn p4c_review_gui_bootstrap_errors_return_with_stalled_output() {
    stalled_output(
        "app::diagnostics_tests::p4c_review_gui_bootstrap_errors_return_with_stalled_output",
        &["フォントを読み込めません", "stalled GUI creation failure"],
        None,
        |services| {
            let font = crate::project::FontData {
                bytes: vec![1, 2, 3],
            };
            let assets = crate::create_gui_assets(Some(&font), Some(&services.tx));
            assert!(assets.fonts.is_empty());
            let result = Err(eframe::Error::AppCreation(Box::new(std::io::Error::other(
                "stalled GUI creation failure",
            ))));
            crate::report_gui_result(&result, Some(&services.tx));
            assert!(result.is_err());
        },
    );
}

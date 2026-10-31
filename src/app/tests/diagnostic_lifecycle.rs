//! Headless lifecycle connections. Native inference, native tray windows and
//! product executables are deliberately absent from these fixtures.

use super::phase_c_support;
use crate::app::{AiJobQueue, AiJobQueueState, run_ai_worker};
use crate::previous_run_diagnostics::{GuiRunDiagnostics, GuiRunOutcome, test_support};
use std::sync::{Arc, Condvar, Mutex, mpsc};
use std::time::{Duration, Instant};

fn finish(owner: &mut GuiRunDiagnostics) {
    assert!(owner.finish_until(
        GuiRunOutcome::ReturnedOk,
        Instant::now() + Duration::from_secs(20),
    ));
}

#[test]
fn headless_root_close_hides_to_tray_and_app_drop_keep_run_running() {
    let mut app = phase_c_support::setup_app();
    let directory = tempfile::tempdir().unwrap();
    let mut owner = GuiRunDiagnostics::new();
    test_support::admit(&mut owner, directory.path());
    app.settings.minimize_to_tray_on_close = true;
    app.tray_controller = Some(crate::tray::TrayController::controller_for_test());
    assert!(app.window_visible);
    #[cfg(windows)]
    assert!(
        app.main_hwnd.is_none(),
        "fixture must never manipulate a real HWND"
    );
    let ctx = egui::Context::default();
    let mut input = egui::RawInput::default();
    input
        .viewports
        .entry(egui::ViewportId::ROOT)
        .or_default()
        .events
        .push(egui::ViewportEvent::Close);
    let mut output = ctx.run_pass(input, |ctx| app.service_root_lifecycle(ctx));
    output.textures_delta.clear();
    assert!(
        output.viewport_output[&egui::ViewportId::ROOT]
            .commands
            .contains(&egui::ViewportCommand::CancelClose)
    );
    assert!(!app.window_visible, "the real tray policy must have run");
    assert!(test_support::is_running(directory.path()));

    // AppTestEnv owns App before its override guard/tempdir, so its actual
    // destructor and worker teardown still run inside the isolated profile.
    drop(app);
    assert!(
        test_support::is_running(directory.path()),
        "App Drop must not claim a clean GUI return"
    );
    finish(&mut owner);
    assert_eq!(
        test_support::terminal(directory.path()),
        Some(GuiRunOutcome::ReturnedOk)
    );
}

#[test]
fn diagnostic_finalization_does_not_wait_for_detached_ai_worker_gate() {
    let app = phase_c_support::setup_app();
    let directory = tempfile::tempdir().unwrap();
    let mut owner = GuiRunDiagnostics::new();
    test_support::admit(&mut owner, directory.path());
    let inner = Arc::new((
        Mutex::new(AiJobQueueState {
            display: Default::default(),
            prefetch: Default::default(),
            shutdown: false,
        }),
        Condvar::new(),
    ));
    let queue = AiJobQueue {
        inner: Arc::clone(&inner),
    };
    let (ready_tx, ready_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (exited_tx, exited_rx) = mpsc::channel();
    let (result_tx, result_rx) = mpsc::channel();
    let worker_inner = Arc::clone(&inner);
    let worker = std::thread::spawn(move || {
        ready_tx.send(()).unwrap();
        // An explicit test gate keeps the real worker alive without executing
        // an ORT job. On release its real dispatcher observes queue shutdown.
        if release_rx.recv().is_ok() {
            run_ai_worker(worker_inner, result_tx);
        }
        let _ = exited_tx.send(());
    });
    ready_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    // Bound this assertion even if a future Drop starts waiting for a worker.
    let (dropped_tx, dropped_rx) = mpsc::channel();
    let dropper = std::thread::spawn(move || {
        drop(queue); // Actual Drop, not a copied shutdown implementation.
        let _ = dropped_tx.send(());
    });
    if dropped_rx.recv_timeout(Duration::from_secs(20)).is_err() {
        let _ = release_tx.send(());
        let _ = exited_rx.recv_timeout(Duration::from_secs(20));
        panic!("AiJobQueue::Drop waited for the blocked AI worker");
    }
    dropper.join().unwrap();
    assert!(inner.0.lock().unwrap().shutdown);
    drop(app);
    assert!(test_support::is_running(directory.path()));
    finish(&mut owner);
    assert_eq!(
        test_support::terminal(directory.path()),
        Some(GuiRunOutcome::ReturnedOk)
    );
    assert!(
        matches!(exited_rx.try_recv(), Err(mpsc::TryRecvError::Empty)),
        "normal GUI finalization must not require AI worker completion"
    );
    release_tx.send(()).unwrap();
    exited_rx.recv_timeout(Duration::from_secs(20)).unwrap();
    worker.join().unwrap();
    assert!(
        result_rx.try_recv().is_err(),
        "fixture must not run inference"
    );
}

#[test]
fn ai_queue_drop_and_worker_dispatch_remain_detached_from_gui_termination() {
    let source = include_str!("../../app.rs");
    let queue_impl = source
        .split_once("impl AiJobQueue {")
        .unwrap()
        .1
        .split_once("impl Drop for AiJobQueue")
        .unwrap()
        .0;
    assert!(queue_impl.contains(".spawn(move || run_ai_worker(worker_inner, tx))"));
    let drop_impl = source
        .split_once("impl Drop for AiJobQueue {")
        .unwrap()
        .1
        .split_once("fn run_ai_worker(")
        .unwrap()
        .0;
    let code = drop_impl
        .lines()
        .filter(|line| !line.trim_start().starts_with("//"))
        .collect::<String>();
    assert!(code.contains("q.shutdown = true;"));
    assert!(code.contains("cv.notify_all();"));
    assert!(!code.contains(".join("));
    assert!(!code.contains("finish_gui_run"));
    assert!(!code.contains("finish_until"));
}

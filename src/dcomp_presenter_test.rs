use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use serde_json::Value;
use windows::Win32::System::Com::{COINIT_MULTITHREADED, CoInitializeEx, CoUninitialize};

use crate::video::engine::actor::state_code;
use crate::video::gpu_renderer::GpuVideoDevice;
use crate::video::native_presenter::{
    NativePresentOutcome, NativePresenterConfig, NativeVideoPresenter,
};
use crate::video::native_window::{
    NativeVideoWindow, NativeVideoWindowConfig, NativeVideoWindowMode,
};

#[derive(Clone, Debug)]
pub struct DcompPresenterTestConfig {
    pub path: PathBuf,
    pub duration: Duration,
    pub width: u32,
    pub height: u32,
    pub sync_interval: u32,
    pub start_secs: f64,
}

pub fn parse_config() -> Option<DcompPresenterTestConfig> {
    let args: Vec<String> = std::env::args().collect();
    let mut path = None;
    let mut duration = Duration::from_secs(10);
    let mut width = 1920u32;
    let mut height = 1080u32;
    let mut sync_interval = 1u32;
    let mut start_secs = 0.0f64;
    let mut i = 1usize;
    while i < args.len() {
        match args[i].as_str() {
            "--dcomp-presenter-test" => {
                if let Some(v) = args.get(i + 1) {
                    path = Some(PathBuf::from(v));
                    i += 1;
                }
            }
            "--dcomp-duration" => {
                if let Some(v) = args.get(i + 1).and_then(|s| s.parse::<f64>().ok()) {
                    duration = Duration::from_secs_f64(v.max(0.1));
                    i += 1;
                }
            }
            "--dcomp-window-size" => {
                if let Some(v) = args.get(i + 1)
                    && let Some((w, h)) = parse_size(v)
                {
                    width = w;
                    height = h;
                    i += 1;
                }
            }
            "--dcomp-sync-interval" => {
                if let Some(v) = args.get(i + 1).and_then(|s| s.parse::<u32>().ok()) {
                    sync_interval = v.min(4);
                    i += 1;
                }
            }
            "--dcomp-start" => {
                if let Some(v) = args.get(i + 1).and_then(|s| s.parse::<f64>().ok()) {
                    start_secs = v.max(0.0);
                    i += 1;
                }
            }
            _ => {}
        }
        i += 1;
    }
    path.map(|path| DcompPresenterTestConfig {
        path,
        duration,
        width,
        height,
        sync_interval,
        start_secs,
    })
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x').or_else(|| s.split_once('X'))?;
    let w = w.parse::<u32>().ok()?.clamp(64, 16384);
    let h = h.parse::<u32>().ok()?.clamp(64, 16384);
    Some((w, h))
}

pub fn run(config: DcompPresenterTestConfig) -> Result<(), String> {
    let _com = ComApartment::init()?;
    let (event_tx, event_rx) = std::sync::mpsc::channel();
    let mut window = NativeVideoWindow::create(NativeVideoWindowConfig {
        mode: NativeVideoWindowMode::Windowed {
            width: config.width,
            height: config.height,
        },
        close_on_escape: true,
        post_quit_on_destroy: true,
        event_tx: Some(event_tx),
    })?;
    let gpu = GpuVideoDevice::new().map_err(|e| e.to_string())?;
    let mut presenter = NativeVideoPresenter::new(NativePresenterConfig {
        hwnd: window.hwnd(),
        width: config.width,
        height: config.height,
        test_overlay: std::env::var_os("MIV_NATIVE_VIDEO_TEST_OVERLAY").is_some(),
        egui_overlay: std::env::var_os("MIV_NATIVE_VIDEO_EGUI_OVERLAY").is_some(),
    })?;

    let seek_serial = Arc::new(AtomicU64::new(0));
    let clock = Arc::new(crate::video::clock::AvClock::new(1.0, seek_serial));
    clock.set_muted(true);
    clock.set_fallback_anchor(config.start_secs);
    clock.request_seek(config.start_secs);
    clock.set_playing(true);

    let cancel = Arc::new(AtomicBool::new(false));
    let engine_state = Arc::new(AtomicU8::new(state_code::PLAYING));
    let skipped_frame_count = Arc::new(AtomicU64::new(0));
    let (engine_event_tx, _engine_event_rx) = crossbeam_channel::unbounded();
    let handles = crate::video::decoder::spawn(
        config.path.clone(),
        Arc::clone(&clock),
        Arc::clone(&cancel),
        48_000,
        true,
        Some(Arc::clone(&gpu)),
        engine_state,
        engine_event_tx,
        Arc::clone(&skipped_frame_count),
    );
    let audio_clock = Arc::clone(&clock);
    let audio_rx = handles.audio_rx.clone();
    let audio_cancel = Arc::clone(&cancel);
    let audio_drain = std::thread::Builder::new()
        .name("dcomp-audio-drain".into())
        .spawn(move || {
            let mut active_serial = None;
            while !audio_cancel.load(Ordering::Acquire) {
                match audio_rx.recv_timeout(Duration::from_millis(50)) {
                    Ok(frame) => {
                        if active_serial != Some(frame.seek_serial) {
                            active_serial = Some(frame.seek_serial);
                            audio_clock.notify_audio_active();
                            audio_clock.set_audio_pts_jump(frame.pts_secs);
                            audio_clock.clear_seek_target_override(frame.seek_serial);
                        } else {
                            audio_clock.set_audio_pts(frame.pts_secs);
                        }
                        audio_clock.add_audio_tx_queued_secs(-frame.duration_secs);
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                }
            }
        })
        .map_err(|e| format!("spawn audio drain: {e}"))?;

    let info = handles
        .info_rx
        .recv_timeout(Duration::from_secs(10))
        .map_err(|e| format!("video info timeout: {e}"))?
        .map_err(|e| format!("video open: {e}"))?;
    log_event(
        "open",
        &[
            ("path", Value::from(config.path.display().to_string())),
            ("width", Value::from(info.width as i64)),
            ("height", Value::from(info.height as i64)),
            ("avg_fps", Value::from(info.avg_fps)),
            ("video_codec", Value::from(info.video_codec.clone())),
            ("video_decoder", Value::from(info.video_decoder.clone())),
            ("hw_decode_active", Value::from(info.hw_decode_active)),
            ("gpu_path_active", Value::from(info.gpu_path_active)),
            ("sync_interval", Value::from(config.sync_interval as i64)),
        ],
    );

    let mut stats = PresentStats::default();
    let mut frame_queue = VecDeque::new();
    let mut timeline_base_pts = None;
    let mut timeline_started_at = None;
    let run_started = Instant::now();
    let mut last_present_wall = None;
    let mut quit = false;
    while !quit
        && timeline_started_at
            .map(|started: Instant| started.elapsed() < config.duration)
            .unwrap_or_else(|| run_started.elapsed() < config.duration)
    {
        quit = crate::video::native_window::pump_thread_messages();
        let mut native_events = Vec::new();
        while let Ok(event) = event_rx.try_recv() {
            native_events.push(event);
        }
        if !native_events.is_empty() {
            presenter.handle_window_events(&native_events)?;
        }

        while let Ok(frame) = handles.video_rx.try_recv() {
            if timeline_base_pts.is_none() {
                timeline_base_pts = Some(frame.pts_secs);
                timeline_started_at = Some(Instant::now());
                log_event(
                    "timeline_latch",
                    &[("base_pts", Value::from(frame.pts_secs))],
                );
            }
            frame_queue.push_back(frame);
        }

        let elapsed = timeline_started_at
            .map(|started: Instant| started.elapsed().as_secs_f64() + config.start_secs)
            .unwrap_or(config.start_secs);
        if let Some(frame) = frame_queue.front()
            && frame.pts_secs - timeline_base_pts.unwrap_or(frame.pts_secs) <= elapsed + 0.001
        {
            let frame = frame_queue.pop_front().unwrap();
            let frame_elapsed = frame.pts_secs - timeline_base_pts.unwrap_or(frame.pts_secs);
            let late_ms = ((elapsed - frame_elapsed) * 1000.0).max(0.0);
            if late_ms > 50.0 && frame_queue.len() > 1 {
                stats.late_drop += 1;
                log_event(
                    "late_drop",
                    &[
                        ("pts", Value::from(frame.pts_secs)),
                        ("late_ms", Value::from(late_ms)),
                        ("queue_len", Value::from(frame_queue.len() as i64)),
                    ],
                );
                continue;
            }

            let present_t0 = Instant::now();
            let outcome = presenter.present(&frame, config.sync_interval)?;
            let total_ms = present_t0.elapsed().as_secs_f64() * 1000.0;
            let interval_ms = last_present_wall
                .map(|last: Instant| {
                    present_t0.saturating_duration_since(last).as_secs_f64() * 1000.0
                })
                .unwrap_or(0.0);
            last_present_wall = Some(present_t0);
            stats.record(&outcome, late_ms, total_ms, interval_ms);
            log_event(
                "present",
                &[
                    ("pts", Value::from(frame.pts_secs)),
                    ("frame_elapsed", Value::from(frame_elapsed)),
                    ("late_ms", Value::from(late_ms)),
                    ("queue_len", Value::from(frame_queue.len() as i64)),
                    ("path", Value::from(outcome.path)),
                    ("wait_ms", Value::from(outcome.wait_ms)),
                    ("fence_wait_ms", Value::from(outcome.fence_wait_ms)),
                    ("copy_ms", Value::from(outcome.copy_ms)),
                    ("present_ms", Value::from(outcome.present_ms)),
                    ("total_ms", Value::from(total_ms)),
                    ("interval_ms", Value::from(interval_ms)),
                ],
            );
        } else {
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    cancel.store(true, Ordering::Release);
    let _ = audio_drain.join();
    stats.emit_summary(config.duration);
    window.destroy();
    Ok(())
}

struct ComApartment;

impl ComApartment {
    fn init() -> Result<Self, String> {
        unsafe {
            CoInitializeEx(None, COINIT_MULTITHREADED)
                .ok()
                .map_err(|e| format!("CoInitializeEx: {e:?}"))?;
        }
        Ok(Self)
    }
}

impl Drop for ComApartment {
    fn drop(&mut self) {
        unsafe {
            CoUninitialize();
        }
    }
}

#[derive(Default)]
struct PresentStats {
    presented: u64,
    gpu: u64,
    cpu: u64,
    late_drop: u64,
    wait_timeout: u64,
    max_late_ms: f64,
    max_total_ms: f64,
    max_interval_ms: f64,
}

impl PresentStats {
    fn record(
        &mut self,
        outcome: &NativePresentOutcome,
        late_ms: f64,
        total_ms: f64,
        interval_ms: f64,
    ) {
        self.presented += 1;
        match outcome.path {
            "d3d11_shared" => self.gpu += 1,
            "cpu_upload" => self.cpu += 1,
            _ => {}
        }
        if outcome.wait_timed_out {
            self.wait_timeout += 1;
        }
        self.max_late_ms = self.max_late_ms.max(late_ms);
        self.max_total_ms = self.max_total_ms.max(total_ms);
        self.max_interval_ms = self.max_interval_ms.max(interval_ms);
    }

    fn emit_summary(&self, duration: Duration) {
        let actual_fps = if duration.as_secs_f64() > 0.0 {
            self.presented as f64 / duration.as_secs_f64()
        } else {
            0.0
        };
        log_event(
            "summary",
            &[
                ("presented", Value::from(self.presented as i64)),
                ("gpu_frames", Value::from(self.gpu as i64)),
                ("cpu_frames", Value::from(self.cpu as i64)),
                ("late_drop", Value::from(self.late_drop as i64)),
                ("wait_timeout", Value::from(self.wait_timeout as i64)),
                ("actual_fps", Value::from(actual_fps)),
                ("max_late_ms", Value::from(self.max_late_ms)),
                ("max_total_ms", Value::from(self.max_total_ms)),
                ("max_interval_ms", Value::from(self.max_interval_ms)),
            ],
        );
        crate::logger::log(format!(
            "dcomp-presenter-test summary: presented={} fps={:.1} gpu={} cpu={} late_drop={} max_late_ms={:.1} max_interval_ms={:.1}",
            self.presented,
            actual_fps,
            self.gpu,
            self.cpu,
            self.late_drop,
            self.max_late_ms,
            self.max_interval_ms
        ));
    }
}

fn log_event(kind: &str, fields: &[(&str, Value)]) {
    crate::perf::event("native_presenter", kind, None, 0, fields);
}

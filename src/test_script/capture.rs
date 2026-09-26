//! Test-script-only viewport screenshots. The UI thread only routes egui's
//! screenshot result; PNG encoding and evidence writes run on worker threads.

use std::collections::HashMap;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::os::windows::fs::MetadataExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use super::{TestScriptWindowPresentation, TestScriptWindowSnapshot};
use eframe::egui;
use image::ImageEncoder;

const REPARSE_POINT: u32 = 0x400;
pub(super) const EXPLICIT_TIMEOUT: Duration = Duration::from_secs(5);
pub(super) const FAILURE_TIMEOUT: Duration = Duration::from_millis(1500);
const MAX_CAPTURES: u64 = 64;
const MAX_PIXELS: usize = 64 * 1024 * 1024;
const PARKED_FROZEN_SKIP_REASON: &str = "parked frozen view (not re-rendered by egui)";

#[derive(Clone, Debug)]
pub(super) struct Target {
    pub viewport_id: egui::ViewportId,
    pub role: String,
    /// The already-verified detached host. Root paints with its normal egui wake.
    pub hwnd: Option<u64>,
    pub availability: Availability,
    pub presentation: TestScriptWindowPresentation,
}

/// Derived from eframe's native viewport table in the current root input.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum Availability {
    Registered,
    Absent,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Readiness {
    Renderable,
    Skipped(&'static str),
}

#[derive(Clone, Copy, Debug)]
struct CaptureToken {
    run_nonce: u64,
    shot_id: u64,
}

#[derive(Debug)]
struct Shot {
    batch_id: u64,
    viewport_id: egui::ViewportId,
    role: String,
    label: String,
    file_name: String,
}

struct Batch {
    remaining: usize,
    errors: Vec<String>,
    reply: Option<mpsc::SyncSender<Result<(), String>>>,
    deadline: Instant,
}

struct WriteResult {
    batch_id: u64,
    result: Result<(), String>,
}

struct Output {
    dir: PathBuf,
    manifest: Mutex<File>,
}

pub(super) struct Coordinator {
    output: Arc<Output>,
    run_nonce: u64,
    next_batch: u64,
    next_shot: u64,
    shots: HashMap<u64, Shot>,
    batches: HashMap<u64, Batch>,
    probe_epoch: u64,
    pass_epoch: HashMap<egui::ViewportId, (u64, &'static str)>,
    write_tx: mpsc::Sender<WriteResult>,
    write_rx: mpsc::Receiver<WriteResult>,
}

fn no_reparse(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path)
        .map_err(|error| format!("cannot inspect {}: {error}", path.display()))?;
    if !metadata.is_dir() || metadata.file_attributes() & REPARSE_POINT != 0 {
        return Err(format!(
            "refusing non-directory or reparse point: {}",
            path.display()
        ));
    }
    Ok(())
}

fn validate_run_dir_under(runs_root: &Path, run_dir: &Path) -> Result<PathBuf, String> {
    no_reparse(runs_root)?;
    no_reparse(run_dir)?;
    let root = fs::canonicalize(runs_root)
        .map_err(|error| format!("cannot resolve screenshot runs root: {error}"))?;
    let run = fs::canonicalize(run_dir)
        .map_err(|error| format!("cannot resolve screenshot run directory: {error}"))?;
    if run.parent() != Some(root.as_path()) {
        return Err(format!(
            "screenshot directory must be one run below {}: {}",
            root.display(),
            run.display()
        ));
    }
    Ok(run)
}

fn native_readiness(target: &Target) -> Result<Readiness, String> {
    use windows::Win32::Foundation::HWND;
    use windows::Win32::UI::WindowsAndMessaging::{IsIconic, IsWindow, IsWindowVisible};

    if target.presentation == TestScriptWindowPresentation::PassiveDeferredFrozen {
        return Ok(Readiness::Skipped(PARKED_FROZEN_SKIP_REASON));
    }
    if target.availability == Availability::Absent {
        return Ok(Readiness::Skipped(
            "viewport absent from eframe native render registry",
        ));
    }
    let Some(raw) = target.hwnd else {
        return Ok(Readiness::Renderable);
    };
    let hwnd = HWND(raw as *mut _);
    if !unsafe { IsWindow(Some(hwnd)) }.as_bool() {
        return Err(format!(
            "capture host is no longer a window: {}",
            target.role
        ));
    }
    if !unsafe { IsWindowVisible(hwnd) }.as_bool() {
        return Ok(Readiness::Skipped("native viewport is hidden"));
    }
    if unsafe { IsIconic(hwnd) }.as_bool() {
        return Ok(Readiness::Skipped("native viewport is minimized"));
    }
    Ok(Readiness::Renderable)
}

fn request_capture_repaint(ctx: &egui::Context, target: &Target) -> Result<(), String> {
    if eframe::miv_test_script_window_witness::capture_probe_detail_allowed() {
        crate::logger::log(format!(
            "[capture-probe] repaint_request role={} viewport={:?} source=root_context",
            target.role, target.viewport_id
        ));
    }
    ctx.request_repaint_of(target.viewport_id);
    Ok(())
}

impl Coordinator {
    pub(super) fn new(run_dir: &Path) -> Result<Self, String> {
        let repository = Path::new(env!("CARGO_MANIFEST_DIR"));
        no_reparse(repository)?;
        no_reparse(&repository.join("target"))?;
        let runs_root = repository.join("target").join("ui-smoke-runs");
        let run = validate_run_dir_under(&runs_root, run_dir)?;
        let dir = run.join("screenshots");
        fs::create_dir(&dir)
            .map_err(|error| format!("cannot create {}: {error}", dir.display()))?;
        no_reparse(&dir)?;
        let manifest_path = dir.join("manifest.jsonl");
        let manifest = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&manifest_path)
            .map_err(|error| format!("cannot create {}: {error}", manifest_path.display()))?;
        let (write_tx, write_rx) = mpsc::channel();
        let run_nonce = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos() as u64
            ^ u64::from(std::process::id());
        Ok(Self {
            output: Arc::new(Output {
                dir,
                manifest: Mutex::new(manifest),
            }),
            run_nonce,
            next_batch: 0,
            next_shot: 0,
            shots: HashMap::new(),
            batches: HashMap::new(),
            probe_epoch: 0,
            pass_epoch: HashMap::new(),
            write_tx,
            write_rx,
        })
    }

    pub(super) fn request(
        &mut self,
        ctx: &egui::Context,
        label: &str,
        targets: Vec<Target>,
        timeout: Duration,
        reply: Option<mpsc::SyncSender<Result<(), String>>>,
    ) -> Result<u64, String> {
        self.request_with_repaint(
            ctx,
            label,
            targets,
            timeout,
            reply,
            native_readiness,
            request_capture_repaint,
        )
    }

    fn request_with_repaint(
        &mut self,
        ctx: &egui::Context,
        label: &str,
        targets: Vec<Target>,
        timeout: Duration,
        reply: Option<mpsc::SyncSender<Result<(), String>>>,
        readiness: impl Fn(&Target) -> Result<Readiness, String>,
        mut repaint: impl FnMut(&egui::Context, &Target) -> Result<(), String>,
    ) -> Result<u64, String> {
        if label.is_empty()
            || label.len() > 48
            || !label
                .bytes()
                .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
        {
            return Err("capture label must contain 1-48 ASCII letters, digits, '_' or '-'".into());
        }
        if targets.is_empty() {
            return Err("capture has no current mImageViewer viewport".into());
        }
        if self.next_batch >= MAX_CAPTURES {
            return Err(format!(
                "capture limit of {MAX_CAPTURES} checkpoints reached"
            ));
        }
        // Determine capture eligibility before publishing commands. Residence
        // alone is insufficient: only the App's parked frozen presentation is
        // skipped, while other registered viewports retain timeout failures.
        let targets = targets
            .into_iter()
            .map(|target| readiness(&target).map(|state| (target, state)))
            .collect::<Result<Vec<_>, _>>()?;
        self.next_batch += 1;
        let batch_id = self.next_batch;
        self.batches.insert(
            batch_id,
            Batch {
                remaining: targets.len(),
                errors: Vec::new(),
                reply,
                deadline: Instant::now() + timeout,
            },
        );
        eframe::miv_test_script_window_witness::set_capture_probe(true);
        for (target, state) in targets {
            if eframe::miv_test_script_window_witness::capture_probe_detail_allowed() {
                crate::logger::log(format!(
                    "[capture-probe] target batch={batch_id} role={} viewport={:?} hwnd={:?} availability={:?} presentation={:?} readiness={:?}",
                    target.role,
                    target.viewport_id,
                    target.hwnd,
                    target.availability,
                    target.presentation,
                    state
                ));
            }
            if let Readiness::Skipped(reason) = state {
                let output = Arc::clone(&self.output);
                let tx = self.write_tx.clone();
                let wake_ctx = ctx.clone();
                let role = target.role;
                let label = label.to_owned();
                let frame = ctx.cumulative_frame_nr();
                if let Err(error) = std::thread::Builder::new()
                    .name("test-script-screenshot-skip".into())
                    .spawn(move || {
                        let result = output.write_skipped(&label, &role, reason, frame);
                        let _ = tx.send(WriteResult { batch_id, result });
                        wake_ctx.request_repaint_of(egui::ViewportId::ROOT);
                    })
                {
                    let _ = self.write_tx.send(WriteResult {
                        batch_id,
                        result: Err(format!("cannot spawn screenshot manifest writer: {error}")),
                    });
                }
                continue;
            }
            self.next_shot += 1;
            let shot_id = self.next_shot;
            let file_name = format!("{batch_id:02}-{label}-{}.png", target.role);
            self.shots.insert(
                shot_id,
                Shot {
                    batch_id,
                    viewport_id: target.viewport_id,
                    role: target.role.clone(),
                    label: label.to_owned(),
                    file_name,
                },
            );
            ctx.send_viewport_cmd_to(
                target.viewport_id,
                egui::ViewportCommand::Screenshot(egui::UserData::new(CaptureToken {
                    run_nonce: self.run_nonce,
                    shot_id,
                })),
            );
            if eframe::miv_test_script_window_witness::capture_probe_detail_allowed() {
                crate::logger::log(format!(
                    "[capture-probe] issue batch={batch_id} shot={shot_id} role={} viewport={:?} run_nonce={} command=Screenshot",
                    target.role, target.viewport_id, self.run_nonce
                ));
            }
            if let Err(error) = repaint(ctx, &target) {
                self.shots.remove(&shot_id);
                let _ = self.write_tx.send(WriteResult {
                    batch_id,
                    result: Err(format!(
                        "capture repaint for {} failed: {error}",
                        target.role
                    )),
                });
            }
        }
        // A missing screenshot event must wake the root viewport so poll() can
        // expire the batch even when no other input arrives.
        ctx.request_repaint_after_for(timeout, egui::ViewportId::ROOT);
        Ok(batch_id)
    }

    pub(super) fn begin_root_frame(&mut self, windows: &[TestScriptWindowSnapshot]) {
        let previous_epoch = self.probe_epoch;
        self.probe_epoch = self.probe_epoch.wrapping_add(1);
        if self.batches.is_empty() {
            return;
        }
        for (shot_id, shot) in &self.shots {
            if !eframe::miv_test_script_window_witness::capture_probe_detail_allowed() {
                continue;
            }
            let window = windows
                .iter()
                .find(|window| window.viewport_id == shot.viewport_id);
            let pass = self.pass_epoch.get(&shot.viewport_id).copied();
            let passed = pass.is_some_and(|(epoch, _)| epoch == previous_epoch);
            crate::logger::log(format!(
                "[capture-probe] pending frame={} batch={} shot={} role={} viewport={:?} expected={} residence={} presentation={:?} backend_token={:?} hwnd={:?} pass_previous_frame={} pass_kind={}",
                self.probe_epoch,
                shot.batch_id,
                shot_id,
                shot.role,
                shot.viewport_id,
                window
                    .and_then(|window| window.identity.as_ref())
                    .map(|identity| identity.describe())
                    .unwrap_or_else(|| "missing".into()),
                window
                    .map(|window| window.residence.as_str())
                    .unwrap_or("missing"),
                window.map(|window| window.presentation),
                window.and_then(|window| window.backend_token),
                window.and_then(|window| window.hwnd),
                passed,
                pass.map(|(_, kind)| kind).unwrap_or("none")
            ));
        }
        self.note_pass(egui::ViewportId::ROOT, "root");
    }

    pub(super) fn note_pass(&mut self, viewport_id: egui::ViewportId, kind: &'static str) {
        if self.batches.is_empty() {
            return;
        }
        self.pass_epoch
            .insert(viewport_id, (self.probe_epoch, kind));
        if eframe::miv_test_script_window_witness::capture_probe_detail_allowed() {
            crate::logger::log(format!(
                "[capture-probe] app_pass frame={} viewport={viewport_id:?} kind={kind} witness={:?}",
                self.probe_epoch,
                eframe::miv_test_script_window_witness::active()
            ));
        }
    }

    pub(super) fn pending_viewport(&self, viewport_id: egui::ViewportId) -> bool {
        self.shots
            .values()
            .any(|shot| shot.viewport_id == viewport_id)
    }

    pub(super) fn receive_events(&mut self, ctx: &egui::Context) {
        if !self.batches.is_empty() {
            let arrival_viewport = ctx.viewport_id();
            ctx.input(|input| {
                for event in &input.events {
                    if let egui::Event::Screenshot { viewport_id, user_data, image } = event {
                        let token = user_data.data.as_ref().and_then(|data| data.downcast_ref::<CaptureToken>());
                        let batch = token.and_then(|token| self.shots.get(&token.shot_id)).map(|shot| shot.batch_id);
                        if eframe::miv_test_script_window_witness::capture_probe_detail_allowed() {
                            crate::logger::log(format!(
                                "[capture-probe] event frame={} arrival={:?} viewport={viewport_id:?} token={token:?} batch={batch:?} image_size={:?}",
                                self.probe_epoch, arrival_viewport, image.size
                            ));
                        }
                    }
                }
            });
        }
        let events = ctx.input(|input| {
            input
                .events
                .iter()
                .filter_map(|event| match event {
                    egui::Event::Screenshot {
                        viewport_id,
                        user_data,
                        image,
                    } => user_data
                        .data
                        .as_ref()
                        .and_then(|data| data.downcast_ref::<CaptureToken>())
                        .copied()
                        .map(|token| (*viewport_id, token, Arc::clone(image))),
                    _ => None,
                })
                .collect::<Vec<_>>()
        });
        for (viewport_id, token, image) in events {
            if token.run_nonce != self.run_nonce {
                continue;
            }
            let Some(shot) = self.shots.remove(&token.shot_id) else {
                continue;
            };
            if shot.viewport_id != viewport_id {
                let _ = self.write_tx.send(WriteResult {
                    batch_id: shot.batch_id,
                    result: Err(format!(
                        "capture viewport mismatch for {}: expected {:?}, got {:?}",
                        shot.role, shot.viewport_id, viewport_id
                    )),
                });
                continue;
            }
            let output = Arc::clone(&self.output);
            let tx = self.write_tx.clone();
            let wake_ctx = ctx.clone();
            let frame = ctx.cumulative_frame_nr();
            let batch_id = shot.batch_id;
            if let Err(error) = std::thread::Builder::new()
                .name("test-script-screenshot".into())
                .spawn(move || {
                    let result = output.write(&shot, &image, frame);
                    let _ = tx.send(WriteResult { batch_id, result });
                    wake_ctx.request_repaint_of(egui::ViewportId::ROOT);
                })
            {
                let _ = self.write_tx.send(WriteResult {
                    batch_id,
                    result: Err(format!("cannot spawn screenshot writer: {error}")),
                });
            }
        }
    }

    pub(super) fn poll(&mut self) {
        while let Ok(completed) = self.write_rx.try_recv() {
            let Some(batch) = self.batches.get_mut(&completed.batch_id) else {
                continue;
            };
            batch.remaining = batch.remaining.saturating_sub(1);
            if let Err(error) = completed.result {
                batch.errors.push(error);
            }
        }
        let now = Instant::now();
        let completed = self
            .batches
            .iter()
            .filter_map(|(&id, batch)| {
                (batch.remaining == 0 || now >= batch.deadline).then_some(id)
            })
            .collect::<Vec<_>>();
        for id in completed {
            let batch = self.batches.remove(&id).expect("listed batch");
            self.shots.retain(|_, shot| shot.batch_id != id);
            let result = if batch.remaining > 0 {
                Err(format!(
                    "capture timed out with {} viewport(s) still pending",
                    batch.remaining
                ))
            } else if batch.errors.is_empty() {
                Ok(())
            } else {
                Err(batch.errors.join("; "))
            };
            crate::logger::log(format!(
                "[capture-probe] outcome batch={id} status={} remaining={} errors={}",
                if result.is_ok() { "ok" } else { "error" },
                batch.remaining,
                batch.errors.len()
            ));
            if let Err(error) = &result {
                crate::logger::log(format!("[test-script] screenshot batch {id}: {error}"));
            }
            if let Some(reply) = batch.reply {
                let _ = reply.send(result);
            }
        }
        if self.batches.is_empty() {
            if eframe::miv_test_script_window_witness::capture_probe_active() {
                let (emitted, suppressed) =
                    eframe::miv_test_script_window_witness::capture_probe_detail_counts();
                crate::logger::log(format!(
                    "[capture-probe] summary detail_emitted={emitted} detail_suppressed={suppressed} limit={} outcome=all_batches_finished",
                    eframe::miv_test_script_window_witness::CAPTURE_PROBE_DETAIL_LIMIT
                ));
            }
            eframe::miv_test_script_window_witness::set_capture_probe(false);
        }
    }

    pub(super) fn is_pending(&self, batch_id: u64) -> bool {
        self.batches.contains_key(&batch_id)
    }
}

impl Drop for Coordinator {
    fn drop(&mut self) {
        eframe::miv_test_script_window_witness::set_capture_probe(false);
    }
}

impl Output {
    fn write_skipped(
        &self,
        label: &str,
        role: &str,
        reason: &str,
        frame: u64,
    ) -> Result<(), String> {
        no_reparse(&self.dir)?;
        let timestamp_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis();
        let record = serde_json::json!({
            "status": "skipped",
            "label": label,
            "viewport": role,
            "reason": reason,
            "frame": frame,
            "timestamp_ms": timestamp_ms,
        });
        let mut manifest = self
            .manifest
            .lock()
            .map_err(|_| "screenshot manifest lock is poisoned".to_string())?;
        writeln!(manifest, "{record}")
            .and_then(|_| manifest.flush())
            .map_err(|error| format!("cannot record skipped screenshot: {error}"))
    }

    fn write(&self, shot: &Shot, image: &egui::ColorImage, frame: u64) -> Result<(), String> {
        no_reparse(&self.dir)?;
        let [width, height] = image.size;
        let pixels = width
            .checked_mul(height)
            .ok_or_else(|| "screenshot dimensions overflow".to_string())?;
        if width == 0 || height == 0 || pixels > MAX_PIXELS || pixels != image.pixels.len() {
            return Err(format!("invalid screenshot dimensions: {width}x{height}"));
        }
        let width_u32 = u32::try_from(width).map_err(|error| error.to_string())?;
        let height_u32 = u32::try_from(height).map_err(|error| error.to_string())?;
        let path = self.dir.join(&shot.file_name);
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
            .map_err(|error| format!("cannot create {}: {error}", path.display()))?;
        let result = (|| {
            let mut bytes = Vec::with_capacity(pixels * 4);
            for pixel in &image.pixels {
                bytes.extend_from_slice(&pixel.to_array());
            }
            let mut writer = BufWriter::new(file);
            image::codecs::png::PngEncoder::new(&mut writer)
                .write_image(
                    &bytes,
                    width_u32,
                    height_u32,
                    image::ColorType::Rgba8.into(),
                )
                .map_err(|error| format!("cannot encode screenshot PNG: {error}"))?;
            writer
                .flush()
                .map_err(|error| format!("cannot flush screenshot PNG: {error}"))?;
            let timestamp_ms = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis();
            let record = serde_json::json!({
                "path": format!("screenshots/{}", shot.file_name),
                "label": shot.label,
                "viewport": shot.role,
                "width": width_u32,
                "height": height_u32,
                "frame": frame,
                "timestamp_ms": timestamp_ms,
            });
            let mut manifest = self
                .manifest
                .lock()
                .map_err(|_| "screenshot manifest lock is poisoned".to_string())?;
            writeln!(manifest, "{record}")
                .and_then(|_| manifest.flush())
                .map_err(|error| format!("cannot record screenshot evidence: {error}"))?;
            Ok(())
        })();
        if result.is_err() {
            let _ = fs::remove_file(path);
        }
        result
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_coordinator(dir: PathBuf) -> Coordinator {
        fs::create_dir(&dir).unwrap();
        let manifest = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join("manifest.jsonl"))
            .unwrap();
        let (write_tx, write_rx) = mpsc::channel();
        Coordinator {
            output: Arc::new(Output {
                dir,
                manifest: Mutex::new(manifest),
            }),
            run_nonce: 123,
            next_batch: 0,
            next_shot: 0,
            shots: HashMap::new(),
            batches: HashMap::new(),
            probe_epoch: 0,
            pass_epoch: HashMap::new(),
            write_tx,
            write_rx,
        }
    }

    #[test]
    fn deferred_capture_repaint_wakes_the_exact_viewport_from_root_context() {
        let ctx = egui::Context::default();
        let deferred = egui::ViewportId::from_hash_of("deferred-capture-repaint");
        let requests = Arc::new(Mutex::new(Vec::new()));
        let observed = Arc::clone(&requests);
        ctx.set_request_repaint_callback(move |info| {
            observed.lock().unwrap().push(info.viewport_id);
        });
        request_capture_repaint(
            &ctx,
            &Target {
                viewport_id: deferred,
                role: "detached-1-1".into(),
                hwnd: Some(101),
                availability: Availability::Registered,
                presentation: TestScriptWindowPresentation::Other,
            },
        )
        .unwrap();
        assert_eq!(*requests.lock().unwrap(), vec![deferred]);
    }

    #[test]
    fn evidence_path_accepts_only_direct_run_children() {
        let temp = tempfile::tempdir().unwrap();
        let runs = temp.path().join("ui-smoke-runs");
        let run = runs.join("one-run");
        let nested = run.join("nested");
        fs::create_dir_all(&nested).unwrap();
        assert_eq!(
            validate_run_dir_under(&runs, &run).unwrap(),
            fs::canonicalize(&run).unwrap()
        );
        assert!(validate_run_dir_under(&runs, &nested).is_err());
        assert!(validate_run_dir_under(&runs, temp.path()).is_err());
        assert!(validate_run_dir_under(&runs, &runs).is_err());
    }

    #[test]
    fn screenshot_result_writes_png_and_manifest_off_the_ui_pass() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("screenshots");
        fs::create_dir(&dir).unwrap();
        let manifest = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(dir.join("manifest.jsonl"))
            .unwrap();
        let output = Output {
            dir: dir.clone(),
            manifest: Mutex::new(manifest),
        };
        let shot = Shot {
            batch_id: 1,
            viewport_id: egui::ViewportId::ROOT,
            role: "root".into(),
            label: "checkpoint".into(),
            file_name: "01-checkpoint-root.png".into(),
        };
        let image = egui::ColorImage::new([2, 2], vec![egui::Color32::RED; 4]);
        output.write(&shot, &image, 17).unwrap();
        let decoded = image::open(dir.join(&shot.file_name)).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (2, 2));
        let manifest = fs::read_to_string(dir.join("manifest.jsonl")).unwrap();
        let record: serde_json::Value = serde_json::from_str(manifest.trim()).unwrap();
        assert_eq!(record["label"], "checkpoint");
        assert_eq!(record["viewport"], "root");
        assert_eq!(record["frame"], 17);
        assert!(output.write(&shot, &image, 18).is_err());
    }

    #[test]
    fn screenshot_event_completes_the_capture_request() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("screenshots");
        let mut capture = test_coordinator(dir.clone());
        let ctx = egui::Context::default();
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(100.0, 100.0),
            )),
            ..Default::default()
        };
        let (reply, received) = mpsc::sync_channel(1);
        let _ = ctx.run(raw.clone(), |ctx| {
            capture
                .request(
                    ctx,
                    "checkpoint",
                    vec![Target {
                        viewport_id: egui::ViewportId::ROOT,
                        role: "root".into(),
                        hwnd: None,
                        availability: Availability::Registered,
                        presentation: TestScriptWindowPresentation::Root,
                    }],
                    Duration::from_secs(2),
                    Some(reply.clone()),
                )
                .unwrap();
        });
        assert!(capture.is_pending(1));
        let token = CaptureToken {
            run_nonce: capture.run_nonce,
            shot_id: 1,
        };
        let image = egui::ColorImage::new([2, 2], vec![egui::Color32::BLUE; 4]);
        let mut response = raw;
        response.events.push(egui::Event::Screenshot {
            viewport_id: egui::ViewportId::ROOT,
            user_data: egui::UserData::new(token),
            image: Arc::new(image),
        });
        let _ = ctx.run(response, |ctx| capture.receive_events(ctx));
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            capture.poll();
            if let Ok(result) = received.try_recv() {
                result.unwrap();
                break;
            }
            assert!(Instant::now() < deadline, "capture writer did not reply");
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(!capture.is_pending(1));
        assert!(dir.join("01-checkpoint-root.png").is_file());
    }

    #[test]
    fn immediate_detached_viewport_capture_round_trips_with_root() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("screenshots");
        let mut capture = test_coordinator(dir.clone());
        let ctx = egui::Context::default();
        let detached = egui::ViewportId::from_hash_of("detached-pdf-test");
        ctx.set_embed_viewports(false);
        egui::Context::set_immediate_viewport_renderer(|ctx, mut viewport| {
            (viewport.viewport_ui_cb)(ctx);
        });
        let raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(100.0, 100.0),
            )),
            ..Default::default()
        };
        let (reply, received) = mpsc::sync_channel(1);
        let repainted = Arc::new(Mutex::new(Vec::new()));
        let output = ctx.run(raw.clone(), |ctx| {
            ctx.show_viewport_immediate(detached, egui::ViewportBuilder::default(), |_, class| {
                assert!(class == egui::ViewportClass::Immediate);
            });
            capture
                .request_with_repaint(
                    ctx,
                    "pdf",
                    vec![
                        Target {
                            viewport_id: egui::ViewportId::ROOT,
                            role: "root".into(),
                            hwnd: None,
                            availability: Availability::Registered,
                            presentation: TestScriptWindowPresentation::Root,
                        },
                        Target {
                            viewport_id: detached,
                            role: "detached-1-7".into(),
                            hwnd: Some(123),
                            availability: Availability::Registered,
                            presentation: TestScriptWindowPresentation::ActiveImmediate,
                        },
                    ],
                    Duration::from_secs(2),
                    Some(reply.clone()),
                    |_| Ok(Readiness::Renderable),
                    |ctx, target| {
                        repainted
                            .lock()
                            .unwrap()
                            .push((target.viewport_id, target.hwnd));
                        ctx.request_repaint_of(target.viewport_id);
                        Ok(())
                    },
                )
                .unwrap();
        });
        assert_eq!(
            *repainted.lock().unwrap(),
            vec![(egui::ViewportId::ROOT, None), (detached, Some(123))],
            "each screenshot must request an explicit repaint of its exact host"
        );
        let child = output.viewport_output.get(&detached).unwrap();
        assert!(
            child
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Screenshot(_)))
        );
        assert!(
            output
                .viewport_output
                .get(&egui::ViewportId::ROOT)
                .unwrap()
                .commands
                .iter()
                .any(|command| matches!(command, egui::ViewportCommand::Screenshot(_)))
        );

        // wgpu returns completed screenshots to the root input queue, each
        // carrying the viewport id of the surface that was painted.
        let mut response = raw;
        for (viewport_id, shot_id) in [(egui::ViewportId::ROOT, 1), (detached, 2)] {
            response.events.push(egui::Event::Screenshot {
                viewport_id,
                user_data: egui::UserData::new(CaptureToken {
                    run_nonce: capture.run_nonce,
                    shot_id,
                }),
                image: Arc::new(egui::ColorImage::new([2, 2], vec![egui::Color32::GREEN; 4])),
            });
        }
        let _ = ctx.run(response, |ctx| capture.receive_events(ctx));
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            capture.poll();
            if let Ok(result) = received.try_recv() {
                result.unwrap();
                break;
            }
            assert!(
                Instant::now() < deadline,
                "multiwindow capture did not reply"
            );
            std::thread::sleep(Duration::from_millis(2));
        }
        assert!(dir.join("01-pdf-root.png").is_file());
        assert!(dir.join("01-pdf-detached-1-7.png").is_file());
        assert_eq!(
            fs::read_to_string(dir.join("manifest.jsonl"))
                .unwrap()
                .lines()
                .count(),
            2
        );
    }

    #[test]
    fn renderable_deferred_viewport_delivers_its_screenshot_in_the_child_pass() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("screenshots");
        let mut capture = test_coordinator(dir.clone());
        let ctx = egui::Context::default();
        let first = egui::ViewportId::from_hash_of("parked-first");
        let second = egui::ViewportId::from_hash_of("active-second");
        ctx.set_embed_viewports(false);
        egui::Context::set_immediate_viewport_renderer(|ctx, mut viewport| {
            (viewport.viewport_ui_cb)(ctx);
        });
        let mut raw = egui::RawInput::default();
        raw.viewports.insert(first, Default::default());
        raw.viewports.insert(second, Default::default());
        let (reply, received) = mpsc::sync_channel(1);
        let repainted = Arc::new(Mutex::new(Vec::new()));
        let output = ctx.run(raw.clone(), |ctx| {
            ctx.show_viewport_deferred(first, egui::ViewportBuilder::default(), |_, class| {
                assert!(class == egui::ViewportClass::Deferred);
            });
            ctx.show_viewport_immediate(second, egui::ViewportBuilder::default(), |_, class| {
                assert!(class == egui::ViewportClass::Immediate);
            });
            capture
                .request_with_repaint(
                    ctx,
                    "two-detached",
                    vec![
                        Target {
                            viewport_id: first,
                            role: "detached-1-1".into(),
                            hwnd: Some(101),
                            availability: Availability::Registered,
                            presentation: TestScriptWindowPresentation::Other,
                        },
                        Target {
                            viewport_id: second,
                            role: "detached-2-2".into(),
                            hwnd: Some(202),
                            availability: Availability::Registered,
                            presentation: TestScriptWindowPresentation::ActiveImmediate,
                        },
                    ],
                    Duration::from_secs(2),
                    Some(reply.clone()),
                    |_| Ok(Readiness::Renderable),
                    |ctx, target| {
                        repainted.lock().unwrap().push(target.viewport_id);
                        ctx.request_repaint_of(target.viewport_id);
                        Ok(())
                    },
                )
                .unwrap();
        });
        assert_eq!(
            *repainted.lock().unwrap(),
            vec![first, second],
            "the deferred target must request its own repaint at issue time"
        );
        assert!(output.viewport_output.get(&first).unwrap().class == egui::ViewportClass::Deferred);
        assert!(
            output
                .viewport_output
                .get(&first)
                .unwrap()
                .viewport_ui_cb
                .is_some()
        );
        assert!(
            output
                .viewport_output
                .get(&second)
                .unwrap()
                .viewport_ui_cb
                .is_none()
        );

        // eframe drains the screenshot channel into the next native viewport
        // input. The passive deferred child can be that next viewport.
        let run_nonce = capture.run_nonce;
        let token = |shot_id| egui::UserData::new(CaptureToken { run_nonce, shot_id });
        let image = Arc::new(egui::ColorImage::new([2, 2], vec![egui::Color32::GREEN; 4]));
        let mut child_input = raw.clone();
        child_input.viewport_id = first;
        child_input.events.push(egui::Event::Screenshot {
            viewport_id: first,
            user_data: token(1),
            image: image.clone(),
        });
        let _ = ctx.run(child_input, |ctx| capture.receive_events(ctx));
        let mut root_input = raw;
        root_input.events.push(egui::Event::Screenshot {
            viewport_id: second,
            user_data: token(2),
            image,
        });
        let _ = ctx.run(root_input, |ctx| capture.receive_events(ctx));

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            capture.poll();
            if let Ok(result) = received.try_recv() {
                result.unwrap();
                break;
            }
            assert!(Instant::now() < deadline, "deferred capture did not reply");
            std::thread::yield_now();
        }
        assert!(dir.join("01-two-detached-detached-1-1.png").is_file());
        assert!(dir.join("01-two-detached-detached-2-2.png").is_file());
    }

    #[test]
    fn frozen_parked_view_is_skipped_without_queuing_a_screenshot() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("screenshots");
        let mut capture = test_coordinator(dir.clone());
        let ctx = egui::Context::default();
        let frozen = egui::ViewportId::from_hash_of("frozen-parked-view");
        let (reply, received) = mpsc::sync_channel(1);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            capture
                .request(
                    ctx,
                    "frozen",
                    vec![Target {
                        viewport_id: frozen,
                        role: "detached-1-1".into(),
                        hwnd: Some(101),
                        availability: Availability::Registered,
                        presentation: TestScriptWindowPresentation::PassiveDeferredFrozen,
                    }],
                    Duration::from_secs(2),
                    Some(reply.clone()),
                )
                .unwrap();
        });
        assert!(
            output
                .viewport_output
                .get(&frozen)
                .is_none_or(|viewport| viewport.commands.is_empty())
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            capture.poll();
            if let Ok(result) = received.try_recv() {
                result.unwrap();
                break;
            }
            assert!(Instant::now() < deadline, "frozen skip did not finish");
            std::thread::yield_now();
        }
        let record: serde_json::Value = serde_json::from_str(
            fs::read_to_string(dir.join("manifest.jsonl"))
                .unwrap()
                .trim(),
        )
        .unwrap();
        assert_eq!(record["status"], "skipped");
        assert_eq!(record["reason"], PARKED_FROZEN_SKIP_REASON);
        assert_eq!(record["viewport"], "detached-1-1");
    }

    #[test]
    fn hidden_viewport_records_a_skip_without_waiting_for_a_screenshot() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("screenshots");
        let mut capture = test_coordinator(dir.clone());
        let ctx = egui::Context::default();
        let detached = egui::ViewportId::from_hash_of("hidden-detached");
        let (reply, received) = mpsc::sync_channel(1);
        let output = ctx.run(egui::RawInput::default(), |ctx| {
            capture
                .request_with_repaint(
                    ctx,
                    "hidden",
                    vec![Target {
                        viewport_id: detached,
                        role: "detached-1-7".into(),
                        hwnd: Some(123),
                        availability: Availability::Registered,
                        presentation: TestScriptWindowPresentation::ActiveImmediate,
                    }],
                    Duration::from_secs(2),
                    Some(reply.clone()),
                    |_| Ok(Readiness::Skipped("native viewport is hidden")),
                    |_, _| panic!("skipped viewport must not request a screenshot repaint"),
                )
                .unwrap();
        });
        assert!(
            output
                .viewport_output
                .get(&detached)
                .is_none_or(|child| child.commands.is_empty())
        );
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            capture.poll();
            if let Ok(result) = received.try_recv() {
                result.unwrap();
                break;
            }
            assert!(Instant::now() < deadline, "skip writer did not reply");
            std::thread::yield_now();
        }
        let manifest = fs::read_to_string(dir.join("manifest.jsonl")).unwrap();
        let record: serde_json::Value = serde_json::from_str(manifest.trim()).unwrap();
        assert_eq!(record["status"], "skipped");
        assert_eq!(record["reason"], "native viewport is hidden");
        assert_eq!(record["viewport"], "detached-1-7");
        assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    }

    #[test]
    fn absent_native_viewport_is_skipped_before_touching_its_old_hwnd() {
        let temp = tempfile::tempdir().unwrap();
        let dir = temp.path().join("screenshots");
        let mut capture = test_coordinator(dir.clone());
        let ctx = egui::Context::default();
        let (reply, received) = mpsc::sync_channel(1);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            capture
                .request(
                    ctx,
                    "parked",
                    vec![Target {
                        viewport_id: egui::ViewportId::from_hash_of("retired-first"),
                        role: "detached-1-1".into(),
                        hwnd: Some(123),
                        availability: Availability::Absent,
                        presentation: TestScriptWindowPresentation::Other,
                    }],
                    Duration::from_secs(2),
                    Some(reply.clone()),
                )
                .unwrap();
        });
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            capture.poll();
            if let Ok(result) = received.try_recv() {
                result.unwrap();
                break;
            }
            assert!(Instant::now() < deadline, "absent viewport did not resolve");
            std::thread::yield_now();
        }
        let manifest = fs::read_to_string(dir.join("manifest.jsonl")).unwrap();
        let record: serde_json::Value = serde_json::from_str(manifest.trim()).unwrap();
        assert_eq!(record["status"], "skipped");
        assert_eq!(
            record["reason"],
            "viewport absent from eframe native render registry"
        );
    }

    #[test]
    fn renderable_deferred_without_screenshot_response_times_out() {
        let temp = tempfile::tempdir().unwrap();
        let mut capture = test_coordinator(temp.path().join("screenshots"));
        let ctx = egui::Context::default();
        let (reply, received) = mpsc::sync_channel(1);
        let _ = ctx.run(egui::RawInput::default(), |ctx| {
            capture
                .request(
                    ctx,
                    "missing",
                    vec![Target {
                        viewport_id: egui::ViewportId::from_hash_of("renderable-deferred"),
                        role: "detached-1-1".into(),
                        hwnd: None,
                        availability: Availability::Registered,
                        presentation: TestScriptWindowPresentation::Other,
                    }],
                    Duration::ZERO,
                    Some(reply.clone()),
                )
                .unwrap();
        });
        capture.poll();
        assert!(
            received
                .recv_timeout(Duration::from_millis(100))
                .unwrap()
                .unwrap_err()
                .contains("timed out")
        );
    }
}

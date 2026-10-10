//! Opt-in release fixtures: worker I/O and typed root-UI requests only.
//! The idle pause intentionally sends no command, log or repaint.
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, OnceLock, RwLock, mpsc};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use rhai::{Dynamic, Engine, EvalAltResult, ImmutableString, Map};
use serde_json::{Value, json};

use super::{
    RunnerBridge, TestScriptActionSelection, TestScriptWindowIdentity, UiCommand, rhai_error,
};

mod tray_witness;

#[derive(Debug)]
pub(crate) enum Operation {
    Observe,
    Open(PathBuf),
    Text(String),
    Pin(PathBuf),
}

pub(crate) struct Observation {
    pub(crate) value: Value,
    pub(crate) catalog: Option<Arc<crate::catalog::CatalogDb>>,
    pub(crate) cache: Option<Arc<RwLock<HashMap<String, crate::catalog::CacheEntry>>>>,
    pub(crate) pins: Option<Arc<crate::folder_thumb_pins::FolderThumbPinDb>>,
}

pub(crate) struct Request {
    pub(crate) owner: TestScriptWindowIdentity,
    pub(crate) operation: Operation,
    pub(crate) deadline: Instant,
    pub(crate) reply: mpsc::SyncSender<Result<Observation, String>>,
}
impl std::fmt::Debug for Request {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ReleaseCheckRequest")
            .field("operation", &self.operation)
            .finish()
    }
}

#[derive(Default)]
pub(crate) struct Session {
    pub(crate) case_id: String,
    pub(crate) directory: PathBuf,
    pub(crate) fixture: PathBuf,
    pub(crate) close_mode: Option<String>,
    pub(crate) close_sequence: u64,
    pub(crate) close_proof: Value,
    pub(crate) cache_proof: Value,
    pub(crate) opens: Vec<Value>,
    pub(crate) preflight_opens: Vec<Value>,
    pub(crate) quit_after_finish: bool,
    // Diagnostic history only; the canonical ThumbnailRequests owner is never mutated.
    pub(crate) witnesses: Vec<Value>,
}

pub(crate) fn session() -> &'static Mutex<Option<Session>> {
    static SESSION: OnceLock<Mutex<Option<Session>>> = OnceLock::new();
    SESSION.get_or_init(|| Mutex::new(None))
}
fn requests() -> &'static Mutex<VecDeque<Request>> {
    static REQUESTS: OnceLock<Mutex<VecDeque<Request>>> = OnceLock::new();
    REQUESTS.get_or_init(|| Mutex::new(VecDeque::new()))
}
pub(crate) fn enqueue(request: Request) {
    requests().lock().unwrap().push_back(request);
}
pub(crate) fn take_request() -> Option<Request> {
    requests().lock().unwrap().pop_front()
}
struct StartupProof {
    case_id: String,
    directory: PathBuf,
    opens: Vec<Value>,
}
static STARTUP: OnceLock<StartupProof> = OnceLock::new();
pub(crate) fn is_active() -> bool {
    STARTUP.get().is_some() || session().lock().is_ok_and(|s| s.is_some())
}
pub(crate) fn take_quit_after_finish() -> bool {
    session()
        .lock()
        .ok()
        .and_then(|mut s| s.as_mut().map(|s| std::mem::take(&mut s.quit_after_finish)))
        .unwrap_or(false)
}
pub(crate) fn trace(kind: &str, mut value: Value) {
    let id = session()
        .lock()
        .ok()
        .and_then(|s| s.as_ref().map(|s| s.case_id.clone()))
        .or_else(|| STARTUP.get().map(|s| s.case_id.clone()));
    if let (Some(id), Some(fields)) = (id, value.as_object_mut()) {
        fields.insert("case_id".into(), Value::String(id));
        let extras: Vec<_> = fields
            .iter()
            .map(|(k, v)| (k.as_str(), v.clone()))
            .collect();
        crate::perf::event("qa", kind, None, 0, &extras);
        // Record after publishing: this conservative clock cannot place evidence
        // earlier than its perf event. Setup polling reads memory, never the log.
        if let Some(t) = crate::perf::program_start().map(|start| start.elapsed().as_secs_f64()) {
            value["kind"] = json!(kind);
            value["t"] = json!(t);
            if let Ok(mut state) = session().lock()
                && let Some(state) = state.as_mut()
            {
                state.witnesses.push(value);
            }
        }
    }
}
pub(crate) fn record_on_exit() {
    trace("on_exit", json!({"flush_complete":true}));
    crate::perf::flush();
}
fn unix_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_millis() as u64
}

// Every ancestor is checked: canonicalize alone would hide a junction escape.
fn plain_path(path: &Path) -> Result<PathBuf, String> {
    for ancestor in path.ancestors() {
        let metadata = std::fs::symlink_metadata(ancestor)
            .map_err(|e| format!("environment: missing path {}: {e}", ancestor.display()))?;
        use std::os::windows::fs::MetadataExt;
        if metadata.file_attributes() & 0x400 != 0 {
            return Err(format!(
                "environment: reparse path refused: {}",
                ancestor.display()
            ));
        }
    }
    let canonical = path.canonicalize().map_err(|e| e.to_string())?;
    let s = canonical.to_string_lossy();
    Ok(if let Some(rest) = s.strip_prefix(r"\\?\UNC\") {
        PathBuf::from(format!(r"\\{rest}"))
    } else if let Some(rest) = s.strip_prefix(r"\\?\") {
        PathBuf::from(rest)
    } else {
        canonical
    })
}
fn validate_layout(exe: &Path, data: &Path, directory: &Path) -> Result<PathBuf, String> {
    let sandbox = exe
        .parent()
        .ok_or("environment: missing executable parent")?;
    if exe.file_name().and_then(|s| s.to_str()) != Some("mimageviewer.exe")
        || sandbox.file_name().and_then(|s| s.to_str()) != Some("portable-smoke")
        || sandbox
            .parent()
            .and_then(|p| p.file_name())
            .and_then(|s| s.to_str())
            != Some("target")
        || data != sandbox.join("data")
    {
        return Err(
            "environment: release checks require the exact portable-smoke core/data layout".into(),
        );
    }
    let target = sandbox.parent().unwrap();
    super::evidence_path::release_fixture(target, directory)
        .map_err(|error| format!("environment: {error}"))
}
fn isolated_context(directory: &Path) -> Result<(PathBuf, PathBuf, PathBuf, PathBuf), String> {
    let exe = plain_path(&std::env::current_exe().map_err(|e| e.to_string())?)?;
    let data = plain_path(&crate::data_dir::get())?;
    let directory =
        super::evidence_path::validate_directory(Path::new(env!("CARGO_MANIFEST_DIR")), directory)?;
    let directory = plain_path(&directory)?;
    let fixture = validate_layout(&exe, &data, &directory)?;
    let fixture = plain_path(&fixture)?;
    let explicit = std::env::args()
        .collect::<Vec<_>>()
        .windows(2)
        .any(|a| a[0] == "--data-dir" && plain_path(Path::new(&a[1])).ok().as_ref() == Some(&data));
    let marker =
        std::fs::read_to_string(data.join(".disposable-smoke-data")).map_err(|e| e.to_string())?;
    if !explicit
        || marker.trim() != "mimageviewer-disposable-smoke-v1;test-script=true"
        || !exe
            .parent()
            .unwrap()
            .join(".test-script-build.json")
            .is_file()
        || !fixture.join("fixture-manifest.json").is_file()
    {
        return Err(
            "environment: explicit isolated data/diagnostic marker/fixture manifest missing".into(),
        );
    }
    Ok((exe, data, directory, fixture))
}

// Before App construction: validate only immutable stopped-profile evidence.
// No catalog access, UI request or lifecycle state change occurs here.
pub(crate) fn arm_startup(directory: &Path) -> Result<(), String> {
    if !directory.join("pre-launch-cache.json").exists() {
        return Ok(());
    }
    let (_, data, directory, fixture) = isolated_context(directory)?;
    let proof_path = plain_path(&directory.join("pre-launch-cache.json"))?;
    let proof: Value =
        serde_json::from_slice(&std::fs::read(proof_path).map_err(|e| e.to_string())?)
            .map_err(|e| e.to_string())?;
    let case_id = proof["case_id"]
        .as_str()
        .ok_or("environment: prelaunch case ID missing")?
        .to_owned();
    if proof["schema_version"] != 1
        || proof["stopped"] != true
        || plain_path(Path::new(proof["data_dir"].as_str().unwrap_or("")))? != data
        || plain_path(Path::new(proof["fixture_manifest"].as_str().unwrap_or("")))?
            != fixture.join("fixture-manifest.json")
    {
        return Err("environment: stopped cache proof context mismatch".into());
    }
    let opens = proof["opens"]
        .as_array()
        .ok_or("environment: stopped cache proof missing")?
        .clone();
    for entry in &opens {
        let folder = plain_path(Path::new(entry["folder_path"].as_str().unwrap_or("")))?;
        if !folder.starts_with(&fixture)
            || folder == fixture
            || !entry["pre_open_absent"].is_boolean()
            || !entry["catalog_keys"].is_array()
        {
            return Err("environment: invalid stopped cache proof target".into());
        }
    }
    STARTUP
        .set(StartupProof {
            case_id,
            directory,
            opens,
        })
        .map_err(|_| "environment: startup proof already armed".into())
}
fn begin(case_id: String, directory: PathBuf) -> Result<(), String> {
    let (exe, data, directory, fixture) = isolated_context(&directory)?;
    let startup = STARTUP
        .get()
        .ok_or("environment: stopped prelaunch cache proof missing")?;
    if startup.case_id != case_id || startup.directory != directory {
        return Err("environment: startup case context mismatch".into());
    }
    let mut state = session()
        .lock()
        .map_err(|_| "environment: release session poisoned")?;
    if state.is_some() {
        return Err("environment: release check already started".into());
    }
    *state = Some(Session {
        case_id,
        directory,
        fixture,
        preflight_opens: startup.opens.clone(),
        ..Default::default()
    });
    drop(state);
    trace(
        "begin",
        json!({"pid":std::process::id(),"executable":exe,"data_dir":data}),
    );
    Ok(())
}

fn fixture_path(path: &str) -> Result<PathBuf, String> {
    let path = plain_path(Path::new(path))?;
    let guard = session().lock().unwrap();
    let fixture = &guard
        .as_ref()
        .ok_or("environment: begin release checks first")?
        .fixture;
    if !path.starts_with(fixture) || path == *fixture {
        return Err("environment: refusing path outside generated fixture".into());
    }
    Ok(path)
}
fn output_path(path: &str) -> Result<PathBuf, String> {
    let path = plain_path(Path::new(path))?;
    if session()
        .lock()
        .unwrap()
        .as_ref()
        .is_none_or(|s| s.directory != path)
    {
        return Err("environment: refusing another case output directory".into());
    }
    Ok(path)
}
fn request(bridge: &RunnerBridge, operation: Operation) -> Result<Observation, String> {
    if !is_active() {
        return Err("environment: begin release checks first".into());
    }
    let owner = match bridge.action_selection()? {
        TestScriptActionSelection::Targeted(owner @ TestScriptWindowIdentity::Root { .. }) => owner,
        _ => return Err("environment: release checks require the selected Root owner".into()),
    };
    let deadline = Instant::now() + Duration::from_secs(30);
    bridge.validate_selected_owner_fresh(&owner, deadline)?;
    let (reply, rx) = mpsc::sync_channel(1);
    bridge.send(UiCommand::ReleaseCheck(Request {
        owner,
        operation,
        deadline,
        reply,
    }))?;
    loop {
        bridge.interrupt.check()?;
        match rx.recv_timeout(Duration::from_millis(10)) {
            Ok(result) => {
                if let Ok(o) = &result {
                    remember_open_targets(&o.value);
                }
                return result;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                return Err("environment: release UI request disconnected".into());
            }
            Err(_) if Instant::now() >= deadline => {
                return Err("environment: release UI request timeout".into());
            }
            Err(_) => (),
        }
    }
}
fn remember_open_targets(observation: &Value) {
    if observation["search_active"] == true {
        return;
    }
    let mut guard = session().lock().unwrap();
    if let Some(s) = guard.as_mut() {
        if s.opens
            .last()
            .is_none_or(|o| o["folder_path"] != observation["current_folder_path"])
        {
            if let Some(proof) = s
                .preflight_opens
                .iter()
                .find(|p| p["folder_path"] == observation["current_folder_path"])
                .cloned()
            {
                s.opens.push(proof);
            }
        }
        if let Some(open) = s.opens.last_mut() {
            if open["folder_path"] == observation["current_folder_path"]
                && observation["loading_complete"] == true
                && open.get("target_keys").is_none()
            {
                open["target_keys"] = json!(
                    observation["targets"]
                        .as_array()
                        .unwrap()
                        .iter()
                        .map(|t| t["key"].clone())
                        .collect::<Vec<_>>()
                );
                open["items_gen"] = observation["items_generation"].clone();
            }
        }
    }
}
fn catalog_preflight(path: &Path) -> Result<Value, String> {
    let catalog = crate::catalog::CatalogDb::open_existing_read_only(
        &crate::catalog::default_cache_dir(),
        path,
    )
    .map_err(|e| e.to_string())?;
    let keys = match catalog {
        Some(c) => c
            .load_all()
            .map_err(|e| e.to_string())?
            .into_keys()
            .collect::<Vec<_>>(),
        None => Vec::new(),
    };
    Ok(json!({"folder_path":path,"pre_open_absent":keys.is_empty(),"catalog_keys":keys}))
}
fn open(bridge: &RunnerBridge, path: &str) -> Result<(), String> {
    let path = fixture_path(path)?;
    let proof = catalog_preflight(&path)?;
    session()
        .lock()
        .unwrap()
        .as_mut()
        .unwrap()
        .opens
        .push(proof);
    request(bridge, Operation::Open(path))?;
    Ok(())
}
fn cache_probe(observation: &Observation) -> Result<Value, String> {
    let catalog = observation
        .catalog
        .as_ref()
        .ok_or("environment: catalog unavailable")?
        .load_all()
        .map_err(|e| e.to_string())?;
    let cache = observation
        .cache
        .as_ref()
        .ok_or("environment: cache map unavailable")?
        .read()
        .map_err(|_| "environment: cache map poisoned")?;
    let mut targets = observation.value["targets"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    for target in &mut targets {
        let base = target["cache_base_key"]
            .as_str()
            .ok_or("environment: missing cache key")?;
        let base = base.to_owned();
        let present = |keys: Vec<&String>| keys.iter().any(|k| k.starts_with(&base));
        target["catalog_absent"] = json!(!present(catalog.keys().collect()));
        target["cache_map_absent"] = json!(!present(cache.keys().collect()));
    }
    Ok(
        json!({"targets":targets,"folder_path":observation.value["current_folder_path"],
        "items_gen":observation.value["items_generation"],"cache_check_unix_ms":unix_ms()}),
    )
}
fn write_json(path: &Path, value: &Value) -> Result<(), String> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(
        &temporary,
        serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    // One producer, fresh case paths. Readers see the complete protocol message.
    std::fs::rename(temporary, path).map_err(|e| e.to_string())
}
fn to_dynamic(value: &Value) -> Dynamic {
    match value {
        Value::Null => Dynamic::UNIT,
        Value::Bool(v) => Dynamic::from(*v),
        Value::Number(v) => v
            .as_i64()
            .map(Dynamic::from)
            .unwrap_or_else(|| Dynamic::from(v.as_f64().unwrap())),
        Value::String(v) => Dynamic::from(v.clone()),
        Value::Array(v) => Dynamic::from(v.iter().map(to_dynamic).collect::<rhai::Array>()),
        Value::Object(v) => Dynamic::from(
            v.iter()
                .map(|(k, v)| (k.clone().into(), to_dynamic(v)))
                .collect::<Map>(),
        ),
    }
}
fn from_dynamic(value: Dynamic) -> Result<Value, String> {
    if value.is_unit() {
        Ok(Value::Null)
    } else if value.is::<bool>() {
        Ok(json!(value.cast::<bool>()))
    } else if value.is::<rhai::INT>() {
        Ok(json!(value.cast::<rhai::INT>()))
    } else if value.is::<rhai::FLOAT>() {
        Ok(json!(value.cast::<rhai::FLOAT>()))
    } else if value.is::<ImmutableString>() {
        Ok(json!(value.cast::<ImmutableString>().as_str()))
    } else if value.is::<rhai::Array>() {
        value
            .cast::<rhai::Array>()
            .into_iter()
            .map(from_dynamic)
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array)
    } else if value.is::<Map>() {
        value
            .cast::<Map>()
            .into_iter()
            .map(|(k, v)| Ok((k.to_string(), from_dynamic(v)?)))
            .collect::<Result<serde_json::Map<_, _>, String>>()
            .map(Value::Object)
    } else {
        Err("environment: unsupported script evidence type".into())
    }
}
fn evidence(bridge: &RunnerBridge, directory: &str, script: Map) -> Result<(), String> {
    let directory = output_path(directory)?;
    let observation = request(bridge, Operation::Observe)?;
    let mut value = from_dynamic(Dynamic::from(script))?;
    {
        let guard = session().lock().unwrap();
        let state = guard.as_ref().unwrap();
        value["case_id"] = json!(state.case_id);
        value["pid"] = json!(std::process::id());
        value["observation"] = observation.value.clone();
        value["requested_px"] = observation.value["requested_px"].clone();
        value["close"] = state.close_proof.clone();
        value["cache"] = json!({"mode":if state.case_id=="perf-uncached" {"uncached"} else {"cached"},"opens":state.opens});
        if state.case_id == "video-pin-background" {
            value["target_key"] = observation.value["targets"]
                .as_array()
                .and_then(|t| t.first())
                .map(|t| t["key"].clone())
                .unwrap_or(Value::Null);
        }
    }
    write_json(&directory.join("case-evidence.json"), &value)
}
pub(super) fn register(engine: &mut Engine, bridge: RunnerBridge) {
    engine.register_fn(
        "release_check_begin",
        |id: ImmutableString, dir: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            begin(id.to_string(), PathBuf::from(dir.as_str())).map_err(rhai_error)
        },
    );
    let b = bridge.clone();
    engine.register_fn(
        "release_check_observe",
        move || -> Result<Dynamic, Box<EvalAltResult>> {
            request(&b, Operation::Observe)
                .map(|o| to_dynamic(&o.value))
                .map_err(rhai_error)
        },
    );
    let b = bridge.clone();
    engine.register_fn(
        "release_check_open",
        move |path: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            open(&b, path.as_str()).map_err(rhai_error)
        },
    );
    let b = bridge.clone();
    engine.register_fn(
        "release_check_text",
        move |text: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            request(&b, Operation::Text(text.to_string()))
                .map(|_| ())
                .map_err(rhai_error)
        },
    );
    let b = bridge.clone();
    engine.register_fn("release_check_pin", move |path: ImmutableString| -> Result<(),Box<EvalAltResult>> {
        let path = fixture_path(path.as_str()).map_err(rhai_error)?;
        // Normal capture/encode/DB producers run on this worker, never the UI thread.
        let frame = crate::video::screenshot::capture_frame(&path,0.5).map_err(|e|rhai_error(e.to_string()))?;
        let webp = webp::Encoder::from_rgba(&frame.rgba,frame.width,frame.height).encode(75.0);
        let db = crate::video_pins::VideoPinDb::open().map_err(|e|rhai_error(e.to_string()))?;
        db.set_pin(&path,0.5,&webp).map_err(|e|rhai_error(e.to_string()))?;
        if db.lookup(&path).is_none_or(|p|p.thumb_webp.is_empty()) { return Err(rhai_error("environment: video pin not persisted")); }
        let pinned=request(&b,Operation::Pin(path.clone())).map_err(rhai_error)?;
        let source=pinned.pins.as_ref().and_then(|db|db.lookup(path.parent().unwrap())).ok_or_else(||rhai_error("environment: normal folder video pin was not persisted"))?;
        if source.db_kind()!="video" || source.rel()!=path.file_name().unwrap().to_string_lossy() {return Err(rhai_error("environment: unexpected folder pin source"));}
        trace("seed",json!({"seed_kind":"video-pin","normal_producer":true,"pin_saved":true,"key":path.to_string_lossy()}));
        Ok(())
    });
    let b = bridge.clone();
    engine.register_fn("release_check_seed", move |kind: ImmutableString| -> Result<(),Box<EvalAltResult>> {
        let o = request(&b,Operation::Observe).map_err(rhai_error)?;
        if o.value["index_settled"] != true || (kind.as_str()=="cache" && o.value["loading_complete"] != true) { return Err(rhai_error("environment: seed not settled")); }
        match kind.as_str() {
            "cache" => {
                let proof = cache_probe(&o).map_err(rhai_error)?;
                let targets=proof["targets"].as_array().unwrap();
                if targets.is_empty() || targets.iter().any(|t|t["catalog_absent"] != false) { return Err(rhai_error("environment: normal cache seed missing kept target")); }
                trace("seed",json!({"seed_kind":"cache","normal_producer":true,"saved_cache":true,"targets":targets,"key":targets[0]["key"]}));
            }
            "search" if o.value["search_done"] == true && o.value["search_searching"] == false => trace("seed",json!({"seed_kind":"search","normal_producer":true,"completed":true,"actual_hits":o.value["search_hits"]})),
            _ => return Err(rhai_error("environment: unknown or unprepared seed")),
        }
        Ok(())
    });
    let b = bridge.clone();
    engine.register_fn(
        "release_check_close",
        move |mode: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            if !matches!(mode.as_str(), "while-loading" | "after-loading") {
                return Err(rhai_error("environment: invalid tray close mode"));
            }
            let deadline = Instant::now() + Duration::from_secs(20);
            let o = loop {
                let o = request(&b, Operation::Observe).map_err(rhai_error)?;
                if tray_witness::close_ready(&o.value, mode.as_str()) {
                    break o;
                }
                if mode.as_str() == "while-loading" && o.value["loading_complete"] == true {
                    return Err(rhai_error(
                        "environment: tray finished before loading close",
                    ));
                }
                if Instant::now() >= deadline {
                    return Err(rhai_error(
                        "environment: tray keep set did not become ready",
                    ));
                }
                b.interrupt
                    .wait(Duration::from_millis(10))
                    .map_err(rhai_error)?;
            };
            let proof = cache_probe(&o).map_err(rhai_error)?;
            if o.value["cache_policy_off"] != true
                || proof["targets"]
                    .as_array()
                    .unwrap()
                    .iter()
                    .any(|t| t["catalog_absent"] != true || t["cache_map_absent"] != true)
            {
                return Err(rhai_error(
                    "environment: tray cache-save suppression/absence not established",
                ));
            }
            {
                let mut s = session().lock().unwrap();
                let s = s.as_mut().unwrap();
                s.close_mode = Some(mode.to_string());
                s.cache_proof = proof;
            }
            let owner = match b.action_selection().map_err(rhai_error)? {
                TestScriptActionSelection::Targeted(
                    owner @ TestScriptWindowIdentity::Root { .. },
                ) => owner,
                _ => return Err(rhai_error("environment: Root selection lost before close")),
            };
            b.validate_selected_owner_fresh(&owner, deadline)
                .map_err(rhai_error)?;
            let hwnd = o.value["root_hwnd"]
                .as_u64()
                .ok_or_else(|| rhai_error("environment: missing root HWND"))?;
            use windows::Win32::Foundation::{HWND, LPARAM, WPARAM};
            use windows::Win32::UI::WindowsAndMessaging::{GetWindowThreadProcessId, PostMessageW};
            b.validate_selected_owner_cached(&owner, deadline)
                .map_err(rhai_error)?;
            let mut native_pid = 0;
            unsafe {
                GetWindowThreadProcessId(HWND(hwnd as usize as *mut _), Some(&mut native_pid));
            }
            if hwnd != owner.hwnd() || native_pid != std::process::id() {
                return Err(rhai_error("environment: close HWND/PID owner changed"));
            }
            unsafe {
                PostMessageW(
                    Some(HWND(hwnd as usize as *mut _)),
                    0x0010,
                    WPARAM(0),
                    LPARAM(0),
                )
            }
            .map_err(|e| rhai_error(e.to_string()))?;
            Ok(())
        },
    );
    let b = bridge.clone();
    engine.register_fn("release_check_pause", move |dir: ImmutableString, seconds: rhai::INT| -> Result<(),Box<EvalAltResult>> {
        let directory=output_path(dir.as_str()).map_err(rhai_error)?;
        if !(30..=60).contains(&seconds) { return Err(rhai_error("environment: release idle pause must be 30..60 seconds")); }
        // Active preparation may wake ROOT to drain real worker messages. It
        // ends BEFORE ready.json and the following no-command idle interval.
        let o=wait_idle_ready(&b).map_err(rhai_error)?;
        let start=unix_ms();
        let target=o.value["targets"].as_array().and_then(|t|t.first()).map(|t|t["key"].clone()).unwrap_or(Value::Null);
        write_json(&directory.join("ready.json"),&json!({"pid":std::process::id(),"root_hwnd":o.value["root_hwnd"],"target_key":target,"sleep_start_unix_ms":start,"sleep_duration_seconds":seconds,"index_scan_wait":if o.value["index_settled"]==true {"settled"} else {"pending"}})).map_err(rhai_error)?;
        b.interrupt.wait(Duration::from_secs(seconds as u64)).map_err(rhai_error)?;
        write_json(&directory.join("completion.json"),&json!({"pid":std::process::id(),"sleep_start_unix_ms":start,"sleep_end_unix_ms":unix_ms()})).map_err(rhai_error)?;
        let deadline=Instant::now()+Duration::from_secs(30);
        while !directory.join("sampler-finished.json").is_file() {
            if Instant::now()>=deadline { return Err(rhai_error("environment: isolated sampler completion timeout")); }
            b.interrupt.wait(Duration::from_millis(50)).map_err(rhai_error)?;
        }
        Ok(())
    });
    let b = bridge.clone();
    engine.register_fn(
        "release_check_evidence",
        move |dir: ImmutableString, values: Map| -> Result<(), Box<EvalAltResult>> {
            evidence(&b, dir.as_str(), values).map_err(rhai_error)
        },
    );
    let b = bridge.clone();
    engine.register_fn(
        "release_check_evidence",
        move |dir: ImmutableString| -> Result<(), Box<EvalAltResult>> {
            evidence(&b, dir.as_str(), Map::new()).map_err(rhai_error)
        },
    );
    let b = bridge.clone();
    engine.register_fn(
        "release_check_quit",
        move || -> Result<(), Box<EvalAltResult>> {
            if request(&b, Operation::Observe).map_err(rhai_error)?.value["tray_available"] != true
            {
                return Err(rhai_error("environment: normal tray Quit unavailable"));
            }
            let mut state = session().lock().unwrap();
            state
                .as_mut()
                .ok_or_else(|| rhai_error("environment: begin release checks first"))?
                .quit_after_finish = true;
            Ok(())
        },
    );
    let b = bridge;
    engine.register_fn("release_check_first_ai",move |path:ImmutableString| -> Result<Dynamic,Box<EvalAltResult>> {
        // Explicit optional case: no pack installation, model download or simulated success.
        use sha2::{Digest,Sha256};
        let model=crate::ai::model_manager::ModelManager::new().model_path(crate::ai::ModelKind::UpscaleRealEsrGeneralV3)
            .ok_or_else(||rhai_error("environment: isolated AI model unavailable"))?;
        let pack=format!("builtin-model-sha256:{:x}",Sha256::digest(std::fs::read(&model).map_err(|e|rhai_error(e.to_string()))?));
        open(&b,path.as_str()).map_err(rhai_error)?;
        let deadline=Instant::now()+Duration::from_secs(180);
        loop {
            let o=request(&b,Operation::Observe).map_err(rhai_error)?;
            if o.value["loading_complete"]==true {break;}
            if Instant::now()>=deadline {return Err(rhai_error("environment: first AI folder did not settle"));}
            b.interrupt.wait(Duration::from_millis(100)).map_err(rhai_error)?;
        }
        super::tap_key_impl(&b,"Home",crate::key_input::SyntheticModifiers::default())?;
        super::tap_key_impl(&b,"Enter",crate::key_input::SyntheticModifiers::default())?;
        loop {
            let o=request(&b,Operation::Observe).map_err(rhai_error)?;
            if o.value["is_fullscreen"]==true {
                if o.value["ai_ready"].as_u64().unwrap_or(0)!=0 || o.value["ai_pending"].as_u64().unwrap_or(0)!=0 {return Err(rhai_error("environment: first AI case already has AI work"));}
                break;
            }
            if Instant::now()>=deadline {return Err(rhai_error("environment: first AI fullscreen unavailable"));}
            b.interrupt.wait(Duration::from_millis(100)).map_err(rhai_error)?;
        }
        let start=crate::perf::program_start().ok_or_else(||rhai_error("environment: perf clock unavailable"))?;
        let start_t=start.elapsed().as_secs_f64();
        b.run_action(crate::keymap::KeyAction::FsAiModelRealEsrGeneralV3,Duration::from_secs(30)).map_err(rhai_error)?;
        loop {
            let o=request(&b,Operation::Observe).map_err(rhai_error)?;
            if o.value["ai_ready"].as_u64().unwrap_or(0)>0 && o.value["ai_pending"]==0 && o.value["ai_backend"].is_string() {
                let proof=json!({"included":true,"first":true,"started":true,"start_t":start_t,
                    "end_t":start.elapsed().as_secs_f64(),"model":"realesr_general_v3","backend":o.value["ai_backend"],"pack":pack});
                protocol_ai_trace(&proof);
                b.run_action(crate::keymap::KeyAction::FsClose,Duration::from_secs(30)).map_err(rhai_error)?;
                return Ok(to_dynamic(&proof));
            }
            if Instant::now()>=deadline {return Err(rhai_error("environment: actual isolated AI inference did not complete"));}
            b.interrupt.wait(Duration::from_millis(100)).map_err(rhai_error)?;
        }
    });
}
fn wait_idle_ready(bridge: &RunnerBridge) -> Result<Observation, String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let observation = request(bridge, Operation::Observe)?;
        let ready = {
            let state = session().lock().unwrap();
            let state = state.as_ref().ok_or("environment: release session lost")?;
            match state.close_mode.as_deref() {
                Some(mode) => crate::perf::program_start().is_some_and(|start| {
                    observation.value["window_visible"] == false
                        && tray_witness::established(
                            &state.witnesses,
                            mode,
                            start.elapsed().as_secs_f64(),
                        )
                        .is_some()
                }),
                None => {
                    observation.value["loading_complete"] == true
                        && observation.value["index_settled"] == true
                }
            }
        };
        if ready {
            return Ok(observation);
        }
        if Instant::now() >= deadline {
            return Err(
                "environment: canonical tray request witnesses not established before idle sleep"
                    .into(),
            );
        }
        bridge.interrupt.wait(Duration::from_millis(25))?;
    }
}

fn protocol_ai_trace(proof: &Value) {
    trace("first_ai", proof.clone());
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn release_layout_refuses_real_data_and_other_binaries() {
        let root = Path::new(r"C:\repo\target\portable-smoke");
        let output = Path::new(r"C:\repo\target\ui-smoke\runs\run1\static");
        let exe = root.join("mimageviewer.exe");
        let data = root.join("data");
        assert!(validate_layout(&exe, &data, output).is_ok());
        assert!(validate_layout(&root.join("mimageviewer-core.exe"), &data, output).is_err());
        assert!(
            validate_layout(
                &exe,
                Path::new(r"C:\Users\user\AppData\Roaming\mimageviewer"),
                output
            )
            .is_err()
        );
        assert!(
            validate_layout(&exe, &data, Path::new(r"C:\repo\target\ui-smoke\runs2\x")).is_err()
        );
    }
    #[test]
    fn script_evidence_keeps_types_and_nested_denominators() {
        let expected = json!({"completed":true,"n":200,"paths":["one","two"],"raw":0.3});
        assert_eq!(from_dynamic(to_dynamic(&expected)).unwrap(), expected);
    }
    #[test]
    fn release_protocol_message_is_published_as_complete_json() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("ready.json");
        write_json(
            &path,
            &json!({"pid":42,"sleep_start_unix_ms":1700000000000_u64}),
        )
        .unwrap();
        assert!(!path.with_extension("json.tmp").exists());
        let value: Value = serde_json::from_slice(&std::fs::read(path).unwrap()).unwrap();
        assert_eq!(value["pid"], 42);
    }
}

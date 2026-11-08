//! Shared model-addon transfer, placement and single worker owner.
//!
//! UI entry points only start workers or read cached results. Installing,
//! cancelling and removing remain exclusive until their completion is polled.

use std::fs::{self, File};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

#[cfg(test)]
use crate::vlm_addon_download::CANCELLED;
use crate::vlm_addon_download::{self as http};

pub(crate) const INDEX_NAME: &str = "vlm-model-index.json";
const DEFAULT_BASE_URL: &str =
    "https://github.com/MikageSawatari/mimageviewer/releases/download/vlm-model-v1";
pub(crate) const MAX_MODEL_BYTES: u64 = 16 * 1024 * 1024 * 1024;
pub(crate) const MAX_PART_BYTES: u64 = 2 * 1024 * 1024 * 1024 - 1;

#[cfg(test)]
mod depth_tests {
    use super::*;
    use crate::depth_addon::tests::fixture;
    use crate::vlm_addon_download::tests::{ok, server};

    #[test]
    fn depth_install_verifies_all_files_and_uses_its_own_index() {
        let dir = tempfile::tempdir().unwrap();
        let (index, bodies) = fixture();
        let mut responses = vec![ok(&serde_json::to_vec(&index).unwrap())];
        responses.extend(bodies.iter().map(|body| ok(body)));
        let (base, requests, join) = server(responses);
        let (tx, _rx) = mpsc::sync_channel(8);
        install_for_kind(
            AddonKind::Depth,
            dir.path(),
            &base,
            &AtomicBool::new(false),
            &tx,
        )
        .unwrap();
        join.join().unwrap();
        let requested: Vec<_> = requests
            .try_iter()
            .map(|request| request.lines().next().unwrap().to_owned())
            .collect();
        assert_eq!(
            requested,
            [
                "GET /depth-model-index.json HTTP/1.1",
                "GET /da3mono_large_fp16_504.onnx HTTP/1.1",
                "GET /LICENSE HTTP/1.1",
                "GET /NOTICE HTTP/1.1",
            ]
        );
        let status = scan_for_kind(AddonKind::Depth, dir.path(), &AtomicBool::new(false));
        assert!(matches!(
            status,
            AddonStatus::Valid {
                mmproj_path: None,
                ..
            }
        ));
        let pack = dir.path().join("packs").join(&index.model_id);
        assert!(pack.join("INSTALL_OK").exists());
        assert!(!pack.join(INDEX_NAME).exists());
        fs::write(pack.join("NOTICE"), b"broken").unwrap();
        assert!(matches!(
            scan_for_kind(AddonKind::Depth, dir.path(), &AtomicBool::new(false)),
            AddonStatus::Corrupt(_)
        ));
        // Equal byte length above isolates SHA-256 validation. Missing indexed
        // assets and a truncated asset independently reject installation too.
        fs::write(pack.join("NOTICE"), &bodies[2]).unwrap();
        fs::remove_file(pack.join("LICENSE")).unwrap();
        assert!(matches!(
            scan_for_kind(AddonKind::Depth, dir.path(), &AtomicBool::new(false)),
            AddonStatus::Corrupt(_)
        ));
        fs::write(pack.join("LICENSE"), b"short").unwrap();
        assert!(matches!(
            scan_for_kind(AddonKind::Depth, dir.path(), &AtomicBool::new(false)),
            AddonStatus::Corrupt(_)
        ));
        fs::write(pack.join("LICENSE"), &bodies[1]).unwrap();
        assert!(matches!(
            scan_for_kind(AddonKind::Depth, dir.path(), &AtomicBool::new(false)),
            AddonStatus::Valid { .. }
        ));
        fs::remove_file(pack.join("INSTALL_OK")).unwrap();
        assert_eq!(
            scan_for_kind(AddonKind::Depth, dir.path(), &AtomicBool::new(false)),
            AddonStatus::Missing
        );
    }

    #[test]
    fn depth_rejects_wrong_index_before_requesting_assets() {
        let dir = tempfile::tempdir().unwrap();
        let (mut index, _) = fixture();
        index.files[0].name = "../escape.onnx".into();
        let (base, requests, join) = server(vec![ok(&serde_json::to_vec(&index).unwrap())]);
        let (tx, _rx) = mpsc::sync_channel(8);
        assert!(
            install_for_kind(
                AddonKind::Depth,
                dir.path(),
                &base,
                &AtomicBool::new(false),
                &tx
            )
            .is_err()
        );
        join.join().unwrap();
        assert_eq!(requests.try_iter().count(), 1);
        assert!(!dir.path().join("downloads").exists());
        assert!(!dir.path().join("active.json").exists());
    }

    #[test]
    fn depth_removal_preserves_sibling_addon_and_user_data() {
        let dir = tempfile::tempdir().unwrap();
        let depth = dir.path().join("addons").join("depth");
        let vlm = dir.path().join("addons").join("vlm");
        fs::create_dir_all(depth.join("downloads")).unwrap();
        fs::write(depth.join("downloads").join("partial"), b"unfinished").unwrap();
        fs::create_dir_all(&vlm).unwrap();
        fs::write(vlm.join("active.json"), b"sibling").unwrap();
        fs::write(dir.path().join("settings.db"), b"user data").unwrap();
        remove(&depth).unwrap();
        assert!(!depth.exists());
        assert_eq!(fs::read(vlm.join("active.json")).unwrap(), b"sibling");
        assert_eq!(
            fs::read(dir.path().join("settings.db")).unwrap(),
            b"user data"
        );
    }

    #[test]
    fn addon_kinds_keep_published_urls_and_storage_names_separate() {
        assert_eq!(AddonKind::Vlm.index_name(), "vlm-model-index.json");
        assert!(AddonKind::Vlm.default_base_url().ends_with("/vlm-model-v1"));
        assert_eq!(AddonKind::Depth.index_name(), "depth-model-index.json");
        assert!(
            AddonKind::Depth
                .default_base_url()
                .ends_with("/depth-model-v1")
        );
    }

    #[test]
    fn addon_thread_names_preserve_vlm_contract_and_identify_depth() {
        assert_eq!(AddonKind::Vlm.scan_thread_name(), "vlm-model-scan");
        assert_eq!(AddonKind::Vlm.install_thread_name(), "vlm-model-install");
        assert_eq!(AddonKind::Vlm.remove_thread_name(), "vlm-model-remove");
        assert_eq!(AddonKind::Depth.scan_thread_name(), "depth-model-scan");
        assert_eq!(
            AddonKind::Depth.install_thread_name(),
            "depth-model-install"
        );
        assert_eq!(AddonKind::Depth.remove_thread_name(), "depth-model-remove");
    }

    #[test]
    fn addon_scan_diagnostics_preserve_vlm_in_lower_error_text() {
        let dir = tempfile::tempdir().unwrap();
        let raw = br#"{"schema":1,"model_id":"VLM-id","VLM_detail":"VLM-source"}"#;
        fs::write(dir.path().join("active.json"), raw).unwrap();
        let source = serde_json::from_slice::<ActivePointer>(raw)
            .err()
            .unwrap()
            .to_string();
        assert!(source.contains("VLM_detail"));
        for (kind, label) in [(AddonKind::Vlm, "VLM"), (AddonKind::Depth, "Depth")] {
            assert_eq!(
                scan_for_kind(kind, dir.path(), &AtomicBool::new(false)),
                AddonStatus::Corrupt(format!("Invalid {label} active.json: {source}"))
            );
        }
    }

    #[test]
    fn addon_scan_diagnostics_preserve_vlm_in_user_path() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("VLM-user-data");
        let pack = root.join("packs").join("VLM-model");
        fs::create_dir_all(&pack).unwrap();
        fs::write(
            root.join("active.json"),
            br#"{"schema":1,"model_id":"VLM-model"}"#,
        )
        .unwrap();
        fs::write(pack.join("INSTALL_OK"), b"1").unwrap();
        for kind in [AddonKind::Vlm, AddonKind::Depth] {
            let path = pack.join(kind.index_name());
            let source = File::open(&path).err().unwrap();
            let expected = format!("Open {}: {source}", path.display());
            assert_eq!(
                scan_for_kind(kind, &root, &AtomicBool::new(false)),
                AddonStatus::Corrupt(expected)
            );
        }
    }

    #[test]
    fn shared_validator_diagnostics_keep_exact_vlm_text() {
        assert_eq!(validate_hash("invalid").unwrap_err(), "Invalid VLM SHA-256");
        assert_eq!(
            validate_hash_for_kind(AddonKind::Depth, "invalid").unwrap_err(),
            "Invalid Depth SHA-256"
        );
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddonKind {
    Vlm,
    Depth,
}

impl AddonKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Vlm => "VLM",
            Self::Depth => "Depth",
        }
    }
    pub(crate) fn cancelled(self) -> &'static str {
        match self {
            Self::Vlm => http::CANCELLED,
            Self::Depth => "Depth installation cancelled",
        }
    }
    fn scan_thread_name(self) -> &'static str {
        match self {
            Self::Vlm => "vlm-model-scan",
            Self::Depth => "depth-model-scan",
        }
    }
    fn install_thread_name(self) -> &'static str {
        match self {
            Self::Vlm => "vlm-model-install",
            Self::Depth => "depth-model-install",
        }
    }
    fn remove_thread_name(self) -> &'static str {
        match self {
            Self::Vlm => "vlm-model-remove",
            Self::Depth => "depth-model-remove",
        }
    }
    pub fn index_name(self) -> &'static str {
        match self {
            Self::Vlm => INDEX_NAME,
            Self::Depth => "depth-model-index.json",
        }
    }
    pub fn default_base_url(self) -> &'static str {
        match self {
            Self::Vlm => DEFAULT_BASE_URL,
            Self::Depth => {
                "https://github.com/MikageSawatari/mimageviewer/releases/download/depth-model-v1"
            }
        }
    }
    fn base_url(self) -> String {
        let variable = match self {
            Self::Vlm => "MIV_VLM_MODEL_BASE_URL",
            Self::Depth => "MIV_DEPTH_MODEL_BASE_URL",
        };
        #[cfg(debug_assertions)]
        if let Ok(value) = std::env::var(variable) {
            if !value.trim().is_empty() {
                return value.trim().trim_end_matches('/').into();
            }
        }
        #[cfg(not(debug_assertions))]
        let _ = variable;
        self.default_base_url().into()
    }
}

/// The published schema. Unsplit files use an empty `parts` list and their own
/// name as the release asset name. Split assets are concatenated in list order.
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelIndex {
    pub schema: u32,
    pub model_id: String,
    pub files: Vec<ModelFile>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum FileKind {
    Model,
    Mmproj,
    License,
    Notice,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelFile {
    pub name: String,
    pub kind: FileKind,
    pub bytes: u64,
    pub sha256: String,
    pub license: String,
    #[serde(default)]
    pub parts: Vec<ModelPart>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ModelPart {
    pub name: String,
    pub bytes: u64,
    pub sha256: String,
}

impl ModelIndex {
    pub fn total_bytes(&self) -> u64 {
        self.files.iter().map(|f| f.bytes).sum()
    }

    pub fn validate_for(&self, kind: AddonKind) -> Result<(), String> {
        match kind {
            AddonKind::Vlm => self.validate(),
            AddonKind::Depth => crate::depth_addon::validate_index(self),
        }
    }

    pub(crate) fn file(&self, kind: FileKind) -> &ModelFile {
        self.files
            .iter()
            .find(|f| f.kind == kind)
            .expect("validated VLM index")
    }
}

pub(crate) fn validate_hash(hash: &str) -> Result<(), String> {
    validate_hash_for_kind(AddonKind::Vlm, hash)
}

pub(crate) fn validate_hash_for_kind(kind: AddonKind, hash: &str) -> Result<(), String> {
    let label = kind.label();
    if hash.len() != 64 || !hash.bytes().all(|c| c.is_ascii_hexdigit()) {
        return Err(format!("Invalid {label} SHA-256"));
    }
    Ok(())
}

pub(crate) fn validate_name(name: &str) -> Result<(), String> {
    validate_name_for_kind(AddonKind::Vlm, name)
}

pub(crate) fn validate_name_for_kind(kind: AddonKind, name: &str) -> Result<(), String> {
    let label = kind.label();
    crate::editing_addon::validate_safe_filename(name)?;
    // Also reject Windows device names, ADS, trailing dots and URL metacharacters.
    if !name
        .bytes()
        .all(|c| c.is_ascii_alphanumeric() || b"-_.".contains(&c))
        || name.ends_with('.')
    {
        return Err(format!("Invalid {label} filename characters"));
    }
    let stem = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    if ["CON", "PRN", "AUX", "NUL"].contains(&stem.as_str())
        || ((stem.starts_with("COM") || stem.starts_with("LPT"))
            && stem.len() == 4
            && matches!(stem.as_bytes()[3], b'1'..=b'9'))
    {
        return Err("Reserved Windows filename".into());
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddonStatus {
    Valid {
        model_id: String,
        bytes: u64,
        model_path: PathBuf,
        mmproj_path: Option<PathBuf>,
    },
    Missing,
    Corrupt(String),
}

impl AddonStatus {
    pub fn is_installed(&self) -> bool {
        matches!(self, Self::Valid { .. })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AddonOperation {
    None,
    Installing,
    Cancelling,
    Removing,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum InstallProgress {
    #[default]
    Idle,
    FetchingIndex,
    Downloading {
        bytes_done: u64,
        bytes_total: u64,
    },
    Verifying,
    Concatenating,
    Installing,
}

#[derive(Debug, Clone)]
pub enum AddonEvent {
    StatusChanged,
    Installed { model_id: String },
    Cancelled,
    Removed,
    Error { message: String },
}

enum WorkerMessage {
    Progress(InstallProgress),
    Complete {
        status: AddonStatus,
        event: AddonEvent,
    },
}

struct Running {
    rx: Receiver<WorkerMessage>,
    cancel: Arc<AtomicBool>,
}

enum WorkerState {
    Idle,
    Scanning(Running),
    Installing(Running),
    Cancelling(Running),
    Removing(Running),
}

impl WorkerState {
    fn running(&self) -> Option<&Running> {
        match self {
            Self::Idle => None,
            Self::Scanning(running)
            | Self::Installing(running)
            | Self::Cancelling(running)
            | Self::Removing(running) => Some(running),
        }
    }
}

/// One UI-owned model lifecycle. No join, disk reads or HTTP on its caller.
pub type VlmAddon = ModelAddon;

pub struct ModelAddon {
    kind: AddonKind,
    root: PathBuf,
    status: AddonStatus,
    state: WorkerState,
    progress: InstallProgress,
    scanned: bool,
}

impl Default for ModelAddon {
    fn default() -> Self {
        Self::new(crate::data_dir::get().join("addons").join("vlm"))
    }
}

impl ModelAddon {
    #[cfg(test)]
    pub(crate) fn with_test_status(root: PathBuf, status: AddonStatus) -> Self {
        let mut addon = Self::new(root);
        addon.set_status_for_test(status);
        addon
    }

    /// A lifecycle fixture with no worker, HTTP, or filesystem activity.
    #[cfg(test)]
    pub(crate) fn with_test_operation(root: PathBuf, operation: AddonOperation) -> Self {
        let mut addon = Self::with_test_status(root, AddonStatus::Missing);
        let running = Running {
            rx: std::sync::mpsc::sync_channel(1).1,
            cancel: Arc::new(AtomicBool::new(false)),
        };
        addon.state = match operation {
            AddonOperation::None => WorkerState::Idle,
            AddonOperation::Installing => WorkerState::Installing(running),
            AddonOperation::Cancelling => WorkerState::Cancelling(running),
            AddonOperation::Removing => WorkerState::Removing(running),
        };
        addon
    }

    #[cfg(test)]
    pub(crate) fn set_status_for_test(&mut self, status: AddonStatus) {
        self.status = status;
        self.scanned = true;
    }

    pub fn new(root: PathBuf) -> Self {
        Self::new_for_kind(AddonKind::Vlm, root)
    }

    pub fn new_for_kind(kind: AddonKind, root: PathBuf) -> Self {
        Self {
            kind,
            root,
            status: AddonStatus::Missing,
            state: WorkerState::Idle,
            progress: InstallProgress::Idle,
            scanned: false,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }
    pub fn status(&self) -> &AddonStatus {
        &self.status
    }
    pub fn operation(&self) -> AddonOperation {
        match self.state {
            WorkerState::Idle | WorkerState::Scanning(_) => AddonOperation::None,
            WorkerState::Installing(_) => AddonOperation::Installing,
            WorkerState::Cancelling(_) => AddonOperation::Cancelling,
            WorkerState::Removing(_) => AddonOperation::Removing,
        }
    }
    pub fn progress(&self) -> &InstallProgress {
        &self.progress
    }
    pub fn is_scanning(&self) -> bool {
        matches!(self.state, WorkerState::Scanning(_))
    }

    /// Lazy startup scan: repeated UI draws do not repeatedly hash gigabytes.
    pub fn request_scan(&mut self) {
        if self.scanned || self.state.running().is_some() {
            return;
        }
        let kind = self.kind;
        match self.spawn(kind.scan_thread_name(), move |root, cancel, _| {
            (
                scan_for_kind(kind, &root, &cancel),
                AddonEvent::StatusChanged,
            )
        }) {
            Ok(running) => self.state = WorkerState::Scanning(running),
            Err(message) => {
                self.scanned = true;
                self.status = AddonStatus::Corrupt(message);
            }
        }
    }

    pub fn start_install(&mut self) -> Result<(), String> {
        self.ensure_idle()?;
        let kind = self.kind;
        let running = self.spawn(kind.install_thread_name(), move |root, cancel, tx| {
            let result = install_for_kind(kind, &root, &kind.base_url(), &cancel, tx);
            let status = scan_for_kind(kind, &root, &AtomicBool::new(false));
            let event = match result {
                Ok(model_id) => AddonEvent::Installed { model_id },
                Err(error) if error == kind.cancelled() => AddonEvent::Cancelled,
                Err(message) => AddonEvent::Error { message },
            };
            (status, event)
        })?;
        self.state = WorkerState::Installing(running);
        self.progress = InstallProgress::FetchingIndex;
        Ok(())
    }

    pub fn cancel(&mut self) {
        match std::mem::replace(&mut self.state, WorkerState::Idle) {
            WorkerState::Installing(running) => {
                running.cancel.store(true, Ordering::Relaxed);
                self.state = WorkerState::Cancelling(running);
            }
            other => self.state = other,
        }
    }

    pub fn start_remove(&mut self) -> Result<(), String> {
        self.ensure_idle()?;
        let kind = self.kind;
        let running = self.spawn(kind.remove_thread_name(), move |root, _, _| {
            let event = match remove_for_kind(kind, &root) {
                Ok(()) => AddonEvent::Removed,
                Err(message) => AddonEvent::Error { message },
            };
            (scan_for_kind(kind, &root, &AtomicBool::new(false)), event)
        })?;
        self.state = WorkerState::Removing(running);
        Ok(())
    }

    fn ensure_idle(&self) -> Result<(), String> {
        if self.state.running().is_some() {
            Err("前の処理を片付けています".into())
        } else {
            Ok(())
        }
    }

    fn spawn(
        &self,
        name: &str,
        work: impl FnOnce(
            PathBuf,
            Arc<AtomicBool>,
            &mpsc::SyncSender<WorkerMessage>,
        ) -> (AddonStatus, AddonEvent)
        + Send
        + 'static,
    ) -> Result<Running, String> {
        let (tx, rx) = mpsc::sync_channel(8);
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let root = self.root.clone();
        std::thread::Builder::new()
            .name(name.into())
            .spawn(move || {
                let (status, event) = work(root, worker_cancel, &tx);
                // No filesystem work after this message: receipt is the completion barrier.
                let _ = tx.send(WorkerMessage::Complete { status, event });
            })
            .map_err(|e| format!("Start {} worker: {e}", self.kind.label()))?;
        Ok(Running { rx, cancel })
    }

    pub fn poll(&mut self) -> Option<AddonEvent> {
        for _ in 0..16 {
            let result = self.state.running()?.rx.try_recv();
            match result {
                Ok(WorkerMessage::Progress(progress)) => self.progress = progress,
                Ok(WorkerMessage::Complete { status, event }) => {
                    self.status = status;
                    self.finish();
                    return Some(event);
                }
                Err(mpsc::TryRecvError::Empty) => return None,
                Err(mpsc::TryRecvError::Disconnected) => {
                    let message = format!("{} worker stopped unexpectedly", self.kind.label());
                    self.status = AddonStatus::Corrupt(message.clone());
                    self.finish();
                    return Some(AddonEvent::Error { message });
                }
            }
        }
        None
    }

    fn finish(&mut self) {
        self.state = WorkerState::Idle;
        self.scanned = true;
        self.progress = InstallProgress::Idle;
    }
}

impl Drop for ModelAddon {
    fn drop(&mut self) {
        if let Some(running) = self.state.running() {
            running.cancel.store(true, Ordering::Relaxed);
        }
    }
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct ActivePointer {
    schema: u32,
    model_id: String,
}

/// Reject reparse points in every existing component, including parent paths.
pub(crate) fn reject_links_for_kind(kind: AddonKind, path: &Path) -> Result<(), String> {
    let label = kind.label();
    for component in path.ancestors() {
        match fs::symlink_metadata(component) {
            Ok(metadata) => {
                let mut link = metadata.file_type().is_symlink();
                #[cfg(windows)]
                {
                    use std::os::windows::fs::MetadataExt;
                    link |= metadata.file_attributes() & 0x400 != 0;
                }
                if link {
                    return Err(format!(
                        "{label} path contains a reparse point: {}",
                        component.display()
                    ));
                }
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Check {label} path: {error}")),
        }
    }
    Ok(())
}

fn read_bounded_for_kind(kind: AddonKind, path: &Path) -> Result<Vec<u8>, String> {
    let label = kind.label();
    reject_links_for_kind(kind, path)?;
    let mut bytes = Vec::new();
    File::open(path)
        .map_err(|e| format!("Open {}: {e}", path.display()))?
        .take(256 * 1024 + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| format!("Read {label} manifest: {e}"))?;
    if bytes.len() > 256 * 1024 {
        return Err(format!("{label} manifest is too large"));
    }
    Ok(bytes)
}

#[cfg(test)]
fn scan(root: &Path, cancel: &AtomicBool) -> AddonStatus {
    scan_for_kind(AddonKind::Vlm, root, cancel)
}

pub(crate) fn scan_for_kind(kind: AddonKind, root: &Path, cancel: &AtomicBool) -> AddonStatus {
    match scan_inner(kind, root, cancel) {
        Ok(Some(status)) => status,
        Ok(None) => AddonStatus::Missing,
        Err(message) => AddonStatus::Corrupt(message),
    }
}

fn scan_inner(
    kind: AddonKind,
    root: &Path,
    cancel: &AtomicBool,
) -> Result<Option<AddonStatus>, String> {
    let label = kind.label();
    reject_links_for_kind(kind, root)?;
    let pointer_path = root.join("active.json");
    reject_links_for_kind(kind, &pointer_path)?;
    if !pointer_path
        .try_exists()
        .map_err(|e| format!("Read active {label}: {e}"))?
    {
        return Ok(None);
    }
    let pointer: ActivePointer =
        serde_json::from_slice(&read_bounded_for_kind(kind, &pointer_path)?)
            .map_err(|e| format!("Invalid {label} active.json: {e}"))?;
    if pointer.schema != 1 {
        return Err(format!("Unsupported {label} active pointer"));
    }
    validate_name_for_kind(kind, &pointer.model_id)?;
    let pack = root.join("packs").join(&pointer.model_id);
    let sentinel = pack.join("INSTALL_OK");
    reject_links_for_kind(kind, &sentinel)?;
    if !sentinel
        .try_exists()
        .map_err(|e| format!("Read {label} sentinel: {e}"))?
    {
        return Ok(None);
    }
    let index: ModelIndex =
        serde_json::from_slice(&read_bounded_for_kind(kind, &pack.join(kind.index_name()))?)
            .map_err(|e| format!("Invalid installed {label} index: {e}"))?;
    index.validate_for(kind)?;
    if index.model_id != pointer.model_id {
        return Err(format!("{label} model pointer mismatch"));
    }
    for file in &index.files {
        let path = pack.join(&file.name);
        reject_links_for_kind(kind, &path)?;
        let metadata = fs::metadata(&path).map_err(|e| format!("{label} file missing: {e}"))?;
        if !metadata.is_file()
            || metadata.len() != file.bytes
            || !http::sha256_file_for_kind(kind, &path, cancel)?.eq_ignore_ascii_case(&file.sha256)
        {
            return Err(format!("{label} file is corrupt: {}", file.name));
        }
    }
    Ok(Some(AddonStatus::Valid {
        model_id: index.model_id.clone(),
        bytes: index.total_bytes(),
        model_path: pack.join(&index.file(FileKind::Model).name),
        mmproj_path: index
            .files
            .iter()
            .find(|file| file.kind == FileKind::Mmproj)
            .map(|file| pack.join(&file.name)),
    }))
}

fn remove_tree(kind: AddonKind, path: &Path) -> Result<(), String> {
    let label = kind.label();
    reject_links_for_kind(kind, path)?;
    if !path
        .try_exists()
        .map_err(|e| format!("Check {label} directory: {e}"))?
    {
        return Ok(());
    }
    // Do not recurse through reparse points, even if an unexpected one was placed
    // inside a failed download directory.
    for entry in fs::read_dir(path).map_err(|e| format!("Read {label} directory: {e}"))? {
        let entry = entry.map_err(|e| format!("Read {label} entry: {e}"))?;
        let path = entry.path();
        reject_links_for_kind(kind, &path)?;
        if entry
            .file_type()
            .map_err(|e| format!("Read {label} type: {e}"))?
            .is_dir()
        {
            remove_tree(kind, &path)?;
        } else {
            fs::remove_file(path).map_err(|e| format!("Remove {label} file: {e}"))?;
        }
    }
    fs::remove_dir(path).map_err(|e| format!("Remove {label} directory: {e}"))
}

fn remove_for_kind(kind: AddonKind, root: &Path) -> Result<(), String> {
    let label = kind.label();
    reject_links_for_kind(kind, root)?;
    let active = root.join("active.json");
    reject_links_for_kind(kind, &active)?;
    match fs::remove_file(active) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Remove {label} pointer: {error}")),
    }
    remove_tree(kind, root)
}

fn install_for_kind(
    kind: AddonKind,
    root: &Path,
    base: &str,
    cancel: &AtomicBool,
    tx: &mpsc::SyncSender<WorkerMessage>,
) -> Result<String, String> {
    let label = kind.label();
    let raw = http::fetch_index_for_kind(kind, &format!("{base}/{}", kind.index_name()), cancel)?;
    let index: ModelIndex =
        serde_json::from_slice(&raw).map_err(|e| format!("Invalid {label} index: {e}"))?;
    index.validate_for(kind)?;
    reject_links_for_kind(kind, root)?;
    let downloads = root.join("downloads").join(&index.model_id);
    reject_links_for_kind(kind, &downloads)?;
    fs::create_dir_all(&downloads).map_err(|e| format!("Create {label} downloads: {e}"))?;
    let staging = downloads.join("staging");
    reject_links_for_kind(kind, &staging)?;
    fs::create_dir_all(&staging).map_err(|e| format!("Create {label} staging: {e}"))?;
    // Completed files remain useful when cancellation happens in a later file.
    // Keep only this validated index's outputs; never publish unrelated remnants.
    for entry in fs::read_dir(&staging).map_err(|e| format!("Read {label} staging: {e}"))? {
        let entry = entry.map_err(|e| format!("Read {label} staging entry: {e}"))?;
        let path = entry.path();
        reject_links_for_kind(kind, &path)?;
        if !index
            .files
            .iter()
            .any(|file| entry.file_name() == file.name.as_str())
        {
            if entry
                .file_type()
                .map_err(|e| format!("Read {label} staging type: {e}"))?
                .is_dir()
            {
                remove_tree(kind, &path)?;
            } else {
                fs::remove_file(path)
                    .map_err(|e| format!("Remove {label} staging remnant: {e}"))?;
            }
        }
    }
    let mut done = 0;
    for file in &index.files {
        let staged_file = staging.join(&file.name);
        reject_links_for_kind(kind, &staged_file)?;
        let _ = tx.try_send(WorkerMessage::Progress(InstallProgress::Verifying));
        match fs::metadata(&staged_file) {
            Ok(metadata) if metadata.is_file() && metadata.len() == file.bytes => {
                if http::sha256_file_for_kind(kind, &staged_file, cancel)?
                    .eq_ignore_ascii_case(&file.sha256)
                {
                    // A cancellation at concatenation's last checkpoint can leave
                    // both a complete output and its verified source partials.
                    // Reclaim only this file's declared assets after revalidation.
                    let names: Vec<_> = if file.parts.is_empty() {
                        vec![file.name.as_str()]
                    } else {
                        file.parts.iter().map(|part| part.name.as_str()).collect()
                    };
                    for name in names {
                        let partial = downloads.join(format!("{name}.partial"));
                        reject_links_for_kind(kind, &partial)?;
                        match fs::remove_file(partial) {
                            Ok(()) => {}
                            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                            Err(error) => {
                                return Err(format!("Remove verified {label} part: {error}"));
                            }
                        }
                    }
                    done += file.bytes;
                    let _ = tx.try_send(WorkerMessage::Progress(InstallProgress::Downloading {
                        bytes_done: done,
                        bytes_total: index.total_bytes(),
                    }));
                    continue;
                }
            }
            Ok(metadata) if metadata.is_dir() => {
                return Err(format!("{label} staging file is a directory"));
            }
            Ok(_) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => return Err(format!("Check staged {label} file: {error}")),
        }
        let mut sources = Vec::new();
        let assets: Vec<_> = if file.parts.is_empty() {
            vec![(file.name.as_str(), file.bytes, file.sha256.as_str())]
        } else {
            file.parts
                .iter()
                .map(|p| (p.name.as_str(), p.bytes, p.sha256.as_str()))
                .collect()
        };
        for (name, bytes, hash) in assets {
            let partial = downloads.join(format!("{name}.partial"));
            let mut last_progress = std::time::Instant::now();
            http::download_for_kind(
                kind,
                &format!("{base}/{name}"),
                &partial,
                bytes,
                hash,
                cancel,
                |current| {
                    if current == bytes
                        || last_progress.elapsed() >= std::time::Duration::from_millis(50)
                    {
                        let _ =
                            tx.try_send(WorkerMessage::Progress(InstallProgress::Downloading {
                                bytes_done: done + current,
                                bytes_total: index.total_bytes(),
                            }));
                        last_progress = std::time::Instant::now();
                    }
                },
            )?;
            done += bytes;
            sources.push(partial);
        }
        let _ = tx.try_send(WorkerMessage::Progress(InstallProgress::Concatenating));
        concatenate_for_kind(kind, &sources, &staged_file, file, cancel)?;
        // Verified source assets can be removed only after the whole file matches.
        for source in sources {
            fs::remove_file(source).map_err(|e| format!("Remove {label} part: {e}"))?;
        }
    }
    http::check_cancel_for_kind(kind, cancel)?;
    fs::write(staging.join(kind.index_name()), &raw)
        .map_err(|e| format!("Save {label} index: {e}"))?;
    let packs = root.join("packs");
    reject_links_for_kind(kind, &packs)?;
    fs::create_dir_all(&packs).map_err(|e| format!("Create {label} packs: {e}"))?;
    let destination = packs.join(&index.model_id);
    // A repair of this immutable model replaces only its own directory. No other
    // pack or user-authored caption data is touched.
    http::check_cancel_for_kind(kind, cancel)?;
    // Commit boundary: once replacing the destination starts, finish publishing
    // rather than allowing cancellation to leave the old pointer without a pack.
    remove_tree(kind, &destination)?;
    let _ = tx.try_send(WorkerMessage::Progress(InstallProgress::Installing));
    fs::rename(&staging, &destination).map_err(|e| format!("Publish {label} model: {e}"))?;
    fs::write(destination.join("INSTALL_OK"), b"1\n")
        .map_err(|e| format!("Write {label} sentinel: {e}"))?;
    let pointer = serde_json::to_vec(&ActivePointer {
        schema: 1,
        model_id: index.model_id.clone(),
    })
    .map_err(|e| format!("Encode {label} pointer: {e}"))?;
    let temporary = root.join("active.json.tmp");
    reject_links_for_kind(kind, &temporary)?;
    reject_links_for_kind(kind, &root.join("active.json"))?;
    fs::write(&temporary, pointer).map_err(|e| format!("Write {label} pointer: {e}"))?;
    fs::rename(temporary, root.join("active.json"))
        .map_err(|e| format!("Publish {label} pointer: {e}"))?;
    Ok(index.model_id)
}

fn concatenate_for_kind(
    kind: AddonKind,
    sources: &[PathBuf],
    output: &Path,
    file: &ModelFile,
    cancel: &AtomicBool,
) -> Result<(), String> {
    let label = kind.label();
    reject_links_for_kind(kind, output)?;
    let mut target = File::create(output).map_err(|e| format!("Create {label} model: {e}"))?;
    let mut hash = Sha256::new();
    let mut total = 0u64;
    let mut buffer = vec![0; 256 * 1024];
    for source in sources {
        reject_links_for_kind(kind, source)?;
        let mut source = File::open(source).map_err(|e| format!("Open {label} part: {e}"))?;
        loop {
            http::check_cancel_for_kind(kind, cancel)?;
            let n = source
                .read(&mut buffer)
                .map_err(|e| format!("Read {label} part: {e}"))?;
            if n == 0 {
                break;
            }
            if n as u64 > file.bytes.saturating_sub(total) {
                return Err(format!("{label} concatenation exceeds declared size"));
            }
            target
                .write_all(&buffer[..n])
                .map_err(|e| format!("Write {label} model: {e}"))?;
            hash.update(&buffer[..n]);
            total += n as u64;
        }
    }
    target
        .flush()
        .map_err(|e| format!("Flush {label} model: {e}"))?;
    if total != file.bytes || !format!("{:x}", hash.finalize()).eq_ignore_ascii_case(&file.sha256) {
        return Err(format!(
            "Concatenated {label} file size or SHA-256 does not match"
        ));
    }
    Ok(())
}

#[cfg(test)]
fn remove(root: &Path) -> Result<(), String> {
    remove_for_kind(AddonKind::Vlm, root)
}

#[cfg(test)]
fn concatenate(
    sources: &[PathBuf],
    output: &Path,
    file: &ModelFile,
    cancel: &AtomicBool,
) -> Result<(), String> {
    concatenate_for_kind(AddonKind::Vlm, sources, output, file, cancel)
}

#[cfg(test)]
fn install(
    root: &Path,
    base: &str,
    cancel: &AtomicBool,
    tx: &mpsc::SyncSender<WorkerMessage>,
) -> Result<String, String> {
    install_for_kind(AddonKind::Vlm, root, base, cancel, tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::vlm_addon_download::tests::{digest, ok, server};
    use std::time::{Duration, Instant};

    fn fixture() -> (ModelIndex, Vec<Vec<u8>>) {
        let bodies = vec![
            b"abcdef".to_vec(),
            b"vision".to_vec(),
            b"license".to_vec(),
            b"notice".to_vec(),
        ];
        let files = [
            ("model.gguf", FileKind::Model),
            ("mmproj.gguf", FileKind::Mmproj),
            ("LICENSE", FileKind::License),
            ("NOTICE", FileKind::Notice),
        ]
        .into_iter()
        .zip(&bodies)
        .map(|((name, kind), body)| ModelFile {
            name: name.into(),
            kind,
            bytes: body.len() as u64,
            sha256: digest(body),
            license: "Apache-2.0".into(),
            parts: Vec::new(),
        })
        .collect();
        (
            ModelIndex {
                schema: 1,
                model_id: format!("qwen3.5-4b-miv-q4km-{}", &digest(&bodies[0])[..12]),
                files,
            },
            bodies,
        )
    }

    fn write_pack(root: &Path) -> ModelIndex {
        let (index, bodies) = fixture();
        let pack = root.join("packs").join(&index.model_id);
        fs::create_dir_all(&pack).unwrap();
        for (file, body) in index.files.iter().zip(bodies) {
            fs::write(pack.join(&file.name), body).unwrap();
        }
        fs::write(pack.join(INDEX_NAME), serde_json::to_vec(&index).unwrap()).unwrap();
        fs::write(pack.join("INSTALL_OK"), b"1").unwrap();
        fs::write(
            root.join("active.json"),
            serde_json::to_vec(&ActivePointer {
                schema: 1,
                model_id: index.model_id.clone(),
            })
            .unwrap(),
        )
        .unwrap();
        index
    }

    fn wait(owner: &mut VlmAddon) -> AddonEvent {
        let until = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(event) = owner.poll() {
                return event;
            }
            assert!(Instant::now() < until, "VLM worker did not complete");
            std::thread::sleep(Duration::from_millis(1));
        }
    }

    /// The index published as the `vlm-model-v1` release asset must pass the same
    /// validation the installer runs.
    #[test]
    fn published_v1_index_validates() {
        let raw = include_str!("../scripts/vlm-eval/vlm-model-index.v1.json");
        let index: ModelIndex = serde_json::from_str(raw).unwrap();
        index.validate().unwrap();
        assert_eq!(index.model_id, "qwen3.5-4b-miv-q4km-777d8abc7e8f");
        assert_eq!(index.file(FileKind::Model).parts.len(), 2);
    }

    #[test]
    fn index_rejects_unsafe_names_duplicate_roles_and_invalid_hashes() {
        let (index, _) = fixture();
        index.validate().unwrap();
        for bad in [
            "../model.gguf",
            "C:model.gguf",
            "CON.gguf",
            "model.gguf.",
            "model?x.gguf",
            ".hidden",
            "foo..gguf",
        ] {
            let mut changed = index.clone();
            changed.files[0].name = bad.into();
            assert!(changed.validate().is_err(), "{bad}");
        }
        let mut changed = index.clone();
        changed.files[1].name = "MODEL.GGUF".into();
        assert!(changed.validate().is_err());
        let mut changed = index.clone();
        changed.files[1].kind = FileKind::Model;
        assert!(changed.validate().is_err());
        let mut changed = index.clone();
        changed.files[0].sha256 = "x".repeat(64);
        assert!(changed.validate().is_err());
        let mut changed = index;
        changed.schema = 0;
        assert!(changed.validate().is_err());
    }

    #[test]
    fn index_requires_sizes_licenses_and_content_identified_model() {
        let (index, _) = fixture();
        for change in 0..4 {
            let mut changed = index.clone();
            match change {
                0 => changed.files[0].bytes = 0,
                1 => changed.files[0].bytes = MAX_MODEL_BYTES + 1,
                2 => changed.files[0].license = "unknown".into(),
                _ => changed.model_id = "qwen3.5-4b-miv-q4km-000000000000".into(),
            }
            assert!(changed.validate().is_err());
        }
        let mut changed = index;
        changed.files[0].parts = vec![ModelPart {
            name: "part01".into(),
            bytes: 5,
            sha256: digest(b"abcde"),
        }];
        assert!(changed.validate().is_err());
        changed.files[0].parts[0].bytes = 6;
        changed.files[0].parts[0].name = "mmproj.gguf".into();
        assert!(changed.validate().is_err());
    }

    #[test]
    fn missing_corrupt_and_valid_states_are_distinct() {
        let dir = tempfile::tempdir().unwrap();
        let cancel = AtomicBool::new(false);
        assert_eq!(scan(dir.path(), &cancel), AddonStatus::Missing);
        fs::write(dir.path().join("active.json"), b"broken").unwrap();
        assert!(matches!(scan(dir.path(), &cancel), AddonStatus::Corrupt(_)));
        let index = write_pack(dir.path());
        assert!(matches!(
            scan(dir.path(), &cancel),
            AddonStatus::Valid { .. }
        ));
        let pack = dir.path().join("packs").join(&index.model_id);
        fs::write(pack.join("unlisted.gguf"), b"untrusted").unwrap();
        assert!(matches!(
            scan(dir.path(), &cancel),
            AddonStatus::Valid { .. }
        ));
        fs::write(pack.join("model.gguf"), b"xxxxxx").unwrap();
        assert!(matches!(scan(dir.path(), &cancel), AddonStatus::Corrupt(_)));
        fs::remove_file(pack.join("INSTALL_OK")).unwrap();
        assert_eq!(scan(dir.path(), &cancel), AddonStatus::Missing);
    }

    #[test]
    fn unsafe_active_pointer_cannot_escape_pack_directory() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(
            dir.path().join("active.json"),
            br#"{"schema":1,"model_id":"../outside"}"#,
        )
        .unwrap();
        assert!(matches!(
            scan(dir.path(), &AtomicBool::new(false)),
            AddonStatus::Corrupt(_)
        ));
    }

    #[test]
    fn concatenation_checks_part_order_total_size_and_overall_hash() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a");
        let b = dir.path().join("b");
        fs::write(&a, b"abc").unwrap();
        fs::write(&b, b"def").unwrap();
        let (index, _) = fixture();
        let output = dir.path().join("model");
        let cancel = AtomicBool::new(false);
        concatenate(&[a.clone(), b.clone()], &output, &index.files[0], &cancel).unwrap();
        assert_eq!(fs::read(&output).unwrap(), b"abcdef");
        assert!(concatenate(&[b.clone(), a.clone()], &output, &index.files[0], &cancel).is_err());
        fs::write(&b, b"defextra").unwrap();
        assert!(concatenate(&[a, b], &output, &index.files[0], &cancel).is_err());
        assert!(fs::metadata(output).unwrap().len() <= 6);
    }

    #[test]
    fn split_install_downloads_and_publishes_only_verified_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("vlm");
        let (mut index, bodies) = fixture();
        index.files[0].parts = vec![
            ModelPart {
                name: "model.gguf.part01".into(),
                bytes: 3,
                sha256: digest(b"abc"),
            },
            ModelPart {
                name: "model.gguf.part02".into(),
                bytes: 3,
                sha256: digest(b"def"),
            },
        ];
        let mut responses = vec![
            ok(&serde_json::to_vec(&index).unwrap()),
            ok(b"abc"),
            ok(b"def"),
        ];
        responses.extend(bodies[1..].iter().map(|body| ok(body)));
        let (base, requests, join) = server(responses);
        let (tx, _rx) = mpsc::sync_channel(8);
        assert_eq!(
            install(&root, &base, &AtomicBool::new(false), &tx).unwrap(),
            index.model_id
        );
        join.join().unwrap();
        let paths: Vec<_> = requests
            .try_iter()
            .map(|r| r.lines().next().unwrap().to_string())
            .collect();
        assert_eq!(paths.len(), 6);
        assert_eq!(paths[0], "GET /vlm-model-index.json HTTP/1.1");
        assert_eq!(paths[1], "GET /model.gguf.part01 HTTP/1.1");
        assert!(matches!(
            scan(&root, &AtomicBool::new(false)),
            AddonStatus::Valid { .. }
        ));
        assert!(
            !root
                .join("downloads")
                .join(&index.model_id)
                .join("model.gguf.part01.partial")
                .exists()
        );
        assert!(
            root.join("packs")
                .join(&index.model_id)
                .join("INSTALL_OK")
                .exists()
        );
    }

    #[test]
    fn corrupt_part_never_publishes_an_active_model() {
        let dir = tempfile::tempdir().unwrap();
        let (index, _) = fixture();
        let (base, _, join) = server(vec![
            ok(&serde_json::to_vec(&index).unwrap()),
            ok(b"xxxxxx"),
        ]);
        let (tx, _rx) = mpsc::sync_channel(8);
        assert!(install(dir.path(), &base, &AtomicBool::new(false), &tx).is_err());
        join.join().unwrap();
        assert!(!dir.path().join("active.json").exists());
        assert_eq!(
            scan(dir.path(), &AtomicBool::new(false)),
            AddonStatus::Missing
        );
    }

    #[test]
    fn cancellation_after_model_staging_resumes_without_redownloading_model() {
        let dir = tempfile::tempdir().unwrap();
        let (index, bodies) = fixture();
        let cancel = Arc::new(AtomicBool::new(false));
        let server_cancel = cancel.clone();
        let (base, _, join) = crate::vlm_addon_download::tests::server_with_hook(
            vec![
                ok(&serde_json::to_vec(&index).unwrap()),
                ok(&bodies[0]),
                ok(&bodies[1]),
            ],
            move |ordinal| {
                if ordinal == 2 {
                    server_cancel.store(true, Ordering::Relaxed);
                }
            },
        );
        let (tx, _rx) = mpsc::sync_channel(8);
        assert_eq!(
            install(dir.path(), &base, &cancel, &tx).unwrap_err(),
            CANCELLED
        );
        join.join().unwrap();
        let staged = dir
            .path()
            .join("downloads")
            .join(&index.model_id)
            .join("staging")
            .join("model.gguf");
        assert_eq!(fs::read(staged).unwrap(), bodies[0]);
        let mut responses = vec![ok(&serde_json::to_vec(&index).unwrap())];
        responses.extend(bodies[1..].iter().map(|body| ok(body)));
        let (base, requests, join) = server(responses);
        install(dir.path(), &base, &AtomicBool::new(false), &tx).unwrap();
        join.join().unwrap();
        let requests: Vec<_> = requests.try_iter().collect();
        assert_eq!(requests.len(), 4);
        assert!(
            !requests
                .iter()
                .any(|request| request.starts_with("GET /model.gguf "))
        );
        assert!(matches!(
            scan(dir.path(), &AtomicBool::new(false)),
            AddonStatus::Valid { .. }
        ));
    }

    #[test]
    fn resumed_complete_staging_reclaims_its_declared_source_parts() {
        let dir = tempfile::tempdir().unwrap();
        let (mut index, bodies) = fixture();
        index.files[0].parts = vec![
            ModelPart {
                name: "model.part01".into(),
                bytes: 3,
                sha256: digest(b"abc"),
            },
            ModelPart {
                name: "model.part02".into(),
                bytes: 3,
                sha256: digest(b"def"),
            },
        ];
        let downloads = dir.path().join("downloads").join(&index.model_id);
        let staging = downloads.join("staging");
        fs::create_dir_all(&staging).unwrap();
        fs::write(staging.join("model.gguf"), &bodies[0]).unwrap();
        fs::write(downloads.join("model.part01.partial"), b"abc").unwrap();
        fs::write(downloads.join("model.part02.partial"), b"def").unwrap();
        let mut responses = vec![ok(&serde_json::to_vec(&index).unwrap())];
        responses.extend(bodies[1..].iter().map(|body| ok(body)));
        let (base, requests, join) = server(responses);
        let (tx, _rx) = mpsc::sync_channel(8);
        install(dir.path(), &base, &AtomicBool::new(false), &tx).unwrap();
        join.join().unwrap();
        assert_eq!(requests.try_iter().count(), 4);
        assert!(!downloads.join("model.part01.partial").exists());
        assert!(!downloads.join("model.part02.partial").exists());
        assert!(matches!(
            scan(dir.path(), &AtomicBool::new(false)),
            AddonStatus::Valid { .. }
        ));
    }

    #[test]
    fn another_model_staging_is_not_reused_for_current_install() {
        let dir = tempfile::tempdir().unwrap();
        let (index, bodies) = fixture();
        let mut other_index = index.clone();
        let mut other_bodies = bodies.clone();
        other_bodies[0] = b"older!".to_vec();
        other_index.files[0].sha256 = digest(&other_bodies[0]);
        other_index.model_id =
            format!("qwen3.5-4b-miv-q4km-{}", &other_index.files[0].sha256[..12]);
        other_index.validate().unwrap();
        assert_ne!(other_index.model_id, index.model_id);
        let other_staging = dir
            .path()
            .join("downloads")
            .join(&other_index.model_id)
            .join("staging");
        fs::create_dir_all(&other_staging).unwrap();
        for (file, body) in other_index.files.iter().zip(&other_bodies) {
            fs::write(other_staging.join(&file.name), body).unwrap();
        }
        // Three outputs have identical hashes across the two models. Ownership
        // still requires fetching their current model's assets, not borrowing
        // another model_id's completed outputs.
        let mut responses = vec![ok(&serde_json::to_vec(&index).unwrap())];
        responses.extend(bodies.iter().map(|body| ok(body)));
        let (base, requests, join) = server(responses);
        let (tx, _rx) = mpsc::sync_channel(8);
        assert_eq!(
            install(dir.path(), &base, &AtomicBool::new(false), &tx).unwrap(),
            index.model_id
        );
        join.join().unwrap();
        let requested: Vec<_> = requests
            .try_iter()
            .map(|request| request.lines().next().unwrap().to_string())
            .collect();
        assert_eq!(
            requested,
            [
                "GET /vlm-model-index.json HTTP/1.1",
                "GET /model.gguf HTTP/1.1",
                "GET /mmproj.gguf HTTP/1.1",
                "GET /LICENSE HTTP/1.1",
                "GET /NOTICE HTTP/1.1",
            ]
        );
        for (file, body) in other_index.files.iter().zip(&other_bodies) {
            assert_eq!(fs::read(other_staging.join(&file.name)).unwrap(), *body);
        }
        match scan(dir.path(), &AtomicBool::new(false)) {
            AddonStatus::Valid { model_id, .. } => assert_eq!(model_id, index.model_id),
            status => panic!("current model was not installed: {status:?}"),
        }
    }

    #[test]
    fn startup_scan_is_lazy_cached_and_exclusive() {
        let dir = tempfile::tempdir().unwrap();
        write_pack(dir.path());
        let mut owner = VlmAddon::new(dir.path().into());
        assert!(!owner.is_scanning());
        owner.request_scan();
        assert!(owner.is_scanning());
        assert!(owner.start_remove().is_err());
        assert!(owner.start_install().is_err());
        assert!(matches!(wait(&mut owner), AddonEvent::StatusChanged));
        assert!(owner.status().is_installed());
        owner.request_scan();
        assert!(!owner.is_scanning());
    }

    #[test]
    fn cancellation_does_not_release_exclusivity_before_completion_is_consumed() {
        let dir = tempfile::tempdir().unwrap();
        let mut owner = VlmAddon::new(dir.path().into());
        let (release_tx, release_rx) = mpsc::channel();
        let running = owner
            .spawn("vlm-test-cancel", move |_, cancel, _| {
                release_rx.recv().unwrap();
                assert!(cancel.load(Ordering::Relaxed));
                (AddonStatus::Missing, AddonEvent::Cancelled)
            })
            .unwrap();
        owner.state = WorkerState::Installing(running);
        owner.cancel();
        assert_eq!(owner.operation(), AddonOperation::Cancelling);
        assert!(owner.start_install().is_err());
        assert!(owner.start_remove().is_err());
        release_tx.send(()).unwrap();
        assert!(owner.start_install().is_err());
        assert!(matches!(wait(&mut owner), AddonEvent::Cancelled));
        assert_eq!(owner.operation(), AddonOperation::None);
    }

    #[test]
    fn removal_finishes_before_reinstall_is_accepted_and_preserves_captions() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().join("addons").join("vlm");
        write_pack(&root);
        let captions = dir.path().join("ai_caption.db");
        fs::write(&captions, b"user captions").unwrap();
        let mut owner = VlmAddon::new(root.clone());
        owner.start_remove().unwrap();
        assert_eq!(owner.operation(), AddonOperation::Removing);
        assert!(owner.start_install().is_err());
        assert!(owner.start_remove().is_err());
        assert!(matches!(wait(&mut owner), AddonEvent::Removed));
        assert_eq!(owner.status(), &AddonStatus::Missing);
        assert!(!root.exists());
        assert_eq!(fs::read(captions).unwrap(), b"user captions");
        // A second no-op removal proves mutation admission reopened without HTTP.
        owner.start_remove().unwrap();
        assert!(matches!(wait(&mut owner), AddonEvent::Removed));
    }
}

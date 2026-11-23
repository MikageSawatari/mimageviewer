//! Windows Shell-backed file operations that need mIV-owned UI.

use std::path::PathBuf;
use std::sync::mpsc;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellTransferOperation {
    Copy,
    Move,
}

#[derive(Debug, Clone)]
pub struct ShellTransferRequest {
    pub operation: ShellTransferOperation,
    /// Only an unmodified, lexically predicted Move may be downgraded by the worker.
    pub default_move_needs_volume_check: bool,
    pub sources: Vec<PathBuf>,
    pub destination: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ShellOperationClassification {
    Completed,
    Interrupted,
    Failed,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ShellItemCounts {
    pub processed: usize,
    pub failed: usize,
    pub cancelled: usize,
    pub skipped: usize,
    /// Diagnostic callback count, not a terminal item result.
    pub pending: usize,
    /// Merge-start callback count, not completed work on the parent.
    pub merged: usize,
    pub first_failed_hresult: Option<i32>,
}

// Store signed HRESULT bits so worker results remain usable on non-Windows builds.
const COPYENGINE_USER_CANCELLED: i32 = 0x80270000u32 as i32;
const WIN32_CANCELLED: i32 = 0x800704C7u32 as i32;
const COPYENGINE_PENDING: i32 = 0x0027000B;
const COPYENGINE_NOT_HANDLED: i32 = 0x00270003;
const COPYENGINE_MERGE: i32 = 0x00270006;

fn is_cancel_hresult(hr: i32) -> bool {
    matches!(hr, COPYENGINE_USER_CANCELLED | WIN32_CANCELLED)
}

fn classify_shell_operation(
    perform_hresult: i32,
    abort_query: Result<bool, i32>,
) -> ShellOperationClassification {
    if abort_query == Ok(true) || is_cancel_hresult(perform_hresult) {
        ShellOperationClassification::Interrupted
    } else if perform_hresult >= 0 && abort_query == Ok(false) {
        ShellOperationClassification::Completed
    } else {
        ShellOperationClassification::Failed
    }
}

#[derive(Debug, Clone, Copy)]
pub struct ShellTransferOutcome {
    pub perform_hresult: i32,
    pub abort_query: Result<bool, i32>,
    pub counts: ShellItemCounts,
}

impl ShellTransferOutcome {
    pub fn classification(&self) -> ShellOperationClassification {
        classify_shell_operation(self.perform_hresult, self.abort_query)
    }

    fn short_reason(&self) -> String {
        let abort = match self.abort_query {
            Ok(true) => "中断あり".to_owned(),
            Ok(false) => "中断なし".to_owned(),
            Err(hr) => format!("照会失敗 0x{hr:08X}"),
        };
        format!("Shell: 0x{:08X}、中断状態: {abort}", self.perform_hresult)
    }

    pub fn notice(&self, rename: bool) -> Option<String> {
        let classification = self.classification();
        let mut message = match classification {
            ShellOperationClassification::Completed => return None,
            ShellOperationClassification::Interrupted if self.counts.processed > 0 => {
                "操作が中断されました。一部の項目は処理済みです。フォルダの内容を確認してください"
                    .to_owned()
            }
            ShellOperationClassification::Interrupted if rename => {
                "名前の変更は中断されました".to_owned()
            }
            ShellOperationClassification::Interrupted if self.counts.merged > 0 => {
                "操作が中断されました。フォルダの内容を確認してください".to_owned()
            }
            ShellOperationClassification::Interrupted => {
                "操作が中断されました。移動・コピーした項目はありません".to_owned()
            }
            ShellOperationClassification::Failed => {
                let mut message =
                    "操作を完了できませんでした。フォルダの内容を確認してください".to_owned();
                if self.counts.processed > 0 {
                    message.push_str("。一部の項目は処理済みです");
                }
                message
            }
        };
        if classification == ShellOperationClassification::Interrupted
            && self.counts.failed > 0
            && let Some(hr) = self.counts.first_failed_hresult
        {
            message.push_str(&format!(
                "\n失敗した項目: {} 件 (Shell: 0x{hr:08X})",
                self.counts.failed,
            ));
        }
        if classification == ShellOperationClassification::Failed
            || (self.perform_hresult < 0 && !is_cancel_hresult(self.perform_hresult))
            || self.abort_query.is_err()
        {
            message.push('\n');
            message.push_str(&self.short_reason());
            if classification == ShellOperationClassification::Failed
                && let Some(hr) = self.counts.first_failed_hresult
                && hr != self.perform_hresult
            {
                message.push_str(&format!("、項目: 0x{hr:08X}"));
            }
        }
        Some(message)
    }

    #[cfg(test)]
    pub(crate) fn test_outcome(
        perform_hresult: i32,
        abort_query: Result<bool, i32>,
        processed: usize,
    ) -> Self {
        Self {
            perform_hresult,
            abort_query,
            counts: ShellItemCounts {
                processed,
                ..Default::default()
            },
        }
    }
}

pub type ShellTransferResult = Result<ShellTransferOutcome, String>;

const SHELL_ITEM_DETAIL_LIMIT: usize = 50;

#[derive(Debug)]
struct ShellItemResult {
    kind: &'static str,
    source: Option<String>,
    destination: Option<String>,
    new_name: Option<String>,
    hresult: i32,
    created: Option<String>,
    #[cfg(test)]
    has_created: bool,
}

#[derive(Default)]
struct ShellItemResults {
    total_callbacks: usize,
    counts: ShellItemCounts,
    details: Vec<ShellItemResult>,
}

impl ShellItemResults {
    fn count(&mut self, hresult: i32, has_created: bool) {
        // Windows 11/NTFS probe (2026-11-22): a completed same-volume move returns
        // COPYENGINE_S_DONT_PROCESS_CHILDREN (0x00270008) with a created item.
        // Count the callback's item presence, independently of parsing-name lookup.
        // https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperationprogresssink-postmoveitem
        self.total_callbacks += 1;
        // Pending is provisional; a later callback supplies the item's result.
        if hresult == COPYENGINE_PENDING {
            self.counts.pending += 1;
        } else if matches!(hresult, COPYENGINE_MERGE | COPYENGINE_NOT_HANDLED) {
            // Wine shlfileop.c notifications6 reports an existing merge parent
            // before its children; the returned item does not establish work done.
            // https://github.com/wine-mirror/wine/blob/master/dlls/shell32/tests/shlfileop.c
            // Dev machine 2026-11-23: a same-volume merge Move reported 0x00270003 for
            // the parent. A merge Copy reported S_OK with the existing parent, which is
            // still counted as processed (errs toward "check the folder"); see
            // docs/folder-tree-dnd-plan.md.
            self.counts.merged += 1;
        } else if is_cancel_hresult(hresult) {
            self.counts.cancelled += 1;
        } else if hresult < 0 {
            self.counts.failed += 1;
            self.counts.first_failed_hresult.get_or_insert(hresult);
        } else if has_created {
            self.counts.processed += 1;
        } else {
            self.counts.skipped += 1;
        }
    }

    fn record(&mut self, item: ShellItemResult) {
        if self.details.len() < SHELL_ITEM_DETAIL_LIMIT {
            self.details.push(item);
        }
    }

    fn omitted(&self) -> usize {
        self.total_callbacks - self.details.len()
    }
}

#[cfg(windows)]
type SharedShellItems = std::sync::Arc<std::sync::Mutex<ShellItemResults>>;

#[cfg(windows)]
struct ShellOperationLog {
    prefix: &'static str,
    id: u64,
}

#[cfg(windows)]
impl ShellOperationLog {
    fn start(
        prefix: &'static str,
        requested: &str,
        resolved: &str,
        sources: usize,
        destination: &std::path::Path,
    ) -> Self {
        use std::sync::atomic::{AtomicU64, Ordering};
        static SEQUENCE: AtomicU64 = AtomicU64::new(1);
        let id = SEQUENCE.fetch_add(1, Ordering::Relaxed);
        crate::logger::log(format!(
            "{prefix}: start id={id} requested={requested} resolved={resolved} sources={sources} destination={destination:?}"
        ));
        Self { prefix, id }
    }

    fn finish(&self, outcome: ShellTransferOutcome, items: &SharedShellItems) {
        let Self { prefix, id } = self;
        if let Ok(items) = items.lock() {
            // Buffered item lines have the end-time log timestamp, not callback times.
            for item in &items.details {
                crate::logger::log(format!(
                    "{prefix}: item id={id} kind={} source={:?} destination={:?} new_name={:?} hr=0x{:08X} created={:?}",
                    item.kind,
                    item.source,
                    item.destination,
                    item.new_name,
                    item.hresult,
                    item.created,
                ));
            }
            let omitted = items.omitted();
            if omitted > 0 {
                crate::logger::log(format!("{prefix}: item id={id} ... and {omitted} more"));
            }
        } else {
            crate::logger::log(format!("{prefix}: item id={id} results lock failed"));
        }
        let abort = match outcome.abort_query {
            Ok(value) => format!("Ok({value})"),
            Err(hr) => format!("Err(0x{hr:08X})"),
        };
        let first_failed = match outcome.counts.first_failed_hresult {
            Some(hr) => format!("0x{hr:08X}"),
            None => "None".to_owned(),
        };
        crate::logger::log(format!(
            "{prefix}: end id={id} classification={:?} perform=0x{:08X} abort={abort} processed={} failed={} cancelled={} skipped={} pending={} merged={} first_failed_hresult={first_failed}",
            outcome.classification(),
            outcome.perform_hresult,
            outcome.counts.processed,
            outcome.counts.failed,
            outcome.counts.cancelled,
            outcome.counts.skipped,
            outcome.counts.pending,
            outcome.counts.merged,
        ));
    }
}

#[cfg(windows)]
fn perform_and_query(
    op: &windows::Win32::UI::Shell::IFileOperation,
    items: &SharedShellItems,
    prefix: &str,
) -> ShellTransferOutcome {
    use windows::core::Interface;
    // The generated Result<()> wrapper discards nonzero success HRESULTs.
    let perform_hresult = unsafe { (op.vtable().PerformOperations)(op.as_raw()) }.0;
    // Always query, even after failure; never substitute false for an unknown state.
    // https://learn.microsoft.com/en-us/windows/win32/api/shobjidl_core/nf-shobjidl_core-ifileoperation-getanyoperationsaborted
    let abort_query = unsafe { op.GetAnyOperationsAborted() }
        .map(|value| value.as_bool())
        .map_err(|error| error.code().0);
    let counts = match items.lock() {
        Ok(items) => items.counts,
        Err(_) => {
            crate::logger::log(format!(
                "{prefix}: results lock failed after Shell operation"
            ));
            ShellItemCounts::default()
        }
    };
    ShellTransferOutcome {
        perform_hresult,
        abort_query,
        counts,
    }
}

#[cfg(windows)]
fn shell_item_parsing_name(item: Option<&windows::Win32::UI::Shell::IShellItem>) -> Option<String> {
    use windows::Win32::System::Com::CoTaskMemFree;
    use windows::Win32::UI::Shell::SIGDN_DESKTOPABSOLUTEPARSING;
    let item = item?;
    match unsafe { item.GetDisplayName(SIGDN_DESKTOPABSOLUTEPARSING) } {
        Ok(name) => {
            let result = unsafe { name.to_string() }.ok();
            unsafe { CoTaskMemFree(Some(name.0.cast())) };
            result
        }
        Err(error) => Some(format!("<unavailable: 0x{:08X}>", error.code().0)),
    }
}

#[cfg(windows)]
#[windows::core::implement(windows::Win32::UI::Shell::IFileOperationProgressSink)]
struct ShellResultSink {
    items: SharedShellItems,
    rename_destination: Option<PathBuf>,
}

#[cfg(windows)]
impl ShellResultSink_Impl {
    fn record(
        &self,
        kind: &'static str,
        source: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
        destination: Option<&windows::Win32::UI::Shell::IShellItem>,
        new_name: &windows::core::PCWSTR,
        hr: windows::core::HRESULT,
        created: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
    ) {
        let has_created = created.as_ref().is_some();
        {
            let Ok(mut items) = self.items.lock() else {
                crate::logger::log(format!(
                    "shell_{}: callback results lock failed",
                    if kind == "Rename" {
                        "rename"
                    } else {
                        "transfer"
                    }
                ));
                return;
            };
            items.count(hr.0, has_created);
            if items.details.len() >= SHELL_ITEM_DETAIL_LIMIT {
                return;
            }
        }
        // Resolve detail only for retained rows, without holding a lock across COM.
        let item = ShellItemResult {
            kind,
            source: shell_item_parsing_name(source.as_ref()),
            destination: shell_item_parsing_name(destination).or_else(|| {
                self.rename_destination
                    .as_ref()
                    .map(|path| path.to_string_lossy().into_owned())
            }),
            new_name: if new_name.is_null() {
                None
            } else {
                unsafe { new_name.to_string() }.ok()
            },
            hresult: hr.0,
            created: shell_item_parsing_name(created.as_ref()),
            #[cfg(test)]
            has_created,
        };
        if let Ok(mut items) = self.items.lock() {
            items.record(item);
        } else {
            crate::logger::log(format!(
                "shell_{}: callback results lock failed",
                if kind == "Rename" {
                    "rename"
                } else {
                    "transfer"
                }
            ));
        }
    }
}

#[cfg(windows)]
#[allow(non_snake_case)]
impl windows::Win32::UI::Shell::IFileOperationProgressSink_Impl for ShellResultSink_Impl {
    fn StartOperations(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn FinishOperations(&self, _: windows::core::HRESULT) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreRenameItem(
        &self,
        _: u32,
        _: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostRenameItem(
        &self,
        _: u32,
        source: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
        name: &windows::core::PCWSTR,
        hr: windows::core::HRESULT,
        created: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
    ) -> windows::core::Result<()> {
        self.record("Rename", source, None, name, hr, created);
        Ok(())
    }
    fn PreMoveItem(
        &self,
        _: u32,
        _: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostMoveItem(
        &self,
        _: u32,
        source: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
        destination: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
        name: &windows::core::PCWSTR,
        hr: windows::core::HRESULT,
        created: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
    ) -> windows::core::Result<()> {
        self.record("Move", source, destination.as_ref(), name, hr, created);
        Ok(())
    }
    fn PreCopyItem(
        &self,
        _: u32,
        _: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostCopyItem(
        &self,
        _: u32,
        source: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
        destination: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
        name: &windows::core::PCWSTR,
        hr: windows::core::HRESULT,
        created: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
    ) -> windows::core::Result<()> {
        self.record("Copy", source, destination.as_ref(), name, hr, created);
        // Returning hr here would cancel subsequent items.
        Ok(())
    }
    fn PreDeleteItem(
        &self,
        _: u32,
        _: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostDeleteItem(
        &self,
        _: u32,
        _: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _: windows::core::HRESULT,
        _: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PreNewItem(
        &self,
        _: u32,
        _: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _: &windows::core::PCWSTR,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn PostNewItem(
        &self,
        _: u32,
        _: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
        _: &windows::core::PCWSTR,
        _: &windows::core::PCWSTR,
        _: u32,
        _: windows::core::HRESULT,
        _: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
    ) -> windows::core::Result<()> {
        Ok(())
    }
    fn UpdateProgress(&self, _: u32, _: u32) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResetTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn PauseTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
    fn ResumeTimer(&self) -> windows::core::Result<()> {
        Ok(())
    }
}

/// The worker owns validation and all Shell calls; the UI only polls the terminal result.
pub fn transfer_items_async(
    hwnd: Option<isize>,
    request: ShellTransferRequest,
    wake_ctx: egui::Context,
) -> mpsc::Receiver<ShellTransferResult> {
    let (tx, rx) = mpsc::channel();
    let tx_on_spawn_error = tx.clone();
    let wake_on_spawn_error = wake_ctx.clone();
    let spawn_result = std::thread::Builder::new()
        .name("shell-transfer-worker".into())
        .spawn(move || {
            let result = transfer_items_blocking(hwnd, request);
            let _ = tx.send(result);
            wake_ctx.request_repaint();
        });
    if let Err(e) = spawn_result {
        let _ = tx_on_spawn_error.send(Err(format!("移動／コピー worker を開始できません: {e}")));
        wake_on_spawn_error.request_repaint();
    }
    rx
}

/// Validate the complete request before queuing any Shell operation.
/// This entry point blocks and must be called on a worker thread.
pub(crate) fn transfer_items_blocking(
    hwnd: Option<isize>,
    request: ShellTransferRequest,
) -> ShellTransferResult {
    let requested = request.operation;
    validate_transfer_request(resolve_default_move(request, volume_identity))
        .and_then(|request| run_transfer_items(hwnd.unwrap_or_default(), request, requested))
}

fn resolve_default_move(
    mut request: ShellTransferRequest,
    mut volume: impl FnMut(&std::path::Path) -> Option<String>,
) -> ShellTransferRequest {
    if request.operation == ShellTransferOperation::Move && request.default_move_needs_volume_check
    {
        let destination_volume = volume(&request.destination);
        let same_volume = destination_volume.as_ref().is_some_and(|destination| {
            request.sources.iter().all(|source| {
                volume(source).is_some_and(|source| source.eq_ignore_ascii_case(destination))
            })
        });
        if !same_volume {
            request.operation = ShellTransferOperation::Copy;
        }
    }
    request.default_move_needs_volume_check = false;
    request
}

#[cfg(windows)]
fn volume_identity(path: &std::path::Path) -> Option<String> {
    use windows::Win32::Storage::FileSystem::{
        GetVolumeNameForVolumeMountPointW, GetVolumePathNameW,
    };
    use windows::core::PCWSTR;

    let spelling = crate::path_key::physical_path_spelling(path)?;
    if let Some(unc) = spelling.strip_prefix("//") {
        let mut parts = unc.split('/');
        return Some(format!("//{}/{}", parts.next()?, parts.next()?));
    }
    let path = wide_null_path(path);
    let mut mount = vec![0u16; 32768];
    unsafe { GetVolumePathNameW(PCWSTR(path.as_ptr()), &mut mount) }.ok()?;
    let end = mount.iter().position(|&ch| ch == 0)?;
    let mount_name = String::from_utf16_lossy(&mount[..end]);
    // Network volumes are identified by their share, rather than a local GUID.
    if mount_name.starts_with(r"\\") && !mount_name.starts_with(r"\\?\") {
        return Some(mount_name.trim_end_matches('\\').to_lowercase());
    }
    let mut guid = vec![0u16; 32768];
    if unsafe { GetVolumeNameForVolumeMountPointW(PCWSTR(mount.as_ptr()), &mut guid) }.is_ok() {
        let end = guid.iter().position(|&ch| ch == 0)?;
        Some(String::from_utf16_lossy(&guid[..end]).to_lowercase())
    } else {
        Some(mount_name.to_lowercase())
    }
}

#[cfg(not(windows))]
fn volume_identity(_path: &std::path::Path) -> Option<String> {
    None
}

fn path_has_nul(path: &std::path::Path) -> bool {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().any(|ch| ch == 0)
    }
    #[cfg(not(windows))]
    {
        path.as_os_str().as_encoded_bytes().contains(&0)
    }
}

// Call only with canonical paths. Component comparison avoids confusing siblings
// such as "photos" and "photos-old", and keeps drive/UNC prefixes in the comparison.
fn canonical_path_components(path: &std::path::Path) -> Vec<String> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().to_lowercase())
        .collect()
}

#[derive(Debug)]
struct ValidatedTransfer {
    request: ShellTransferRequest,
    folders: Vec<PathBuf>,
}

impl ValidatedTransfer {
    fn mutation_folders(&self) -> &[PathBuf] {
        if self.request.operation == ShellTransferOperation::Move {
            &self.folders
        } else {
            &[]
        }
    }
}

fn validate_transfer_request(request: ShellTransferRequest) -> Result<ValidatedTransfer, String> {
    if request.sources.is_empty() {
        return Err("移動／コピーする対象がありません".into());
    }
    if !request.destination.is_absolute() || path_has_nul(&request.destination) {
        return Err("移動／コピー先には NUL を含まない絶対パスを指定してください".into());
    }
    let destination = std::fs::canonicalize(&request.destination).map_err(|e| {
        format!(
            "移動／コピー先を確認できません: {} ({e})",
            request.destination.display()
        )
    })?;
    if !std::fs::metadata(&destination)
        .map_err(|e| {
            format!(
                "移動／コピー先を確認できません: {} ({e})",
                request.destination.display()
            )
        })?
        .is_dir()
    {
        return Err(format!(
            "移動／コピー先は実フォルダではありません: {}",
            request.destination.display()
        ));
    }
    let destination_components = canonical_path_components(&destination);
    let mut folders = Vec::new();
    for source in &request.sources {
        if !source.is_absolute() || path_has_nul(source) {
            return Err(format!(
                "対象には NUL を含まない絶対パスが必要です: {}",
                source.display()
            ));
        }
        let canonical = std::fs::canonicalize(source)
            .map_err(|e| format!("対象を確認できません: {} ({e})", source.display()))?;
        let metadata = std::fs::metadata(&canonical)
            .map_err(|e| format!("対象を確認できません: {} ({e})", source.display()))?;
        if !metadata.is_dir() && !metadata.is_file() {
            return Err(format!(
                "対象は実ファイル／実フォルダではありません: {}",
                source.display()
            ));
        }
        if request.operation == ShellTransferOperation::Move {
            // Shell moves the selected item, including a junction itself. Resolve
            // its containing folder separately from the item's link target.
            if let Some(parent) = source.parent() {
                let parent = std::fs::canonicalize(parent).map_err(|e| {
                    format!(
                        "対象の親フォルダを確認できません: {} ({e})",
                        parent.display()
                    )
                })?;
                if canonical_path_components(&parent) == destination_components {
                    return Err(format!(
                        "同じフォルダなので移動できません: {}",
                        source.display()
                    ));
                }
            }
        }
        if metadata.is_dir()
            && destination_components.starts_with(&canonical_path_components(&canonical))
        {
            return Err(format!(
                "フォルダ自身またはその配下には移動／コピーできません: {}",
                source.display()
            ));
        }
        if metadata.is_dir() {
            folders.push(source.clone());
        }
    }
    // Resolve aliases for safety checks without changing the selected Shell item:
    // a junction's user-visible name and Windows' link handling remain Shell-owned.
    Ok(ValidatedTransfer { request, folders })
}

#[cfg(windows)]
fn run_transfer_items(
    hwnd: isize,
    request: ValidatedTransfer,
    requested: ShellTransferOperation,
) -> ShellTransferResult {
    run_transfer_items_with_flags(
        hwnd,
        request,
        requested,
        transfer_flags(),
        std::sync::Arc::new(std::sync::Mutex::new(ShellItemResults::default())),
    )
}

#[cfg(windows)]
fn transfer_flags() -> windows::Win32::UI::Shell::FILEOPERATION_FLAGS {
    use windows::Win32::UI::Shell::{FOF_ALLOWUNDO, FOFX_ADDUNDORECORD};
    #[cfg(test)]
    if FENCE_TEST_QUIET.with(std::cell::Cell::get) {
        use windows::Win32::UI::Shell::{FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT};
        return FOF_ALLOWUNDO
            | FOFX_ADDUNDORECORD
            | FOF_NOCONFIRMATION
            | FOF_NOERRORUI
            | FOF_SILENT;
    }
    FOF_ALLOWUNDO | FOFX_ADDUNDORECORD
}

#[cfg(all(windows, test))]
thread_local! {
    static FENCE_TEST_QUIET: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

#[cfg(all(windows, test))]
pub(crate) fn fence_858b_transfer_for_probe(
    source: PathBuf,
    destination: PathBuf,
    bypass: bool,
) -> ShellTransferResult {
    struct RestoreQuiet(bool);
    impl Drop for RestoreQuiet {
        fn drop(&mut self) {
            FENCE_TEST_QUIET.with(|quiet| quiet.set(self.0));
        }
    }
    let _quiet = RestoreQuiet(FENCE_TEST_QUIET.with(|quiet| quiet.replace(true)));
    let run = || {
        transfer_items_blocking(
            None,
            ShellTransferRequest {
                operation: ShellTransferOperation::Move,
                default_move_needs_volume_check: false,
                sources: vec![source],
                destination,
            },
        )
    };
    if bypass {
        crate::mutation_fence::without_fence_for_test(run)
    } else {
        run()
    }
}

#[cfg(windows)]
fn run_transfer_items_with_flags(
    hwnd: isize,
    validated: ValidatedTransfer,
    requested: ShellTransferOperation,
    flags: windows::Win32::UI::Shell::FILEOPERATION_FLAGS,
    items: SharedShellItems,
) -> ShellTransferResult {
    let request = &validated.request;
    use windows::Win32::Foundation::HWND;
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::UI::Shell::{
        FileOperation, IFileOperation, IFileOperationProgressSink, IShellItem,
        SHCreateItemFromParsingName,
    };
    use windows::core::{IUnknown, PCWSTR};

    struct ComStaGuard;
    impl Drop for ComStaGuard {
        fn drop(&mut self) {
            unsafe { CoUninitialize() };
        }
    }
    unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) }
        .ok()
        .map_err(|e| format!("CoInitializeEx(STA) failed: {e}"))?;
    let _com = ComStaGuard;
    let destination_w = wide_null_path(&request.destination);
    let destination: IShellItem = unsafe {
        SHCreateItemFromParsingName(
            PCWSTR(destination_w.as_ptr()),
            None::<&windows::Win32::System::Com::IBindCtx>,
        )
    }
    .map_err(|e| format!("移動／コピー先を開けません: {e}"))?;
    let op: IFileOperation =
        unsafe { CoCreateInstance(&FileOperation, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
            .map_err(|e| format!("IFileOperation を作成できません: {e}"))?;
    if hwnd != 0 {
        unsafe { op.SetOwnerWindow(HWND(hwnd as *mut core::ffi::c_void)) }
            .map_err(|e| format!("Shell 操作の owner window を設定できません: {e}"))?;
    }
    // Keep Windows' conflict, error, cancellation and progress UI enabled.
    unsafe { op.SetOperationFlags(flags) }
        .map_err(|e| format!("Shell 操作フラグを設定できません: {e}"))?;
    // Per-item sinks follow only this request, without Advise/Unadvise lifetime state.
    let sink: IFileOperationProgressSink = ShellResultSink {
        items: items.clone(),
        rename_destination: None,
    }
    .into();
    for source in &request.sources {
        let source_w = wide_null_path(source);
        let item: IShellItem = unsafe {
            SHCreateItemFromParsingName(
                PCWSTR(source_w.as_ptr()),
                None::<&windows::Win32::System::Com::IBindCtx>,
            )
        }
        .map_err(|e| format!("対象を開けません: {} ({e})", source.display()))?;
        match request.operation {
            ShellTransferOperation::Copy => unsafe {
                op.CopyItem(&item, &destination, PCWSTR::null(), &sink)
            },
            ShellTransferOperation::Move => unsafe {
                op.MoveItem(&item, &destination, PCWSTR::null(), &sink)
            },
        }
        .map_err(|e| format!("移動／コピーを予約できません: {} ({e})", source.display()))?;
    }
    let log = ShellOperationLog::start(
        "shell_transfer",
        &format!("{requested:?}"),
        &format!("{:?}", request.operation),
        request.sources.len(),
        &request.destination,
    );
    let fence = crate::mutation_fence::begin(validated.mutation_folders());
    let outcome = perform_and_query(&op, &items, log.prefix);
    drop(fence);
    log.finish(outcome, &items);
    Ok(outcome)
}

#[cfg(not(windows))]
fn run_transfer_items(
    _hwnd: isize,
    _request: ValidatedTransfer,
    _requested: ShellTransferOperation,
) -> ShellTransferResult {
    Err("移動／コピーは Windows Shell 経由でのみ利用できます".into())
}

#[derive(Debug, Clone)]
pub struct ShellRenameOutcome {
    pub target: PathBuf,
    pub new_path: PathBuf,
    pub operation: ShellTransferOutcome,
}

pub type ShellRenameResult = Result<ShellRenameOutcome, String>;

pub fn rename_item_async(
    hwnd: Option<isize>,
    target: PathBuf,
    new_name: String,
) -> mpsc::Receiver<ShellRenameResult> {
    let (tx, rx) = mpsc::channel();
    let spawn_target = target.clone();
    let spawn_name = new_name.clone();
    let tx_on_spawn_error = tx.clone();
    let spawn_result = std::thread::Builder::new()
        .name("shell-rename-worker".into())
        .spawn(move || {
            let result = run_rename_item(hwnd.unwrap_or_default(), spawn_target, spawn_name);
            let _ = tx.send(result);
        });
    if let Err(e) = spawn_result {
        let _ = tx_on_spawn_error.send(Err(format!("名前変更 worker を開始できません: {e}")));
    }
    rx
}

#[cfg(windows)]
fn run_rename_item(hwnd: isize, target: PathBuf, new_name: String) -> ShellRenameResult {
    use windows::Win32::Foundation::{HWND, RPC_E_CHANGED_MODE};
    use windows::Win32::System::Com::{
        CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx,
        CoUninitialize,
    };
    use windows::Win32::UI::Shell::{
        FileOperation, IFileOperation, IFileOperationProgressSink, IShellItem,
        SHCreateItemFromParsingName,
    };
    use windows::core::{IUnknown, PCWSTR};

    struct ComStaGuard {
        uninitialize: bool,
    }

    impl ComStaGuard {
        fn new() -> Result<Self, String> {
            let hr = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
            if hr.is_ok() {
                Ok(Self { uninitialize: true })
            } else if hr == RPC_E_CHANGED_MODE {
                Ok(Self {
                    uninitialize: false,
                })
            } else {
                Err(format!("CoInitializeEx(STA) failed: 0x{:08x}", hr.0))
            }
        }
    }

    impl Drop for ComStaGuard {
        fn drop(&mut self) {
            if self.uninitialize {
                unsafe { CoUninitialize() };
            }
        }
    }

    let _com = ComStaGuard::new()?;
    let target_w = wide_null_path(&target);
    let new_name_w = wide_null_str(&new_name);
    let item: IShellItem = unsafe {
        SHCreateItemFromParsingName(
            PCWSTR(target_w.as_ptr()),
            None::<&windows::Win32::System::Com::IBindCtx>,
        )
    }
    .map_err(|e| format!("対象を開けません: {e}"))?;
    let op: IFileOperation =
        unsafe { CoCreateInstance(&FileOperation, None::<&IUnknown>, CLSCTX_INPROC_SERVER) }
            .map_err(|e| format!("IFileOperation を作成できません: {e}"))?;

    if hwnd != 0 {
        unsafe { op.SetOwnerWindow(HWND(hwnd as *mut core::ffi::c_void)) }
            .map_err(|e| format!("Shell 操作の owner window を設定できません: {e}"))?;
    }
    unsafe { op.SetOperationFlags(transfer_flags()) }
        .map_err(|e| format!("Shell 操作フラグを設定できません: {e}"))?;
    let items = std::sync::Arc::new(std::sync::Mutex::new(ShellItemResults::default()));
    let sink: IFileOperationProgressSink = ShellResultSink {
        items: items.clone(),
        rename_destination: target.parent().map(std::path::Path::to_path_buf),
    }
    .into();
    unsafe { op.RenameItem(&item, PCWSTR(new_name_w.as_ptr()), &sink) }
        .map_err(|e| format!("名前変更を予約できません: {e}"))?;
    let log = ShellOperationLog::start(
        "shell_rename",
        "Rename",
        "Rename",
        1,
        target.parent().unwrap_or(&target),
    );
    let folders = match std::fs::metadata(&target) {
        Ok(metadata) if metadata.is_dir() => vec![target.clone()],
        Ok(_) => Vec::new(),
        Err(error) => {
            crate::logger::log(format!(
                "mutation_fence: cannot classify {target:?}: {error}"
            ));
            Vec::new()
        }
    };
    let fence = crate::mutation_fence::begin(&folders);
    let operation = perform_and_query(&op, &items, log.prefix);
    drop(fence);
    log.finish(operation, &items);

    let new_path = target
        .parent()
        .map(|parent| parent.join(&new_name))
        .unwrap_or_else(|| PathBuf::from(&new_name));
    if operation.classification() == ShellOperationClassification::Completed {
        let new_path_exists = new_path.try_exists().map_err(|e| {
            format!(
                "名前変更後の項目を確認できません: {} ({e})",
                new_path.display()
            )
        })?;
        if !new_path_exists {
            return Err(format!(
                "名前変更後の項目を確認できません: {}",
                new_path.display()
            ));
        }
        let target_still_exists = target.try_exists().map_err(|e| {
            format!(
                "名前変更前の項目を確認できません: {} ({e})",
                target.display()
            )
        })?;
        if target_still_exists && !crate::folder_tree::path_eq(&target, &new_path) {
            return Err(format!(
                "名前変更が完了していない可能性があります: {}",
                target.display()
            ));
        }
    }
    Ok(ShellRenameOutcome {
        target,
        new_path,
        operation,
    })
}

#[cfg(not(windows))]
fn run_rename_item(_hwnd: isize, target: PathBuf, new_name: String) -> ShellRenameResult {
    let _ = (target, new_name);
    Err("名前変更は Windows Shell 経由でのみ利用できます".to_string())
}

#[cfg(windows)]
fn wide_null_path(path: &std::path::Path) -> Vec<u16> {
    use std::os::windows::ffi::OsStrExt;

    path.as_os_str()
        .encode_wide()
        .map(|ch| if ch == b'/' as u16 { b'\\' as u16 } else { ch })
        .chain(std::iter::once(0))
        .collect()
}

#[cfg(windows)]
fn wide_null_str(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(test)]
mod transfer_tests {
    use super::*;

    #[test]
    fn shell_op_858_classifies_perform_and_abort_independently() {
        use ShellOperationClassification::{Completed, Failed, Interrupted};
        let failure = 0x80070005u32 as i32;
        let query_failure = 0x80004005u32 as i32;
        for (perform, abort, expected) in [
            (0, Ok(false), Completed),
            (1, Ok(false), Completed),
            (0, Ok(true), Interrupted),
            (failure, Ok(true), Interrupted),
            (failure, Ok(false), Failed),
            (0x80270001u32 as i32, Ok(false), Failed),
            (0, Err(query_failure), Failed),
            (failure, Err(query_failure), Failed),
            (COPYENGINE_USER_CANCELLED, Ok(false), Interrupted),
            (WIN32_CANCELLED, Ok(false), Interrupted),
            (COPYENGINE_USER_CANCELLED, Ok(true), Interrupted),
            (WIN32_CANCELLED, Ok(true), Interrupted),
            (COPYENGINE_USER_CANCELLED, Err(query_failure), Interrupted),
            (WIN32_CANCELLED, Err(query_failure), Interrupted),
        ] {
            let outcome = ShellTransferOutcome::test_outcome(perform, abort, 0);
            assert_eq!(outcome.classification(), expected, "{outcome:?}");
            assert_eq!(outcome.perform_hresult, perform);
            assert_eq!(outcome.abort_query, abort);
        }
    }

    #[test]
    fn shell_op_858_notices_preserve_errors_and_processed_evidence() {
        let failure = 0x80070005u32 as i32;
        let query_failure = 0x80004005u32 as i32;
        for rename in [false, true] {
            assert_eq!(
                ShellTransferOutcome::test_outcome(0, Ok(false), 1).notice(rename),
                None
            );
            for perform in [0, COPYENGINE_USER_CANCELLED, WIN32_CANCELLED] {
                assert_eq!(
                    ShellTransferOutcome::test_outcome(perform, Ok(false), 0).notice(rename),
                    if perform == 0 {
                        None
                    } else {
                        Some(
                            if rename {
                                "名前の変更は中断されました"
                            } else {
                                "操作が中断されました。移動・コピーした項目はありません"
                            }
                            .to_owned(),
                        )
                    },
                );
            }
            assert_eq!(
                ShellTransferOutcome::test_outcome(0, Ok(true), 0)
                    .notice(rename)
                    .as_deref(),
                Some(if rename {
                    "名前の変更は中断されました"
                } else {
                    "操作が中断されました。移動・コピーした項目はありません"
                }),
            );
            assert_eq!(
                ShellTransferOutcome::test_outcome(0, Ok(true), 1)
                    .notice(rename)
                    .as_deref(),
                Some(
                    "操作が中断されました。一部の項目は処理済みです。フォルダの内容を確認してください"
                ),
            );
            for processed in [0, 1] {
                for abort in [Ok(false), Err(query_failure)] {
                    let message = ShellTransferOutcome::test_outcome(failure, abort, processed)
                        .notice(rename)
                        .unwrap();
                    assert!(message.starts_with(
                        "操作を完了できませんでした。フォルダの内容を確認してください"
                    ));
                    assert!(message.contains("0x80070005"));
                    assert_eq!(message.contains("一部の項目は処理済みです"), processed > 0);
                    assert!(message.lines().nth(1).unwrap().contains("0x80070005"));
                    assert!(message.contains(if abort.is_err() {
                        "照会失敗 0x80004005"
                    } else {
                        "中断なし"
                    }));
                }
                let interrupted = ShellTransferOutcome::test_outcome(failure, Ok(true), processed)
                    .notice(rename)
                    .unwrap();
                assert!(interrupted.contains("中断されました"));
                assert!(interrupted.contains("0x80070005、中断状態: 中断あり"));
            }
            let message = ShellTransferOutcome::test_outcome(0, Err(query_failure), 0)
                .notice(rename)
                .unwrap();
            assert!(message.contains("Shell: 0x00000000、中断状態: 照会失敗 0x80004005"));
            let message = ShellTransferOutcome::test_outcome(
                COPYENGINE_USER_CANCELLED,
                Err(query_failure),
                0,
            )
            .notice(rename)
            .unwrap();
            assert!(message.contains("Shell: 0x80270000、中断状態: 照会失敗 0x80004005"));
        }
    }

    fn callback_item(hr: i32) -> ShellItemResult {
        ShellItemResult {
            kind: "Move",
            source: Some("source".into()),
            destination: Some("destination".into()),
            new_name: None,
            hresult: hr,
            created: None,
            has_created: false,
        }
    }

    fn record_callback(
        items: &mut ShellItemResults,
        kind: &'static str,
        source: &str,
        hr: i32,
        has_created: bool,
    ) {
        items.count(hr, has_created);
        let mut item = callback_item(hr);
        item.kind = kind;
        item.source = Some(source.to_owned());
        item.created = has_created.then(|| format!("destination/{source}"));
        item.has_created = has_created;
        items.record(item);
    }

    #[test]
    fn shell_op_858_pending_then_cancel_counts_one_result_per_item() {
        for (kind, rename) in [("Move", false), ("Rename", true)] {
            for cancel in [COPYENGINE_USER_CANCELLED, WIN32_CANCELLED] {
                let mut items = ShellItemResults::default();
                record_callback(&mut items, kind, "locked-folder", COPYENGINE_PENDING, false);
                assert_eq!(
                    items.counts,
                    ShellItemCounts {
                        pending: 1,
                        ..Default::default()
                    }
                );
                record_callback(&mut items, kind, "locked-folder", cancel, false);
                assert_eq!(
                    items.counts,
                    ShellItemCounts {
                        cancelled: 1,
                        pending: 1,
                        ..Default::default()
                    }
                );
                assert_eq!(items.total_callbacks, 2);
                assert_eq!(items.details.len(), 2);
                assert_eq!(items.details[0].source, items.details[1].source);
                assert_eq!(items.details[0].hresult, COPYENGINE_PENDING);
                assert_eq!(items.details[1].hresult, cancel);
                assert_eq!(items.omitted(), 0);
                let outcome = ShellTransferOutcome {
                    perform_hresult: cancel,
                    abort_query: Ok(true),
                    counts: items.counts,
                };
                assert_eq!(
                    outcome.classification(),
                    ShellOperationClassification::Interrupted
                );
                assert_eq!(
                    outcome.notice(rename).as_deref(),
                    Some(if rename {
                        "名前の変更は中断されました"
                    } else {
                        "操作が中断されました。移動・コピーした項目はありません"
                    })
                );
            }
        }
    }

    #[test]
    fn shell_op_858_pending_then_success_counts_only_final_item() {
        for has_created in [false, true] {
            let mut items = ShellItemResults::default();
            record_callback(
                &mut items,
                "Move",
                "folder",
                COPYENGINE_PENDING,
                has_created,
            );
            assert_eq!(
                items.counts,
                ShellItemCounts {
                    pending: 1,
                    ..Default::default()
                }
            );
            record_callback(&mut items, "Move", "folder", 0x00270008, true);
            assert_eq!(
                items.counts,
                ShellItemCounts {
                    processed: 1,
                    pending: 1,
                    ..Default::default()
                }
            );
            assert_eq!(items.details.len(), 2);
            assert_eq!(items.omitted(), 0);
        }
    }

    #[test]
    fn shell_op_858_sub_pc2_partial_move_notice_excludes_cancel_failure() {
        let mut items = ShellItemResults::default();
        for (source, hr, has_created) in [
            ("locked-folder", COPYENGINE_PENDING, false),
            ("ok-folder", 0x00270008, true),
            ("locked-folder", COPYENGINE_USER_CANCELLED, false),
        ] {
            record_callback(&mut items, "Move", source, hr, has_created);
        }
        assert_eq!(
            items.counts,
            ShellItemCounts {
                processed: 1,
                cancelled: 1,
                pending: 1,
                ..Default::default()
            }
        );
        assert_eq!(items.total_callbacks, 3);
        assert_eq!(items.details.len(), 3);
        let outcome = ShellTransferOutcome {
            perform_hresult: COPYENGINE_USER_CANCELLED,
            abort_query: Ok(true),
            counts: items.counts,
        };
        assert_eq!(
            outcome.notice(false).as_deref(),
            Some(
                "操作が中断されました。一部の項目は処理済みです。フォルダの内容を確認してください"
            )
        );
    }

    #[test]
    fn shell_op_858_cancel_mixed_with_real_failure_keeps_real_hresult() {
        let failure = 0x80270027u32 as i32;
        for cancel in [COPYENGINE_USER_CANCELLED, WIN32_CANCELLED] {
            for (kind, rename) in [("Move", false), ("Rename", true)] {
                for cancel_first in [false, true] {
                    let mut items = ShellItemResults::default();
                    for hr in if cancel_first {
                        [cancel, failure]
                    } else {
                        [failure, cancel]
                    } {
                        record_callback(
                            &mut items,
                            kind,
                            if hr == cancel {
                                "cancelled-folder"
                            } else {
                                "failed-folder"
                            },
                            hr,
                            false,
                        );
                    }
                    assert_eq!(
                        items.counts,
                        ShellItemCounts {
                            failed: 1,
                            cancelled: 1,
                            first_failed_hresult: Some(failure),
                            ..Default::default()
                        }
                    );
                    let outcome = ShellTransferOutcome {
                        perform_hresult: cancel,
                        abort_query: Ok(true),
                        counts: items.counts,
                    };
                    let headline = if rename {
                        "名前の変更は中断されました"
                    } else {
                        "操作が中断されました。移動・コピーした項目はありません"
                    };
                    assert_eq!(
                        outcome.notice(rename).unwrap(),
                        format!("{headline}\n失敗した項目: 1 件 (Shell: 0x80270027)")
                    );
                }
            }
        }
    }

    #[test]
    fn shell_op_858_omitted_counts_pending_callbacks_beyond_detail_limit() {
        let mut items = ShellItemResults::default();
        for _ in 0..SHELL_ITEM_DETAIL_LIMIT + 3 {
            record_callback(
                &mut items,
                "Move",
                "locked-folder",
                COPYENGINE_PENDING,
                false,
            );
        }
        assert_eq!(
            items.counts,
            ShellItemCounts {
                pending: SHELL_ITEM_DETAIL_LIMIT + 3,
                ..Default::default()
            }
        );
        assert_eq!(items.details.len(), SHELL_ITEM_DETAIL_LIMIT);
        assert_eq!(items.omitted(), 3);
        record_callback(
            &mut items,
            "Move",
            "locked-folder",
            COPYENGINE_USER_CANCELLED,
            false,
        );
        assert_eq!(items.counts.cancelled, 1);
        assert_eq!(items.counts.failed, 0);
        assert_eq!(items.counts.first_failed_hresult, None);
        assert_eq!(items.total_callbacks, SHELL_ITEM_DETAIL_LIMIT + 4);
        assert_eq!(items.omitted(), 4);
    }

    #[test]
    fn shell_op_858_failed_notice_uses_only_first_real_failure() {
        let failure = 0x80270027u32 as i32;
        let other_failure = 0x80070005u32 as i32;
        for cancel in [COPYENGINE_USER_CANCELLED, WIN32_CANCELLED] {
            for rename in [false, true] {
                let mut items = ShellItemResults::default();
                record_callback(
                    &mut items,
                    "Move",
                    "locked-folder",
                    COPYENGINE_PENDING,
                    false,
                );
                record_callback(&mut items, "Move", "locked-folder", cancel, true);
                let mut outcome = ShellTransferOutcome {
                    perform_hresult: other_failure,
                    abort_query: Ok(false),
                    counts: items.counts,
                };
                assert_eq!(
                    outcome.classification(),
                    ShellOperationClassification::Failed
                );
                assert_eq!(
                    outcome.notice(rename).unwrap(),
                    "操作を完了できませんでした。フォルダの内容を確認してください\nShell: 0x80070005、中断状態: 中断なし"
                );
                record_callback(&mut items, "Move", "other-folder", failure, false);
                record_callback(&mut items, "Move", "last-folder", other_failure, false);
                outcome.counts = items.counts;
                assert_eq!(outcome.counts.first_failed_hresult, Some(failure));
                assert_eq!(
                    outcome.notice(rename).unwrap(),
                    "操作を完了できませんでした。フォルダの内容を確認してください\nShell: 0x80070005、中断状態: 中断なし、項目: 0x80270027"
                );
            }
        }
    }

    #[test]
    fn shell_op_858_merged_interruption_notice_checks_folder() {
        let failure = 0x80270027u32 as i32;
        for rename in [false, true] {
            for processed in [0, 1] {
                let mut outcome = ShellTransferOutcome::test_outcome(0, Ok(true), processed);
                outcome.counts.merged = 1;
                let headline = if processed > 0 {
                    "操作が中断されました。一部の項目は処理済みです。フォルダの内容を確認してください"
                } else if rename {
                    "名前の変更は中断されました"
                } else {
                    "操作が中断されました。フォルダの内容を確認してください"
                };
                assert_eq!(outcome.notice(rename).as_deref(), Some(headline));
                outcome.counts.failed = 1;
                outcome.counts.first_failed_hresult = Some(failure);
                assert_eq!(
                    outcome.notice(rename).unwrap(),
                    format!("{headline}\n失敗した項目: 1 件 (Shell: 0x80270027)")
                );
            }
        }
    }

    #[test]
    fn shell_op_858_merge_parent_then_child_cancel_has_no_processed_items() {
        for parent_hr in [COPYENGINE_NOT_HANDLED, COPYENGINE_MERGE] {
            for kind in ["Move", "Copy"] {
                for has_created in [false, true] {
                    let mut items = ShellItemResults::default();
                    record_callback(&mut items, kind, "m", parent_hr, has_created);
                    assert_eq!(
                        items.counts,
                        ShellItemCounts {
                            merged: 1,
                            ..Default::default()
                        }
                    );
                    record_callback(
                        &mut items,
                        kind,
                        "m/a.txt",
                        COPYENGINE_USER_CANCELLED,
                        false,
                    );
                    assert_eq!(
                        items.counts,
                        ShellItemCounts {
                            merged: 1,
                            cancelled: 1,
                            ..Default::default()
                        }
                    );
                    assert_eq!(items.total_callbacks, 2);
                    assert_eq!(items.details.len(), 2);
                    assert_eq!(items.omitted(), 0);
                    let outcome = ShellTransferOutcome {
                        perform_hresult: COPYENGINE_USER_CANCELLED,
                        abort_query: Ok(true),
                        counts: items.counts,
                    };
                    assert_eq!(
                        outcome.notice(false).as_deref(),
                        Some("操作が中断されました。フォルダの内容を確認してください")
                    );
                }
            }
        }
    }

    #[test]
    fn shell_op_858_merge_parent_then_child_processed_counts_only_child() {
        for parent_hr in [COPYENGINE_NOT_HANDLED, COPYENGINE_MERGE] {
            for (kind, child_hr) in [("Move", 0x00270008), ("Copy", 0)] {
                let mut items = ShellItemResults::default();
                record_callback(&mut items, kind, "m", parent_hr, true);
                record_callback(&mut items, kind, "m/a.txt", child_hr, true);
                assert_eq!(
                    items.counts,
                    ShellItemCounts {
                        merged: 1,
                        processed: 1,
                        ..Default::default()
                    }
                );
                assert_eq!(items.total_callbacks, 2);
                assert_eq!(items.details.len(), 2);
                assert_eq!(items.omitted(), 0);
            }
        }
    }

    #[test]
    fn shell_op_858_pending_then_real_failure_counts_only_failure() {
        let failure = 0x80270027u32 as i32;
        let mut items = ShellItemResults::default();
        record_callback(&mut items, "Move", "m/a.txt", COPYENGINE_PENDING, false);
        record_callback(&mut items, "Move", "m/a.txt", failure, false);
        assert_eq!(
            items.counts,
            ShellItemCounts {
                pending: 1,
                failed: 1,
                first_failed_hresult: Some(failure),
                ..Default::default()
            }
        );
        assert_eq!(items.total_callbacks, 2);
        assert_eq!(items.details.len(), 2);
        assert_eq!(items.omitted(), 0);
    }

    #[test]
    fn shell_op_858_pending_then_success_without_created_counts_only_skip() {
        let mut items = ShellItemResults::default();
        record_callback(&mut items, "Copy", "m/a.txt", COPYENGINE_PENDING, false);
        record_callback(&mut items, "Copy", "m/a.txt", 0, false);
        assert_eq!(
            items.counts,
            ShellItemCounts {
                pending: 1,
                skipped: 1,
                ..Default::default()
            }
        );
        assert_eq!(items.total_callbacks, 2);
        assert_eq!(items.details.len(), 2);
        assert_eq!(items.omitted(), 0);
    }

    #[test]
    fn shell_op_858_counts_only_completed_items_and_bounds_detail() {
        for hr in [
            0, 1, 0x00270001, 0x00270004, 0x00270005, 0x00270008, 0x0027000A, 0x0027000C,
            0x0027000D, 0x0027000E, 0x0027000F, 0x00270010, 0x00270011, 0x0027FFFF,
        ] {
            for has_created in [false, true] {
                let mut items = ShellItemResults::default();
                // Item presence must count even when its parsing name is unavailable.
                items.count(hr, has_created);
                items.record(callback_item(hr));
                assert_eq!(
                    items.counts,
                    ShellItemCounts {
                        processed: usize::from(has_created),
                        skipped: usize::from(!has_created),
                        ..Default::default()
                    },
                    "hr=0x{hr:08X}, created={has_created}"
                );
            }
        }
        let failure = 0x80270027u32 as i32;
        for has_created in [false, true] {
            let mut items = ShellItemResults::default();
            items.count(failure, has_created);
            items.record(callback_item(failure));
            assert_eq!(
                items.counts,
                ShellItemCounts {
                    failed: 1,
                    first_failed_hresult: Some(failure),
                    ..Default::default()
                }
            );
        }
        let mut items = ShellItemResults::default();
        for _ in 0..SHELL_ITEM_DETAIL_LIMIT {
            items.count(0, true);
            items.record(callback_item(0));
        }
        // The count-only path must retain item presence and the first failure too.
        items.count(0x00270008, true);
        items.count(0, false);
        items.count(failure, false);
        items.count(0x80070005u32 as i32, true);
        items.count(COPYENGINE_MERGE, true);
        items.count(COPYENGINE_NOT_HANDLED, false);
        assert_eq!(
            items.counts,
            ShellItemCounts {
                processed: SHELL_ITEM_DETAIL_LIMIT + 1,
                skipped: 1,
                failed: 2,
                merged: 2,
                first_failed_hresult: Some(failure),
                ..Default::default()
            }
        );
        assert_eq!(items.details.len(), SHELL_ITEM_DETAIL_LIMIT);
        assert_eq!(items.omitted(), 6);
        let skipped = ShellTransferOutcome {
            perform_hresult: 0,
            abort_query: Ok(true),
            counts: ShellItemCounts {
                skipped: 5,
                ..Default::default()
            },
        };
        assert_eq!(
            skipped.notice(false).as_deref(),
            Some("操作が中断されました。移動・コピーした項目はありません")
        );
    }

    #[test]
    fn shell_op_858_item_failure_notices_keep_first_hresult() {
        let failure = 0x80270027u32 as i32;
        let other_failure = 0x80070005u32 as i32;
        for rename in [false, true] {
            for processed in [0, 1] {
                let mut outcome = ShellTransferOutcome::test_outcome(0, Ok(true), processed);
                outcome.counts.failed = 2;
                outcome.counts.first_failed_hresult = Some(failure);
                let headline = if processed > 0 {
                    "操作が中断されました。一部の項目は処理済みです。フォルダの内容を確認してください"
                } else if rename {
                    "名前の変更は中断されました"
                } else {
                    "操作が中断されました。移動・コピーした項目はありません"
                };
                assert_eq!(
                    outcome.notice(rename).unwrap(),
                    format!("{headline}\n失敗した項目: 2 件 (Shell: 0x80270027)")
                );
                outcome.perform_hresult = other_failure;
                let message = outcome.notice(rename).unwrap();
                assert_eq!(
                    message.lines().nth(1),
                    Some("失敗した項目: 2 件 (Shell: 0x80270027)")
                );
                assert_eq!(
                    message.lines().nth(2),
                    Some("Shell: 0x80070005、中断状態: 中断あり")
                );
                outcome.abort_query = Ok(false);
                let message = outcome.notice(rename).unwrap();
                assert!(
                    message.ends_with("Shell: 0x80070005、中断状態: 中断なし、項目: 0x80270027")
                );
                outcome.perform_hresult = failure;
                let message = outcome.notice(rename).unwrap();
                assert!(message.ends_with("Shell: 0x80270027、中断状態: 中断なし"));
                assert!(!message.contains("、項目:"));
                outcome.perform_hresult = 0;
                outcome.abort_query = Err(other_failure);
                assert!(outcome.notice(rename).unwrap().ends_with(
                    "Shell: 0x00000000、中断状態: 照会失敗 0x80070005、項目: 0x80270027"
                ));
            }
        }
    }

    #[cfg(windows)]
    struct FenceCachedZipCleanup(PathBuf);

    #[cfg(windows)]
    impl Drop for FenceCachedZipCleanup {
        fn drop(&mut self) {
            crate::zip_loader::retire_archive_directory_for_mutation(&self.0);
        }
    }

    #[cfg(windows)]
    #[test]
    fn shell_op_858_hresult_values_match_windows_crate() {
        use windows::Win32::Foundation::{ERROR_CANCELLED, S_OK};
        use windows::Win32::UI::Shell::*;
        assert_eq!(COPYENGINE_USER_CANCELLED, COPYENGINE_E_USER_CANCELLED.0);
        assert_eq!(COPYENGINE_PENDING, COPYENGINE_S_PENDING.0);
        assert_eq!(COPYENGINE_NOT_HANDLED, COPYENGINE_S_NOT_HANDLED.0);
        assert_eq!(COPYENGINE_MERGE, COPYENGINE_S_MERGE.0);
        assert_eq!(
            WIN32_CANCELLED,
            windows::core::HRESULT::from_win32(ERROR_CANCELLED.0).0
        );
        assert_eq!(S_OK.0, 0);
        for (hr, expected) in [
            (COPYENGINE_S_YES, 0x00270001),
            (COPYENGINE_S_NOT_HANDLED, 0x00270003),
            (COPYENGINE_S_USER_RETRY, 0x00270004),
            (COPYENGINE_S_USER_IGNORED, 0x00270005),
            (COPYENGINE_S_MERGE, 0x00270006),
            (COPYENGINE_S_DONT_PROCESS_CHILDREN, 0x00270008),
            (COPYENGINE_S_ALREADY_DONE, 0x0027000A),
            (COPYENGINE_S_PENDING, 0x0027000B),
            (COPYENGINE_S_KEEP_BOTH, 0x0027000C),
            (COPYENGINE_S_CLOSE_PROGRAM, 0x0027000D),
            (COPYENGINE_S_COLLISIONRESOLVED, 0x0027000E),
            (COPYENGINE_S_PROGRESS_PAUSE, 0x0027000F),
            (COPYENGINE_S_PENDING_DELETE, 0x00270010),
        ] {
            assert_eq!(hr.0, expected);
        }
    }

    #[cfg(windows)]
    #[windows::core::implement(windows::Win32::UI::Shell::IShellItem)]
    struct ShellOp858CreatedItem;

    #[cfg(windows)]
    #[allow(non_snake_case)]
    impl windows::Win32::UI::Shell::IShellItem_Impl for ShellOp858CreatedItem_Impl {
        fn BindToHandler(
            &self,
            _: windows::core::Ref<windows::Win32::System::Com::IBindCtx>,
            _: *const windows::core::GUID,
            _: *const windows::core::GUID,
            _: *mut *mut core::ffi::c_void,
        ) -> windows::core::Result<()> {
            Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_NOTIMPL,
            ))
        }
        fn GetParent(&self) -> windows::core::Result<windows::Win32::UI::Shell::IShellItem> {
            Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_NOTIMPL,
            ))
        }
        fn GetDisplayName(
            &self,
            _: windows::Win32::UI::Shell::SIGDN,
        ) -> windows::core::Result<windows::core::PWSTR> {
            Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_NOTIMPL,
            ))
        }
        fn GetAttributes(
            &self,
            _: windows::Win32::System::SystemServices::SFGAO_FLAGS,
        ) -> windows::core::Result<windows::Win32::System::SystemServices::SFGAO_FLAGS> {
            Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_NOTIMPL,
            ))
        }
        fn Compare(
            &self,
            _: windows::core::Ref<windows::Win32::UI::Shell::IShellItem>,
            _: u32,
        ) -> windows::core::Result<i32> {
            Err(windows::core::Error::from_hresult(
                windows::Win32::Foundation::E_NOTIMPL,
            ))
        }
    }

    #[cfg(windows)]
    #[test]
    fn shell_op_858_sink_records_failures_without_stopping_later_items() {
        use windows::Win32::UI::Shell::{IFileOperationProgressSink, IShellItem};
        use windows::core::{HRESULT, PCWSTR};
        let items = std::sync::Arc::new(std::sync::Mutex::new(ShellItemResults::default()));
        let sink: IFileOperationProgressSink = ShellResultSink {
            items: items.clone(),
            rename_destination: Some(PathBuf::from(r"C:\source")),
        }
        .into();
        let none = None::<&IShellItem>;
        let created: IShellItem = ShellOp858CreatedItem.into();
        let failure = HRESULT(0x80070005u32 as i32);
        unsafe {
            sink.PostMoveItem(0, none, none, PCWSTR::null(), failure, none)
                .unwrap();
            sink.PostCopyItem(0, none, none, PCWSTR::null(), failure, none)
                .unwrap();
            sink.PostRenameItem(0, none, PCWSTR::null(), failure, none)
                .unwrap();
            sink.PostCopyItem(0, none, none, PCWSTR::null(), HRESULT(0), none)
                .unwrap();
            sink.PostCopyItem(0, none, none, PCWSTR::null(), HRESULT(0), &created)
                .unwrap();
            sink.PostMoveItem(0, none, none, PCWSTR::null(), HRESULT(0x00270008), &created)
                .unwrap();
            sink.PostRenameItem(0, none, PCWSTR::null(), HRESULT(0x00270008), &created)
                .unwrap();
        }
        let recorded = items.lock().unwrap();
        assert_eq!(
            recorded.counts,
            ShellItemCounts {
                processed: 3,
                failed: 3,
                skipped: 1,
                first_failed_hresult: Some(failure.0),
                ..Default::default()
            }
        );
        assert_eq!(
            recorded
                .details
                .iter()
                .map(|item| item.kind)
                .collect::<Vec<_>>(),
            ["Move", "Copy", "Rename", "Copy", "Copy", "Move", "Rename"]
        );
        assert_eq!(
            recorded.details[2].destination.as_deref(),
            Some(r"C:\source")
        );
        assert_eq!(recorded.details[0].hresult, failure.0);
        drop(recorded);
        for _ in 7..SHELL_ITEM_DETAIL_LIMIT {
            unsafe { sink.PostCopyItem(0, none, none, PCWSTR::null(), HRESULT(0), none) }.unwrap();
        }
        unsafe {
            sink.PostMoveItem(0, none, none, PCWSTR::null(), HRESULT(0x00270008), &created)
                .unwrap();
            sink.PostRenameItem(
                0,
                none,
                PCWSTR::null(),
                HRESULT(0x80004005u32 as i32),
                &created,
            )
            .unwrap();
        }
        let recorded = items.lock().unwrap();
        assert_eq!(recorded.counts.processed, 4);
        assert_eq!(recorded.counts.failed, 4);
        assert_eq!(recorded.counts.skipped, 44);
        assert_eq!(recorded.counts.first_failed_hresult, Some(failure.0));
        assert_eq!(recorded.details.len(), SHELL_ITEM_DETAIL_LIMIT);
        assert_eq!(recorded.omitted(), 2);
        drop(recorded);
        for hr in [
            COPYENGINE_PENDING,
            COPYENGINE_USER_CANCELLED,
            WIN32_CANCELLED,
            COPYENGINE_MERGE,
            COPYENGINE_NOT_HANDLED,
        ] {
            unsafe {
                sink.PostMoveItem(0, none, none, PCWSTR::null(), HRESULT(hr), &created)
                    .unwrap();
                sink.PostCopyItem(0, none, none, PCWSTR::null(), HRESULT(hr), &created)
                    .unwrap();
                sink.PostRenameItem(0, none, PCWSTR::null(), HRESULT(hr), &created)
                    .unwrap();
            }
        }
        let recorded = items.lock().unwrap();
        assert_eq!(recorded.counts.processed, 4);
        assert_eq!(recorded.counts.failed, 4);
        assert_eq!(recorded.counts.cancelled, 6);
        assert_eq!(recorded.counts.skipped, 44);
        assert_eq!(recorded.counts.pending, 3);
        assert_eq!(recorded.counts.merged, 6);
        assert_eq!(recorded.counts.first_failed_hresult, Some(failure.0));
        assert_eq!(recorded.total_callbacks, SHELL_ITEM_DETAIL_LIMIT + 17);
        assert_eq!(recorded.omitted(), 17);
    }

    fn request(
        operation: ShellTransferOperation,
        sources: Vec<PathBuf>,
        destination: PathBuf,
    ) -> ShellTransferRequest {
        ShellTransferRequest {
            operation,
            default_move_needs_volume_check: false,
            sources,
            destination,
        }
    }

    #[test]
    fn fence_858b_file_only_and_copy_operations_register_none() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("folder");
        let destination = temp.path().join("destination");
        let file = temp.path().join("book.zip");
        std::fs::create_dir(&folder).unwrap();
        std::fs::create_dir(&destination).unwrap();
        std::fs::write(&file, b"file").unwrap();
        for (operation, sources) in [
            (ShellTransferOperation::Move, vec![file.clone()]),
            (
                ShellTransferOperation::Copy,
                vec![folder.clone(), file.clone()],
            ),
        ] {
            let validated =
                validate_transfer_request(request(operation, sources, destination.clone()))
                    .unwrap();
            assert!(validated.mutation_folders().is_empty());
            let _guard =
                crate::mutation_fence::begin_with_releases(validated.mutation_folders(), || {
                    panic!("must not release")
                });
            assert!(!crate::mutation_fence::covers(&folder.join("child.zip")));
        }
        let moved = validate_transfer_request(request(
            ShellTransferOperation::Move,
            vec![folder.clone(), file],
            destination,
        ))
        .unwrap();
        assert_eq!(moved.mutation_folders(), &[folder.clone()]);
        let _guard = crate::mutation_fence::begin_with_releases(moved.mutation_folders(), || {});
        assert!(crate::mutation_fence::covers(&folder.join("child.zip")));
    }

    #[test]
    fn fence_858b_downgraded_move_registers_none() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("folder");
        let destination = temp.path().join("destination");
        std::fs::create_dir(&folder).unwrap();
        std::fs::create_dir(&destination).unwrap();
        let mut input = request(
            ShellTransferOperation::Move,
            vec![folder.clone()],
            destination.clone(),
        );
        input.default_move_needs_volume_check = true;
        let resolved = resolve_default_move(input, |path| {
            Some(
                if path == destination {
                    "destination"
                } else {
                    "source"
                }
                .into(),
            )
        });
        let validated = validate_transfer_request(resolved).unwrap();
        assert_eq!(validated.request.operation, ShellTransferOperation::Copy);
        assert!(validated.mutation_folders().is_empty());
        let _guard =
            crate::mutation_fence::begin_with_releases(validated.mutation_folders(), || {
                panic!("copy must not release")
            });
        assert!(!crate::mutation_fence::covers(&folder.join("child.zip")));
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Windows Shell; suppressed UI and disposable folders"]
    fn fence_858b_real_move_cached_zip_folder() {
        for bypass in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let folder = temp.path().join("source");
            let destination = temp.path().join("destination");
            std::fs::create_dir(&folder).unwrap();
            std::fs::create_dir(&destination).unwrap();
            let zip = folder.join("book.zip");
            let _cleanup = FenceCachedZipCleanup(zip.clone());
            crate::zip_loader::fence_858b_cache_zip(&zip);
            let source = folder.clone();
            let destination_worker = destination.clone();
            let outcome = std::thread::spawn(move || {
                FENCE_TEST_QUIET.with(|quiet| quiet.set(true));
                let run = || {
                    transfer_items_blocking(
                        None,
                        request(
                            ShellTransferOperation::Move,
                            vec![source],
                            destination_worker,
                        ),
                    )
                };
                if bypass {
                    crate::mutation_fence::without_fence_for_test(run)
                } else {
                    run()
                }
            })
            .join()
            .unwrap()
            .unwrap();
            eprintln!("fence_858b_real_move: bypass={bypass} outcome={outcome:?}");
            crate::zip_loader::retire_archive_directory_for_mutation(&zip);
            if bypass {
                assert_eq!(
                    outcome.classification(),
                    ShellOperationClassification::Interrupted
                );
                assert_eq!(outcome.abort_query, Ok(true));
                assert!(folder.exists());
            } else {
                assert_eq!(
                    outcome.classification(),
                    ShellOperationClassification::Completed
                );
                assert!(!folder.exists());
                assert!(destination.join("source/book.zip").exists());
            }
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Windows Shell; suppressed UI and disposable folders"]
    fn fence_858b_real_rename_cached_zip_folder() {
        for bypass in [true, false] {
            let temp = tempfile::tempdir().unwrap();
            let folder = temp.path().join("source");
            std::fs::create_dir(&folder).unwrap();
            let zip = folder.join("book.zip");
            let _cleanup = FenceCachedZipCleanup(zip.clone());
            crate::zip_loader::fence_858b_cache_zip(&zip);
            let source = folder.clone();
            let outcome = std::thread::spawn(move || {
                FENCE_TEST_QUIET.with(|quiet| quiet.set(true));
                let run = || run_rename_item(0, source, "renamed".into());
                if bypass {
                    crate::mutation_fence::without_fence_for_test(run)
                } else {
                    run()
                }
            })
            .join()
            .unwrap()
            .unwrap();
            eprintln!("fence_858b_real_rename: bypass={bypass} outcome={outcome:?}");
            crate::zip_loader::retire_archive_directory_for_mutation(&zip);
            if bypass {
                assert_eq!(
                    outcome.operation.classification(),
                    ShellOperationClassification::Interrupted
                );
                assert_eq!(outcome.operation.abort_query, Ok(true));
                assert!(folder.exists());
            } else {
                assert_eq!(
                    outcome.operation.classification(),
                    ShellOperationClassification::Completed
                );
                assert!(!folder.exists());
                assert!(temp.path().join("renamed/book.zip").exists());
            }
        }
    }

    #[test]
    fn shell_transfer_default_move_volume_resolution_only_downgrades() {
        for (operation, check) in [
            (ShellTransferOperation::Copy, false),
            (ShellTransferOperation::Copy, true),
            (ShellTransferOperation::Move, false),
        ] {
            let mut input = request(operation, vec![r"D:\src".into()], r"C:\MountD\dst".into());
            input.default_move_needs_volume_check = check;
            let resolved = resolve_default_move(input, |_| {
                panic!("Copy preview and explicit Move must not query volumes")
            });
            assert_eq!(resolved.operation, operation);
            assert!(!resolved.default_move_needs_volume_check);
        }
        for (destination, first, second, expected) in [
            (
                Some("VOLUME"),
                Some("volume"),
                Some("volume"),
                ShellTransferOperation::Move,
            ),
            (
                Some("volume"),
                Some("other"),
                Some("volume"),
                ShellTransferOperation::Copy,
            ),
            (
                Some("volume"),
                Some("volume"),
                Some("other"),
                ShellTransferOperation::Copy,
            ),
            (
                Some("volume"),
                Some("volume"),
                None,
                ShellTransferOperation::Copy,
            ),
            (
                None,
                Some("volume"),
                Some("volume"),
                ShellTransferOperation::Copy,
            ),
        ] {
            let mut input = request(
                ShellTransferOperation::Move,
                vec!["first".into(), "second".into()],
                "destination".into(),
            );
            input.default_move_needs_volume_check = true;
            let resolved = resolve_default_move(input, |path| match path.to_str().unwrap() {
                "destination" => destination.map(str::to_owned),
                "first" => first.map(str::to_owned),
                "second" => second.map(str::to_owned),
                _ => unreachable!(),
            });
            assert_eq!(resolved.operation, expected);
            assert!(!resolved.default_move_needs_volume_check);
        }
    }

    #[test]
    fn shell_transfer_path_validation_all_sources_and_same_parent() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("folder");
        std::fs::create_dir(&folder).unwrap();
        let file = temp.path().join("file.txt");
        std::fs::write(&file, b"file").unwrap();
        let copy = validate_transfer_request(request(
            ShellTransferOperation::Copy,
            vec![file.clone(), folder.clone()],
            temp.path().to_path_buf(),
        ))
        .unwrap();
        assert_eq!(copy.request.sources.len(), 2);
        let another_parent = temp.path().join("other");
        std::fs::create_dir(&another_parent).unwrap();
        let another_file = another_parent.join("other.txt");
        std::fs::write(&another_file, b"other").unwrap();
        let error = validate_transfer_request(request(
            ShellTransferOperation::Move,
            vec![another_file, file],
            temp.path().to_path_buf(),
        ))
        .unwrap_err();
        assert!(error.contains("同じフォルダ"));
    }

    #[test]
    fn shell_transfer_path_validation_self_descendant_and_neighbor() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("photos");
        let child = folder.join("child");
        let neighbor = temp.path().join("photos-old");
        std::fs::create_dir_all(&child).unwrap();
        std::fs::create_dir(&neighbor).unwrap();
        for operation in [ShellTransferOperation::Copy, ShellTransferOperation::Move] {
            for destination in [
                folder.clone(),
                child.clone(),
                child.join(".."),
                child.join("."),
            ] {
                let error = validate_transfer_request(request(
                    operation,
                    vec![folder.clone()],
                    destination,
                ))
                .unwrap_err();
                assert!(error.contains("フォルダ自身またはその配下"));
            }
            validate_transfer_request(request(operation, vec![folder.clone()], neighbor.clone()))
                .unwrap();
        }
    }

    #[test]
    fn shell_transfer_path_validation_missing_relative_nul_and_not_folder() {
        let temp = tempfile::tempdir().unwrap();
        let file = temp.path().join("file.txt");
        std::fs::write(&file, b"file").unwrap();
        for destination in [
            temp.path().join("missing"),
            PathBuf::from("relative"),
            temp.path().join("bad\0path"),
            file.clone(),
        ] {
            assert!(
                validate_transfer_request(request(
                    ShellTransferOperation::Copy,
                    vec![file.clone()],
                    destination
                ))
                .is_err()
            );
        }
        for source in [
            temp.path().join("missing"),
            PathBuf::from("relative"),
            temp.path().join("bad\0path"),
        ] {
            assert!(
                validate_transfer_request(request(
                    ShellTransferOperation::Copy,
                    vec![file.clone(), source],
                    temp.path().to_path_buf()
                ))
                .is_err()
            );
        }
        assert!(
            validate_transfer_request(request(
                ShellTransferOperation::Copy,
                vec![],
                temp.path().to_path_buf()
            ))
            .is_err()
        );
    }

    #[cfg(windows)]
    #[test]
    fn shell_transfer_path_components_drive_unc_case_and_separator() {
        for (left, right) in [
            (r"C:\PHOTOS\child", r"c:/photos/child/"),
            (
                r"\\server\share\PHOTOS\child",
                r"\\SERVER\SHARE\photos\child\",
            ),
        ] {
            assert_eq!(
                canonical_path_components(std::path::Path::new(left)),
                canonical_path_components(std::path::Path::new(right))
            );
        }
        let folder = canonical_path_components(std::path::Path::new(r"C:\photos"));
        for neighbor in [
            r"C:\photos-old",
            r"D:\photos\child",
            r"\\server\share\photos\child",
        ] {
            assert!(
                !canonical_path_components(std::path::Path::new(neighbor)).starts_with(&folder)
            );
        }
        let share = canonical_path_components(std::path::Path::new(r"\\server\share\photos"));
        assert!(
            !canonical_path_components(std::path::Path::new(r"\\server\share2\photos\child"))
                .starts_with(&share)
        );
    }

    #[cfg(windows)]
    #[test]
    fn shell_transfer_path_validation_resolves_junction() {
        let temp = tempfile::tempdir().unwrap();
        let folder = temp.path().join("source");
        let child = folder.join("child");
        let alias = temp.path().join("alias");
        std::fs::create_dir_all(&child).unwrap();
        let output = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(&alias)
            .arg(&child)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "junction creation failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let copy = validate_transfer_request(request(
            ShellTransferOperation::Copy,
            vec![folder.clone()],
            alias.clone(),
        ));
        let moved = validate_transfer_request(request(
            ShellTransferOperation::Move,
            vec![folder],
            alias.clone(),
        ));
        let aliased_source = validate_transfer_request(request(
            ShellTransferOperation::Copy,
            vec![alias.clone()],
            temp.path().to_path_buf(),
        ));
        let same_parent_move = validate_transfer_request(request(
            ShellTransferOperation::Move,
            vec![alias.clone()],
            temp.path().to_path_buf(),
        ));
        let other = child.join("other.txt");
        std::fs::write(&other, b"other item").unwrap();
        validate_transfer_request(request(
            ShellTransferOperation::Move,
            vec![other.clone()],
            temp.path().to_path_buf(),
        ))
        .unwrap();
        let mixed_move = validate_transfer_request(request(
            ShellTransferOperation::Move,
            vec![other.clone(), alias.clone()],
            temp.path().to_path_buf(),
        ));
        let target_parent_move = validate_transfer_request(request(
            ShellTransferOperation::Move,
            vec![alias.clone()],
            child.parent().unwrap().to_path_buf(),
        ));
        for refusal in [same_parent_move, mixed_move] {
            let error = refusal.unwrap_err();
            assert!(error.contains("同じフォルダなので移動できません"));
            assert!(error.contains(&alias.display().to_string()));
        }
        assert_eq!(
            target_parent_move.unwrap().request.sources,
            vec![alias.clone()]
        );
        assert_eq!(std::fs::read(&other).unwrap(), b"other item");
        assert!(!temp.path().join("other.txt").exists());
        std::fs::remove_dir(&alias).unwrap();
        assert!(copy.unwrap_err().contains("フォルダ自身またはその配下"));
        assert!(moved.unwrap_err().contains("フォルダ自身またはその配下"));
        assert_eq!(aliased_source.unwrap().request.sources, vec![alias]);
    }

    /// Print raw callbacks without requiring a particular merge-start HRESULT.
    #[cfg(windows)]
    #[test]
    #[ignore = "Windows Shell/environment dependent; suppressed UI, disposable temp data"]
    fn shell_op_858_real_merge_into_existing_folder_records_callbacks() {
        use std::os::windows::fs::OpenOptionsExt;
        use windows::Win32::Storage::FileSystem::FILE_SHARE_READ;
        use windows::Win32::UI::Shell::{FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT};

        for (case, operation, locked, empty) in [
            ("a-move", ShellTransferOperation::Move, false, false),
            ("b-locked-move", ShellTransferOperation::Move, true, false),
            ("c-copy", ShellTransferOperation::Copy, false, false),
            ("d-empty-move", ShellTransferOperation::Move, false, true),
        ] {
            let temp = tempfile::Builder::new()
                .prefix("miv-shell-op-858-merge-")
                .tempdir_in(std::env::temp_dir())
                .unwrap();
            let folder_name = if empty { "e" } else { "m" };
            let source_folder = temp.path().join("src").join(folder_name);
            let destination = temp.path().join("dst");
            let destination_folder = destination.join(folder_name);
            std::fs::create_dir_all(&source_folder).unwrap();
            std::fs::create_dir_all(&destination_folder).unwrap();
            let source_file = source_folder.join("a.txt");
            let destination_file = destination_folder.join("a.txt");
            let existing_file = destination_folder.join("b.txt");
            if !empty {
                std::fs::write(&source_file, b"a contents").unwrap();
            }
            std::fs::write(&existing_file, b"b contents").unwrap();
            let held = locked.then(|| {
                std::fs::OpenOptions::new()
                    .read(true)
                    .share_mode(FILE_SHARE_READ.0)
                    .open(&source_file)
                    .unwrap()
            });
            let items = std::sync::Arc::new(std::sync::Mutex::new(ShellItemResults::default()));
            let worker_items = items.clone();
            let input = validate_transfer_request(request(
                operation,
                vec![source_folder.clone()],
                destination,
            ))
            .unwrap();
            let outcome = std::thread::spawn(move || {
                run_transfer_items_with_flags(
                    0,
                    input,
                    operation,
                    transfer_flags() | FOF_NOERRORUI | FOF_NOCONFIRMATION | FOF_SILENT,
                    worker_items,
                )
            })
            .join()
            .unwrap();
            drop(held);
            {
                let recorded = items.lock().unwrap();
                for item in &recorded.details {
                    let source_name = item
                        .source
                        .as_deref()
                        .and_then(|source| std::path::Path::new(source).file_name());
                    eprintln!(
                        "shell_op_858_merge: case={case} kind={} hr=0x{:08X} has_created={} source_file_name={source_name:?}",
                        item.kind, item.hresult, item.has_created,
                    );
                }
            }
            eprintln!("shell_op_858_merge: case={case} outcome={outcome:?}");
            let outcome = outcome.unwrap();
            assert_eq!(
                std::fs::read(&existing_file).unwrap(),
                b"b contents",
                "{case}"
            );
            if empty {
                let source_exists = source_folder.try_exists().unwrap();
                eprintln!(
                    "shell_op_858_merge: case={case} source_folder_state={}",
                    if source_exists {
                        "still_exists"
                    } else {
                        "removed"
                    }
                );
                if source_exists {
                    assert!(source_folder.is_dir(), "{case}");
                }
                assert_eq!(outcome.counts.failed, 0, "{case}: {outcome:?}");
            } else if locked {
                assert_eq!(
                    std::fs::read(&source_file).unwrap(),
                    b"a contents",
                    "{case}"
                );
                assert!(!destination_file.exists(), "{case}");
                assert_eq!(outcome.counts.processed, 0, "{case}: {outcome:?}");
                assert_ne!(
                    outcome.classification(),
                    ShellOperationClassification::Completed,
                    "{case}"
                );
            } else {
                assert_eq!(
                    std::fs::read(&destination_file).unwrap(),
                    b"a contents",
                    "{case}"
                );
                if operation == ShellTransferOperation::Copy {
                    assert_eq!(
                        std::fs::read(&source_file).unwrap(),
                        b"a contents",
                        "{case}"
                    );
                } else {
                    assert!(!source_folder.exists(), "{case}");
                }
                assert!(outcome.counts.processed >= 1, "{case}: {outcome:?}");
                assert_eq!(outcome.counts.failed, 0, "{case}: {outcome:?}");
            }
            temp.close().unwrap();
        }
    }

    #[cfg(windows)]
    #[test]
    #[ignore = "Windows Shell/environment dependent; suppressed UI, disposable temp data"]
    fn shell_op_858_real_locked_folder_move_records_both_results() {
        use windows::Win32::UI::Shell::{FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT};
        let temp = tempfile::Builder::new()
            .prefix("miv-shell-op-858-")
            .tempdir_in(std::env::temp_dir())
            .unwrap();
        let folder = temp.path().join("source");
        let destination = temp.path().join("destination");
        std::fs::create_dir(&folder).unwrap();
        std::fs::create_dir(&destination).unwrap();
        let child = folder.join("held-open.txt");
        std::fs::write(&child, b"held open during folder move").unwrap();
        let held = std::fs::File::open(&child).unwrap();
        // Open descendants prevent directory rename, including same-volume moves.
        // https://learn.microsoft.com/en-us/windows-hardware/drivers/ddi/ntifs/ns-ntifs-_file_rename_information
        let items = std::sync::Arc::new(std::sync::Mutex::new(ShellItemResults::default()));
        let worker_items = items.clone();
        let input = validate_transfer_request(request(
            ShellTransferOperation::Move,
            vec![folder.clone()],
            destination.clone(),
        ))
        .unwrap();
        assert_eq!(volume_identity(&folder), volume_identity(&destination));
        assert!(volume_identity(&folder).is_some());
        let outcome = std::thread::spawn(move || {
            run_transfer_items_with_flags(
                0,
                input,
                ShellTransferOperation::Move,
                transfer_flags() | FOF_NOERRORUI | FOF_NOCONFIRMATION | FOF_SILENT,
                worker_items,
            )
        })
        .join()
        .unwrap()
        .unwrap();
        // The dev machine observed perform=S_OK and abort=Ok(true) with suppressed UI.
        // Its raw item HRESULT is unknown; accept cancel or a real item failure.
        assert_eq!(
            outcome.classification(),
            ShellOperationClassification::Interrupted
        );
        assert_eq!(outcome.abort_query, Ok(true));
        assert_eq!(outcome.counts.processed, 0);
        let notice = outcome.notice(false).unwrap();
        assert!(notice.contains("移動・コピーした項目はありません"));
        assert!(folder.is_dir());
        assert!(!destination.join("source").exists());
        let canonical_folder = std::fs::canonicalize(&folder).unwrap();
        let folder_hresult = items
            .lock()
            .unwrap()
            .details
            .iter()
            .find(|item| {
                item.kind == "Move"
                    && item.hresult < 0
                    && item.source.as_deref().is_some_and(|source| {
                        std::fs::canonicalize(source).ok().as_ref() == Some(&canonical_folder)
                    })
            })
            .map(|item| item.hresult)
            .expect("sink must record the stopped folder, not just a child");
        if is_cancel_hresult(folder_hresult) {
            assert!(outcome.counts.cancelled >= 1);
            assert_eq!(outcome.counts.failed, 0);
            assert_eq!(outcome.counts.first_failed_hresult, None);
            assert!(!notice.contains("失敗した項目"));
        } else {
            assert!(outcome.counts.failed >= 1);
            assert!(notice.lines().nth(1).unwrap().starts_with("失敗した項目: "));
        }
        drop(held);
        temp.close().unwrap();
    }

    /// Windows may display native progress/error UI; run explicitly on the user's desktop.
    #[cfg(windows)]
    #[test]
    #[ignore = "Windows Shell UI/environment dependent; uses disposable temp data"]
    fn shell_transfer_real_copy_and_move_file_and_folder() {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let copied = temp.path().join("copied");
        let moved = temp.path().join("moved");
        let folder = source.join("folder");
        std::fs::create_dir_all(folder.join("nested")).unwrap();
        std::fs::create_dir(&copied).unwrap();
        std::fs::create_dir(&moved).unwrap();
        let file = source.join("file.txt");
        std::fs::write(&file, b"file contents").unwrap();
        std::fs::write(folder.join("nested/child.txt"), b"folder contents").unwrap();
        for (operation, destination) in [
            (ShellTransferOperation::Copy, &copied),
            (ShellTransferOperation::Move, &moved),
        ] {
            let result = transfer_items_async(
                None,
                request(
                    operation,
                    vec![file.clone(), folder.clone()],
                    destination.clone(),
                ),
                egui::Context::default(),
            )
            .recv_timeout(std::time::Duration::from_secs(300))
            .expect("Shell operation did not finish within five minutes")
            .expect("Shell operation failed");
            assert_eq!(
                result.classification(),
                ShellOperationClassification::Completed
            );
            assert!(
                result.counts.processed >= 2,
                "file and folder callbacks: {result:?}"
            );
            assert_eq!(result.counts.failed, 0);
            assert_eq!(result.counts.skipped, 0);
            assert_eq!(
                std::fs::read(destination.join("file.txt")).unwrap(),
                b"file contents"
            );
            assert_eq!(
                std::fs::read(destination.join("folder/nested/child.txt")).unwrap(),
                b"folder contents"
            );
            assert_eq!(file.exists(), operation == ShellTransferOperation::Copy);
            assert_eq!(folder.exists(), operation == ShellTransferOperation::Copy);
        }
    }
}

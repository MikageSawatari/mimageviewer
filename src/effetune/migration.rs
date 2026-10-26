//! One-shot import of the released Mixwright data. Run this on a worker, before
//! starting either VST host. The source WebView lease spans quiescence and copy.

use std::fs::{self, File, OpenOptions};
use std::io;
use std::path::{Path, PathBuf};
use std::time::Duration;

#[cfg(test)]
mod tests;
#[cfg(windows)]
mod windows;

const DONE: &str = "migration-v1.done";
const NOT_APPLICABLE: &str = "migration-v1.not-applicable";
const PROCESS_WAIT_LIMIT: Duration = Duration::from_secs(5);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MigrationMode {
    Normal,
    Alternate,
    Portable,
}

#[derive(Clone, Debug)]
pub(crate) struct MigrationRequest {
    /// The old shared APPDATA root, never the mIV data directory.
    pub source_root: PathBuf,
    /// The selected data directory's `effetune` subdirectory.
    pub destination: PathBuf,
    pub mode: MigrationMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum MigrationOutcome {
    Done,
    NotApplicable,
    AlreadyDecided,
}

pub(crate) fn migrate(request: &MigrationRequest) -> Result<MigrationOutcome, String> {
    migrate_with_observer(request, |_| Ok(()))
}

/// The observer is for the isolated migration probe. All phase callbacks from
/// `lock_acquired` through `copy_end` run while the exclusive source lease lives.
pub(crate) fn migrate_with_observer(
    request: &MigrationRequest,
    observer: impl FnMut(&str) -> Result<(), String>,
) -> Result<MigrationOutcome, String> {
    #[cfg(windows)]
    {
        migrate_using(request, &mut windows::Backend::new(), observer)
    }
    #[cfg(not(windows))]
    {
        let _ = (request, observer);
        Err("EffeTune migration requires the Windows WebView2 lock backend".into())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Owner {
    SameUser,
    OtherUser,
    Unknown,
}

struct ProcessCandidate<H> {
    pid: u32,
    /// Unreadable includes processes already in STATUS_PROCESS_IS_TERMINATING.
    command: Option<String>,
    owner: Owner,
    synchronize: Option<H>,
}

trait MigrationBackend {
    type Lease;
    type Process;
    /// Nonfatal diagnostics only. Host presence must never gate migration.
    fn diagnostic_hosts(&mut self) {}
    fn lock(&mut self, path: &Path) -> Result<Self::Lease, String>;
    /// Return every msedgewebview2.exe, not only browser parent processes.
    fn processes(&mut self) -> Result<Vec<ProcessCandidate<Self::Process>>, String>;
    fn wait(&mut self, handle: Self::Process, remaining: Duration) -> Result<bool, String>;
    fn elapsed(&self) -> Duration;
    fn matches_udf(&self, command: &str, expected: &Path) -> Result<bool, String>;
}

fn migrate_using<B: MigrationBackend>(
    request: &MigrationRequest,
    backend: &mut B,
    mut observer: impl FnMut(&str) -> Result<(), String>,
) -> Result<MigrationOutcome, String> {
    if present(&request.destination)? {
        require_directory(&request.destination)?;
    }
    for name in [DONE, NOT_APPLICABLE] {
        let mark = request.destination.join(name);
        if present(&mark)? {
            require_regular_file(&mark)?;
            return Ok(MigrationOutcome::AlreadyDecided);
        }
    }
    backend.diagnostic_hosts();
    if request.mode == MigrationMode::Portable
        || (request.mode == MigrationMode::Alternate
            && !present(&request.destination.join("mixwright-state.json"))?)
    {
        write_mark(&request.destination, NOT_APPLICABLE)?;
        return Ok(MigrationOutcome::NotApplicable);
    }

    if present(&request.source_root)? {
        require_directory(&request.source_root)?;
    }

    let plugin_source = request.source_root.join("Frieve").join("EffeTunePlugin");
    for root in [request.source_root.join("Frieve"), plugin_source.clone()] {
        if present(&root)? {
            require_directory(&root)?;
        }
    }
    let preferred_presets = request.source_root.join("effetune");
    let preset_source = if present(&preferred_presets)? {
        require_directory(&preferred_presets)?;
        preferred_presets
    } else {
        plugin_source.clone()
    };
    let mut json_sources = Vec::new();
    for (root, names) in [
        (&plugin_source, ["config.json", "sfz-references.json"]),
        (
            &preset_source,
            ["effetune_presets.json", "effetune_plugin_presets.json"],
        ),
    ] {
        for name in names {
            let source = root.join(name);
            if present(&source)? {
                require_regular_file(&source)?;
                json_sources.push((source, name));
            }
        }
    }
    let webview_source = request.source_root.join("mimageviewer-vst3-host.exe");
    let has_webview = present(&webview_source)?;
    if has_webview {
        require_directory(&webview_source)?;
    }
    if request.mode == MigrationMode::Normal && !has_webview && json_sources.is_empty() {
        write_mark(&request.destination, NOT_APPLICABLE)?;
        return Ok(MigrationOutcome::NotApplicable);
    }
    for source in [&plugin_source, &preset_source, &webview_source] {
        ensure_separate_trees(source, &request.destination)?;
    }

    // Never replace unknown destination data. This run owns only directories
    // created below, including when a later copy or mark publication fails.
    for name in ["plugin", "webview"] {
        let path = request.destination.join(name);
        if present(&path)? {
            return Err(format!(
                "migration destination already exists: {}",
                path.display()
            ));
        }
    }
    let _lease = if has_webview {
        let ebwebview = webview_source.join("EBWebView");
        require_directory(&ebwebview)?;
        let lease = backend.lock(&ebwebview.join("lockfile"))?;
        observer("lock_acquired")?;
        observer("process_wait_begin")?;
        wait_for_source_processes(backend, &ebwebview)?;
        observer("process_wait_end")?;
        Some(lease)
    } else {
        None
    };

    fs::create_dir_all(&request.destination)
        .map_err(|e| io_error("create migration destination", &request.destination, e))?;
    let mut owned = Vec::new();
    let result = (|| {
        observer("copy_begin")?;
        if !json_sources.is_empty() {
            let plugin = request.destination.join("plugin");
            create_owned_directory(&plugin, &mut owned)?;
            for (source, name) in json_sources {
                let target = plugin.join(name);
                copy_regular_file(&source, &target)?;
                let copied =
                    fs::read(&target).map_err(|e| io_error("read copied JSON", &target, e))?;
                serde_json::from_slice::<serde_json::Value>(&copied)
                    .map_err(|e| format!("invalid copied JSON {}: {e}", target.display()))?;
            }
        }
        if has_webview {
            let webview = request.destination.join("webview");
            create_owned_directory(&webview, &mut owned)?;
            copy_webview_tree(&webview_source, &webview, Path::new(""))?;
        }
        observer("copy_end")?;
        write_mark(&request.destination, DONE)?;
        Ok(MigrationOutcome::Done)
    })();
    if result.is_err() {
        for path in owned {
            // Copy rejects reparse points, and cleanup never follows them.
            if let Err(e) = remove_owned_tree(&path) {
                crate::logger::log(format!(
                    "[EffeTune migration] cleanup {}: {e}",
                    path.display()
                ));
            }
        }
    }
    result
}

fn wait_for_source_processes<B: MigrationBackend>(
    backend: &mut B,
    udf: &Path,
) -> Result<(), String> {
    let started = backend.elapsed();
    loop {
        if backend.elapsed().saturating_sub(started) >= PROCESS_WAIT_LIMIT {
            return Err("WebView2 process quiescence exceeded 5 seconds".into());
        }
        let mut waiting = Vec::new();
        for candidate in backend.processes()? {
            let readable_match = candidate.command.as_deref().and_then(|command| {
                match backend.matches_udf(command, udf) {
                    Ok(matches) => Some(matches),
                    Err(reason) => {
                        crate::logger::log(format!(
                            "[EffeTune migration] PID {} command unreadable: {reason}",
                            candidate.pid
                        ));
                        None
                    }
                }
            });
            let wait = match readable_match {
                Some(matches) => matches,
                None => candidate.owner != Owner::OtherUser,
            };
            if readable_match.is_none() && !wait {
                crate::logger::log(format!(
                    "[EffeTune migration] excluding WebView2 PID {}: command unreadable, token SID confirms another user",
                    candidate.pid
                ));
            }
            if wait {
                crate::logger::log(format!(
                    "[EffeTune migration] waiting for WebView2 PID {} (command readable={}, owner={:?})",
                    candidate.pid,
                    readable_match.is_some(),
                    candidate.owner
                ));
                waiting.push((
                    candidate.pid,
                    candidate.synchronize.ok_or_else(|| {
                        format!(
                            "cannot obtain synchronization handle for WebView2 PID {}",
                            candidate.pid
                        )
                    })?,
                ));
            }
        }
        if backend.elapsed().saturating_sub(started) >= PROCESS_WAIT_LIMIT {
            return Err("WebView2 process quiescence exceeded 5 seconds".into());
        }
        if waiting.is_empty() {
            return Ok(());
        }
        for (pid, handle) in waiting {
            let remaining =
                PROCESS_WAIT_LIMIT.saturating_sub(backend.elapsed().saturating_sub(started));
            if remaining.is_zero() || !backend.wait(handle, remaining)? {
                return Err(format!(
                    "WebView2 PID {pid} did not exit within the shared 5 second deadline"
                ));
            }
            crate::logger::log(format!("[EffeTune migration] WebView2 PID {pid} exited"));
        }
        // Re-enumerate after exit: failed startup attempts can still be exiting.
        // The held lock prevents any new successful source-profile browser.
    }
}

fn present(path: &Path) -> Result<bool, String> {
    match fs::symlink_metadata(path) {
        Ok(_) => Ok(true),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(false),
        Err(e) => Err(io_error("inspect", path, e)),
    }
}

fn metadata_without_reparse(path: &Path) -> Result<fs::Metadata, String> {
    let metadata = fs::symlink_metadata(path).map_err(|e| io_error("inspect", path, e))?;
    #[cfg(windows)]
    let is_reparse = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let is_reparse = metadata.file_type().is_symlink();
    if is_reparse {
        return Err(format!(
            "migration does not follow reparse points: {}",
            path.display()
        ));
    }
    Ok(metadata)
}

fn require_directory(path: &Path) -> Result<(), String> {
    if metadata_without_reparse(path)?.is_dir() {
        Ok(())
    } else {
        Err(format!(
            "migration source is not a directory: {}",
            path.display()
        ))
    }
}

fn require_regular_file(path: &Path) -> Result<(), String> {
    if metadata_without_reparse(path)?.is_file() {
        Ok(())
    } else {
        Err(format!(
            "migration source is not a regular file: {}",
            path.display()
        ))
    }
}

fn ensure_separate_trees(source: &Path, destination: &Path) -> Result<(), String> {
    // Resolve existing ancestors to catch a test-source override nested in the
    // destination (or a destination nested in the profile being copied).
    fn resolved(path: &Path) -> Result<PathBuf, String> {
        let absolute =
            std::path::absolute(path).map_err(|e| io_error("resolve migration path", path, e))?;
        let mut ancestor = absolute.as_path();
        let mut missing = Vec::new();
        while !present(ancestor)? {
            let name = ancestor
                .file_name()
                .ok_or_else(|| format!("cannot resolve migration path: {}", path.display()))?;
            missing.push(name.to_os_string());
            ancestor = ancestor
                .parent()
                .ok_or_else(|| format!("cannot resolve migration path: {}", path.display()))?;
        }
        let mut result = fs::canonicalize(ancestor)
            .map_err(|e| io_error("resolve existing migration path", ancestor, e))?;
        for name in missing.into_iter().rev() {
            result.push(name);
        }
        Ok(result)
    }
    let source = resolved(source)?;
    let destination = resolved(destination)?;
    // canonicalize returns consistent Windows prefixes; components implement
    // the platform's prefix rules, while ordinary components need case folding.
    #[cfg(windows)]
    let (source, destination) = (
        PathBuf::from(source.to_string_lossy().to_lowercase()),
        PathBuf::from(destination.to_string_lossy().to_lowercase()),
    );
    if source.starts_with(&destination) || destination.starts_with(&source) {
        Err(format!(
            "migration source and destination overlap: {} / {}",
            source.display(),
            destination.display()
        ))
    } else {
        Ok(())
    }
}

fn create_owned_directory(path: &Path, owned: &mut Vec<PathBuf>) -> Result<(), String> {
    fs::create_dir(path).map_err(|e| io_error("create migration copy directory", path, e))?;
    owned.push(path.to_path_buf());
    Ok(())
}

fn copy_regular_file(source: &Path, destination: &Path) -> Result<(), String> {
    require_regular_file(source)?;
    let mut input = File::open(source).map_err(|e| io_error("open migration source", source, e))?;
    let mut output = OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(destination)
        .map_err(|e| io_error("create migration copy", destination, e))?;
    io::copy(&mut input, &mut output)
        .map_err(|e| io_error("copy migration file", destination, e))?;
    Ok(())
}

fn excluded_webview_path(relative: &Path) -> bool {
    // Exact paths only. In particular, do not exclude similarly named user data
    // elsewhere in the profile, or broaden the two CrashpadMetrics filenames.
    let normalized = relative
        .to_string_lossy()
        .replace('\\', "/")
        .to_ascii_lowercase();
    matches!(
        normalized.as_str(),
        "ebwebview/lockfile"
            | "ebwebview/crashpad"
            | "ebwebview/crashpadmetrics.pma"
            | "ebwebview/crashpadmetrics-active.pma"
            | "ebwebview/browsermetrics"
            | "ebwebview/deferredbrowsermetrics"
            | "ebwebview/ebwebviewmetrics"
    )
}

fn copy_webview_tree(source: &Path, destination: &Path, relative: &Path) -> Result<(), String> {
    for entry in fs::read_dir(source).map_err(|e| io_error("list migration source", source, e))? {
        let entry = entry.map_err(|e| io_error("read migration directory entry", source, e))?;
        let relative = relative.join(entry.file_name());
        if excluded_webview_path(&relative) {
            continue;
        }
        let source = entry.path();
        let target = destination.join(entry.file_name());
        let metadata = metadata_without_reparse(&source)?;
        if metadata.is_dir() {
            fs::create_dir(&target).map_err(|e| io_error("create copied directory", &target, e))?;
            copy_webview_tree(&source, &target, &relative)?;
        } else if metadata.is_file() {
            copy_regular_file(&source, &target)?;
        } else {
            return Err(format!(
                "unsupported migration source entry: {}",
                source.display()
            ));
        }
    }
    Ok(())
}

fn remove_owned_tree(path: &Path) -> io::Result<()> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    let is_reparse = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let is_reparse = metadata.file_type().is_symlink();
    if metadata.is_dir() && !is_reparse {
        for entry in fs::read_dir(path)? {
            remove_owned_tree(&entry?.path())?;
        }
        fs::remove_dir(path)
    } else if metadata.is_dir() {
        fs::remove_dir(path)
    } else {
        fs::remove_file(path)
    }
}

fn write_mark(destination: &Path, name: &str) -> Result<(), String> {
    fs::create_dir_all(destination)
        .map_err(|e| io_error("create migration decision directory", destination, e))?;
    let path = destination.join(name);
    // The name itself is the decision; no serialized schema or DB changes.
    OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&path)
        .map_err(|e| io_error("publish migration decision", &path, e))?;
    Ok(())
}

fn io_error(stage: &str, path: &Path, error: io::Error) -> String {
    format!("{stage} {}: {error}", path.display())
}

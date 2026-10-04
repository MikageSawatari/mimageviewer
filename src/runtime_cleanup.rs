//! Post-startup, best-effort collection of launcher-owned immutable runtimes.
//! No UI I/O, joins, timers or in-process retries. Unknown layouts are retained.
//! Parent-directory TOCTOU swaps by another process running as the same user are
//! accepted (2026-10-05): that process can delete the user's files directly.
//! Canonical confinement/reparse pre-checks remain; they are not a handle-pinned
//! guarantee against a malicious concurrent replacement of a parent directory.
//! Legacy owners do not take leases: a single process-image snapshot on this
//! worker retains their trees (and all generations if another same-version core
//! runs). Starting an unchanged old launcher after the snapshot remains an
//! accepted race: it requires concurrent user launches, and re-extracts next time.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

#[cfg(all(test, not(feature = "portable")))]
use fs4::fs_std::FileExt;

use crate::effetune::bundle_location;
use crate::runtime_locks;

#[cfg(all(windows, not(feature = "portable")))]
pub(crate) fn pin_running_version() -> io::Result<Option<fs::File>> {
    let exe = std::env::current_exe()?;
    pin_version_at(&exe, env!("CARGO_PKG_VERSION"))
}

#[cfg(windows)]
fn pin_version_at(exe: &Path, running_version: &str) -> io::Result<Option<fs::File>> {
    let Some(version) = exe.parent().filter(|version| {
        version
            .file_name()
            .and_then(|name| name.to_str())
            .is_some_and(|name| name.eq_ignore_ascii_case(running_version))
            && version
                .parent()
                .and_then(Path::file_name)
                .and_then(|name| name.to_str())
                .is_some_and(|name| name.eq_ignore_ascii_case("runtime"))
    }) else {
        return Ok(None);
    };
    // Lease identity follows the executable's real directory. Redirected APPDATA
    // remains runnable; cleanup's stricter ancestor refusal only skips deletion.
    runtime_locks::shared(&fs::canonicalize(version)?).map(Some)
}

#[cfg(all(windows, not(feature = "portable"), not(test)))]
pub(crate) fn spawn(data_dir: PathBuf, pinned_bundle: Option<PathBuf>) {
    if let Err(error) = std::thread::Builder::new()
        .name("runtime-cleanup".into())
        .spawn(move || {
            cleanup(
                &data_dir,
                env!("CARGO_PKG_VERSION"),
                pinned_bundle.as_deref(),
            )
        })
    {
        crate::logger::log(format!("runtime cleanup: could not start worker: {error}"));
    }
}

fn cleanup(data_dir: &Path, version: &str, pinned_bundle: Option<&Path>) {
    cleanup_with_processes(data_dir, version, pinned_bundle, process_images);
}

fn cleanup_with_processes(
    data_dir: &Path,
    version: &str,
    pinned_bundle: Option<&Path>,
    processes: impl FnOnce() -> io::Result<Vec<(u32, PathBuf)>>,
) {
    // Portable uses loose dependencies, even when --data-dir points at a normal profile.
    if cfg!(feature = "portable") {
        return;
    }
    let result = (|| -> io::Result<()> {
        let current = semver::Version::parse(version).map_err(io::Error::other)?;
        // Check ancestors too: canonicalization must not first traverse a junction.
        checked_ancestors(data_dir)?;
        let data_dir = fs::canonicalize(data_dir)?;
        let runtime = checked_child(&data_dir, &data_dir.join("runtime"))?;
        // One snapshot per startup, never per candidate. Failure/truncation keeps
        // everything this time; ordinary protected/exited processes are skipped
        // by the native provider. Canonicalize image identities once, off the UI.
        let processes: Vec<_> = processes()?
            .into_iter()
            .map(|(pid, image)| (pid, fs::canonicalize(&image).unwrap_or(image)))
            .collect();
        for entry in fs::read_dir(&runtime)? {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    log_failure(&runtime, &error);
                    continue;
                }
            };
            let name = entry.file_name();
            let Some(name) = name.to_str() else { continue };
            let Ok(candidate) = semver::Version::parse(name) else {
                continue;
            };
            if candidate != current {
                remove_candidate(&runtime, &runtime, &entry.path(), &processes);
            }
        }
        if let Some(bundle) = pinned_bundle {
            if let Err(error) = cleanup_effetune(&runtime, version, bundle, &processes) {
                log_failure(&runtime.join(version).join("effetune"), &error);
            }
        }
        // None means preparation/resolve is unavailable, or a retry may be in flight.
        // Do not guess the worker's generation from a possibly changing pointer.
        Ok(())
    })();
    if let Err(error) = result {
        if error.kind() != io::ErrorKind::NotFound {
            log_failure(&data_dir.join("runtime"), &error);
        }
    }
}

fn cleanup_effetune(
    runtime: &Path,
    version: &str,
    bundle: &Path,
    processes: &[(u32, PathBuf)],
) -> io::Result<()> {
    let current = checked_child(runtime, &runtime.join(version))?;
    // A legacy core's executable is in the version, not its pinned generation.
    // Its plugin may not be loaded yet, so image/module checks of the generation
    // alone cannot prove it idle. Conservatively retain every generation while
    // another core uses this version; helper children do not trigger this gate.
    if processes.iter().any(|(pid, image)| {
        *pid != std::process::id()
            && image_under(image, &current)
            && image.file_name().is_some_and(|name| {
                name.to_string_lossy()
                    .eq_ignore_ascii_case("mimageviewer-core.exe")
            })
    }) {
        return Err(io::Error::other("another core uses this runtime version"));
    }
    let container = checked_child(&current, &current.join("effetune"))?;
    // Only the controller's recognized, pinned generation belongs to this sweep.
    // A development loose bundle (or a different data-dir's bundle) is not evidence.
    checked_ancestors(bundle)?;
    let bundle = fs::canonicalize(bundle)?;
    let active = bundle
        .parent()
        .ok_or_else(|| io::Error::other("no bundle parent"))?;
    let active_name = active.file_name().and_then(|name| name.to_str());
    if active.parent() != Some(container.as_path())
        || !active_name.is_some_and(bundle_location::valid_generation)
        || bundle
            .file_name()
            .is_none_or(|name| name != "EffeTune Mixwright.vst3")
    {
        return Err(io::Error::other(
            "unrecognized pinned EffeTune layout; keeping generations",
        ));
    }

    let publisher = runtime_locks::exclusive(&current, runtime_locks::PUBLISHER)?;
    let candidates = generation_candidates(&container, active_name.unwrap())?;
    for candidate in candidates {
        remove_candidate(runtime, &container, &candidate, processes);
    }
    drop(publisher);
    Ok(())
}

fn generation_candidates(container: &Path, active_name: &str) -> io::Result<Vec<PathBuf>> {
    // Caller holds the version's publisher lock through candidate removal.
    let pointer = bundle_location::read_pointer(container)?;
    // A malformed/dangling/reparse current is insufficient evidence for deletion.
    let pointer_generation = checked_child(container, &container.join(&pointer))?;
    // Use the resolved identity, not the pointer's spelling: Windows permits
    // case variants of the nonce to name the same current directory.
    let pointer_name = pointer_generation.file_name().unwrap();
    let mut candidates = Vec::new();
    for entry in fs::read_dir(container)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if bundle_location::valid_generation(name)
            && name != active_name
            && entry.file_name() != pointer_name
        {
            candidates.push(entry.path());
        }
    }
    Ok(candidates)
}

fn checked_metadata(path: &Path) -> io::Result<fs::Metadata> {
    let metadata = fs::symlink_metadata(path)?;
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0 // FILE_ATTRIBUTE_REPARSE_POINT
    };
    #[cfg(not(windows))]
    let reparse = metadata.file_type().is_symlink();
    if reparse {
        return Err(io::Error::other(
            "reparse point; leaving runtime tree untouched",
        ));
    }
    Ok(metadata)
}

fn checked_ancestors(path: &Path) -> io::Result<()> {
    let absolute = std::path::absolute(path)?;
    let mut ancestors: Vec<_> = absolute.ancestors().collect();
    ancestors.reverse();
    for ancestor in ancestors {
        bundle_location::checked_directory(ancestor)?;
    }
    Ok(())
}

/// Canonical direct-child check, used before enumeration and every deletion.
fn checked_child(parent: &Path, path: &Path) -> io::Result<PathBuf> {
    bundle_location::checked_directory(path)?;
    let canonical = fs::canonicalize(path)?;
    if canonical.parent() != Some(parent) || !canonical.starts_with(parent) {
        return Err(io::Error::other(
            "runtime target escaped its owning directory",
        ));
    }
    Ok(canonical)
}

fn checked_tree(path: &Path) -> io::Result<()> {
    let metadata = checked_metadata(path)?;
    if metadata.is_dir() {
        for entry in fs::read_dir(path)? {
            checked_tree(&entry?.path())?;
        }
    } else if !metadata.is_file() {
        return Err(io::Error::other("non-regular runtime entry"));
    }
    Ok(())
}

fn remove_candidate(runtime: &Path, parent: &Path, path: &Path, processes: &[(u32, PathBuf)]) {
    let result = (|| -> io::Result<()> {
        let target = checked_child(parent, path)?;
        if !target.starts_with(runtime) || target == runtime {
            return Err(io::Error::other("deletion target outside runtime"));
        }
        if processes
            .iter()
            .any(|(_, image)| image_under(image, &target))
        {
            return Err(io::Error::other("a running process image uses this tree"));
        }
        // Skip the whole candidate if ANY nested entry is a reparse point. On
        // Windows std::fs::remove_dir_all also opens entries with
        // FILE_FLAG_OPEN_REPARSE_POINT and deletes relative to directory handles,
        // providing protection against link replacement during recursive removal.
        checked_tree(&target)?;
        // Acquire every owner before touching any asset. All acquisitions are
        // nonblocking, and handles stay alive through remove_dir_all.
        let lease = runtime_locks::exclusive(&target, runtime_locks::IN_USE)?;
        let version_locks = if parent == runtime {
            Some((
                runtime_locks::exclusive(&target, runtime_locks::EXTRACTION)?,
                runtime_locks::exclusive(&target, runtime_locks::PUBLISHER)?,
            ))
        } else {
            None
        };
        checked_child(parent, &target)?;
        fs::remove_dir_all(&target)?;
        drop(version_locks);
        drop(lease);
        crate::logger::log(format!("runtime cleanup: removed {}", target.display()));
        Ok(())
    })();
    if let Err(error) = result {
        log_failure(path, &error);
    }
}

fn image_under(image: &Path, tree: &Path) -> bool {
    // Canonical paths normally already have matching spelling. The fallback
    // handles disappeared images and Windows case/extended-prefix aliases.
    #[cfg(windows)]
    {
        fn normalized(path: &Path) -> PathBuf {
            let text = path.to_string_lossy().to_uppercase();
            PathBuf::from(text.strip_prefix(r"\\?\").unwrap_or(&text))
        }
        normalized(image).starts_with(normalized(tree))
    }
    #[cfg(not(windows))]
    image.starts_with(tree)
}

#[cfg(all(windows, not(feature = "portable")))]
fn native_process_images() -> io::Result<Vec<(u32, PathBuf)>> {
    use std::os::windows::ffi::OsStringExt;
    use windows_sys::Win32::Foundation::CloseHandle;
    use windows_sys::Win32::System::ProcessStatus::K32EnumProcesses;
    use windows_sys::Win32::System::Threading::{
        OpenProcess, PROCESS_QUERY_LIMITED_INFORMATION, QueryFullProcessImageNameW,
    };

    // Bounded storage, one enumeration, one limited-rights image query per PID.
    // No module enumeration, VM reads, subprocesses, waits or retry loops.
    let mut pids = vec![0u32; 16384];
    let capacity = (pids.len() * std::mem::size_of::<u32>()) as u32;
    let mut bytes = 0;
    if unsafe { K32EnumProcesses(pids.as_mut_ptr(), capacity, &mut bytes) } == 0 {
        return Err(io::Error::last_os_error());
    }
    if bytes >= capacity {
        return Err(io::Error::other(
            "process snapshot truncated; keeping runtimes",
        ));
    }
    let mut buffer = vec![0u16; 32768];
    let mut images = Vec::new();
    for pid in pids
        .into_iter()
        .take(bytes as usize / std::mem::size_of::<u32>())
    {
        let handle = unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) };
        if handle.is_null() {
            continue; // Protected system process, or already exited.
        }
        let mut size = buffer.len() as u32;
        let found =
            unsafe { QueryFullProcessImageNameW(handle, 0, buffer.as_mut_ptr(), &mut size) };
        unsafe { CloseHandle(handle) };
        if found != 0 {
            images.push((
                pid,
                std::ffi::OsString::from_wide(&buffer[..size as usize]).into(),
            ));
        }
    }
    Ok(images)
}

fn process_images() -> io::Result<Vec<(u32, PathBuf)>> {
    #[cfg(all(windows, not(feature = "portable"), not(test)))]
    return native_process_images();
    #[cfg(any(not(windows), feature = "portable", test))]
    Ok(Vec::new()) // Unit fixtures inject snapshots; portable never calls this.
}

fn log_failure(path: &Path, error: &io::Error) {
    crate::logger::log(format!(
        "runtime cleanup: keeping {} for a later startup: {error}",
        path.display()
    ));
}

#[cfg(test)]
mod tests {
    use super::*;

    const VERSION: &str = "4.3.0";
    const ACTIVE: &str = "aaaaaaaaaaaa-Abc123";
    const POINTER: &str = "bbbbbbbbbbbb-Def456";
    const OLD: &str = "cccccccccccc-Ghi789";

    fn tree(path: &Path) {
        fs::create_dir_all(path.join("nested")).unwrap();
        fs::write(path.join("nested/asset"), b"asset").unwrap();
    }

    fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let runtime = temp.path().join("runtime");
        let current = runtime.join(VERSION);
        tree(&current);
        let container = current.join("effetune");
        for generation in [ACTIVE, POINTER, OLD] {
            tree(&container.join(generation).join("EffeTune Mixwright.vst3"));
        }
        fs::write(current.join(".effetune.lock"), b"").unwrap();
        fs::write(
            container.join("current"),
            bundle_location::encode_pointer(POINTER).unwrap(),
        )
        .unwrap();
        let bundle = container.join(ACTIVE).join("EffeTune Mixwright.vst3");
        (temp, runtime, bundle)
    }

    #[cfg(feature = "portable")]
    #[test]
    fn portable_cleanup_is_a_no_op() {
        let (temp, runtime, bundle) = fixture();
        tree(&runtime.join("3.0.0"));
        cleanup(temp.path(), VERSION, Some(&bundle));
        assert!(runtime.join("3.0.0/nested/asset").exists());
        assert!(runtime.join(VERSION).join("effetune").join(OLD).exists());
    }

    #[cfg(not(feature = "portable"))]
    mod normal {
        use super::*;

        #[test]
        fn keeps_current_and_both_pinned_and_pointer_generations() {
            let (temp, runtime, bundle) = fixture();
            for name in ["2.13.0", "4.2.0", "4.3.0-rc.1", "5.0.0"] {
                tree(&runtime.join(name));
            }
            for name in ["notes", "4.3", "v4.2.0", "4.2.0.backup"] {
                tree(&runtime.join(name));
            }
            fs::write(runtime.join("1.0.0"), b"ordinary file").unwrap();
            let container = runtime.join(VERSION).join("effetune");
            tree(&container.join("unknown"));
            tree(&container.join("aaaaaaaaaaaa-bad"));
            cleanup(temp.path(), VERSION, Some(&bundle));
            assert!(runtime.join(VERSION).join("nested/asset").exists());
            for name in ["2.13.0", "4.2.0", "4.3.0-rc.1", "5.0.0"] {
                assert!(!runtime.join(name).exists(), "{name}");
            }
            for name in ["notes", "4.3", "v4.2.0", "4.2.0.backup", "1.0.0"] {
                assert!(runtime.join(name).exists(), "{name}");
            }
            for name in [ACTIVE, POINTER, "unknown", "aaaaaaaaaaaa-bad", "current"] {
                assert!(container.join(name).exists(), "{name}");
            }
            assert!(!container.join(OLD).exists());
        }

        #[test]
        fn unresolved_or_unrecognized_reader_keeps_generations() {
            let (temp, runtime, bundle) = fixture();
            tree(&runtime.join("3.0.0"));
            cleanup(temp.path(), VERSION, None);
            assert!(!runtime.join("3.0.0").exists());
            let loose = temp.path().join("dev/effetune/EffeTune Mixwright.vst3");
            tree(&loose);
            cleanup(temp.path(), VERSION, Some(&loose));
            assert!(bundle.exists());
            assert!(runtime.join(VERSION).join("effetune").join(OLD).exists());
        }

        #[test]
        fn invalid_or_missing_current_pointer_keeps_generations() {
            let (temp, runtime, bundle) = fixture();
            let container = runtime.join(VERSION).join("effetune");
            for pointer in ["../outside", "effetune-v2\ndddddddddddd-Jkl012\n"] {
                fs::write(container.join("current"), pointer).unwrap();
                cleanup(temp.path(), VERSION, Some(&bundle));
                assert!(container.join(OLD).exists());
            }
            fs::remove_file(container.join("current")).unwrap();
            cleanup(temp.path(), VERSION, Some(&bundle));
            assert!(container.join(OLD).exists());
        }

        #[cfg(windows)]
        #[test]
        fn preserves_canonical_reader_and_pointer_with_case_variant_nonces() {
            let (temp, runtime, bundle) = fixture();
            let container = runtime.join(VERSION).join("effetune");
            fs::write(
                container.join("current"),
                bundle_location::encode_pointer("bbbbbbbbbbbb-DEF456").unwrap(),
            )
            .unwrap();
            let reader_alias = container
                .join("aaaaaaaaaaaa-ABC123")
                .join("EffeTune Mixwright.vst3");
            cleanup(temp.path(), VERSION, Some(&reader_alias));
            assert!(bundle.exists());
            assert!(container.join(POINTER).exists());
            assert!(!container.join(OLD).exists());
        }

        #[test]
        fn publisher_busy_skips_generations_and_next_startup_collects() {
            let (temp, runtime, bundle) = fixture();
            let lock = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(runtime.join(VERSION).join(".effetune.lock"))
                .unwrap();
            assert!(lock.try_lock_exclusive().unwrap());
            tree(&runtime.join("3.0.0"));
            cleanup(temp.path(), VERSION, Some(&bundle));
            assert!(!runtime.join("3.0.0").exists());
            let old = runtime.join(VERSION).join("effetune").join(OLD);
            assert!(old.exists());
            drop(lock);
            cleanup(temp.path(), VERSION, Some(&bundle));
            assert!(!old.exists());
        }

        #[test]
        fn publisher_stays_locked_through_generation_deletion() {
            let (temp, runtime, bundle) = fixture();
            let current = fs::canonicalize(runtime.join(VERSION)).unwrap();
            let container = current.join("effetune");
            let publisher = runtime_locks::exclusive(&current, runtime_locks::PUBLISHER).unwrap();
            let candidates = generation_candidates(&container, ACTIVE).unwrap();
            let runtime = fs::canonicalize(runtime).unwrap();
            for candidate in candidates {
                assert!(runtime_locks::exclusive(&current, runtime_locks::PUBLISHER).is_err());
                remove_candidate(&runtime, &container, &candidate, &[]);
                assert!(runtime_locks::exclusive(&current, runtime_locks::PUBLISHER).is_err());
            }
            drop(publisher);
            assert!(runtime_locks::exclusive(&current, runtime_locks::PUBLISHER).is_ok());
            assert!(!container.join(OLD).exists());
            assert!(container.join(POINTER).exists());
            assert!(bundle.exists());
            // All I/O remained inside the fixture; no process-global data-dir.
            assert!(temp.path().exists());
        }

        #[test]
        fn shared_version_and_generation_leases_keep_whole_trees() {
            let (temp, runtime, bundle) = fixture();
            let old = runtime.join("3.0.0");
            tree(&old);
            let generation = runtime.join(VERSION).join("effetune").join(OLD);
            let version_reader = runtime_locks::shared(&old).unwrap();
            let generation_reader = runtime_locks::shared(&generation).unwrap();
            cleanup(temp.path(), VERSION, Some(&bundle));
            assert_eq!(fs::read(old.join("nested/asset")).unwrap(), b"asset");
            assert_eq!(
                fs::read(generation.join("EffeTune Mixwright.vst3/nested/asset")).unwrap(),
                b"asset"
            );
            drop((version_reader, generation_reader));
            cleanup(temp.path(), VERSION, Some(&bundle));
            assert!(!old.exists());
            assert!(!generation.exists());
        }

        #[test]
        fn other_version_publisher_and_extraction_locks_prevent_any_deletion() {
            let (temp, runtime, _) = fixture();
            let publisher_version = runtime.join("3.0.0");
            let extraction_version = runtime.join("2.0.0");
            tree(&publisher_version);
            tree(&extraction_version);
            let publisher =
                runtime_locks::exclusive(&publisher_version, runtime_locks::PUBLISHER).unwrap();
            let extraction =
                runtime_locks::exclusive(&extraction_version, runtime_locks::EXTRACTION).unwrap();
            cleanup(temp.path(), VERSION, None);
            for version in [&publisher_version, &extraction_version] {
                assert_eq!(fs::read(version.join("nested/asset")).unwrap(), b"asset");
            }
            drop((publisher, extraction));
            cleanup(temp.path(), VERSION, None);
            assert!(!publisher_version.exists());
            assert!(!extraction_version.exists());
        }

        #[test]
        fn old_versions_without_lock_files_are_collected() {
            let (temp, runtime, _) = fixture();
            let old = runtime.join("3.0.0");
            tree(&old);
            assert!(!old.join(runtime_locks::IN_USE).exists());
            cleanup(temp.path(), VERSION, None);
            assert!(!old.exists());
        }

        #[test]
        fn legacy_process_snapshot_keeps_in_use_version_and_generation() {
            let (temp, runtime, bundle) = fixture();
            let old = runtime.join("3.0.0");
            let idle = runtime.join("2.0.0");
            tree(&old);
            tree(&idle);
            let generation = runtime.join(VERSION).join("effetune").join(OLD);
            let images = vec![
                (101, old.join("mimageviewer-core.exe")),
                (102, generation.join("host.exe")),
            ];
            cleanup_with_processes(temp.path(), VERSION, Some(&bundle), || Ok(images));
            assert!(old.join("nested/asset").exists());
            assert!(
                generation
                    .join("EffeTune Mixwright.vst3/nested/asset")
                    .exists()
            );
            assert!(!old.join(runtime_locks::IN_USE).exists());
            assert!(!generation.join(runtime_locks::IN_USE).exists());
            assert!(!idle.exists());
            cleanup_with_processes(temp.path(), VERSION, Some(&bundle), || Ok(vec![]));
            assert!(!old.exists());
            assert!(!generation.exists());
        }

        #[test]
        fn another_core_keeps_legacy_generations_even_before_plugin_load() {
            let (temp, runtime, bundle) = fixture();
            let current = runtime.join(VERSION);
            let generation = current.join("effetune").join(OLD);
            let other_pid = std::process::id().wrapping_add(1);
            cleanup_with_processes(temp.path(), VERSION, Some(&bundle), || {
                Ok(vec![(other_pid, current.join("mimageviewer-core.exe"))])
            });
            assert!(
                generation
                    .join("EffeTune Mixwright.vst3/nested/asset")
                    .exists()
            );
            assert!(!generation.join(runtime_locks::IN_USE).exists());
            // Own core and ordinary helper children do not suppress collection.
            cleanup_with_processes(temp.path(), VERSION, Some(&bundle), || {
                Ok(vec![
                    (std::process::id(), current.join("mimageviewer-core.exe")),
                    (other_pid, current.join("mimageviewer-remote.exe")),
                ])
            });
            assert!(!generation.exists());
        }

        #[test]
        fn process_snapshot_failure_keeps_all_candidates_and_portion_names_do_not_match() {
            let (temp, runtime, bundle) = fixture();
            let old = runtime.join("3.0.0");
            tree(&old);
            cleanup_with_processes(temp.path(), VERSION, Some(&bundle), || {
                Err(io::Error::other("injected snapshot failure"))
            });
            assert!(old.join("nested/asset").exists());
            assert!(runtime.join(VERSION).join("effetune").join(OLD).exists());
            // Prefix text is insufficient: matching is by path components.
            cleanup_with_processes(temp.path(), VERSION, None, || {
                Ok(vec![(101, runtime.join("3.0.0-other/host.exe"))])
            });
            assert!(!old.exists());
        }

        #[cfg(windows)]
        #[test]
        fn native_process_snapshot_finds_self_and_measures_worker_cost() {
            let started = std::time::Instant::now();
            let images = native_process_images().unwrap();
            let own_image = images.iter().find(|(pid, _)| *pid == std::process::id());
            assert!(own_image.is_some());
            let canonical: Vec<_> = images
                .iter()
                .map(|(_, image)| fs::canonicalize(image).unwrap_or_else(|_| image.clone()))
                .collect();
            eprintln!(
                "process snapshot: {} readable images, enumeration + canonicalization {:?}",
                canonical.len(),
                started.elapsed()
            );
            let alias = PathBuf::from(r"C:\DATA\Runtime\3.0.0\CORE.EXE");
            assert!(image_under(&alias, Path::new(r"\\?\C:\data\runtime\3.0.0")));
        }

        #[cfg(windows)]
        #[test]
        fn version_lease_uses_executable_directory_with_windows_case_alias() {
            let (temp, runtime, _) = fixture();
            let old = runtime.join("3.0.0");
            tree(&old);
            let exe = temp
                .path()
                .join("RUNTIME")
                .join("3.0.0")
                .join("mimageviewer-core.exe");
            let reader = pin_version_at(&exe, "3.0.0").unwrap().unwrap();
            cleanup(temp.path(), VERSION, None);
            assert!(old.join("nested/asset").exists());
            drop(reader);
            cleanup(temp.path(), VERSION, None);
            assert!(!old.exists());
        }

        #[cfg(windows)]
        #[test]
        fn redirected_runtime_is_runnable_while_cleanup_stays_disabled() {
            let temp = tempfile::tempdir().unwrap();
            let outside = temp.path().join("outside");
            tree(&outside.join(VERSION));
            tree(&outside.join("3.0.0"));
            let data = temp.path().join("data");
            fs::create_dir(&data).unwrap();
            junction(&data.join("runtime"), &outside);
            let exe = data
                .join("runtime")
                .join(VERSION)
                .join("mimageviewer-core.exe");
            let reader = pin_version_at(&exe, VERSION).unwrap().unwrap();
            assert!(
                runtime_locks::exclusive(&outside.join(VERSION), runtime_locks::IN_USE).is_err()
            );
            cleanup(&data, VERSION, None);
            assert!(outside.join("3.0.0/nested/asset").exists());
            drop(reader);
            fs::remove_dir(data.join("runtime")).unwrap();
        }

        #[test]
        fn missing_runtime_does_not_create_anything() {
            let temp = tempfile::tempdir().unwrap();
            cleanup(temp.path(), VERSION, None);
            assert_eq!(fs::read_dir(temp.path()).unwrap().count(), 0);
        }

        #[test]
        fn refuses_targets_outside_the_canonical_runtime() {
            let (temp, runtime, _) = fixture();
            let outside = temp.path().join("3.0.0");
            tree(&outside);
            let canonical_runtime = fs::canonicalize(runtime).unwrap();
            remove_candidate(&canonical_runtime, &canonical_runtime, &outside, &[]);
            assert_eq!(fs::read(outside.join("nested/asset")).unwrap(), b"asset");
        }

        #[cfg(windows)]
        #[test]
        fn locked_file_does_not_stop_other_deletions_and_is_retried_next_startup() {
            use std::os::windows::fs::OpenOptionsExt;
            let (temp, runtime, bundle) = fixture();
            let old = runtime.join("3.0.0");
            tree(&old);
            tree(&runtime.join("2.0.0"));
            let locked_generation = runtime.join(VERSION).join("effetune").join(OLD);
            let version_lock = fs::OpenOptions::new()
                .read(true)
                .share_mode(1) // FILE_SHARE_READ, no FILE_SHARE_DELETE
                .open(old.join("nested/asset"))
                .unwrap();
            let generation_lock = fs::OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(locked_generation.join("EffeTune Mixwright.vst3/nested/asset"))
                .unwrap();
            cleanup(temp.path(), VERSION, Some(&bundle));
            assert!(old.exists());
            assert!(locked_generation.exists());
            assert!(!runtime.join("2.0.0").exists());
            assert!(bundle.exists());
            drop((version_lock, generation_lock));
            cleanup(temp.path(), VERSION, Some(&bundle));
            assert!(!old.exists());
            assert!(!locked_generation.exists());
        }

        #[cfg(windows)]
        fn junction(link: &Path, destination: &Path) {
            use std::os::windows::process::CommandExt;
            // TempDir paths only; creates a link, never performs shell deletion.
            let output = std::process::Command::new("cmd.exe")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(link)
                .arg(destination)
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
        }

        #[cfg(windows)]
        #[test]
        fn skips_direct_and_nested_junctions_in_versions_and_generations() {
            let (temp, runtime, bundle) = fixture();
            let outside = temp.path().join("outside");
            tree(&outside);
            let container = runtime.join(VERSION).join("effetune");
            let links = [
                runtime.join("2.0.0"),
                runtime.join("3.0.0").join("link"),
                container.join("dddddddddddd-Jkl012"),
                container.join(OLD).join("link"),
            ];
            tree(&runtime.join("3.0.0"));
            for link in &links {
                junction(link, &outside);
            }
            cleanup(temp.path(), VERSION, Some(&bundle));
            assert_eq!(fs::read(outside.join("nested/asset")).unwrap(), b"asset");
            for link in &links {
                assert!(fs::symlink_metadata(link).is_ok());
                fs::remove_dir(link).unwrap();
            }
            assert!(runtime.join("3.0.0/nested/asset").exists());
            assert!(container.join(OLD).exists());
        }

        #[cfg(windows)]
        #[test]
        fn skips_reparse_runtime_root_pointer_and_publisher_lock() {
            let temp = tempfile::tempdir().unwrap();
            let outside = temp.path().join("outside");
            tree(&outside.join("3.0.0"));
            let data = temp.path().join("data");
            fs::create_dir(&data).unwrap();
            junction(&data.join("runtime"), &outside);
            cleanup(&data, VERSION, None);
            assert!(outside.join("3.0.0/nested/asset").exists());
            fs::remove_dir(data.join("runtime")).unwrap();

            let (temp, runtime, bundle) = fixture();
            let container = runtime.join(VERSION).join("effetune");
            let pointer = container.join("current");
            fs::remove_file(&pointer).unwrap();
            junction(&pointer, &outside);
            cleanup(temp.path(), VERSION, Some(&bundle));
            assert!(container.join(OLD).exists());
            fs::remove_dir(pointer).unwrap();
            fs::write(
                container.join("current"),
                bundle_location::encode_pointer(POINTER).unwrap(),
            )
            .unwrap();
            let lock = runtime.join(VERSION).join(".effetune.lock");
            fs::remove_file(&lock).unwrap();
            junction(&lock, &outside);
            cleanup(temp.path(), VERSION, Some(&bundle));
            assert!(container.join(OLD).exists());
            fs::remove_dir(lock).unwrap();
        }
    }
}

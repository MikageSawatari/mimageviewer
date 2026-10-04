//! Post-startup, best-effort collection of launcher-owned immutable runtimes.
//! No UI I/O, joins, timers or in-process retries. Unknown layouts are retained.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use fs4::fs_std::FileExt;

use crate::effetune::bundle_location;

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
                remove_candidate(&runtime, &runtime, &entry.path());
            }
        }
        if let Some(bundle) = pinned_bundle {
            if let Err(error) = cleanup_effetune(&runtime, version, bundle) {
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

fn cleanup_effetune(runtime: &Path, version: &str, bundle: &Path) -> io::Result<()> {
    let current = checked_child(runtime, &runtime.join(version))?;
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

    let candidates = generation_candidates(&current, &container, active_name.unwrap())?;
    for candidate in candidates {
        remove_candidate(runtime, &container, &candidate);
    }
    Ok(())
}

fn generation_candidates(
    current: &Path,
    container: &Path,
    active_name: &str,
) -> io::Result<Vec<PathBuf>> {
    // Share the publisher's OS lock, but never wait. Capture the complete list
    // while no unpublished stage can be under construction. Drop the lock before
    // recursive validation/deletion, so repair never waits behind that work.
    let lock_path = current.join(".effetune.lock");
    let metadata = checked_metadata(&lock_path)?;
    if !metadata.is_file() {
        return Err(io::Error::other("invalid EffeTune publisher lock"));
    }
    let mut options = fs::OpenOptions::new();
    options.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        options.custom_flags(0x00200000); // FILE_FLAG_OPEN_REPARSE_POINT
    }
    let lock = options.open(lock_path)?;
    if !lock.try_lock_exclusive()? {
        return Err(io::Error::new(
            io::ErrorKind::WouldBlock,
            "EffeTune publisher busy",
        ));
    }
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
    // Publisher invariant: valid current is reused unchanged; repair ALWAYS
    // publishes a fresh unique directory, never an existing inactive generation.
    // Future stages/current are absent from this snapshot, and these candidates
    // cannot become current after unlocking. Republish/rollback would require a
    // new cleanup ownership design. File drop releases the publisher lock here.
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

fn remove_candidate(runtime: &Path, parent: &Path, path: &Path) {
    let result = (|| -> io::Result<()> {
        let target = checked_child(parent, path)?;
        if !target.starts_with(runtime) || target == runtime {
            return Err(io::Error::other("deletion target outside runtime"));
        }
        // Skip the whole candidate if ANY nested entry is a reparse point. On
        // Windows std::fs::remove_dir_all also opens entries with
        // FILE_FLAG_OPEN_REPARSE_POINT and deletes relative to directory handles,
        // providing protection against link replacement during recursive removal.
        checked_tree(&target)?;
        checked_child(parent, &target)?;
        fs::remove_dir_all(&target)?;
        crate::logger::log(format!("runtime cleanup: removed {}", target.display()));
        Ok(())
    })();
    if let Err(error) = result {
        log_failure(path, &error);
    }
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
        fn future_publication_is_outside_snapshot_and_lock_is_released_before_deletion() {
            let (temp, runtime, bundle) = fixture();
            let current = fs::canonicalize(runtime.join(VERSION)).unwrap();
            let container = current.join("effetune");
            let candidates = generation_candidates(&current, &container, ACTIVE).unwrap();
            let publisher = fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(current.join(".effetune.lock"))
                .unwrap();
            assert!(publisher.try_lock_exclusive().unwrap());
            let fresh = "dddddddddddd-Jkl012";
            tree(&container.join(fresh).join("EffeTune Mixwright.vst3"));
            fs::write(
                container.join("current"),
                bundle_location::encode_pointer(fresh).unwrap(),
            )
            .unwrap();
            drop(publisher);
            let runtime = fs::canonicalize(runtime).unwrap();
            for candidate in candidates {
                remove_candidate(&runtime, &container, &candidate);
            }
            assert!(!container.join(OLD).exists());
            assert!(container.join(POINTER).exists());
            assert!(container.join(fresh).exists());
            assert!(bundle.exists());
            // All I/O remained inside the fixture; no process-global data-dir.
            assert!(temp.path().exists());
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
            remove_candidate(&canonical_runtime, &canonical_runtime, &outside);
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

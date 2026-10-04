//! Write-once generations: only the small current pointer is replaced.
//! Published bundle trees are never moved or deleted during launcher startup.
use crate::bundle_location;
use crate::bundle_paths::{checked_metadata, exists_checked, inventory_metadata, relative_name};
use crate::runtime_locks::{self, PinnedGeneration};
use fs4::fs_std::FileExt;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::{Duration, UNIX_EPOCH};

pub struct BundleFile {
    pub name: &'static str,
    pub bytes: &'static [u8],
    pub hash: &'static str,
}

#[derive(Debug)]
pub struct PreparationError {
    pub reason: io::Error,
    pub rejected_generation: Option<String>,
}

impl std::fmt::Display for PreparationError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        self.reason.fmt(formatter)
    }
}

pub fn ensure_bundle(
    runtime: &Path,
    files: &[BundleFile],
    manifest: &str,
) -> Result<PathBuf, PreparationError> {
    ensure_bundle_with_wait(runtime, files, manifest, Duration::from_secs(60))
}

pub fn ensure_pinned_bundle(
    runtime: &Path,
    files: &[BundleFile],
    manifest: &str,
) -> Result<PinnedGeneration, PreparationError> {
    let mut rejected_generation = None;
    ensure_bundle_inner(
        runtime,
        files,
        manifest,
        Duration::from_secs(60),
        &mut rejected_generation,
    )
    .map_err(|reason| PreparationError {
        reason,
        rejected_generation,
    })
}

fn ensure_bundle_with_wait(
    runtime: &Path,
    files: &[BundleFile],
    manifest: &str,
    wait: Duration,
) -> Result<PathBuf, PreparationError> {
    let mut rejected_generation = None;
    ensure_bundle_inner(runtime, files, manifest, wait, &mut rejected_generation)
        .map(|pin| pin.path.clone())
        .map_err(|reason| PreparationError {
            reason,
            rejected_generation,
        })
}

fn ensure_bundle_inner(
    runtime: &Path,
    files: &[BundleFile],
    manifest: &str,
    wait: Duration,
    rejected_generation: &mut Option<String>,
) -> io::Result<PinnedGeneration> {
    if !checked_metadata(runtime)?.is_dir() {
        return Err(io::Error::other("runtime directory required"));
    }
    let container = runtime.join("effetune");
    if exists_checked(&container)? && !checked_metadata(&container)?.is_dir() {
        return Err(io::Error::other("EffeTune container must be a directory"));
    }
    // A valid cooperating installation needs only a shared read-only lease,
    // without a publisher write lock or another asset hash pass.
    if let Ok(root) = ready_generation(&container, files, manifest, rejected_generation) {
        return Ok(root);
    }
    std::fs::create_dir_all(&container)?;
    let lock_path = runtime.join(".effetune.lock");
    if exists_checked(&lock_path)? && !checked_metadata(&lock_path)?.is_file() {
        return Err(io::Error::other("bundle lock must be a regular file"));
    }
    let lock = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(lock_path)?;
    // Blocking OS locking runs on a worker, with a bounded launcher wait. A
    // timed-out worker only releases its lock: it never publishes anything.
    let _lock = match wait_for_publish_lock(lock, wait) {
        Ok(lock) => lock,
        Err(error) => {
            return ready_generation(&container, files, manifest, rejected_generation)
                .or(Err(error));
        }
    };
    if let Ok(root) = ready_generation(&container, files, manifest, rejected_generation) {
        return Ok(root);
    }
    publish_generation(&container, files, manifest)
}

pub(crate) fn wait_for_publish_lock(
    lock: std::fs::File,
    wait: Duration,
) -> io::Result<std::fs::File> {
    if lock.try_lock_exclusive()? {
        return Ok(lock);
    }
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::Builder::new()
        .name("effetune-publish-lock".into())
        .spawn(move || {
            let result = lock.lock_exclusive().map(|()| lock);
            let _ = tx.send(result);
        })?;
    rx.recv_timeout(wait).map_err(|error| {
        io::Error::new(
            io::ErrorKind::TimedOut,
            format!("EffeTune publisher wait failed after {wait:?}: {error}; retry 音響調整"),
        )
    })?
}

fn ready_generation(
    container: &Path,
    files: &[BundleFile],
    manifest: &str,
    rejected_generation: &mut Option<String>,
) -> io::Result<PinnedGeneration> {
    if !checked_metadata(container)?.is_dir() {
        return Err(io::Error::other("EffeTune directory required"));
    }
    *rejected_generation = None;
    let generation = bundle_location::read_pointer(container)?;
    *rejected_generation = Some(generation.clone());
    let root = container.join(generation);
    bundle_location::checked_directory(&root)?;
    let pin = PinnedGeneration::new(root.clone())?;
    let fingerprint = crate::hex_lower(&Sha256::digest(manifest.as_bytes()));
    if !root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with(&format!(
            "{}-",
            &fingerprint[..bundle_location::FINGERPRINT_LENGTH]
        ))
    {
        return Err(io::Error::other("EffeTune generation fingerprint mismatch"));
    }
    let expected = snapshot(&root, files, manifest)?;
    let stamp = root.join(".manifest");
    if !checked_metadata(&stamp)?.is_file() || std::fs::read_to_string(stamp)? != expected {
        return Err(io::Error::other("EffeTune extraction stamp mismatch"));
    }
    Ok(pin)
}

fn snapshot(root: &Path, files: &[BundleFile], manifest: &str) -> io::Result<String> {
    let entries = inventory_metadata(root)?;
    let mut expected = BTreeSet::new();
    for file in files {
        relative_name(Path::new(file.name))?;
        if !expected.insert((file.name.to_string(), false)) {
            return Err(io::Error::other("duplicate bundle file"));
        }
        let mut parent = Path::new(file.name).parent();
        while let Some(dir) = parent.filter(|p| !p.as_os_str().is_empty()) {
            expected.insert((relative_name(dir)?, true));
            parent = dir.parent();
        }
    }
    let actual: BTreeSet<_> = entries
        .iter()
        .filter(|(name, _, _)| name != ".manifest" && name != runtime_locks::IN_USE)
        .map(|(name, meta, _)| (name.clone(), meta.is_dir()))
        .collect();
    if actual != expected {
        return Err(io::Error::other("bundle inventory mismatch"));
    }
    if let Some((_, metadata, _)) = entries
        .iter()
        .find(|(name, _, _)| name == runtime_locks::IN_USE)
    {
        if !metadata.is_file() {
            return Err(io::Error::other("generation lease must be a regular file"));
        }
    }
    let metadata: BTreeMap<_, _> = entries
        .into_iter()
        .map(|(name, meta, _)| (name, meta))
        .collect();
    let mut stamp = format!("effetune-v2\n{manifest}\n");
    for file in files {
        let meta = &metadata[file.name];
        if !meta.is_file() || meta.len() != file.bytes.len() as u64 {
            return Err(io::Error::other("bundle size mismatch"));
        }
        let modified = meta
            .modified()?
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        let created = meta
            .created()?
            .duration_since(UNIX_EPOCH)
            .map_err(io::Error::other)?
            .as_nanos();
        stamp.push_str(&format!(
            "{}\t{}\t{modified}\t{created}\n",
            file.name,
            meta.len()
        ));
    }
    Ok(stamp)
}

fn publish_generation(
    container: &Path,
    files: &[BundleFile],
    manifest: &str,
) -> io::Result<PinnedGeneration> {
    let fingerprint = crate::hex_lower(&Sha256::digest(manifest.as_bytes()));
    let stage = tempfile::Builder::new()
        .prefix(&format!(
            "{}-",
            &fingerprint[..bundle_location::FINGERPRINT_LENGTH]
        ))
        .tempdir_in(container)?;
    check_publish_path_length(stage.path(), files)?;
    let pin = PinnedGeneration::new(stage.path().to_path_buf())?;
    for file in files {
        relative_name(Path::new(file.name))?;
        let path = stage.path().join(file.name);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let mut output = std::fs::File::create(&path)?;
        output.write_all(file.bytes)?;
        output.sync_all()?;
        drop(output);
        if crate::sha256_file_hex(&path)? != file.hash {
            return Err(io::Error::other(format!(
                "bundle hash mismatch: {}",
                file.name
            )));
        }
    }
    let stamp = snapshot(stage.path(), files, manifest)?;
    let mut output = std::fs::File::create(stage.path().join(".manifest"))?;
    output.write_all(stamp.as_bytes())?;
    output.sync_all()?;
    drop(output);
    // Only the private, unpublished stage is cleaned up on error. There is no
    // fallible operation after pointer commit; published generations are kept.
    let root = stage.path().to_path_buf();
    let mut pointer = tempfile::NamedTempFile::new_in(container)?;
    pointer.write_all(
        bundle_location::encode_pointer(root.file_name().unwrap().to_str().unwrap())?.as_bytes(),
    )?;
    pointer.as_file().sync_all()?;
    pointer
        .persist(container.join(bundle_location::POINTER_FILE))
        .map_err(|e| e.error)?;
    // Cleanup is outside startup: absence of a lock cannot prove assets are unused.
    let _ = stage.keep();
    Ok(pin)
}

fn check_publish_path_length(root: &Path, files: &[BundleFile]) -> io::Result<()> {
    let root = std::path::absolute(root)?;
    let mut paths = files
        .iter()
        .map(|file| root.join(file.name))
        .collect::<Vec<_>>();
    paths.push(root.join(".manifest"));
    let deepest = paths.into_iter().max_by_key(|path| path_units(path));
    if let Some(path) = deepest {
        let length = path_units(&path);
        if length >= 260 {
            return Err(io::Error::other(format!(
                "{}: {length} UTF-16 units (maximum 259): {}. Use a shorter APPDATA path.",
                bundle_location::PATH_TOO_LONG_MARKER,
                path.display()
            )));
        }
    }
    Ok(())
}

fn path_units(path: &Path) -> usize {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        path.as_os_str().encode_wide().count()
    }
    #[cfg(not(windows))]
    path.to_string_lossy().encode_utf16().count()
}

#[cfg(test)]
mod tests {
    use super::*;
    fn with_fixture(test: impl FnOnce(&Path, &[BundleFile])) {
        let temp = tempfile::tempdir().unwrap();
        let files = [BundleFile {
            name: "EffeTune Mixwright.vst3/Contents/test.txt",
            bytes: b"bundle",
            hash: Box::leak(crate::hex_lower(&Sha256::digest(b"bundle")).into_boxed_str()),
        }];
        test(temp.path(), &files);
    }
    #[test]
    fn extracts_and_reuses_complete_verified_tree() {
        with_fixture(|dir, files| {
            let root = ensure_bundle(dir, files, "current").unwrap();
            let stamp = std::fs::read(root.join(".manifest")).unwrap();
            assert_eq!(ensure_bundle(dir, files, "current").unwrap(), root);
            assert_eq!(std::fs::read(root.join(".manifest")).unwrap(), stamp);
            assert_eq!(std::fs::read(root.join(files[0].name)).unwrap(), b"bundle");
        });
    }
    #[test]
    fn repairs_into_new_generations_without_modifying_old_trees() {
        with_fixture(|dir, files| {
            let mut root = ensure_bundle(dir, files, "current").unwrap();
            for damage in 0..4 {
                match damage {
                    0 => std::fs::write(root.join(files[0].name), b"damage").unwrap(),
                    1 => std::fs::write(root.join("stale.txt"), b"extra").unwrap(),
                    2 => std::fs::remove_file(root.join(files[0].name)).unwrap(),
                    _ => std::fs::remove_file(root.join(".manifest")).unwrap(),
                }
                if damage == 0 {
                    std::fs::File::options()
                        .write(true)
                        .open(root.join(files[0].name))
                        .unwrap()
                        .set_modified(
                            std::time::SystemTime::now() + std::time::Duration::from_secs(2),
                        )
                        .unwrap();
                }
                let next = ensure_bundle(dir, files, "current").unwrap();
                assert_ne!(next, root);
                assert!(root.is_dir());
                assert_eq!(std::fs::read(next.join(files[0].name)).unwrap(), b"bundle");
                root = next;
            }
            assert_ne!(ensure_bundle(dir, files, "changed").unwrap(), root);
            assert!(root.is_dir());
        });
    }
    #[test]
    fn failed_publication_keeps_old_pointer_and_tree() {
        with_fixture(|dir, files| {
            let root = ensure_bundle(dir, files, "current").unwrap();
            let before = std::fs::read(dir.join("effetune/current")).unwrap();
            let invalid = [BundleFile {
                name: files[0].name,
                bytes: files[0].bytes,
                hash: "invalid",
            }];
            assert!(ensure_bundle(dir, &invalid, "changed").is_err());
            assert_eq!(std::fs::read(dir.join("effetune/current")).unwrap(), before);
            assert_eq!(std::fs::read(root.join(files[0].name)).unwrap(), b"bundle");
            assert_eq!(ensure_bundle(dir, files, "current").unwrap(), root);
        });
    }
    #[test]
    fn concurrent_publishers_never_expose_partial_trees() {
        with_fixture(|dir, files| {
            std::thread::scope(|scope| {
                let handles: Vec<_> = (0..6)
                    .map(|_| scope.spawn(|| ensure_bundle(dir, files, "same")))
                    .collect();
                let roots = handles
                    .into_iter()
                    .map(|h| h.join().unwrap().unwrap())
                    .collect::<Vec<_>>();
                assert!(roots.iter().all(|root| root == &roots[0]));
            });
            let root = ensure_bundle(dir, files, "same").unwrap();
            assert!(snapshot(&root, files, "same").is_ok());
            assert_eq!(
                inventory_metadata(&dir.join("effetune"))
                    .unwrap()
                    .iter()
                    .filter(
                        |(name, meta, _)| meta.is_dir() && bundle_location::valid_generation(name)
                    )
                    .count(),
                1
            );
        });
    }
    #[test]
    fn rejects_traversal_and_preserves_legacy_tree() {
        with_fixture(|dir, files| {
            let legacy = dir.join("effetune/EffeTune Mixwright.vst3");
            std::fs::create_dir_all(&legacy).unwrap();
            std::fs::write(legacy.join("keep"), b"old").unwrap();
            ensure_bundle(dir, files, "current").unwrap();
            assert_eq!(std::fs::read(legacy.join("keep")).unwrap(), b"old");
            let invalid = [BundleFile {
                name: "../escape",
                bytes: b"",
                hash: "invalid",
            }];
            assert!(ensure_bundle(dir, &invalid, "changed").is_err());
            assert!(!dir.join("effetune/escape").exists());
        });
    }
    #[cfg(windows)]
    #[test]
    fn in_use_tree_is_preserved_when_pointer_or_stamp_is_missing() {
        use std::os::windows::fs::OpenOptionsExt;
        with_fixture(|dir, files| {
            let root = ensure_bundle(dir, files, "current").unwrap();
            let _in_use = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(1)
                .open(root.join(files[0].name))
                .unwrap();
            std::fs::remove_file(root.join(".manifest")).unwrap();
            std::fs::remove_file(dir.join("effetune/current")).unwrap();
            let next = ensure_bundle(dir, files, "current").unwrap();
            assert_ne!(next, root);
            assert_eq!(std::fs::read(root.join(files[0].name)).unwrap(), b"bundle");
        });
    }
    #[cfg(windows)]
    #[test]
    fn valid_read_only_pointer_needs_no_write_lock() {
        use std::os::windows::fs::OpenOptionsExt;
        with_fixture(|dir, files| {
            let root = ensure_bundle(dir, files, "current").unwrap();
            let pointer = dir.join("effetune/current");
            let mut perms = std::fs::metadata(&pointer).unwrap().permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(&pointer, perms.clone()).unwrap();
            let _no_lock_access = std::fs::OpenOptions::new()
                .read(true)
                .share_mode(0)
                .open(dir.join(".effetune.lock"))
                .unwrap();
            assert_eq!(ensure_bundle(dir, files, "current").unwrap(), root);
            perms.set_readonly(false);
            std::fs::set_permissions(pointer, perms).unwrap();
        });
    }
    #[test]
    fn publisher_wait_is_bounded_and_later_publication_can_be_used() {
        with_fixture(|dir, files| {
            let lock = std::fs::File::create(dir.join(".effetune.lock")).unwrap();
            lock.lock_exclusive().unwrap();
            let error = ensure_bundle_with_wait(dir, files, "current", Duration::from_millis(20))
                .unwrap_err();
            assert_eq!(error.reason.kind(), io::ErrorKind::TimedOut);
            lock.unlock().unwrap();
            assert!(ensure_bundle(dir, files, "current").is_ok());
        });
    }

    #[test]
    fn checks_deepest_utf16_path_before_writing_bundle_files() {
        with_fixture(|dir, files| {
            let root = dir.join("a".repeat(260));
            let error = check_publish_path_length(&root, files).unwrap_err();
            assert!(
                error
                    .to_string()
                    .contains(bundle_location::PATH_TOO_LONG_MARKER)
            );
            assert!(!root.exists());
            let short = ensure_bundle(dir, files, "current").unwrap();
            assert_eq!(
                short.file_name().unwrap().len(),
                bundle_location::FINGERPRINT_LENGTH + 1 + 6
            );
            assert!(path_units(&short.join(files[0].name)) < 260);
        });
    }

    #[test]
    fn failure_identifies_the_generation_actually_rejected_after_pointer_change() {
        with_fixture(|dir, files| {
            let first = ensure_bundle(dir, files, "first").unwrap();
            let second = ensure_bundle(dir, files, "second").unwrap();
            std::fs::remove_file(second.join(".manifest")).unwrap();
            let invalid = [BundleFile {
                name: files[0].name,
                bytes: files[0].bytes,
                hash: "invalid",
            }];
            // Stale pre-capture must not survive the real integrity check.
            let mut rejected = Some(first.file_name().unwrap().to_string_lossy().into_owned());
            assert!(
                ensure_bundle_inner(
                    dir,
                    &invalid,
                    "second",
                    Duration::from_secs(1),
                    &mut rejected
                )
                .is_err()
            );
            assert_eq!(rejected.as_deref(), second.file_name().unwrap().to_str());
            let error = ensure_bundle(dir, &invalid, "second").unwrap_err();
            assert_eq!(error.rejected_generation, rejected);
        });
    }
}

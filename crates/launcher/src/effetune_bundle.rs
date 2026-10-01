//! Write-once generations: only the small current pointer is replaced.
//! Published bundle trees are never moved or deleted during launcher startup.
use crate::bundle_location;
use crate::bundle_paths::{checked_metadata, exists_checked, inventory_metadata, relative_name};
use fs4::fs_std::FileExt;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, BTreeSet};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

pub struct BundleFile {
    pub name: &'static str,
    pub bytes: &'static [u8],
    pub hash: &'static str,
}

pub fn ensure_bundle(runtime: &Path, files: &[BundleFile], manifest: &str) -> io::Result<PathBuf> {
    if !checked_metadata(runtime)?.is_dir() {
        return Err(io::Error::other("runtime directory required"));
    }
    let container = runtime.join("effetune");
    if exists_checked(&container)? && !checked_metadata(&container)?.is_dir() {
        return Err(io::Error::other("EffeTune container must be a directory"));
    }
    // A valid installation has no write/lock requirement, including read-only APPDATA.
    if let Ok(root) = ready_generation(&container, files, manifest) {
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
    // A busy/unavailable publisher must not hold up application startup.
    if !lock.try_lock_exclusive()? {
        return ready_generation(&container, files, manifest).map_err(|_| {
            io::Error::new(
                io::ErrorKind::WouldBlock,
                "EffeTune publisher is busy; retry on the next launch",
            )
        });
    }
    if let Ok(root) = ready_generation(&container, files, manifest) {
        return Ok(root);
    }
    publish_generation(&container, files, manifest)
}

fn ready_generation(container: &Path, files: &[BundleFile], manifest: &str) -> io::Result<PathBuf> {
    if !checked_metadata(container)?.is_dir() {
        return Err(io::Error::other("EffeTune directory required"));
    }
    let root = bundle_location::read_generation(container)?;
    let fingerprint = crate::hex_lower(&Sha256::digest(manifest.as_bytes()));
    if !root
        .file_name()
        .unwrap()
        .to_string_lossy()
        .starts_with(&format!("{fingerprint}-"))
    {
        return Err(io::Error::other("EffeTune generation fingerprint mismatch"));
    }
    let expected = snapshot(&root, files, manifest)?;
    let stamp = root.join(".manifest");
    if !checked_metadata(&stamp)?.is_file() || std::fs::read_to_string(stamp)? != expected {
        return Err(io::Error::other("EffeTune extraction stamp mismatch"));
    }
    Ok(root)
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
        .filter(|(name, _, _)| name != ".manifest")
        .map(|(name, meta, _)| (name.clone(), meta.is_dir()))
        .collect();
    if actual != expected {
        return Err(io::Error::other("bundle inventory mismatch"));
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
) -> io::Result<PathBuf> {
    let fingerprint = crate::hex_lower(&Sha256::digest(manifest.as_bytes()));
    let stage = tempfile::Builder::new()
        .prefix(&format!("{fingerprint}-"))
        .tempdir_in(container)?;
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
    Ok(stage.keep())
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
                assert!(
                    handles
                        .into_iter()
                        .filter_map(|h| h.join().unwrap().ok())
                        .count()
                        >= 1
                );
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
    fn busy_publisher_returns_without_blocking_startup() {
        with_fixture(|dir, files| {
            let lock = std::fs::File::create(dir.join(".effetune.lock")).unwrap();
            lock.lock_exclusive().unwrap();
            assert!(ensure_bundle(dir, files, "current").is_err());
        });
    }
}

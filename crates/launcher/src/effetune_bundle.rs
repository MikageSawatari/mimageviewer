//! Whole-tree publication under an OS lock. Fast launches compare the exact inventory
//! and file metadata with the verified extraction stamp, without rehashing 38 MB.
use std::collections::BTreeSet;
use std::io::{self, Write};
use std::path::Path;
use std::time::UNIX_EPOCH;

use crate::bundle_paths::{
    checked_metadata, exists_checked, inventory, relative_name, remove_owned,
};
use fs4::fs_std::FileExt;

pub struct BundleFile {
    pub name: &'static str,
    pub bytes: &'static [u8],
    pub hash: &'static str,
}

pub fn ensure_bundle(runtime: &Path, files: &[BundleFile], manifest: &str) -> io::Result<()> {
    checked_metadata(runtime)?;
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
    lock.lock_exclusive()
        .map_err(|e| io::Error::new(e.kind(), format!("bundle lock: {e}")))?;
    // Lock release is automatic on close/process termination, including failed extraction.
    ensure_locked(runtime, files, manifest)
}

fn snapshot(root: &Path, files: &[BundleFile], manifest: &str) -> io::Result<String> {
    let entries = inventory(root)?;
    let mut expected = BTreeSet::new();
    for file in files {
        relative_name(Path::new(file.name))?;
        expected.insert((file.name.to_string(), false));
        let mut parent = Path::new(file.name).parent();
        while let Some(dir) = parent.filter(|p| !p.as_os_str().is_empty()) {
            expected.insert((relative_name(dir)?, true));
            parent = dir.parent();
        }
    }
    let actual: BTreeSet<_> = entries
        .iter()
        .filter(|(name, _, _)| name != ".manifest")
        .map(|(name, dir, _)| (name.clone(), *dir))
        .collect();
    if actual != expected {
        return Err(io::Error::other("bundle inventory mismatch"));
    }
    let mut stamp = format!("effetune-v1\n{manifest}\n");
    for file in files {
        let meta = checked_metadata(&root.join(file.name))?;
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

fn ensure_locked(runtime: &Path, files: &[BundleFile], manifest: &str) -> io::Result<()> {
    let root = runtime.join("effetune");
    let stage = runtime.join(".effetune-stage");
    let old = runtime.join(".effetune-old");
    if exists_checked(&root)? && checked_metadata(&root)?.is_dir() {
        // Reparse errors are fatal; do not overwrite or recurse through them.
        inventory(&root)?;
        let stamp = root.join(".manifest");
        exists_checked(&stamp)?;
        if let Ok(current) = snapshot(&root, files, manifest)
            && std::fs::read_to_string(&stamp).is_ok_and(|stored| stored == current)
        {
            remove_owned(&stage)?;
            remove_owned(&old)?;
            return Ok(());
        }
    }
    remove_owned(&stage)?;
    remove_owned(&old)?;
    std::fs::create_dir(&stage)?;
    for file in files {
        relative_name(Path::new(file.name))?;
        let path = stage.join(file.name);
        std::fs::create_dir_all(path.parent().unwrap())?;
        let mut output = std::fs::File::create(&path)?;
        output.write_all(file.bytes)?;
        output.sync_all()?;
        if crate::sha256_file_hex(&path)? != file.hash {
            return Err(io::Error::other(format!(
                "bundle hash mismatch: {}",
                file.name
            )));
        }
    }
    let stamp = snapshot(&stage, files, manifest)?;
    let mut output = std::fs::File::create(stage.join(".manifest"))?;
    output.write_all(stamp.as_bytes())?;
    output.sync_all()?;
    // Windows directory publication must not retain an open descendant handle.
    drop(output);
    if exists_checked(&root)? {
        std::fs::rename(&root, &old)?;
    }
    if let Err(error) = std::fs::rename(&stage, &root) {
        if exists_checked(&old)? {
            let _ = std::fs::rename(&old, &root);
        }
        return Err(io::Error::new(
            error.kind(),
            format!("publish bundle tree: {error}"),
        ));
    }
    remove_owned(&old)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use sha2::{Digest, Sha256};

    fn fixture() -> Vec<BundleFile> {
        vec![BundleFile {
            name: "EffeTune Mixwright.vst3/Contents/test.txt",
            bytes: b"bundle",
            hash: "",
        }]
    }

    fn with_fixture(test: impl FnOnce(&Path, &[BundleFile])) {
        let temp = tempfile::tempdir().unwrap();
        let mut files = fixture();
        files[0].hash =
            Box::leak(crate::hex_lower(&Sha256::digest(files[0].bytes)).into_boxed_str());
        test(temp.path(), &files);
    }

    #[test]
    fn extracts_and_reuses_complete_verified_tree() {
        with_fixture(|dir, files| {
            ensure_bundle(dir, files, "v0.11.1:hash").unwrap();
            let stamp = std::fs::read(dir.join("effetune/.manifest")).unwrap();
            ensure_bundle(dir, files, "v0.11.1:hash").unwrap();
            assert_eq!(
                std::fs::read(dir.join("effetune/.manifest")).unwrap(),
                stamp
            );
            assert_eq!(
                std::fs::read(dir.join("effetune").join(files[0].name)).unwrap(),
                b"bundle"
            );
        });
    }

    #[test]
    fn repairs_partial_extra_stale_and_same_length_corrupt_trees() {
        with_fixture(|dir, files| {
            let path = dir.join("effetune").join(files[0].name);
            ensure_bundle(dir, files, "old-version").unwrap();
            ensure_bundle(dir, files, "new-version").unwrap();
            std::fs::write(&path, b"damage").unwrap();
            // Ensure a metadata change even on filesystems with coarse timestamps.
            let changed = std::time::SystemTime::now() + std::time::Duration::from_secs(2);
            std::fs::File::options()
                .write(true)
                .open(&path)
                .unwrap()
                .set_modified(changed)
                .unwrap();
            ensure_bundle(dir, files, "new-version").unwrap();
            assert_eq!(std::fs::read(&path).unwrap(), b"bundle");
            std::fs::write(dir.join("effetune/stale.txt"), b"stale").unwrap();
            ensure_bundle(dir, files, "new-version").unwrap();
            assert!(!dir.join("effetune/stale.txt").exists());
            std::fs::remove_file(&path).unwrap();
            ensure_bundle(dir, files, "new-version").unwrap();
            assert!(path.is_file());
            std::fs::remove_file(dir.join("effetune/.manifest")).unwrap();
            ensure_bundle(dir, files, "new-version").unwrap();
            assert!(dir.join("effetune/.manifest").is_file());
        });
    }

    #[test]
    fn recovers_interrupted_publication_and_rejects_invalid_embedded_hash() {
        with_fixture(|dir, files| {
            ensure_bundle(dir, files, "current").unwrap();
            std::fs::rename(dir.join("effetune"), dir.join(".effetune-old")).unwrap();
            std::fs::create_dir(dir.join(".effetune-stage")).unwrap();
            ensure_bundle(dir, files, "current").unwrap();
            assert!(dir.join("effetune").join(files[0].name).is_file());
            let invalid = [BundleFile {
                name: files[0].name,
                bytes: files[0].bytes,
                hash: "invalid",
            }];
            assert!(ensure_bundle(dir, &invalid, "changed").is_err());
            assert!(dir.join("effetune").join(files[0].name).is_file());
        });
    }

    #[test]
    fn serializes_concurrent_extraction() {
        with_fixture(|dir, files| {
            std::thread::scope(|scope| {
                for _ in 0..4 {
                    scope.spawn(|| ensure_bundle(dir, files, "same").unwrap());
                }
            });
            assert!(snapshot(&dir.join("effetune"), files, "same").is_ok());
        });
    }

    #[test]
    fn repairs_wrong_types_and_rejects_traversal() {
        with_fixture(|dir, files| {
            std::fs::write(dir.join("effetune"), b"not a directory").unwrap();
            ensure_bundle(dir, files, "current").unwrap();
            std::fs::remove_file(dir.join("effetune/.manifest")).unwrap();
            std::fs::create_dir(dir.join("effetune/.manifest")).unwrap();
            ensure_bundle(dir, files, "current").unwrap();
            assert!(dir.join("effetune/.manifest").is_file());
            let invalid = [BundleFile {
                name: "../escape",
                bytes: b"",
                hash: "invalid",
            }];
            assert!(ensure_bundle(dir, &invalid, "changed").is_err());
            assert!(!dir.join("escape").exists());
        });
    }
}

//! Worker-thread-only content-addressed extraction; old workers remain untouched.
use sha2::{Digest, Sha256};
use std::io;
use std::path::{Path, PathBuf};

static WORKER: &[u8] = include_bytes!("../../vendor/vlm-worker/mimageviewer-vlm.exe");

pub(super) fn worker_executable() -> io::Result<PathBuf> {
    prepare(&crate::data_dir::get(), WORKER)
}

fn prepare(data_dir: &Path, bytes: &[u8]) -> io::Result<PathBuf> {
    use crate::video::dsp::extract::{ensure_bytes, plain_directory};
    let data_dir = std::path::absolute(data_dir)?;
    // Check existing ancestors before creating anything through them. Each new
    // directory and the destination file use the same checks as the VST3 host.
    for path in data_dir.ancestors() {
        match std::fs::symlink_metadata(path) {
            Ok(meta) => {
                use std::os::windows::fs::MetadataExt;
                if !meta.is_dir() || meta.file_attributes() & 0x400 != 0 {
                    return Err(io::Error::other(
                        "VLM extraction rejects reparse directories",
                    ));
                }
            }
            Err(e) if e.kind() == io::ErrorKind::NotFound => (),
            Err(e) => return Err(e),
        }
    }
    plain_directory(&data_dir)?;
    let base = data_dir.join("vlm");
    plain_directory(&base)?;
    let workers = base.join("workers");
    plain_directory(&workers)?;
    let directory = workers.join(format!("{:x}", Sha256::digest(bytes)));
    plain_directory(&directory)?;
    let executable = directory.join("mimageviewer-vlm.exe");
    ensure_bytes(&executable, bytes)?;
    Ok(executable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extraction_reuses_repairs_and_keeps_older_versions() {
        let temp = tempfile::tempdir().unwrap();
        let first = prepare(temp.path(), b"worker one").unwrap();
        let modified = std::fs::metadata(&first).unwrap().modified().unwrap();
        assert_eq!(prepare(temp.path(), b"worker one").unwrap(), first);
        assert_eq!(
            std::fs::metadata(&first).unwrap().modified().unwrap(),
            modified
        );
        std::fs::write(&first, b"corrupt").unwrap();
        prepare(temp.path(), b"worker one").unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"worker one");
        let second = prepare(temp.path(), b"worker two").unwrap();
        assert_ne!(first, second);
        assert_eq!(std::fs::read(first).unwrap(), b"worker one");
        assert_eq!(std::fs::read(second).unwrap(), b"worker two");
    }

    #[test]
    fn extraction_rejects_non_directory_and_non_file_targets() {
        let temp = tempfile::tempdir().unwrap();
        std::fs::write(temp.path().join("vlm"), b"blocked").unwrap();
        assert!(prepare(temp.path(), b"worker").is_err());
        std::fs::remove_file(temp.path().join("vlm")).unwrap();
        let path = prepare(temp.path(), b"worker").unwrap();
        std::fs::remove_file(&path).unwrap();
        std::fs::create_dir(&path).unwrap();
        assert!(prepare(temp.path(), b"worker").is_err());
    }

    #[test]
    fn extraction_rejects_junctions_before_writing_through_them() {
        use std::os::windows::process::CommandExt;
        let temp = tempfile::tempdir().unwrap();
        let destination = temp.path().join("outside");
        std::fs::create_dir(&destination).unwrap();
        for nested in [false, true] {
            let link = temp.path().join(if nested { "vlm" } else { "alias" });
            let output = std::process::Command::new("cmd.exe")
                .args(["/d", "/c", "mklink", "/J"])
                .arg(&link)
                .arg(&destination)
                .creation_flags(0x08000000)
                .output()
                .unwrap();
            assert!(output.status.success(), "{output:?}");
            let base = if nested {
                temp.path().to_owned()
            } else {
                link.join("new-data")
            };
            let result = prepare(&base, b"worker");
            // Remove only the junction itself before TempDir's recursive cleanup.
            std::fs::remove_dir(&link).unwrap();
            assert!(result.is_err());
            assert_eq!(std::fs::read_dir(&destination).unwrap().count(), 0);
        }
    }
}

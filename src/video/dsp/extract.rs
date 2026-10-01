//! VST3 host を data_dir/vst3/hosts/<content hash>/ に展開する。
//! host の basename は維持し、CRT は検索対象外の vcrt/ に置く。

use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[cfg(not(feature = "portable"))]
static BRIDGE_EXE_BYTES: &[u8] =
    include_bytes!("../../../vendor/vst3-host/mimageviewer-vst3-host.exe");

// Only successful preparation is cached. A transient extraction error may be
// retried in this process; callers keep the existing &'static PathBuf interface.
static EXE_PATH: OnceLock<PathBuf> = OnceLock::new();
static EXTRACT_LOCK: Mutex<()> = Mutex::new(());

pub fn ensure_bridge_extracted() -> Result<&'static PathBuf, String> {
    cached_bridge_path(&EXE_PATH, &EXTRACT_LOCK, || {
        #[cfg(feature = "portable")]
        {
            crate::native_assets::bundled("mimageviewer-vst3-host.exe")
        }
        #[cfg(not(feature = "portable"))]
        {
            prepare_bridge(&crate::data_dir::get().join("vst3"))
                .map_err(|error| format!("vst3 bridge preparation failed: {error}"))
        }
    })
}

fn cached_bridge_path<'a>(
    cache: &'a OnceLock<PathBuf>,
    lock: &Mutex<()>,
    prepare: impl FnOnce() -> Result<PathBuf, String>,
) -> Result<&'a PathBuf, String> {
    if let Some(path) = cache.get() {
        return Ok(path);
    }
    let _guard = lock
        .lock()
        .map_err(|_| "vst3 extraction lock poisoned".to_string())?;
    if let Some(path) = cache.get() {
        return Ok(path);
    }
    let path = prepare()?;
    let _ = cache.set(path);
    Ok(cache.get().expect("successful preparation is cached"))
}

#[cfg(not(feature = "portable"))]
const HOST_VCRT: &[(&str, &[u8])] = &[
    (
        "msvcp140.dll",
        include_bytes!("../../../vendor/vcrt/msvcp140.dll"),
    ),
    (
        "msvcp140_1.dll",
        include_bytes!("../../../vendor/vcrt/msvcp140_1.dll"),
    ),
    (
        "vcruntime140.dll",
        include_bytes!("../../../vendor/vcrt/vcruntime140.dll"),
    ),
    (
        "vcruntime140_1.dll",
        include_bytes!("../../../vendor/vcrt/vcruntime140_1.dll"),
    ),
];

#[cfg(not(feature = "portable"))]
fn host_content_id() -> String {
    use sha2::{Digest, Sha256};
    let mut hash = Sha256::new();
    hash.update(BRIDGE_EXE_BYTES);
    for &(name, bytes) in HOST_VCRT {
        hash.update(name.as_bytes());
        hash.update((bytes.len() as u64).to_le_bytes());
        hash.update(bytes);
    }
    format!("{:x}", hash.finalize())
}

#[cfg(not(feature = "portable"))]
fn plain_directory(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(path)?;
    let metadata = std::fs::symlink_metadata(path)?;
    #[cfg(windows)]
    let reparse = {
        use std::os::windows::fs::MetadataExt;
        metadata.file_attributes() & 0x400 != 0
    };
    #[cfg(not(windows))]
    let reparse = metadata.file_type().is_symlink();
    if !metadata.is_dir() || reparse {
        return Err(std::io::Error::other(
            "vst3 preparation rejects reparse directories",
        ));
    }
    Ok(())
}

#[cfg(not(feature = "portable"))]
fn prepare_bridge(base: &std::path::Path) -> std::io::Result<PathBuf> {
    // New host/CRT bytes have their own directory: never overwrite the old host,
    // or touch legacy app-local CRTs in base. The old process may still use them.
    plain_directory(base)?;
    let hosts = base.join("hosts");
    plain_directory(&hosts)?;
    let dir = hosts.join(host_content_id());
    plain_directory(&dir)?;
    let runtime = dir.join("vcrt");
    plain_directory(&runtime)?;
    for &(name, bytes) in HOST_VCRT {
        ensure_bytes(&runtime.join(name), bytes)?;
    }
    let exe = dir.join("mimageviewer-vst3-host.exe");
    ensure_bytes(&exe, BRIDGE_EXE_BYTES)?;
    Ok(exe)
}

#[cfg(not(feature = "portable"))]
fn ensure_bytes(path: &std::path::Path, bytes: &[u8]) -> std::io::Result<()> {
    use std::io::Write;
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(windows)]
            let reparse = {
                use std::os::windows::fs::MetadataExt;
                metadata.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let reparse = metadata.file_type().is_symlink();
            if !metadata.is_file() || reparse {
                return Err(std::io::Error::other(
                    "vst3 preparation requires a regular file",
                ));
            }
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => (),
        Err(error) => return Err(error),
    }
    if std::fs::read(path).is_ok_and(|actual| actual == bytes) {
        return Ok(());
    }
    let mut staged = tempfile::NamedTempFile::new_in(path.parent().unwrap())?;
    staged.write_all(bytes)?;
    staged.as_file().sync_all()?;
    staged.persist(path).map_err(|error| error.error)?;
    Ok(())
}

#[cfg(all(test, not(feature = "portable")))]
mod tests {
    #[test]
    fn transient_failure_is_not_cached_but_success_is() {
        let cache = std::sync::OnceLock::new();
        let lock = std::sync::Mutex::new(());
        assert!(super::cached_bridge_path(&cache, &lock, || Err("locked file".into())).is_err());
        let path = super::cached_bridge_path(&cache, &lock, || Ok("host.exe".into())).unwrap();
        assert_eq!(path, std::path::Path::new("host.exe"));
        let reused =
            super::cached_bridge_path(&cache, &lock, || panic!("must reuse success")).unwrap();
        assert_eq!(path, reused);
    }

    #[test]
    fn host_is_content_addressed_and_vcrt_does_not_shadow_legacy_or_system() {
        let temp = tempfile::tempdir().unwrap();
        let legacy = temp.path().join("msvcp140.dll");
        std::fs::write(&legacy, b"legacy in-use CRT").unwrap();
        let exe = super::prepare_bridge(temp.path()).unwrap();
        assert_eq!(exe.file_name().unwrap(), "mimageviewer-vst3-host.exe");
        assert_eq!(std::fs::read(&legacy).unwrap(), b"legacy in-use CRT");
        for &(name, bytes) in super::HOST_VCRT {
            assert!(!exe.parent().unwrap().join(name).exists());
            let path = exe.parent().unwrap().join("vcrt").join(name);
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            let mut corrupt = bytes.to_vec();
            corrupt[0] ^= 0xff;
            std::fs::write(&path, corrupt).unwrap();
        }
        assert_eq!(super::prepare_bridge(temp.path()).unwrap(), exe);
        for &(name, bytes) in super::HOST_VCRT {
            assert_eq!(
                std::fs::read(exe.parent().unwrap().join("vcrt").join(name)).unwrap(),
                bytes
            );
        }
    }

    #[test]
    fn failed_preparation_can_be_retried() {
        let temp = tempfile::tempdir().unwrap();
        let blocked = temp.path().join("hosts");
        std::fs::write(&blocked, b"blocked").unwrap();
        assert!(super::prepare_bridge(temp.path()).is_err());
        std::fs::remove_file(&blocked).unwrap();
        assert!(super::prepare_bridge(temp.path()).unwrap().is_file());
    }
}

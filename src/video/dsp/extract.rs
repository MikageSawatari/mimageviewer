//! VST3 host bridge exe を `%APPDATA%\mimageviewer\vst3\` に展開する。
//! PDFium / Susie ワーカー / FFmpeg DLL と同じパターン (CLAUDE.md 参照)。

use std::path::PathBuf;
use std::sync::OnceLock;

// portable ビルドでは埋め込まず exe 隣の loose bridge exe を使う (native_assets 参照)。
#[cfg(not(feature = "portable"))]
static BRIDGE_EXE_BYTES: &[u8] =
    include_bytes!("../../../vendor/vst3-host/mimageviewer-vst3-host.exe");

static EXE_PATH: OnceLock<Result<PathBuf, String>> = OnceLock::new();

/// bridge exe を APPDATA に展開し、そのパスを返す。
/// 既に展開済み (= サイズ一致) ならスキップ。
/// portable ビルドでは展開せず、exe と同じディレクトリの loose exe を返す。
pub fn ensure_bridge_extracted() -> Result<&'static PathBuf, String> {
    EXE_PATH
        .get_or_init(|| {
            #[cfg(feature = "portable")]
            {
                crate::native_assets::bundled("mimageviewer-vst3-host.exe")
            }
            #[cfg(not(feature = "portable"))]
            {
                let dir = crate::data_dir::get().join("vst3");
                std::fs::create_dir_all(&dir)
                    .map_err(|e| format!("vst3 dir create failed: {e}"))?;
                let exe = dir.join("mimageviewer-vst3-host.exe");
                crate::data_dir::extract_embedded_file(
                    &exe,
                    BRIDGE_EXE_BYTES,
                    "mimageviewer-vst3-host.exe",
                )
                .map_err(|e| format!("vst3 bridge extract failed: {e}"))?;
                ensure_host_vcrt(&dir)
                    .map_err(|e| format!("vst3 VC runtime extract failed: {e}"))?;
                Ok(exe)
            }
        })
        .as_ref()
        .map_err(|e| e.clone())
}

// The plugin imports dynamic VC runtime, while the host runs outside the launcher's
// runtime directory. Windows resolves these imports beside the host executable.
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
fn ensure_host_vcrt(dir: &std::path::Path) -> std::io::Result<()> {
    use std::io::Write;
    for &(name, bytes) in HOST_VCRT {
        let path = dir.join(name);
        // Once per process, verify actual bytes, including same-length corruption.
        if std::fs::read(&path).is_ok_and(|actual| actual == bytes) {
            continue;
        }
        let mut staged = tempfile::NamedTempFile::new_in(dir)?;
        staged.write_all(bytes)?;
        staged.as_file().sync_all()?;
        staged.persist(&path).map_err(|error| error.error)?;
    }
    Ok(())
}

#[cfg(all(test, not(feature = "portable")))]
mod tests {
    #[test]
    fn host_vcrt_is_complete_and_repairs_same_length_corruption() {
        let temp = tempfile::tempdir().unwrap();
        super::ensure_host_vcrt(temp.path()).unwrap();
        for &(name, bytes) in super::HOST_VCRT {
            let path = temp.path().join(name);
            assert_eq!(std::fs::read(&path).unwrap(), bytes);
            let mut corrupt = bytes.to_vec();
            corrupt[0] ^= 0xff;
            std::fs::write(&path, corrupt).unwrap();
        }
        super::ensure_host_vcrt(temp.path()).unwrap();
        for &(name, bytes) in super::HOST_VCRT {
            assert_eq!(std::fs::read(temp.path().join(name)).unwrap(), bytes);
        }
    }
}

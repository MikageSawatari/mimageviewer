//! VST3 host bridge exe を `%APPDATA%\mimageviewer\vst3\` に展開する。
//! PDFium / Susie ワーカー / FFmpeg DLL と同じパターン (CLAUDE.md 参照)。

use std::path::PathBuf;
use std::sync::OnceLock;

static BRIDGE_EXE_BYTES: &[u8] =
    include_bytes!("../../../vendor/vst3-host/mimageviewer-vst3-host.exe");

static EXE_PATH: OnceLock<Result<PathBuf, String>> = OnceLock::new();

/// bridge exe を APPDATA に展開し、そのパスを返す。
/// 既に展開済み (= サイズ一致) ならスキップ。
pub fn ensure_bridge_extracted() -> Result<&'static PathBuf, String> {
    EXE_PATH
        .get_or_init(|| {
            let dir = crate::data_dir::get().join("vst3");
            std::fs::create_dir_all(&dir).map_err(|e| format!("vst3 dir create failed: {e}"))?;
            let exe = dir.join("mimageviewer-vst3-host.exe");
            crate::data_dir::extract_embedded_file(
                &exe,
                BRIDGE_EXE_BYTES,
                "mimageviewer-vst3-host.exe",
            )
            .map_err(|e| format!("vst3 bridge extract failed: {e}"))?;
            Ok(exe)
        })
        .as_ref()
        .map_err(|e| e.clone())
}

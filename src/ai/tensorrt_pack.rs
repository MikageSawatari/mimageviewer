//! TensorRT 高速化パックの管理。
//!
//! TensorRT EP に必要な NVIDIA ランタイム DLL 群 (CUDA Runtime / cuDNN /
//! TensorRT / Microsoft.ML.OnnxRuntime.Gpu) は合計 ~1.5 GB あり、本体 exe に
//! 埋め込めない。ユーザーが設定で TensorRT を有効化したときに別途
//! GitHub Releases からダウンロードして `%APPDATA%/mimageviewer/tensorrt/` に
//! 展開する。
//!
//! Phase 1 ではこのスケルトンだけを提供し、Phase 2 で download / extract /
//! verify の本実装を入れる。Phase 1 では `is_pack_installed()` が常に false を
//! 返すため、TensorRT バックエンドが選択されても DirectML フォールバックする。
//!
//! 互換性のため、本モジュールで使う定数 (DLL 名一覧、sentinel ファイル名、
//! ディレクトリ構造) はワーカープロセス側 (`bin/tensorrt_build.rs`) と
//! ランタイム側 (`runtime.rs`) の両方から参照する。

use std::path::PathBuf;

/// 対応 pack バージョン。pack 内 INSTALL_OK の version と一致する必要がある。
/// CUDA / cuDNN / TensorRT / ort のいずれかが更新されたら bump する。
///
/// - v1 (Apr 28): 取り下げ済 (DLL trim 過剰で CPU fallback、worker crash 多発)
/// - v2 (Apr 29): trim test を `session_run min < 200ms` 判定に強化、4 個 DLL を REQUIRED へ
#[allow(dead_code)]
pub const EXPECTED_TRT_PACK_VERSION: u32 = 2;

/// pack 展開先ルートディレクトリ。
/// `%APPDATA%/mimageviewer/tensorrt/`
pub fn pack_dir() -> PathBuf {
    crate::data_dir::get().join("tensorrt")
}

/// TensorRT エンジンキャッシュのルートディレクトリ。
/// `%APPDATA%/mimageviewer/tensorrt-engines/`
///
/// モデルごとにサブディレクトリを作って、TRT EP の engine cache path に渡す。
#[allow(dead_code)]
pub fn engine_cache_dir() -> PathBuf {
    crate::data_dir::get().join("tensorrt-engines")
}

/// pack インストール完了 sentinel ファイル。展開完了時に最後に書き込まれる。
/// 中身は JSON で `{"version": N, "trt_version": "10.x", ...}` を保持。
#[allow(dead_code)]
pub fn install_sentinel_path() -> PathBuf {
    pack_dir().join("INSTALL_OK")
}

/// pack 内の onnxruntime.dll (Microsoft.ML.OnnxRuntime.Gpu 版) のパス。
/// `ort::init_from()` の引数になる。
pub fn pack_ort_dll_path() -> PathBuf {
    pack_dir().join("onnxruntime.dll")
}

/// TensorRT pack がインストール済みかを判定する。
///
/// Phase 1: sentinel ファイル + 主要 DLL の存在確認だけ。
/// Phase 2: pack version 検証 (INSTALL_OK 内 version vs EXPECTED_TRT_PACK_VERSION) を追加。
pub fn is_pack_installed() -> bool {
    let sentinel = install_sentinel_path();
    let ort_dll = pack_ort_dll_path();
    sentinel.exists() && ort_dll.exists()
}

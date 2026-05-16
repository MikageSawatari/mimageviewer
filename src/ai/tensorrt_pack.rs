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
/// - v3 (May 02): `UpscaleRealEsrGeneralV3` を pack 同梱対象から除外 (= in-process
///   DirectML 経路に固定)。bench で TRT/DirectML がほぼ互角だったため worker IPC
///   overhead を払う価値なしと判断。詳細は
///   `docs/tensorrt-batching-feasibility.md` を参照。
#[allow(dead_code)]
pub const EXPECTED_TRT_PACK_VERSION: u32 = 3;

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

/// TensorRT pack のインストール状態 (T46 / Codex R-AI-001、2026-05-16)。
///
/// `is_pack_installed()` の bool では区別できなかった「version mismatch」「JSON 壊れ」
/// を UI / runtime 側で分岐できるよう細分化する。`Stale` の場合は再 install を促す
/// バナーを出す、`Corrupt` の場合は手動削除を案内する、等の使い分けが可能になる。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PackStatus {
    /// sentinel・DLL 揃いかつ INSTALL_OK の version が EXPECTED_TRT_PACK_VERSION と一致。
    Valid,
    /// sentinel か必須 DLL が無い。
    Missing,
    /// version mismatch (INSTALL_OK の version != EXPECTED_TRT_PACK_VERSION)。
    /// pack v(N) を入れて mIV を更新した場合に発生。再 install で復旧。
    Stale(u32),
    /// INSTALL_OK JSON のパース失敗、または必須フィールド欠落。
    Corrupt(String),
}

/// pack の現状を返す (T46)。`is_pack_installed()` はこの結果を bool 化したラッパー。
pub fn pack_status() -> PackStatus {
    let sentinel = install_sentinel_path();
    let ort_dll = pack_ort_dll_path();
    if !sentinel.exists() || !ort_dll.exists() {
        return PackStatus::Missing;
    }
    let raw = match std::fs::read_to_string(&sentinel) {
        Ok(s) => s,
        Err(e) => return PackStatus::Corrupt(format!("INSTALL_OK read failed: {e}")),
    };
    let value: serde_json::Value = match serde_json::from_str(&raw) {
        Ok(v) => v,
        Err(e) => return PackStatus::Corrupt(format!("INSTALL_OK JSON parse failed: {e}")),
    };
    let version = match value.get("version").and_then(|v| v.as_u64()) {
        Some(v) => v as u32,
        None => return PackStatus::Corrupt("INSTALL_OK missing 'version' field".to_owned()),
    };
    if version != EXPECTED_TRT_PACK_VERSION {
        return PackStatus::Stale(version);
    }
    PackStatus::Valid
}

/// TensorRT pack がインストール済みかを判定する。
///
/// T46: 旧 phase 1 (sentinel + DLL 存在のみ) では pack v(N) を入れた状態で
/// mIV を更新したときに stale pack をロードしようとして「spawn failed」になっていた。
/// 現実装は `pack_status() == Valid` をチェックする。
pub fn is_pack_installed() -> bool {
    matches!(pack_status(), PackStatus::Valid)
}

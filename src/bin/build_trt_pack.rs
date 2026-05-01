//! TensorRT 配布パックの GitHub Releases 用アセット作成ツール (Apr 28 改訂)。
//!
//! ## 入力
//!
//! - `%APPDATA%/mimageviewer/tensorrt/`: `setup-tensorrt-pack.ps1` で展開した DLL 群
//! - `%APPDATA%/mimageviewer/tensorrt-engines/<model>/`: `mimageviewer.exe --tensorrt-build`
//!   で生成した AMPERE_PLUS engine 群 (6 モデル分)
//!
//! ## 出力 (`dist/trt-pack-v<N>/`)
//!
//! - `manifest.json` - 全アセットの SHA-256 + サイズ + 各種バージョン情報
//! - `<dll>.dll` × ~17 - GitHub Releases にそのままアップロードする runtime DLL 群
//! - `engines-ampere_plus.zip` - 6 モデル分の事前ビルド済み engine をまとめた zip
//! - `NOTICE-NVIDIA.txt` - NVIDIA SDK SLA / 各 Supplement の attribution と利用条件抜粋
//! - `LICENSE-onnxruntime.txt` - ONNX Runtime (Microsoft) の MIT ライセンス全文
//!
//! ## 設計判断 (Apr 28 ライセンス調査結果反映)
//!
//! - **`nvinfer_builder_resource_*.dll` は配布しない**: TensorRT SLA で再配布許諾が
//!   明確でない (`runtime files` に該当しない可能性が高い) ため、エンジンを
//!   mikage 側で事前 build して `.engine` ファイルとして配布する方針に切替。
//!   ユーザー機での engine compile は行わない。
//! - **`kAMPERE_PLUS` モード**: `runtime.rs` で `with_engine_hw_compatible(true)` を
//!   ハードコードしているため、生成 engine は sm80+ (RTX 30/40/50) で動く。実測 perf
//!   低下は wall time 平均 +5.4%、最大 +8.8%。Turing (sm75) は将来別 engine pack で対応。
//! - **per-DLL 配信** (vs 1 個の zip): GitHub Releases の 2 GiB / file 上限に収まる、
//!   resume が file 単位で素直、必要なら部分更新できる。
//! - **engine だけ zip**: 6 モデル分の engine + profile (~12 ファイル) を 1 zip に
//!   まとめる。ファイル単位で SHA-256 を取るより manifest シンプル + DL も 1 トランザクション。
//!
//! ## 使い方
//! ```sh
//! cargo run --release --bin build_trt_pack
//! ```

use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

/// pack バージョン。`tensorrt_pack.rs::EXPECTED_TRT_PACK_VERSION` と揃える。
/// CUDA / cuDNN / TensorRT / ORT のいずれかを更新したら bump する。
///
/// - v1 (Apr 28): 初版。trim test 不十分で TRT EP load 失敗 → CPU fallback で
///   公開直後に取り下げ
/// - v2 (Apr 29): trim test を `session_run min < 200ms` 判定に強化して再決定。
///   v1 で誤って REMOVABLE 判定していた 4 個 (cublas64_12, cudnn64_9,
///   cudnn_graph64_9, nvonnxparser_10) を REQUIRED に戻した
const PACK_VERSION: u32 = 2;

/// `NOTICE-NVIDIA.txt` 文面。pack に同梱する NVIDIA コンポーネントの attribution と
/// 利用条件 (= mIV 専用、抽出再配布禁止、リバースエンジニアリング禁止) を明記する。
///
/// 出典:
/// - docs/licensing-tensorrt.md §NOTICE-NVIDIA.txt 推奨文面 (Apr 28 確定)
/// - 列挙する DLL は §最終 DLL セット の REQUIRED 17 個に合わせて整理
///
/// 注意: バージョン番号 (CUDA 12.9 / cuDNN 9.21 / TensorRT 10.16) を更新する場合は
/// `setup-tensorrt-pack.ps1` 側の `$*_VERSION` と同期させる。
const NOTICE_NVIDIA: &str = "\
This product includes software components from NVIDIA Corporation, redistributed under
the NVIDIA Software License Agreement for NVIDIA Software Development Kits and its
supplements. Use of these components is subject to those agreements.

Components included (mImageViewer TensorRT acceleration pack v2):

  CUDA Runtime / Math / NVRTC / nvJitLink (CUDA Toolkit 12.9)
    cudart64_12.dll
    cublas64_12.dll
    cublasLt64_12.dll
    cufft64_11.dll
    nvJitLink_120_0.dll
    nvrtc64_120_0.dll
    nvrtc-builtins64_129.dll

  cuDNN (NVIDIA cuDNN 9.21)
    cudnn64_9.dll
    cudnn_graph64_9.dll
    cudnn_ops64_9.dll

  TensorRT (NVIDIA TensorRT 10.16)
    nvinfer_10.dll
    nvinfer_plugin_10.dll
    nvonnxparser_10.dll

  Pre-built TensorRT engines (kAMPERE_PLUS hardware-compatible mode, sm80+)
    engines-ampere_plus.zip
      Built with TensorRT 10.16 from publicly distributed ONNX models
      (Real-ESRGAN, Real-CUGAN, NMKD-Siax, RealPLKSR). The .engine binaries
      are derivative artifacts of the TensorRT builder; their distribution is
      governed by the TensorRT supplement's runtime distribution clause.

Copyright (c) NVIDIA Corporation. All rights reserved.

Source license texts (please consult the latest revision at the URLs below):
  CUDA Toolkit EULA:   https://docs.nvidia.com/cuda/eula/index.html
  cuDNN SLA:           https://docs.nvidia.com/deeplearning/cudnn/sla/index.html
  TensorRT SLA:        https://docs.nvidia.com/deeplearning/tensorrt/sla/index.html

These components are redistributed solely for use with mImageViewer
(https://mikage.to/mimageviewer/). The following are prohibited:

  - Reverse engineering, decompilation, or disassembly of the components,
    except to the extent expressly permitted by applicable law.
  - Extraction of the components for use outside of mImageViewer or for
    redistribution as a standalone or repackaged NVIDIA SDK.
  - Use of the components in violation of the NVIDIA license agreements
    referenced above, including but not limited to the Distribution
    Requirements of the NVIDIA Software License Agreement for SDKs.

mImageViewer makes no claim of endorsement or affiliation with NVIDIA
Corporation. \"NVIDIA\", \"CUDA\", \"cuDNN\", and \"TensorRT\" are trademarks of
NVIDIA Corporation.
";

/// `LICENSE-onnxruntime.txt` 文面 (Microsoft ONNX Runtime MIT License 全文)。
///
/// 出典: https://github.com/microsoft/onnxruntime/blob/main/LICENSE
/// (2024 年版を確認、本文は 2018 年から実質的な変更なし)
const LICENSE_ONNXRUNTIME: &str = "\
MIT License

Copyright (c) Microsoft Corporation

Permission is hereby granted, free of charge, to any person obtaining a copy
of this software and associated documentation files (the \"Software\"), to deal
in the Software without restriction, including without limitation the rights
to use, copy, modify, merge, publish, distribute, sublicense, and/or sell
copies of the Software, and to permit persons to whom the Software is
furnished to do so, subject to the following conditions:

The above copyright notice and this permission notice shall be included in all
copies or substantial portions of the Software.

THE SOFTWARE IS PROVIDED \"AS IS\", WITHOUT WARRANTY OF ANY KIND, EXPRESS OR
IMPLIED, INCLUDING BUT NOT LIMITED TO THE WARRANTIES OF MERCHANTABILITY,
FITNESS FOR A PARTICULAR PURPOSE AND NONINFRINGEMENT. IN NO EVENT SHALL THE
AUTHORS OR COPYRIGHT HOLDERS BE LIABLE FOR ANY CLAIM, DAMAGES OR OTHER
LIABILITY, WHETHER IN AN ACTION OF CONTRACT, TORT OR OTHERWISE, ARISING FROM,
OUT OF OR IN CONNECTION WITH THE SOFTWARE OR THE USE OR OTHER DEALINGS IN THE
SOFTWARE.
";

/// Apr 28 multi-model trim test で「全 6 モデルが動く最小セット」から除外可能と
/// 確定した DLL のリスト。**Apr 29 trim test v2** で全 6 モデル (Real-ESRGAN
/// x4plus / anime6b / general_v3 / RealCUGAN-4x / NMKD-Siax-4x / RealPLKSR) で
/// TRT EP が実際に動作することを実機検証した結果。
///
/// ## v1 trim test の問題と v2 での修正
///
/// v1 (Apr 28) は `bench_ai --runs 1` の "wall total" 出力だけで成功判定していたが、
/// ORT は **TRT EP load 失敗時に CUDA → CPU と silent fallback** するので、CPU で
/// 完走しても "成功" に見える落とし穴があった。実機 distribution 後にユーザー機
/// (RTX 4090) で worker crash (STATUS_STACK_BUFFER_OVERRUN) が判明し、原因は
/// 4 個の DLL の hard import 不足 (= LoadLibrary 失敗の連鎖) と特定。
///
/// v2 では以下の検証強化:
/// 1. `bench_ai --warmup 1 --runs 1` を全 6 モデルで実行
/// 2. **全モデルで session_run min < 200 ms を確認** (TRT は 10-50ms、CUDA EP は
///    200-500ms、CPU EP は 1500ms+。200ms 閾値で TRT 経路を保証)
/// 3. crash (worker EOF) も明示的に検出
///
/// 検証スクリプト: `scripts/trim_dlls_v2.sh` (実行ログ: `/tmp/trim_dlls_v2/result.txt`)
///
/// `nvinfer_builder_resource_*.dll` は別判定 (= ライセンス上必ず除外、prefix match)。
/// ただし v2 trim test でも全 8 個が REMOVABLE と確認できているので、技術的にも
/// 不要であることが裏付け済み。
///
/// 最終 REQUIRED DLL (= mIV 配布物に含まれる) は 17 個、約 2.05 GB:
///   cublas64_12, cublasLt64_12, cudart64_12, cudnn64_9, cudnn_graph64_9,
///   cudnn_ops64_9, cufft64_11, nvJitLink_120_0, nvinfer_10, nvinfer_plugin_10,
///   nvonnxparser_10, nvrtc-builtins64_129, nvrtc64_120_0, onnxruntime,
///   onnxruntime_providers_{shared,cuda,tensorrt}
const REMOVABLE_DLLS: &[&str] = &[
    // 数学ライブラリ系 (cuFFTW / cuRAND / cuSOLVER / cuSPARSE)
    // 注意: cublas64_12 は v1 で REMOVABLE 判定したが provider DLL の hard import
    // にあったため REQUIRED へ戻した (= ここから除外)
    "cufftw64_11.dll",
    "curand64_10.dll",
    "cusolver64_11.dll",
    "cusolverMg64_11.dll",
    "cusparse64_12.dll",
    // cuDNN 補助 (cudnn64_9, cudnn_ops64_9, cudnn_graph64_9 は REQUIRED で残す)
    // 注意: cudnn64_9, cudnn_graph64_9 は v1 で REMOVABLE 判定したが trim_dlls_v2
    // で REQUIRED と確定したため除外
    "cudnn_adv64_9.dll",
    "cudnn_cnn64_9.dll",
    "cudnn_engines_precompiled64_9.dll",
    "cudnn_engines_runtime_compiled64_9.dll",
    "cudnn_engines_tensor_ir64_9.dll",
    "cudnn_heuristic64_9.dll",
    // TensorRT 補助 runtime (lean / dispatch / vc_plugin)
    // 注意: nvonnxparser_10 は v1 で REMOVABLE 判定したが provider DLL の hard
    // import にあったため REQUIRED へ戻した
    "nvinfer_lean_10.dll",
    "nvinfer_dispatch_10.dll",
    "nvinfer_vc_plugin_10.dll",
    // NVRTC alt variant (Hopper 系で使う代替経路、画像ビューワーでは未使用)
    "nvrtc64_120_0.alt.dll",
];

/// `onnxruntime_providers_*.dll` の hard import 一覧 (`dumpbin /dependents` 由来)。
/// pack 構築時の sanity check に使う: ここに列挙される DLL が REQUIRED に
/// 含まれていなければ build を失敗させる (= v1 で起きた CPU fallback バグの
/// 再発防止)。
///
/// 出典: `dumpbin /dependents <provider_dll>` の出力 (Apr 29 確認、
/// CUDA Toolkit 12.9 / cuDNN 9.21 / TensorRT 10.16)。CUDA / cuDNN / TRT の
/// メジャー版が変わったら再確認すること。
///
/// system DLL (KERNEL32, MSVCP140, api-ms-win-crt-*, dbghelp 等) は除外。
const PROVIDER_DLL_IMPORTS: &[(&str, &[&str])] = &[
    (
        "onnxruntime_providers_tensorrt.dll",
        &[
            "cublas64_12.dll",
            "nvinfer_10.dll",
            "nvonnxparser_10.dll",
            "onnxruntime_providers_shared.dll",
            "cudart64_12.dll",
            "cudnn64_9.dll",
        ],
    ),
    (
        "onnxruntime_providers_cuda.dll",
        &[
            "cublasLt64_12.dll",
            "cublas64_12.dll",
            "cufft64_11.dll",
            "cudart64_12.dll",
            "onnxruntime_providers_shared.dll",
            "cudnn64_9.dll",
        ],
    ),
];

/// 既存 INSTALL_OK の中身 (setup-tensorrt-pack.ps1 が書いた版情報)。
#[derive(Debug, Deserialize)]
struct InstallOk {
    version: u32,
    ort_gpu_version: String,
    cuda_cudart_version: String,
    cuda_cublas_version: String,
    cudnn_version: String,
    trt_version: String,
    #[allow(dead_code)]
    installed_at: String,
}

/// アセット 1 個のメタデータ。
#[derive(Debug, Serialize)]
struct AssetEntry {
    /// ファイル名 (DLL 名そのまま、または engine zip ファイル名)。
    name: String,
    /// SHA-256 (hex 小文字、64 文字)。
    sha256: String,
    /// バイト数。
    bytes: u64,
}

/// engine pack 1 セットのメタデータ。将来 sm75 (Turing) 用 pack を追加するので Vec で持つ。
#[derive(Debug, Serialize)]
struct EnginePack {
    /// pack 内部識別子 (URL 安全、英小文字+数字+underscore)。
    id: String,
    /// 対応する最小 CUDA Compute Capability (× 10)。例: 80 = sm80+。
    /// downloader が GPU SM を検出して、該当する最大値の pack を選ぶ。
    compute_capability_min: u32,
    /// UI / マニュアル表示用の人間可読ラベル。
    human_label: String,
    /// この pack を構成するファイル群 (通常 1 個の zip)。
    files: Vec<AssetEntry>,
}

/// マニフェスト全体。
#[derive(Debug, Serialize)]
struct Manifest {
    /// マニフェスト構造のバージョン。`Manifest` のフィールド構造を変えたら bump。
    /// - v2 (Apr 28): per_sm/optional/PTX を撤廃、engines bucket を導入
    /// - v3 (Apr 28): NOTICE-NVIDIA.txt / LICENSE-onnxruntime.txt の同梱を追加
    manifest_format: u32,
    /// pack バージョン (`PACK_VERSION`)。
    pack_version: u32,
    /// 各種ライブラリバージョン (UI 表示用 + ローカル INSTALL_OK 検証用)。
    versions: BTreeMap<String, String>,
    /// 全ユーザーが DL する DLL 群 (CUDA / cuDNN / TRT runtime / ORT)。
    /// `nvinfer_builder_resource_*.dll` は含めない (ライセンス上の判断)。
    common: Vec<AssetEntry>,
    /// 法的に同梱必須のテキストファイル (NOTICE-NVIDIA.txt, LICENSE-onnxruntime.txt)。
    /// ハッシュ検証対象。downloader は common と同じ経路で DL し
    /// `%APPDATA%/mimageviewer/tensorrt/` に配置する。
    notices: Vec<AssetEntry>,
    /// GPU 世代別の engine pack (zip)。downloader は SM に合わせて 1 個だけ DL。
    engines: Vec<EnginePack>,
    /// common DL 量の合計 (UI で進捗表示する際の分母)。
    common_total_bytes: u64,
    /// pack 作成時刻 (UTC ISO 8601)。
    created_at: String,
}

fn main() {
    let src_pack = data_dir().join("tensorrt");
    let src_engines = data_dir().join("tensorrt-engines");
    if !src_pack.is_dir() {
        eprintln!(
            "ERROR: source pack directory not found: {}\n\
             先に scripts/setup-tensorrt-pack.ps1 を走らせて DLL を展開してください。",
            src_pack.display()
        );
        std::process::exit(1);
    }
    if !src_engines.is_dir() {
        eprintln!(
            "ERROR: engines directory not found: {}\n\
             先に各モデルを `mimageviewer.exe --tensorrt-build <kind>` で build してください。",
            src_engines.display()
        );
        std::process::exit(1);
    }

    // INSTALL_OK 読み取り
    let install_ok_path = src_pack.join("INSTALL_OK");
    let install_ok_text = fs::read_to_string(&install_ok_path).unwrap_or_else(|e| {
        eprintln!("ERROR: read {}: {}", install_ok_path.display(), e);
        std::process::exit(1);
    });
    let install_ok_text = install_ok_text.trim_start_matches('\u{feff}'); // BOM 除去
    let install_ok: InstallOk = serde_json::from_str(install_ok_text).unwrap_or_else(|e| {
        eprintln!(
            "ERROR: parse {}: {} (中身: {})",
            install_ok_path.display(),
            e,
            install_ok_text
        );
        std::process::exit(1);
    });
    if install_ok.version != PACK_VERSION {
        eprintln!(
            "WARNING: INSTALL_OK.version ({}) != PACK_VERSION ({})。\
             続行するが本コードと DLL のバージョン整合に注意。",
            install_ok.version, PACK_VERSION
        );
    }

    // 出力ディレクトリ準備
    let dist_dir = PathBuf::from(format!("dist/trt-pack-v{}", PACK_VERSION));
    if dist_dir.exists() {
        println!(
            "[build_trt_pack] 既存 {} を削除して作り直す",
            dist_dir.display()
        );
        if let Err(e) = fs::remove_dir_all(&dist_dir) {
            eprintln!("ERROR: remove_dir_all {}: {}", dist_dir.display(), e);
            std::process::exit(1);
        }
    }
    fs::create_dir_all(&dist_dir).unwrap();

    // ── common DLLs ──
    let mut common: Vec<AssetEntry> = Vec::new();
    let mut common_total_bytes: u64 = 0;
    let mut excluded_builder_resource: usize = 0;
    let mut excluded_multi_trim: usize = 0;

    let dll_entries: Vec<_> = fs::read_dir(&src_pack)
        .unwrap()
        .filter_map(Result::ok)
        .filter(|e| {
            let p = e.path();
            p.is_file() && p.extension().map(|x| x == "dll").unwrap_or(false)
        })
        .collect();
    println!(
        "[build_trt_pack] DLL 走査: {} 個を {} から",
        dll_entries.len(),
        src_pack.display()
    );

    for entry in dll_entries {
        let path = entry.path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();

        // ライセンス調査結果 (docs/licensing-tensorrt.md): builder_resource は再配布
        // 許諾が明確でないため除外。AMPERE_PLUS engine を事前ビルドして同梱する方針。
        if name.starts_with("nvinfer_builder_resource_") {
            excluded_builder_resource += 1;
            println!("  excluded (builder_resource): {}", name);
            continue;
        }

        // Apr 28 multi-model trim test (docs/licensing-tensorrt.md §最終 DLL セット):
        // 全 6 モデル (Real-ESRGAN/CUGAN/NMKD-Siax/RealPLKSR) で TRT 推論が成功する
        // 最小 DLL セットを実機検証して以下の DLL を除外。
        // ORT/TRT EP が startup probe で実際にロードしないものを実機テストで確定。
        if REMOVABLE_DLLS.contains(&name.as_str()) {
            excluded_multi_trim += 1;
            println!("  excluded (multi-model trim): {}", name);
            continue;
        }

        let asset = compute_asset(&path).unwrap_or_else(|e| {
            eprintln!("ERROR: hash {}: {}", path.display(), e);
            std::process::exit(1);
        });
        copy_to_dist(&path, &dist_dir);
        common_total_bytes += asset.bytes;
        println!(
            "  common: {} ({:.1} MiB, sha256={}…)",
            name,
            asset.bytes as f64 / 1024.0 / 1024.0,
            &asset.sha256[..12]
        );
        common.push(asset);
    }
    common.sort_by(|a, b| a.name.cmp(&b.name));

    // ── 静的依存チェック: provider DLL の hard import が全て REQUIRED に含まれているか ──
    // v1 でこれを怠ったため CPU fallback バグが起きた。再発防止のガード。
    if let Err(missing) = check_provider_imports(&common) {
        eprintln!(
            "ERROR: 静的依存チェック失敗\n\n  以下の DLL は provider DLL の hard \
             import に列挙されているが REQUIRED に含まれていない:"
        );
        for m in &missing {
            eprintln!("    - {} (required by {})", m.dll, m.required_by);
        }
        eprintln!(
            "\nREMOVABLE_DLLS から該当 DLL を除外するか、setup-tensorrt-pack.ps1 で\n\
             該当 DLL が tensorrt/ に展開されているか確認してください。"
        );
        std::process::exit(1);
    }
    println!(
        "[build_trt_pack] 静的依存チェック OK ({} provider DLL)",
        PROVIDER_DLL_IMPORTS.len()
    );

    // ── engine pack (AMPERE_PLUS) ──
    // 6 モデルの engine cache (`<model>/<file>.engine` + `<file>.profile`) を 1 zip に。
    // ファイル名は ORT TRT EP がモデルハッシュから導出するので変えてはいけない。
    let engine_zip_name = "engines-ampere_plus.zip";
    let engine_zip_path = dist_dir.join(engine_zip_name);
    let zip_bytes = build_engine_zip(&src_engines, &engine_zip_path).unwrap_or_else(|e| {
        eprintln!("ERROR: build engine zip: {e}");
        std::process::exit(1);
    });
    let engine_zip_asset = compute_asset(&engine_zip_path).unwrap_or_else(|e| {
        eprintln!("ERROR: hash engine zip: {e}");
        std::process::exit(1);
    });
    println!(
        "  engine pack [ampere_plus]: {} ({:.1} MiB, sha256={}…)",
        engine_zip_asset.name,
        engine_zip_asset.bytes as f64 / 1024.0 / 1024.0,
        &engine_zip_asset.sha256[..12]
    );

    let engines = vec![EnginePack {
        id: "ampere_plus".to_string(),
        compute_capability_min: 80,
        human_label: "RTX 30/40/50 series (compute capability 8.0+)".to_string(),
        files: vec![engine_zip_asset],
    }];
    let engine_total_bytes: u64 = zip_bytes;

    // ── notices (NOTICE-NVIDIA.txt, LICENSE-onnxruntime.txt) ──
    // ライセンス文書は const 文字列を直接ファイルに書き出し、SHA-256 を取って
    // manifest に登録する。downloader 側はこれを common DLL と同じ要領で
    // %APPDATA%/mimageviewer/tensorrt/ に DL & 検証して配置する。
    // 改行コードは LF で固定 (= const 文字列のまま) にして OS をまたいでもハッシュが
    // 安定するようにする。
    let notices = write_and_hash_notices(&dist_dir).unwrap_or_else(|e| {
        eprintln!("ERROR: write notices: {e}");
        std::process::exit(1);
    });
    for asset in &notices {
        println!(
            "  notice: {} ({} bytes, sha256={}…)",
            asset.name,
            asset.bytes,
            &asset.sha256[..12]
        );
    }

    // ── manifest ──
    let mut versions = BTreeMap::new();
    versions.insert("ort_gpu".to_string(), install_ok.ort_gpu_version.clone());
    versions.insert(
        "cuda_cudart".to_string(),
        install_ok.cuda_cudart_version.clone(),
    );
    versions.insert(
        "cuda_cublas".to_string(),
        install_ok.cuda_cublas_version.clone(),
    );
    versions.insert("cudnn".to_string(), install_ok.cudnn_version.clone());
    versions.insert("trt".to_string(), install_ok.trt_version.clone());

    let manifest = Manifest {
        manifest_format: 3,
        pack_version: PACK_VERSION,
        versions,
        common,
        notices,
        engines,
        common_total_bytes,
        created_at: utc_now_iso8601(),
    };
    let manifest_path = dist_dir.join("manifest.json");
    let manifest_json = serde_json::to_string_pretty(&manifest).unwrap();
    fs::write(&manifest_path, &manifest_json).unwrap();

    let total_user_dl = common_total_bytes + engine_total_bytes;

    println!();
    println!("==================== 完了 ====================");
    println!("出力ディレクトリ: {}", dist_dir.display());
    println!("manifest:        {}", manifest_path.display());
    println!(
        "common DLL: {} 個、合計 {:.2} GB \
         (builder_resource を {} 個 + multi-model trim で {} 個 = 計 {} 個を除外)",
        manifest.common.len(),
        common_total_bytes as f64 / 1_073_741_824.0,
        excluded_builder_resource,
        excluded_multi_trim,
        excluded_builder_resource + excluded_multi_trim
    );
    println!(
        "engine pack [ampere_plus]: {:.1} MiB",
        engine_total_bytes as f64 / 1024.0 / 1024.0
    );
    let notices_total_bytes: u64 = manifest.notices.iter().map(|a| a.bytes).sum();
    println!(
        "notices: {} ファイル、{} bytes (NOTICE-NVIDIA.txt + LICENSE-onnxruntime.txt)",
        manifest.notices.len(),
        notices_total_bytes
    );
    println!(
        "ユーザー DL 量 (sm80+ ユーザー): 約 {:.2} GB",
        total_user_dl as f64 / 1_073_741_824.0
    );
    println!();
    println!("次のステップ:");
    println!(
        "  1. dist/trt-pack-v{}/ 内のすべてのファイルを GitHub Releases にアップロード",
        PACK_VERSION
    );
    println!("     (タグ名: trt-pack-v{} 推奨)", PACK_VERSION);
    println!("  2. mIV の `tensorrt_pack` モジュールに manifest URL を埋め込む");
    println!("  3. cargo build --release でビルド & 配布");
}

/// `NOTICE-NVIDIA.txt` と `LICENSE-onnxruntime.txt` を `dist_dir` に書き出して
/// SHA-256 を計算する。改行コードは LF 固定 (= const 文字列そのまま) にして、
/// OS や VCS の autocrlf 設定の影響でハッシュが揺れないようにする。
///
/// pack DL 経路でこの 2 つは common DLL と同じ位置 (`tensorrt/<name>`) に配置される
/// 想定なので、ファイル名は manifest 通りそのまま使う。
fn write_and_hash_notices(dist_dir: &Path) -> std::io::Result<Vec<AssetEntry>> {
    let entries: &[(&str, &str)] = &[
        ("NOTICE-NVIDIA.txt", NOTICE_NVIDIA),
        ("LICENSE-onnxruntime.txt", LICENSE_ONNXRUNTIME),
    ];
    let mut out = Vec::with_capacity(entries.len());
    for (name, body) in entries {
        let path = dist_dir.join(name);
        // バイナリモードで書き出して、Windows でも CRLF 変換が掛からないようにする。
        let bytes = body.as_bytes();
        fs::write(&path, bytes)?;
        // ハッシュは const 文字列そのものから計算 (= ファイル内容と一致するはず)。
        let mut hasher = Sha256::new();
        hasher.update(bytes);
        let hash = hasher.finalize();
        out.push(AssetEntry {
            name: (*name).to_string(),
            sha256: hex_encode(&hash),
            bytes: bytes.len() as u64,
        });
    }
    Ok(out)
}

/// pack に同梱必須なモデル一覧 (= worker 経由で TRT 動作させたい全モデル)。
/// `runtime.rs::should_route_to_worker` で TRT に route される 6 モデルと一致させる。
/// build_engine_zip がこれら全モデルの engine を見つけられなかったら build を失敗
/// させて出荷ミスを未然に防ぐ (Codex P2.4 指摘)。
const REQUIRED_ENGINE_MODELS: &[&str] = &[
    "realesrgan_x4plus",
    "realesrgan_anime6b",
    "realesr_general_v3",
    "realcugan_4x",
    "nmkd_siax_4x",
    "denoise_realplksr",
];

/// 6 モデル分の engine cache を 1 つの zip にまとめる。
/// 戻り値: 生成された zip ファイルのバイト数。
///
/// 全 `REQUIRED_ENGINE_MODELS` が揃っていることを検証し、欠けているモデルが
/// あれば build を失敗させる (= 中途半端な pack を distribute しないためのガード)。
fn build_engine_zip(src_engines: &Path, dst_zip: &Path) -> Result<u64, String> {
    let file = fs::File::create(dst_zip).map_err(|e| format!("create zip: {e}"))?;
    let mut zip = zip::ZipWriter::new(file);
    let opts: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .compression_level(Some(3)) // 軽い圧縮 (engine は乱数に近いので Deflate でも 5% 程度)
        .large_file(true); // zip64 対応 (4 GB 超ケースの保険、実際は数百 MB)

    let mut total_files: usize = 0;
    let mut total_bytes_in: u64 = 0;
    let mut found_models: std::collections::HashSet<String> = std::collections::HashSet::new();
    for model_entry in fs::read_dir(src_engines).map_err(|e| format!("read_dir engines: {e}"))? {
        let model_entry = model_entry.map_err(|e| format!("entry: {e}"))?;
        let model_dir = model_entry.path();
        if !model_dir.is_dir() {
            continue;
        }
        let model_name = model_dir.file_name().unwrap().to_string_lossy().to_string();

        // 各モデルディレクトリの中身 (.engine + .profile) を zip に。
        // ファイル名は ORT TRT EP のハッシュ命名を保つ必要がある (deserialize 時の lookup
        // キーになる)。
        let mut has_engine_file = false;
        let mut has_profile_file = false;
        for f in fs::read_dir(&model_dir).map_err(|e| format!("read_dir model: {e}"))? {
            let f = f.map_err(|e| format!("file entry: {e}"))?;
            let p = f.path();
            if !p.is_file() {
                continue;
            }
            let fname = p.file_name().unwrap().to_string_lossy().to_string();
            if fname.ends_with(".engine") {
                has_engine_file = true;
            } else if fname.ends_with(".profile") {
                has_profile_file = true;
            }
            // zip 内パスは `<model_name>/<file>` で、runtime 展開時に
            // `tensorrt-engines/<model_name>/<file>` になる
            let zip_path = format!("{}/{}", model_name, fname);
            zip.start_file(&zip_path, opts.clone())
                .map_err(|e| format!("start_file {zip_path}: {e}"))?;
            let mut src = fs::File::open(&p).map_err(|e| format!("open {}: {e}", p.display()))?;
            let mut buf = [0u8; 64 * 1024];
            loop {
                let n = src.read(&mut buf).map_err(|e| format!("read: {e}"))?;
                if n == 0 {
                    break;
                }
                zip.write_all(&buf[..n])
                    .map_err(|e| format!("zip write: {e}"))?;
                total_bytes_in += n as u64;
            }
            total_files += 1;
        }
        if has_engine_file && has_profile_file {
            found_models.insert(model_name);
        }
    }
    zip.finish().map_err(|e| format!("zip finish: {e}"))?;

    // 必須モデルがすべて揃っているかチェック (= .engine + .profile 両方ある)
    let missing: Vec<&str> = REQUIRED_ENGINE_MODELS
        .iter()
        .filter(|m| !found_models.contains(**m))
        .copied()
        .collect();
    if !missing.is_empty() {
        // zip は既に書き終わっているので削除して errror.
        let _ = fs::remove_file(dst_zip);
        return Err(format!(
            "engine pack に以下のモデルの engine/.profile が見つからない:\n  - {}\n\
             先に `mimageviewer.exe --tensorrt-build <model_kind>` でこれらの engine を\n\
             build してください。",
            missing.join("\n  - ")
        ));
    }

    let zip_size = fs::metadata(dst_zip)
        .map(|m| m.len())
        .map_err(|e| format!("metadata: {e}"))?;

    println!(
        "[engine zip] {} ファイル、{:.1} MiB → {:.1} MiB ({:.0}% 圧縮)",
        total_files,
        total_bytes_in as f64 / 1024.0 / 1024.0,
        zip_size as f64 / 1024.0 / 1024.0,
        if total_bytes_in == 0 {
            0.0
        } else {
            100.0 * (1.0 - zip_size as f64 / total_bytes_in as f64)
        }
    );
    Ok(zip_size)
}

/// `PROVIDER_DLL_IMPORTS` に列挙された hard import が全て `common` に含まれている
/// ことを検証する。一つでも欠けていたら `Err(Vec<MissingImport>)` を返す。
fn check_provider_imports(common: &[AssetEntry]) -> Result<(), Vec<MissingImport>> {
    let names: std::collections::HashSet<&str> = common.iter().map(|a| a.name.as_str()).collect();
    let mut missing = Vec::new();
    for (provider, imports) in PROVIDER_DLL_IMPORTS {
        for imp in *imports {
            if !names.contains(imp) {
                missing.push(MissingImport {
                    dll: imp.to_string(),
                    required_by: provider.to_string(),
                });
            }
        }
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(missing)
    }
}

/// `check_provider_imports` のエラー報告型。
#[derive(Debug)]
struct MissingImport {
    dll: String,
    required_by: String,
}

/// SHA-256 + サイズを計算してアセットエントリを作る。
fn compute_asset(path: &Path) -> std::io::Result<AssetEntry> {
    let mut file = fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 64 * 1024];
    let mut bytes: u64 = 0;
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
        bytes += n as u64;
    }
    let hash = hasher.finalize();
    Ok(AssetEntry {
        name: path.file_name().unwrap().to_string_lossy().to_string(),
        sha256: hex_encode(&hash),
        bytes,
    })
}

/// バイト列を hex 小文字に変換 (依存追加せず手書き)。
fn hex_encode(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0xf) as usize] as char);
    }
    out
}

/// 入力 DLL を出力ディレクトリへハードリンク (失敗したらコピー)。
/// ハードリンクが効けば 7 GB のコピーが瞬時に終わる。
fn copy_to_dist(src: &Path, dist_dir: &Path) {
    let dst = dist_dir.join(src.file_name().unwrap());
    if fs::hard_link(src, &dst).is_err() {
        // ボリューム跨ぎ等でハードリンク不可ならコピー
        if let Err(e) = fs::copy(src, &dst) {
            eprintln!("ERROR: copy {} → {}: {}", src.display(), dst.display(), e);
            std::process::exit(1);
        }
    }
}

fn data_dir() -> PathBuf {
    mimageviewer::data_dir::get()
}

/// 現在時刻を UTC ISO 8601 形式で返す (chrono 等の依存追加を避ける手書き実装)。
fn utc_now_iso8601() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap();
    let secs = now.as_secs();
    let (year, month, day, hour, min, sec) = unix_to_ymdhms(secs);
    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hour, min, sec
    )
}

/// Unix epoch 秒 → (年, 月, 日, 時, 分, 秒) UTC。グレゴリオ暦・うるう秒なし。
fn unix_to_ymdhms(mut secs: u64) -> (u32, u32, u32, u32, u32, u32) {
    let sec = (secs % 60) as u32;
    secs /= 60;
    let min = (secs % 60) as u32;
    secs /= 60;
    let hour = (secs % 24) as u32;
    secs /= 24;
    let mut days = secs;
    let mut year: u32 = 1970;
    loop {
        let days_in_year: u64 = if is_leap(year) { 366 } else { 365 };
        if days < days_in_year {
            break;
        }
        days -= days_in_year;
        year += 1;
    }
    let leap = is_leap(year);
    let mdays: [u32; 12] = [
        31,
        if leap { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    let mut month: u32 = 1;
    let mut day = days as u32 + 1;
    for &dm in &mdays {
        if day <= dm {
            break;
        }
        day -= dm;
        month += 1;
    }
    (year, month, day, hour, min, sec)
}

fn is_leap(y: u32) -> bool {
    (y % 4 == 0 && y % 100 != 0) || y % 400 == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hex_encode_basic() {
        assert_eq!(hex_encode(&[0x00, 0xff, 0xab]), "00ffab");
        assert_eq!(hex_encode(&[]), "");
    }

    #[test]
    fn unix_to_ymdhms_known() {
        // 2026-01-01T00:00:00Z = 1767225600 秒
        let (y, m, d, h, mi, s) = unix_to_ymdhms(1767225600);
        assert_eq!((y, m, d, h, mi, s), (2026, 1, 1, 0, 0, 0));
    }
}

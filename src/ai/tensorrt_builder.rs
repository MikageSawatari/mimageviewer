//! TensorRT エンジンビルダー (mikage 側で配布用 engine を生成するための CLI)。
//!
//! ## 用途
//!
//! mikage が `mimageviewer.exe --tensorrt-build <model_kind>` を実行すると、
//! 指定モデルの ONNX を読み込んで TensorRT engine をコンパイルし、結果を
//! `%APPDATA%/mimageviewer/tensorrt-engines/<model_kind>/` にキャッシュする。
//! これを `build_trt_pack` でまとめて配布パックに同梱する。
//!
//! Apr 28 のライセンス調査で「`nvinfer_builder_resource_*.dll` の再配布は
//! TensorRT SLA 上グレー」と判明したため、**ユーザー機での engine compile は
//! 廃止し、配布物には事前 build した engine をそのまま入れる方針**に切り替えた。
//! このバイナリ (= `--tensorrt-build` サブコマンド) はその事前 build 専用ツール。
//!
//! ## なぜ子プロセスか
//!
//! `ort::init_from()` はプロセス内 1 回限りなので、DirectML で動いている
//! メイン GUI 側で TRT 版 onnxruntime.dll をロードして engine だけ生成する、
//! ということができない。サブコマンドとして別プロセスで起動して、そのプロセス内で
//! TRT 版 ORT を初期化して仕事を終えれば良い。
//!
//! ## I/O フォーマット
//!
//! 引数: `mimageviewer.exe --tensorrt-build <model_kind>`
//! stdout: `[load|compile|done|error] <kind> <message>` 形式の人間可読ログ
//! 終了コード: 0 = 成功、1 = エラー

use super::ModelKind;

/// メインから子プロセスを起動するときに渡す引数。
pub const TRT_BUILD_ARG: &str = "--tensorrt-build";

/// `--tensorrt-build <model_kind>` で起動された子プロセスのエントリ。
/// 成功で exit 0、失敗で exit 1 を返す。
pub fn run_worker_process() -> ! {
    // 引数解析: --tensorrt-build <model_kind_str>
    let args: Vec<String> = std::env::args().collect();
    let kind_str = match args.iter().position(|a| a == TRT_BUILD_ARG) {
        Some(i) if i + 1 < args.len() => args[i + 1].clone(),
        _ => {
            eprintln!("usage: mimageviewer.exe {} <model_kind>", TRT_BUILD_ARG);
            std::process::exit(1);
        }
    };

    let kind = match ModelKind::from_str(&kind_str) {
        Some(k) => k,
        None => {
            eprintln!("unknown model_kind: {kind_str}");
            std::process::exit(1);
        }
    };

    // モデルパスの取得
    super::model_manager::ensure_models_extracted();
    let mm = super::model_manager::ModelManager::new();
    let model_path = match mm.model_path(kind) {
        Some(p) => p,
        None => {
            eprintln!("model file not found for {kind:?}");
            std::process::exit(1);
        }
    };

    println!("[load] {kind_str} from {}", model_path.display());

    // AiRuntime を TensorRT バックエンドで初期化。
    // この呼び出し自体は速い (DLL extract & init_from のみ)。
    let runtime = match super::runtime::AiRuntime::new_with_backend(super::AiBackend::TensorRt) {
        Ok(rt) => rt,
        Err(e) => {
            eprintln!("[error] {kind_str}: AiRuntime init failed: {e}");
            std::process::exit(1);
        }
    };

    let active = runtime.active_backend();
    if active.effective != super::AiBackend::TensorRt {
        eprintln!(
            "[error] {kind_str}: TensorRT backend unavailable, runtime fell back to {:?} ({})",
            active.effective,
            active.fallback_reason.as_deref().unwrap_or("unknown")
        );
        std::process::exit(1);
    }

    // load_model → session.commit_from_file が走り、形状情報が固定されると
    // engine の準備に入る。動的 shape のモデルは最初の session.run で完全に
    // コンパイルされるので、後段の warmup が必須。
    println!("[compile] {kind_str}");

    let t0 = std::time::Instant::now();
    if let Err(e) = runtime.load_model(kind, &model_path) {
        eprintln!("[error] {kind_str}: load_model failed: {e}");
        std::process::exit(1);
    }

    // ── warmup inference: runtime tile size でダミー session.run ──
    // 動的形状モデル (Real-ESRGAN / NMKD-Siax / RealCUGAN) は load_model だけでは
    // 完全に engine がコンパイルされず、最初の session.run() で shape 単位の
    // 最終コンパイルが走る。配布用 engine では「shape 込みでフルコンパイル済み」
    // にしておきたいので、ここで強制的に session.run を走らせる。
    let (n, c, h, w) = warmup_input_shape(kind);
    let dummy = ndarray::Array4::<f32>::zeros((n, c, h, w));
    let warmup_result = runtime.with_session(kind, |session| {
        let tensor = ort::value::Tensor::from_array(dummy)
            .map_err(|e| super::AiError::Ort(format!("warmup tensor: {e}")))?;
        let _ = session
            .run(ort::inputs![tensor])
            .map_err(|e| super::AiError::Ort(format!("warmup session.run: {e}")))?;
        Ok(())
    });
    if let Err(e) = warmup_result {
        eprintln!("[error] {kind_str}: warmup inference failed: {e}");
        std::process::exit(1);
    }

    let elapsed_ms = t0.elapsed().as_millis() as u64;
    let cache_dir = super::tensorrt_pack::engine_cache_dir().join(kind.as_str());
    println!(
        "[done] {kind_str}: {elapsed_ms} ms, cache={}",
        cache_dir.display()
    );
    std::process::exit(0);
}

/// 各 ModelKind に対する warmup 推論の入力テンソル shape (N, C, H, W)。
///
/// runtime で実際に呼ばれる shape と一致させる必要がある。一致していないと
/// TRT engine が runtime 1 タイル目で再コンパイルされ、ユーザーが体感する
/// 30〜60 秒の hang が発生する (Apr 28 のユーザー報告で発覚)。
///
/// shape の出どころ:
/// - `ClassifierMobileNet`: `ai/classify.rs::preprocess` の `SIZE = 384`
/// - `DenoiseRealplksr`: 256×256 固定 (モデル仕様)
/// - `InpaintMiGan`: `ui_erase.rs::MIGAN_SIZE = 512`、4 ch (RGB + mask)
/// - `UpscaleRealEsrGeneralV3`: TRT tile = 512 (`upscale.rs::model_tile_size`)
/// - その他アップスケール: TRT tile = 256
fn warmup_input_shape(kind: ModelKind) -> (usize, usize, usize, usize) {
    match kind {
        ModelKind::ClassifierMobileNet => (1, 3, 384, 384),
        ModelKind::DenoiseRealplksr => (1, 3, 256, 256),
        ModelKind::InpaintMiGan => (1, 4, 512, 512),
        ModelKind::UpscaleRealEsrGeneralV3 => (1, 3, 512, 512),
        ModelKind::UpscaleRealEsrganX4Plus
        | ModelKind::UpscaleRealEsrganAnime6B
        | ModelKind::UpscaleRealCugan4x
        | ModelKind::UpscaleNmkdSiax4x => (1, 3, 256, 256),
    }
}

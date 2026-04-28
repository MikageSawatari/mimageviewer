//! TensorRT エンジンビルダー (子プロセスワーカー + 親プロセス側ランチャー)。
//!
//! ## なぜ子プロセスか
//!
//! TensorRT のエンジンコンパイルは初回 30 秒〜2 分かかる重い処理で、
//! 完了後は engine cache に保存されて 2 回目以降は瞬時ロードされる。
//! このコンパイルを子プロセスで実行する理由:
//!
//! - `ort::init_from()` がプロセス内 1 回限りなので、メインプロセスが
//!   DirectML で動いている状態で TRT パックの onnxruntime.dll を
//!   ロードして engine だけ生成、ということができない。子プロセスなら
//!   そのプロセス内で TRT 版 ORT を初期化して仕事を終えれば良い。
//! - キャンセル時に process kill で確実に止められる (UI の応答性確保)。
//! - クラッシュしてもメイン GUI が落ちない。
//! - PDFium ワーカーと同じパターンで実装が読みやすい。
//!
//! ## ワイヤフォーマット
//!
//! 親 → 子: コマンドライン引数のみ (`mimageviewer.exe --tensorrt-build <model_kind>`)
//! 子 → 親: stdout に行単位 JSON を書く (TrtBuildEvent をシリアライズしたもの)
//!
//! 終了コード: 0 = 成功、1 = エラー

use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Command, Stdio};

use serde::{Deserialize, Serialize};

use super::ModelKind;

/// メインから子プロセスを起動するときに渡す引数。
pub const TRT_BUILD_ARG: &str = "--tensorrt-build";

/// 子 → 親の進捗イベント (JSON 行で stdout に書き出される)。
/// 親側は serde_json::from_str で 1 行ずつパースする。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "phase", rename_all = "snake_case")]
pub enum TrtBuildEvent {
    /// onnxruntime.dll とモデルファイルのロード開始
    Loading {
        model_kind: String,
        model_path: String,
    },
    /// ORT セッション構築開始 (= TensorRT engine compile が始まる)
    /// この後 30 秒〜数分のブロッキングが続く
    Compiling { model_kind: String },
    /// 成功
    Done {
        model_kind: String,
        elapsed_ms: u64,
        cache_path: String,
    },
    /// 失敗
    Error {
        model_kind: String,
        message: String,
    },
}

// ─────────────────────────────────────────────────────────────
// 子プロセス側: ワーカーエントリ
// ─────────────────────────────────────────────────────────────

/// `--tensorrt-build <model_kind>` で起動された子プロセスのエントリ。
/// 成功で exit 0、失敗で exit 1 を返す。
///
/// stdout には行単位の JSON が書き出される (親が読み取る)。stderr は
/// エラー時の人間可読メッセージ用。
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

    // 子プロセス内では logger は最低限 (進捗 JSON だけは確実に stdout に出したいので
    // バッファリング無効にする — Python の `print(flush=True)` 相当)
    let stdout = std::io::stdout();
    let mut out_lock = stdout.lock();
    let mut emit = |ev: &TrtBuildEvent| {
        use std::io::Write;
        if let Ok(s) = serde_json::to_string(ev) {
            let _ = writeln!(out_lock, "{s}");
            let _ = out_lock.flush();
        }
    };

    // モデルパスの取得
    super::model_manager::ensure_models_extracted();
    let mm = super::model_manager::ModelManager::new();
    let model_path = match mm.model_path(kind) {
        Some(p) => p,
        None => {
            emit(&TrtBuildEvent::Error {
                model_kind: kind_str.clone(),
                message: format!("model file not found for {kind:?}"),
            });
            std::process::exit(1);
        }
    };

    emit(&TrtBuildEvent::Loading {
        model_kind: kind_str.clone(),
        model_path: model_path.display().to_string(),
    });

    // AiRuntime を TensorRT バックエンドで初期化。
    // この呼び出し自体は速い (DLL extract & init_from のみ)。
    let runtime = match super::runtime::AiRuntime::new_with_backend(super::AiBackend::TensorRt, true)
    {
        Ok(rt) => rt,
        Err(e) => {
            emit(&TrtBuildEvent::Error {
                model_kind: kind_str.clone(),
                message: format!("AiRuntime init failed: {e}"),
            });
            std::process::exit(1);
        }
    };

    let active = runtime.active_backend();
    if active.effective != super::AiBackend::TensorRt {
        emit(&TrtBuildEvent::Error {
            model_kind: kind_str.clone(),
            message: format!(
                "TensorRT backend unavailable, runtime fell back to {:?} ({})",
                active.effective,
                active.fallback_reason.as_deref().unwrap_or("unknown")
            ),
        });
        std::process::exit(1);
    }

    // load_model() の中で session.commit_from_file() が呼ばれ、これが engine compile を
    // 走らせる。所要時間: 初回 30 秒〜数分、エンジンキャッシュ HIT 時は数百 ms。
    emit(&TrtBuildEvent::Compiling {
        model_kind: kind_str.clone(),
    });

    let t0 = std::time::Instant::now();
    if let Err(e) = runtime.load_model(kind, &model_path) {
        emit(&TrtBuildEvent::Error {
            model_kind: kind_str.clone(),
            message: format!("load_model failed: {e}"),
        });
        std::process::exit(1);
    }
    let elapsed_ms = t0.elapsed().as_millis() as u64;

    let cache_dir = super::tensorrt_pack::engine_cache_dir().join(kind.as_str());
    emit(&TrtBuildEvent::Done {
        model_kind: kind_str,
        elapsed_ms,
        cache_path: cache_dir.display().to_string(),
    });
    std::process::exit(0);
}

// ─────────────────────────────────────────────────────────────
// 親プロセス側: 子プロセスを起動して進捗をコールバックする
// ─────────────────────────────────────────────────────────────

/// 1 モデル分のエンジンビルドを子プロセスで実行する。
///
/// `on_event` は stdout から読んだ各 JSON 行ごとに呼ばれる。
/// 戻り値: ビルド成功で `Ok(elapsed_ms)`、失敗で `Err(message)`。
pub fn build_engine_for(
    kind: ModelKind,
    mut on_event: impl FnMut(&TrtBuildEvent),
) -> Result<u64, String> {
    let exe = std::env::current_exe()
        .map_err(|e| format!("current_exe: {e}"))?;
    let mut child = Command::new(&exe)
        .arg(TRT_BUILD_ARG)
        .arg(kind.as_str())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| format!("spawn child: {e}"))?;

    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| "child stdout missing".to_string())?;
    let reader = BufReader::new(stdout);

    let mut last_done_ms: Option<u64> = None;
    let mut last_error: Option<String> = None;
    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                crate::logger::log(format!(
                    "[TRT-builder] stdout read error for {kind:?}: {e}"
                ));
                continue;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        match serde_json::from_str::<TrtBuildEvent>(&line) {
            Ok(ev) => {
                match &ev {
                    TrtBuildEvent::Done { elapsed_ms, .. } => last_done_ms = Some(*elapsed_ms),
                    TrtBuildEvent::Error { message, .. } => last_error = Some(message.clone()),
                    _ => {}
                }
                on_event(&ev);
            }
            Err(e) => {
                crate::logger::log(format!(
                    "[TRT-builder] unparseable stdout line for {kind:?}: {line:?} ({e})"
                ));
            }
        }
    }

    // 子プロセス終了待ち + stderr 取得
    let status = child.wait().map_err(|e| format!("wait: {e}"))?;
    let stderr_text = child
        .stderr
        .as_mut()
        .map(|s| {
            use std::io::Read;
            let mut buf = String::new();
            let _ = s.read_to_string(&mut buf);
            buf
        })
        .unwrap_or_default();

    if !status.success() {
        let msg = match last_error {
            Some(m) => m,
            None if !stderr_text.trim().is_empty() => stderr_text.trim().to_string(),
            None => format!("child exited with {status}"),
        };
        return Err(msg);
    }
    last_done_ms.ok_or_else(|| "child finished without Done event".to_string())
}

/// 全 ModelKind のエンジンビルドを順次実行する。
/// 既にエンジンキャッシュが温まっているモデルは load_model が瞬時に終わる
/// (数百 ms) ので、未ビルドのものだけが 30 秒〜数分かかる。
///
/// `on_progress` は (current_index, total, current_kind, event) で呼ばれる。
#[allow(dead_code)] // Phase 2-full step 3 (UI ダイアログ) から呼ばれる
pub fn build_all_engines(
    kinds: &[ModelKind],
    mut on_progress: impl FnMut(usize, usize, ModelKind, &TrtBuildEvent),
) -> Result<Vec<(ModelKind, u64)>, (ModelKind, String)> {
    let total = kinds.len();
    let mut results = Vec::with_capacity(total);
    for (i, &kind) in kinds.iter().enumerate() {
        let mut last_event_ms = 0u64;
        let r = build_engine_for(kind, |ev| {
            on_progress(i, total, kind, ev);
            if let TrtBuildEvent::Done { elapsed_ms, .. } = ev {
                last_event_ms = *elapsed_ms;
            }
        });
        match r {
            Ok(ms) => results.push((kind, ms)),
            Err(msg) => return Err((kind, msg)),
        }
        let _ = last_event_ms;
    }
    Ok(results)
}

#[allow(dead_code)] // Phase 2-full step 3 で使う
pub fn engine_cache_path_for(kind: ModelKind) -> PathBuf {
    super::tensorrt_pack::engine_cache_dir().join(kind.as_str())
}

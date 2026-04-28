//! TensorRT 推論ワーカープロセス側のメインループ。
//!
//! `mimageviewer.exe --tensorrt-infer-worker` で起動された子プロセスが
//! 走らせる。stdin から JSON コマンドを 1 行ずつ読み、TRT バックエンドで
//! セッションをロードして推論し、結果を stdout に書く。
//!
//! Step 1 (現状) では `LoadModel` / `Shutdown` のみ実装。`Infer` は Step 2 で追加。
//!
//! ## ライフサイクル
//!
//! 1. 親が `Command::spawn(mimageviewer.exe --tensorrt-infer-worker)`
//! 2. 子: data_dir::init → ORT を TRT pack で初期化 → ハンドシェイク待機
//! 3. ループ:
//!    - stdin から WorkerCmd を読む
//!    - LoadModel: AiRuntime::load_model でセッション作成 (engine cache HIT で速い)
//!    - Infer: shm から入力読み、推論、shm に結果書き、Resp 返す (Step 2)
//!    - Shutdown: ループ抜け
//! 4. AiRuntime drop → ORT セッション破棄 → exit 0

use std::io::{BufRead, BufReader, Write};

use super::trt_worker_proto::{TRT_INFER_WORKER_ARG, WorkerCmd, WorkerResp};
use super::{AiBackend, ModelKind};

/// 子プロセス側のエントリ。`main.rs` から `--tensorrt-infer-worker` 引数で
/// 呼ばれたときに実行される。`!` (never) を返す = 関数内で `std::process::exit`。
pub fn run_infer_worker() -> ! {
    // logger は親プロセスで初期化されているとは限らないので、子プロセス独自に
    // init する。ログは %APPDATA%/mimageviewer/logs/mimageviewer.log に追記。
    crate::logger::init();
    crate::logger::log(format!(
        "[TRT-worker] 起動 (pid={})",
        std::process::id()
    ));

    // ORT を TensorRT バックエンドで初期化。pack 不在なら DirectML フォールバック
    // するが、その場合は親が DirectML 経路で直接動かすべきなので、ここで TRT
    // フォールバックが起きたらエラーを返してすぐ exit する。
    let runtime = match super::runtime::AiRuntime::new_with_backend(AiBackend::TensorRt) {
        Ok(rt) => rt,
        Err(e) => {
            emit_resp(&WorkerResp::err(format!("AiRuntime init failed: {e}")));
            crate::logger::log(format!("[TRT-worker] AiRuntime init 失敗: {e}"));
            std::process::exit(1);
        }
    };

    let active = runtime.active_backend();
    if active.effective != AiBackend::TensorRt {
        emit_resp(&WorkerResp::err(format!(
            "TensorRT pack 未利用、effective={:?} (理由: {})",
            active.effective,
            active.fallback_reason.as_deref().unwrap_or("不明")
        )));
        crate::logger::log(format!(
            "[TRT-worker] TRT で初期化されず、effective={:?}",
            active.effective
        ));
        std::process::exit(1);
    }

    // ハンドシェイク: 起動成功を親に通知
    emit_resp(&WorkerResp::ok_simple(0));
    crate::logger::log("[TRT-worker] 初期化完了、コマンド待機開始".to_string());

    let mm = super::model_manager::ModelManager::new();

    let stdin = std::io::stdin();
    let stdin_lock = stdin.lock();
    let reader = BufReader::new(stdin_lock);

    for line in reader.lines() {
        let line = match line {
            Ok(l) => l,
            Err(e) => {
                crate::logger::log(format!("[TRT-worker] stdin 読込エラー: {e}"));
                break;
            }
        };
        if line.trim().is_empty() {
            continue;
        }
        let cmd: WorkerCmd = match serde_json::from_str(&line) {
            Ok(c) => c,
            Err(e) => {
                emit_resp(&WorkerResp::err(format!(
                    "コマンド JSON のパースに失敗: {e} ({line:?})"
                )));
                continue;
            }
        };

        match cmd {
            WorkerCmd::LoadModel { kind } => {
                let resp = handle_load_model(&runtime, &mm, &kind);
                emit_resp(&resp);
            }
            WorkerCmd::Infer { kind, .. } => {
                // Step 2 で実装。現時点ではエラーを返す。
                emit_resp(&WorkerResp::err(format!(
                    "infer は Step 2 で実装予定 (kind={kind})"
                )));
            }
            WorkerCmd::Shutdown => {
                emit_resp(&WorkerResp::ok_simple(0));
                crate::logger::log("[TRT-worker] Shutdown 受信、終了".to_string());
                break;
            }
        }
    }

    // ここまで来たら明示的に exit 0
    std::process::exit(0);
}

fn handle_load_model(
    runtime: &super::runtime::AiRuntime,
    mm: &super::model_manager::ModelManager,
    kind_str: &str,
) -> WorkerResp {
    let Some(kind) = ModelKind::from_str(kind_str) else {
        return WorkerResp::err(format!("unknown model_kind: {kind_str}"));
    };
    let model_path = match mm.model_path(kind) {
        Some(p) => p,
        None => {
            return WorkerResp::err(format!("model file not found for {kind:?}"));
        }
    };

    let t0 = std::time::Instant::now();
    if let Err(e) = runtime.load_model(kind, &model_path) {
        return WorkerResp::err(format!("load_model failed: {e}"));
    }
    let elapsed_ms = t0.elapsed().as_millis() as u64;
    crate::logger::log(format!(
        "[TRT-worker] LoadModel {kind_str}: {elapsed_ms} ms"
    ));
    WorkerResp::ok_simple(elapsed_ms)
}

/// レスポンスを stdout に行単位 JSON で書く。flush も毎回。
fn emit_resp(resp: &WorkerResp) {
    let stdout = std::io::stdout();
    let mut out = stdout.lock();
    if let Ok(s) = serde_json::to_string(resp) {
        let _ = writeln!(out, "{s}");
        let _ = out.flush();
    }
}

/// `main.rs` から呼ぶ判定: 子プロセスならこれが true。
pub fn is_worker_invocation() -> bool {
    std::env::args().any(|a| a == TRT_INFER_WORKER_ARG)
}

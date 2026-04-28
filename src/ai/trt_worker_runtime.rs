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

use super::trt_worker_proto::{TRT_INFER_WORKER_ARG, WorkerCmd, WorkerInferBreakdown, WorkerResp};
use super::{AiBackend, ModelKind};

/// 子プロセス側のエントリ。`main.rs` から `--tensorrt-infer-worker` 引数で
/// 呼ばれたときに実行される。`!` (never) を返す = 関数内で `std::process::exit`。
pub fn run_infer_worker() -> ! {
    // logger は親プロセスで初期化されているとは限らないので、子プロセス独自に
    // init する。**worker は専用ファイル `trt-worker.log` に append**
    // (= 親の mimageviewer.log を truncate しない)。
    crate::logger::init_for_worker("trt-worker");
    crate::logger::log(format!(
        "[TRT-worker] 起動 (pid={})",
        std::process::id()
    ));

    // Windows: プロセス優先度を HIGH に設定。子プロセスのデフォルトは NORMAL で、
    // Phase 3 計測で worker session.run が main 比 +15 ms/tile かかる原因として
    // 「CUDA scheduling や Win32 スケジューラ粒度の影響」を疑い、検証のため設定。
    #[cfg(windows)]
    unsafe {
        use windows::Win32::System::Threading::{
            GetCurrentProcess, HIGH_PRIORITY_CLASS, SetPriorityClass,
        };
        let h = GetCurrentProcess();
        match SetPriorityClass(h, HIGH_PRIORITY_CLASS) {
            Ok(_) => {
                crate::logger::log(
                    "[TRT-worker] process priority = HIGH に設定".to_string(),
                );
            }
            Err(e) => {
                crate::logger::log(format!(
                    "[TRT-worker] SetPriorityClass(HIGH) 失敗: {e:?}"
                ));
            }
        }
    }

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

    // 永続共有メモリのキャッシュ。Infer cmd で渡される shm 名が同じなら
    // 開き直さない (親側も pool 起動時の 1 回だけ create するので名前は固定)。
    #[cfg(windows)]
    let mut shm_cache: ShmCache = ShmCache::new();

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
            WorkerCmd::Infer {
                kind,
                input_shm,
                input_bytes,
                input_shape,
                output_shm,
                output_capacity,
            } => {
                #[cfg(windows)]
                let resp = handle_infer(
                    &runtime,
                    &kind,
                    &input_shm,
                    input_bytes,
                    &input_shape,
                    &output_shm,
                    output_capacity,
                    &mut shm_cache,
                );
                #[cfg(not(windows))]
                let resp = handle_infer(
                    &runtime,
                    &kind,
                    &input_shm,
                    input_bytes,
                    &input_shape,
                    &output_shm,
                    output_capacity,
                );
                emit_resp(&resp);
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

/// 共有メモリのキャッシュ (worker 側、永続 shm を使い回す)。
/// 親が pool 起動時に create した shm を最初の Infer で open し、以降同じ名前で
/// 来たら再 open しない。
#[cfg(windows)]
struct ShmCache {
    in_shm: Option<(String, super::trt_worker_shm::SharedMem)>,
    out_shm: Option<(String, super::trt_worker_shm::SharedMem)>,
}

#[cfg(windows)]
impl ShmCache {
    fn new() -> Self {
        Self {
            in_shm: None,
            out_shm: None,
        }
    }

    /// 名前 + サイズが既存キャッシュと一致したら既存 SharedMem を返す。
    /// 一致しない (初回 or 名前変更) なら open + キャッシュ更新。
    fn get_or_open<'a>(
        slot: &'a mut Option<(String, super::trt_worker_shm::SharedMem)>,
        name: &str,
        size: usize,
    ) -> Result<&'a mut super::trt_worker_shm::SharedMem, String> {
        let needs_open = match slot.as_ref() {
            Some((cached_name, cached_shm)) => {
                cached_name != name || cached_shm.size() != size
            }
            None => true,
        };
        if needs_open {
            let shm = super::trt_worker_shm::SharedMem::open(name, size)
                .map_err(|e| format!("open '{name}' size={size}: {e}"))?;
            *slot = Some((name.to_string(), shm));
        }
        Ok(&mut slot.as_mut().unwrap().1)
    }
}

/// Infer コマンド処理: 共有メモリ経由で入力を受け、推論し、結果を共有メモリに書く。
///
/// 入出力ともに NCHW float32。共有メモリは生バイト列として扱い、`unsafe`
/// `std::slice::from_raw_parts` で f32 として解釈する (ページ整列されているので
/// f32 alignment は満たされる)。
///
/// `shm_cache` で永続 shm を使い回す。同じ name + size で来た場合は再 open しない。
#[cfg(windows)]
fn handle_infer(
    runtime: &super::runtime::AiRuntime,
    kind_str: &str,
    input_shm: &str,
    input_bytes: usize,
    input_shape: &[i64],
    output_shm: &str,
    output_capacity: usize,
    shm_cache: &mut ShmCache,
) -> WorkerResp {
    let Some(kind) = ModelKind::from_str(kind_str) else {
        return WorkerResp::err(format!("unknown model_kind: {kind_str}"));
    };

    // 形状チェック
    if input_shape.len() != 4 {
        return WorkerResp::err(format!(
            "input_shape は 4 次元 NCHW でなければならない (got {} 次元)",
            input_shape.len()
        ));
    }
    let shape_count: i64 = input_shape.iter().product();
    let expected_bytes = shape_count as usize * 4;
    if expected_bytes != input_bytes {
        return WorkerResp::err(format!(
            "shape product * 4 ({expected_bytes}) != input_bytes ({input_bytes})"
        ));
    }

    // 共有メモリは ShmCache で使い回す (毎回 open しない、起動時 1 回 + キャッシュ)。
    let in_shm = match ShmCache::get_or_open(&mut shm_cache.in_shm, input_shm, PERSIST_IN_SHM_SIZE_HINT.max(input_bytes)) {
        Ok(s) => s,
        Err(e) => return WorkerResp::err(format!("get_or_open input shm: {e}")),
    };

    // ── 計装 phase 1: 入力 shm 読み込み (Vec<f32>) ──
    let t_phase = std::time::Instant::now();
    let f32_count = input_bytes / 4;
    let input_data: Vec<f32> = unsafe {
        let bytes = in_shm.as_slice(input_bytes);
        std::slice::from_raw_parts(bytes.as_ptr() as *const f32, f32_count).to_vec()
    };
    let read_input_ms = t_phase.elapsed().as_secs_f64() * 1000.0;

    // ── 計装 phase 2: ndarray::Array4 + ort::Tensor 構築 ──
    let t_phase = std::time::Instant::now();
    let shape = (
        input_shape[0] as usize,
        input_shape[1] as usize,
        input_shape[2] as usize,
        input_shape[3] as usize,
    );
    let array = match ndarray::Array4::from_shape_vec(shape, input_data) {
        Ok(a) => a,
        Err(e) => return WorkerResp::err(format!("ndarray reshape: {e}")),
    };
    let tensor = match ort::value::Tensor::from_array(array) {
        Ok(t) => t,
        Err(e) => return WorkerResp::err(format!("Tensor::from_array: {e}")),
    };
    let tensor_build_ms = t_phase.elapsed().as_secs_f64() * 1000.0;

    // ── 計装 phase 3+4: session.run() (純粋) + extract + 出力 shm 書き込み ──
    // closure 内では session.run のみを別個に計測し、extract+write は外で計測する
    // ことで GPU 純粋時間と CPU 後処理時間を分離する。
    //
    // 環境変数 MIV_TRT_BENCH_LOOP を設定すると、同じ tensor で session.run を N 回
    // 連続実行して時間を計測 (Phase 3 調査用、最後の output だけ shm に書く)。
    // 結果は logger に出力。N >= 2 で「IPC 間隔が原因か process-level か」が
    // 分かる: 1st と 2nd 以降の差が大きければ IPC 間隔起因 (= GPU sleep)、
    // 全部同じなら process-level の何か (= ORT/CUDA の固定 cost)。
    let bench_loop_n: usize = std::env::var("MIV_TRT_BENCH_LOOP")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(1);
    let t_total = std::time::Instant::now();
    let out_shm_slot = &mut shm_cache.out_shm;
    let mut session_run_ms_inner: f64 = 0.0;
    let result: Result<Vec<i64>, super::AiError> = runtime.with_session(kind, |session| {
        // phase 3: pure session.run() 計測 (オプションで連続 N 回)
        let t_run = std::time::Instant::now();
        let mut consec_times: Vec<f64> = Vec::with_capacity(bench_loop_n);
        // 最初の N-1 回は出力をすぐ drop して計測のみ (借用衝突回避)
        for _ in 0..bench_loop_n.saturating_sub(1) {
            let t_one = std::time::Instant::now();
            let _outputs = session
                .run(ort::inputs![tensor.view()])
                .map_err(|e| super::AiError::Ort(format!("session.run: {e}")))?;
            consec_times.push(t_one.elapsed().as_secs_f64() * 1000.0);
            // _outputs はこのスコープで drop
        }
        // 最後の 1 回 (loop_n=1 の場合は唯一の) — 出力を保持
        let t_one = std::time::Instant::now();
        let outputs = session
            .run(ort::inputs![tensor.view()])
            .map_err(|e| super::AiError::Ort(format!("session.run: {e}")))?;
        consec_times.push(t_one.elapsed().as_secs_f64() * 1000.0);
        session_run_ms_inner = t_run.elapsed().as_secs_f64() * 1000.0;
        if bench_loop_n > 1 {
            crate::logger::log(format!(
                "[TRT-worker] consecutive session.run times (n={}): {:?}",
                bench_loop_n, consec_times
            ));
        }

        // phase 4: extract + shm write
        let (shape, raw) = outputs[0]
            .try_extract_tensor::<f32>()
            .map_err(|e| super::AiError::Ort(format!("extract_tensor: {e}")))?;

        let output_shape: Vec<i64> = shape.iter().copied().collect();
        let total_count: i64 = output_shape.iter().product();
        let total_bytes = total_count as usize * 4;

        if total_bytes > output_capacity {
            return Err(super::AiError::Ort(format!(
                "output too large: needed {total_bytes} bytes, shm capacity {output_capacity}"
            )));
        }
        if raw.len() < total_count as usize {
            return Err(super::AiError::Ort(format!(
                "output tensor shorter than expected: {} < {}",
                raw.len(),
                total_count
            )));
        }

        let raw_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(raw.as_ptr() as *const u8, total_bytes)
        };
        let out_shm = ShmCache::get_or_open(out_shm_slot, output_shm, output_capacity)
            .map_err(|e| super::AiError::Ort(format!("get_or_open output shm: {e}")))?;
        out_shm.write(raw_bytes);
        Ok(output_shape)
    });

    let total_with_session_ms = t_total.elapsed().as_secs_f64() * 1000.0;
    // extract + shm write の時間 = with_session 全体 - 純粋 session_run 時間
    let extract_and_write_ms = (total_with_session_ms - session_run_ms_inner).max(0.0);

    match result {
        Ok(shape) => {
            let elapsed_ms = total_with_session_ms as u64;
            let breakdown = WorkerInferBreakdown {
                read_input_ms,
                tensor_build_ms,
                session_run_ms: session_run_ms_inner,
                extract_and_write_ms,
            };
            WorkerResp::ok_infer(elapsed_ms, shape, breakdown)
        }
        Err(e) => WorkerResp::err(format!("inference failed: {e}")),
    }
}

/// 永続入力 shm のヒントサイズ (親側 PERSIST_IN_SHM_SIZE と一致)。
/// 子側はこれをデフォルトサイズとして使い、入力 bytes が大きければそちらを使う。
#[cfg(windows)]
const PERSIST_IN_SHM_SIZE_HINT: usize = 4 * 1024 * 1024;

#[cfg(not(windows))]
fn handle_infer(
    _runtime: &super::runtime::AiRuntime,
    _kind_str: &str,
    _input_shm: &str,
    _input_bytes: usize,
    _input_shape: &[i64],
    _output_shm: &str,
    _output_capacity: usize,
) -> WorkerResp {
    WorkerResp::err("Infer は Windows 専用 (共有メモリ依存)".to_string())
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

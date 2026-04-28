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
    // SharedMem は同名で複数回 open 可能だが、syscall コストを避けるため cache する。
    let in_shm = match ShmCache::get_or_open(&mut shm_cache.in_shm, input_shm, PERSIST_IN_SHM_SIZE_HINT.max(input_bytes)) {
        Ok(s) => s,
        Err(e) => return WorkerResp::err(format!("get_or_open input shm: {e}")),
    };

    // 入力 bytes → Vec<f32> (shm 領域からコピー、Drop と独立に保持するため)
    let input_data: Vec<f32> = {
        let bytes = in_shm.read_to_vec(input_bytes);
        let f32_count = input_bytes / 4;
        // SAFETY: bytes は Vec<u8> で 8-byte aligned (Rust global allocator)、
        // length は 4 の倍数。f32 (4-byte aligned) として解釈安全。
        let f32_slice: &[f32] = unsafe {
            std::slice::from_raw_parts(bytes.as_ptr() as *const f32, f32_count)
        };
        f32_slice.to_vec()
    };
    // ※ 出力 shm は推論後に書き込むので、まだ open しない (借用エラー回避)。

    // ndarray::Array4 構築
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

    // 推論実行 (with_session 内で session.run + extract_tensor + bytes 書き込み)
    let t0 = std::time::Instant::now();
    let result = runtime.with_session(kind, |session| {
        let outputs = session
            .run(ort::inputs![tensor])
            .map_err(|e| super::AiError::Ort(format!("session.run: {e}")))?;
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
                "output tensor returned shorter than expected: raw.len()={} expected={}",
                raw.len(),
                total_count
            )));
        }

        // raw (f32 slice) → bytes view → shm に直接コピー
        // SAFETY: raw は ORT 内部のテンソルバッファへの参照、
        // f32 アライメントが揃っている。len * 4 バイトの U8 view として読める。
        let raw_bytes: &[u8] = unsafe {
            std::slice::from_raw_parts(
                raw.as_ptr() as *const u8,
                total_count as usize * std::mem::size_of::<f32>(),
            )
        };
        Ok((output_shape, raw_bytes.to_vec()))
    });

    let elapsed_ms = t0.elapsed().as_millis() as u64;

    match result {
        Ok((shape, out_bytes)) => {
            // 出力 shm を cache から取得 (なければ open)
            let out_shm = match ShmCache::get_or_open(&mut shm_cache.out_shm, output_shm, output_capacity) {
                Ok(s) => s,
                Err(e) => {
                    return WorkerResp::err(format!("get_or_open output shm: {e}"));
                }
            };
            out_shm.write(&out_bytes);
            WorkerResp::ok_infer(elapsed_ms, shape)
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

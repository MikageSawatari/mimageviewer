//! TensorRT 推論ワーカーの親プロセス側プール。
//!
//! Step 1 (現状): スポーン + ハンドシェイク + LoadModel + Shutdown のみ。
//! Step 2 で Infer を追加。
//!
//! ## 構造
//!
//! - `TrtWorkerPool`: 1 個のワーカー子プロセスを管理 (現状は 1 ワーカー固定。
//!   将来複数並列化したくなったら拡張可能)。
//! - `WorkerHandle`: 子プロセスの stdin/stdout + Child handle を保持。
//! - すべての通信は親側の Mutex でシリアライズ (1 度に 1 コマンドのみ in-flight)。
//!
//! ## ライフサイクル
//!
//! ```text
//! TrtWorkerPool::start()
//!   └─ spawn child mimageviewer.exe --tensorrt-infer-worker
//!       └─ 子: AiRuntime::new_with_backend(TensorRt) → 起動成功 Resp
//!   └─ 親: 起動 Resp を待つ → start() 完了
//!
//! pool.send_cmd(LoadModel { kind })
//!   └─ stdin に書く → stdout から Resp を読む
//!
//! pool.shutdown() (Drop でも自動)
//!   └─ stdin に Shutdown 送信 → 子 exit → child.wait()
//! ```

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};
use std::sync::Mutex;

use super::trt_worker_proto::{TRT_INFER_WORKER_ARG, WorkerCmd, WorkerResp};

/// 1 個のワーカー子プロセスのハンドル。
struct WorkerHandle {
    child: Child,
    stdin: ChildStdin,
    stdout: BufReader<ChildStdout>,
}

impl WorkerHandle {
    /// 親プロセス側でワーカー子プロセスを起動し、ハンドシェイクを待つ。
    fn spawn(exe: &std::path::Path) -> Result<Self, String> {
        let mut child = Command::new(exe)
            .arg(TRT_INFER_WORKER_ARG)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| format!("spawn worker: {e}"))?;

        let stdin = child
            .stdin
            .take()
            .ok_or_else(|| "child stdin missing".to_string())?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| "child stdout missing".to_string())?;
        let mut stdout = BufReader::new(stdout);

        // ハンドシェイク: 子の起動成功 / 失敗のレスポンスを 1 行読む。
        // pack 不在 + DirectML フォールバック等の場合は失敗で来る。
        let resp = read_resp_line(&mut stdout)?;
        if !resp.ok {
            // 子は exit するはずなので wait しておく
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "ワーカー初期化失敗: {}",
                resp.error.unwrap_or_else(|| "(理由不明)".to_string())
            ));
        }

        Ok(WorkerHandle {
            child,
            stdin,
            stdout,
        })
    }

    fn send_cmd(&mut self, cmd: &WorkerCmd) -> Result<WorkerResp, String> {
        let s = serde_json::to_string(cmd)
            .map_err(|e| format!("cmd serialize: {e}"))?;
        writeln!(self.stdin, "{s}").map_err(|e| format!("stdin write: {e}"))?;
        self.stdin
            .flush()
            .map_err(|e| format!("stdin flush: {e}"))?;
        read_resp_line(&mut self.stdout)
    }

    fn shutdown_and_wait(mut self) {
        // Shutdown コマンドを送って Resp を待ってから wait。
        // 失敗しても child を kill して回収。
        let _ = self.send_cmd(&WorkerCmd::Shutdown);
        match self.child.wait() {
            Ok(status) => crate::logger::log(format!(
                "[TRT-worker-pool] worker exited: {status:?}"
            )),
            Err(e) => crate::logger::log(format!("[TRT-worker-pool] wait failed: {e}")),
        }
    }
}

fn read_resp_line(reader: &mut BufReader<ChildStdout>) -> Result<WorkerResp, String> {
    let mut line = String::new();
    let n = reader
        .read_line(&mut line)
        .map_err(|e| format!("stdout read: {e}"))?;
    if n == 0 {
        return Err("worker stdout が EOF (子プロセスが予期せず終了した可能性)".to_string());
    }
    let trimmed = line.trim();
    if trimmed.is_empty() {
        return Err("worker から空レスポンス".to_string());
    }
    serde_json::from_str::<WorkerResp>(trimmed)
        .map_err(|e| format!("resp parse: {e} (raw: {line:?})"))
}

/// 永続共有メモリのサイズ (各 tile で create/open し直さず使い回す)。
/// アップスケール最大 (512x512 入力 × 4x スケール × 3 ch × f32) を考慮:
///   入力: 512² × 3 × 4 = 3 MB
///   出力: 2048² × 3 × 4 = 48 MB
/// 余裕を持って 4 MB / 64 MB を確保する。
const PERSIST_IN_SHM_SIZE: usize = 4 * 1024 * 1024;
const PERSIST_OUT_SHM_SIZE: usize = 64 * 1024 * 1024;

/// アプリ全体で 1 つだけ持つワーカープール。`Arc<TrtWorkerPool>` で共有。
///
/// 現状は 1 ワーカー固定。複数並列化が必要になったら `Vec<WorkerHandle>` に
/// 拡張する (まだ複数同時に走らせる必要はない、推論は per-tile で sequential)。
///
/// 共有メモリは pool 起動時に 1 回 create して以降使い回す。各 infer で
/// create/open し直すよりも、syscall 数が大幅に減って per-tile IPC overhead が
/// 半減する (実測 ~37 ms → ~20 ms 目標)。
#[cfg(windows)]
pub struct TrtWorkerPool {
    worker: Mutex<Option<WorkerHandle>>,
    /// 永続入力共有メモリ。pool 起動時に 1 回 create、Drop で破棄。
    in_shm: Mutex<Option<super::trt_worker_shm::SharedMem>>,
    /// 永続出力共有メモリ。同上。
    out_shm: Mutex<Option<super::trt_worker_shm::SharedMem>>,
    /// 共有メモリ名 (子に伝えるため)
    in_shm_name: String,
    out_shm_name: String,
}

#[cfg(not(windows))]
pub struct TrtWorkerPool {
    worker: Mutex<Option<WorkerHandle>>,
}

impl TrtWorkerPool {
    /// プールを起動 (現在の exe をワーカーとして spawn)。
    /// メインアプリ (mimageviewer.exe) から呼ぶ用途。
    pub fn start() -> Result<Self, String> {
        let exe = std::env::current_exe()
            .map_err(|e| format!("current_exe: {e}"))?;
        Self::start_with_exe(&exe)
    }

    /// プールを起動 (任意の exe パスをワーカーとして spawn)。
    /// `bench_ai` 等の別 bin から呼ぶときは、sibling の `mimageviewer.exe` を
    /// 指定する用途。指定 exe には `--tensorrt-infer-worker` 引数が付くので
    /// 当該サブコマンド処理を持つバイナリでなければならない。
    #[cfg(windows)]
    pub fn start_with_exe(exe: &std::path::Path) -> Result<Self, String> {
        use super::trt_worker_proto::shm_name;
        use super::trt_worker_shm::SharedMem;

        crate::logger::log(format!(
            "[TRT-worker-pool] 起動中... worker={}",
            exe.display()
        ));
        let worker = WorkerHandle::spawn(exe)?;

        // 永続共有メモリを pool 起動時に 1 回だけ確保。worker 側も最初の Infer で
        // open + cache する (毎回 open しない)。
        let pid = std::process::id();
        let in_shm_name = shm_name("in", pid, 0);
        let out_shm_name = shm_name("out", pid, 0);
        let in_shm = SharedMem::create(&in_shm_name, PERSIST_IN_SHM_SIZE)
            .map_err(|e| format!("create persistent in_shm: {e}"))?;
        let out_shm = SharedMem::create(&out_shm_name, PERSIST_OUT_SHM_SIZE)
            .map_err(|e| format!("create persistent out_shm: {e}"))?;

        crate::logger::log(format!(
            "[TRT-worker-pool] 起動完了 (persistent shm: in={} ({} MiB), out={} ({} MiB))",
            in_shm_name,
            PERSIST_IN_SHM_SIZE / 1024 / 1024,
            out_shm_name,
            PERSIST_OUT_SHM_SIZE / 1024 / 1024,
        ));
        Ok(Self {
            worker: Mutex::new(Some(worker)),
            in_shm: Mutex::new(Some(in_shm)),
            out_shm: Mutex::new(Some(out_shm)),
            in_shm_name,
            out_shm_name,
        })
    }

    #[cfg(not(windows))]
    pub fn start_with_exe(_exe: &std::path::Path) -> Result<Self, String> {
        Err("TRT worker pool は Windows 専用".to_string())
    }

    /// 指定モデルをワーカー側でロードする (engine cache HIT で速い)。
    pub fn load_model(&self, kind: super::ModelKind) -> Result<u64, String> {
        let mut guard = self.worker.lock().map_err(|_| "worker mutex poisoned".to_string())?;
        let Some(w) = guard.as_mut() else {
            return Err("worker is shut down".to_string());
        };
        let resp = w.send_cmd(&WorkerCmd::LoadModel {
            kind: kind.as_str().to_string(),
        })?;
        if !resp.ok {
            return Err(resp.error.unwrap_or_else(|| "(error 不明)".to_string()));
        }
        Ok(resp.elapsed_ms.unwrap_or(0))
    }

    /// 指定モデルで推論を実行する。入力 NCHW Array4<f32> を永続共有メモリに書き、
    /// ワーカーが推論し、結果を別の永続共有メモリから読み取って返す。
    ///
    /// 戻り値: `(出力 shape NCHW, 出力テンソル平坦化 Vec<f32>)`。
    /// 出力サイズ計算: NCHW 形状なので
    ///   output.len() == output_shape.iter().product::<i64>() as usize
    ///
    /// 並行呼び出しは Mutex で直列化される (現状ワーカー 1 個のみなので)。
    /// 共有メモリは pool 起動時に 1 回 create して以降使い回す (per-tile IPC
    /// overhead 削減)。
    #[cfg(windows)]
    pub fn infer(
        &self,
        kind: super::ModelKind,
        input: &ndarray::Array4<f32>,
    ) -> Result<(Vec<i64>, Vec<f32>), String> {
        let mut guard = self
            .worker
            .lock()
            .map_err(|_| "worker mutex poisoned".to_string())?;
        let Some(w) = guard.as_mut() else {
            return Err("worker is shut down".to_string());
        };

        let input_shape: Vec<i64> = input.shape().iter().map(|&d| d as i64).collect();
        let input_bytes = input.len() * std::mem::size_of::<f32>();

        // サイズ check
        if input_bytes > PERSIST_IN_SHM_SIZE {
            return Err(format!(
                "入力サイズ {input_bytes} が永続 shm 容量 {PERSIST_IN_SHM_SIZE} を超過"
            ));
        }

        // 永続入力 shm に書き込み (毎回 create しない、起動時の 1 回のみ)
        let mut in_shm_guard = self
            .in_shm
            .lock()
            .map_err(|_| "in_shm mutex poisoned".to_string())?;
        let in_shm = in_shm_guard
            .as_mut()
            .ok_or_else(|| "in_shm が shutdown 済み".to_string())?;

        let input_slice = input
            .as_slice()
            .ok_or_else(|| "input array が contiguous ではない".to_string())?;
        // SAFETY: f32 slice → u8 slice は同一データの読み取り視点変更のみ。
        let input_bytes_view: &[u8] = unsafe {
            std::slice::from_raw_parts(
                input_slice.as_ptr() as *const u8,
                input_slice.len() * std::mem::size_of::<f32>(),
            )
        };
        in_shm.write(input_bytes_view);
        drop(in_shm_guard); // 早めに解放 (worker 側の open ロックと干渉しないように)

        // コマンド送信 + レスポンス取得
        let cmd = WorkerCmd::Infer {
            kind: kind.as_str().to_string(),
            input_shm: self.in_shm_name.clone(),
            input_bytes,
            input_shape,
            output_shm: self.out_shm_name.clone(),
            output_capacity: PERSIST_OUT_SHM_SIZE,
        };
        let resp = w.send_cmd(&cmd)?;
        if !resp.ok {
            return Err(resp.error.unwrap_or_else(|| "(error 不明)".to_string()));
        }
        let output_shape = resp
            .output_shape
            .ok_or_else(|| "Resp.output_shape が None".to_string())?;

        // 永続出力 shm から bytes 読み取り → Vec<f32>
        let out_count: usize = output_shape.iter().product::<i64>() as usize;
        let out_bytes_len = out_count * std::mem::size_of::<f32>();
        if out_bytes_len > PERSIST_OUT_SHM_SIZE {
            return Err(format!(
                "出力 shape {:?} → {out_bytes_len} bytes が永続 shm 容量 {PERSIST_OUT_SHM_SIZE} を超過",
                output_shape
            ));
        }

        let out_shm_guard = self
            .out_shm
            .lock()
            .map_err(|_| "out_shm mutex poisoned".to_string())?;
        let out_shm = out_shm_guard
            .as_ref()
            .ok_or_else(|| "out_shm が shutdown 済み".to_string())?;
        let out_bytes = out_shm.read_to_vec(out_bytes_len);
        drop(out_shm_guard);

        // SAFETY: out_bytes は Vec<u8> で 8-byte 整列、中身は f32 の little-endian。
        let output: Vec<f32> = unsafe {
            std::slice::from_raw_parts(out_bytes.as_ptr() as *const f32, out_count)
        }
        .to_vec();

        Ok((output_shape, output))
    }

    /// プールをシャットダウン。drop でも呼ばれる。
    pub fn shutdown(&self) {
        let mut guard = match self.worker.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(w) = guard.take() {
            w.shutdown_and_wait();
        }
    }
}

impl Drop for TrtWorkerPool {
    fn drop(&mut self) {
        self.shutdown();
    }
}

// 安全性: WorkerHandle はプロセス間 IPC のハンドルを保持しているが、
// Mutex で共有するので Send + Sync 扱いにする。
unsafe impl Send for TrtWorkerPool {}
unsafe impl Sync for TrtWorkerPool {}

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
    fn spawn() -> Result<Self, String> {
        let exe = std::env::current_exe()
            .map_err(|e| format!("current_exe: {e}"))?;

        let mut child = Command::new(&exe)
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

/// アプリ全体で 1 つだけ持つワーカープール。`Arc<TrtWorkerPool>` で共有。
///
/// 現状は 1 ワーカー固定。複数並列化が必要になったら `Vec<WorkerHandle>` に
/// 拡張する (まだ複数同時に走らせる必要はない、推論は per-tile で sequential)。
pub struct TrtWorkerPool {
    worker: Mutex<Option<WorkerHandle>>,
}

impl TrtWorkerPool {
    /// プールを起動 (子プロセスを spawn してハンドシェイク完了まで待つ)。
    pub fn start() -> Result<Self, String> {
        crate::logger::log("[TRT-worker-pool] 起動中...".to_string());
        let worker = WorkerHandle::spawn()?;
        crate::logger::log("[TRT-worker-pool] 起動完了".to_string());
        Ok(Self {
            worker: Mutex::new(Some(worker)),
        })
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

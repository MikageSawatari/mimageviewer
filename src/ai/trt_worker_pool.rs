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

/// アプリ全体で 1 つだけ持つワーカープール。`Arc<TrtWorkerPool>` で共有。
///
/// 現状は 1 ワーカー固定。複数並列化が必要になったら `Vec<WorkerHandle>` に
/// 拡張する (まだ複数同時に走らせる必要はない、推論は per-tile で sequential)。
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
    pub fn start_with_exe(exe: &std::path::Path) -> Result<Self, String> {
        crate::logger::log(format!(
            "[TRT-worker-pool] 起動中... worker={}",
            exe.display()
        ));
        let worker = WorkerHandle::spawn(exe)?;
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

    /// 指定モデルで推論を実行する。入力 NCHW Array4<f32> を共有メモリに書き、
    /// ワーカーが推論し、結果を別の共有メモリから読み取って返す。
    ///
    /// 戻り値: `(出力 shape NCHW, 出力テンソル平坦化 Vec<f32>)`。
    /// 出力サイズ計算: NCHW 形状なので
    ///   output.len() == output_shape.iter().product::<i64>() as usize
    ///
    /// 並行呼び出しは Mutex で直列化される (現状ワーカー 1 個のみなので)。
    #[cfg(windows)]
    pub fn infer(
        &self,
        kind: super::ModelKind,
        input: &ndarray::Array4<f32>,
    ) -> Result<(Vec<i64>, Vec<f32>), String> {
        use super::trt_worker_proto::shm_name;
        use super::trt_worker_shm::SharedMem;

        let mut guard = self
            .worker
            .lock()
            .map_err(|_| "worker mutex poisoned".to_string())?;
        let Some(w) = guard.as_mut() else {
            return Err("worker is shut down".to_string());
        };

        let pid = std::process::id();
        // 共有メモリ名は (pid, seq) で一意化。今のところ seq は 0 固定 (1 度に
        // 1 推論しか走らないため衝突なし)。
        let in_name = shm_name("in", pid, 0);
        let out_name = shm_name("out", pid, 0);

        let input_shape: Vec<i64> = input.shape().iter().map(|&d| d as i64).collect();
        let input_bytes = input.len() * std::mem::size_of::<f32>();

        // 出力容量: 4x スケール + バッファ。最悪ケースでも収まるよう 16 倍 + 余裕。
        // (256x256 入力 → 1024x1024 出力 = 16x、デノイズは 1x、MI-GAN は 1x)
        let output_capacity = input_bytes.saturating_mul(16).saturating_add(64);

        let mut in_shm = SharedMem::create(&in_name, input_bytes)
            .map_err(|e| format!("create input shm: {e}"))?;
        let out_shm = SharedMem::create(&out_name, output_capacity)
            .map_err(|e| format!("create output shm: {e}"))?;

        // 入力 f32 slice を bytes として shm に書き込む
        let input_slice = input
            .as_slice()
            .ok_or_else(|| "input array が contiguous ではない".to_string())?;
        // SAFETY: f32 slice → u8 slice は同一データの読み取り視点変更のみ。
        // input_slice の lifetime は input の借用、in_shm.write はコピー。
        let input_bytes_view: &[u8] = unsafe {
            std::slice::from_raw_parts(
                input_slice.as_ptr() as *const u8,
                input_slice.len() * std::mem::size_of::<f32>(),
            )
        };
        in_shm.write(input_bytes_view);

        // コマンド送信 + レスポンス取得
        let cmd = WorkerCmd::Infer {
            kind: kind.as_str().to_string(),
            input_shm: in_name.clone(),
            input_bytes,
            input_shape,
            output_shm: out_name.clone(),
            output_capacity,
        };
        let resp = w.send_cmd(&cmd)?;
        if !resp.ok {
            return Err(resp.error.unwrap_or_else(|| "(error 不明)".to_string()));
        }
        let output_shape = resp
            .output_shape
            .ok_or_else(|| "Resp.output_shape が None".to_string())?;

        // 出力 shm から bytes 読み取り → Vec<f32>
        let out_count: usize = output_shape.iter().product::<i64>() as usize;
        let out_bytes_len = out_count * std::mem::size_of::<f32>();
        if out_bytes_len > output_capacity {
            return Err(format!(
                "出力 shape {:?} → {out_bytes_len} bytes が capacity {output_capacity} 超過",
                output_shape
            ));
        }
        let out_bytes = out_shm.read_to_vec(out_bytes_len);
        // SAFETY: out_bytes は Vec<u8> で 8-byte 整列されている (Rust の Global
        // allocator は最低 8 バイト境界、f32 の 4 バイト alignment より厳しい)。
        // 中身は worker が書いた x86_64 little-endian の f32 なので、ポインタを
        // *const f32 にキャストして slice を作り、そこから to_vec で Vec<f32> に
        // コピーするだけで OK。byte-by-byte from_le_bytes ループより 100x 以上速い。
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

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

use std::collections::HashSet;
use std::io::{BufRead, BufReader, Write};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{LazyLock, Mutex};

/// SHM 名の seq 部分。プロセス内で start_with_exe が呼ばれるたびに 1 加算する。
///
/// 同一 PID で `miv_trt_in_<pid>_<seq>` が衝突するのを避けるため。原則として
/// pool は 1 個しか attach されない (= 同時に 2 つ start することはない) が、
/// 以下のケースで前回の SHM ハンドルが残っているとカーネルオブジェクトが残存し
/// `CreateFileMappingW` が `ERROR_ALREADY_EXISTS` を返してしまうため:
///
/// - pool detach → drop が遅延して、次の start_with_exe との間で重なる
/// - 子プロセス側が SHM を open した状態で main 側が drop しても、子側のハンドル
///   保持中はカーネルオブジェクトが残る (= 子プロセスの shutdown 完了待ちが必要)
/// - 何らかのバグで Drop パスをスキップした場合 (panic 経由など)
///
/// seq を毎回更新すれば、上記のような残骸 SHM があっても新規 start は成功する。
/// SHM 名は `WorkerCmd::Infer` で子プロセスに毎回送るので、子側の互換性問題はない。
static SHM_SEQ: AtomicU32 = AtomicU32::new(0);
static TRT_INFER_BREAKDOWN_LOG_COUNTER: AtomicU64 = AtomicU64::new(0);
static LOG_ALL_TRT_INFER_BREAKDOWN: LazyLock<bool> =
    LazyLock::new(|| std::env::var_os("MIV_TRT_INFER_BREAKDOWN_LOG").is_some());

use super::trt_worker_proto::{TRT_INFER_WORKER_ARG, WorkerCmd, WorkerResp};

/// 1 個のワーカー子プロセスのハンドル。
struct WorkerHandle {
    child: Child,
    stdin: ChildStdin,
    /// stdout 読み取りを行うスレッドからの response 行受信チャネル。
    /// EOF 時は `Ok(None)`、I/O エラーは `Err(string)` を 1 回だけ送って閉じる。
    stdout_rx: std::sync::mpsc::Receiver<Result<String, String>>,
    /// stderr ドレインスレッドの join handle。Drop 時に join される。
    /// stderr を読み捨てない場合、子側がパイプ満杯で block する病理的ケースが
    /// あるので必ず thread に逃がす (Codex P1 指摘)。
    stderr_join: Option<std::thread::JoinHandle<()>>,
    /// stdout 読み取りスレッドの join handle。
    stdout_join: Option<std::thread::JoinHandle<()>>,
}

/// `read_resp_line` / `send_cmd` の最大待機時間。
/// CUDA / TensorRT の初回 engine deserialize は数百 ms かかるが、5 秒で済むのが
/// 普通。10 秒を超えるなら子が hang していると判断して kill する。
const WORKER_RESP_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// 起動ハンドシェイクの最大待機時間。`AiRuntime::new_with_backend` 内で
/// `ort::init_from` + provider DLL preload + TRT pack scan が走るため、
/// 通常枠より長めの 30 秒。
const WORKER_HANDSHAKE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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
        let stderr = child
            .stderr
            .take()
            .ok_or_else(|| "child stderr missing".to_string())?;

        // stderr ドレインスレッド: 子の stderr を黙々と読み捨てる
        // (= 子のパイプ満杯で write block するのを防ぐ)。CUDA / TensorRT /
        // ORT は init / load 中に多量の警告 / 進捗を stderr に出すため必須。
        let stderr_join = std::thread::Builder::new()
            .name("trt-worker-stderr-drain".to_string())
            .spawn(move || {
                let mut reader = std::io::BufReader::new(stderr);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) => break, // EOF
                        Ok(_) => {
                            // worker 由来 stderr は logger に流す (= デバッグ時に役立つ)。
                            // logger は別ファイル open + Mutex 保護で thread-safe。
                            crate::logger::log(format!("[TRT-worker stderr] {}", line.trim_end()));
                        }
                        Err(_) => break,
                    }
                }
            })
            .map_err(|e| format!("spawn stderr drain thread: {e}"))?;

        // stdout 読み取りスレッド: 行ごとに channel に push。
        // 親が read_resp_line で recv_timeout して拾う。これにより
        // read_line の無限 block を timeout 内に切り上げられる。
        let (tx, stdout_rx) = std::sync::mpsc::channel();
        let stdout_join = std::thread::Builder::new()
            .name("trt-worker-stdout-reader".to_string())
            .spawn(move || {
                let mut reader = BufReader::new(stdout);
                let mut line = String::new();
                loop {
                    line.clear();
                    match reader.read_line(&mut line) {
                        Ok(0) => {
                            // EOF: 子が exit。
                            let _ = tx.send(Ok(String::new()));
                            break;
                        }
                        Ok(_) => {
                            if tx.send(Ok(line.clone())).is_err() {
                                // 親が drop された
                                break;
                            }
                        }
                        Err(e) => {
                            let _ = tx.send(Err(format!("stdout read: {e}")));
                            break;
                        }
                    }
                }
            })
            .map_err(|e| format!("spawn stdout reader thread: {e}"))?;

        let mut handle = WorkerHandle {
            child,
            stdin,
            stdout_rx,
            stderr_join: Some(stderr_join),
            stdout_join: Some(stdout_join),
        };

        // ハンドシェイク: 子の起動成功 / 失敗のレスポンスを 1 行読む。
        // pack 不在 + DirectML フォールバック等の場合は失敗で来る。
        let resp = handle
            .recv_resp_with_timeout(WORKER_HANDSHAKE_TIMEOUT)
            .map_err(|e| {
                // ハンドシェイクが時間内に来ない / 失敗 → 子を kill して回収
                let _ = handle.child.kill();
                let _ = handle.child.wait();
                format!("ワーカー起動 timeout / 通信失敗: {e}")
            })?;
        if !resp.ok {
            let _ = handle.child.kill();
            let _ = handle.child.wait();
            return Err(format!(
                "ワーカー初期化失敗: {}",
                resp.error.unwrap_or_else(|| "(理由不明)".to_string())
            ));
        }

        Ok(handle)
    }

    fn send_cmd(&mut self, cmd: &WorkerCmd) -> Result<WorkerResp, String> {
        let s = serde_json::to_string(cmd).map_err(|e| format!("cmd serialize: {e}"))?;
        writeln!(self.stdin, "{s}").map_err(|e| format!("stdin write: {e}"))?;
        self.stdin
            .flush()
            .map_err(|e| format!("stdin flush: {e}"))?;
        self.recv_resp_with_timeout(WORKER_RESP_TIMEOUT)
    }

    /// stdout から 1 つの response 行を timeout 付きで受信して JSON parse する。
    /// timeout 切れ / EOF / 解析失敗は全て Err を返す。
    /// 呼び出し側 (send_cmd / spawn) は Err を「子が応答不能」と解釈して
    /// 必要なら子を kill する。
    fn recv_resp_with_timeout(
        &mut self,
        timeout: std::time::Duration,
    ) -> Result<WorkerResp, String> {
        let line = match self.stdout_rx.recv_timeout(timeout) {
            Ok(Ok(line)) if line.is_empty() => {
                return Err("worker stdout が EOF (子プロセスが予期せず終了した可能性)".to_string());
            }
            Ok(Ok(line)) => line,
            Ok(Err(e)) => return Err(e),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                return Err(format!(
                    "worker 応答 timeout ({} 秒、子プロセスが hang した可能性)",
                    timeout.as_secs()
                ));
            }
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                return Err("worker stdout reader thread が終了している".to_string());
            }
        };
        let trimmed = line.trim();
        if trimmed.is_empty() {
            return Err("worker から空レスポンス".to_string());
        }
        serde_json::from_str::<WorkerResp>(trimmed)
            .map_err(|e| format!("resp parse: {e} (raw: {line:?})"))
    }

    fn shutdown_and_wait(mut self) {
        // **アプリ終了時の高速 shutdown**: worker には保存すべき状態が無いので、
        // graceful shutdown は行わず即座に kill する。これにより ORT / CUDA
        // context の cleanup (~1 秒) を待たずに済む。kill は冪等なので worker が
        // 既に exit していても問題ない。
        //
        // 旧実装は `WorkerCmd::Shutdown` を送って response を待って `child.wait()`
        // していたが、ORT cleanup が遅いため UI 終了まで 1 秒近くもたついていた
        // (Apr 29 ユーザー報告)。
        let _ = self.child.kill();
        match self.child.wait() {
            Ok(status) => crate::logger::log(format!(
                "[TRT-worker-pool] worker killed and reaped: {status:?}"
            )),
            Err(e) => crate::logger::log(format!("[TRT-worker-pool] wait failed: {e}")),
        }
        // stderr / stdout drain thread は子の stdout/stderr が close した時点で
        // 自然に EOF を受け取って終了する。join で完全終了を確認。
        if let Some(h) = self.stderr_join.take() {
            let _ = h.join();
        }
        if let Some(h) = self.stdout_join.take() {
            let _ = h.join();
        }
    }
}

impl Drop for WorkerHandle {
    fn drop(&mut self) {
        // shutdown_and_wait を経由しない drop (= キャンセル / panic 経由) でも
        // 子と drain thread を回収する。
        let _ = self.child.kill();
        let _ = self.child.wait();
        if let Some(h) = self.stderr_join.take() {
            let _ = h.join();
        }
        if let Some(h) = self.stdout_join.take() {
            let _ = h.join();
        }
    }
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
    /// Parent-side view of models already loaded in the current worker.
    ///
    /// The worker also keeps sessions cached, but sending an idempotent
    /// LoadModel command before every tile still costs a JSON IPC round trip.
    /// Cache it here so steady-state inference only sends Infer commands.
    loaded_models: Mutex<HashSet<super::ModelKind>>,
    /// 永続入力共有メモリ。pool 起動時に 1 回 create、Drop で破棄。
    in_shm: Mutex<Option<super::trt_worker_shm::SharedMem>>,
    /// 永続出力共有メモリ。同上。
    out_shm: Mutex<Option<super::trt_worker_shm::SharedMem>>,
    /// 共有メモリ名 (子に伝えるため)
    in_shm_name: String,
    out_shm_name: String,
    /// 子プロセスが死んだ (I/O が EOF になった、kill された) ことを示すフラグ。
    /// `true` になったら以降の `infer` / `load_model` は即 Err。`AiRuntime` 側で
    /// このフラグを見て worker_pool を detach し、UI に通知する。
    is_dead: AtomicBool,
}

#[cfg(not(windows))]
pub struct TrtWorkerPool {
    worker: Mutex<Option<WorkerHandle>>,
    loaded_models: Mutex<HashSet<super::ModelKind>>,
    is_dead: AtomicBool,
}

impl TrtWorkerPool {
    /// プールを起動 (現在の exe をワーカーとして spawn)。
    /// メインアプリ (mimageviewer.exe) から呼ぶ用途。
    pub fn start() -> Result<Self, String> {
        let exe = std::env::current_exe().map_err(|e| format!("current_exe: {e}"))?;
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
        // seq はプロセス内グローバルカウンタ。残骸 SHM 衝突を回避する目的。
        let pid = std::process::id();
        let seq = SHM_SEQ.fetch_add(1, Ordering::Relaxed);
        let in_shm_name = shm_name("in", pid, seq);
        let out_shm_name = shm_name("out", pid, seq);
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
            loaded_models: Mutex::new(HashSet::new()),
            in_shm: Mutex::new(Some(in_shm)),
            out_shm: Mutex::new(Some(out_shm)),
            in_shm_name,
            out_shm_name,
            is_dead: AtomicBool::new(false),
        })
    }

    #[cfg(not(windows))]
    pub fn start_with_exe(_exe: &std::path::Path) -> Result<Self, String> {
        Err("TRT worker pool は Windows 専用".to_string())
    }

    /// 子プロセスが死亡判定済みか。
    pub fn is_dead(&self) -> bool {
        self.is_dead.load(Ordering::Acquire)
    }

    /// 子プロセスが死亡したと判定する (内部用)。
    /// 1 度立ったら戻らない (pool は使い捨て、再起動は新 pool を作って差し替える)。
    fn mark_dead(&self) {
        self.is_dead.store(true, Ordering::Release);
    }

    /// 内部 send_cmd 呼び出しのエラー文字列が「子プロセスが死んだ」ことを示すかを判定。
    /// 該当パターン (`stdin write` / `stdout read` / EOF) なら mark_dead して true。
    /// 該当しない (子の `ok=false` レスポンス等) なら false。
    fn classify_io_error(&self, err: &str) {
        // I/O が壊れているパターンは IPC 上の文字列 prefix で検出する
        // (read_resp_line / send_cmd で使われるエラーメッセージと一致させる)
        let killed = err.starts_with("stdin write")
            || err.starts_with("stdin flush")
            || err.starts_with("stdout read")
            || err.contains("worker stdout が EOF");
        if killed {
            self.mark_dead();
            crate::logger::log(format!(
                "[TRT-worker-pool] worker 子プロセス死亡を検出: {err}"
            ));
        }
    }

    /// 指定モデルをワーカー側でロードする (engine cache HIT で速い)。
    pub fn load_model(&self, kind: super::ModelKind) -> Result<u64, String> {
        if self.is_dead() {
            return Err("worker is dead (前段で死亡判定済み)".to_string());
        }
        if self
            .loaded_models
            .lock()
            .map_err(|_| "loaded_models mutex poisoned".to_string())?
            .contains(&kind)
        {
            return Ok(0);
        }
        let mut guard = self
            .worker
            .lock()
            .map_err(|_| "worker mutex poisoned".to_string())?;
        let Some(w) = guard.as_mut() else {
            return Err("worker is shut down".to_string());
        };
        let resp = match w.send_cmd(&WorkerCmd::LoadModel {
            kind: kind.as_str().to_string(),
        }) {
            Ok(r) => r,
            Err(e) => {
                self.classify_io_error(&e);
                return Err(e);
            }
        };
        if !resp.ok {
            return Err(resp.error.unwrap_or_else(|| "(error 不明)".to_string()));
        }
        self.loaded_models
            .lock()
            .map_err(|_| "loaded_models mutex poisoned".to_string())?
            .insert(kind);
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
        if self.is_dead() {
            return Err("worker is dead (前段で死亡判定済み)".to_string());
        }
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
        let resp = match w.send_cmd(&cmd) {
            Ok(r) => r,
            Err(e) => {
                self.classify_io_error(&e);
                return Err(e);
            }
        };
        if !resp.ok {
            return Err(resp.error.unwrap_or_else(|| "(error 不明)".to_string()));
        }
        let output_shape = resp
            .output_shape
            .ok_or_else(|| "Resp.output_shape が None".to_string())?;

        // 計装: ワーカー breakdown を logger に出す (bench でログから集計可能)
        if let Some(b) = resp.breakdown.as_ref() {
            let seq = TRT_INFER_BREAKDOWN_LOG_COUNTER.fetch_add(1, Ordering::Relaxed);
            if *LOG_ALL_TRT_INFER_BREAKDOWN || seq % 256 == 0 {
                crate::logger::log(format!(
                    "[TRT-pool] infer breakdown{}: read_input={:.3} tensor_build={:.3} session_run={:.3} extract_and_write={:.3} total={:.3} ms",
                    if *LOG_ALL_TRT_INFER_BREAKDOWN {
                        ""
                    } else {
                        " (sampled)"
                    },
                    b.read_input_ms,
                    b.tensor_build_ms,
                    b.session_run_ms,
                    b.extract_and_write_ms,
                    b.read_input_ms + b.tensor_build_ms + b.session_run_ms + b.extract_and_write_ms
                ));
            }
        }

        // 永続出力 shm から bytes 読み取り → Vec<f32>
        let out_count: usize = output_shape.iter().product::<i64>() as usize;
        let out_bytes_len = out_count * std::mem::size_of::<f32>();
        if out_bytes_len > PERSIST_OUT_SHM_SIZE {
            return Err(format!(
                "出力 shape {:?} → {out_bytes_len} bytes が永続 shm 容量 {PERSIST_OUT_SHM_SIZE} を超過",
                output_shape
            ));
        }

        // 出力 shm から直接 Vec<f32> にコピー (read_to_vec の Vec<u8> 中間生成を回避、
        // 1 回の memcpy のみ)。
        let out_shm_guard = self
            .out_shm
            .lock()
            .map_err(|_| "out_shm mutex poisoned".to_string())?;
        let out_shm = out_shm_guard
            .as_ref()
            .ok_or_else(|| "out_shm が shutdown 済み".to_string())?;
        // SAFETY: out_shm は永続 mapped view、ページ整列。中身は worker が書いた
        // f32 little-endian。as_slice の slice lifetime は unsafe ブロック内に
        // 閉じて to_vec で即 owned 化、その後 guard を drop しても output は安全。
        let output: Vec<f32> = unsafe {
            let bytes = out_shm.as_slice(out_bytes_len);
            std::slice::from_raw_parts(bytes.as_ptr() as *const f32, out_count).to_vec()
        };
        drop(out_shm_guard);

        Ok((output_shape, output))
    }

    /// プールをシャットダウン。drop でも呼ばれる。
    pub fn shutdown(&self) {
        let mut guard = match self.worker.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        if let Some(mut w) = guard.take() {
            // is_dead 済みの場合、子プロセスは既に exit している (broken pipe を
            // 親が観測している) のが普通。ただし「子が無限ループに陥って I/O だけ
            // 詰まった」病理的ケースに備え、shutdown_and_wait の前に kill しておく
            // (kill は冪等で、既に exit したプロセスへ呼んでも無害)。
            if self.is_dead() {
                let _ = w.child.kill();
            }
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

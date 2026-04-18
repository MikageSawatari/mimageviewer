//! Susie 画像プラグイン (`.spi`) の実行支援。
//!
//! ## アーキテクチャ (PDFium と同型)
//!
//! `.spi` は 32bit DLL なので 64bit の本体プロセスから直接は呼べない。そこで、
//! `mimageviewer-susie32.exe` (32bit) を子プロセスとして起動し、stdin/stdout の
//! バイナリプロトコルでデコードを依頼する。
//!
//! ```text
//! [Main Process, 64bit]
//!   └── SusieWorkerPool (N プロセス、デフォルト 3)
//!         ├── Worker 0: mimageviewer-susie32.exe
//!         ├── Worker 1: mimageviewer-susie32.exe
//!         └── Worker 2: mimageviewer-susie32.exe
//! ```
//!
//! 起動直後に全ワーカーへ `Handshake { plugin_dir }` を投げ、ロード済み
//! プラグイン一覧と対応拡張子集合を取得する。以降 `decode_file` / `decode_bytes`
//! 要求は手が空いているワーカーへ回される (優先度なし、単純 FIFO)。
//!
//! ## 並列実行の停止
//!
//! 古い Susie プラグインは並列実行を想定していない場合がある (一時ファイル衝突、
//! INI の race 書き込み等)。`Settings::susie_allow_parallel = false` のときは
//! プールを 1 プロセスに落とし、問題プラグインの切り分けをユーザー側で可能にする。
//!
//! ## プラグインフォルダ
//!
//! `<data_dir>/susie_plugins/` を規定位置とする (`plugin_dir()` 参照)。初回起動時に
//! 作成し、`README.txt` を配置する (入手先案内)。

use std::collections::HashSet;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Condvar, Mutex, OnceLock, RwLock};

// -----------------------------------------------------------------------
// ワーカー exe の埋め込みと APPDATA への展開
// -----------------------------------------------------------------------

/// 32bit Susie ワーカー exe (PDFium DLL と同じパターンで本体 exe に埋め込む)。
/// 初回起動時に `%APPDATA%/mimageviewer/mimageviewer-susie32.exe` へ展開される。
/// インストール先 (Program Files) に書き込み不要で、本体 exe のフォルダにも
/// 追加ファイルを置かない。
static SUSIE_WORKER_BYTES: &[u8] =
    include_bytes!("../vendor/susie-worker/mimageviewer-susie32.exe");

static WORKER_EXE_PATH: OnceLock<PathBuf> = OnceLock::new();

/// 埋め込みバイト列を APPDATA に展開する。サイズ一致でスキップ。
/// 起動時に一度だけ呼ぶ (main.rs の data_dir 初期化直後)。
pub fn ensure_worker_extracted() {
    let _ = worker_exe_cached_path();
}

/// ワーカー exe の展開先パス。
/// 環境変数 `MIV_SUSIE_WORKER` が指定されていればそれを優先 (テスト/開発用)。
/// そうでなければ `<data_dir>/mimageviewer-susie32.exe` に埋め込みバイト列を
/// 必要に応じて書き出し、そのパスを返す。
fn worker_exe_cached_path() -> PathBuf {
    if let Ok(p) = std::env::var("MIV_SUSIE_WORKER") {
        return PathBuf::from(p);
    }
    WORKER_EXE_PATH
        .get_or_init(|| {
            let dir = crate::data_dir::get();
            if let Err(e) = std::fs::create_dir_all(&dir) {
                crate::logger::log(format!(
                    "susie: data_dir create failed: {e} (path: {})",
                    dir.display()
                ));
                // 展開失敗時も期待パスを返す (is_ready=false で UI にエラーが出る)
                return dir.join(WORKER_EXE_NAME);
            }
            let exe_path = dir.join(WORKER_EXE_NAME);
            // 埋め込みが空 (開発時 vendor/susie-worker 未設置) の場合は展開しない。
            // 既存の実ファイルを 0 バイトで上書きして壊すのを避ける。
            if SUSIE_WORKER_BYTES.is_empty() {
                return exe_path;
            }
            // サイズ比較だけではアップデート時に同サイズ・別内容のバイナリを
            // 取り違える可能性があるため、既存ファイル全体を読んで中身比較する。
            // 169KB 程度なので起動時の 1 回読みは許容範囲。
            let needs_extract = match std::fs::read(&exe_path) {
                Ok(existing) => existing.as_slice() != SUSIE_WORKER_BYTES,
                Err(_) => true,
            };
            if needs_extract {
                // 他プロセス (旧 mImageViewer インスタンス) がワーカーを起動中で
                // ファイルをロックしている場合 write は失敗する。その場合は
                // 古いバイナリのまま続行 (次回起動で書き換わる)。
                match std::fs::write(&exe_path, SUSIE_WORKER_BYTES) {
                    Ok(()) => {
                        crate::logger::log(format!(
                            "susie: worker extracted to {} ({} bytes)",
                            exe_path.display(),
                            SUSIE_WORKER_BYTES.len(),
                        ));
                    }
                    Err(e) => {
                        crate::logger::log(format!(
                            "susie: worker extract failed: {e} (path: {})",
                            exe_path.display()
                        ));
                    }
                }
            }
            exe_path
        })
        .clone()
}

// ─────────────────────────────────────────────────────────────────
// 定数 / プロトコル定数 (worker 側と一致)
// ─────────────────────────────────────────────────────────────────

const MSG_HANDSHAKE: u8 = 1;
const MSG_DECODE_FILE: u8 = 2;
const MSG_DECODE_BYTES: u8 = 3;
const MSG_SHUTDOWN: u8 = 4;

const STATUS_OK: u8 = 0;
const STATUS_ERR: u8 = 1;

/// ワーカーバイナリ名 (リリース時には `mimageviewer.exe` と同じディレクトリに配置)。
pub const WORKER_EXE_NAME: &str = "mimageviewer-susie32.exe";

#[cfg(windows)]
const CREATE_NO_WINDOW: u32 = 0x08000000;

// ─────────────────────────────────────────────────────────────────
// 公開データ型
// ─────────────────────────────────────────────────────────────────

#[derive(Debug, Clone)]
pub struct PluginInfo {
    pub name: String,
    /// 小文字・先頭 `.` なし
    pub extensions: Vec<String>,
}

/// プラグインフォルダの既定パス `<data_dir>/susie_plugins`。
/// 環境変数 `MIV_SUSIE_PLUGIN_DIR` で上書き可能 (テスト / 開発用)。
pub fn plugin_dir() -> PathBuf {
    if let Ok(p) = std::env::var("MIV_SUSIE_PLUGIN_DIR") {
        return PathBuf::from(p);
    }
    crate::data_dir::get().join("susie_plugins")
}

/// `plugin_dir()` を作成し、存在しなければ `README.txt` を書き出す。
pub fn ensure_plugin_dir() -> std::io::Result<PathBuf> {
    let dir = plugin_dir();
    std::fs::create_dir_all(&dir)?;
    let readme = dir.join("README.txt");
    if !readme.exists() {
        let msg = "\
このフォルダに Susie 画像プラグイン (.spi) を配置すると、mImageViewer で\
対応形式を表示できます。配置後、環境設定の「Susie プラグイン」ページで\
「プラグインを再読み込み」を押してください。\r\n\
\r\n\
代表的なプラグイン:\r\n\
  ifpi.spi    - PC-98 PI 形式\r\n\
  ifmag.spi   - PC-98 MAG 形式\r\n\
  ifq4.spi    - Q0/Q4 形式\r\n\
  ifxld4.spi  - X68000 PIC/PIC2 形式\r\n\
  ifmaki.spi  - MAKI 形式\r\n\
\r\n\
※ Susie プラグインは 32bit DLL です。mImageViewer は 32bit ワーカープロセス\r\n\
  (mimageviewer-susie32.exe) を介してロードするため、本体が 64bit でも利用できます。\r\n\
※ プラグインのクラッシュはワーカープロセスに閉じ込められ、本体には影響しません。\r\n\
";
        let _ = std::fs::write(readme, msg);
    }
    Ok(dir)
}

// ─────────────────────────────────────────────────────────────────
// 内部: 1 ワーカープロセス = 1 IPC チャネル + 1 ディスパッチャースレッド
// ─────────────────────────────────────────────────────────────────

struct WorkerIo {
    stdin: std::process::ChildStdin,
    stdout: BufReader<std::process::ChildStdout>,
}

struct Job {
    request: Vec<u8>,
    reply: mpsc::Sender<std::io::Result<Vec<u8>>>,
    cancel: Option<Arc<AtomicBool>>,
    enqueued_at: std::time::Instant,
}

struct JobQueue {
    jobs: std::collections::VecDeque<Job>,
    shutdown: bool,
}

// ─────────────────────────────────────────────────────────────────
// SusieWorkerPool
// ─────────────────────────────────────────────────────────────────

pub struct SusieWorkerPool {
    queue: Arc<(Mutex<JobQueue>, Condvar)>,
    worker_count: usize,
    /// ロード済みプラグイン (全ワーカーで共通、handshake 応答をマージ済み)
    plugins: Vec<PluginInfo>,
    /// 拡張子の集合 (小文字)。全プラグインの対応拡張子を合算したもの。
    extensions: HashSet<String>,
    dispatcher_threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl SusieWorkerPool {
    /// 初期化: ワーカープロセスを起動して handshake を完了させる。
    /// `parallel = false` の場合はプールサイズを 1 に固定する (並列問題の回避)。
    fn start(parallel: bool) -> Self {
        let pool_size = if parallel { 3 } else { 1 };
        let exe = worker_exe_path();
        let plugin_dir = match ensure_plugin_dir() {
            Ok(d) => d,
            Err(e) => {
                crate::logger::log(format!("susie: plugin dir setup failed: {e}"));
                return empty_pool();
            }
        };

        if !exe.exists() {
            crate::logger::log(format!(
                "susie: worker exe not found at {}, Susie support disabled",
                exe.display()
            ));
            return empty_pool();
        }

        let queue = Arc::new((
            Mutex::new(JobQueue {
                jobs: std::collections::VecDeque::new(),
                shutdown: false,
            }),
            Condvar::new(),
        ));

        let mut plugins_merged: Vec<PluginInfo> = Vec::new();
        let mut extensions: HashSet<String> = HashSet::new();
        let mut dispatcher_threads = Vec::with_capacity(pool_size);
        let mut worker_count = 0usize;

        for i in 0..pool_size {
            match spawn_worker_and_handshake(&exe, &plugin_dir) {
                Ok((child, io, plugin_list)) => {
                    crate::logger::log(format!(
                        "susie: worker {i} started (pid={}, plugins={})",
                        child.id(),
                        plugin_list.len()
                    ));
                    if i == 0 {
                        // 最初のワーカーの結果を公式扱いにする (全ワーカーが同じ .spi を
                        // 読み込むので結果は一致するはず)。拡張子集合も同じ。
                        for pi in &plugin_list {
                            for ext in &pi.extensions {
                                extensions.insert(ext.clone());
                            }
                        }
                        plugins_merged = plugin_list;
                    }
                    worker_count += 1;
                    let q = Arc::clone(&queue);
                    let handle = std::thread::Builder::new()
                        .name(format!("susie-pool-{i}"))
                        .spawn(move || run_dispatcher(i, q, child, io))
                        .expect("susie: failed to spawn dispatcher thread");
                    dispatcher_threads.push(handle);
                }
                Err(e) => {
                    crate::logger::log(format!("susie: worker {i} spawn/handshake failed: {e}"));
                }
            }
        }

        if worker_count == 0 {
            crate::logger::log("susie: no workers available, Susie support disabled");
        } else {
            crate::logger::log(format!(
                "susie: {worker_count} workers ready, {} plugins, {} extensions",
                plugins_merged.len(),
                extensions.len(),
            ));
        }

        SusieWorkerPool {
            queue,
            worker_count,
            plugins: plugins_merged,
            extensions,
            dispatcher_threads: Mutex::new(dispatcher_threads),
        }
    }

    pub fn is_ready(&self) -> bool {
        self.worker_count > 0
    }

    pub fn plugins(&self) -> &[PluginInfo] {
        &self.plugins
    }

    /// この拡張子 (小文字、先頭 `.` なし) をいずれかのプラグインが扱えるか。
    pub fn supports_extension(&self, ext_lower: &str) -> bool {
        self.extensions.contains(ext_lower)
    }

    /// 対応拡張子の snapshot (UI 表示用)。
    pub fn extensions(&self) -> Vec<String> {
        let mut v: Vec<String> = self.extensions.iter().cloned().collect();
        v.sort();
        v
    }

    fn execute(
        &self,
        request: &[u8],
        cancel: Option<&Arc<AtomicBool>>,
    ) -> std::io::Result<Vec<u8>> {
        if self.worker_count == 0 {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "susie: no workers available",
            ));
        }
        let (reply_tx, reply_rx) = mpsc::channel();
        let job = Job {
            request: request.to_vec(),
            reply: reply_tx,
            cancel: cancel.cloned(),
            enqueued_at: std::time::Instant::now(),
        };
        {
            let (mtx, cv) = &*self.queue;
            let mut q = mtx.lock().unwrap();
            q.jobs.push_back(job);
            cv.notify_one();
        }
        loop {
            match reply_rx.recv_timeout(std::time::Duration::from_millis(50)) {
                Ok(r) => return r,
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    if let Some(c) = cancel
                        && c.load(Ordering::Relaxed)
                    {
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::Interrupted,
                            "susie: cancelled while waiting",
                        ));
                    }
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::Other,
                        "susie: dispatcher disconnected",
                    ));
                }
            }
        }
    }
}

fn empty_pool() -> SusieWorkerPool {
    SusieWorkerPool {
        queue: Arc::new((
            Mutex::new(JobQueue {
                jobs: std::collections::VecDeque::new(),
                shutdown: false,
            }),
            Condvar::new(),
        )),
        worker_count: 0,
        plugins: Vec::new(),
        extensions: HashSet::new(),
        dispatcher_threads: Mutex::new(Vec::new()),
    }
}

impl Drop for SusieWorkerPool {
    fn drop(&mut self) {
        {
            let (mtx, cv) = &*self.queue;
            if let Ok(mut q) = mtx.lock() {
                q.shutdown = true;
                cv.notify_all();
            }
        }
        if let Ok(mut threads) = self.dispatcher_threads.lock() {
            for h in threads.drain(..) {
                let _ = h.join();
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────
// グローバルシングルトン (設定から起動時に初期化)
// ─────────────────────────────────────────────────────────────────

static POOL: OnceLock<RwLock<Arc<SusieWorkerPool>>> = OnceLock::new();

/// 初回呼び出し時にプールを起動する (アプリ起動直後などで一度呼ぶ想定)。
/// 再起動は `reload()` を使う。
pub fn get_pool() -> Arc<SusieWorkerPool> {
    let lock = POOL.get_or_init(|| {
        let settings = crate::settings::Settings::load();
        let enabled = settings.susie_enabled;
        let parallel = settings.susie_allow_parallel;
        if enabled {
            RwLock::new(Arc::new(SusieWorkerPool::start(parallel)))
        } else {
            RwLock::new(Arc::new(empty_pool()))
        }
    });
    Arc::clone(&lock.read().unwrap())
}

/// プールが既に初期化されていれば `Some` を返す。未初期化なら `None` (spawn しない)。
/// 起動時パス判定など軽量処理で呼ばれる想定。
pub fn try_get_pool() -> Option<Arc<SusieWorkerPool>> {
    POOL.get().map(|lock| Arc::clone(&lock.read().unwrap()))
}

/// ある拡張子が Susie プラグインで扱えるか (プール未初期化時は false)。
/// `folder_tree::is_recognized_image_ext` から呼ばれる軽量判定。
pub fn supports_extension(ext_lower: &str) -> bool {
    match try_get_pool() {
        Some(pool) => pool.supports_extension(ext_lower),
        None => false,
    }
}

/// プラグインフォルダ更新 / 並列オプション変更時にワーカープールを再起動する。
pub fn reload(enabled: bool, parallel: bool) {
    let lock = POOL.get_or_init(|| {
        RwLock::new(Arc::new(empty_pool()))
    });
    let new_pool = if enabled {
        SusieWorkerPool::start(parallel)
    } else {
        empty_pool()
    };
    // 旧プールの Drop がここで走る
    *lock.write().unwrap() = Arc::new(new_pool);
}

// ─────────────────────────────────────────────────────────────────
// 公開 API (デコード)
// ─────────────────────────────────────────────────────────────────

/// 指定ファイルパスをワーカーに渡してデコードする。
/// 戻り値は BGRA (top-down、行優先) ピクセル + 幅 + 高さ。
pub fn decode_file(
    path: &Path,
    cancel: Option<Arc<AtomicBool>>,
) -> std::io::Result<image::DynamicImage> {
    let pool = get_pool();
    if !pool.is_ready() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "susie: not available",
        ));
    }
    let req = encode_decode_file_request(path);
    let resp = pool.execute(&req, cancel.as_ref())?;
    parse_decode_response(&resp)
}

/// メモリ上のバイト列からデコードする (ZIP 内画像用)。
pub fn decode_bytes(
    filename_hint: &str,
    bytes: &[u8],
    cancel: Option<Arc<AtomicBool>>,
) -> std::io::Result<image::DynamicImage> {
    let pool = get_pool();
    if !pool.is_ready() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::Other,
            "susie: not available",
        ));
    }
    let req = encode_decode_bytes_request(filename_hint, bytes);
    let resp = pool.execute(&req, cancel.as_ref())?;
    parse_decode_response(&resp)
}

// ─────────────────────────────────────────────────────────────────
// プロセス起動・ハンドシェイク
// ─────────────────────────────────────────────────────────────────

/// ワーカー exe のパス (診断表示用にも公開)。
/// 展開先は `<data_dir>/mimageviewer-susie32.exe`。環境変数
/// `MIV_SUSIE_WORKER` が指定されていればそちらを使う (テスト用)。
pub fn worker_exe_path() -> PathBuf {
    worker_exe_cached_path()
}

/// 診断情報: プール未起動の理由を UI に返すためのステート。
#[derive(Debug, Clone)]
pub enum PoolStatus {
    /// プール未初期化 (起動直後のバックグラウンド初期化が未完了)
    NotInitialized,
    /// ワーカー exe が見つからない (Susie サポート無効)
    WorkerExeMissing { expected_path: PathBuf },
    /// 設定で Susie を無効化している
    DisabledBySettings,
    /// 正常起動したがプラグインが 0 件
    ReadyButEmpty,
    /// 正常起動してプラグインもロード済み
    ReadyWithPlugins { count: usize },
    /// ワーカー起動に失敗した (exe はあるが spawn/handshake 失敗)
    WorkerSpawnFailed,
}

/// UI から状態を問い合わせる。プール未初期化でも軽量に判定できる。
///
/// `enabled` は呼び出し側の "今表示中の有効フラグ" (Preferences ダイアログなら
/// 編集中の `state.settings.susie_enabled`、それ以外なら `Settings::load()` の
/// `susie_enabled`) を渡す。これによりチェックボックス操作直後の表示と
/// 診断パネルが食い違わない。
pub fn pool_status(enabled: bool) -> PoolStatus {
    if !enabled {
        return PoolStatus::DisabledBySettings;
    }
    let exe = worker_exe_path();
    if !exe.exists() {
        return PoolStatus::WorkerExeMissing { expected_path: exe };
    }
    match try_get_pool() {
        None => PoolStatus::NotInitialized,
        Some(pool) if pool.is_ready() => {
            let n = pool.plugins().len();
            if n == 0 {
                PoolStatus::ReadyButEmpty
            } else {
                PoolStatus::ReadyWithPlugins { count: n }
            }
        }
        Some(_) => PoolStatus::WorkerSpawnFailed,
    }
}

fn spawn_worker_and_handshake(
    exe: &Path,
    plugin_dir: &Path,
) -> std::io::Result<(Child, WorkerIo, Vec<PluginInfo>)> {
    let mut cmd = Command::new(exe);
    cmd.stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt as _;
        cmd.creation_flags(CREATE_NO_WINDOW);
    }

    let mut child = cmd.spawn()?;
    let stdin = child
        .stdin
        .take()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "no stdin"))?;
    let stdout = child
        .stdout
        .take()
        .ok_or_else(|| std::io::Error::new(std::io::ErrorKind::Other, "no stdout"))?;
    let mut io = WorkerIo {
        stdin,
        stdout: BufReader::new(stdout),
    };

    // Handshake: プラグインフォルダを送信
    let req = encode_handshake_request(plugin_dir);
    write_msg(&mut io.stdin, &req)?;
    let resp = read_msg(&mut io.stdout)?;
    let plugins = parse_handshake_response(&resp)?;

    Ok((child, io, plugins))
}

fn run_dispatcher(
    worker_id: usize,
    queue: Arc<(Mutex<JobQueue>, Condvar)>,
    mut child: Child,
    mut io: WorkerIo,
) {
    let pid = child.id();
    loop {
        let job = {
            let (mtx, cv) = &*queue;
            let mut q = mtx.lock().unwrap();
            loop {
                if q.shutdown {
                    break None;
                }
                if let Some(j) = q.jobs.pop_front() {
                    break Some(j);
                }
                q = cv.wait(q).unwrap();
            }
        };
        let Some(job) = job else { break };

        let cancelled = job
            .cancel
            .as_ref()
            .is_some_and(|c| c.load(Ordering::Relaxed));
        if cancelled {
            let _ = job.reply.send(Err(std::io::Error::new(
                std::io::ErrorKind::Interrupted,
                "susie: cancelled in queue",
            )));
            continue;
        }

        let _ = job.enqueued_at; // 将来 perf 計装で使う
        let result = send_recv(&mut io, &job.request);
        let _ = job.reply.send(result);
    }

    crate::logger::log(format!(
        "susie: worker {worker_id} shutting down (pid={pid})"
    ));
    let _ = write_msg(&mut io.stdin, &[MSG_SHUTDOWN]);
    let _ = child.wait();
}

fn send_recv(io: &mut WorkerIo, request: &[u8]) -> std::io::Result<Vec<u8>> {
    write_msg(&mut io.stdin, request)?;
    read_msg(&mut io.stdout)
}

// ─────────────────────────────────────────────────────────────────
// バイナリフレーム IO
// ─────────────────────────────────────────────────────────────────

fn write_msg<W: Write>(w: &mut W, data: &[u8]) -> std::io::Result<()> {
    let len = data.len() as u32;
    w.write_all(&len.to_le_bytes())?;
    w.write_all(data)?;
    w.flush()
}

fn read_msg<R: Read>(r: &mut R) -> std::io::Result<Vec<u8>> {
    let mut len_buf = [0u8; 4];
    r.read_exact(&mut len_buf)?;
    let len = u32::from_le_bytes(len_buf) as usize;
    if len > 256 * 1024 * 1024 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("susie: message too large ({len} bytes)"),
        ));
    }
    let mut buf = vec![0u8; len];
    r.read_exact(&mut buf)?;
    Ok(buf)
}

// ─────────────────────────────────────────────────────────────────
// リクエスト / レスポンスのエンコード・デコード
// ─────────────────────────────────────────────────────────────────

fn encode_handshake_request(plugin_dir: &Path) -> Vec<u8> {
    let mut buf = Vec::with_capacity(32);
    buf.push(MSG_HANDSHAKE);
    let s = plugin_dir.to_string_lossy();
    let b = s.as_bytes();
    buf.extend_from_slice(&(b.len() as u16).to_le_bytes());
    buf.extend_from_slice(b);
    buf
}

fn parse_handshake_response(data: &[u8]) -> std::io::Result<Vec<PluginInfo>> {
    if data.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "susie: empty handshake response",
        ));
    }
    if data[0] == STATUS_ERR {
        let msg = std::str::from_utf8(&data[1..]).unwrap_or("unknown error");
        return Err(std::io::Error::new(std::io::ErrorKind::Other, msg));
    }
    if data[0] != STATUS_OK || data.len() < 3 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "susie: invalid handshake response",
        ));
    }
    let plugin_count = u16::from_le_bytes([data[1], data[2]]) as usize;
    let mut plugins = Vec::with_capacity(plugin_count);
    let mut off = 3;
    for _ in 0..plugin_count {
        if off >= data.len() {
            break;
        }
        let name_len = data[off] as usize;
        off += 1;
        if off + name_len > data.len() {
            break;
        }
        let name = std::str::from_utf8(&data[off..off + name_len])
            .unwrap_or("?")
            .to_string();
        off += name_len;
        if off + 2 > data.len() {
            break;
        }
        let ext_count = u16::from_le_bytes([data[off], data[off + 1]]) as usize;
        off += 2;
        let mut exts = Vec::with_capacity(ext_count);
        for _ in 0..ext_count {
            if off >= data.len() {
                break;
            }
            let el = data[off] as usize;
            off += 1;
            if off + el > data.len() {
                break;
            }
            let e = std::str::from_utf8(&data[off..off + el])
                .unwrap_or("")
                .to_string();
            off += el;
            if !e.is_empty() {
                exts.push(e);
            }
        }
        plugins.push(PluginInfo {
            name,
            extensions: exts,
        });
    }
    Ok(plugins)
}

fn encode_decode_file_request(path: &Path) -> Vec<u8> {
    let mut buf = Vec::with_capacity(64);
    buf.push(MSG_DECODE_FILE);
    let s = path.to_string_lossy();
    let b = s.as_bytes();
    buf.extend_from_slice(&(b.len() as u16).to_le_bytes());
    buf.extend_from_slice(b);
    buf
}

fn encode_decode_bytes_request(hint: &str, bytes: &[u8]) -> Vec<u8> {
    let mut buf = Vec::with_capacity(bytes.len() + 16);
    buf.push(MSG_DECODE_BYTES);
    let hb = hint.as_bytes();
    buf.extend_from_slice(&(hb.len() as u16).to_le_bytes());
    buf.extend_from_slice(hb);
    buf.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    buf.extend_from_slice(bytes);
    buf
}

fn parse_decode_response(data: &[u8]) -> std::io::Result<image::DynamicImage> {
    if data.is_empty() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "susie: empty decode response",
        ));
    }
    if data[0] == STATUS_ERR {
        let msg = std::str::from_utf8(&data[1..]).unwrap_or("unknown error");
        return Err(std::io::Error::new(std::io::ErrorKind::Other, msg));
    }
    if data[0] != STATUS_OK || data.len() < 9 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "susie: invalid decode response",
        ));
    }
    let w = u32::from_le_bytes([data[1], data[2], data[3], data[4]]);
    let h = u32::from_le_bytes([data[5], data[6], data[7], data[8]]);
    let pixels = &data[9..];
    let expected = (w as usize) * (h as usize) * 4;
    if pixels.len() != expected {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!(
                "susie: pixel size mismatch: expected {expected}, got {}",
                pixels.len()
            ),
        ));
    }
    // Worker は BGRA (top-down) を返す。image クレート (RGBA) へ変換。
    let mut rgba = Vec::with_capacity(expected);
    for chunk in pixels.chunks_exact(4) {
        rgba.push(chunk[2]); // R
        rgba.push(chunk[1]); // G
        rgba.push(chunk[0]); // B
        rgba.push(chunk[3]); // A
    }
    let img = image::RgbaImage::from_raw(w, h, rgba).ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "susie: RgbaImage::from_raw failed",
        )
    })?;
    Ok(image::DynamicImage::ImageRgba8(img))
}

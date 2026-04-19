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
use std::io::BufReader;
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

// 32bit ワーカー側の `crates/susie-worker/src/protocol.rs` を直接 include して
// 定数を共有する。ワーカーは別ターゲット (i686) でビルドされるため Cargo 依存は
// 張れないが、ファイル共有で MSG_* / STATUS_* がドリフトする事故を防げる。
#[path = "../crates/susie-worker/src/protocol.rs"]
mod susie_protocol;

use susie_protocol::{
    read_msg, write_msg, MSG_DECODE_BYTES, MSG_DECODE_FILE, MSG_HANDSHAKE, MSG_SHUTDOWN,
    STATUS_ERR, STATUS_OK,
};

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
    /// 診断ログ用ヒント (拡張子 or "(bytes:<filename>)")。リクエストペイロードを
    /// パースし直さずに済ませるための軽量メタ。
    hint: String,
    /// 可視セルのデコード要求は true。プールキュー先頭に push される。
    /// false (通常) は末尾に push される。スクロール中に画面外へ出た残存ジョブが
    /// キュー前方に居座って新しい可視セルを待たせる現象を回避する。
    priority: bool,
}

struct JobQueue {
    /// 可視セル等の優先ジョブ。dispatcher はこちらを先に pop する。
    /// 2 本に分けて FIFO(priority) → FIFO(regular) の順で処理することで、
    /// app 側が `worker_priority_key()` で決めた順序 (例: 可視セルを近い順) を
    /// キューが反転させない (旧実装は 1 本の VecDeque + push_front で priority 内が
    /// LIFO になり、serial worker 時に体感読み込み順が逆転した)。
    priority_jobs: std::collections::VecDeque<Job>,
    regular_jobs: std::collections::VecDeque<Job>,
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
                priority_jobs: std::collections::VecDeque::new(),
                regular_jobs: std::collections::VecDeque::new(),
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
        hint: &str,
        priority: bool,
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
            hint: hint.to_string(),
            priority,
        };
        {
            let (mtx, cv) = &*self.queue;
            let mut q = mtx.lock().unwrap();
            // priority ジョブは priority 用キューの末尾、regular は regular 用キューの
            // 末尾に積む。dispatcher は priority を先に pop するので、priority 同士は
            // FIFO、regular より常に先。これで app 側が決めた読み込み順を壊さない。
            if priority {
                q.priority_jobs.push_back(job);
            } else {
                q.regular_jobs.push_back(job);
            }
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
                priority_jobs: std::collections::VecDeque::new(),
                regular_jobs: std::collections::VecDeque::new(),
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

/// ある拡張子が Susie プラグインで扱えるか。
/// `folder_tree::is_recognized_image_ext` から呼ばれる判定。
///
/// プール未初期化時は `get_pool()` が handshake 完了まで待機するため、ここで
/// ブロックする可能性がある (通常数百 ms、バックグラウンド init スレッドが
/// 走っているならその join を待つだけ)。ネイティブ対応拡張子は
/// `is_recognized_image_ext` 内の `SUPPORTED_EXTENSIONS.contains` でショート
/// サーキットされるため、ここに来るのは非ネイティブ拡張子 (PI / MAG 等、または
/// 未知の拡張子) のみ。Susie を無効化している場合は `get_pool()` が即座に
/// `empty_pool()` を返すのでブロックしない。
///
/// 以前は `try_get_pool()` を使っていたが、起動直後の「last folder 復元」等で
/// プール初期化より先に ZIP / フォルダ列挙が走ると Susie 拡張子が false で
/// 返ってしまい、MAG / PI がサムネイル一覧から落ちる race があった (v0.7.0 修正)。
pub fn supports_extension(ext_lower: &str) -> bool {
    get_pool().supports_extension(ext_lower)
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
///
/// `priority = true` (可視セルなど) の場合、プールキュー先頭に挿入されて
/// すぐ処理される。スクロール中に画面外へ出た残存ジョブの後ろに並ぶのを避ける。
pub fn decode_file(
    path: &Path,
    priority: bool,
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
    let hint = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("?")
        .to_ascii_lowercase();
    let resp = pool.execute(&req, &hint, priority, cancel.as_ref())?;
    parse_decode_response(&resp)
}

/// メモリ上のバイト列からデコードする (ZIP 内画像用)。
pub fn decode_bytes(
    filename_hint: &str,
    bytes: &[u8],
    priority: bool,
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
    let hint = std::path::Path::new(filename_hint)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("?")
        .to_ascii_lowercase();
    let resp = pool.execute(&req, &hint, priority, cancel.as_ref())?;
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
    // 環境変数 MIV_SUSIE_PERF_LOG=1 で 1 ジョブごとの計測ログを出す。
    // (常時 ON だと数千枚のサムネイル一括ロード時にログが膨大になるため非推奨)
    let perf_log = std::env::var("MIV_SUSIE_PERF_LOG")
        .ok()
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));

    loop {
        let job = {
            let (mtx, cv) = &*queue;
            let mut q = mtx.lock().unwrap();
            loop {
                if q.shutdown {
                    break None;
                }
                // priority を先に、空なら regular を取り出す。どちらも FIFO なので
                // enqueue 順が保持される。
                if let Some(j) = q.priority_jobs.pop_front() {
                    break Some(j);
                }
                if let Some(j) = q.regular_jobs.pop_front() {
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

        // キュー待ち時間 = ディスパッチャが pop した時刻 - execute() が enqueue した時刻
        let dequeued_at = std::time::Instant::now();
        let queue_wait_ms = (dequeued_at - job.enqueued_at).as_secs_f64() * 1000.0;

        let req_size = job.request.len();
        let ipc_start = std::time::Instant::now();
        let result = send_recv(&mut io, &job.request);
        let ipc_ms = ipc_start.elapsed().as_secs_f64() * 1000.0;

        if perf_log {
            let resp_size = result.as_ref().map(|r| r.len()).unwrap_or(0);
            let status = if result.is_ok() { "OK " } else { "ERR" };
            let prio = if job.priority { "P" } else { "-" };
            crate::logger::log(format!(
                "susie: w{worker_id} {status} {prio} ext={:6} queue={:6.1}ms ipc={:7.1}ms req={}B resp={}B",
                job.hint, queue_wait_ms, ipc_ms, req_size, resp_size,
            ));
        }

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

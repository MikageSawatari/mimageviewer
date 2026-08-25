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
use std::sync::{Arc, Condvar, Mutex, OnceLock, RwLock, mpsc};

// -----------------------------------------------------------------------
// ワーカー exe の埋め込みと APPDATA への展開
// -----------------------------------------------------------------------

/// 32bit Susie ワーカー exe (PDFium DLL と同じパターンで本体 exe に埋め込む)。
/// 初回起動時に `%APPDATA%/mimageviewer/mimageviewer-susie32.exe` へ展開される。
/// インストール先 (Program Files) に書き込み不要で、本体 exe のフォルダにも
/// 追加ファイルを置かない。
/// portable ビルドでは埋め込まず exe 隣の loose exe を直接使う。
#[cfg(not(feature = "portable"))]
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
            // portable: 展開せず exe と同じディレクトリの loose worker exe を使う。
            // 存在しなければ後段の起動が is_ready=false になり UI にエラーが出る。
            #[cfg(feature = "portable")]
            {
                return crate::native_assets::bundled_root().join(WORKER_EXE_NAME);
            }
            #[cfg(not(feature = "portable"))]
            {
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
            }
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
    MSG_DECODE_BYTES, MSG_DECODE_FILE, MSG_HANDSHAKE, MSG_SHUTDOWN, STATUS_ERR, STATUS_OK,
    read_msg, write_msg,
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

/// ワーカーの健康状態の snapshot。診断パネルと通知の両方がここだけを読む。
///
/// 「一度も起動できなかった」と「起動したが尽きた」はどちらも `live_workers == 0`
/// になるが、利用者に伝えるべきことが違う (前者は起動そのものの失敗、後者は
/// 繰り返しのクラッシュ)。区別できるように起動時の数を残す。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SusieWorkerHealth {
    /// handshake まで成功した枠の数。0 なら一度も起動できていない。
    pub started_workers: usize,
    /// 今生きている枠の数。
    pub live_workers: usize,
    /// 枠を作り直した回数の合計。
    pub restarts: usize,
    /// 作り直しの上限に達して諦めた枠の数。
    pub gave_up_workers: usize,
    /// このセッションでワーカーを落とした対象の数。
    pub crashing_subjects: usize,
    /// 直近の失敗理由。応答が途切れた理由か、作り直しに失敗した理由。
    pub last_failure: Option<String>,
}

impl SusieWorkerHealth {
    /// 起動には成功したのに、今は 1 枠も残っていない。
    pub fn exhausted(&self) -> bool {
        self.started_workers > 0 && self.live_workers == 0
    }

    /// 起動時より枠が減っている。まだ読めるが余力が落ちている状態。
    pub fn degraded(&self) -> bool {
        self.live_workers > 0 && self.live_workers < self.started_workers
    }
}

/// スロットスレッドが書き込む集計。`SusieWorkerHealth` はここから作る。
#[derive(Debug, Default)]
struct HealthCounters {
    started_workers: usize,
    restarts: usize,
    gave_up_workers: usize,
    last_failure: Option<String>,
}

fn record_health(health: &Mutex<HealthCounters>, edit: impl FnOnce(&mut HealthCounters)) {
    let mut guard = health
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    edit(&mut guard);
}

/// 3 つの持ち主 (集計 / 生存数 / 落とした対象) から snapshot を組む。
/// **1 つの lock を持ったまま次を取らない**ように、順に読んで組み立てる。
fn health_snapshot(
    health: &Mutex<HealthCounters>,
    live_workers: &std::sync::atomic::AtomicUsize,
    crashers: &Mutex<HashSet<String>>,
) -> SusieWorkerHealth {
    let mut snapshot = {
        let counters = health
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        SusieWorkerHealth {
            started_workers: counters.started_workers,
            restarts: counters.restarts,
            gave_up_workers: counters.gave_up_workers,
            last_failure: counters.last_failure.clone(),
            ..SusieWorkerHealth::default()
        }
    };
    snapshot.live_workers = live_workers.load(std::sync::atomic::Ordering::Relaxed);
    snapshot.crashing_subjects = crashers.lock().map(|set| set.len()).unwrap_or(0);
    snapshot
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
    /// この要求が指す対象の識別子。ワーカーを落とした対象を覚えて二度と投げないために使う。
    /// 実ファイルは正規化パス、ZIP 内画像は「エントリ名#バイト長」。
    subject: String,
    /// 可視セルのデコード要求は true。プールキュー先頭に push される。
    /// false (通常) は末尾に push される。スクロール中に画面外へ出た残存ジョブが
    /// キュー前方に居座って新しい可視セルを待たせる現象を回避する。
    priority: bool,
}

/// 1 スロットが作り直しを試みる上限。プラグインが必ず落ちる画像を掴んだ場合、
/// 無限に作り直しても同じなので打ち切る。
const MAX_WORKER_RESTARTS: usize = 5;

/// 作り直しの間隔 (回数に比例して伸ばす)。crash loop が CPU を焼き切らないための
/// 間隔であって、競合を時間で隠すためのものではない。
const WORKER_RESTART_BACKOFF_MS: u64 = 200;

struct JobQueue {
    /// 可視セル等の優先ジョブ。dispatcher はこちらを先に pop する。
    /// 2 本に分けて FIFO(priority) → FIFO(regular) の順で処理することで、
    /// app 側が `worker_priority_key()` で決めた順序 (例: 可視セルを近い順) を
    /// キューが反転させない (旧実装は 1 本の VecDeque + push_front で priority 内が
    /// LIFO になり、serial worker 時に体感読み込み順が逆転した)。
    priority_jobs: std::collections::VecDeque<Job>,
    regular_jobs: std::collections::VecDeque<Job>,
    shutdown: bool,
    /// 最後のスロットが閉じた。以降 enqueue しても誰も pop しないので、
    /// ロックの中でこれを見て即座に断る。`is_ready()` の外側チェックだけでは、
    /// 見てから積むまでの間に最後のスロットが閉じた要求が宙に浮く。
    workers_gone: bool,
}

// ─────────────────────────────────────────────────────────────────
// SusieWorkerPool
// ─────────────────────────────────────────────────────────────────

pub struct SusieWorkerPool {
    queue: Arc<(Mutex<JobQueue>, Condvar)>,
    /// **生きているスロット数**。ワーカーが落ちて作り直せなくなるとここが減る。
    /// 起動時の本数で固定していたときは、全滅しても `is_ready()` が真を返し続け、
    /// 要求が誰にも拾われないまま失敗していた。
    live_workers: Arc<std::sync::atomic::AtomicUsize>,
    /// ロード済みプラグイン (全ワーカーで共通、handshake 応答をマージ済み)
    plugins: Vec<PluginInfo>,
    /// 拡張子の集合 (小文字)。全プラグインの対応拡張子を合算したもの。
    extensions: HashSet<String>,
    dispatcher_threads: Mutex<Vec<std::thread::JoinHandle<()>>>,
    /// このセッションでワーカーを落とした対象。二度目からは投げずに即失敗させる。
    crashers: Arc<Mutex<HashSet<String>>>,
    /// スロットの喪失・作り直しの集計。診断表示と通知の材料。
    health: Arc<Mutex<HealthCounters>>,
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
                workers_gone: false,
            }),
            Condvar::new(),
        ));

        let mut plugins_merged: Vec<PluginInfo> = Vec::new();
        let mut extensions: HashSet<String> = HashSet::new();
        let mut dispatcher_threads = Vec::with_capacity(pool_size);
        let mut worker_count = 0usize;
        let live_workers = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let crashers: Arc<Mutex<HashSet<String>>> = Arc::new(Mutex::new(HashSet::new()));
        let health: Arc<Mutex<HealthCounters>> = Arc::new(Mutex::new(HealthCounters::default()));

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
                    let slot_exe = exe.clone();
                    let slot_plugin_dir = plugin_dir.clone();
                    let slot_live = Arc::clone(&live_workers);
                    let slot_crashers = Arc::clone(&crashers);
                    let slot_health = Arc::clone(&health);
                    // スレッドを起こす**前**に数える。起動直後に落ちたワーカーが先に
                    // 減算すると、ループの後で `store` する形では 0 を下回って戻れない。
                    live_workers.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    let handle = std::thread::Builder::new()
                        .name(format!("susie-pool-{i}"))
                        .spawn(move || {
                            run_worker_slot(
                                i,
                                q,
                                slot_exe,
                                slot_plugin_dir,
                                (child, io),
                                slot_live,
                                slot_crashers,
                                slot_health,
                            )
                        })
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

        record_health(&health, |c| c.started_workers = worker_count);

        SusieWorkerPool {
            queue,
            live_workers,
            plugins: plugins_merged,
            extensions,
            dispatcher_threads: Mutex::new(dispatcher_threads),
            crashers,
            health,
        }
    }

    pub fn is_ready(&self) -> bool {
        self.live_workers.load(std::sync::atomic::Ordering::Relaxed) > 0
    }

    /// 生きているワーカースロット数。診断表示と、全滅の判定に使う。
    pub fn live_worker_count(&self) -> usize {
        self.live_workers.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// この対象は既にワーカーを落としているか。
    fn subject_killed_a_worker(&self, subject: &str) -> bool {
        self.crashers
            .lock()
            .map(|set| set.contains(subject))
            .unwrap_or(false)
    }

    /// このセッションでワーカーを落とした対象の数 (診断表示用)。
    pub fn crashing_subject_count(&self) -> usize {
        self.crashers.lock().map(|set| set.len()).unwrap_or(0)
    }

    /// 診断表示と通知が読む健康状態。
    pub fn health(&self) -> SusieWorkerHealth {
        health_snapshot(&self.health, &self.live_workers, &self.crashers)
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
        subject: &str,
        priority: bool,
        cancel: Option<&Arc<AtomicBool>>,
    ) -> std::io::Result<Vec<u8>> {
        // 一度ワーカーを落とした対象は投げ直さない。失敗したデコードはサムネイルとして
        // 残らないので、同じフォルダを開くたびに同じ画像が投げられ、そのたびにワーカーが
        // 死ぬ。再起動が追いつく間は読めるが、プロセスを作り直し続けることになる。
        // 記憶はプールの生存期間だけで、アプリを起動し直せば忘れる。
        if self.subject_killed_a_worker(subject) {
            return Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "susie: this file crashed a plugin earlier in this session",
            ));
        }
        // 生存スロットを見る。全滅した状態で enqueue すると、誰も pop しない
        // キューに積まれて要求が宙に浮く。
        if self.live_worker_count() == 0 {
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
            subject: subject.to_string(),
            priority,
        };
        {
            let (mtx, cv) = &*self.queue;
            let mut q = mtx.lock().unwrap();
            // 上の live_worker_count() を見てからここへ来るまでに最後のスロットが
            // 閉じることがある。積むのと同じロックの中で確かめる。
            if q.workers_gone || q.shutdown {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::Other,
                    "susie: no workers available",
                ));
            }
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
                workers_gone: false,
            }),
            Condvar::new(),
        )),
        live_workers: Arc::new(std::sync::atomic::AtomicUsize::new(0)),
        plugins: Vec::new(),
        extensions: HashSet::new(),
        dispatcher_threads: Mutex::new(Vec::new()),
        crashers: Arc::new(Mutex::new(HashSet::new())),
        health: Arc::new(Mutex::new(HealthCounters::default())),
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

/// `init_pool` 完了フラグ + 待機用 Condvar (Codex P2 v14 2026-05-14)。
///
/// 旧版は `get_pool()` 側で `POOL.get_or_init(|| empty_pool())` していたため、
/// `get_pool()` が `init_pool()` より先に呼ばれると **永久に empty_pool が
/// 採用されてしまう** 競合があった (= main の susie-init background thread より早く
/// 別 thread から `is_recognized_image_ext` 経由でアクセスするケース)。
///
/// 新版は `init_pool()` が完了するまで `get_pool()` を Condvar でブロックする。
/// 5 秒の timeout で empty_pool fallback に倒し、テストパス (= init_pool 未呼の状態
/// で `is_recognized_image_ext` を起動する) も hang しないようにする。
static INIT_DONE: std::sync::Mutex<bool> = std::sync::Mutex::new(false);
static INIT_COND: std::sync::Condvar = std::sync::Condvar::new();

/// `init_pool` / `reload` 操作の世代カウンタ (Codex P2 v14c 2026-05-14)。
///
/// 各操作の入口で `fetch_add(1)` して自分の世代をスナップショットし、heavy build
/// 完了後の swap 直前に `load()` で再確認する。値が変わっていれば「自分のビルド中に
/// 別の操作 (reload や 2 重 init) が走った」ことを意味するので、こちらの swap は
/// 諦めて build 物を捨てる。
///
/// このおかげで「main の susie-init thread が startup settings で build 中、ユーザーが
/// Preferences で reload して別 pool を入れる、その後 startup build が完了して
/// ユーザー設定を **上書き** してしまう」事故を防ぐ。
static INIT_GENERATION: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

/// プールを **明示的に** 初期化する (Phase 4: spec §8.2 2026-05-14)。
///
/// 旧版 `get_pool()` は内部で `Settings::load()` を呼んでいたが、Phase 4 で並列
/// `Settings::load()` を撲滅するため、`main.rs` の起動シーケンスから一度だけ呼ぶ
/// `init_pool(enabled, parallel)` に分離した。複数回呼ぶと最初の値が採用される。
///
/// **2 段階初期化** (Codex P2 v14b 2026-05-14):
/// 1. **Cheap install**: `POOL.get_or_init` で `RwLock<Arc<empty_pool>>` を **即座に**
///    入れる。OnceLock の init closure は数 µs で完了するので、他スレッドの
///    `OnceLock::get_or_init` が待たされない。
/// 2. **Heavy build**: `SusieWorkerPool::start` を OnceLock の **外側で** 実行する。
///    worker handshake が hang しても OnceLock は既に populate 済みなので、
///    `get_pool()` の timeout fallback (= 別の `get_or_init` 呼び出し) は即時 return できる。
/// 3. **Swap**: 完成したプールを `RwLock` に書き込む。
/// 4. **Notify**: `INIT_DONE = true` で待機者を起こす。
///
/// このため、handshake hang 中の状態は「OnceLock = Some(empty_pool)、INIT_DONE = false」となる。
/// `get_pool()` の待機者は INIT_TIMEOUT (= 5s) 後に empty_pool を取って先へ進める。
/// 後で hang が解けた場合は swap が走り、それ以降の `get_pool()` 呼び出しは real pool を見る。
pub fn init_pool(enabled: bool, parallel: bool) {
    use std::sync::atomic::Ordering;
    // Step 0: 世代スナップショット (Codex P2 v14c 2026-05-14)。
    let my_gen = INIT_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    // Step 1: cheap empty install.
    let rwlock = POOL.get_or_init(|| RwLock::new(Arc::new(empty_pool())));
    // Step 2: heavy build outside the OnceLock init.
    let pool = if enabled {
        SusieWorkerPool::start(parallel)
    } else {
        empty_pool()
    };
    // Step 3: 世代再確認 + swap。reload や別の init_pool が走っていれば
    // 自分の build 物は捨てる (= ユーザー choice / 最新値を尊重)。
    if INIT_GENERATION.load(Ordering::SeqCst) == my_gen {
        let mut writer = rwlock.write().unwrap_or_else(|e| e.into_inner());
        *writer = Arc::new(pool);
    } else {
        crate::logger::log(
            "susie_loader: init_pool: generation moved during build; \
             discarding stale pool (reload won)",
        );
        drop(pool);
    }
    // Step 4: signal (= swap したかどうかに関わらず、起動完了状態にする)。
    {
        let mut done = INIT_DONE.lock().unwrap_or_else(|e| e.into_inner());
        *done = true;
    }
    INIT_COND.notify_all();
}

/// 初期化済みプールへのハンドルを返す (アプリ全体の Susie 経路で使う)。
///
/// `init_pool()` がまだ完了していない場合は **完了まで block する** (Codex P2 v14)。
/// 5 秒以上待っても init_pool が呼ばれなければ、テスト等で誰も init_pool を
/// 呼ばないケースとして empty_pool に fallback する (= プロセス全体は動くが
/// Susie 機能は無効化された状態で進行)。`Settings::load()` には戻らない
/// (= boot race 防止、spec §8.2)。
pub fn get_pool() -> Arc<SusieWorkerPool> {
    // Fast-path: 既に init 完了済みなら即時返す。
    {
        let done = INIT_DONE.lock().unwrap_or_else(|e| e.into_inner());
        if *done {
            // POOL は init_pool で必ず populate されている前提だが、防御的に。
            if let Some(rwlock) = POOL.get() {
                return Arc::clone(&rwlock.read().unwrap());
            }
        }
    }
    // 待機 path: init_pool を待つ。timeout したら empty_pool。
    // 本番: 5s (= 多プラグイン環境の handshake が完了するまで余裕を見る)
    // テスト: 100ms (= init_pool 未呼のテスト経路で suite 全体が遅くなるのを防ぐ)
    const INIT_TIMEOUT_MS: u64 = if cfg!(test) { 100 } else { 5000 };
    let timeout = std::time::Duration::from_millis(INIT_TIMEOUT_MS);
    let mut done = INIT_DONE.lock().unwrap_or_else(|e| e.into_inner());
    while !*done {
        let (g, result) = INIT_COND.wait_timeout(done, timeout).unwrap_or_else(|e| {
            // poison 経路: lock の中身を取り出して timeout 扱いで継続。
            let g = e.into_inner();
            let result = g.1;
            (g.0, result)
        });
        done = g;
        if result.timed_out() && !*done {
            // timeout: empty_pool で永続化して終わる。
            crate::logger::log(&format!(
                "susie_loader: get_pool: init_pool not called within {INIT_TIMEOUT_MS}ms; \
                 installing empty_pool fallback"
            ));
            POOL.get_or_init(|| RwLock::new(Arc::new(empty_pool())));
            *done = true;
            break;
        }
    }
    let rwlock = POOL
        .get()
        .expect("POOL set by init_pool or timeout fallback");
    Arc::clone(&rwlock.read().unwrap())
}

/// テスト用: `INIT_DONE` フラグをリセットして次回 `init_pool` を再走らせる。
/// 本番では呼ばれない。
#[cfg(test)]
pub fn reset_init_for_test() {
    let mut done = INIT_DONE.lock().unwrap_or_else(|e| e.into_inner());
    *done = false;
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
///
/// 世代カウンタを上げてから build → swap する (Codex P2 v14c)。並行する
/// `init_pool` (= main の startup thread が build 中) が swap しようとしても、
/// 世代不一致で諦めるので user 設定が上書きされない。
///
/// reload 自体が複数回呼ばれたケースでも、最後の reload が世代の最新値を持って
/// swap するので「ユーザーが連続変更したときも最後の choice が反映される」性質を
/// 維持する。
pub fn reload(enabled: bool, parallel: bool) {
    use std::sync::atomic::Ordering;
    let my_gen = INIT_GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
    let lock = POOL.get_or_init(|| RwLock::new(Arc::new(empty_pool())));
    let new_pool = if enabled {
        SusieWorkerPool::start(parallel)
    } else {
        empty_pool()
    };
    if INIT_GENERATION.load(Ordering::SeqCst) == my_gen {
        // 旧プールの Drop がここで走る
        *lock.write().unwrap_or_else(|e| e.into_inner()) = Arc::new(new_pool);
    } else {
        crate::logger::log(
            "susie_loader: reload: generation moved during build; \
             discarding stale pool (later reload won)",
        );
        drop(new_pool);
    }
    // reload は init_pool より後に呼ばれる前提だが、念のため起動完了状態に
    // しておく (= init_pool が呼ばれずに reload だけのケースでも get_pool が即時返る)。
    {
        let mut done = INIT_DONE.lock().unwrap_or_else(|e| e.into_inner());
        *done = true;
    }
    INIT_COND.notify_all();
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
    let subject = crate::path_key::normalize_keep_drive(path);
    let resp = pool.execute(&req, &hint, &subject, priority, cancel.as_ref())?;
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
    // ZIP 内画像はパスを持たないので、エントリ名とバイト長で識別する。
    // 名前だけだと別の書庫の同名エントリまで巻き添えにする。
    let subject = format!("{filename_hint}#{}", bytes.len());
    let resp = pool.execute(&req, &hint, &subject, priority, cancel.as_ref())?;
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
    ReadyWithPlugins {
        count: usize,
        health: SusieWorkerHealth,
    },
    /// ワーカー起動に失敗した (exe はあるが spawn/handshake 失敗)
    WorkerSpawnFailed,
    /// 起動はできたが、繰り返しのクラッシュで作り直しの上限に達し、枠が尽きた。
    ///
    /// 以前はこれも `WorkerSpawnFailed` として出ていた。起動に失敗したと書かれるが
    /// 実際には起動できており、利用者が確認すべきものが違う。
    WorkersExhausted { health: SusieWorkerHealth },
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
        Some(pool) => pool_status_from(pool.plugins().len(), pool.health()),
    }
}

/// プールが在るときの状態判定。プロセスを持たない純関数にして、
/// 「起動できなかった」と「起動したが尽きた」の分岐をテストできるようにする。
fn pool_status_from(plugin_count: usize, health: SusieWorkerHealth) -> PoolStatus {
    if health.live_workers == 0 {
        return if health.exhausted() {
            PoolStatus::WorkersExhausted { health }
        } else {
            PoolStatus::WorkerSpawnFailed
        };
    }
    if plugin_count == 0 {
        PoolStatus::ReadyButEmpty
    } else {
        PoolStatus::ReadyWithPlugins {
            count: plugin_count,
            health,
        }
    }
}

// ─────────────────────────────────────────────────────────────────
// 全滅の一度きりの通知
// ─────────────────────────────────────────────────────────────────

static SUSIE_WORKER_NOTICE: Mutex<Option<SusieWorkerNotice>> = Mutex::new(None);

/// Susie 形式が読めなくなったことを UI へ 1 回だけ渡す typed notice。
///
/// 枠が 1 つ減っただけでは出さない。残りの枠が処理を続けられる間は利用者にできる
/// ことが無く、伝えても雑音にしかならない。**最後の枠が諦めたときだけ**発行する。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct SusieWorkerNotice {
    pub(crate) health: SusieWorkerHealth,
    pub(crate) logs_dir: PathBuf,
}

fn publish_worker_notice_to(slot: &Mutex<Option<SusieWorkerNotice>>, notice: SusieWorkerNotice) {
    let mut guard = slot.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    if guard.is_none() {
        *guard = Some(notice);
    }
}

fn take_worker_notice_from(slot: &Mutex<Option<SusieWorkerNotice>>) -> Option<SusieWorkerNotice> {
    slot.lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take()
}

/// App の update loop が poll する。取り出した notice は再送しない。
pub(crate) fn take_worker_notice() -> Option<SusieWorkerNotice> {
    take_worker_notice_from(&SUSIE_WORKER_NOTICE)
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

/// dispatcher の loop が終わった理由。
///
/// 以前は理由を持たず、`shutdown` でしか抜けなかった。ワーカーが落ちても loop は
/// 回り続け、**死んだ pipe を持つ dispatcher が共有キューから後続を取り続けた**。
/// しかも失敗は即座に返るので、生きているワーカーより速く job を吸う。
#[derive(Debug)]
enum DispatcherExit {
    /// プール終了。スロットも畳む。
    Shutdown,
    /// ワーカープロセスが応答しなくなった。スロットは再起動を試みる。
    ///
    /// `served` は**このワーカーが落ちるまでに返した応答数**。1 件でも返していれば
    /// そのワーカーは正常に働けていたので、再起動回数を数え直す。累積で数えると、
    /// たまに落ちるプラグインでも使い続けるうちに必ず枯渇する。
    WorkerLost { reason: String, served: u64 },
}

/// ワーカーが落ちた後の再起動回数。**1 件でも応答を返せていたワーカーは働けていた**
/// ので数え直す。累積で数えると、たまに落ちるプラグインでも使い続けるうちに必ず上限へ
/// 達し、Susie が丸ごと死ぬ (実機で 3 スロットとも枯渇した)。上限が意味を持つのは
/// 「起動しても働けないまま落ちる」場合だけである。
fn restart_count_after_loss(current: usize, served: u64) -> usize {
    if served > 0 { 0 } else { current }
}

/// ワーカーが落ちたと判断できる transport error か。
///
/// プロトコル違反 (`InvalidData`) は含めない。それはワーカーが生きたまま壊れた
/// 応答を返した場合で、再起動しても同じ結果になる。
fn is_worker_lost(err: &std::io::Error) -> bool {
    matches!(
        err.kind(),
        std::io::ErrorKind::UnexpectedEof
            | std::io::ErrorKind::BrokenPipe
            | std::io::ErrorKind::ConnectionAborted
            | std::io::ErrorKind::ConnectionReset
    )
}

/// 1 スロットの生涯。ワーカーが落ちたら backoff を挟んで作り直し、同じキューへ戻す。
///
/// **クラッシュを起こした要求は再送しない。** 同じ不正画像で crash loop になるためで、
/// その 1 件はエラーとして返し、後続のためだけにワーカーを補充する。
fn run_worker_slot(
    worker_id: usize,
    queue: Arc<(Mutex<JobQueue>, Condvar)>,
    exe: PathBuf,
    plugin_dir: PathBuf,
    initial: (Child, WorkerIo),
    live_workers: Arc<std::sync::atomic::AtomicUsize>,
    crashers: Arc<Mutex<HashSet<String>>>,
    health: Arc<Mutex<HealthCounters>>,
) {
    let mut current = Some(initial);
    let mut restarts = 0usize;

    let exit = loop {
        let Some((child, io)) = current.take() else {
            // ワーカーが居ない状態。上限まで作り直しを試みる。
            if restarts >= MAX_WORKER_RESTARTS {
                crate::logger::log(format!(
                    "susie: worker {worker_id} reached the restart limit ({MAX_WORKER_RESTARTS})"
                ));
                record_health(&health, |c| c.gave_up_workers += 1);
                break SlotExit::GaveUp;
            }
            restarts += 1;
            record_health(&health, |c| c.restarts += 1);
            // 落ちた直後に作り直しても同じ画像でまた落ちることがある。時間で競合を
            // 隠すためではなく、crash loop が CPU を焼き切らないための間隔。
            std::thread::sleep(std::time::Duration::from_millis(
                WORKER_RESTART_BACKOFF_MS * restarts as u64,
            ));
            if queue.0.lock().map(|q| q.shutdown).unwrap_or(true) {
                break SlotExit::Shutdown;
            }
            match spawn_worker_and_handshake(&exe, &plugin_dir) {
                Ok((child, io, plugins)) => {
                    crate::logger::log(format!(
                        "susie: worker {worker_id} restarted (pid={}, plugins={}, attempt {restarts})",
                        child.id(),
                        plugins.len()
                    ));
                    current = Some((child, io));
                }
                Err(e) => {
                    crate::logger::log(format!(
                        "susie: worker {worker_id} restart {restarts} failed: {e}"
                    ));
                    record_health(&health, |c| {
                        c.last_failure = Some(format!("restart failed: {e}"))
                    });
                }
            }
            continue;
        };

        match run_dispatcher(worker_id, Arc::clone(&queue), child, io, &crashers) {
            DispatcherExit::Shutdown => break SlotExit::Shutdown,
            DispatcherExit::WorkerLost { reason, served } => {
                crate::logger::log(format!(
                    "susie: worker {worker_id} lost after {served} reply/replies ({reason})"
                ));
                record_health(&health, |c| c.last_failure = Some(reason));
                restarts = restart_count_after_loss(restarts, served);
                // current は None のまま次の周回へ。そこで作り直す。
            }
        }
    };

    finish_worker_slot(
        worker_id,
        exit,
        &queue,
        &live_workers,
        &crashers,
        &health,
        &SUSIE_WORKER_NOTICE,
    );
}

/// スロットが閉じるときの後始末。**プロセスを持たないのでテストから直接呼べる。**
///
/// 通知を出すかどうかの配線はここにしか無い。無言で出ない / 終了時に出る、のどちらも
/// 実機でしか気付けない類の失敗なので、経路ごと確かめられる形にしてある。
fn finish_worker_slot(
    worker_id: usize,
    exit: SlotExit,
    queue: &Arc<(Mutex<JobQueue>, Condvar)>,
    live_workers: &std::sync::atomic::AtomicUsize,
    crashers: &Mutex<HashSet<String>>,
    health: &Mutex<HealthCounters>,
    notice_slot: &Mutex<Option<SusieWorkerNotice>>,
) -> usize {
    let remaining = live_workers.fetch_sub(1, std::sync::atomic::Ordering::Relaxed) - 1;
    crate::logger::log(format!(
        "susie: worker {worker_id} slot closed ({exit:?}); {remaining} worker slot(s) left"
    ));
    if remaining == 0 {
        // 最後の一本。積まれたままの要求を誰も拾えないので、待たせ続けずに返す。
        // これが無いと呼び出し側は応答を永久に待ち、UI は「読込中」で固まる。
        let drained = fail_pending_jobs_no_workers(queue);
        if drained > 0 {
            crate::logger::log(format!(
                "susie: no workers left; failed {drained} pending request(s)"
            ));
        }
    }
    if should_notify_workers_gone(exit, remaining) {
        let snapshot = health_snapshot(health, live_workers, crashers);
        crate::logger::log(format!(
            "susie: no workers left after {} restart(s); notifying the user (started={}, crashers={})",
            snapshot.restarts, snapshot.started_workers, snapshot.crashing_subjects
        ));
        publish_worker_notice_to(
            notice_slot,
            SusieWorkerNotice {
                health: snapshot,
                logs_dir: crate::data_dir::logs_dir(),
            },
        );
    }
    remaining
}

/// スロットが閉じた理由。**通知するかどうかがこれで決まる**ので、抜けた場所で
/// 理由を確定させる。後からフラグを読み直すと、閉じてから読むまでの間に
/// 終了要求が来た場合に「自分から畳んだ」と誤って読める。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SlotExit {
    /// プール終了 (Drop / 再読み込み)。利用者に伝えることは無い。
    Shutdown,
    /// 作り直しの上限に達した。この枠はもう戻らない。
    GaveUp,
}

/// 最後の枠が諦めたときだけ通知する。
///
/// 枠が 1 つ減っただけでは出さない。残りの枠が処理を続けられる間、利用者にできる
/// ことは無い。終了・再読み込みで畳んだ場合も出さない。
fn should_notify_workers_gone(exit: SlotExit, remaining: usize) -> bool {
    remaining == 0 && matches!(exit, SlotExit::GaveUp)
}

/// 全スロットが閉じたときに、キューに残った要求へエラーを返す。
/// 以降の enqueue も `workers_gone` で断る。判定と排出を同じロックの中で行うので、
/// 「空きを見てから積む」間に閉じた要求も取りこぼさない。
fn fail_pending_jobs_no_workers(queue: &Arc<(Mutex<JobQueue>, Condvar)>) -> usize {
    let (mtx, cv) = &**queue;
    let mut drained = 0usize;
    if let Ok(mut q) = mtx.lock() {
        q.workers_gone = true;
        let pending: Vec<Job> = q
            .priority_jobs
            .drain(..)
            .collect::<Vec<_>>()
            .into_iter()
            .chain(q.regular_jobs.drain(..))
            .collect();
        for job in pending {
            let _ = job.reply.send(Err(std::io::Error::new(
                std::io::ErrorKind::Other,
                "susie: no workers available",
            )));
            drained += 1;
        }
    }
    cv.notify_all();
    drained
}

fn run_dispatcher(
    worker_id: usize,
    queue: Arc<(Mutex<JobQueue>, Condvar)>,
    mut child: Child,
    mut io: WorkerIo,
    crashers: &Mutex<HashSet<String>>,
) -> DispatcherExit {
    let pid = child.id();
    // 環境変数 MIV_SUSIE_PERF_LOG=1 で 1 ジョブごとの計測ログを出す。
    // (常時 ON だと数千枚のサムネイル一括ロード時にログが膨大になるため非推奨)
    let perf_log = std::env::var("MIV_SUSIE_PERF_LOG")
        .ok()
        .is_some_and(|v| v == "1" || v.eq_ignore_ascii_case("true"));
    // このワーカーが返した応答数。落ちたときに「働けていたか」を伝えるために数える。
    let mut served: u64 = 0;

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
        let Some(job) = job else {
            crate::logger::log(format!(
                "susie: worker {worker_id} shutting down (pid={pid})"
            ));
            let _ = write_msg(&mut io.stdin, &[MSG_SHUTDOWN]);
            let _ = child.wait();
            return DispatcherExit::Shutdown;
        };

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

        // ワーカーが応答しなくなったら、この 1 件はエラーで返して loop を抜ける。
        // **要求は再送しない** — 同じ画像でまた落ちるなら crash loop になる。
        // 抜けずに回り続けると、死んだ pipe を持つこの dispatcher が共有キューから
        // 後続を取り続け、しかも失敗が即座に返るぶん生きたワーカーより速く吸ってしまう。
        let lost = match &result {
            Err(e) if is_worker_lost(e) => Some(format!("{e}")),
            _ => None,
        };
        let subject = job.subject.clone();
        if lost.is_none() {
            served += 1;
        }
        let _ = job.reply.send(result);
        if let Some(reason) = lost {
            // この対象がワーカーを落とした。次からは投げずに即失敗させる。
            // 記録しないと、同じフォルダを開くたびに同じ画像でワーカーが死に続ける
            // (失敗したデコードはサムネイルとして残らないので毎回投げられる)。
            if let Ok(mut set) = crashers.lock() {
                if set.insert(subject.clone()) {
                    crate::logger::log(format!(
                        "susie: {subject} crashed a plugin; it will not be sent again this session"
                    ));
                }
            }
            // 既に死んでいる想定だが、handle を確実に閉じる。
            let _ = child.kill();
            let _ = child.wait();
            return DispatcherExit::WorkerLost { reason, served };
        }
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// ワーカーが落ちたと判断してよいのは transport が切れたときだけ。
    /// プロトコル違反や画像側のエラーで作り直すと、同じ結果を繰り返す。
    #[test]
    fn only_a_broken_transport_counts_as_a_lost_worker() {
        use std::io::{Error, ErrorKind};

        for kind in [
            ErrorKind::UnexpectedEof,
            ErrorKind::BrokenPipe,
            ErrorKind::ConnectionAborted,
            ErrorKind::ConnectionReset,
        ] {
            assert!(
                is_worker_lost(&Error::new(kind, "gone")),
                "{kind:?} should be treated as a lost worker"
            );
        }

        for kind in [
            ErrorKind::InvalidData,
            ErrorKind::NotFound,
            ErrorKind::PermissionDenied,
            ErrorKind::Other,
        ] {
            assert!(
                !is_worker_lost(&Error::new(kind, "still alive")),
                "{kind:?} must not restart a worker that is still answering"
            );
        }
    }

    /// 1 件でも返せたワーカーが落ちたら、再起動回数は数え直す。数え続けると、
    /// たまに落ちるプラグインでも使い続けるうちに必ず枯渇する (実機で 3 スロットとも
    /// 上限に達して Susie が丸ごと死んだ)。上限が働くのは「起動しても何も返せないまま
    /// 落ちる」場合だけでよい。
    #[test]
    fn a_worker_that_answered_before_dying_does_not_count_toward_the_limit() {
        assert_eq!(restart_count_after_loss(3, 1), 0);
        assert_eq!(restart_count_after_loss(MAX_WORKER_RESTARTS, 42), 0);
        // 何も返せずに落ちたときだけ、回数を持ち越す。
        assert_eq!(restart_count_after_loss(3, 0), 3);
        assert_eq!(restart_count_after_loss(0, 0), 0);
    }

    /// 全スロットが閉じたら、積まれたままの要求へエラーを返す。返さないと呼び出し側は
    /// 応答を永久に待ち、UI は「読込中」で固まる (実機で確認)。
    #[test]
    fn the_last_slot_closing_fails_whatever_is_still_queued() {
        let queue = Arc::new((
            Mutex::new(JobQueue {
                priority_jobs: std::collections::VecDeque::new(),
                regular_jobs: std::collections::VecDeque::new(),
                shutdown: false,
                workers_gone: false,
            }),
            Condvar::new(),
        ));
        let mut receivers = Vec::new();
        for priority in [true, false, false] {
            let (tx, rx) = mpsc::channel();
            receivers.push(rx);
            let job = Job {
                request: vec![1, 2, 3],
                reply: tx,
                cancel: None,
                enqueued_at: std::time::Instant::now(),
                hint: "test".to_string(),
                subject: format!("test-{priority}"),
                priority,
            };
            let mut q = queue.0.lock().unwrap();
            if priority {
                q.priority_jobs.push_back(job);
            } else {
                q.regular_jobs.push_back(job);
            }
        }

        assert_eq!(fail_pending_jobs_no_workers(&queue), 3);
        for rx in receivers {
            let answer = rx.try_recv().expect("every queued job must be answered");
            assert!(answer.is_err(), "a queued job must not be left waiting");
        }

        let q = queue.0.lock().unwrap();
        assert!(
            q.workers_gone,
            "later requests must be refused rather than queued"
        );
        assert!(q.priority_jobs.is_empty() && q.regular_jobs.is_empty());
    }

    /// 一度ワーカーを落とした対象は、二度目から投げずに即失敗させる。失敗した
    /// デコードはサムネイルとして残らないので、記録しないと同じフォルダを開くたびに
    /// 同じ画像が投げられ、そのたびにワーカーが死ぬ (実機で確認)。
    #[test]
    fn a_subject_that_killed_a_worker_is_not_sent_again() {
        let pool = empty_pool();
        assert!(!pool.subject_killed_a_worker("c:/books/bad.pi"));
        assert_eq!(pool.crashing_subject_count(), 0);

        pool.crashers
            .lock()
            .unwrap()
            .insert("c:/books/bad.pi".to_string());

        assert!(pool.subject_killed_a_worker("c:/books/bad.pi"));
        assert_eq!(pool.crashing_subject_count(), 1);
        // 別の対象は巻き添えにしない。
        assert!(!pool.subject_killed_a_worker("c:/books/good.pi"));
    }

    /// 作り直しの間隔は回数に比例して伸び、上限で止まる。crash loop が CPU を
    /// 焼き切らないための間隔なので、0 にはしない。
    #[test]
    fn the_restart_backoff_grows_and_is_bounded() {
        let delays: Vec<u64> = (1..=MAX_WORKER_RESTARTS)
            .map(|attempt| WORKER_RESTART_BACKOFF_MS * attempt as u64)
            .collect();
        assert_eq!(delays.len(), MAX_WORKER_RESTARTS);
        assert!(delays.iter().all(|&d| d > 0));
        assert!(delays.windows(2).all(|w| w[1] > w[0]));
        // 上限まで使い切っても、待ち時間の合計が体感を壊すほど長くならないこと。
        assert!(delays.iter().sum::<u64>() < 5_000);
    }

    /// 起動できなかった場合と、起動したのに尽きた場合を取り違えない。
    ///
    /// 以前はどちらも `WorkerSpawnFailed` になり、繰り返しクラッシュで枠を使い切った
    /// 利用者に「起動またはハンドシェイクに失敗しました」と出ていた。実際には起動して
    /// 動いていたので、確認すべきものが違う。
    #[test]
    fn a_pool_that_ran_and_then_died_is_not_reported_as_a_failed_start() {
        let never_started = SusieWorkerHealth {
            started_workers: 0,
            live_workers: 0,
            ..SusieWorkerHealth::default()
        };
        assert!(matches!(
            pool_status_from(0, never_started),
            PoolStatus::WorkerSpawnFailed
        ));

        let exhausted = SusieWorkerHealth {
            started_workers: 3,
            live_workers: 0,
            restarts: 5,
            gave_up_workers: 3,
            ..SusieWorkerHealth::default()
        };
        assert!(matches!(
            pool_status_from(2, exhausted),
            PoolStatus::WorkersExhausted { .. }
        ));
    }

    /// 生きている枠が 1 つでもあれば通常表示のまま。減っていることは診断側で示す。
    #[test]
    fn a_partly_lost_pool_still_reports_its_plugins() {
        let degraded = SusieWorkerHealth {
            started_workers: 3,
            live_workers: 1,
            restarts: 2,
            ..SusieWorkerHealth::default()
        };
        assert!(degraded.degraded());
        assert!(!degraded.exhausted());
        match pool_status_from(4, degraded) {
            PoolStatus::ReadyWithPlugins { count, health } => {
                assert_eq!(count, 4);
                assert_eq!(health.live_workers, 1);
            }
            other => panic!("unexpected status: {other:?}"),
        }

        let healthy = SusieWorkerHealth {
            started_workers: 3,
            live_workers: 3,
            ..SusieWorkerHealth::default()
        };
        assert!(!healthy.degraded());
        assert!(matches!(
            pool_status_from(0, healthy),
            PoolStatus::ReadyButEmpty
        ));
    }

    /// 通知は「最後の枠が諦めたとき」だけ。枠が減っただけ、終了で畳んだだけでは出さない。
    #[test]
    fn only_the_last_slot_giving_up_notifies_the_user() {
        assert!(should_notify_workers_gone(SlotExit::GaveUp, 0));
        // まだ残っている枠が処理を続けられる。利用者にできることは無い。
        assert!(!should_notify_workers_gone(SlotExit::GaveUp, 1));
        // 終了 / 再読み込みで畳んだ場合。全部閉じるのが正常な経路である。
        assert!(!should_notify_workers_gone(SlotExit::Shutdown, 0));
        assert!(!should_notify_workers_gone(SlotExit::Shutdown, 2));
    }

    /// notice は 1 回だけ取り出せる。取り出した後に再送しない。
    #[test]
    fn the_workers_gone_notice_is_delivered_once() {
        let slot: Mutex<Option<SusieWorkerNotice>> = Mutex::new(None);
        let notice = SusieWorkerNotice {
            health: SusieWorkerHealth {
                started_workers: 3,
                live_workers: 0,
                restarts: 5,
                gave_up_workers: 3,
                crashing_subjects: 1,
                last_failure: Some("broken pipe".to_string()),
            },
            logs_dir: PathBuf::from(r"C:\data\logs"),
        };

        publish_worker_notice_to(&slot, notice.clone());
        // 後から届いた 2 通目で上書きしない。最初の理由の方が原因に近い。
        publish_worker_notice_to(
            &slot,
            SusieWorkerNotice {
                health: SusieWorkerHealth::default(),
                logs_dir: PathBuf::from(r"C:\other"),
            },
        );

        assert_eq!(take_worker_notice_from(&slot), Some(notice));
        assert_eq!(take_worker_notice_from(&slot), None);
    }

    /// スロットが閉じるときの後始末を、理由と残り枠数の組み合わせごとに確かめる。
    ///
    /// 単体の述語 (`should_notify_workers_gone`) が正しくても、呼び出し側の配線を
    /// 間違えれば通知は無言で出ない / 終了のたびに出る。そこを分けて確かめる。
    fn drive_slot_close(exit: SlotExit, slots: usize) -> (usize, Option<SusieWorkerNotice>, bool) {
        let pool = empty_pool();
        record_health(&pool.health, |c| {
            c.started_workers = slots;
            c.restarts = 5;
            c.last_failure = Some("unexpected end of file".to_string());
        });
        pool.live_workers
            .store(slots, std::sync::atomic::Ordering::Relaxed);
        let notice_slot: Mutex<Option<SusieWorkerNotice>> = Mutex::new(None);

        let remaining = finish_worker_slot(
            0,
            exit,
            &pool.queue,
            &pool.live_workers,
            &pool.crashers,
            &pool.health,
            &notice_slot,
        );
        let workers_gone = pool.queue.0.lock().unwrap().workers_gone;
        (
            remaining,
            take_worker_notice_from(&notice_slot),
            workers_gone,
        )
    }

    #[test]
    fn the_last_slot_giving_up_is_what_reaches_the_user() {
        let (remaining, notice, workers_gone) = drive_slot_close(SlotExit::GaveUp, 1);
        assert_eq!(remaining, 0);
        assert!(workers_gone, "以降の enqueue を断る印が立っていない");
        let notice = notice.expect("最後の枠が諦めたのに通知が出ていない");
        assert_eq!(notice.health.started_workers, 1);
        assert_eq!(notice.health.live_workers, 0);
        assert_eq!(
            notice.health.last_failure.as_deref(),
            Some("unexpected end of file")
        );
    }

    #[test]
    fn closing_one_of_several_slots_says_nothing_to_the_user() {
        let (remaining, notice, workers_gone) = drive_slot_close(SlotExit::GaveUp, 3);
        assert_eq!(remaining, 2);
        assert!(notice.is_none(), "まだ読めるのに通知が出ている");
        assert!(!workers_gone, "まだ枠が残っているのにキューを閉じている");
    }

    /// 終了・再読み込みでは全部の枠が閉じる。これは正常な経路なので通知しない。
    #[test]
    fn shutting_down_closes_every_slot_without_telling_the_user() {
        let (remaining, notice, workers_gone) = drive_slot_close(SlotExit::Shutdown, 1);
        assert_eq!(remaining, 0);
        assert!(notice.is_none(), "終了しただけで通知が出ている");
        // 通知はしないが、積まれたままの要求は返す (待たせ続けない)。
        assert!(workers_gone);
    }

    /// 起動した枠の数は集計へ記録され、snapshot から読める。
    #[test]
    fn the_health_snapshot_reads_from_all_three_owners() {
        let pool = empty_pool();
        record_health(&pool.health, |c| {
            c.started_workers = 3;
            c.restarts = 2;
            c.last_failure = Some("broken pipe".to_string());
        });
        pool.live_workers
            .store(1, std::sync::atomic::Ordering::Relaxed);
        pool.crashers.lock().unwrap().insert("bad.pi".to_string());

        let health = pool.health();
        assert_eq!(health.started_workers, 3);
        assert_eq!(health.live_workers, 1);
        assert_eq!(health.restarts, 2);
        assert_eq!(health.crashing_subjects, 1);
        assert_eq!(health.last_failure.as_deref(), Some("broken pipe"));
    }
}

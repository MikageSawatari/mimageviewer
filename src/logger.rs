/// シンプルなファイルロガー（パフォーマンス分析用）
///
/// ログは mimageviewer.log に出力される。
/// 書式: [経過秒数][スレッドID] メッセージ
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static FILE: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

/// メインプロセス用 logger 初期化。`mimageviewer.log` を truncate して書き始める。
pub fn init() {
    init_inner("mimageviewer.log", /* truncate = */ true);
}

/// ワーカープロセス用 logger 初期化。**メインの mimageviewer.log を truncate せず**、
/// ワーカー専用ファイル `<worker_kind>.log` に append する。
///
/// 以前は worker も `init()` を呼んでいたため、parent が書いた直近の mimageviewer.log
/// を truncate で消し飛ばしてしまい、起動失敗時のデバッグ情報が消えていた
/// (Codex P3 指摘 / Apr 28-29 の trt_worker 死亡解析でも実害)。
///
/// `worker_kind` は識別用の短い名前 (例: "trt-worker", "pdf-worker")。複数のワーカーが
/// 並走しても別ファイルに書き分けられる。
pub fn init_for_worker(worker_kind: &str) {
    init_inner(&format!("{worker_kind}.log"), /* truncate = */ false);
}

fn init_inner(file_name: &str, truncate: bool) {
    START.set(Instant::now()).ok();
    let log_dir = crate::data_dir::logs_dir();
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join(file_name);
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true);
    if truncate {
        opts.truncate(true);
    } else {
        opts.append(true);
    }
    match opts.open(&log_path) {
        Ok(f) => {
            FILE.set(Mutex::new(f)).ok();
        }
        Err(e) => eprintln!("ログファイル作成失敗: {e} (path: {})", log_path.display()),
    }
}

pub fn log(msg: impl AsRef<str>) {
    let elapsed = START
        .get()
        .map(|s| s.elapsed().as_secs_f64())
        .unwrap_or(0.0);

    let tid_num = current_thread_id_num()
        .map(|n| n.to_string())
        .unwrap_or_else(|| "?".to_owned());

    if let Some(file) = FILE.get() {
        if let Ok(mut f) = file.lock() {
            let _ = writeln!(f, "[{elapsed:>8.3}s][t{tid_num:>3}] {}", msg.as_ref());
            let _ = f.flush();
        }
    }
}

/// 現在スレッドの ID から数字部分だけを取り出す。
/// `ThreadId(N)` → `Some(N)`、パースできなければ `None`。
/// `logger` と `perf` の両方から参照される共通ヘルパ。
pub fn current_thread_id_num() -> Option<u64> {
    let tid = format!("{:?}", std::thread::current().id());
    tid.trim_start_matches("ThreadId(")
        .trim_end_matches(')')
        .parse::<u64>()
        .ok()
}

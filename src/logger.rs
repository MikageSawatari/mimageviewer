/// シンプルなファイルロガー（パフォーマンス分析用）
///
/// ログは mimageviewer.log に出力される。
/// 書式: [経過秒数][スレッドID] メッセージ
use std::io::Write;
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static START: OnceLock<Instant> = OnceLock::new();
static FILE: OnceLock<Mutex<std::fs::File>> = OnceLock::new();

pub fn init() {
    START.set(Instant::now()).ok();
    let log_dir = crate::data_dir::logs_dir();
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("mimageviewer.log");
    match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_path)
    {
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

    // "ThreadId(N)" → "N" に短縮
    let tid = format!("{:?}", std::thread::current().id());
    let tid_num = tid
        .trim_start_matches("ThreadId(")
        .trim_end_matches(')');
    let tid_num = if tid_num.parse::<u64>().is_ok() {
        tid_num.to_owned()
    } else {
        "?".to_owned()
    };

    if let Some(file) = FILE.get() {
        if let Ok(mut f) = file.lock() {
            let _ = writeln!(
                f,
                "[{elapsed:>8.3}s][t{tid_num:>3}] {}",
                msg.as_ref()
            );
            let _ = f.flush();
        }
    }
}

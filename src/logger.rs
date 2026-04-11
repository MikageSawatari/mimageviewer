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
    match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open("mimageviewer.log")
    {
        Ok(f) => {
            FILE.set(Mutex::new(f)).ok();
        }
        Err(e) => eprintln!("ログファイル作成失敗: {e}"),
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

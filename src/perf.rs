//! パフォーマンス計装用の構造化イベントログ (JSON Lines)。
//!
//! 既存の `logger.rs` (人間可読フラットテキスト) と併存する。
//! `--perf-log` 起動引数が指定された場合のみ有効化し、
//! 無効時は `is_enabled()` の Atomic 読みのみで即 return する。
//!
//! 出力先: `%APPDATA%\mimageviewer\logs\perf_events.jsonl`
//! (起動毎に truncate)
//!
//! 行フォーマット:
//! ```text
//! {"t":12.345,"tid":5,"cat":"fs","kind":"paint","key":"C:\\a.jpg","seq":42,"decode_ms":15.2}
//! ```
//!
//! - `t`: 起動からの経過秒 (f64, 3 桁)
//! - `tid`: `ThreadId` から数字部分のみ
//! - `cat`: イベントカテゴリ (input / fs / thumb / pdf / ai / frame など)
//! - `kind`: begin / end / request / ready / paint / enqueue / pick / skip 等
//! - `key`: 画像・ページ識別キー (省略可)
//! - `seq`: 相関する input_seq (省略時は 0)
//! - その他: 呼び出し側が任意キーで追加できる (extras)

use serde_json::Value;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);
static START: OnceLock<Instant> = OnceLock::new();
static FILE: OnceLock<Mutex<BufWriter<File>>> = OnceLock::new();

/// 起動時に 1 回だけ呼ぶ。`enabled=false` なら何もしない。
pub fn init(enabled: bool) {
    if !enabled {
        return;
    }
    START.set(Instant::now()).ok();
    let log_dir = crate::data_dir::logs_dir();
    let _ = std::fs::create_dir_all(&log_dir);
    let log_path = log_dir.join("perf_events.jsonl");
    match std::fs::OpenOptions::new()
        .create(true)
        .write(true)
        .truncate(true)
        .open(&log_path)
    {
        Ok(f) => {
            if FILE.set(Mutex::new(BufWriter::with_capacity(64 * 1024, f))).is_ok() {
                ENABLED.store(true, Ordering::Release);
                crate::logger::log(format!(
                    "perf: JSONL log enabled at {}",
                    log_path.display()
                ));
            }
        }
        Err(e) => {
            eprintln!("perf ログファイル作成失敗: {e} (path: {})", log_path.display());
            crate::logger::log(format!("perf: init failed: {e}"));
        }
    }
}

/// ホットパスで先頭に挟むチェック関数。
#[inline]
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// 1 イベントを 1 行書き込む。`is_enabled()` が false なら即 return。
///
/// `key` と `seq` はそれぞれ省略可能 (None / 0)。
/// `extras` は serde_json 可能な任意キー/値ペア。
pub fn event(cat: &str, kind: &str, key: Option<&str>, seq: u64, extras: &[(&str, Value)]) {
    if !is_enabled() {
        return;
    }
    let Some(start) = START.get() else { return };
    let Some(file) = FILE.get() else { return };

    let t = start.elapsed().as_secs_f64();
    let tid = thread_id_num();

    // serde_json::Map で構築 → 1 行シリアライズ
    let mut map = serde_json::Map::with_capacity(6 + extras.len());
    // extras を先に入れて、予約名 (t/tid/cat/kind/key/seq) を後から上書きする。
    // こうすれば呼び出し側が誤って `("kind", ...)` を extras に入れても事故らない。
    for (k, v) in extras {
        map.insert((*k).to_string(), v.clone());
    }
    map.insert("t".into(), Value::from((t * 1000.0).round() / 1000.0));
    map.insert("tid".into(), Value::from(tid));
    map.insert("cat".into(), Value::from(cat));
    map.insert("kind".into(), Value::from(kind));
    if let Some(k) = key {
        map.insert("key".into(), Value::from(k));
    }
    if seq != 0 {
        map.insert("seq".into(), Value::from(seq));
    }

    let line = match serde_json::to_string(&Value::Object(map)) {
        Ok(s) => s,
        Err(_) => return,
    };

    if let Ok(mut f) = file.lock() {
        let _ = writeln!(f, "{line}");
    }
}

/// `BufWriter` を明示的にフラッシュする。フレーム境界で定期的に呼ぶ。
pub fn flush() {
    if !is_enabled() {
        return;
    }
    if let Some(file) = FILE.get()
        && let Ok(mut f) = file.lock()
    {
        let _ = f.flush();
    }
}

fn thread_id_num() -> u64 {
    let tid = format!("{:?}", std::thread::current().id());
    tid.trim_start_matches("ThreadId(")
        .trim_end_matches(')')
        .parse::<u64>()
        .unwrap_or(0)
}

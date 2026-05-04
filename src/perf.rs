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
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Mutex, OnceLock};
use std::time::Instant;

static ENABLED: AtomicBool = AtomicBool::new(false);
static START: OnceLock<Instant> = OnceLock::new();
static FILE: OnceLock<Mutex<BufWriter<File>>> = OnceLock::new();

/// 直近何世代分の perf log を残すか。`perf_events.jsonl` (現在) +
/// `perf_events.1.jsonl` 〜 `perf_events.{N-1}.jsonl` (過去) で N ファイル。
/// 同じ動画症状を複数回試したいときに、毎回手動で退避する手間を省くため。
const MAX_GENERATIONS: usize = 5;

/// 起動時に 1 回だけ呼ぶ。`enabled=false` なら何もしない。
///
/// `start_override` に `Some(Instant)` を渡すと、それを `t=0` の基準にする。
/// `main()` 冒頭で取得した Instant を渡すことで、perf::init よりも前に実行された
/// 初期化ステップ (data_dir / モデル展開 / Susie 展開 等) の時間も、
/// 後からイベントとして打刻できる。`None` なら `Instant::now()` を基準にする。
pub fn init(enabled: bool, start_override: Option<Instant>) {
    init_with_path(enabled, start_override, None);
}

/// `init` と同じだが、`log_path_override` が指定された場合はそのパスへ直接書く。
/// soak test では 1 動画 1 JSONL に分けるため、固定ログの rotation は行わない。
pub fn init_with_path(
    enabled: bool,
    start_override: Option<Instant>,
    log_path_override: Option<PathBuf>,
) {
    if !enabled {
        return;
    }
    let start = start_override.unwrap_or_else(Instant::now);
    START.set(start).ok();
    let log_path = if let Some(path) = log_path_override {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        path
    } else {
        let log_dir = crate::data_dir::logs_dir();
        let _ = std::fs::create_dir_all(&log_dir);
        rotate_logs(&log_dir);
        log_dir.join("perf_events.jsonl")
    };
    let mut opts = std::fs::OpenOptions::new();
    opts.create(true).write(true).truncate(true);
    allow_live_log_reading(&mut opts);
    match opts.open(&log_path) {
        Ok(f) => {
            if FILE
                .set(Mutex::new(BufWriter::with_capacity(64 * 1024, f)))
                .is_ok()
            {
                ENABLED.store(true, Ordering::Release);
                crate::logger::log(format!("perf: JSONL log enabled at {}", log_path.display()));
            }
        }
        Err(e) => {
            eprintln!(
                "perf ログファイル作成失敗: {e} (path: {})",
                log_path.display()
            );
            crate::logger::log(format!("perf: init failed: {e}"));
        }
    }
}

fn allow_live_log_reading(opts: &mut std::fs::OpenOptions) {
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;

        const FILE_SHARE_READ: u32 = 0x0000_0001;
        const FILE_SHARE_WRITE: u32 = 0x0000_0002;
        const FILE_SHARE_DELETE: u32 = 0x0000_0004;

        opts.share_mode(FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE);
    }
    #[cfg(not(windows))]
    let _ = opts;
}

/// `perf_events.jsonl` をローテーションする。current → `.1` → `.2` → ... と
/// 番号をずらし、最古 (= `MAX_GENERATIONS - 1` 番) は削除する。`init()` から
/// truncate-create する直前に呼ぶ。失敗は静かに無視 (perf-log は debug 用途)。
fn rotate_logs(log_dir: &std::path::Path) {
    let path_for = |n: usize| -> std::path::PathBuf {
        if n == 0 {
            log_dir.join("perf_events.jsonl")
        } else {
            log_dir.join(format!("perf_events.{n}.jsonl"))
        }
    };
    // 最古を消し、上から順番に下にずらす。
    let oldest = path_for(MAX_GENERATIONS - 1);
    let _ = std::fs::remove_file(&oldest);
    for n in (0..MAX_GENERATIONS - 1).rev() {
        let from = path_for(n);
        let to = path_for(n + 1);
        if from.exists() {
            let _ = std::fs::rename(&from, &to);
        }
    }
}

/// ホットパスで先頭に挟むチェック関数。
#[inline]
pub fn is_enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// perf-log 有効時に `init` が基準として設定した `Instant` を返す。
/// 起動時間計測 (`startup.first_frame` 等) で `main()` 入口からの経過を
/// 計算するのに使う。無効時 / 未初期化時は `None`。
pub fn program_start() -> Option<Instant> {
    START.get().copied()
}

/// 共通ヘルパー: `t0.elapsed()` を `ms` として perf イベントを emit する。
/// `is_enabled()` が false なら即 return。`ms` は必ず最初の extra として付く。
/// 追加の `extras` を渡したい場合はこのヘルパーではなく `event()` を直接呼ぶ。
#[inline]
pub fn emit_ms(cat: &str, kind: &str, seq: u64, t0: Instant) {
    if !is_enabled() {
        return;
    }
    let ms = t0.elapsed().as_secs_f64() * 1000.0;
    event(cat, kind, None, seq, &[("ms", serde_json::Value::from(ms))]);
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
    let tid = crate::logger::current_thread_id_num().unwrap_or(0);

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

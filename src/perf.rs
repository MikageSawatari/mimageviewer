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
use std::path::PathBuf;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

pub(crate) mod memory;
pub(crate) mod stall;

static ENABLED: AtomicBool = AtomicBool::new(false);
static START: OnceLock<Instant> = OnceLock::new();

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
    let rotate = log_path_override.is_none();
    let log_path =
        log_path_override.unwrap_or_else(|| crate::data_dir::logs_dir().join("perf_events.jsonl"));
    let t = START.get().unwrap().elapsed().as_secs_f64();
    let mut session = serde_json::json!({"t":(t*1000.0).round()/1000.0,"tid":crate::logger::current_thread_id_num().unwrap_or(0),"cat":"session","kind":"start","pid":std::process::id()});
    if let Some(created) = process_start_unix_ms() {
        session["process_start_unix_ms"] = created.into();
    }
    if let Some(anchor) = wall_clock_anchor(SystemTime::now(), START.get().unwrap().elapsed()) {
        for (k, v) in anchor {
            session[k] = v;
        }
    }
    if crate::log_writer::configure_perf(crate::log_writer::PerfConfig {
        path: log_path.clone(),
        rotate,
        start: *START.get().unwrap(),
        session,
    }) {
        stall::init();
        ENABLED.store(true, Ordering::Release);
        flush();
        memory::emit("perf_enabled", "milestone");
        memory::start_sampler();
        crate::logger::log(format!("perf: JSONL log enabled at {}", log_path.display()));
    }
}

fn wall_clock_anchor(wall: SystemTime, elapsed: Duration) -> Option<[(&'static str, Value); 2]> {
    let unix_ms = u64::try_from(wall.duration_since(UNIX_EPOCH).ok()?.as_millis()).ok()?;
    Some([
        ("wall_unix_ms", unix_ms.into()),
        ("wall_t", elapsed.as_secs_f64().into()),
    ])
}

#[cfg(any(windows, test))]
fn filetime_to_unix_ms(ticks: u64) -> Option<u64> {
    // FILETIME counts 100 ns since 1601-01-01; Unix time starts at 1970-01-01.
    ticks
        .checked_sub(116_444_736_000_000_000)
        .map(|v| v / 10_000)
}

#[cfg(windows)]
pub(crate) fn process_start_unix_ms() -> Option<u64> {
    use windows_sys::Win32::Foundation::FILETIME;
    use windows_sys::Win32::System::Threading::{GetCurrentProcess, GetProcessTimes};

    unsafe {
        let mut creation: FILETIME = std::mem::zeroed();
        let mut exit = std::mem::zeroed();
        let mut kernel = std::mem::zeroed();
        let mut user = std::mem::zeroed();
        if GetProcessTimes(
            GetCurrentProcess(),
            &mut creation,
            &mut exit,
            &mut kernel,
            &mut user,
        ) == 0
        {
            return None;
        }
        filetime_to_unix_ms(
            (u64::from(creation.dwHighDateTime) << 32) | u64::from(creation.dwLowDateTime),
        )
    }
}

#[cfg(not(windows))]
pub(crate) fn process_start_unix_ms() -> Option<u64> {
    None
}

/// `perf_events.jsonl` をローテーションする。current → `.1` → `.2` → ... と
/// 番号をずらし、最古 (= `MAX_GENERATIONS - 1` 番) は削除する。`init()` から
/// truncate-create する直前に呼ぶ。失敗は静かに無視 (perf-log は debug 用途)。
pub(crate) fn rotate_logs(log_dir: &std::path::Path) -> std::io::Result<()> {
    let path_for = |n: usize| -> std::path::PathBuf {
        if n == 0 {
            log_dir.join("perf_events.jsonl")
        } else {
            log_dir.join(format!("perf_events.{n}.jsonl"))
        }
    };
    // 最古を消し、上から順番に下にずらす。
    let oldest = path_for(MAX_GENERATIONS - 1);
    crate::logger::remove_old_generation(&oldest)?;
    for n in (0..MAX_GENERATIONS - 1).rev() {
        let from = path_for(n);
        let to = path_for(n + 1);
        if from.exists() {
            std::fs::rename(&from, &to)?;
        }
    }
    Ok(())
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

#[cfg(test)]
pub(crate) fn install_test_clock(start: Instant) {
    assert!(START.set(start).is_ok());
    stall::init();
    ENABLED.store(true, Ordering::Release);
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
#[track_caller]
pub fn event(cat: &str, kind: &str, key: Option<&str>, seq: u64, extras: &[(&str, Value)]) {
    if !is_enabled() {
        return;
    }
    let Some(start) = START.get() else { return };
    let Some(publication) = crate::log_writer::publication(crate::log_writer::Sink::Perf) else {
        return;
    };

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

    publication.submit(format!("{line}\n"), t, tid, std::panic::Location::caller());
    // One process-memory query per startup marker, after releasing the log lock.
    // Memory events use their own category so they cannot recurse into this hook.
    if cat == "startup" {
        memory::emit(kind, "milestone");
    }
}

/// Nonblocking delivery request. Completion is a separate worker-only API.
#[track_caller]
pub fn flush() {
    if is_enabled() {
        crate::log_writer::request_flush();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_clock_pairs_wall_time_with_nonzero_log_time() {
        let anchor = wall_clock_anchor(
            UNIX_EPOCH + Duration::from_millis(1_700_000_000_123),
            Duration::from_millis(5_250),
        )
        .unwrap();
        assert_eq!(
            anchor[0],
            ("wall_unix_ms", Value::from(1_700_000_000_123_u64))
        );
        assert_eq!(anchor[1], ("wall_t", Value::from(5.25)));
    }

    #[test]
    fn process_creation_filetime_converts_epoch_and_truncates_submilliseconds() {
        let epoch = 116_444_736_000_000_000;
        assert_eq!(filetime_to_unix_ms(epoch), Some(0));
        assert_eq!(filetime_to_unix_ms(epoch + 12_345_678), Some(1234));
        assert_eq!(filetime_to_unix_ms(epoch - 1), None);
    }

    #[test]
    #[cfg(windows)]
    fn process_creation_time_is_available_for_the_current_test_process() {
        let created = process_start_unix_ms().expect("current process creation time");
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_millis();
        assert!(created > 0);
        assert!(u128::from(created) <= now);
    }
}

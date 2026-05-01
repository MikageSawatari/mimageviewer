//! インデクサ進捗レポーター。
//!
//! `search_walker` / `ingest_worker` から "今何してる" を短いメッセージで通知し、
//! `indexer_supervisor` → UI に伝える軽量チャネル。
//!
//! ## なぜ別モジュールか
//!
//! `SupervisorStats` に `current_activity: Option<String>` を載せるだけだと、
//! 下位モジュール (walker / ingest_worker) が `indexer_supervisor::SupervisorStats`
//! を import する必要があり、層構造が逆転してしまう。代わりに共通の軽量型を
//! ここに置き、supervisor が Arc で保持して下位に渡す。
//!
//! ## メッセージ + 構造化カウント
//!
//! - メッセージ: 自由文 (例: "取込 (123/4567) /foo/bar.jpg")
//! - 構造化カウント: `(current, total)` を別フィールドで保持し、直近 10 秒のサンプルから
//!   処理レートを計算して ETA を出す。UI は `snapshot_eta()` で残り時間を取得できる。
//!
//! 書き込み/読み出しは `Mutex` で直列化する。Walker のホットループではスロットルして
//! 使う (毎エントリ lock は過剰)。

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// 直近サンプルの保持期間。これより古いサンプルは ETA 計算で破棄する。
/// 短かすぎると 1 アイテム重い/軽いの差で ETA が大きく振動して読みづらくなる。
/// 60 秒 = 1 分は「ファイル種別の偏り」を平均化しつつ、状況変化への追従も
/// それなりに保てるバランス点。
const ETA_WINDOW: Duration = Duration::from_secs(60);

/// サンプル件数のハード上限。`set_count` が時刻が進まない状況で連打されたとき
/// (テストや単一 Instant の精度限界) の暴走を防ぐ防御。
/// ingest_worker は 1 ファイル処理ごとに 1 サンプル push するので、実測 155 件/秒
/// の環境では 60 秒窓 = 9300 サンプル必要。1024 だとサンプル上限が先に効いて
/// 実効窓が ~6.6 秒に縮み、ETA が振動する原因になる (実害確認済み)。
/// 16384 なら 60 秒 × 273 件/秒まで余裕、16B/サンプル × 16384 ≈ 256 KB。
const ETA_SAMPLES_MAX: usize = 16384;

/// 進捗カウントの ETA スナップショット。
#[derive(Clone, Copy, Debug)]
pub struct EtaSnapshot {
    /// 現在のカウント (例: 取込済み件数)
    pub current: u64,
    /// 全体件数
    pub total: u64,
    /// 直近 ETA_WINDOW のレート (件/秒)。0 なら ETA 未確定。
    pub rate_per_sec: f64,
    /// 残り秒数 (整数切り上げ)。レート 0 なら None。
    pub remaining_secs: Option<u64>,
}

#[derive(Default)]
struct Inner {
    msg: Option<String>,
    count: Option<CountState>,
}

struct CountState {
    current: u64,
    total: u64,
    /// 直近の `(時刻, current)` サンプル。先頭が最古、末尾が最新。
    /// 古いサンプル (> ETA_WINDOW) は push 時に pop_front して破棄する。
    samples: VecDeque<(Instant, u64)>,
}

impl CountState {
    fn new(current: u64, total: u64) -> Self {
        let mut samples = VecDeque::with_capacity(64);
        samples.push_back((Instant::now(), current));
        Self {
            current,
            total,
            samples,
        }
    }

    fn update(&mut self, current: u64, total: u64) {
        self.current = current;
        self.total = total;
        let now = Instant::now();
        self.samples.push_back((now, current));
        while let Some(&(t, _)) = self.samples.front() {
            if now.duration_since(t) > ETA_WINDOW {
                self.samples.pop_front();
            } else {
                break;
            }
        }
        while self.samples.len() > ETA_SAMPLES_MAX {
            self.samples.pop_front();
        }
    }

    fn eta(&self) -> EtaSnapshot {
        let (rate, remaining_secs) = if self.samples.len() >= 2 {
            let (t_old, c_old) = *self.samples.front().unwrap();
            let (t_new, c_new) = *self.samples.back().unwrap();
            let dt = t_new.duration_since(t_old).as_secs_f64();
            let dc = c_new.saturating_sub(c_old) as f64;
            if dt > 0.0 && dc > 0.0 {
                let rate = dc / dt;
                let remaining = self.total.saturating_sub(self.current) as f64;
                let secs = (remaining / rate).ceil();
                let secs = if secs.is_finite() && secs >= 0.0 {
                    Some(secs as u64)
                } else {
                    None
                };
                (rate, secs)
            } else {
                (0.0, None)
            }
        } else {
            (0.0, None)
        };
        EtaSnapshot {
            current: self.current,
            total: self.total,
            rate_per_sec: rate,
            remaining_secs,
        }
    }
}

/// 軽量な "今何してる" レポーター。Clone で同じ共有状態を参照する。
#[derive(Clone, Default)]
pub struct ProgressReporter {
    inner: Arc<Mutex<Inner>>,
}

impl ProgressReporter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 現在の作業メッセージを設定する (以前の値は上書き)。カウント情報は触らない。
    pub fn set<S: Into<String>>(&self, msg: S) {
        if let Ok(mut g) = self.inner.lock() {
            g.msg = Some(msg.into());
        }
    }

    /// 構造化カウント `(current, total)` を更新する。連続呼び出しでサンプルが
    /// 蓄積され、`snapshot_eta()` で残り時間が計算される。`total` が 0 のときは
    /// カウントをクリアする (= 進捗バー終了)。
    pub fn set_count(&self, current: u64, total: u64) {
        if let Ok(mut g) = self.inner.lock() {
            apply_count(&mut g, current, total);
        }
    }

    /// メッセージとカウントを 1 lock で同時更新するホットパス向け版。
    /// ingest ループのように毎件呼ばれる場所で `set` + `set_count` を別々に呼ぶと
    /// Mutex を 2 回取るので、ここで 1 回にまとめる。
    pub fn set_msg_and_count<S: Into<String>>(&self, msg: S, current: u64, total: u64) {
        if let Ok(mut g) = self.inner.lock() {
            g.msg = Some(msg.into());
            apply_count(&mut g, current, total);
        }
    }

    /// 進捗カウントだけクリア (フェーズ完了時)。メッセージは触らない。
    /// `set_count(0, 0)` の名前付きバージョン。
    pub fn clear_count(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.count = None;
        }
    }

    /// 作業完了を示し、メッセージとカウントをクリアする。
    pub fn clear(&self) {
        if let Ok(mut g) = self.inner.lock() {
            g.msg = None;
            g.count = None;
        }
    }

    /// メッセージのみ取得 (カウントには触れない)。
    pub fn snapshot(&self) -> Option<String> {
        self.inner.lock().ok().and_then(|g| g.msg.clone())
    }

    /// カウント情報からの ETA スナップショット。カウント未設定なら None。
    pub fn snapshot_eta(&self) -> Option<EtaSnapshot> {
        self.inner
            .lock()
            .ok()
            .and_then(|g| g.count.as_ref().map(|c| c.eta()))
    }
}

fn apply_count(g: &mut Inner, current: u64, total: u64) {
    if total == 0 {
        g.count = None;
        return;
    }
    // phase 切替検出: total 変化 OR current の逆行 (= 新フェーズが小さい current で
    // 始まる) の両方で sample buffer をリセット。delete (1..N) → ingest (1..N) で
    // 同じ N でも、current が N → 1 に逆行するので新フェーズと判別できる (Codex P3)。
    let need_reset = match g.count.as_ref() {
        Some(c) => c.total != total || current < c.current,
        None => true,
    };
    if need_reset {
        g.count = Some(CountState::new(current, total));
    } else if let Some(c) = g.count.as_mut() {
        c.update(current, total);
    }
}

/// `EtaSnapshot::remaining_secs` を `hh:mm:ss` (1h 以上) または `mm:ss` (1h 未満)
/// にフォーマットする。
pub fn format_eta_hms(secs: u64) -> String {
    let h = secs / 3600;
    let m = (secs % 3600) / 60;
    let s = secs % 60;
    if h > 0 {
        format!("{h:02}:{m:02}:{s:02}")
    } else {
        format!("{m:02}:{s:02}")
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::thread::sleep;

    #[test]
    fn set_and_snapshot_roundtrip() {
        let r = ProgressReporter::new();
        assert_eq!(r.snapshot(), None);
        r.set("hello");
        assert_eq!(r.snapshot().as_deref(), Some("hello"));
        r.set("world");
        assert_eq!(r.snapshot().as_deref(), Some("world"));
        r.clear();
        assert_eq!(r.snapshot(), None);
    }

    #[test]
    fn clone_shares_state() {
        let a = ProgressReporter::new();
        let b = a.clone();
        a.set("x");
        assert_eq!(b.snapshot().as_deref(), Some("x"));
        b.set("y");
        assert_eq!(a.snapshot().as_deref(), Some("y"));
    }

    #[test]
    fn count_eta_progresses_with_samples() {
        let r = ProgressReporter::new();
        assert!(r.snapshot_eta().is_none());
        r.set_count(0, 100);
        // 1 サンプルだけだと rate 計算できない
        let e = r.snapshot_eta().unwrap();
        assert_eq!(e.current, 0);
        assert_eq!(e.total, 100);
        assert_eq!(e.remaining_secs, None);
        // 50ms で 10 件進めたサンプルを足す
        sleep(Duration::from_millis(50));
        r.set_count(10, 100);
        let e = r.snapshot_eta().unwrap();
        assert_eq!(e.current, 10);
        assert!(e.rate_per_sec > 0.0);
        assert!(e.remaining_secs.is_some());
    }

    #[test]
    fn count_total_zero_clears() {
        let r = ProgressReporter::new();
        r.set_count(5, 100);
        assert!(r.snapshot_eta().is_some());
        r.set_count(0, 0);
        assert!(r.snapshot_eta().is_none());
    }

    #[test]
    fn count_current_decrease_resets_samples() {
        // delete phase で N 件ぶん溜めて、ingest phase が同じ total で current=1 に
        // 戻ると新 phase 扱いで sample buffer がリセットされる。
        let r = ProgressReporter::new();
        r.set_count(10, 100);
        sleep(Duration::from_millis(20));
        r.set_count(50, 100);
        // ここまでで 2 サンプル溜まり ETA 算出可
        assert!(r.snapshot_eta().unwrap().remaining_secs.is_some());
        // ingest 開始 (同じ total, current 逆行) → reset
        r.set_count(1, 100);
        let eta = r.snapshot_eta().unwrap();
        assert_eq!(eta.current, 1);
        assert_eq!(eta.remaining_secs, None);
    }

    #[test]
    fn count_total_change_resets_samples() {
        let r = ProgressReporter::new();
        r.set_count(0, 100);
        sleep(Duration::from_millis(20));
        r.set_count(10, 100);
        assert!(r.snapshot_eta().unwrap().remaining_secs.is_some());
        // total が変わると新規 phase 扱いでサンプル破棄
        r.set_count(0, 200);
        assert_eq!(r.snapshot_eta().unwrap().remaining_secs, None);
    }

    #[test]
    fn format_eta_hms_formats() {
        assert_eq!(format_eta_hms(0), "00:00");
        assert_eq!(format_eta_hms(59), "00:59");
        assert_eq!(format_eta_hms(60), "01:00");
        assert_eq!(format_eta_hms(3599), "59:59");
        assert_eq!(format_eta_hms(3600), "01:00:00");
        assert_eq!(format_eta_hms(7325), "02:02:05");
    }
}

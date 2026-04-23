//! `ActivityGate` — ユーザー操作中はバックグラウンドインデクサを一時停止する。
//!
//! ## 背景 (2026-04)
//!
//! Ctrl+↑↓ でフォルダを連続移動するとサムネ decode が HDD に集中し、同じ HDD で
//! メタインデクサ (XMP 読み込み) / 名前インデクサ (read_dir) が動いているとディスク
//! シークが衝突して decode が 10 秒スケールまで膨らむ問題があった。
//! `docs/ui-responsiveness.md` の指針に沿って、**操作している間は indexer を止める**
//! のが最も単純かつ効果的な解。
//!
//! ## 契約
//!
//! - `bump()`: UI スレッドが入力 (キー / クリック / スクロール) 毎に呼ぶ。
//! - `wait_until_idle(cancel)`: ワーカーが各 unit of work (ファイル 1 本 / フォルダ 1 つ)
//!   の前に呼ぶ。最後の `bump()` から `quiet_threshold_ms` 経過するまでブロック。
//! - `cancel` が立つと即 return する (キャンセル時にここで詰まらないため)。
//!
//! ## 実装
//!
//! 単一の `AtomicU64` に「最終操作時刻 (ms, プロセス起動からの経過)」を格納。
//! 読み出しは Relaxed で OK — 多少古い値が見えても、次の wait ループで訂正される
//! (単調増加なので「寝すぎる」ことはあっても「起きなすぎる」ことはない)。
//!
//! `SystemTime` ではなく `Instant` ベースの単調時刻を使う (DST / 時刻合わせの影響回避)。

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::OnceLock;
use std::time::{Duration, Instant};

/// デフォルトの「最終操作からこの ms 以上無操作なら indexer が再開してよい」閾値。
/// HDD で Ctrl+↑↓ 連打の体感と indexer 進捗のバランスをとった実測値。
pub const DEFAULT_QUIET_MS: u64 = 1000;

/// プロセス開始時刻。`now_ms()` の基準点として lazily 初期化する。
static EPOCH: OnceLock<Instant> = OnceLock::new();

fn epoch() -> Instant {
    *EPOCH.get_or_init(Instant::now)
}

fn now_ms() -> u64 {
    epoch().elapsed().as_millis() as u64
}

/// 「操作中は indexer を止める」ためのゲート。`Arc` で共有する。
///
/// v0.9 で `paused` フラグを追加。トレイ常駐時にユーザーが明示的にインデックスを
/// 一時停止したい場合、`set_paused(true)` を呼ぶと `wait_until_idle` が paused=false
/// になるまでブロックする。`cancel` は paused を貫通して即 return させる (アプリ終了時に
/// 常駐スレッドが固まらないため)。
pub struct ActivityGate {
    last_activity_ms: AtomicU64,
    quiet_threshold_ms: u64,
    /// トレイ常駐 + ユーザーによる明示的な「インデックス一時停止」で true。
    /// `wait_until_idle` は paused の間ループ内で sleep し、cancel か paused 解除で抜ける。
    paused: AtomicBool,
}

impl ActivityGate {
    /// `quiet_threshold_ms` ミリ秒操作がないときだけ通過するゲートを作る。
    ///
    /// 起動直後は「最後の操作が遠い過去」として扱う (= ワーカーは起動直後から動ける)。
    /// `u64::MAX` をセンチネルとして「まだ一度も bump されていない」を表し、
    /// `wait_until_idle` はこの状態を即 return で抜ける。`bump()` 後は `now_ms()` を
    /// 通常どおり格納するので、センチネルと衝突することはない。
    pub fn new(quiet_threshold_ms: u64) -> Self {
        let _ = epoch(); // 起動時に fix しておく
        Self {
            last_activity_ms: AtomicU64::new(u64::MAX),
            quiet_threshold_ms,
            paused: AtomicBool::new(false),
        }
    }

    /// UI スレッドから入力イベントで呼ぶ。
    /// 呼び出しコストは atomic store 1 回。毎フレーム条件付きで呼んで問題なし。
    pub fn bump(&self) {
        self.last_activity_ms.store(now_ms(), Ordering::Relaxed);
    }

    /// トレイ常駐時の「インデックス一時停止」スイッチ (v0.9)。
    /// true の間は `wait_until_idle` が cancel / 解除 までブロックする。
    ///
    /// 注意: paused は cancel を貫通させない (cancel の方が優先)。アプリ終了時に
    /// supervisor スレッドがここで固まらないよう、`wait_until_idle` は paused より
    /// 先に cancel をチェックする。
    pub fn set_paused(&self, paused: bool) {
        self.paused.store(paused, Ordering::Relaxed);
    }

    /// 現在 paused 状態か (テスト / UI 表示用)。
    pub fn is_paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// `quiet_threshold_ms` が経過するまでブロックする。
    /// cancel が立ったら即 return。paused の間はループブロック (cancel か解除で抜ける)。
    ///
    /// 寝る時間は「残り必要時間」を計算してから `min(remain, 200)` ms ずつ。
    /// 200ms でキャップするのは、cancel が立ったときに最大 200ms で抜けるため。
    pub fn wait_until_idle(&self, cancel: &AtomicBool) {
        loop {
            if cancel.load(Ordering::Relaxed) {
                return;
            }
            // paused は cancel の次にチェック。paused 中は short-sleep でループ。
            // 解除された瞬間に通常の idle 判定に落ちるので、最大 200ms の反応遅延で
            // 再開する。ユーザー体感としては即時。
            if self.paused.load(Ordering::Relaxed) {
                std::thread::sleep(Duration::from_millis(200));
                continue;
            }
            let last = self.last_activity_ms.load(Ordering::Relaxed);
            if last == u64::MAX {
                return; // 未 bump (起動から何も操作がない) → idle 扱い
            }
            let now = now_ms();
            let elapsed = now.saturating_sub(last);
            if elapsed >= self.quiet_threshold_ms {
                return;
            }
            let remain = self.quiet_threshold_ms - elapsed;
            std::thread::sleep(Duration::from_millis(remain.min(200)));
        }
    }

    /// 現在 idle (= `quiet_threshold_ms` 以上操作なし) かどうか。テスト / 計装用。
    /// paused 中は idle でも false を返す (= 止まっている状態を idle とは呼ばない)。
    pub fn is_idle(&self) -> bool {
        if self.paused.load(Ordering::Relaxed) {
            return false;
        }
        let last = self.last_activity_ms.load(Ordering::Relaxed);
        if last == u64::MAX {
            return true;
        }
        now_ms().saturating_sub(last) >= self.quiet_threshold_ms
    }
}

/// ワーカーの unit-of-work 境界で呼ぶ定型ヘルパ。
///
/// `gate` が `Some` なら `wait_until_idle(cancel)` で待機したあと、`cancel` フラグを
/// 再確認する。戻り値が `true` のとき呼び出し側は「キャンセルされた」として
/// ループを抜ける。gate が None なら `cancel` だけを見る。
///
/// 複数のワーカー (ingest / search_walker / name_bulk_indexer) で同じ idiom を
/// 書き分けていたのを統合する。
pub fn wait_and_check_cancel(gate: Option<&ActivityGate>, cancel: &AtomicBool) -> bool {
    if let Some(g) = gate {
        g.wait_until_idle(cancel);
    }
    cancel.load(Ordering::Relaxed)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use std::thread;
    use std::time::Instant;

    #[test]
    fn wait_returns_immediately_when_idle() {
        let gate = ActivityGate::new(100);
        // bump せず放置 → 最初から idle
        let cancel = AtomicBool::new(false);
        let t0 = Instant::now();
        gate.wait_until_idle(&cancel);
        assert!(t0.elapsed().as_millis() < 50, "idle 時は即 return");
    }

    #[test]
    fn wait_blocks_until_quiet_threshold() {
        let gate = ActivityGate::new(150);
        gate.bump();
        let cancel = AtomicBool::new(false);
        let t0 = Instant::now();
        gate.wait_until_idle(&cancel);
        let elapsed = t0.elapsed().as_millis();
        assert!(
            (100..500).contains(&(elapsed as u64)),
            "threshold (150ms) 付近で起きる: got {elapsed}ms"
        );
    }

    #[test]
    fn bump_extends_wait() {
        let gate = Arc::new(ActivityGate::new(200));
        gate.bump();
        let cancel = Arc::new(AtomicBool::new(false));
        let g = Arc::clone(&gate);
        let c = Arc::clone(&cancel);
        let h = thread::spawn(move || g.wait_until_idle(&c));
        // 100ms 後に再 bump → wait がさらに 200ms 延びる
        thread::sleep(Duration::from_millis(100));
        gate.bump();
        let t0 = Instant::now();
        h.join().unwrap();
        let after_rebump = t0.elapsed().as_millis();
        assert!(
            after_rebump >= 150,
            "再 bump で延長されるはず: got {after_rebump}ms"
        );
    }

    #[test]
    fn cancel_wakes_wait() {
        let gate = Arc::new(ActivityGate::new(10_000)); // 10 秒 — 通常は寝きらない
        gate.bump();
        let cancel = Arc::new(AtomicBool::new(false));
        let g = Arc::clone(&gate);
        let c = Arc::clone(&cancel);
        let h = thread::spawn(move || g.wait_until_idle(&c));
        thread::sleep(Duration::from_millis(50));
        cancel.store(true, Ordering::Relaxed);
        let t0 = Instant::now();
        h.join().unwrap();
        assert!(
            t0.elapsed().as_millis() < 500,
            "cancel 後は 500ms 以内に抜ける"
        );
    }

    #[test]
    fn paused_blocks_wait_until_released() {
        // paused=true の間は idle でも wait がブロックされ、解除で抜ける。
        let gate = Arc::new(ActivityGate::new(50));
        gate.set_paused(true);
        let cancel = Arc::new(AtomicBool::new(false));
        let g = Arc::clone(&gate);
        let c = Arc::clone(&cancel);
        let h = thread::spawn(move || g.wait_until_idle(&c));
        // 200ms 待って pause 解除 → wait が抜けてくる
        thread::sleep(Duration::from_millis(200));
        gate.set_paused(false);
        let t0 = Instant::now();
        h.join().unwrap();
        assert!(
            t0.elapsed().as_millis() < 500,
            "pause 解除後 500ms 以内に抜ける"
        );
    }

    #[test]
    fn paused_cancel_wakes_wait() {
        // paused=true のまま cancel を立てると即 return する (アプリ終了時の固まり防止)。
        let gate = Arc::new(ActivityGate::new(100));
        gate.set_paused(true);
        let cancel = Arc::new(AtomicBool::new(false));
        let g = Arc::clone(&gate);
        let c = Arc::clone(&cancel);
        let h = thread::spawn(move || g.wait_until_idle(&c));
        thread::sleep(Duration::from_millis(50));
        cancel.store(true, Ordering::Relaxed);
        let t0 = Instant::now();
        h.join().unwrap();
        assert!(
            t0.elapsed().as_millis() < 500,
            "paused 中でも cancel で 500ms 以内に抜ける"
        );
    }

    #[test]
    fn is_paused_reflects_setter() {
        let gate = ActivityGate::new(100);
        assert!(!gate.is_paused());
        gate.set_paused(true);
        assert!(gate.is_paused());
        gate.set_paused(false);
        assert!(!gate.is_paused());
    }

    #[test]
    fn paused_is_not_idle() {
        let gate = ActivityGate::new(100);
        assert!(gate.is_idle(), "未 bump 状態は idle");
        gate.set_paused(true);
        assert!(
            !gate.is_idle(),
            "paused 中は idle=false (停止状態を idle と呼ばない)"
        );
    }
}

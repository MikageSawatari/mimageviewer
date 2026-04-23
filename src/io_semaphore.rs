//! `GlobalIoSemaphore` — ワーカー横断の I/O 同時実行制御。
//!
//! docs/search-expansion-design.md §7.5 + §15.1.6 に準拠。
//!
//! 目的: UI スレッドのスクロール・入力応答がバックグラウンド I/O 競合で阻害されないよう、
//! ディスク同時アクセス数に上限を設ける。PDF ワーカー / サムネイルワーカー / 全文インデクサ
//! のような複数サブシステムが同じ I/O リソースを奪い合うのを調停する。
//!
//! ## 優先度
//!
//! - `High`: ユーザが今見ているフォルダ / ページ (UI 経路)
//! - `Normal`: PDF 背景レンダリング、通常サムネロード
//! - `Low`: インデクサ (Ctrl+G 用の全文メタスキャン)
//!
//! 高優先度の待ち行列が空になるまで、低優先度は新規取得できない (飢餓 vs. 公平性の妥協点)。
//! 既に permit を握っている worker は優先度に関係なく継続できる。
//!
//! ## 飢餓ポリシー (明示)
//!
//! High が連続して投入される状況では Low は無制限に待たされる可能性がある。
//! これは **UI 応答性最優先** という本アプリの方針に基づく意図的な選択:
//! - High = UI スレッドが今見ているフォルダの I/O 要求 (サムネ・メタ読み)
//! - Low  = バックグラウンドインデクサ (ユーザの目の前の操作ではない)
//!
//! ユーザがアクティブに操作している間は Low は事実上止まるが、
//! アイドル時間 (入力なし数秒) には High キューが空き、Low が進む。
//! もし Low の進捗がほしい場面で High が連続しすぎるケースが発生したら、
//! 「AC 電源時のみインデックス」等の別機構で制御する (§7.6)。
//!
//! ## 実装方針
//!
//! `Mutex + Condvar` パターン。`try_lock + sleep` は禁止 (CLAUDE.md §並行処理 参照)。
//! permit の drop で自動的に release し、`notify_all` で起床させる。
//! Condvar は spurious wakeup 耐性のため `while` ループで条件を再確認する。

use std::sync::{Arc, Condvar, Mutex};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum IoPriority {
    Low = 0,
    Normal = 1,
    High = 2,
}

struct SemState {
    available: usize,
    total: usize,
    /// 各優先度の待機数 (対応する `IoPriority` をキーに、0..=2)
    waiting: [usize; 3],
    /// v0.9 トレイ常駐時の throttle モード。true の間は実効上限 1 permit として振る舞う
    /// (既存 holder は drop まで維持、新規取得は in_use == 0 のときだけ通す)。
    /// 解除時に `notify_all` で待機者を起こす。
    throttled: bool,
}

impl SemState {
    /// 指定優先度が今 permit を取得してよいか?
    /// - available > 0
    /// - かつ自分より高い優先度の待機者がいない (または自分が最高優先度)
    /// - かつ throttled=true の場合は in_use == 0 (= available == total) である
    fn can_acquire(&self, pri: IoPriority) -> bool {
        if self.available == 0 {
            return false;
        }
        // throttle モード: 実効上限 1 permit。in_use >= 1 なら新規取得不可。
        if self.throttled && self.available < self.total {
            return false;
        }
        // 自分より高い優先度の waiter がいるなら、その人に譲る
        for higher in (pri as usize + 1)..3 {
            if self.waiting[higher] > 0 {
                return false;
            }
        }
        true
    }
}

pub struct GlobalIoSemaphore {
    inner: Arc<(Mutex<SemState>, Condvar)>,
}

impl GlobalIoSemaphore {
    /// 最大 `total` 個の I/O 同時実行を許可するセマフォを作る。
    pub fn new(total: usize) -> Self {
        assert!(total >= 1, "total permits must be >= 1");
        Self {
            inner: Arc::new((
                Mutex::new(SemState {
                    available: total,
                    total,
                    waiting: [0, 0, 0],
                    throttled: false,
                }),
                Condvar::new(),
            )),
        }
    }

    /// v0.9 トレイ常駐時の throttle 切替。true にすると実効上限 1 permit として動作し、
    /// 複数 worker が同時に I/O を掴むのを抑える。false で解除し、待機者を全員起こす。
    ///
    /// 効用: トレイ常駐中に mIV が他アプリ (ゲーム / 動画再生 / エディタ) の I/O 帯域を
    /// 奪わないようにする。indexer は permit を 1 本ずつしか掴めなくなるので、
    /// 見かけ上スループットは設定 `indexer_speed_profile` によらず 1 permit 相当に落ちる。
    ///
    /// 既存の permit holder は revoke しない (drop まで維持)。throttle 発効直後に
    /// `available == total` になるまで最大 1 permit 分の処理が走る点は許容する
    /// (通常は数百 ms 単位)。
    pub fn set_throttled(&self, throttled: bool) {
        let (mu, cv) = &*self.inner;
        let mut st = mu.lock().unwrap();
        if st.throttled == throttled {
            return;
        }
        st.throttled = throttled;
        // 解除時: 全 waiter を起こして再判定させる。
        // 有効化時: 待機者が blocked になるだけなので通知は不要 (新規 wait が can_acquire で弾かれる)。
        if !throttled {
            cv.notify_all();
        }
    }

    /// 現在 throttled 状態か (計装 / UI 表示用)。
    pub fn is_throttled(&self) -> bool {
        let (mu, _cv) = &*self.inner;
        mu.lock().unwrap().throttled
    }

    /// Blocking acquire。permit が取れるまで待ち、`IoPermit` を返す (Drop で自動 release)。
    pub fn acquire(&self, priority: IoPriority) -> IoPermit {
        let (mu, cv) = &*self.inner;
        let mut st = mu.lock().unwrap();
        st.waiting[priority as usize] += 1;
        // spurious wakeup 耐性のため while ループで再確認
        while !st.can_acquire(priority) {
            st = cv.wait(st).unwrap();
        }
        st.waiting[priority as usize] -= 1;
        st.available -= 1;
        IoPermit {
            sem: Arc::clone(&self.inner),
        }
    }

    /// Non-blocking acquire。permit が取れないなら即 None を返す。
    /// キャンセル済み・best-effort タスク向け (CLAUDE.md でも try_lock 自体は OK)。
    pub fn try_acquire(&self, priority: IoPriority) -> Option<IoPermit> {
        let (mu, _cv) = &*self.inner;
        let mut st = mu.lock().unwrap();
        if !st.can_acquire(priority) {
            return None;
        }
        st.available -= 1;
        Some(IoPermit {
            sem: Arc::clone(&self.inner),
        })
    }

    /// 現在の available / total を返す (メトリクス用)。
    pub fn stats(&self) -> (usize, usize) {
        let (mu, _cv) = &*self.inner;
        let st = mu.lock().unwrap();
        (st.available, st.total)
    }
}

/// RAII permit。Drop で available を元に戻し、Condvar で待機者を起床させる。
pub struct IoPermit {
    sem: Arc<(Mutex<SemState>, Condvar)>,
}

impl Drop for IoPermit {
    fn drop(&mut self) {
        let (mu, cv) = &*self.sem;
        let mut st = mu.lock().unwrap();
        st.available += 1;
        debug_assert!(
            st.available <= st.total,
            "IoPermit: available exceeded total (double-release?)"
        );
        // 待機者に通知。優先度別に最適化した notify にしたいが、Condvar はキー別 wait を
        // 持たないので notify_all で全員起床させ、`can_acquire` で二次判定させる。
        // 待機数が少ない前提なので thundering herd は問題にならない。
        cv.notify_all();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::time::Duration;

    #[test]
    fn acquire_decrements_available() {
        let sem = GlobalIoSemaphore::new(3);
        let (a, t) = sem.stats();
        assert_eq!((a, t), (3, 3));
        let _p = sem.acquire(IoPriority::Normal);
        let (a, _) = sem.stats();
        assert_eq!(a, 2);
    }

    #[test]
    fn drop_releases_permit() {
        let sem = GlobalIoSemaphore::new(1);
        {
            let _p = sem.acquire(IoPriority::Normal);
            assert_eq!(sem.stats().0, 0);
        }
        assert_eq!(sem.stats().0, 1);
    }

    #[test]
    fn try_acquire_fails_when_empty() {
        let sem = GlobalIoSemaphore::new(1);
        let _p = sem.acquire(IoPriority::Normal);
        assert!(sem.try_acquire(IoPriority::Normal).is_none());
    }

    #[test]
    fn try_acquire_low_blocked_when_high_waiting() {
        // permit=1、high 待機者 1 人 → low は try_acquire で取れない
        let sem = Arc::new(GlobalIoSemaphore::new(1));
        let _holder = sem.acquire(IoPriority::Normal); // 先に 1 つ掴む

        // high 優先で acquire する別スレッドを起動
        let sem_h = Arc::clone(&sem);
        let handle = std::thread::spawn(move || {
            let _p = sem_h.acquire(IoPriority::High);
        });
        // high が実際に waiting 状態に入るまで spin-wait (CI 耐性、時間依存を排除)
        let start = std::time::Instant::now();
        loop {
            {
                let (mu, _) = &*sem.inner;
                if mu.lock().unwrap().waiting[IoPriority::High as usize] >= 1 {
                    break;
                }
            }
            if start.elapsed() >= Duration::from_secs(2) {
                panic!("high waiter did not reach waiting state");
            }
            std::thread::yield_now();
        }

        // Low 優先で try_acquire: high 待機者がいるので取れない
        assert!(sem.try_acquire(IoPriority::Low).is_none());

        // holder を解放
        drop(_holder);
        handle.join().unwrap();
    }

    #[test]
    fn high_priority_acquires_before_low_waiting_longer() {
        // permit=1、Low が先に 1 つ掴む → Low がさらに 1 人待機 + High が 1 人待機 →
        // holder を release したとき High が先に取得すること。
        //
        // 並列テスト実行時のスケジュール揺れに耐えるため、スレッドが本当に wait 状態に
        // 入ったことを `waiting` カウンタで spin-wait で確認する (sleep だけだと CI で flaky)。
        fn wait_until<F: Fn() -> bool>(cond: F, deadline: Duration) {
            let start = std::time::Instant::now();
            while !cond() {
                if start.elapsed() >= deadline {
                    panic!("wait_until timeout ({deadline:?})");
                }
                std::thread::yield_now();
            }
        }

        let sem = Arc::new(GlobalIoSemaphore::new(1));
        let holder = sem.acquire(IoPriority::Normal);

        let order = Arc::new(Mutex::new(Vec::<&'static str>::new()));

        let sem_low = Arc::clone(&sem);
        let order_low = Arc::clone(&order);
        let low_handle = std::thread::spawn(move || {
            let _p = sem_low.acquire(IoPriority::Low);
            order_low.lock().unwrap().push("low");
        });

        // Low が本当に wait 状態に入るまで spin
        let sem_ref = Arc::clone(&sem);
        wait_until(
            || {
                let (mu, _) = &*sem_ref.inner;
                mu.lock().unwrap().waiting[IoPriority::Low as usize] == 1
            },
            Duration::from_secs(2),
        );

        let sem_high = Arc::clone(&sem);
        let order_high = Arc::clone(&order);
        let high_handle = std::thread::spawn(move || {
            let _p = sem_high.acquire(IoPriority::High);
            order_high.lock().unwrap().push("high");
        });

        // High も wait 状態に入るまで spin
        let sem_ref = Arc::clone(&sem);
        wait_until(
            || {
                let (mu, _) = &*sem_ref.inner;
                mu.lock().unwrap().waiting[IoPriority::High as usize] == 1
            },
            Duration::from_secs(2),
        );
        drop(holder); // release

        high_handle.join().unwrap();
        low_handle.join().unwrap();

        let ord = order.lock().unwrap();
        assert_eq!(
            ord.as_slice(),
            &["high", "low"],
            "High が Low より先に取得するはず"
        );
    }

    #[test]
    fn throttled_limits_concurrency_to_one() {
        // 全体 permit=4 でも throttled=true なら実効 1。
        let sem = GlobalIoSemaphore::new(4);
        sem.set_throttled(true);
        let p1 = sem.acquire(IoPriority::Normal);
        // 1 本は取れる (in_use=1)。だが 2 本目は取れない。
        assert!(sem.try_acquire(IoPriority::Normal).is_none());
        drop(p1);
        // release で in_use=0 に戻るので 1 本目は取れる。
        let _p2 = sem.acquire(IoPriority::Normal);
    }

    #[test]
    fn unthrottle_wakes_waiters() {
        // throttled 中にブロックされた acquire が、解除で取得できること。
        let sem = Arc::new(GlobalIoSemaphore::new(4));
        sem.set_throttled(true);
        let holder = sem.acquire(IoPriority::Normal); // in_use=1

        let sem2 = Arc::clone(&sem);
        let handle = std::thread::spawn(move || {
            // throttled 中なので取れない → ブロック
            let _p = sem2.acquire(IoPriority::Normal);
        });

        // waiter が wait 状態に入るまで spin
        let start = std::time::Instant::now();
        loop {
            {
                let (mu, _) = &*sem.inner;
                if mu.lock().unwrap().waiting[IoPriority::Normal as usize] >= 1 {
                    break;
                }
            }
            if start.elapsed() >= Duration::from_secs(2) {
                panic!("waiter did not enter wait state");
            }
            std::thread::yield_now();
        }

        drop(holder);
        // holder は drop して notify したが throttled はまだ有効なので
        // waiter は取れない (in_use=0 なら取れる)。実際は holder drop で
        // in_use=0 になったので、throttled でも in_use < 1 で acquire 可。
        // → join が成功すること
        handle.join().unwrap();
    }

    #[test]
    fn is_throttled_reflects_setter() {
        let sem = GlobalIoSemaphore::new(2);
        assert!(!sem.is_throttled());
        sem.set_throttled(true);
        assert!(sem.is_throttled());
        sem.set_throttled(false);
        assert!(!sem.is_throttled());
    }

    #[test]
    fn waiters_eventually_all_acquire() {
        let sem = Arc::new(GlobalIoSemaphore::new(2));
        let done = Arc::new(AtomicUsize::new(0));
        let mut handles = Vec::new();
        for i in 0..8 {
            let sem = Arc::clone(&sem);
            let done = Arc::clone(&done);
            let pri = if i % 3 == 0 {
                IoPriority::High
            } else if i % 3 == 1 {
                IoPriority::Normal
            } else {
                IoPriority::Low
            };
            handles.push(std::thread::spawn(move || {
                let _p = sem.acquire(pri);
                std::thread::sleep(Duration::from_millis(5));
                done.fetch_add(1, Ordering::SeqCst);
            }));
        }
        for h in handles {
            h.join().unwrap();
        }
        assert_eq!(done.load(Ordering::SeqCst), 8);
        // 最終的に available が 2 に戻っていること
        assert_eq!(sem.stats().0, 2);
    }
}

// ── prefetch suppression during scroll (docs/prefetch-suppression-during-scroll-plan.md) ──
//
// スクロール中 / visible 待ち中はサムネ prefetch (= 非可視範囲) の enqueue を抑制し、
// PDFium pool に prefetch が流れて in-flight を埋めてしまうのを防ぐ。
//
// 純関数として切り出して unit test 可能にする。実 caller は
// `App::prefetch_allowed_now` (App field を捌いて引数を作る薄い wrapper)。

/// スクロール停止 (= 最後の scroll input から) 経過時間がこの閾値以上なら "idle" 扱い。
pub(crate) const PREFETCH_IDLE_THRESHOLD: std::time::Duration =
    std::time::Duration::from_millis(100);

/// visible が永久 Pending のまま prefetch が永久停止しないよう、絶対 timeout。
/// この時間 scroll なしが経過したら visible_pending によらず prefetch を allow する。
pub(crate) const PREFETCH_BACKSTOP: std::time::Duration = std::time::Duration::from_secs(3);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AllowReason {
    /// `last_prefetch_scroll_at == None` (= 起動直後 / フォルダ切替時の sentinel `Some(now)` でなく未設定)。
    NoScrollYet,
    /// scroll idle 100ms 経過 + visible 全部 ready。
    ScrollIdleAndVisibleReady,
    /// 3 秒 backstop 発動 (= visible が永久 Pending でも prefetch 再開)。
    Backstop3s,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum BlockReason {
    /// 最後の scroll input から `PREFETCH_IDLE_THRESHOLD` 未満。
    ScrollNotIdle { elapsed_ms: u64 },
    /// scroll idle だが visible 範囲のサムネがまだ Loaded/Failed でない。
    VisibleStillLoading { pending: usize },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PrefetchDecision {
    Allow { reason: AllowReason },
    Block { reason: BlockReason },
}

/// prefetch (= 非可視範囲) を enqueue してよいか判定。
///
/// 順序:
/// 1. `last_prefetch_scroll_at` が `PREFETCH_BACKSTOP` 以上前 → 無条件 Allow (Backstop3s)
/// 2. `last_prefetch_scroll_at` から `PREFETCH_IDLE_THRESHOLD` 未満 → Block (ScrollNotIdle)
/// 3. `visible_state_pending > 0` → Block (VisibleStillLoading)
/// 4. それ以外 → Allow (NoScrollYet or ScrollIdleAndVisibleReady)
///
/// `last_prefetch_scroll_at = None` は「起動直後 / 一度もスクロールしてない」状態。
/// `emit_scroll_settle_event` で `last_scroll_event_at` は clear されるが、
/// 本関数が見る `last_prefetch_scroll_at` は **clear されない** (= backstop 計時起点が安定)。
pub(crate) fn decide_prefetch_allowed(
    now: std::time::Instant,
    last_prefetch_scroll_at: Option<std::time::Instant>,
    visible_state_pending: usize,
) -> PrefetchDecision {
    if let Some(t) = last_prefetch_scroll_at {
        let elapsed = now.saturating_duration_since(t);
        // (1) backstop: 3 秒経ったら無条件 allow
        if elapsed >= PREFETCH_BACKSTOP {
            return PrefetchDecision::Allow {
                reason: AllowReason::Backstop3s,
            };
        }
        // (2) scroll idle 不足
        if elapsed < PREFETCH_IDLE_THRESHOLD {
            return PrefetchDecision::Block {
                reason: BlockReason::ScrollNotIdle {
                    elapsed_ms: elapsed.as_millis() as u64,
                },
            };
        }
    }
    // (3) visible ready
    if visible_state_pending > 0 {
        return PrefetchDecision::Block {
            reason: BlockReason::VisibleStillLoading {
                pending: visible_state_pending,
            },
        };
    }
    let reason = if last_prefetch_scroll_at.is_none() {
        AllowReason::NoScrollYet
    } else {
        AllowReason::ScrollIdleAndVisibleReady
    };
    PrefetchDecision::Allow { reason }
}

/// 先読み対象を距離順・forward 先で交互配置: +1, -1, +2, -2, +3, -3, …
/// 同距離の組では forward (次ページ方向) が先。片側が尽きたら反対側だけ続く。
/// fs_cache / AI アップスケール / サムネイルグリッド の全先読みで方針統一。
pub(crate) fn interleaved_prefetch_targets(
    image_indices: &[usize],
    pos: usize,
    n: usize,
    pf_forward: usize,
    pf_back: usize,
) -> Vec<usize> {
    let max_d = pf_forward.max(pf_back);
    let mut out = Vec::with_capacity(pf_forward + pf_back);
    for d in 1..=max_d {
        if d <= pf_forward {
            if let Some(p) = pos.checked_add(d) {
                if p < n {
                    out.push(image_indices[p]);
                }
            }
        }
        if d <= pf_back {
            if let Some(p) = pos.checked_sub(d) {
                out.push(image_indices[p]);
            }
        }
    }
    out
}

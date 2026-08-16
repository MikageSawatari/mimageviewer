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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FinalEffectPrefetchAdmission {
    Allow,
    NotInKeepSet,
    OverLowWatermark,
}

impl FinalEffectPrefetchAdmission {
    pub(crate) fn blocked_reason(self) -> Option<&'static str> {
        match self {
            Self::Allow => None,
            Self::NotInKeepSet => Some("not_in_keep_set"),
            Self::OverLowWatermark => Some("over_low_watermark"),
        }
    }
}

/// final-effect の先読み対象を viewer mode、連結読み keep-set、texel LOW 水位から判定する。
/// ページ送りでは keep-set / 水位を参照せず従来の AI 先読み対象を維持する。連結読みは
/// keep-set 内だけを許可し、準備帯は LOW 水位をバイパス、それ以外は LOW 未満に限定する。
pub(crate) fn should_prefetch_final_effect(
    reading_is_paged: bool,
    continuous_keep_set: &std::collections::HashSet<usize>,
    idx: usize,
    in_prepare_band: bool,
    continuous_loaded_texels: usize,
    continuous_low_watermark: Option<usize>,
) -> FinalEffectPrefetchAdmission {
    if reading_is_paged {
        return FinalEffectPrefetchAdmission::Allow;
    }
    if !continuous_keep_set.contains(&idx) {
        return FinalEffectPrefetchAdmission::NotInKeepSet;
    }
    if in_prepare_band || continuous_low_watermark.is_none_or(|low| continuous_loaded_texels < low)
    {
        FinalEffectPrefetchAdmission::Allow
    } else {
        FinalEffectPrefetchAdmission::OverLowWatermark
    }
}

/// 先読み対象を距離順・forward 先で交互配置: +1, -1, +2, -2, +3, -3, …
/// 同距離の組では forward (次ページ方向) が先。片側が尽きたら反対側だけ続く。
/// fs_cache / AI アップスケール / サムネイルグリッド の全先読みで方針統一。
pub(crate) fn interleaved_prefetch_positions(
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
                    out.push(p);
                }
            }
        }
        if d <= pf_back {
            if let Some(p) = pos.checked_sub(d) {
                out.push(p);
            }
        }
    }
    out
}

/// 表示順の位置で選んだ先読み対象を raw item index へ引き直す。
pub(crate) fn interleaved_prefetch_targets(
    image_indices: &[usize],
    pos: usize,
    n: usize,
    pf_forward: usize,
    pf_back: usize,
) -> Vec<usize> {
    interleaved_prefetch_positions(pos, n, pf_forward, pf_back)
        .into_iter()
        .map(|position| image_indices[position])
        .collect()
}

/// フルスクリーンの AI 先読み表示で使うページ単位の状態。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum FsPrefetchPageState {
    Ready,
    Active,
    Missing,
}

/// 完了済みを処理中より優先して、先読みページの表示状態を一意に決める。
pub(crate) fn fs_prefetch_page_state(ready: bool, active: bool) -> FsPrefetchPageState {
    if ready {
        FsPrefetchPageState::Ready
    } else if active {
        FsPrefetchPageState::Active
    } else {
        FsPrefetchPageState::Missing
    }
}

/// 遠方分を状態別の個数へ畳んだ 1 要素。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct FsPrefetchStateCount {
    pub(crate) state: FsPrefetchPageState,
    pub(crate) count: usize,
}

/// 片側の描画モデル。dots は現在ページに近い最大 `MAX_DOTS_PER_SIDE` ページ、
/// far_counts はそれより遠いページを状態別の個数へまとめたもの。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FsPrefetchSideDisplay {
    pub(crate) dots: Vec<FsPrefetchPageState>,
    pub(crate) far_counts: Vec<FsPrefetchStateCount>,
}

/// 現在ページを挟んだ AI 先読み表示。behind は遠い→近い、ahead は近い→遠い。
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct FsPrefetchIndicator {
    pub(crate) behind: Vec<FsPrefetchPageState>,
    pub(crate) ahead: Vec<FsPrefetchPageState>,
    pub(crate) ready_count: usize,
    pub(crate) active_count: usize,
    pub(crate) missing_count: usize,
}

impl FsPrefetchIndicator {
    /// 先読み枚数の設定上限は前後とも 10 枚。ここを 8 にすると上限まで増やしても
    /// 数値へ畳まれるのは 2 枚だけで、行が長いままになる。4 なら既定 (後方 1 / 前方 2)
    /// では全部が点のまま出て、増やしたときだけ遠方が数値へ退く。
    pub(crate) const MAX_DOTS_PER_SIDE: usize = 4;

    pub(crate) fn tooltip_text(&self) -> String {
        format!(
            "先読み: 取得済み {} / 取得中 {} / 未取得 {}",
            self.ready_count, self.active_count, self.missing_count
        )
    }

    /// behind は末尾が現在ページに最も近い。
    pub(crate) fn behind_display(&self) -> FsPrefetchSideDisplay {
        let first_dot = self.behind.len().saturating_sub(Self::MAX_DOTS_PER_SIDE);
        FsPrefetchSideDisplay {
            dots: self.behind[first_dot..].to_vec(),
            far_counts: summarize_fs_prefetch_states(&self.behind[..first_dot]),
        }
    }

    /// ahead は先頭が現在ページに最も近い。
    pub(crate) fn ahead_display(&self) -> FsPrefetchSideDisplay {
        let dot_count = self.ahead.len().min(Self::MAX_DOTS_PER_SIDE);
        FsPrefetchSideDisplay {
            dots: self.ahead[..dot_count].to_vec(),
            far_counts: summarize_fs_prefetch_states(&self.ahead[dot_count..]),
        }
    }
}

fn summarize_fs_prefetch_states(states: &[FsPrefetchPageState]) -> Vec<FsPrefetchStateCount> {
    [
        FsPrefetchPageState::Ready,
        FsPrefetchPageState::Active,
        FsPrefetchPageState::Missing,
    ]
    .into_iter()
    .filter_map(|state| {
        let count = states.iter().filter(|&&value| value == state).count();
        (count > 0).then_some(FsPrefetchStateCount { state, count })
    })
    .collect()
}

/// AI 先読み対象を表示順で現在ページの前後へ分け、リモート表示と同じ遠近順に整える。
///
/// 対象なし、現在ページ自身の AI 処理中、全対象の取得完了時は、従来どおり表示しない。
pub(crate) fn build_fs_prefetch_indicator(
    current_pos: usize,
    current_busy: bool,
    pages: impl IntoIterator<Item = (usize, FsPrefetchPageState)>,
) -> Option<FsPrefetchIndicator> {
    let mut behind = Vec::new();
    let mut ahead = Vec::new();
    let mut ready_count = 0;
    let mut active_count = 0;
    let mut missing_count = 0;

    for (pos, state) in pages {
        match pos.cmp(&current_pos) {
            std::cmp::Ordering::Less => behind.push((pos, state)),
            std::cmp::Ordering::Greater => ahead.push((pos, state)),
            std::cmp::Ordering::Equal => continue,
        }
        match state {
            FsPrefetchPageState::Ready => ready_count += 1,
            FsPrefetchPageState::Active => active_count += 1,
            FsPrefetchPageState::Missing => missing_count += 1,
        }
    }

    if behind.is_empty() && ahead.is_empty() {
        return None;
    }

    // 表示順の位置は単調に増える。behind は昇順で遠い→近い、ahead は昇順で近い→遠い。
    behind.sort_unstable_by_key(|(pos, _)| *pos);
    ahead.sort_unstable_by_key(|(pos, _)| *pos);

    if current_busy || active_count + missing_count == 0 {
        return None;
    }

    Some(FsPrefetchIndicator {
        behind: behind.into_iter().map(|(_, state)| state).collect(),
        ahead: ahead.into_iter().map(|(_, state)| state).collect(),
        ready_count,
        active_count,
        missing_count,
    })
}

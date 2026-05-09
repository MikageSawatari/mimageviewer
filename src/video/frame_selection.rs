//! Native presenter の 1 tick 分のフレーム選択ロジック (純粋関数)。
//!
//! 設計意図 (詳細は plan c-home-youtube-download-001 参照):
//! - 旧実装は `last_present_wall + source_delta` で「次の present 時刻」を相対積み上げ
//!   していたため、毎フレーム ~0.16ms の系統バイアスが累積し、約 7 秒に 1 回 frame
//!   drop が発生していた。
//! - 本モジュールは `clock.now_secs()` の絶対値だけで「今 tick で何をするか」を決め、
//!   寝過ごしや起床ジッタを次回基準に混ぜない。
//! - 副作用なし (perf event / drop counter 更新は呼び出し側) なので unit test 可能。

use crate::video::clock;

/// queue 先頭からの 1 アクション。
#[derive(Debug, PartialEq, Clone, Copy)]
pub(crate) enum PopAction {
    /// 古い seek_serial の frame: 静かに捨てる (late_drop には計上しない)。
    DiscardStale,
    /// 同 tick で display の前に置き換えで捨てる: late_drop +1。
    /// scheduler stall 等で複数 frame が同時 eligible になった時のみ発生する想定。
    LateDrop,
    /// この frame を `latest_renderable` にする (= 表示候補)。
    /// actions 中に高々 1 回現れる。Display 以降は actions に追加されない。
    Display,
}

/// 1 tick 分の frame 選択結果。actions の順番が `VecDeque::pop_front()` 順。
///
/// 不変条件:
/// - actions 中の `Display` は高々 1 つ。
/// - `Display` 以降は actions に追加されない (= present に進む)。
/// - 空の actions は「何もしない (Wait)」を表す。
#[derive(Debug, PartialEq)]
pub(crate) struct FrameSelection {
    pub actions: Vec<PopAction>,
}

/// 純粋関数 helper のための frame メタ。実 `VideoFrame` から (pts, serial) だけ抜き出す。
#[derive(Debug, Clone, Copy)]
pub(crate) struct FrameCandidate {
    pub pts_secs: f64,
    pub seek_serial: u64,
}

/// 1 tick の frame 選択を計算する純粋関数。
///
/// アルゴリズム (queue 先頭から走査):
/// 1. `seek_serial < current_seek_serial` なら `DiscardStale` を吐いて次へ。
/// 2. `force_first_frame` (= source 切替直後の first present 待ち) なら `Display` を
///    1 つ吐いて即終了 (= 起動 burst-drop の防止)。
/// 3. `force_display_seek` (= seek 中で target 近傍 / overshoot 許容) または
///    `pts_secs <= now_secs + tol` (= 通常 eligibility) なら eligible。前に Display を
///    出していれば、それを `LateDrop` に格下げして新しい `Display` を吐く。
///    `force_display_seek` は Fast seek の keyframe→target burst 消化設計
///    ([src/video/clock.rs] の `SeekKind::Fast` コメント参照) を維持するため
///    coalesce を許可する (= 1 枚で打ち切らない)。
/// 4. eligible でなければ走査終了 (= 残りは next tick 以降)。
pub(crate) fn select_frame_for_present(
    queue: &[FrameCandidate],
    now_secs: f64,
    current_seek_serial: u64,
    last_seen_serial: u64,
    waiting_for_first_frame: bool,
    is_seeking: bool,
    display_lead_tolerance_secs: f64,
) -> FrameSelection {
    let mut actions: Vec<PopAction> = Vec::new();
    let mut have_display_candidate = false;

    for frame in queue {
        if frame.seek_serial < current_seek_serial {
            actions.push(PopAction::DiscardStale);
            continue;
        }

        let force_first_frame = waiting_for_first_frame && frame.seek_serial == last_seen_serial;
        if force_first_frame {
            actions.push(PopAction::Display);
            return FrameSelection { actions };
        }

        // `force_display_seek` は seek 中の first frame に限り pts > now でも eligible
        // 扱いする (`pts_clears_seek_override` が片側許容 = pts > target は無制限)。
        // 2 枚目以降は have_display_candidate=true で false になり、自然に通常
        // eligibility 経路に戻る (= 元実装と同じ挙動)。
        let force_display_seek = is_seeking
            && !have_display_candidate
            && frame.seek_serial == last_seen_serial
            && clock::pts_clears_seek_override(frame.pts_secs, now_secs);

        let eligible =
            force_display_seek || frame.pts_secs <= now_secs + display_lead_tolerance_secs;

        if eligible {
            if have_display_candidate {
                let last = actions
                    .last_mut()
                    .expect("have_display_candidate implies a prior Display");
                debug_assert_eq!(*last, PopAction::Display);
                *last = PopAction::LateDrop;
            }
            actions.push(PopAction::Display);
            have_display_candidate = true;
            continue;
        }

        break;
    }

    FrameSelection { actions }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TOL: f64 = 0.001;

    fn frame(pts_secs: f64, seek_serial: u64) -> FrameCandidate {
        FrameCandidate {
            pts_secs,
            seek_serial,
        }
    }

    #[test]
    fn waiting_for_first_frame_picks_only_one() {
        let queue = vec![
            frame(0.000, 1),
            frame(0.033, 1),
            frame(0.067, 1),
            frame(0.100, 1),
            frame(0.133, 1),
        ];
        let sel = select_frame_for_present(&queue, 0.500, 1, 1, true, false, TOL);
        assert_eq!(sel.actions, vec![PopAction::Display]);
    }

    #[test]
    fn steady_state_one_eligible_no_drop() {
        let queue = vec![frame(0.033, 1), frame(0.066, 1)];
        let sel = select_frame_for_present(&queue, 0.0333, 1, 1, false, false, TOL);
        assert_eq!(sel.actions, vec![PopAction::Display]);
    }

    #[test]
    fn scheduler_stall_drops_older_frame() {
        let queue = vec![frame(0.033, 1), frame(0.066, 1)];
        let sel = select_frame_for_present(&queue, 0.0665, 1, 1, false, false, TOL);
        assert_eq!(sel.actions, vec![PopAction::LateDrop, PopAction::Display]);
    }

    #[test]
    fn future_frame_remains_in_queue() {
        let queue = vec![frame(0.100, 1)];
        let sel = select_frame_for_present(&queue, 0.033, 1, 1, false, false, TOL);
        assert_eq!(sel.actions, vec![] as Vec<PopAction>);
    }

    #[test]
    fn stale_frames_are_discarded_not_dropped() {
        let queue = vec![frame(0.010, 1), frame(0.033, 2)];
        let sel = select_frame_for_present(&queue, 0.0333, 2, 2, false, false, TOL);
        assert_eq!(
            sel.actions,
            vec![PopAction::DiscardStale, PopAction::Display]
        );
    }

    #[test]
    fn force_display_seek_allows_coalesce_to_target() {
        // Fast seek: keyframe→target burst を 1 tick で消化する設計
        // ([src/video/clock.rs] `SeekKind::Fast` 参照)。target 近傍の連続 frame は
        // 通常 eligibility 経由で coalesce され、最新だけ Display される。
        let queue = vec![frame(4.500, 1), frame(4.533, 1), frame(5.000, 1)];
        let sel = select_frame_for_present(&queue, 5.000, 1, 1, false, true, TOL);
        assert_eq!(
            sel.actions,
            vec![PopAction::LateDrop, PopAction::LateDrop, PopAction::Display]
        );
    }

    #[test]
    fn force_display_seek_picks_overshoot_future_frame() {
        // 4K HEVC の GOP overshoot: keyframe が target を越して着地するケース。
        // 通常 eligibility (pts <= now + tol) では false だが、
        // pts_clears_seek_override (片側許容、pts > target は無制限) で eligible 扱い。
        let queue = vec![frame(5.500, 1)];
        let sel = select_frame_for_present(&queue, 5.000, 1, 1, false, true, TOL);
        assert_eq!(sel.actions, vec![PopAction::Display]);
    }

    #[test]
    fn force_display_seek_inactive_when_not_seeking() {
        // is_seeking=false なら force_display_seek は発火しない。
        // pts > now + tol の future frame はそのまま queue に残る。
        let queue = vec![frame(5.500, 1)];
        let sel = select_frame_for_present(&queue, 5.000, 1, 1, false, false, TOL);
        assert_eq!(sel.actions, vec![] as Vec<PopAction>);
    }

    #[test]
    fn three_eligible_drops_two_displays_one() {
        // 真の scheduler stall: 3 枚同時 eligible。古い 2 枚 LateDrop、最後を Display。
        let queue = vec![frame(0.033, 1), frame(0.066, 1), frame(0.099, 1)];
        let sel = select_frame_for_present(&queue, 0.0995, 1, 1, false, false, TOL);
        assert_eq!(
            sel.actions,
            vec![PopAction::LateDrop, PopAction::LateDrop, PopAction::Display]
        );
    }

    #[test]
    fn stale_then_force_first_frame() {
        // 古い世代の frame が queue に残っており、その後ろに新世代の frame がある。
        // stale を Discard してから force_first_frame で 1 枚 Display。
        let queue = vec![frame(0.500, 1), frame(0.000, 2)];
        let sel = select_frame_for_present(&queue, 0.000, 2, 2, true, false, TOL);
        assert_eq!(
            sel.actions,
            vec![PopAction::DiscardStale, PopAction::Display]
        );
    }
}

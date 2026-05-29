//! サムネイル比率の自動選択 — 純関数モジュール。
//!
//! 設計ドキュメント: [docs/auto-thumb-aspect-plan.md](../../docs/auto-thumb-aspect-plan.md)
//!
//! 副作用なしの判定ロジックのみをここに置く。cooldown / streak / scroll-idle /
//! switches_done のような **state-dependent** な判定は呼び出し側 (App) で行う。
//!
//! ## 設計上のメモ: なぜ「median-of-log-ratio」か
//!
//! 設計レビュー段階では Codex 提案の「フィット指標 `min(r/a, a/r)` の平均最大化」を
//! 採用していたが、実装着手時の単体テストで「半数 r=0.5 + 半数 r=2.0」のような
//! **bimodal 対称分布で Landscape16x9 / Portrait9x16 が選ばれる**ことが判明
//! (両端バケットの「合う側 0.889」が「合わない側 0.281」を mean で上回り、
//! Square の 0.5 を勝つため)。
//!
//! ユーザー決定 C「混在 → Square」と矛盾するため、**log(ratio) の中央値 →
//! 最近接バケット**方式に切り替えた:
//! - 中央値は外れ値に頑健で、左右対称な分布なら自然に median=0 (= log 1.0) に収束
//! - 「対称な縦横混在 → Square」というユーザーの直感に一致
//! - 一次元のため計算もシンプル
//!
//! `fit_score` 関数は残してあるが、選択ロジックではなく診断・テスト用途のみ。

use crate::settings::ThumbAspect;
use std::collections::HashMap;
use std::time::Instant;

/// サムネイル比率の自動選択のランタイム状態。
///
/// App 構造体に 1 つ保持し、フォルダ切替で `reset_for_new_generation()` を呼ぶ。
/// 行動 (sample 取り込み・切替判定・cooldown 管理) は `App` の impl 側で行う。
#[derive(Debug)]
pub struct AutoAspectState {
    /// このフォルダの items 世代。世代が変わったら全リセット。
    pub items_generation: u64,
    /// 集計済みサンプル: idx -> ratio (h/w)。重複追加防止のため idx キー。
    pub samples: HashMap<usize, f32>,
    /// 確定済み (or 仮確定) の自動比率。None なら未確定。
    /// auto モード時に `App::effective_thumb_aspect` から参照される。
    pub current: Option<ThumbAspect>,
    /// auto_aspect_cache.db から復元した比率を、前回と同等以上の実測 sample が
    /// 集まるまで上書きしないためのゲート。None なら通常判定。
    pub cached_sample_gate: Option<usize>,
    /// このフォルダで何回切り替えたか (0..=2)。最大値到達後は再切替しない。
    pub switches_done: u8,
    /// 直近切替時刻 (cooldown 判定用)。
    pub last_switch_at: Option<Instant>,
    /// 連勝中の候補とその継続情報。
    /// `(候補, 連勝開始時刻, 連勝開始時のサンプル数)` — 切替条件「750ms または
    /// +8 サンプルで同候補が勝ち続ける」を判定する。
    pub streak: Option<(ThumbAspect, Instant, usize)>,
}

impl Default for AutoAspectState {
    fn default() -> Self {
        Self {
            items_generation: 0,
            samples: HashMap::new(),
            current: None,
            cached_sample_gate: None,
            switches_done: 0,
            last_switch_at: None,
            streak: None,
        }
    }
}

impl AutoAspectState {
    /// 新しいフォルダに移ったときの全リセット。
    pub fn reset_for_new_generation(&mut self, generation: u64) {
        self.items_generation = generation;
        self.samples.clear();
        self.current = None;
        self.cached_sample_gate = None;
        self.switches_done = 0;
        self.last_switch_at = None;
        self.streak = None;
    }

    /// 「自動」をユーザーが (再) 選択したときの確定値リセット。
    /// `samples` は活かしたまま、次の判定で再評価できるようにする。
    pub fn reset_decision_only(&mut self) {
        self.current = None;
        self.cached_sample_gate = None;
        self.switches_done = 0;
        self.last_switch_at = None;
        self.streak = None;
    }
}

/// 「セル比率 `candidate` に対して画像比率 `ratio` がどれだけセルを埋めるか」のスコア。
///
/// - `ratio = h / w` (画像の高さ / 幅)
/// - `candidate.height_ratio() = a` (セルの高さ / 幅)
/// - 戻り値は `min(r/a, a/r)` で `0 < fit ≤ 1`、1 が完全一致
///
/// 主に **診断・テスト用**。実際のバケット選択は `pick_best` (= median-of-log-ratio)
/// が行う。
pub fn fit_score(ratio: f32, candidate: ThumbAspect) -> f32 {
    if !ratio.is_finite() || ratio <= 0.0 {
        return 0.0;
    }
    let a = candidate.height_ratio();
    if !a.is_finite() || a <= 0.0 {
        return 0.0;
    }
    (ratio / a).min(a / ratio)
}

/// `log(r)` の中央値に最も近い `log(height_ratio)` を持つ `ThumbAspect` を返す。
/// `target` 単独で渡せるよう純関数にしてある (`pick_best` のヘルパー兼テスト容易性)。
fn nearest_bucket_to_log_ratio(target: f32) -> ThumbAspect {
    let mut best: ThumbAspect = ThumbAspect::Square;
    let mut best_dist = f32::INFINITY;
    for &candidate in ThumbAspect::all() {
        let a = candidate.height_ratio();
        if !a.is_finite() || a <= 0.0 {
            continue;
        }
        let dist = (target - a.ln()).abs();
        if dist < best_dist {
            best_dist = dist;
            best = candidate;
        }
    }
    best
}

/// `samples` (各要素は `h / w`) から **log 空間の中央値**を計算し、その値に
/// 最も近い `ThumbAspect` バケットを返す。
///
/// - `samples` が空、または有効値がない場合は `None`
/// - 不正値 (`<= 0.0`, NaN, Inf) は集計から除外
/// - 偶数個の場合は中央 2 値の平均を取る (= 教科書的な中央値)
///
/// 計算量: `O(N log N)` (ソート)。N は数十程度なので無視できる。
pub fn pick_best(samples: &[f32]) -> Option<ThumbAspect> {
    let mut log_ratios: Vec<f32> = samples
        .iter()
        .filter(|&&r| r > 0.0 && r.is_finite())
        .map(|&r| r.ln())
        .collect();
    if log_ratios.is_empty() {
        return None;
    }
    log_ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = log_ratios.len();
    let median = if n % 2 == 1 {
        log_ratios[n / 2]
    } else {
        (log_ratios[n / 2 - 1] + log_ratios[n / 2]) / 2.0
    };
    Some(nearest_bucket_to_log_ratio(median))
}

/// 確定判定に必要な最小サンプル数を返す。
///
/// 式 (plan §4.3):
/// - 基本は `max(8, eligible_total / 4)`、上限 `24`
/// - ただし `eligible_total` 自体を超えないよう clip (= 小フォルダ対応)
///
/// 例: `eligible_total=5 → 5`、`eligible_total=20 → 8`、`eligible_total=100 → 24`。
pub fn min_samples_for(eligible_total: usize) -> usize {
    let ideal = (eligible_total / 4).max(8).min(24);
    ideal.min(eligible_total)
}

/// `decide_auto_aspect` の戻り値。
///
/// - `Hold`: 切り替えない (サンプル不足 / current と同じ / 改善幅不足)
/// - `Switch(best)`: `best` に切り替えるべき
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum AspectDecision {
    Hold,
    Switch(ThumbAspect),
}

/// サンプルとヒステリシス基準だけで「切り替えるべきか」を決める純関数。
///
/// この関数の責務は **サンプル下限到達** + **log 距離マージン判定** のみ。
/// cooldown / switches_done / streak / scroll-idle は呼び出し側 (App) で判定する。
///
/// `log_margin` はバケット間距離の log 空間でのマージン。
/// 例: `0.10` (= 約 10% の比率差) で「current バケットより best バケットの方が
/// log_margin 以上中央値に近い」場合のみ switch。
///
/// 隣接バケット間の log 距離の最小は約 0.117 (例: Landscape4x3↔Landscape3x2)
/// なので、`log_margin = 0.05` 程度なら隣接遷移も許容、`0.10` でやや慎重、
/// `0.15` で「2 バケット以上跨ぐ場合のみ」に近い。実機で調整する。
pub fn decide_auto_aspect(
    samples: &[f32],
    eligible_total: usize,
    current: ThumbAspect,
    log_margin: f32,
) -> AspectDecision {
    if samples.len() < min_samples_for(eligible_total) {
        return AspectDecision::Hold;
    }
    // median を取る (pick_best のロジックを再利用するため一旦中央値を計算)
    let mut log_ratios: Vec<f32> = samples
        .iter()
        .filter(|&&r| r > 0.0 && r.is_finite())
        .map(|&r| r.ln())
        .collect();
    if log_ratios.is_empty() {
        return AspectDecision::Hold;
    }
    log_ratios.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let n = log_ratios.len();
    let median = if n % 2 == 1 {
        log_ratios[n / 2]
    } else {
        (log_ratios[n / 2 - 1] + log_ratios[n / 2]) / 2.0
    };
    let best = nearest_bucket_to_log_ratio(median);
    if best == current {
        return AspectDecision::Hold;
    }
    let curr_log = current.height_ratio().ln();
    let best_log = best.height_ratio().ln();
    let curr_dist = (median - curr_log).abs();
    let best_dist = (median - best_log).abs();
    if curr_dist - best_dist > log_margin {
        AspectDecision::Switch(best)
    } else {
        AspectDecision::Hold
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approx_eq(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    // --- fit_score (診断用) ---

    #[test]
    fn fit_score_perfect_match_is_1() {
        assert!(approx_eq(fit_score(1.0, ThumbAspect::Square), 1.0));
        assert!(approx_eq(
            fit_score(9.0 / 16.0, ThumbAspect::Landscape16x9),
            1.0
        ));
        assert!(approx_eq(fit_score(1.5, ThumbAspect::Portrait2x3), 1.0));
    }

    #[test]
    fn fit_score_symmetric() {
        let a = ThumbAspect::Square;
        assert!(approx_eq(fit_score(2.0, a), fit_score(0.5, a)));
        assert!(approx_eq(fit_score(2.0, ThumbAspect::Square), 0.5));
    }

    #[test]
    fn fit_score_invalid_inputs() {
        assert_eq!(fit_score(0.0, ThumbAspect::Square), 0.0);
        assert_eq!(fit_score(-1.0, ThumbAspect::Square), 0.0);
        assert_eq!(fit_score(f32::NAN, ThumbAspect::Square), 0.0);
        assert_eq!(fit_score(f32::INFINITY, ThumbAspect::Square), 0.0);
    }

    // --- nearest_bucket_to_log_ratio ---

    #[test]
    fn nearest_bucket_zero_is_square() {
        // log(1.0) = 0 → Square にぴったり
        assert_eq!(nearest_bucket_to_log_ratio(0.0), ThumbAspect::Square);
    }

    #[test]
    fn nearest_bucket_exact_buckets() {
        for &candidate in ThumbAspect::all() {
            let target = candidate.height_ratio().ln();
            assert_eq!(
                nearest_bucket_to_log_ratio(target),
                candidate,
                "exact log(height_ratio) should return the same bucket"
            );
        }
    }

    // --- pick_best ---

    #[test]
    fn pick_best_empty_is_none() {
        assert_eq!(pick_best(&[]), None);
    }

    #[test]
    fn pick_best_all_square() {
        let samples = vec![1.0; 10];
        assert_eq!(pick_best(&samples), Some(ThumbAspect::Square));
    }

    #[test]
    fn pick_best_all_portrait_2x3() {
        // r = 1.5 → Portrait2x3 (h/w = 3/2 = 1.5) が完全一致
        let samples = vec![1.5; 10];
        assert_eq!(pick_best(&samples), Some(ThumbAspect::Portrait2x3));
    }

    #[test]
    fn pick_best_all_landscape_16x9() {
        // r = 9/16 ≈ 0.5625 → Landscape16x9 (a=9/16) が完全一致
        let samples = vec![9.0 / 16.0; 10];
        assert_eq!(pick_best(&samples), Some(ThumbAspect::Landscape16x9));
    }

    #[test]
    fn pick_best_mixed_lands_on_square() {
        // 半数 r=0.5 (横長), 半数 r=2.0 (縦長): 対称分布
        // log(0.5) = -0.693, log(2.0) = 0.693, median = 0 → Square
        let samples = vec![0.5, 2.0, 0.5, 2.0, 0.5, 2.0, 0.5, 2.0];
        assert_eq!(pick_best(&samples), Some(ThumbAspect::Square));
    }

    #[test]
    fn pick_best_all_portrait_9x16() {
        // r = 16/9 ≈ 1.778 → Portrait9x16 (a=16/9) が完全一致
        let samples = vec![16.0 / 9.0; 10];
        assert_eq!(pick_best(&samples), Some(ThumbAspect::Portrait9x16));
    }

    #[test]
    fn pick_best_ignores_invalid_values() {
        // 不正値は除外、残りの 1.5 のみが効く
        let samples = vec![0.0, f32::NAN, f32::INFINITY, -1.0, 1.5, 1.5, 1.5];
        assert_eq!(pick_best(&samples), Some(ThumbAspect::Portrait2x3));
    }

    #[test]
    fn pick_best_all_invalid_is_none() {
        let samples = vec![0.0, -1.0, f32::NAN];
        assert_eq!(pick_best(&samples), None);
    }

    // --- min_samples_for (境界値表 — plan §9.1) ---

    #[test]
    fn min_samples_for_boundaries() {
        assert_eq!(min_samples_for(0), 0);
        assert_eq!(min_samples_for(1), 1);
        assert_eq!(min_samples_for(5), 5);
        assert_eq!(min_samples_for(8), 8);
        assert_eq!(min_samples_for(20), 8); // 20/4 = 5 < 8 → 下限 8
        assert_eq!(min_samples_for(32), 8); // 32/4 = 8 ぴったり
        assert_eq!(min_samples_for(36), 9); // 36/4 = 9 で 25% ルール
        assert_eq!(min_samples_for(96), 24); // 96/4 = 24 上限ぴったり
        assert_eq!(min_samples_for(100), 24); // 上限でクリップ
        assert_eq!(min_samples_for(1000), 24); // 上限維持
    }

    // --- decide_auto_aspect (ヒステリシステスト) ---

    #[test]
    fn decide_holds_when_samples_below_min() {
        // eligible_total = 20 → min_samples = 8。samples = 3 件は未達
        let samples = vec![1.5; 3];
        let d = decide_auto_aspect(&samples, 20, ThumbAspect::Square, 0.10);
        assert_eq!(d, AspectDecision::Hold);
    }

    #[test]
    fn decide_small_folder_can_decide_when_full() {
        // eligible_total = 5 → min_samples = 5。samples = 5 件で到達
        // log(1.5) ≈ 0.405、log(1.0)=0 で Square から見ると 0.405 離れる。
        // log(Portrait2x3) = log(1.5) = 0.405 でぴったり、距離 0。
        // 改善 = 0.405 - 0 = 0.405 > log_margin 0.10 → Switch
        let samples = vec![1.5; 5];
        let d = decide_auto_aspect(&samples, 5, ThumbAspect::Square, 0.10);
        assert_eq!(d, AspectDecision::Switch(ThumbAspect::Portrait2x3));
    }

    #[test]
    fn decide_small_folder_holds_when_short() {
        // eligible_total = 5, samples = 4 件は未達
        let samples = vec![1.5; 4];
        let d = decide_auto_aspect(&samples, 5, ThumbAspect::Square, 0.10);
        assert_eq!(d, AspectDecision::Hold);
    }

    #[test]
    fn decide_holds_when_best_equals_current() {
        // 全件 1:1 で current も Square
        let samples = vec![1.0; 10];
        let d = decide_auto_aspect(&samples, 10, ThumbAspect::Square, 0.10);
        assert_eq!(d, AspectDecision::Hold);
    }

    #[test]
    fn decide_holds_for_small_improvement() {
        // 全件 r = 1.05 (log ≈ 0.0488)、current = Square (log=0)
        //   curr_dist = 0.0488
        //   best = Square (= current、最近接) → Hold (best == current 経路)
        //
        // current を 1 つズラして Portrait3x4 (log=0.288) にする:
        //   median = 0.0488
        //   curr_dist (Portrait3x4) = |0.0488 - 0.288| = 0.239
        //   best = Square (距離 0.0488)
        //   改善 = 0.239 - 0.0488 = 0.190 > 0.10 → Switch
        // → これは Switch される。「小改善で Hold」を見るには median を best と
        //    current のちょうど真ん中近くに置く必要がある。
        //
        // median = 0.15 (Square と Portrait3x4 の中間付近)。
        //   curr (Square) 距離 0.15、best (Portrait3x4) 距離 0.138 → 改善 0.012 → Hold
        let target_log: f32 = 0.15;
        let r = target_log.exp(); // ≈ 1.162
        let samples = vec![r; 10];
        let d = decide_auto_aspect(&samples, 10, ThumbAspect::Square, 0.10);
        assert_eq!(d, AspectDecision::Hold);
    }

    #[test]
    fn decide_switches_for_large_improvement() {
        // 全件 r = 1.0 (Square にぴったり)、current = Portrait9x16 (log = 0.575)
        //   median = 0
        //   curr_dist = 0.575
        //   best (Square) 距離 0
        //   改善 = 0.575 > 0.10 → Switch
        let samples = vec![1.0; 10];
        let d = decide_auto_aspect(&samples, 10, ThumbAspect::Portrait9x16, 0.10);
        assert_eq!(d, AspectDecision::Switch(ThumbAspect::Square));
    }

    #[test]
    fn decide_mixed_symmetric_switches_to_square_from_portrait() {
        // 対称混在、current が Portrait9x16 のとき Square に向かう
        let samples = vec![0.5, 2.0, 0.5, 2.0, 0.5, 2.0, 0.5, 2.0, 0.5, 2.0];
        let d = decide_auto_aspect(&samples, 10, ThumbAspect::Portrait9x16, 0.10);
        assert_eq!(d, AspectDecision::Switch(ThumbAspect::Square));
    }

    // --- AutoAspectState reset methods ---

    #[test]
    fn auto_state_reset_for_new_generation_clears_everything() {
        let mut s = AutoAspectState::default();
        s.samples.insert(0, 1.5);
        s.samples.insert(1, 0.5);
        s.current = Some(ThumbAspect::Portrait2x3);
        s.cached_sample_gate = Some(24);
        s.switches_done = 2;
        s.last_switch_at = Some(Instant::now());
        s.streak = Some((ThumbAspect::Square, Instant::now(), 3));

        s.reset_for_new_generation(42);

        assert_eq!(s.items_generation, 42);
        assert!(s.samples.is_empty());
        assert_eq!(s.current, None);
        assert_eq!(s.cached_sample_gate, None);
        assert_eq!(s.switches_done, 0);
        assert!(s.last_switch_at.is_none());
        assert!(s.streak.is_none());
    }

    #[test]
    fn auto_state_reset_decision_only_keeps_samples() {
        let mut s = AutoAspectState::default();
        s.items_generation = 7;
        s.samples.insert(0, 1.5);
        s.samples.insert(1, 1.5);
        s.current = Some(ThumbAspect::Portrait2x3);
        s.cached_sample_gate = Some(24);
        s.switches_done = 1;
        s.last_switch_at = Some(Instant::now());
        s.streak = Some((ThumbAspect::Square, Instant::now(), 0));

        s.reset_decision_only();

        // samples / items_generation は活きる
        assert_eq!(s.items_generation, 7);
        assert_eq!(s.samples.len(), 2);
        // 決定状態だけリセット
        assert_eq!(s.current, None);
        assert_eq!(s.cached_sample_gate, None);
        assert_eq!(s.switches_done, 0);
        assert!(s.last_switch_at.is_none());
        assert!(s.streak.is_none());
    }
}

//! フルスクリーン系シークバーの目盛り位置を生成する純ロジック。
//!
//! 呼び出し側は返された論理 fraction を各シークバー固有の方向規約で x 座標へ変換し、
//! 描画だけを担当する。ここではページ番号と時間の「切りの良い」刻み選択を一元化する。

/// ページ目盛りに確保する最小の論理間隔。
pub(crate) const SEEK_RULER_MIN_SPACING: f32 = 16.0;
/// 時間目盛りに確保する最小の論理間隔。
///
/// ページより広く取る。1 ページは利用者にとって数えられる単位だが、1 秒はそうではなく、
/// 同じ間隔だと 2 分程度の動画で秒刻みが選ばれて櫛のようになる (実機 FB 2026-08-12)。
pub(crate) const SEEK_RULER_TIME_MIN_SPACING: f32 = 48.0;
/// トラック下端から目盛りまでの空き。
pub(crate) const SEEK_RULER_GAP: f32 = 2.0;
pub(crate) const SEEK_RULER_MINOR_HEIGHT: f32 = 2.0;
pub(crate) const SEEK_RULER_MAJOR_HEIGHT: f32 = 5.0;
pub(crate) const SEEK_RULER_STROKE_WIDTH: f32 = 1.0;
/// 小 / 大の無彩色。長さだけでは差が読み取れないので明度差も付ける (実機 FB 2026-08-12)。
/// どちらもトラック (gray 74) やつまみより主張を弱くする。
pub(crate) const SEEK_RULER_MINOR_GRAY: u8 = 92;
pub(crate) const SEEK_RULER_MAJOR_GRAY: u8 = 168;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct RulerTick {
    pub(crate) fraction: f32,
    pub(crate) major: bool,
}

const TIME_STEPS_SECS: [u64; 16] = [
    1,
    2,
    5,
    10,
    15,
    30,
    60,
    2 * 60,
    5 * 60,
    10 * 60,
    15 * 60,
    30 * 60,
    60 * 60,
    2 * 60 * 60,
    6 * 60 * 60,
    12 * 60 * 60,
];

fn max_tick_count(track_width: f32, min_spacing: f32) -> Option<usize> {
    if !track_width.is_finite()
        || !min_spacing.is_finite()
        || track_width <= 0.0
        || min_spacing <= 0.0
    {
        return None;
    }
    let count = (track_width / min_spacing).floor() as usize;
    (count >= 2).then_some(count)
}

fn nice_page_step_at_least(required: usize) -> Option<usize> {
    let required = required.max(1);
    let mut magnitude = 1usize;
    loop {
        for multiplier in [1usize, 2, 5] {
            let candidate = magnitude.checked_mul(multiplier)?;
            if candidate >= required {
                return Some(candidate);
            }
        }
        magnitude = magnitude.checked_mul(10)?;
    }
}

fn page_major_step(step: usize) -> usize {
    let mut leading = step;
    while leading >= 10 {
        leading /= 10;
    }
    let factor = if leading == 5 { 2 } else { 5 };
    step.saturating_mul(factor)
}

/// ページ / 表示ユニット数から目盛りを作る。
///
/// ページ番号そのものが 1 / 2 / 5 系の刻みへ揃うよう、間引き時は 10, 20, ...
/// のような刻みの倍数だけを返す。fraction は先頭=0、末尾=1 のページ seek 規約。
pub(crate) fn page_ruler_ticks(
    page_count: usize,
    track_width: f32,
    min_spacing: f32,
) -> Vec<RulerTick> {
    if page_count <= 1 {
        return Vec::new();
    }
    let Some(max_ticks) = max_tick_count(track_width, min_spacing) else {
        return Vec::new();
    };
    let required_step = page_count.div_ceil(max_ticks);
    let Some(step) = nice_page_step_at_least(required_step) else {
        return Vec::new();
    };
    let tick_count = if step == 1 {
        page_count
    } else {
        page_count / step
    };
    if tick_count == 0 || tick_count > max_ticks {
        return Vec::new();
    }

    let major_step = page_major_step(step);
    let first_page = if step == 1 { 1 } else { step };
    let denominator = (page_count - 1) as f32;
    let mut ticks = Vec::with_capacity(tick_count);
    let mut page_number = first_page;
    while page_number <= page_count {
        ticks.push(RulerTick {
            fraction: ((page_number - 1) as f32 / denominator).clamp(0.0, 1.0),
            major: page_number % major_step == 0,
        });
        let Some(next) = page_number.checked_add(step) else {
            break;
        };
        page_number = next;
    }
    ticks
}

fn time_tick_count(duration_secs: f64, step_secs: u64) -> Option<usize> {
    let intervals = (duration_secs / step_secs as f64).floor();
    if !intervals.is_finite() || intervals > (usize::MAX - 1) as f64 {
        return None;
    }
    Some(intervals as usize + 1)
}

/// 小目盛りに対する大目盛りの刻み。
///
/// 「次の単位 (分 / 時 / 日)」固定にすると、1 秒刻みで大目盛りが 60 本に 1 本しか出ず、
/// 目盛りが一様な櫛に見える (実機 FB 2026-08-12)。同じ許可系列の中から、小目盛りの
/// 倍数かつ 4 倍以上の最小値を選び、4〜6 本ごとに大目盛りが来るようにする。
fn time_major_step(step_secs: u64) -> u64 {
    let step_secs = step_secs.max(1);
    TIME_STEPS_SECS
        .iter()
        .copied()
        .find(|&candidate| candidate >= step_secs.saturating_mul(4) && candidate % step_secs == 0)
        .unwrap_or_else(|| step_secs.saturating_mul(5))
}

/// 再生時間から目盛りを作る。
///
/// 秒・分・時間の許可系列から、描画本数が幅に比例する最小の刻みを選ぶ。大目盛りは
/// 秒刻みなら分、分刻みなら時、時刻みなら日の切れ目に揃える。
pub(crate) fn duration_ruler_ticks(
    duration_secs: f64,
    track_width: f32,
    min_spacing: f32,
) -> Vec<RulerTick> {
    if !duration_secs.is_finite() || duration_secs <= 0.0 {
        return Vec::new();
    }
    let Some(max_ticks) = max_tick_count(track_width, min_spacing) else {
        return Vec::new();
    };
    let Some((step_secs, tick_count)) = TIME_STEPS_SECS.iter().find_map(|&step| {
        let count = time_tick_count(duration_secs, step)?;
        (count <= max_ticks).then_some((step, count))
    }) else {
        return Vec::new();
    };

    let major_every = time_major_step(step_secs) / step_secs;
    (0..tick_count)
        .map(|index| {
            let seconds = index as f64 * step_secs as f64;
            RulerTick {
                fraction: (seconds / duration_secs).clamp(0.0, 1.0) as f32,
                major: index as u64 % major_every == 0,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_fraction_contract(ticks: &[RulerTick]) {
        assert!(!ticks.is_empty());
        for tick in ticks {
            assert!((0.0..=1.0).contains(&tick.fraction));
        }
        assert!(
            ticks
                .windows(2)
                .all(|pair| pair[0].fraction < pair[1].fraction),
        );
    }

    fn is_page_125_step(mut step: usize) -> bool {
        while step >= 10 && step % 10 == 0 {
            step /= 10;
        }
        matches!(step, 1 | 2 | 5)
    }

    #[test]
    fn huge_page_tick_count_is_width_bounded() {
        let ticks = page_ruler_ticks(100_000, 1000.0, 8.0);
        assert!(!ticks.is_empty());
        assert!(ticks.len() <= 125);
    }

    #[test]
    fn ten_hour_tick_count_is_width_bounded() {
        let ticks = duration_ruler_ticks(10.0 * 60.0 * 60.0, 1000.0, 8.0);
        assert!(!ticks.is_empty());
        assert!(ticks.len() <= 125);
    }

    #[test]
    fn generated_fractions_are_bounded_and_strictly_increasing() {
        assert_fraction_contract(&page_ruler_ticks(100_000, 1000.0, 8.0));
        assert_fraction_contract(&duration_ruler_ticks(10.0 * 60.0 * 60.0, 1000.0, 8.0));
    }

    #[test]
    fn selected_page_step_belongs_to_125_series_and_clean_pages() {
        let page_count = 100_000;
        let ticks = page_ruler_ticks(page_count, 1000.0, 8.0);
        let pages = ticks
            .iter()
            .map(|tick| (tick.fraction * (page_count - 1) as f32).round() as usize + 1)
            .collect::<Vec<_>>();
        let step = pages[1] - pages[0];

        assert!(is_page_125_step(step));
        assert!(pages.iter().all(|page| page % step == 0));
    }

    #[test]
    fn selected_time_step_belongs_to_required_series() {
        let duration_secs = 10.0 * 60.0 * 60.0;
        let ticks = duration_ruler_ticks(duration_secs, 1000.0, 8.0);
        let step = (ticks[1].fraction as f64 * duration_secs).round() as u64;

        assert!(TIME_STEPS_SECS.contains(&step));
    }

    #[test]
    fn small_page_counts_use_one_page_steps() {
        for page_count in [2usize, 3, 10] {
            let ticks = page_ruler_ticks(page_count, 1000.0, 8.0);
            assert_eq!(ticks.len(), page_count);
            for (index, tick) in ticks.iter().enumerate() {
                let page_number = (tick.fraction * (page_count - 1) as f32).round() as usize + 1;
                assert_eq!(page_number, index + 1);
            }
        }
    }

    #[test]
    fn invalid_or_too_thin_tracks_have_no_ticks() {
        assert!(page_ruler_ticks(1, 1000.0, 8.0).is_empty());
        assert!(duration_ruler_ticks(0.0, 1000.0, 8.0).is_empty());
        assert!(page_ruler_ticks(10, 15.0, 8.0).is_empty());
        assert!(duration_ruler_ticks(60.0, 15.0, 8.0).is_empty());
    }

    #[test]
    fn short_video_ruler_is_sparse_instead_of_one_tick_per_second() {
        // 実機 FB 2026-08-12: 1:44 の動画が 1 秒刻み 105 本になり、大目盛りは 0 秒と 60 秒の
        // 2 本だけで、一様な櫛に見えていた。
        let ticks = duration_ruler_ticks(104.0, 1350.0, SEEK_RULER_TIME_MIN_SPACING);

        assert!(ticks.len() <= 24, "got {} ticks", ticks.len());
        assert!(ticks.iter().filter(|tick| tick.major).count() >= 3);
    }

    #[test]
    fn time_major_ticks_stay_within_a_readable_number_of_minor_steps() {
        for duration_secs in [30.0, 104.0, 600.0, 3600.0, 7200.0, 10.0 * 3600.0] {
            let ticks = duration_ruler_ticks(duration_secs, 1350.0, SEEK_RULER_TIME_MIN_SPACING);
            let majors = ticks
                .iter()
                .enumerate()
                .filter(|(_, tick)| tick.major)
                .map(|(index, _)| index)
                .collect::<Vec<_>>();

            assert!(!majors.is_empty(), "duration={duration_secs}");
            for pair in majors.windows(2) {
                assert!(
                    pair[1] - pair[0] <= 8,
                    "duration={duration_secs} gap={}",
                    pair[1] - pair[0]
                );
            }
        }
    }

    #[test]
    fn time_major_step_is_a_multiple_of_the_minor_step() {
        for &step in TIME_STEPS_SECS.iter() {
            let major = time_major_step(step);
            assert_eq!(major % step, 0, "step={step} major={major}");
            assert!(major >= step * 4, "step={step} major={major}");
        }
    }

    #[test]
    fn major_ticks_remain_members_of_the_minor_sequences() {
        for ticks in [
            page_ruler_ticks(100, 1000.0, 8.0),
            duration_ruler_ticks(10.0 * 60.0, 1000.0, 8.0),
        ] {
            let all = ticks
                .iter()
                .map(|tick| tick.fraction.to_bits())
                .collect::<std::collections::HashSet<_>>();
            let major = ticks.iter().filter(|tick| tick.major).collect::<Vec<_>>();

            assert!(!major.is_empty());
            assert!(
                major
                    .iter()
                    .all(|tick| all.contains(&tick.fraction.to_bits()))
            );
        }
    }
}

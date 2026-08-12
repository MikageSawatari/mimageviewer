//! フルスクリーン系シークバーの目盛り位置を生成する純ロジック。
//!
//! 呼び出し側は返された論理 fraction を各シークバー固有の方向規約で x 座標へ変換し、
//! 描画だけを担当する。ここではページ番号と時間の「切りの良い」刻み選択を一元化する。

/// 目盛り同士に確保する最小の論理間隔。
pub(crate) const SEEK_RULER_MIN_SPACING: f32 = 8.0;
/// トラック下端から目盛りまでの空き。
pub(crate) const SEEK_RULER_GAP: f32 = 2.0;
pub(crate) const SEEK_RULER_MINOR_HEIGHT: f32 = 3.0;
pub(crate) const SEEK_RULER_MAJOR_HEIGHT: f32 = 5.0;
pub(crate) const SEEK_RULER_STROKE_WIDTH: f32 = 1.0;
/// 3 面で共通に使う無彩色。トラックやつまみより弱く見せる。
pub(crate) const SEEK_RULER_GRAY: u8 = 112;

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

fn time_major_step(step_secs: u64) -> u64 {
    if step_secs < 60 {
        60
    } else if step_secs < 60 * 60 {
        60 * 60
    } else {
        24 * 60 * 60
    }
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

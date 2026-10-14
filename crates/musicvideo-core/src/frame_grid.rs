//! The single output-frame lattice shared by every music-video entry point.
pub use music_core::PcmView;
use std::fmt;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameGrid {
    num: u32,
    den: u32,
    frames: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct FrameGridError;
impl fmt::Display for FrameGridError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("invalid output frame rate, duration, or sample count")
    }
}
impl std::error::Error for FrameGridError {}

impl FrameGrid {
    pub fn new(fps: f64, duration: f64) -> Result<Self, FrameGridError> {
        const SCALE: u64 = 1_000_000;
        let scaled = (fps * SCALE as f64).round();
        let frames = (duration * fps).round();
        if !fps.is_finite()
            || fps <= 0.0
            || !duration.is_finite()
            || duration <= 0.0
            || !scaled.is_finite()
            || scaled < 1.0
            || scaled >= u64::MAX as f64
            || !frames.is_finite()
            || frames < 1.0
            || frames >= i64::MAX as f64
        {
            return Err(FrameGridError);
        }
        let mut a = scaled as u64;
        let mut b = SCALE;
        while b != 0 {
            (a, b) = (b, a % b);
        }
        let num = (scaled as u64) / a;
        let den = SCALE / a;
        if num > i32::MAX as u64 || den > i32::MAX as u64 {
            return Err(FrameGridError);
        }
        Ok(Self {
            num: num as u32,
            den: den as u32,
            frames: frames as u64,
        })
    }
    pub fn frames(self) -> u64 {
        self.frames
    }
    pub fn numerator(self) -> u32 {
        self.num
    }
    pub fn denominator(self) -> u32 {
        self.den
    }
    pub fn time(self, n: u64) -> f64 {
        n as f64 * self.den as f64 / self.num as f64
    }
    pub fn index_at(self, position: f64) -> u64 {
        if position.is_nan() || position <= 0.0 {
            return 0;
        }
        let mut n =
            ((position * self.num as f64 / self.den as f64).floor() as u64).min(self.frames - 1);
        if n > 0 && self.time(n) > position {
            n -= 1;
        }
        if n + 1 < self.frames && self.time(n + 1) <= position {
            n += 1;
        }
        n
    }
    pub(crate) fn checkpoint_interval(self) -> u64 {
        (self.num as u64).div_ceil(self.den as u64)
    }
}

/// Same rounded stereo length as AAC fitting, without copying or extending PCM.
pub fn pcm_view(
    samples: &[f32],
    sample_rate: u32,
    duration: f64,
) -> Result<PcmView<'_>, FrameGridError> {
    let frames = (duration * sample_rate as f64).round();
    if sample_rate == 0 || !frames.is_finite() || frames < 1.0 || frames >= (usize::MAX / 2) as f64
    {
        return Err(FrameGridError);
    }
    Ok(PcmView::new(samples, frames as usize))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn grid_boundaries_round_trip_without_epsilon() {
        for fps in [60.0, 59.94, 29.97, 0.5] {
            let grid = FrameGrid::new(fps, 300.0).unwrap();
            for n in 1..grid.frames() {
                let t = grid.time(n);
                assert_eq!(grid.index_at(t), n);
                assert_eq!(grid.index_at(f64::from_bits(t.to_bits() - 1)), n - 1);
                assert_eq!(grid.index_at(f64::from_bits(t.to_bits() + 1)), n);
            }
            assert_eq!(grid.index_at(f64::INFINITY), grid.frames() - 1);
            assert_eq!(grid.index_at(-1.0), 0);
        }
    }
    #[test]
    fn grid_uses_driver_round_and_rational_rules() {
        assert_eq!(FrameGrid::new(60.0, 0.025).unwrap().frames(), 2);
        assert_eq!(FrameGrid::new(60.0, 0.024).unwrap().frames(), 1);
        assert!(FrameGrid::new(60.0, 0.008).is_err());
        let grid = FrameGrid::new(59.94, 1.0).unwrap();
        assert_eq!((grid.numerator(), grid.denominator()), (2997, 50));
        for (fps, duration) in [(f64::NAN, 1.0), (0.0, 1.0), (60.0, -1.0), (1e20, 1.0)] {
            assert!(FrameGrid::new(fps, duration).is_err());
        }
    }
}

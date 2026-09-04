use crate::model::{Animatable, Fill, ImageId, Interp, NormRect, Rgba8, Track};

/// Values that can be sampled from an animation track.
///
/// Discrete values set `CONTINUOUS` to false, which gives them step semantics
/// regardless of the interpolation stored on a keyframe.
pub trait Interpolate: Clone {
    const CONTINUOUS: bool = true;

    fn interpolate(from: &Self, to: &Self, amount: f32) -> Self;
}

impl<T: Interpolate> Animatable<T> {
    pub fn evaluate(&self, t_secs: f64) -> T {
        match self {
            Animatable::Const(value) => value.clone(),
            Animatable::Track(track) => evaluate_track(track, t_secs),
        }
    }
}

pub fn evaluate_track<T: Interpolate>(track: &Track<T>, t_secs: f64) -> T {
    let keys = track.keys();
    let t = if t_secs.is_finite() { t_secs } else { 0.0 };

    if t < keys[0].t {
        return keys[0].value.clone();
    }

    // `partition_point` selects the last duplicate at exactly `t`, so later
    // duplicate keys win without requiring a special equality branch.
    let next = keys.partition_point(|key| key.t <= t);
    let current_index = next.saturating_sub(1);
    let current = &keys[current_index];
    let Some(following) = keys.get(next) else {
        return current.value.clone();
    };

    if !T::CONTINUOUS || current.interp == Interp::Step {
        return current.value.clone();
    }

    let span = following.t - current.t;
    if span <= 0.0 {
        return following.value.clone();
    }
    let u = ((t - current.t) / span).clamp(0.0, 1.0) as f32;
    T::interpolate(
        &current.value,
        &following.value,
        easing_amount(current.interp, u),
    )
}

pub fn easing_amount(interp: Interp, u: f32) -> f32 {
    let u = u.clamp(0.0, 1.0);
    match interp {
        Interp::Step => {
            if u >= 1.0 {
                1.0
            } else {
                0.0
            }
        }
        Interp::Linear => u,
        Interp::EaseIn => u * u,
        Interp::EaseOut => 1.0 - (1.0 - u) * (1.0 - u),
        Interp::EaseInOut => u * u * (3.0 - 2.0 * u),
    }
}

impl Interpolate for f32 {
    fn interpolate(from: &Self, to: &Self, amount: f32) -> Self {
        from + (to - from) * amount
    }
}

impl Interpolate for bool {
    const CONTINUOUS: bool = false;

    fn interpolate(from: &Self, _to: &Self, _amount: f32) -> Self {
        *from
    }
}

impl Interpolate for ImageId {
    const CONTINUOUS: bool = false;

    fn interpolate(from: &Self, _to: &Self, _amount: f32) -> Self {
        from.clone()
    }
}

impl Interpolate for NormRect {
    fn interpolate(from: &Self, to: &Self, amount: f32) -> Self {
        Self {
            x: f32::interpolate(&from.x, &to.x, amount),
            y: f32::interpolate(&from.y, &to.y, amount),
            width: f32::interpolate(&from.width, &to.width, amount),
            height: f32::interpolate(&from.height, &to.height, amount),
        }
    }
}

impl Interpolate for Rgba8 {
    fn interpolate(from: &Self, to: &Self, amount: f32) -> Self {
        let channel = |from: u8, to: u8| {
            (from as f32 + (to as f32 - from as f32) * amount)
                .round()
                .clamp(0.0, 255.0) as u8
        };
        Self {
            r: channel(from.r, to.r),
            g: channel(from.g, to.g),
            b: channel(from.b, to.b),
            a: channel(from.a, to.a),
        }
    }
}

impl Interpolate for Fill {
    fn interpolate(from: &Self, to: &Self, amount: f32) -> Self {
        match (from, to) {
            (Fill::Solid(from), Fill::Solid(to)) => {
                Fill::Solid(Rgba8::interpolate(from, to, amount))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{Keyframe, Track, TrackError};

    fn key(t: f64, value: f32, interp: Interp) -> Keyframe<f32> {
        Keyframe { t, value, interp }
    }

    #[test]
    fn one_key_and_outside_range_hold_endpoint_values() {
        let one = Track::new(vec![key(2.0, 7.0, Interp::Linear)]).unwrap();
        assert_eq!(evaluate_track(&one, -10.0), 7.0);
        assert_eq!(evaluate_track(&one, 20.0), 7.0);

        let multiple = Track::new(vec![
            key(1.0, 10.0, Interp::Linear),
            key(3.0, 30.0, Interp::Linear),
        ])
        .unwrap();
        assert_eq!(evaluate_track(&multiple, 0.0), 10.0);
        assert_eq!(evaluate_track(&multiple, 4.0), 30.0);
    }

    #[test]
    fn stable_sort_and_duplicate_time_make_later_key_win() {
        let track = Track::new(vec![
            key(2.0, 20.0, Interp::Linear),
            key(1.0, 10.0, Interp::Linear),
            key(1.0, 15.0, Interp::Linear),
        ])
        .unwrap();
        assert_eq!(track.keys()[0].value, 10.0);
        assert_eq!(track.keys()[1].value, 15.0);
        assert_eq!(evaluate_track(&track, 1.0), 15.0);
        assert_eq!(evaluate_track(&track, 1.5), 17.5);
    }

    #[test]
    fn empty_and_non_finite_tracks_are_rejected() {
        assert_eq!(Track::<f32>::new(Vec::new()), Err(TrackError::Empty));
        assert_eq!(
            Track::new(vec![key(f64::NAN, 0.0, Interp::Step)]),
            Err(TrackError::NonFiniteTime)
        );
    }

    #[test]
    fn step_and_linear_have_distinct_midpoints() {
        let step = Track::new(vec![
            key(0.0, 0.0, Interp::Step),
            key(1.0, 10.0, Interp::Step),
        ])
        .unwrap();
        let linear = Track::new(vec![
            key(0.0, 0.0, Interp::Linear),
            key(1.0, 10.0, Interp::Step),
        ])
        .unwrap();
        assert_eq!(evaluate_track(&step, 0.5), 0.0);
        assert_eq!(evaluate_track(&linear, 0.5), 5.0);
    }

    #[test]
    fn bool_is_always_step_even_when_key_says_linear() {
        let track = Track::new(vec![
            Keyframe {
                t: 0.0,
                value: false,
                interp: Interp::Linear,
            },
            Keyframe {
                t: 1.0,
                value: true,
                interp: Interp::Linear,
            },
        ])
        .unwrap();
        assert!(!evaluate_track(&track, 0.999));
        assert!(evaluate_track(&track, 1.0));
    }

    #[test]
    fn image_id_is_always_step_even_when_key_says_eased() {
        let track = Track::new(vec![
            Keyframe {
                t: 0.0,
                value: ImageId::new("first"),
                interp: Interp::EaseInOut,
            },
            Keyframe {
                t: 1.0,
                value: ImageId::new("second"),
                interp: Interp::EaseInOut,
            },
        ])
        .unwrap();
        assert_eq!(evaluate_track(&track, 0.999), ImageId::new("first"));
        assert_eq!(evaluate_track(&track, 1.0), ImageId::new("second"));
    }

    #[test]
    fn all_easing_functions_have_exact_endpoints() {
        for interp in [
            Interp::Step,
            Interp::Linear,
            Interp::EaseIn,
            Interp::EaseOut,
            Interp::EaseInOut,
        ] {
            assert_eq!(easing_amount(interp, 0.0), 0.0, "{interp:?} at zero");
            assert_eq!(easing_amount(interp, 1.0), 1.0, "{interp:?} at one");
        }
    }
}

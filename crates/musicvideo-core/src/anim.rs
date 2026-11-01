use crate::model::{
    Animatable, Fill, Glow, GradientStop, GradientStops, ImageId, Interp, NormRect, Rgba8, Track,
};

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

impl Interpolate for Glow {
    fn interpolate(from: &Self, to: &Self, amount: f32) -> Self {
        Self {
            color: Rgba8::interpolate(&from.color, &to.color, amount),
            radius_px: f32::interpolate(&from.radius_px, &to.radius_px, amount),
            spread_px: f32::interpolate(&from.spread_px, &to.spread_px, amount),
        }
    }
}

impl<T: Interpolate> Interpolate for Option<T> {
    fn interpolate(from: &Self, to: &Self, amount: f32) -> Self {
        match (from, to) {
            (Some(from), Some(to)) => Some(T::interpolate(from, to, amount)),
            _ => from.clone(),
        }
    }
}

impl Interpolate for Fill {
    fn interpolate(from: &Self, to: &Self, amount: f32) -> Self {
        match (from, to) {
            (Fill::Solid(from), Fill::Solid(to)) => {
                Fill::Solid(Rgba8::interpolate(from, to, amount))
            }
            (
                Fill::LinearGradient {
                    stops: from_stops,
                    angle_deg: from_angle,
                },
                Fill::LinearGradient {
                    stops: to_stops,
                    angle_deg: to_angle,
                },
            ) => interpolate_stops(from_stops, to_stops, amount)
                .map(|stops| Fill::LinearGradient {
                    stops,
                    angle_deg: f32::interpolate(from_angle, to_angle, amount),
                })
                .unwrap_or_else(|| from.clone()),
            (
                Fill::RadialGradient {
                    stops: from_stops,
                    center: from_center,
                    radius: from_radius,
                },
                Fill::RadialGradient {
                    stops: to_stops,
                    center: to_center,
                    radius: to_radius,
                },
            ) => interpolate_stops(from_stops, to_stops, amount)
                .map(|stops| Fill::RadialGradient {
                    stops,
                    center: (
                        f32::interpolate(&from_center.0, &to_center.0, amount),
                        f32::interpolate(&from_center.1, &to_center.1, amount),
                    ),
                    radius: f32::interpolate(from_radius, to_radius, amount),
                })
                .unwrap_or_else(|| from.clone()),
            (Fill::BandGradient { stops: from_stops }, Fill::BandGradient { stops: to_stops }) => {
                interpolate_stops(from_stops, to_stops, amount)
                    .map(|stops| Fill::BandGradient { stops })
                    .unwrap_or_else(|| from.clone())
            }
            _ => from.clone(),
        }
    }
}

fn interpolate_stops(
    from: &GradientStops,
    to: &GradientStops,
    amount: f32,
) -> Option<GradientStops> {
    if from.as_slice().len() != to.as_slice().len()
        || from
            .as_slice()
            .iter()
            .zip(to.as_slice())
            .any(|(from, to)| from.position != to.position)
    {
        return None;
    }
    GradientStops::new(
        from.as_slice()
            .iter()
            .zip(to.as_slice())
            .map(|(from, to)| GradientStop {
                position: from.position,
                color: Rgba8::interpolate(&from.color, &to.color, amount),
            })
            .collect(),
    )
    .ok()
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

    fn gradient_stops(position: f32, first: Rgba8, second: Rgba8) -> GradientStops {
        GradientStops::new(vec![
            GradientStop {
                position: 0.0,
                color: first,
            },
            GradientStop {
                position,
                color: second,
            },
        ])
        .unwrap()
    }

    #[test]
    fn fill_interpolates_colors_only_when_stop_layout_matches() {
        let from = Fill::LinearGradient {
            stops: gradient_stops(1.0, Rgba8::BLACK, Rgba8::new(100, 0, 0, 255)),
            angle_deg: 0.0,
        };
        let to = Fill::LinearGradient {
            stops: gradient_stops(1.0, Rgba8::WHITE, Rgba8::new(200, 0, 0, 255)),
            angle_deg: 90.0,
        };
        let Fill::LinearGradient { stops, angle_deg } = Fill::interpolate(&from, &to, 0.5) else {
            panic!("matching gradients must remain linear gradients");
        };
        assert_eq!(angle_deg, 45.0);
        assert_eq!(stops.as_slice()[0].color, Rgba8::new(128, 128, 128, 255));
        assert_eq!(stops.as_slice()[1].color, Rgba8::new(150, 0, 0, 255));

        let mismatched = Fill::LinearGradient {
            stops: gradient_stops(0.8, Rgba8::WHITE, Rgba8::WHITE),
            angle_deg: 90.0,
        };
        assert_eq!(Fill::interpolate(&from, &mismatched, 0.5), from);
    }

    #[test]
    fn optional_glow_interpolates_some_to_some_and_steps_across_none() {
        let from = Some(Glow {
            color: Rgba8::BLACK,
            radius_px: 4.0,
            spread_px: 0.0,
        });
        let to = Some(Glow {
            color: Rgba8::WHITE,
            radius_px: 12.0,
            spread_px: 2.0,
        });
        let middle = Option::<Glow>::interpolate(&from, &to, 0.5).unwrap();
        assert_eq!(middle.radius_px, 8.0);
        assert_eq!(middle.spread_px, 1.0);
        assert_eq!(middle.color, Rgba8::new(128, 128, 128, 255));
        assert_eq!(Option::<Glow>::interpolate(&from, &None, 0.9), from);
    }
}

impl Interpolate for crate::TextStyle {
    fn interpolate(from: &Self, to: &Self, amount: f32) -> Self {
        if from.outline.is_some() != to.outline.is_some()
            || from.extra_outlines.len() != to.extra_outlines.len()
            || from.shadow.is_some() != to.shadow.is_some()
            || from.echo.is_some() != to.echo.is_some()
            || from.background.is_some() != to.background.is_some()
            || from.echo.map(|e| e.count) != to.echo.map(|e| e.count)
        {
            return from.clone();
        }
        let scalar = |a: f32, b: f32| f32::interpolate(&a, &b, amount);
        let color = |a: crate::TextColor, b: crate::TextColor| {
            let c = crate::Rgba8::interpolate(
                &crate::Rgba8::new(a.r, a.g, a.b, a.a),
                &crate::Rgba8::new(b.r, b.g, b.b, b.a),
                amount,
            );
            crate::TextColor::new(c.r, c.g, c.b, c.a)
        };
        let stroke = |a: &crate::StrokeStyle, b: &crate::StrokeStyle| crate::StrokeStyle {
            color: color(a.color, b.color),
            width_px: scalar(a.width_px, b.width_px),
        };
        Self {
            outline: from.outline.zip(to.outline).map(|(a, b)| stroke(&a, &b)),
            extra_outlines: from
                .extra_outlines
                .iter()
                .zip(&to.extra_outlines)
                .map(|(a, b)| stroke(a, b))
                .collect(),
            shadow: from
                .shadow
                .zip(to.shadow)
                .map(|(a, b)| crate::TextShadowStyle {
                    color: color(a.color, b.color),
                    offset: (
                        scalar(a.offset.0, b.offset.0),
                        scalar(a.offset.1, b.offset.1),
                    ),
                    blur_px: scalar(a.blur_px, b.blur_px),
                    spread_px: scalar(a.spread_px, b.spread_px),
                }),
            echo: from.echo.zip(to.echo).map(|(a, b)| crate::TextEchoStyle {
                color: color(a.color, b.color),
                offset: (
                    scalar(a.offset.0, b.offset.0),
                    scalar(a.offset.1, b.offset.1),
                ),
                count: a.count,
            }),
            background: from.background.zip(to.background).map(|(a, b)| {
                crate::TextBackgroundStyle {
                    fill: color(a.fill, b.fill),
                    padding_px: scalar(a.padding_px, b.padding_px),
                    corner_px: scalar(a.corner_px, b.corner_px),
                }
            }),
            letter_gap: scalar(from.letter_gap, to.letter_gap),
            line_gap: scalar(from.line_gap, to.line_gap),
        }
    }
}

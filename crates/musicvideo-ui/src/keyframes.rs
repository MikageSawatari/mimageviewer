use std::fmt;

use musicvideo_core::{
    Animatable, Fill, Glow, Interp, Interpolate, Keyframe, Layer, LayerId, LayerKind, NormRect,
    Project, Track,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnimProperty {
    Visible,
    Opacity,
    Rect,
    Rotation,
    CornerRadius,
    Glow,
    SolidFill,
    SpectrumFill,
    KeyboardBackground,
    KeyboardFill,
    ProgressFill,
    ProgressBorderFill,
    TimeTextFill,
}

impl AnimProperty {
    pub fn label(self) -> &'static str {
        match self {
            Self::Visible => "表示",
            Self::Opacity => "不透明度",
            Self::Rect => "位置・サイズ",
            Self::Rotation => "回転",
            Self::CornerRadius => "角丸",
            Self::Glow => "グロー",
            Self::SolidFill => "塗り",
            Self::SpectrumFill => "スペクトラムの塗り",
            Self::KeyboardBackground => "鍵盤の背景",
            Self::KeyboardFill => "点灯する鍵の塗り",
            Self::ProgressFill => "進捗の塗り",
            Self::ProgressBorderFill => "枠の塗り",
            Self::TimeTextFill => "文字の塗り",
        }
    }

    pub fn is_continuous(self) -> bool {
        self != Self::Visible
    }

    fn default_interp(self) -> Interp {
        if self.is_continuous() {
            Interp::Linear
        } else {
            Interp::Step
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum AnimationValue {
    Bool(bool),
    Float(f32),
    Rect(NormRect),
    Glow(Option<Glow>),
    Fill(Fill),
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct KeyMeta {
    pub t: f64,
    pub interp: Interp,
}

#[derive(Debug, Clone, PartialEq)]
pub enum EditorCommand {
    AddKey {
        layer: LayerId,
        property: AnimProperty,
        t: f64,
    },
    SetValue {
        layer: LayerId,
        property: AnimProperty,
        t: f64,
        key_index: Option<usize>,
        value: AnimationValue,
    },
    MoveKey {
        layer: LayerId,
        property: AnimProperty,
        key_index: usize,
        t: f64,
    },
    DeleteKey {
        layer: LayerId,
        property: AnimProperty,
        key_index: usize,
    },
    SetInterpolation {
        layer: LayerId,
        property: AnimProperty,
        key_index: usize,
        interp: Interp,
    },
    CollapseToConst {
        layer: LayerId,
        property: AnimProperty,
        t: f64,
    },
}

impl EditorCommand {
    pub fn target(&self) -> (LayerId, AnimProperty) {
        match *self {
            Self::AddKey {
                layer, property, ..
            }
            | Self::SetValue {
                layer, property, ..
            }
            | Self::MoveKey {
                layer, property, ..
            }
            | Self::DeleteKey {
                layer, property, ..
            }
            | Self::SetInterpolation {
                layer, property, ..
            }
            | Self::CollapseToConst {
                layer, property, ..
            } => (layer, property),
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct CommandResult {
    pub changed: bool,
    pub selected_key: Option<usize>,
    pub clear_selection: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorCommandError {
    MissingLayer,
    PropertyDoesNotBelongToLayer,
    ValueTypeMismatch,
    InvalidTime,
    MissingKey,
}

impl fmt::Display for EditorCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::MissingLayer => f.write_str("編集対象のレイヤーがありません"),
            Self::PropertyDoesNotBelongToLayer => {
                f.write_str("編集対象のプロパティがレイヤーにありません")
            }
            Self::ValueTypeMismatch => f.write_str("プロパティ値の型が一致しません"),
            Self::InvalidTime => f.write_str("キーフレーム時刻が不正です"),
            Self::MissingKey => f.write_str("編集対象のキーフレームがありません"),
        }
    }
}

impl std::error::Error for EditorCommandError {}

trait AnimationValueType: Interpolate + Clone {
    fn from_animation_value(value: &AnimationValue) -> Option<Self>;
    fn into_animation_value(self) -> AnimationValue;
}

macro_rules! value_type {
    ($ty:ty, $variant:ident) => {
        impl AnimationValueType for $ty {
            fn from_animation_value(value: &AnimationValue) -> Option<Self> {
                match value {
                    AnimationValue::$variant(value) => Some(value.clone()),
                    _ => None,
                }
            }

            fn into_animation_value(self) -> AnimationValue {
                AnimationValue::$variant(self)
            }
        }
    };
}

value_type!(bool, Bool);
value_type!(f32, Float);
value_type!(NormRect, Rect);
value_type!(Option<Glow>, Glow);
value_type!(Fill, Fill);

pub fn animation_properties(layer: &Layer) -> Vec<AnimProperty> {
    let mut properties = vec![
        AnimProperty::Visible,
        AnimProperty::Opacity,
        AnimProperty::Rect,
        AnimProperty::Rotation,
        AnimProperty::CornerRadius,
        AnimProperty::Glow,
    ];
    match &layer.kind {
        LayerKind::Solid { .. } => properties.push(AnimProperty::SolidFill),
        LayerKind::Image { .. } => {}
        LayerKind::Spectrum(_) => properties.push(AnimProperty::SpectrumFill),
        LayerKind::PitchKeyboard(_) => {
            properties.push(AnimProperty::KeyboardBackground);
            properties.push(AnimProperty::KeyboardFill);
        }
        LayerKind::ProgressBar(_) => {
            properties.push(AnimProperty::ProgressFill);
            properties.push(AnimProperty::ProgressBorderFill);
        }
        LayerKind::TimeText(_) => properties.push(AnimProperty::TimeTextFill),
    }
    properties
}

pub fn animation_keys(layer: &Layer, property: AnimProperty) -> Option<Vec<KeyMeta>> {
    macro_rules! keys {
        ($animation:expr) => {
            Some(key_meta($animation))
        };
    }
    match property {
        AnimProperty::Visible => keys!(&layer.visible),
        AnimProperty::Opacity => keys!(&layer.opacity),
        AnimProperty::Rect => keys!(&layer.rect),
        AnimProperty::Rotation => keys!(&layer.rotation),
        AnimProperty::CornerRadius => keys!(&layer.corner_radius),
        AnimProperty::Glow => keys!(&layer.glow),
        AnimProperty::SolidFill => match &layer.kind {
            LayerKind::Solid { fill } => keys!(fill),
            _ => None,
        },
        AnimProperty::SpectrumFill => match &layer.kind {
            LayerKind::Spectrum(spectrum) => keys!(&spectrum.fill),
            _ => None,
        },
        AnimProperty::KeyboardBackground => match &layer.kind {
            LayerKind::PitchKeyboard(keyboard) => keys!(&keyboard.background),
            _ => None,
        },
        AnimProperty::KeyboardFill => match &layer.kind {
            LayerKind::PitchKeyboard(keyboard) => keys!(&keyboard.fill),
            _ => None,
        },
        AnimProperty::ProgressFill => match &layer.kind {
            LayerKind::ProgressBar(progress) => keys!(&progress.fill),
            _ => None,
        },
        AnimProperty::ProgressBorderFill => match &layer.kind {
            LayerKind::ProgressBar(progress) => keys!(&progress.border_fill),
            _ => None,
        },
        AnimProperty::TimeTextFill => match &layer.kind {
            LayerKind::TimeText(text) => keys!(&text.fill),
            _ => None,
        },
    }
}

pub fn animation_value(
    layer: &Layer,
    property: AnimProperty,
    t: f64,
    key_index: Option<usize>,
) -> Option<AnimationValue> {
    macro_rules! value {
        ($animation:expr) => {
            Some(value_at($animation, t, key_index).into_animation_value())
        };
    }
    match property {
        AnimProperty::Visible => value!(&layer.visible),
        AnimProperty::Opacity => value!(&layer.opacity),
        AnimProperty::Rect => value!(&layer.rect),
        AnimProperty::Rotation => value!(&layer.rotation),
        AnimProperty::CornerRadius => value!(&layer.corner_radius),
        AnimProperty::Glow => value!(&layer.glow),
        AnimProperty::SolidFill => match &layer.kind {
            LayerKind::Solid { fill } => value!(fill),
            _ => None,
        },
        AnimProperty::SpectrumFill => match &layer.kind {
            LayerKind::Spectrum(spectrum) => value!(&spectrum.fill),
            _ => None,
        },
        AnimProperty::KeyboardBackground => match &layer.kind {
            LayerKind::PitchKeyboard(keyboard) => value!(&keyboard.background),
            _ => None,
        },
        AnimProperty::KeyboardFill => match &layer.kind {
            LayerKind::PitchKeyboard(keyboard) => value!(&keyboard.fill),
            _ => None,
        },
        AnimProperty::ProgressFill => match &layer.kind {
            LayerKind::ProgressBar(progress) => value!(&progress.fill),
            _ => None,
        },
        AnimProperty::ProgressBorderFill => match &layer.kind {
            LayerKind::ProgressBar(progress) => value!(&progress.border_fill),
            _ => None,
        },
        AnimProperty::TimeTextFill => match &layer.kind {
            LayerKind::TimeText(text) => value!(&text.fill),
            _ => None,
        },
    }
}

pub fn apply_editor_command(
    project: &mut Project,
    command: &EditorCommand,
) -> Result<CommandResult, EditorCommandError> {
    let (layer_id, property) = command.target();
    let duration_secs = project.duration_secs.max(0.0);
    let layer = project
        .layers
        .iter_mut()
        .find(|layer| layer.id == layer_id)
        .ok_or(EditorCommandError::MissingLayer)?;
    macro_rules! apply {
        ($animation:expr) => {
            apply_typed($animation, property, command, duration_secs)
        };
    }
    match property {
        AnimProperty::Visible => apply!(&mut layer.visible),
        AnimProperty::Opacity => apply!(&mut layer.opacity),
        AnimProperty::Rect => apply!(&mut layer.rect),
        AnimProperty::Rotation => apply!(&mut layer.rotation),
        AnimProperty::CornerRadius => apply!(&mut layer.corner_radius),
        AnimProperty::Glow => apply!(&mut layer.glow),
        AnimProperty::SolidFill => match &mut layer.kind {
            LayerKind::Solid { fill } => apply!(fill),
            _ => Err(EditorCommandError::PropertyDoesNotBelongToLayer),
        },
        AnimProperty::SpectrumFill => match &mut layer.kind {
            LayerKind::Spectrum(spectrum) => apply!(&mut spectrum.fill),
            _ => Err(EditorCommandError::PropertyDoesNotBelongToLayer),
        },
        AnimProperty::KeyboardBackground => match &mut layer.kind {
            LayerKind::PitchKeyboard(keyboard) => apply!(&mut keyboard.background),
            _ => Err(EditorCommandError::PropertyDoesNotBelongToLayer),
        },
        AnimProperty::KeyboardFill => match &mut layer.kind {
            LayerKind::PitchKeyboard(keyboard) => apply!(&mut keyboard.fill),
            _ => Err(EditorCommandError::PropertyDoesNotBelongToLayer),
        },
        AnimProperty::ProgressFill => match &mut layer.kind {
            LayerKind::ProgressBar(progress) => apply!(&mut progress.fill),
            _ => Err(EditorCommandError::PropertyDoesNotBelongToLayer),
        },
        AnimProperty::ProgressBorderFill => match &mut layer.kind {
            LayerKind::ProgressBar(progress) => apply!(&mut progress.border_fill),
            _ => Err(EditorCommandError::PropertyDoesNotBelongToLayer),
        },
        AnimProperty::TimeTextFill => match &mut layer.kind {
            LayerKind::TimeText(text) => apply!(&mut text.fill),
            _ => Err(EditorCommandError::PropertyDoesNotBelongToLayer),
        },
    }
}

fn key_meta<T>(animation: &Animatable<T>) -> Vec<KeyMeta> {
    match animation {
        Animatable::Const(_) => Vec::new(),
        Animatable::Track(track) => track
            .keys()
            .iter()
            .map(|key| KeyMeta {
                t: key.t,
                interp: key.interp,
            })
            .collect(),
    }
}

fn value_at<T: AnimationValueType>(
    animation: &Animatable<T>,
    t: f64,
    key_index: Option<usize>,
) -> T {
    if let (Animatable::Track(track), Some(index)) = (animation, key_index)
        && let Some(key) = track.keys().get(index)
    {
        return key.value.clone();
    }
    animation.evaluate(t)
}

fn apply_typed<T: AnimationValueType>(
    animation: &mut Animatable<T>,
    property: AnimProperty,
    command: &EditorCommand,
    duration_secs: f64,
) -> Result<CommandResult, EditorCommandError> {
    match command {
        EditorCommand::AddKey { t, .. } => {
            let t = checked_time(*t, duration_secs)?;
            let value = animation.evaluate(t);
            let interp = property.default_interp();
            let selected_key = match animation {
                Animatable::Const(_) => {
                    *animation = Animatable::Track(
                        Track::new(vec![Keyframe { t, value, interp }])
                            .expect("validated UI key time and non-empty track"),
                    );
                    0
                }
                Animatable::Track(track) => {
                    let mut keys = track.keys().to_vec();
                    let index = keys.partition_point(|key| key.t <= t);
                    keys.insert(index, Keyframe { t, value, interp });
                    *track = Track::new(keys).expect("validated UI key time and non-empty track");
                    index
                }
            };
            Ok(CommandResult {
                changed: true,
                selected_key: Some(selected_key),
                clear_selection: false,
            })
        }
        EditorCommand::SetValue {
            t,
            key_index,
            value,
            ..
        } => {
            let value =
                T::from_animation_value(value).ok_or(EditorCommandError::ValueTypeMismatch)?;
            match animation {
                Animatable::Const(current) => {
                    *current = value;
                    Ok(CommandResult {
                        changed: true,
                        ..CommandResult::default()
                    })
                }
                Animatable::Track(track) => {
                    let mut keys = track.keys().to_vec();
                    let selected_key = if let Some(index) = key_index {
                        let key = keys.get_mut(*index).ok_or(EditorCommandError::MissingKey)?;
                        key.value = value;
                        *index
                    } else {
                        let t = checked_time(*t, duration_secs)?;
                        if let Some(index) = keys.iter().rposition(|key| key.t == t) {
                            keys[index].value = value;
                            index
                        } else {
                            let index = keys.partition_point(|key| key.t <= t);
                            keys.insert(
                                index,
                                Keyframe {
                                    t,
                                    value,
                                    interp: property.default_interp(),
                                },
                            );
                            index
                        }
                    };
                    *track = Track::new(keys).expect("value edits retain a non-empty valid track");
                    Ok(CommandResult {
                        changed: true,
                        selected_key: Some(selected_key),
                        clear_selection: false,
                    })
                }
            }
        }
        EditorCommand::MoveKey { key_index, t, .. } => {
            let t = checked_time(*t, duration_secs)?;
            let Animatable::Track(track) = animation else {
                return Err(EditorCommandError::MissingKey);
            };
            let mut keys = track.keys().to_vec();
            if *key_index >= keys.len() {
                return Err(EditorCommandError::MissingKey);
            }
            let mut key = keys.remove(*key_index);
            key.t = t;
            let selected_key = keys.partition_point(|existing| existing.t <= t);
            keys.insert(selected_key, key);
            *track = Track::new(keys).expect("move retains a non-empty valid track");
            Ok(CommandResult {
                changed: true,
                selected_key: Some(selected_key),
                clear_selection: false,
            })
        }
        EditorCommand::DeleteKey { key_index, .. } => {
            let Animatable::Track(track) = animation else {
                return Err(EditorCommandError::MissingKey);
            };
            let mut keys = track.keys().to_vec();
            if *key_index >= keys.len() {
                return Err(EditorCommandError::MissingKey);
            }
            let removed = keys.remove(*key_index);
            if keys.is_empty() {
                *animation = Animatable::Const(removed.value);
            } else {
                *track = Track::new(keys).expect("delete retains a non-empty valid track");
            }
            Ok(CommandResult {
                changed: true,
                selected_key: None,
                clear_selection: true,
            })
        }
        EditorCommand::SetInterpolation {
            key_index, interp, ..
        } => {
            let Animatable::Track(track) = animation else {
                return Err(EditorCommandError::MissingKey);
            };
            let mut keys = track.keys().to_vec();
            let key = keys
                .get_mut(*key_index)
                .ok_or(EditorCommandError::MissingKey)?;
            key.interp = if property.is_continuous() {
                *interp
            } else {
                Interp::Step
            };
            *track = Track::new(keys).expect("interpolation edit retains a valid track");
            Ok(CommandResult {
                changed: true,
                selected_key: Some(*key_index),
                clear_selection: false,
            })
        }
        EditorCommand::CollapseToConst { t, .. } => {
            let t = checked_time(*t, duration_secs)?;
            let value = animation.evaluate(t);
            *animation = Animatable::Const(value);
            Ok(CommandResult {
                changed: true,
                selected_key: None,
                clear_selection: true,
            })
        }
    }
}

fn checked_time(t: f64, duration_secs: f64) -> Result<f64, EditorCommandError> {
    if !t.is_finite() {
        return Err(EditorCommandError::InvalidTime);
    }
    Ok(t.clamp(0.0, duration_secs))
}

#[cfg(test)]
mod tests {
    use super::*;
    use musicvideo_core::{ImageId, Rgba8, default_project, new_image_layer};

    fn opacity_keys(project: &Project) -> Vec<Keyframe<f32>> {
        let Animatable::Track(track) = &project.layers[0].opacity else {
            panic!("opacity should be a track");
        };
        track.keys().to_vec()
    }

    #[test]
    fn moving_a_key_past_another_resorts_the_track() {
        let mut project = default_project(10.0);
        let layer = project.layers[0].id;
        project.layers[0].opacity = Animatable::Track(
            Track::new(vec![
                Keyframe {
                    t: 1.0,
                    value: 0.1,
                    interp: Interp::Linear,
                },
                Keyframe {
                    t: 2.0,
                    value: 0.2,
                    interp: Interp::EaseOut,
                },
            ])
            .unwrap(),
        );
        let result = apply_editor_command(
            &mut project,
            &EditorCommand::MoveKey {
                layer,
                property: AnimProperty::Opacity,
                key_index: 0,
                t: 3.0,
            },
        )
        .unwrap();
        let keys = opacity_keys(&project);
        assert_eq!(keys.iter().map(|key| key.t).collect::<Vec<_>>(), [2.0, 3.0]);
        assert_eq!(keys[1].value, 0.1);
        assert_eq!(result.selected_key, Some(1));
    }

    #[test]
    fn moving_to_a_duplicate_time_keeps_the_moved_key_last() {
        let mut project = default_project(10.0);
        let layer = project.layers[0].id;
        project.layers[0].opacity = Animatable::Track(
            Track::new(vec![
                Keyframe {
                    t: 1.0,
                    value: 0.1,
                    interp: Interp::Linear,
                },
                Keyframe {
                    t: 2.0,
                    value: 0.2,
                    interp: Interp::Linear,
                },
            ])
            .unwrap(),
        );
        apply_editor_command(
            &mut project,
            &EditorCommand::MoveKey {
                layer,
                property: AnimProperty::Opacity,
                key_index: 0,
                t: 2.0,
            },
        )
        .unwrap();
        let keys = opacity_keys(&project);
        assert_eq!(keys.iter().map(|key| key.t).collect::<Vec<_>>(), [2.0, 2.0]);
        assert_eq!(project.layers[0].opacity.evaluate(2.0), 0.1);
    }

    #[test]
    fn deleting_the_last_key_collapses_to_its_value() {
        let mut project = default_project(10.0);
        let layer = project.layers[0].id;
        project.layers[0].opacity = Animatable::Track(
            Track::new(vec![Keyframe {
                t: 4.0,
                value: 0.35,
                interp: Interp::EaseInOut,
            }])
            .unwrap(),
        );
        let result = apply_editor_command(
            &mut project,
            &EditorCommand::DeleteKey {
                layer,
                property: AnimProperty::Opacity,
                key_index: 0,
            },
        )
        .unwrap();
        assert_eq!(project.layers[0].opacity, Animatable::Const(0.35));
        assert!(result.clear_selection);
    }

    #[test]
    fn all_animatable_property_types_use_the_same_command_path() {
        let mut project = default_project(10.0);
        let layer = project.layers[2].id;
        for (property, value) in [
            (AnimProperty::Opacity, AnimationValue::Float(0.4)),
            (
                AnimProperty::Rect,
                AnimationValue::Rect(NormRect::new(0.1, 0.2, 0.3, 0.4)),
            ),
            (
                AnimProperty::ProgressFill,
                AnimationValue::Fill(Fill::Solid(Rgba8::new(1, 2, 3, 4))),
            ),
        ] {
            apply_editor_command(
                &mut project,
                &EditorCommand::SetValue {
                    layer,
                    property,
                    t: 2.0,
                    key_index: None,
                    value,
                },
            )
            .unwrap();
        }
        assert_eq!(project.layers[2].opacity.evaluate(2.0), 0.4);
        assert_eq!(
            project.layers[2].rect.evaluate(2.0),
            NormRect::new(0.1, 0.2, 0.3, 0.4)
        );
        let LayerKind::ProgressBar(progress) = &project.layers[2].kind else {
            unreachable!();
        };
        assert_eq!(
            progress.fill.evaluate(2.0),
            Fill::Solid(Rgba8::new(1, 2, 3, 4))
        );
    }

    #[test]
    fn every_layer_animatable_can_be_promoted_to_a_track() {
        let mut project = default_project(10.0);
        let mut solid = new_image_layer(&project, ImageId::new("solid-placeholder"));
        solid.kind = LayerKind::Solid {
            fill: Animatable::Const(Fill::Solid(Rgba8::new(10, 20, 30, 255))),
        };
        project.layers.push(solid);
        let keyboard = musicvideo_core::new_pitch_keyboard_layer(&project);
        project.layers.push(keyboard);
        let targets = project
            .layers
            .iter()
            .flat_map(|layer| {
                animation_properties(layer)
                    .into_iter()
                    .map(|property| (layer.id, property))
            })
            .collect::<Vec<_>>();

        for (layer, property) in targets {
            apply_editor_command(
                &mut project,
                &EditorCommand::AddKey {
                    layer,
                    property,
                    t: 2.0,
                },
            )
            .unwrap_or_else(|error| panic!("{property:?} could not become a track: {error}"));
            let layer = project
                .layers
                .iter()
                .find(|candidate| candidate.id == layer)
                .unwrap();
            assert_eq!(animation_keys(layer, property).unwrap().len(), 1);
        }
    }

    #[test]
    fn bool_keys_are_always_step_even_when_command_requests_linear() {
        let mut project = default_project(10.0);
        let layer = project.layers[0].id;
        apply_editor_command(
            &mut project,
            &EditorCommand::AddKey {
                layer,
                property: AnimProperty::Visible,
                t: 1.0,
            },
        )
        .unwrap();
        apply_editor_command(
            &mut project,
            &EditorCommand::SetInterpolation {
                layer,
                property: AnimProperty::Visible,
                key_index: 0,
                interp: Interp::Linear,
            },
        )
        .unwrap();
        let keys = animation_keys(&project.layers[0], AnimProperty::Visible).unwrap();
        assert_eq!(keys[0].interp, Interp::Step);
    }

    #[test]
    fn two_image_opacity_tracks_form_a_crossfade() {
        let mut project = default_project(10.0);
        project
            .layers
            .push(new_image_layer(&project, ImageId::new("image-2")));
        let first = project.layers[0].id;
        let second = project.layers.last().unwrap().id;

        for (layer, start, end) in [(first, 1.0, 0.0), (second, 0.0, 1.0)] {
            apply_editor_command(
                &mut project,
                &EditorCommand::AddKey {
                    layer,
                    property: AnimProperty::Opacity,
                    t: 4.0,
                },
            )
            .unwrap();
            apply_editor_command(
                &mut project,
                &EditorCommand::SetValue {
                    layer,
                    property: AnimProperty::Opacity,
                    t: 4.0,
                    key_index: Some(0),
                    value: AnimationValue::Float(start),
                },
            )
            .unwrap();
            apply_editor_command(
                &mut project,
                &EditorCommand::AddKey {
                    layer,
                    property: AnimProperty::Opacity,
                    t: 6.0,
                },
            )
            .unwrap();
            apply_editor_command(
                &mut project,
                &EditorCommand::SetValue {
                    layer,
                    property: AnimProperty::Opacity,
                    t: 6.0,
                    key_index: Some(1),
                    value: AnimationValue::Float(end),
                },
            )
            .unwrap();
        }

        assert_eq!(project.layers[0].opacity.evaluate(5.0), 0.5);
        assert_eq!(project.layers.last().unwrap().opacity.evaluate(5.0), 0.5);
    }
}

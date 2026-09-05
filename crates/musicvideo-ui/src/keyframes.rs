use std::fmt;

use musicvideo_core::{
    Animatable, Fill, Glow, Interp, Interpolate, Keyframe, Layer, LayerId, LayerKind, LyricLine,
    NormRect, Project, Track,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum AnimProperty {
    Visible,
    Opacity,
    Rect,
    Rotation,
    CornerRadius,
    Glow,
    ImageZoom,
    SolidFill,
    SpectrumFill,
    KeyboardBackground,
    KeyboardFill,
    ProgressFill,
    ProgressBorderFill,
    TimeTextFill,
    TextFill,
    LyricsFill,
}

impl AnimProperty {
    pub fn label(self) -> &'static str {
        match self {
            Self::Visible => "表示",
            Self::Opacity => "不透明度",
            Self::Rect => "レイヤー矩形の位置・サイズ",
            Self::Rotation => "回転",
            Self::CornerRadius => "角丸",
            Self::Glow => "グロー",
            Self::ImageZoom => "絵の拡大",
            Self::SolidFill => "塗り",
            Self::SpectrumFill => "スペクトラムの塗り",
            Self::KeyboardBackground => "鍵盤の背景",
            Self::KeyboardFill => "点灯する鍵の塗り",
            Self::ProgressFill => "進捗の塗り",
            Self::ProgressBorderFill => "枠の塗り",
            Self::TimeTextFill => "文字の塗り",
            Self::TextFill => "文字の塗り",
            Self::LyricsFill => "歌詞の塗り",
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
    DeleteLayer {
        layer: LayerId,
    },
    MoveLayer {
        layer: LayerId,
        direction: LayerMoveDirection,
    },
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
    ReplaceLyrics {
        layer: LayerId,
        lines: Vec<LyricLine>,
    },
    ReplaceLyricsText {
        layer: LayerId,
        text: String,
    },
    SetLyricTime {
        layer: LayerId,
        line_index: usize,
        time_secs: Option<f64>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LayerMoveDirection {
    TowardBack,
    TowardFront,
}

impl EditorCommand {
    pub fn layer(&self) -> LayerId {
        match *self {
            Self::DeleteLayer { layer }
            | Self::MoveLayer { layer, .. }
            | Self::AddKey { layer, .. }
            | Self::SetValue { layer, .. }
            | Self::MoveKey { layer, .. }
            | Self::DeleteKey { layer, .. }
            | Self::SetInterpolation { layer, .. }
            | Self::CollapseToConst { layer, .. } => layer,
            Self::ReplaceLyrics { layer, .. }
            | Self::ReplaceLyricsText { layer, .. }
            | Self::SetLyricTime { layer, .. } => layer,
        }
    }

    pub fn animation_target(&self) -> Option<(LayerId, AnimProperty)> {
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
            } => Some((layer, property)),
            Self::DeleteLayer { .. }
            | Self::MoveLayer { .. }
            | Self::ReplaceLyrics { .. }
            | Self::ReplaceLyricsText { .. }
            | Self::SetLyricTime { .. } => None,
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
    MissingLyricLine,
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
            Self::MissingLyricLine => f.write_str("編集対象の歌詞行がありません"),
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
        LayerKind::Image { .. } => properties.push(AnimProperty::ImageZoom),
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
        LayerKind::Text(_) => properties.push(AnimProperty::TextFill),
        LayerKind::Lyrics(_) => properties.push(AnimProperty::LyricsFill),
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
        AnimProperty::ImageZoom => match &layer.kind {
            LayerKind::Image { zoom, .. } => keys!(zoom),
            _ => None,
        },
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
        AnimProperty::TextFill => match &layer.kind {
            LayerKind::Text(text) => keys!(&text.fill),
            _ => None,
        },
        AnimProperty::LyricsFill => match &layer.kind {
            LayerKind::Lyrics(lyrics) => keys!(&lyrics.fill),
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
        AnimProperty::ImageZoom => match &layer.kind {
            LayerKind::Image { zoom, .. } => value!(zoom),
            _ => None,
        },
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
        AnimProperty::TextFill => match &layer.kind {
            LayerKind::Text(text) => value!(&text.fill),
            _ => None,
        },
        AnimProperty::LyricsFill => match &layer.kind {
            LayerKind::Lyrics(lyrics) => value!(&lyrics.fill),
            _ => None,
        },
    }
}

pub fn apply_editor_command(
    project: &mut Project,
    command: &EditorCommand,
) -> Result<CommandResult, EditorCommandError> {
    match *command {
        EditorCommand::DeleteLayer { layer } => return delete_layer(project, layer),
        EditorCommand::MoveLayer { layer, direction } => {
            return move_layer(project, layer, direction);
        }
        EditorCommand::ReplaceLyrics { layer, ref lines } => {
            return replace_lyrics(project, layer, lines.clone());
        }
        EditorCommand::ReplaceLyricsText { layer, ref text } => {
            return replace_lyrics_text(project, layer, text);
        }
        EditorCommand::SetLyricTime {
            layer,
            line_index,
            time_secs,
        } => return set_lyric_time(project, layer, line_index, time_secs),
        _ => {}
    }

    let Some((layer_id, property)) = command.animation_target() else {
        unreachable!();
    };
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
        AnimProperty::ImageZoom => match &mut layer.kind {
            LayerKind::Image { zoom, .. } => apply!(zoom),
            _ => Err(EditorCommandError::PropertyDoesNotBelongToLayer),
        },
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
        AnimProperty::TextFill => match &mut layer.kind {
            LayerKind::Text(text) => apply!(&mut text.fill),
            _ => Err(EditorCommandError::PropertyDoesNotBelongToLayer),
        },
        AnimProperty::LyricsFill => match &mut layer.kind {
            LayerKind::Lyrics(lyrics) => apply!(&mut lyrics.fill),
            _ => Err(EditorCommandError::PropertyDoesNotBelongToLayer),
        },
    }
}

fn replace_lyrics(
    project: &mut Project,
    layer_id: LayerId,
    lines: Vec<LyricLine>,
) -> Result<CommandResult, EditorCommandError> {
    let layer = project
        .layers
        .iter_mut()
        .find(|layer| layer.id == layer_id)
        .ok_or(EditorCommandError::MissingLayer)?;
    let LayerKind::Lyrics(lyrics) = &mut layer.kind else {
        return Err(EditorCommandError::PropertyDoesNotBelongToLayer);
    };
    let changed = lyrics.lines != lines;
    lyrics.lines = lines;
    Ok(CommandResult {
        changed,
        selected_key: None,
        clear_selection: true,
    })
}

fn replace_lyrics_text(
    project: &mut Project,
    layer_id: LayerId,
    text: &str,
) -> Result<CommandResult, EditorCommandError> {
    let layer = project
        .layers
        .iter_mut()
        .find(|layer| layer.id == layer_id)
        .ok_or(EditorCommandError::MissingLayer)?;
    let LayerKind::Lyrics(lyrics) = &mut layer.kind else {
        return Err(EditorCommandError::PropertyDoesNotBelongToLayer);
    };
    let texts = if text.is_empty() {
        Vec::new()
    } else {
        text.split('\n')
            .map(|line| line.strip_suffix('\r').unwrap_or(line).to_owned())
            .collect::<Vec<_>>()
    };
    let lines = texts
        .into_iter()
        .enumerate()
        .map(|(index, text)| LyricLine {
            text,
            time_secs: lyrics.lines.get(index).and_then(|line| line.time_secs),
        })
        .collect::<Vec<_>>();
    let changed = lyrics.lines != lines;
    lyrics.lines = lines;
    Ok(CommandResult {
        changed,
        selected_key: None,
        clear_selection: true,
    })
}

fn set_lyric_time(
    project: &mut Project,
    layer_id: LayerId,
    line_index: usize,
    time_secs: Option<f64>,
) -> Result<CommandResult, EditorCommandError> {
    let duration = project.duration_secs.max(0.0);
    let time_secs = match time_secs {
        Some(time) => Some(checked_time(time, duration)?),
        None => None,
    };
    let layer = project
        .layers
        .iter_mut()
        .find(|layer| layer.id == layer_id)
        .ok_or(EditorCommandError::MissingLayer)?;
    let LayerKind::Lyrics(lyrics) = &mut layer.kind else {
        return Err(EditorCommandError::PropertyDoesNotBelongToLayer);
    };
    let line = lyrics
        .lines
        .get_mut(line_index)
        .ok_or(EditorCommandError::MissingLyricLine)?;
    let changed = line.time_secs != time_secs;
    line.time_secs = time_secs;
    Ok(CommandResult {
        changed,
        selected_key: None,
        clear_selection: false,
    })
}

fn delete_layer(
    project: &mut Project,
    layer_id: LayerId,
) -> Result<CommandResult, EditorCommandError> {
    let index = project
        .layers
        .iter()
        .position(|layer| layer.id == layer_id)
        .ok_or(EditorCommandError::MissingLayer)?;
    let removed_image = match &project.layers[index].kind {
        LayerKind::Image { image, .. } => Some(image.clone()),
        _ => None,
    };
    project.layers.remove(index);

    if let Some(image) = removed_image {
        let still_referenced = project.layers.iter().any(
            |layer| matches!(&layer.kind, LayerKind::Image { image: candidate, .. } if candidate == &image),
        );
        if !still_referenced {
            project.images.retain(|reference| reference.id != image);
        }
    }

    Ok(CommandResult {
        changed: true,
        selected_key: None,
        clear_selection: true,
    })
}

fn move_layer(
    project: &mut Project,
    layer_id: LayerId,
    direction: LayerMoveDirection,
) -> Result<CommandResult, EditorCommandError> {
    let index = project
        .layers
        .iter()
        .position(|layer| layer.id == layer_id)
        .ok_or(EditorCommandError::MissingLayer)?;
    let destination = match direction {
        LayerMoveDirection::TowardBack if index > 0 => Some(index - 1),
        LayerMoveDirection::TowardFront if index + 1 < project.layers.len() => Some(index + 1),
        LayerMoveDirection::TowardBack | LayerMoveDirection::TowardFront => None,
    };
    let Some(destination) = destination else {
        return Ok(CommandResult::default());
    };
    project.layers.swap(index, destination);
    Ok(CommandResult {
        changed: true,
        selected_key: None,
        clear_selection: false,
    })
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
        EditorCommand::DeleteLayer { .. }
        | EditorCommand::MoveLayer { .. }
        | EditorCommand::ReplaceLyrics { .. }
        | EditorCommand::ReplaceLyricsText { .. }
        | EditorCommand::SetLyricTime { .. } => unreachable!(),
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
    use std::path::PathBuf;

    use musicvideo_core::{
        Assets, Frame, ImageId, ImageRef, ModulationBinding, ModulationCurve, ModulationTarget,
        ModulatorId, Rgba8, SpectrumFrame, default_project, evaluate, new_image_layer,
        new_lyrics_layer, new_solid_layer, render_frame,
    };

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
    fn lyric_text_and_times_are_changed_only_through_editor_commands() {
        let mut project = default_project(10.0);
        let lyrics = new_lyrics_layer(&project);
        let layer = lyrics.id;
        project.layers.push(lyrics);

        apply_editor_command(
            &mut project,
            &EditorCommand::ReplaceLyricsText {
                layer,
                text: "一行目\n\n三行目".to_owned(),
            },
        )
        .unwrap();
        apply_editor_command(
            &mut project,
            &EditorCommand::SetLyricTime {
                layer,
                line_index: 0,
                time_secs: Some(1.25),
            },
        )
        .unwrap();
        apply_editor_command(
            &mut project,
            &EditorCommand::SetLyricTime {
                layer,
                line_index: 1,
                time_secs: Some(2.5),
            },
        )
        .unwrap();

        let LayerKind::Lyrics(lyrics) = &project.layers.last().unwrap().kind else {
            unreachable!()
        };
        assert_eq!(lyrics.lines.len(), 3);
        assert_eq!(lyrics.lines[0].text, "一行目");
        assert_eq!(lyrics.lines[0].time_secs, Some(1.25));
        assert_eq!(lyrics.lines[1].text, "");
        assert_eq!(lyrics.lines[1].time_secs, Some(2.5));
        assert_eq!(lyrics.lines[2].time_secs, None);
    }

    #[test]
    fn replacing_lyric_text_preserves_times_by_line_and_trailing_empty_line() {
        let mut project = default_project(10.0);
        let lyrics = new_lyrics_layer(&project);
        let layer = lyrics.id;
        project.layers.push(lyrics);
        apply_editor_command(
            &mut project,
            &EditorCommand::ReplaceLyrics {
                layer,
                lines: vec![
                    LyricLine {
                        text: "旧一".into(),
                        time_secs: Some(1.0),
                    },
                    LyricLine {
                        text: "旧二".into(),
                        time_secs: Some(2.0),
                    },
                ],
            },
        )
        .unwrap();
        apply_editor_command(
            &mut project,
            &EditorCommand::ReplaceLyricsText {
                layer,
                text: "新一\n新二\n".to_owned(),
            },
        )
        .unwrap();
        let LayerKind::Lyrics(lyrics) = &project.layers.last().unwrap().kind else {
            unreachable!()
        };
        assert_eq!(
            lyrics
                .lines
                .iter()
                .map(|line| line.text.as_str())
                .collect::<Vec<_>>(),
            ["新一", "新二", ""]
        );
        assert_eq!(
            lyrics
                .lines
                .iter()
                .map(|line| line.time_secs)
                .collect::<Vec<_>>(),
            [Some(1.0), Some(2.0), None]
        );
    }

    #[test]
    fn deleting_one_of_two_layers_keeps_the_shared_image_reference() {
        let mut project = default_project(10.0);
        project.layers.clear();
        let image = ImageId::new("shared-image");
        project.images.push(ImageRef {
            id: image.clone(),
            path: PathBuf::from("media/shared.png"),
        });
        let mut first = new_image_layer(&project, image.clone());
        first.modulations.push(ModulationBinding {
            target: ModulationTarget::ImageZoom,
            modulator: ModulatorId(42),
            amount: 0.15,
            curve: ModulationCurve::Squared,
        });
        project.layers.push(first);
        project
            .layers
            .push(new_image_layer(&project, image.clone()));

        let deleted = project.layers[0].id;
        apply_editor_command(&mut project, &EditorCommand::DeleteLayer { layer: deleted }).unwrap();

        assert_eq!(project.layers.len(), 1);
        assert!(project.layers[0].modulations.is_empty());
        assert_eq!(project.images.len(), 1);
        assert_eq!(project.images[0].id, image);
    }

    #[test]
    fn deleting_the_last_image_layer_removes_its_image_reference() {
        let mut project = default_project(10.0);
        project.layers.clear();
        let image = ImageId::new("last-image");
        project.images.push(ImageRef {
            id: image.clone(),
            path: PathBuf::from("media/last.png"),
        });
        project.layers.push(new_image_layer(&project, image));

        let deleted = project.layers[0].id;
        apply_editor_command(&mut project, &EditorCommand::DeleteLayer { layer: deleted }).unwrap();

        assert!(project.layers.is_empty());
        assert!(project.images.is_empty());
    }

    #[test]
    fn moving_layers_matches_back_to_front_project_order() {
        let mut project = default_project(10.0);
        let original = project
            .layers
            .iter()
            .map(|layer| layer.id)
            .collect::<Vec<_>>();
        let moved = original[1];

        let toward_front = apply_editor_command(
            &mut project,
            &EditorCommand::MoveLayer {
                layer: moved,
                direction: LayerMoveDirection::TowardFront,
            },
        )
        .unwrap();
        assert!(toward_front.changed);
        assert_eq!(
            project
                .layers
                .iter()
                .map(|layer| layer.id)
                .collect::<Vec<_>>(),
            [original[0], original[2], original[1], original[3]]
        );

        apply_editor_command(
            &mut project,
            &EditorCommand::MoveLayer {
                layer: moved,
                direction: LayerMoveDirection::TowardBack,
            },
        )
        .unwrap();
        assert_eq!(
            project
                .layers
                .iter()
                .map(|layer| layer.id)
                .collect::<Vec<_>>(),
            original
        );
    }

    #[test]
    fn moving_a_layer_past_either_edge_is_a_noop() {
        let mut project = default_project(10.0);
        let before = project.clone();
        let backmost = project.layers.first().unwrap().id;
        let frontmost = project.layers.last().unwrap().id;

        let back_result = apply_editor_command(
            &mut project,
            &EditorCommand::MoveLayer {
                layer: backmost,
                direction: LayerMoveDirection::TowardBack,
            },
        )
        .unwrap();
        let front_result = apply_editor_command(
            &mut project,
            &EditorCommand::MoveLayer {
                layer: frontmost,
                direction: LayerMoveDirection::TowardFront,
            },
        )
        .unwrap();

        assert!(!back_result.changed);
        assert!(!front_result.changed);
        assert_eq!(project, before);
    }

    #[test]
    fn toward_front_changes_the_actual_overlap_in_the_same_direction_as_the_list() {
        let mut project = default_project(10.0);
        project.output.width = 1;
        project.output.height = 1;
        project.layers.clear();

        let mut red = new_solid_layer(&project);
        red.rect = Animatable::Const(NormRect::FULL);
        red.corner_radius = Animatable::Const(0.0);
        red.kind = LayerKind::Solid {
            fill: Animatable::Const(Fill::Solid(Rgba8::new(255, 0, 0, 255))),
        };
        let red_id = red.id;
        project.layers.push(red);

        let mut blue = new_solid_layer(&project);
        blue.rect = Animatable::Const(NormRect::FULL);
        blue.corner_radius = Animatable::Const(0.0);
        blue.kind = LayerKind::Solid {
            fill: Animatable::Const(Fill::Solid(Rgba8::new(0, 0, 255, 255))),
        };
        project.layers.push(blue);

        let assets = Assets::default();
        let spectrum = SpectrumFrame::default();
        let mut frame = Frame::new(1, 1).unwrap();
        let report =
            render_frame(&evaluate(&project, 0.0, &spectrum), &assets, &mut frame).unwrap();
        assert!(report.unresolved_assets.is_empty());
        assert_eq!(frame.rgba, [0, 0, 255, 255]);

        apply_editor_command(
            &mut project,
            &EditorCommand::MoveLayer {
                layer: red_id,
                direction: LayerMoveDirection::TowardFront,
            },
        )
        .unwrap();
        let report =
            render_frame(&evaluate(&project, 0.0, &spectrum), &assets, &mut frame).unwrap();
        assert!(report.unresolved_assets.is_empty());
        assert_eq!(frame.rgba, [255, 0, 0, 255]);
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

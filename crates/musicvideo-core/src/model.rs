use std::fmt;

use serde::{Deserialize, Deserializer, Serialize, de};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LayerId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImageId(pub String);

impl ImageId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    pub output: Output,
    pub duration_secs: f64,
    /// Drawing order. The first layer is the backmost layer.
    pub layers: Vec<Layer>,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Output {
    pub width: u32,
    pub height: u32,
    pub fps: f64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Layer {
    pub id: LayerId,
    pub name: String,
    pub visible: Animatable<bool>,
    pub opacity: Animatable<f32>,
    pub rect: Animatable<NormRect>,
    pub kind: LayerKind,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LayerKind {
    Solid { fill: Animatable<Fill> },
    Image { image: ImageId, fit: Fit },
    Spectrum(SpectrumLayer),
    ProgressBar(ProgressBarLayer),
    TimeText(TimeTextLayer),
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpectrumLayer {
    pub band_count: usize,
    pub gap_ratio: f32,
    pub min_height_px: f32,
    pub thickness: f32,
    pub fill: Animatable<Fill>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressBarLayer {
    pub border_thickness_px: f32,
    pub fill: Animatable<Fill>,
    pub border_fill: Animatable<Fill>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeTextLayer {
    pub format: TimeFormat,
    pub font_key: String,
    pub size_px: f32,
    pub align: TextAlign,
    pub fill: Animatable<Fill>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TimeFormat {
    MmSs,
    MmSsOfTotal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum TextAlign {
    Start,
    Center,
    End,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fit {
    Stretch,
    Contain,
    Cover,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Fill {
    Solid(Rgba8),
}

impl Fill {
    pub fn color(self) -> Rgba8 {
        match self {
            Fill::Solid(color) => color,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Rgba8 {
    pub r: u8,
    pub g: u8,
    pub b: u8,
    pub a: u8,
}

impl Rgba8 {
    pub const BLACK: Self = Self::new(0, 0, 0, 255);
    pub const WHITE: Self = Self::new(255, 255, 255, 255);

    pub const fn new(r: u8, g: u8, b: u8, a: u8) -> Self {
        Self { r, g, b, a }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct NormRect {
    pub x: f32,
    pub y: f32,
    pub width: f32,
    pub height: f32,
}

impl NormRect {
    pub const FULL: Self = Self::new(0.0, 0.0, 1.0, 1.0);

    pub const fn new(x: f32, y: f32, width: f32, height: f32) -> Self {
        Self {
            x,
            y,
            width,
            height,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Animatable<T> {
    Const(T),
    Track(Track<T>),
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct Track<T> {
    keys: Vec<Keyframe<T>>,
}

impl<T> Track<T> {
    /// Sorts keys by time while preserving input order among duplicate times.
    /// That stable ordering makes the later duplicate key authoritative.
    pub fn new(mut keys: Vec<Keyframe<T>>) -> Result<Self, TrackError> {
        if keys.is_empty() {
            return Err(TrackError::Empty);
        }
        if keys.iter().any(|key| !key.t.is_finite()) {
            return Err(TrackError::NonFiniteTime);
        }
        keys.sort_by(|left, right| left.t.total_cmp(&right.t));
        Ok(Self { keys })
    }

    pub fn keys(&self) -> &[Keyframe<T>] {
        &self.keys
    }
}

impl<'de, T> Deserialize<'de> for Track<T>
where
    T: Deserialize<'de>,
{
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        #[derive(Deserialize)]
        struct TrackWire<T> {
            keys: Vec<Keyframe<T>>,
        }

        let wire = TrackWire::deserialize(deserializer)?;
        Track::new(wire.keys).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Keyframe<T> {
    pub t: f64,
    pub value: T,
    pub interp: Interp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Interp {
    Step,
    Linear,
    EaseIn,
    EaseOut,
    EaseInOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrackError {
    Empty,
    NonFiniteTime,
}

impl fmt::Display for TrackError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            TrackError::Empty => f.write_str("a track must contain at least one keyframe"),
            TrackError::NonFiniteTime => f.write_str("keyframe times must be finite"),
        }
    }
}

impl std::error::Error for TrackError {}

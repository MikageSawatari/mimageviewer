use std::fmt;
use std::path::PathBuf;

use serde::{Deserialize, Deserializer, Serialize, de};

use crate::spectrum::Smoothing;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct LayerId(pub u64);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ImageId(pub String);

impl ImageId {
    pub fn new(value: impl Into<String>) -> Self {
        Self(value.into())
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ModulatorId(pub u64);

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Project {
    pub version: u32,
    pub audio: Option<AudioRef>,
    pub images: Vec<ImageRef>,
    pub output: Output,
    pub duration_secs: f64,
    /// 鍵盤表示へ適用する追加の表示ゲイン。音声そのものの音量は変えない。
    #[serde(default)]
    pub display_gain_db: f32,
    /// Audio-driven sources evaluated once per frame before layer bindings.
    #[serde(default)]
    pub modulators: Vec<Modulator>,
    /// Drawing order. The first layer is the backmost layer.
    pub layers: Vec<Layer>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Modulator {
    pub id: ModulatorId,
    pub name: String,
    pub source: ModulatorSource,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum ModulatorSource {
    BandEnergy {
        range: BandRange,
        smoothing: Smoothing,
        #[serde(default)]
        weighting: BandWeighting,
        gain_db: f32,
        floor: f32,
        ceiling: f32,
        /// Whether the input range is taken from the saved manual values or
        /// calibrated from the complete decoded track.
        #[serde(default)]
        calibration: BandCalibration,
        /// Values below this normalized 0..=1 level are treated as silence
        /// before temporal smoothing.
        #[serde(default)]
        threshold: f32,
    },
    /// Positive change in a band's gained display energy. Unlike
    /// `BandEnergy`, a sustained bass note settles back to zero.
    BandOnset {
        range: BandRange,
        smoothing: Smoothing,
        #[serde(default)]
        weighting: BandWeighting,
        gain_db: f32,
        floor: f32,
        ceiling: f32,
        #[serde(default)]
        calibration: BandCalibration,
        #[serde(default)]
        threshold: f32,
    },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub enum BandCalibration {
    /// Preserve the P3c/P3d floor and ceiling exactly. This is also the default
    /// for version-1 JSON written before automatic calibration existed.
    #[default]
    Manual,
    /// Map these percentiles of the complete decoded track to 0 and 1.
    Auto {
        low_percentile: f32,
        high_percentile: f32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum BandRange {
    Hz { low_hz: f32, high_hz: f32 },
    Semitone { low_midi: u8, high_midi: u8 },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BandWeighting {
    #[default]
    Raw,
    MivDisplay,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModulationTarget {
    Opacity,
    Scale,
    ImageZoom,
    Rotation,
    CornerRadius,
    GlowRadius,
    GlowAlpha,
    FillBrightness,
    SpectrumThickness,
    SpectrumRadius,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ModulationCurve {
    #[default]
    Linear,
    Squared,
    Sqrt,
}

impl ModulationCurve {
    pub fn apply(self, value: f32) -> f32 {
        let value = if value.is_finite() {
            value.clamp(0.0, 1.0)
        } else {
            0.0
        };
        match self {
            Self::Linear => value,
            Self::Squared => value * value,
            Self::Sqrt => value.sqrt(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ModulationBinding {
    pub target: ModulationTarget,
    pub modulator: ModulatorId,
    pub amount: f32,
    #[serde(default)]
    pub curve: ModulationCurve,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ModulatorValue {
    pub id: ModulatorId,
    pub value: f32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioRef {
    pub path: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ImageRef {
    pub id: ImageId,
    pub path: PathBuf,
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
    #[serde(default = "default_rotation")]
    pub rotation: Animatable<f32>,
    #[serde(default = "default_corner_radius")]
    pub corner_radius: Animatable<f32>,
    #[serde(default = "default_glow")]
    pub glow: Animatable<Option<Glow>>,
    #[serde(default)]
    pub modulations: Vec<ModulationBinding>,
    pub kind: LayerKind,
}

fn default_rotation() -> Animatable<f32> {
    Animatable::Const(0.0)
}

fn default_corner_radius() -> Animatable<f32> {
    Animatable::Const(0.0)
}

fn default_glow() -> Animatable<Option<Glow>> {
    Animatable::Const(None)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum LayerKind {
    Solid {
        fill: Animatable<Fill>,
    },
    Image {
        image: ImageId,
        fit: Fit,
        #[serde(default = "default_image_zoom")]
        zoom: Animatable<f32>,
    },
    Spectrum(SpectrumLayer),
    PitchKeyboard(PitchKeyboardLayer),
    ProgressBar(ProgressBarLayer),
    TimeText(TimeTextLayer),
    Text(TextLayer),
    Lyrics(LyricsLayer),
}

fn default_image_zoom() -> Animatable<f32> {
    Animatable::Const(1.0)
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SpectrumLayer {
    pub band_count: usize,
    pub point_count: usize,
    pub shape: SpectrumShape,
    #[serde(default)]
    pub color_mode: MusicColorMode,
    pub fill: Animatable<Fill>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PitchKeyboardLayer {
    /// 鍵盤レイヤ全体の背景。書き出し側の既定は透明。
    pub background: Animatable<Fill>,
    #[serde(default)]
    pub color_mode: MusicColorMode,
    /// 点灯している鍵に使う塗り。消灯色と枠は本体鍵盤と同じ規則で決まる。
    pub fill: Animatable<Fill>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum MusicColorMode {
    #[default]
    Custom,
    Miv,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub enum SpectrumShape {
    Bars {
        gap_ratio: f32,
        min_height_px: f32,
        thickness_px: f32,
    },
    Radial {
        radius_px: f32,
        thickness_px: f32,
        start_angle_deg: f32,
        sweep_deg: f32,
        direction: SpectrumDirection,
    },
    /// mIV 本体と同じ対数周波数軸・残光・立ち上がり強調の横長表示。
    MivHorizontal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SpectrumDirection {
    Clockwise,
    CounterClockwise,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ProgressBarLayer {
    pub border_thickness_px: f32,
    pub fill: Animatable<Fill>,
    pub border_fill: Animatable<Fill>,
}

/// Shared comic-core paint effects. Glyph Fill and layer Glow stay independent.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct TextStyle {
    pub outline: Option<StrokeStyle>,
    pub extra_outlines: Vec<StrokeStyle>,
    pub shadow: Option<TextShadowStyle>,
    pub echo: Option<TextEchoStyle>,
    pub background: Option<TextBackgroundStyle>,
    pub letter_gap: f32,
    pub line_gap: f32,
}

pub use comic_core::{
    Rgba as TextColor, StrokeStyle, TextBackgroundStyle, TextEchoStyle, TextShadowStyle,
};

impl TextStyle {
    pub fn has_paint(&self) -> bool {
        self.outline.is_some()
            || !self.extra_outlines.is_empty()
            || self.shadow.is_some()
            || self.echo.is_some()
            || self.background.is_some()
    }
}

fn default_text_style() -> Animatable<TextStyle> {
    Animatable::Const(TextStyle::default())
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TimeTextLayer {
    pub format: TimeFormat,
    pub font_key: String,
    pub size_px: f32,
    pub align: TextAlign,
    #[serde(default = "default_text_style")]
    pub style: Animatable<TextStyle>,
    pub fill: Animatable<Fill>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TextLayer {
    pub text: String,
    pub font_key: String,
    pub size_px: f32,
    pub align: TextAlign,
    #[serde(default = "default_text_style")]
    pub style: Animatable<TextStyle>,
    pub fill: Animatable<Fill>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricsLayer {
    pub lines: Vec<LyricLine>,
    pub fade_in_secs: f32,
    pub fade_out_secs: f32,
    pub font_key: String,
    pub size_px: f32,
    pub align: TextAlign,
    #[serde(default = "default_text_style")]
    pub style: Animatable<TextStyle>,
    pub fill: Animatable<Fill>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct LyricLine {
    pub text: String,
    pub time_secs: Option<f64>,
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

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum Fill {
    Solid(Rgba8),
    LinearGradient {
        stops: GradientStops,
        angle_deg: f32,
    },
    RadialGradient {
        stops: GradientStops,
        center: (f32, f32),
        radius: f32,
    },
    BandGradient {
        stops: GradientStops,
    },
}

impl Fill {
    pub fn color(&self) -> Rgba8 {
        match self {
            Fill::Solid(color) => *color,
            Fill::LinearGradient { stops, .. }
            | Fill::RadialGradient { stops, .. }
            | Fill::BandGradient { stops } => stops.sample(0.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct GradientStop {
    pub position: f32,
    pub color: Rgba8,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
#[serde(transparent)]
pub struct GradientStops(Vec<GradientStop>);

impl GradientStops {
    /// Validates and stably sorts stops. For duplicate positions, the later
    /// stop is authoritative at the exact boundary.
    pub fn new(mut stops: Vec<GradientStop>) -> Result<Self, GradientStopsError> {
        if stops.is_empty() {
            return Err(GradientStopsError::Empty);
        }
        if stops.iter().any(|stop| !stop.position.is_finite()) {
            return Err(GradientStopsError::NonFinitePosition);
        }
        if stops
            .iter()
            .any(|stop| !(0.0..=1.0).contains(&stop.position))
        {
            return Err(GradientStopsError::PositionOutOfRange);
        }
        stops.sort_by(|left, right| left.position.total_cmp(&right.position));
        Ok(Self(stops))
    }

    pub fn as_slice(&self) -> &[GradientStop] {
        &self.0
    }

    pub fn to_vec(&self) -> Vec<GradientStop> {
        self.0.clone()
    }

    pub fn sample(&self, position: f32) -> Rgba8 {
        let position = if position.is_finite() {
            position.clamp(0.0, 1.0)
        } else {
            0.0
        };
        let next = self.0.partition_point(|stop| stop.position <= position);
        if next == 0 {
            return self.0[0].color;
        }
        let current = self.0[next - 1];
        let Some(following) = self.0.get(next).copied() else {
            return current.color;
        };
        let span = following.position - current.position;
        if span <= 0.0 {
            return following.color;
        }
        let amount = (position - current.position) / span;
        interpolate_rgba(current.color, following.color, amount)
    }
}

impl<'de> Deserialize<'de> for GradientStops {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let stops = Vec::<GradientStop>::deserialize(deserializer)?;
        Self::new(stops).map_err(de::Error::custom)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradientStopsError {
    Empty,
    NonFinitePosition,
    PositionOutOfRange,
}

impl fmt::Display for GradientStopsError {
    fn fmt(&self, f: &mut fmt::Formatter) -> fmt::Result {
        match self {
            Self::Empty => f.write_str("グラデーションには 1 個以上のストップが必要です"),
            Self::NonFinitePosition => {
                f.write_str("グラデーションのストップ位置は有限値である必要があります")
            }
            Self::PositionOutOfRange => {
                f.write_str("グラデーションのストップ位置は 0 から 1 の範囲で指定してください")
            }
        }
    }
}

impl std::error::Error for GradientStopsError {}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Glow {
    pub color: Rgba8,
    pub radius_px: f32,
    pub spread_px: f32,
}

fn interpolate_rgba(from: Rgba8, to: Rgba8, amount: f32) -> Rgba8 {
    let channel = |from: u8, to: u8| {
        (from as f32 + (to as f32 - from as f32) * amount)
            .round()
            .clamp(0.0, 255.0) as u8
    };
    Rgba8 {
        r: channel(from.r, to.r),
        g: channel(from.g, to.g),
        b: channel(from.b, to.b),
        a: channel(from.a, to.a),
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::preset::default_project;

    fn stop(position: f32, color: Rgba8) -> GradientStop {
        GradientStop { position, color }
    }

    #[test]
    fn gradient_stops_reject_empty_non_finite_and_out_of_range_input() {
        assert_eq!(GradientStops::new(vec![]), Err(GradientStopsError::Empty));
        assert_eq!(
            GradientStops::new(vec![stop(f32::NAN, Rgba8::WHITE)]),
            Err(GradientStopsError::NonFinitePosition)
        );
        assert_eq!(
            GradientStops::new(vec![stop(1.1, Rgba8::WHITE)]),
            Err(GradientStopsError::PositionOutOfRange)
        );
    }

    #[test]
    fn one_stop_is_constant_and_stops_are_stably_sorted() {
        let one = GradientStops::new(vec![stop(0.4, Rgba8::new(1, 2, 3, 4))]).unwrap();
        assert_eq!(one.sample(0.0), Rgba8::new(1, 2, 3, 4));
        assert_eq!(one.sample(1.0), Rgba8::new(1, 2, 3, 4));

        let sorted =
            GradientStops::new(vec![stop(1.0, Rgba8::WHITE), stop(0.0, Rgba8::BLACK)]).unwrap();
        assert_eq!(sorted.as_slice()[0].position, 0.0);
        assert_eq!(sorted.sample(0.5), Rgba8::new(128, 128, 128, 255));
    }

    #[test]
    fn later_duplicate_stop_wins_at_exact_position() {
        let stops = GradientStops::new(vec![
            stop(0.0, Rgba8::BLACK),
            stop(0.5, Rgba8::new(100, 0, 0, 255)),
            stop(0.5, Rgba8::new(200, 0, 0, 255)),
            stop(1.0, Rgba8::WHITE),
        ])
        .unwrap();
        assert_eq!(stops.sample(0.5), Rgba8::new(200, 0, 0, 255));
        assert!(stops.sample(0.49).r < 100);
    }

    #[test]
    fn deserialization_cannot_construct_an_empty_gradient() {
        assert!(serde_json::from_str::<GradientStops>("[]").is_err());
    }

    #[test]
    fn older_project_json_defaults_display_gain_and_music_color_modes() {
        let mut project = default_project(10.0);
        project
            .layers
            .push(crate::preset::new_pitch_keyboard_layer(&project));
        let mut value = serde_json::to_value(project).unwrap();
        value.as_object_mut().unwrap().remove("display_gain_db");
        value.as_object_mut().unwrap().remove("modulators");
        for layer in value["layers"].as_array_mut().unwrap() {
            layer.as_object_mut().unwrap().remove("modulations");
            let kind = &mut layer["kind"];
            if let Some(spectrum) = kind.get_mut("Spectrum") {
                spectrum.as_object_mut().unwrap().remove("color_mode");
            }
            if let Some(keyboard) = kind.get_mut("PitchKeyboard") {
                keyboard.as_object_mut().unwrap().remove("color_mode");
            }
            if let Some(image) = kind.get_mut("Image") {
                image.as_object_mut().unwrap().remove("zoom");
            }
        }
        let restored: Project = serde_json::from_value(value).unwrap();
        assert_eq!(restored.display_gain_db, 0.0);
        assert!(restored.modulators.is_empty());
        assert!(
            restored
                .layers
                .iter()
                .all(|layer| layer.modulations.is_empty())
        );
        let LayerKind::Spectrum(spectrum) = &restored.layers[1].kind else {
            panic!("既定シーンの 2 番目はスペクトラムです");
        };
        assert_eq!(spectrum.color_mode, MusicColorMode::Custom);
        let LayerKind::PitchKeyboard(keyboard) = &restored.layers[4].kind else {
            panic!("既定シーンへ追加した 5 番目は鍵盤です");
        };
        assert_eq!(keyboard.color_mode, MusicColorMode::Custom);
        let LayerKind::Image { zoom, .. } = &restored.layers[0].kind else {
            panic!("既定シーンの 1 番目は画像です");
        };
        assert_eq!(zoom, &Animatable::Const(1.0));
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

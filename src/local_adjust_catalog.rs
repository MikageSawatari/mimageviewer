use local_adjust_core::*;

macro_rules! effect_kind_catalog {
    ($($kind:ident => $variant:ident($params:ty)),+ $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub(crate) enum EffectKind {
            None,
            $($kind,)+
        }

        impl EffectKind {
            pub(crate) fn from_effect(effect: &LocalEffect) -> Self {
                match effect {
                    LocalEffect::None => Self::None,
                    $(LocalEffect::$variant(_) => Self::$kind,)+
                }
            }

            pub(crate) fn default_effect(self) -> LocalEffect {
                match self {
                    Self::None => LocalEffect::None,
                    $(Self::$kind => LocalEffect::$variant(<$params>::default()),)+
                }
            }
        }
    };
}

effect_kind_catalog! {
    Tone => Tone(ToneParams),
    ToneCurve => ToneCurve(ToneCurveParams),
    RgbToneCurve => RgbToneCurve(RgbToneCurveParams),
    ColorBalance => ColorBalance(ColorBalanceParams),
    PhotoFilter => PhotoFilter(PhotoFilterParams),
    ThreeWayColorGrading => ThreeWayColorGrading(ThreeWayColorGradingParams),
    SelectiveColor => SelectiveColor(SelectiveColorParams),
    PartColor => PartColor(PartColorParams),
    ChannelMixer => ChannelMixer(ChannelMixerParams),
    MonochromeMixer => MonochromeMixer(MonochromeMixerParams),
    Clarity => Clarity(ClarityParams),
    Texture => Texture(TextureParams),
    HighPass => HighPass(HighPassParams),
    FrequencySeparation => FrequencySeparation(FrequencySeparationParams),
    HighlightsShadows => HighlightsShadows(HighlightsShadowsParams),
    Dehaze => Dehaze(DehazeParams),
    Blur => Blur(BlurParams),
    MotionBlur => MotionBlur(MotionBlurParams),
    Wind => Wind(WindParams),
    SpeedLines => SpeedLines(SpeedLinesParams),
    TiltShift => TiltShift(TiltShiftParams),
    LensBlur => LensBlur(LensBlurParams),
    BokehSprite => BokehSprite(BokehSpriteParams),
    LensDirt => LensDirt(LensDirtParams),
    RadialBlur => RadialBlur(RadialBlurParams),
    WaveDistortion => WaveDistortion(WaveDistortionParams),
    HeatHaze => HeatHaze(HeatHazeParams),
    PinchSpherize => PinchSpherize(PinchSpherizeParams),
    Twirl => Twirl(TwirlParams),
    PolarCoordinates => PolarCoordinates(PolarCoordinatesParams),
    GlassDisplacement => GlassDisplacement(GlassDisplacementParams),
    LensCorrection => LensCorrection(LensCorrectionParams),
    LineExtract => LineExtract(LineExtractParams),
    ArtisticMedia => ArtisticMedia(ArtisticMediaParams),
    BrushStroke => BrushStroke(BrushStrokeParams),
    Cutout => Cutout(CutoutParams),
    ToonShade => ToonShade(ToonShadeParams),
    Emboss => Emboss(EmbossParams),
    PixelStylize => PixelStylize(PixelStylizeParams),
    Solarize => Solarize(SolarizeParams),
    GlowingEdges => GlowingEdges(GlowingEdgesParams),
    OilPaint => OilPaint(OilPaintParams),
    SoftFocus => SoftFocus(SoftFocusParams),
    Orton => Orton(OrtonParams),
    Mosaic => Mosaic(MosaicParams),
    Sharpen => Sharpen(SharpenParams),
    SmartSharpen => SmartSharpen(SmartSharpenParams),
    Hsl => Hsl(HslParams),
    ColorMixer => ColorMixer(ColorMixerParams),
    Look => Look(LookParams),
    CubeLut => CubeLut(CubeLutParams),
    Posterize => Posterize(PosterizeParams),
    RetroPalette => RetroPalette(RetroPaletteParams),
    CrtDisplay => CrtDisplay(CrtDisplayParams),
    Threshold => Threshold(ThresholdParams),
    Invert => Invert(InvertParams),
    Duotone => Duotone(DuotoneParams),
    Equalize => Equalize(EqualizeParams),
    GradientMap => GradientMap(GradientMapParams),
    ColorFill => ColorFill(ColorFillParams),
    Frame => Frame(FrameParams),
    OutlineStroke => OutlineStroke(OutlineStrokeParams),
    RimLight => RimLight(RimLightParams),
    ContactShadow => ContactShadow(ContactShadowParams),
    ColorTrace => ColorTrace(ColorTraceParams),
    ColorOverlay => ColorOverlay(ColorOverlayParams),
    NeonGlow => NeonGlow(NeonGlowParams),
    DiffuseGlow => DiffuseGlow(DiffuseGlowParams),
    Bloom => Bloom(BloomParams),
    Halation => Halation(HalationParams),
    ColorDodgeGlow => ColorDodgeGlow(ColorDodgeGlowParams),
    GodRays => GodRays(GodRaysParams),
    LensFlare => LensFlare(LensFlareParams),
    AnamorphicFlare => AnamorphicFlare(AnamorphicFlareParams),
    LightLeak => LightLeak(LightLeakParams),
    BacklightHaze => BacklightHaze(BacklightHazeParams),
    CloudFog => CloudFog(CloudFogParams),
    WaterCaustics => WaterCaustics(WaterCausticsParams),
    ParticleOverlay => ParticleOverlay(ParticleOverlayParams),
    Aurora => Aurora(AuroraParams),
    Spotlight => Spotlight(SpotlightParams),
    RadialFlash => RadialFlash(RadialFlashParams),
    Vignette => Vignette(VignetteParams),
    FilmGrain => FilmGrain(FilmGrainParams),
    Noise => Noise(NoiseParams),
    ChromaticAberration => ChromaticAberration(ChromaticAberrationParams),
    Anaglyph3d => Anaglyph3d(AnaglyphParams),
    Defringe => Defringe(DefringeParams),
    ScanlineGlitch => ScanlineGlitch(ScanlineGlitchParams),
    Vhs => Vhs(VhsParams),
    DataMosh => DataMosh(DataMoshParams),
    PixelSort => PixelSort(PixelSortParams),
    OldFilm => OldFilm(OldFilmParams),
    Halftone => Halftone(HalftoneParams),
    ScreenTone => ScreenTone(ScreenToneParams),
    ColorHalftone => ColorHalftone(ColorHalftoneParams),
    CmykPlateShift => CmykPlateShift(CmykPlateShiftParams),
    Lithograph => Lithograph(LithographParams),
    Engraving => Engraving(EngravingParams),
    NewspaperPrint => NewspaperPrint(NewspaperPrintParams),
    Textureizer => Textureizer(TextureizerParams),
    StarGlow => StarGlow(StarGlowParams),
    DiffractionStarburst => DiffractionStarburst(DiffractionStarburstParams),
    EdgeSmooth => EdgeSmooth(EdgeSmoothParams),
    Despeckle => Despeckle(DespeckleParams),
    Median => Median(MedianParams),
}

impl EffectKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::None => "効果なし",
            Self::ThreeWayColorGrading => "3-wayグレーディング",
            Self::HighlightsShadows => "ハイライト/シャドウ",
            Self::MotionBlur => "移動ぼかし",
            Self::SpeedLines => "集中線/スピード線",
            Self::RadialBlur => "放射/回転ぼかし",
            Self::ArtisticMedia => "水彩/鉛筆",
            Self::BrushStroke => "ドライブラシ/塗料",
            Self::Posterize => "ポスタリゼーション",
            Self::Threshold => "2値化",
            Self::Invert => "階調反転/ネガ",
            Self::Duotone => "ダブルトーン",
            Self::Equalize => "ヒストグラム平坦化",
            Self::Frame => "フレーム/黒帯",
            Self::ColorOverlay => "塗り/グラデーション",
            Self::AnamorphicFlare => "アナモルフィックフレア",
            Self::ParticleOverlay => "雨/雪/花びら",
            Self::Noise => "ノイズ付加",
            Self::Vhs => "VHS/アナログ",
            _ => self.default_effect().display_label(),
        }
    }

    pub(crate) fn picker_label(self) -> &'static str {
        match self {
            Self::ThreeWayColorGrading => "3-wayカラー",
            Self::HighlightsShadows => "ハイライト/影",
            Self::Equalize => "ヒスト平坦化",
            Self::AnamorphicFlare => "アナモルフフレア",
            Self::BokehSprite => "玉ボケ粒子",
            Self::PhotoFilter => "フォトフィルタ",
            Self::MonochromeMixer => "白黒ミキサー",
            Self::LensDirt => "レンズ汚れ",
            Self::FrequencySeparation => "周波数分離",
            Self::RetroPalette => "レトロ減色",
            Self::CrtDisplay => "CRT表示",
            Self::Frame => "フレーム",
            Self::DiffractionStarburst => "回折スター",
            Self::WaterCaustics => "水中光網",
            Self::ParticleOverlay => "天候粒子",
            Self::RadialFlash => "フラッシュ",
            Self::Anaglyph3d => "アナグリフ",
            _ => self.label(),
        }
    }

    pub(crate) fn description(self) -> &'static str {
        match self {
            Self::None => "加工を行わず、マスクだけを準備します。",
            Self::Tone => "明るさ、コントラスト、彩度、色温度などをまとめて調整します。",
            Self::PhotoFilter => "暖色・寒色・セピアなどの色付きフィルターを重ねます。",
            Self::MonochromeMixer => "色ごとの寄与を調整して白黒化します。",
            Self::Blur => "画像をぼかします。",
            Self::Sharpen => "輪郭を強調して画像を引き締めます。",
            Self::Vignette => "周辺を暗く、または明るくして視線を中央へ誘導します。",
            Self::CrtDisplay => "スキャンラインやRGBマスクでブラウン管表示風にします。",
            Self::Anaglyph3d => "RGBチャンネルをずらして立体視風や色ズレ表現を作ります。",
            _ => self.label(),
        }
    }

    pub(crate) fn layer(self) -> LocalAdjustmentLayer {
        LocalAdjustmentLayer::new(self.label(), LocalMask::Full, self.default_effect())
    }
}

pub(crate) struct EffectGroup {
    pub(crate) title: &'static str,
    pub(crate) kinds: &'static [EffectKind],
}

pub(crate) const EFFECT_GROUPS: &[EffectGroup] = &[
    EffectGroup {
        title: "基本",
        kinds: &[
            EffectKind::None,
            EffectKind::ColorFill,
            EffectKind::Frame,
            EffectKind::OutlineStroke,
        ],
    },
    EffectGroup {
        title: "色調補正",
        kinds: &[
            EffectKind::Tone,
            EffectKind::ToneCurve,
            EffectKind::RgbToneCurve,
            EffectKind::ColorBalance,
            EffectKind::PhotoFilter,
            EffectKind::ThreeWayColorGrading,
            EffectKind::SelectiveColor,
            EffectKind::PartColor,
            EffectKind::ChannelMixer,
            EffectKind::MonochromeMixer,
            EffectKind::Hsl,
            EffectKind::ColorMixer,
            EffectKind::HighlightsShadows,
            EffectKind::Dehaze,
            EffectKind::Equalize,
            EffectKind::Defringe,
        ],
    },
    EffectGroup {
        title: "色変換・ルック",
        kinds: &[
            EffectKind::Look,
            EffectKind::CubeLut,
            EffectKind::GradientMap,
            EffectKind::Posterize,
            EffectKind::RetroPalette,
            EffectKind::Threshold,
            EffectKind::Invert,
            EffectKind::Duotone,
        ],
    },
    EffectGroup {
        title: "ぼかし・フォーカス",
        kinds: &[
            EffectKind::Blur,
            EffectKind::MotionBlur,
            EffectKind::TiltShift,
            EffectKind::LensBlur,
            EffectKind::BokehSprite,
            EffectKind::RadialBlur,
            EffectKind::SoftFocus,
            EffectKind::Orton,
            EffectKind::EdgeSmooth,
            EffectKind::Despeckle,
            EffectKind::Median,
        ],
    },
    EffectGroup {
        title: "シャープ・ディテール",
        kinds: &[
            EffectKind::Clarity,
            EffectKind::Texture,
            EffectKind::HighPass,
            EffectKind::FrequencySeparation,
            EffectKind::Sharpen,
            EffectKind::SmartSharpen,
        ],
    },
    EffectGroup {
        title: "変形・歪み",
        kinds: &[
            EffectKind::WaveDistortion,
            EffectKind::HeatHaze,
            EffectKind::PinchSpherize,
            EffectKind::Twirl,
            EffectKind::PolarCoordinates,
            EffectKind::GlassDisplacement,
            EffectKind::LensCorrection,
        ],
    },
    EffectGroup {
        title: "表現・絵画調",
        kinds: &[
            EffectKind::Wind,
            EffectKind::SpeedLines,
            EffectKind::RadialFlash,
            EffectKind::LineExtract,
            EffectKind::ColorTrace,
            EffectKind::ArtisticMedia,
            EffectKind::BrushStroke,
            EffectKind::Cutout,
            EffectKind::ToonShade,
            EffectKind::Emboss,
            EffectKind::PixelStylize,
            EffectKind::Solarize,
            EffectKind::GlowingEdges,
            EffectKind::OilPaint,
            EffectKind::Halftone,
            EffectKind::ScreenTone,
            EffectKind::ColorHalftone,
            EffectKind::CmykPlateShift,
            EffectKind::Lithograph,
            EffectKind::Engraving,
            EffectKind::NewspaperPrint,
            EffectKind::Textureizer,
            EffectKind::CrtDisplay,
            EffectKind::ScanlineGlitch,
            EffectKind::Vhs,
            EffectKind::DataMosh,
            EffectKind::PixelSort,
            EffectKind::OldFilm,
        ],
    },
    EffectGroup {
        title: "隠蔽・加工",
        kinds: &[EffectKind::Mosaic],
    },
    EffectGroup {
        title: "光・雰囲気",
        kinds: &[
            EffectKind::ColorOverlay,
            EffectKind::NeonGlow,
            EffectKind::DiffuseGlow,
            EffectKind::Bloom,
            EffectKind::Halation,
            EffectKind::ColorDodgeGlow,
            EffectKind::RimLight,
            EffectKind::ContactShadow,
            EffectKind::GodRays,
            EffectKind::LensFlare,
            EffectKind::LensDirt,
            EffectKind::AnamorphicFlare,
            EffectKind::LightLeak,
            EffectKind::BacklightHaze,
            EffectKind::CloudFog,
            EffectKind::WaterCaustics,
            EffectKind::ParticleOverlay,
            EffectKind::Aurora,
            EffectKind::Spotlight,
            EffectKind::StarGlow,
            EffectKind::DiffractionStarburst,
            EffectKind::Vignette,
            EffectKind::FilmGrain,
            EffectKind::Noise,
            EffectKind::ChromaticAberration,
            EffectKind::Anaglyph3d,
        ],
    },
];

const EFFECT_PICKER_BUTTON_MIN_W: f32 = 78.0;
const EFFECT_PICKER_BUTTON_MAX_W: f32 = 150.0;

pub(crate) fn effect_picker_button_width(available_width: f32) -> f32 {
    let spacing = 6.0;
    let available_width = available_width.max(EFFECT_PICKER_BUTTON_MIN_W);
    let columns = ((available_width + spacing) / (EFFECT_PICKER_BUTTON_MIN_W + spacing))
        .floor()
        .clamp(1.0, 5.0);
    ((available_width - spacing * (columns - 1.0)) / columns)
        .clamp(EFFECT_PICKER_BUTTON_MIN_W, EFFECT_PICKER_BUTTON_MAX_W)
}

pub(crate) fn effect_picker_matches_query(kind: EffectKind, query: &str) -> bool {
    let query = query.trim().to_lowercase();
    if query.is_empty() {
        return true;
    }

    let fields = [
        kind.label().to_lowercase(),
        kind.picker_label().to_lowercase(),
        kind.description().to_lowercase(),
    ];
    query
        .split_whitespace()
        .all(|token| fields.iter().any(|field| field.contains(token)))
}

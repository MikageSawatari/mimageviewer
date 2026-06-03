use eframe::egui::{self, Color32, ComboBox, Pos2, Sense};
use local_adjust_core::*;

trait LabHoverTipExt {
    fn lab_hover_tip(self, text: impl Into<egui::WidgetText>) -> Self;
}

impl LabHoverTipExt for egui::Response {
    fn lab_hover_tip(self, text: impl Into<egui::WidgetText>) -> Self {
        self.on_hover_text(text)
    }
}

fn lab_combo_box<R>(
    ui: &mut egui::Ui,
    id_salt: impl std::hash::Hash,
    selected_text: impl Into<egui::WidgetText>,
    add_contents: impl FnOnce(&mut egui::Ui) -> R,
) -> egui::InnerResponse<Option<R>> {
    ComboBox::from_id_salt(id_salt)
        .selected_text(selected_text)
        .height(420.0)
        .show_ui(ui, add_contents)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum RgbPickTarget {
    ColorFillStart,
    ColorFillMiddle,
    ColorFillEnd,
    FrameColor,
    FrameLineColor,
    PhotoFilterColor,
    MonochromeMixerTint,
    ColorOverlayStart,
    ColorOverlayEnd,
    NeonGlowSource,
    NeonGlowTint,
    SpeedLinesColor,
    CloudFogColor,
    ParticleOverlayColor,
    AuroraColor,
    AuroraSecondaryColor,
    SpotlightTint,
    OutlineStrokeColor,
    RimLightColor,
    ContactShadowColor,
    ColorDodgeGlowColor,
    AnamorphicFlareColor,
    LightLeakColor,
    BacklightHazeColor,
    LithographInkA,
    LithographInkB,
    LithographPaper,
    EngravingInk,
    EngravingPaper,
    PartColorTarget,
    HalationTint,
    ToonShadeShadowTint,
    ToonShadeLightTint,
}

impl RgbPickTarget {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::ColorFillStart => "塗りつぶしの開始色",
            Self::ColorFillMiddle => "塗りつぶしの中間色",
            Self::ColorFillEnd => "塗りつぶしの終了色",
            Self::FrameColor => "フレームの色",
            Self::FrameLineColor => "フレームの内側ライン色",
            Self::PhotoFilterColor => "フォトフィルターのカスタム色",
            Self::MonochromeMixerTint => "モノクロミキサーの色調",
            Self::ColorOverlayStart => "塗り/グラデーションの開始色",
            Self::ColorOverlayEnd => "塗り/グラデーションの終了色",
            Self::NeonGlowSource => "ネオングローの発光源色",
            Self::NeonGlowTint => "ネオングローの着色",
            Self::SpeedLinesColor => "集中線/スピード線の線色",
            Self::CloudFogColor => "雲/霧の色",
            Self::ParticleOverlayColor => "雨/雪/花びらの色",
            Self::AuroraColor => "オーロラの主色",
            Self::AuroraSecondaryColor => "オーロラの副色",
            Self::SpotlightTint => "スポットライトの光色",
            Self::OutlineStrokeColor => "縁取りの線色",
            Self::RimLightColor => "リムライトの光色",
            Self::ContactShadowColor => "接触影の影色",
            Self::ColorDodgeGlowColor => "覆い焼き発光の光色",
            Self::AnamorphicFlareColor => "アナモルフィックフレアの色",
            Self::LightLeakColor => "ライトリークの光色",
            Self::BacklightHazeColor => "逆光ヘイズの光色",
            Self::LithographInkA => "リソグラフのインク1",
            Self::LithographInkB => "リソグラフのインク2",
            Self::LithographPaper => "リソグラフの紙色",
            Self::EngravingInk => "銅版画のインク",
            Self::EngravingPaper => "銅版画の紙色",
            Self::PartColorTarget => "パートカラーの対象色",
            Self::HalationTint => "ハレーションの暖色",
            Self::ToonShadeShadowTint => "トゥーン影色",
            Self::ToonShadeLightTint => "トゥーン光色",
        }
    }
}

fn noise_distribution_label(distribution: NoiseDistribution) -> &'static str {
    match distribution {
        NoiseDistribution::Uniform => "均一",
        NoiseDistribution::Gaussian => "ガウス",
    }
}

fn anaglyph_mode_label(mode: AnaglyphMode) -> &'static str {
    match mode {
        AnaglyphMode::RedCyan => "赤シアン",
        AnaglyphMode::GreenMagenta => "緑マゼンタ",
        AnaglyphMode::AmberBlue => "琥珀青",
        AnaglyphMode::RgbSplit => "RGB分離",
    }
}

fn pixel_sort_direction_label(direction: PixelSortDirection) -> &'static str {
    match direction {
        PixelSortDirection::Horizontal => "横方向",
        PixelSortDirection::Vertical => "縦方向",
    }
}

fn pixel_sort_order_label(order: PixelSortOrder) -> &'static str {
    match order {
        PixelSortOrder::DarkToLight => "暗い→明るい",
        PixelSortOrder::LightToDark => "明るい→暗い",
    }
}

fn mosaic_boundary_label(boundary: MosaicBoundary) -> &'static str {
    match boundary {
        MosaicBoundary::Opaque => "タイル不透明",
        MosaicBoundary::Translucent => "割合で半透明",
        MosaicBoundary::MaskShape => "マスク形状",
    }
}

fn look_preset_label(preset: LookPreset) -> &'static str {
    match preset {
        LookPreset::None => "選択してください",
        LookPreset::Sunset => "夕焼け",
        LookPreset::Night => "夜景",
        LookPreset::BrightSun => "明るい日光",
        LookPreset::Pale => "淡色",
        LookPreset::Cool => "寒色",
        LookPreset::Warm => "暖色",
        LookPreset::RetroFilm => "レトロ/フィルム",
        LookPreset::TealOrange => "ティール&オレンジ",
        LookPreset::CherryBlossom => "桜色",
        LookPreset::FreshGreen => "新緑",
        LookPreset::Moonlight => "月明かり",
        LookPreset::HighKey => "ハイキー",
        LookPreset::LowKey => "ローキー",
        LookPreset::Sepia => "セピア",
        LookPreset::Cyberpunk => "サイバーパンク",
    }
}

fn retro_palette_mode_label(mode: RetroPaletteMode) -> &'static str {
    match mode {
        RetroPaletteMode::Dither1Bit => "1bitディザ",
        RetroPaletteMode::GameBoy => "GameBoy",
        RetroPaletteMode::Famicom => "ファミコン",
        RetroPaletteMode::Msx2Plus => "MSX2+",
        RetroPaletteMode::Pc98 => "PC-98",
        RetroPaletteMode::GameGear => "ゲームギア",
        RetroPaletteMode::MegaDrive => "メガドライブ",
        RetroPaletteMode::Sfc => "SFC",
    }
}

fn crt_display_mode_label(mode: CrtDisplayMode) -> &'static str {
    match mode {
        CrtDisplayMode::Simple => "控えめ",
        CrtDisplayMode::Full => "フル",
        CrtDisplayMode::Arcade => "アーケード",
    }
}

fn color_mixer_band_label(index: usize) -> &'static str {
    match index {
        0 => "赤",
        1 => "橙/肌",
        2 => "黄",
        3 => "緑",
        4 => "シアン",
        5 => "青",
        6 => "紫",
        7 => "マゼンタ",
        _ => "色帯",
    }
}

fn gradient_map_preset_label(preset: GradientMapPreset) -> &'static str {
    match preset {
        GradientMapPreset::None => "選択してください",
        GradientMapPreset::Mono => "モノクロ",
        GradientMapPreset::Sepia => "セピア",
        GradientMapPreset::Sunset => "夕焼け",
        GradientMapPreset::Twilight => "薄暮",
        GradientMapPreset::TealOrange => "ティール&オレンジ",
        GradientMapPreset::Cherry => "桜色",
        GradientMapPreset::Forest => "森",
        GradientMapPreset::Fire => "炎",
        GradientMapPreset::Ice => "氷",
    }
}

fn color_overlay_shape_label(shape: ColorOverlayShape) -> &'static str {
    match shape {
        ColorOverlayShape::Unselected => "選択してください",
        ColorOverlayShape::Solid => "単色",
        ColorOverlayShape::Linear => "線形グラデーション",
        ColorOverlayShape::Radial => "円形グラデーション",
    }
}

fn frame_mode_label(mode: FrameMode) -> &'static str {
    match mode {
        FrameMode::Border => "フレーム",
        FrameMode::Letterbox => "レターボックス",
        FrameMode::RoundedMatte => "角丸マット",
    }
}

fn photo_filter_preset_label(preset: PhotoFilterPreset) -> &'static str {
    match preset {
        PhotoFilterPreset::Custom => "カスタム",
        PhotoFilterPreset::Warm85 => "Warm 85",
        PhotoFilterPreset::Warm81 => "Warm 81",
        PhotoFilterPreset::Cool80 => "Cool 80",
        PhotoFilterPreset::Cool82 => "Cool 82",
        PhotoFilterPreset::Sepia => "セピア",
        PhotoFilterPreset::Sunset => "夕景",
        PhotoFilterPreset::Underwater => "水中",
        PhotoFilterPreset::Magenta => "マゼンタ",
        PhotoFilterPreset::Green => "グリーン",
    }
}

fn outline_stroke_placement_label(placement: OutlineStrokePlacement) -> &'static str {
    match placement {
        OutlineStrokePlacement::Outside => "外側",
        OutlineStrokePlacement::Inside => "内側",
        OutlineStrokePlacement::Center => "中央",
    }
}

fn hue_degrees_from_rgb(rgb: [u8; 3]) -> f32 {
    let r = rgb[0] as f32 / 255.0;
    let g = rgb[1] as f32 / 255.0;
    let b = rgb[2] as f32 / 255.0;
    let max = r.max(g).max(b);
    let min = r.min(g).min(b);
    let delta = max - min;
    if delta <= f32::EPSILON {
        return 0.0;
    }
    let hue = if (max - r).abs() <= f32::EPSILON {
        ((g - b) / delta).rem_euclid(6.0)
    } else if (max - g).abs() <= f32::EPSILON {
        (b - r) / delta + 2.0
    } else {
        (r - g) / delta + 4.0
    };
    (hue * 60.0).rem_euclid(360.0)
}

fn hsl_swatch_color(hue_degrees: f32, saturation: f32, lightness: f32) -> Color32 {
    let h = hue_degrees.rem_euclid(360.0) / 360.0;
    let s = saturation.clamp(0.0, 1.0);
    let l = lightness.clamp(0.0, 1.0);
    if s <= f32::EPSILON {
        let v = (l * 255.0).round() as u8;
        return Color32::from_rgb(v, v, v);
    }
    let q = if l < 0.5 {
        l * (1.0 + s)
    } else {
        l + s - l * s
    };
    let p = 2.0 * l - q;
    let r = hue_channel(p, q, h + 1.0 / 3.0);
    let g = hue_channel(p, q, h);
    let b = hue_channel(p, q, h - 1.0 / 3.0);
    Color32::from_rgb(
        (r * 255.0).round() as u8,
        (g * 255.0).round() as u8,
        (b * 255.0).round() as u8,
    )
}

fn hue_channel(p: f32, q: f32, t: f32) -> f32 {
    let t = t.rem_euclid(1.0);
    if t < 1.0 / 6.0 {
        p + (q - p) * 6.0 * t
    } else if t < 0.5 {
        q
    } else if t < 2.0 / 3.0 {
        p + (q - p) * (2.0 / 3.0 - t) * 6.0
    } else {
        p
    }
}

fn color_overlay_blend_mode_label(mode: ColorOverlayBlendMode) -> &'static str {
    match mode {
        ColorOverlayBlendMode::Normal => "通常",
        ColorOverlayBlendMode::Multiply => "乗算",
        ColorOverlayBlendMode::Screen => "スクリーン",
        ColorOverlayBlendMode::Overlay => "オーバーレイ",
        ColorOverlayBlendMode::SoftLight => "ソフトライト",
        ColorOverlayBlendMode::Color => "カラー",
    }
}

#[derive(Debug, Default)]
struct RgbColorControlResponse {
    changed: bool,
    start_pick: Option<RgbPickTarget>,
    cancel_pick: bool,
}

fn draw_rgb_color_control(
    ui: &mut egui::Ui,
    label: &str,
    rgb: &mut [u8; 3],
    target: RgbPickTarget,
    active_pick: Option<RgbPickTarget>,
) -> RgbColorControlResponse {
    let before = *rgb;
    let mut start_pick = None;
    let mut cancel_pick = false;
    let pick_active = active_pick == Some(target);
    ui.horizontal(|ui| {
        ui.label(egui::RichText::new(label).color(Color32::from_gray(190)));
        ui.label(
            egui::RichText::new(format!("#{:02X}{:02X}{:02X}", rgb[0], rgb[1], rgb[2]))
                .monospace()
                .color(Color32::from_gray(170)),
        );
        let response = ui.color_edit_button_srgb(rgb);
        response.lab_hover_tip(format!("{label}を選びます。"));
        let button_text = if pick_active {
            "スポイト解除"
        } else {
            "スポイト"
        };
        let pick_response = ui.selectable_label(pick_active, button_text);
        if pick_response.clicked() {
            if pick_active {
                cancel_pick = true;
            } else {
                start_pick = Some(target);
            }
        }
        pick_response.lab_hover_tip("画像上をクリックしてこの色へ取り込みます。");
    });
    ui.indent((label, "rgb_sliders"), |ui| {
        let mut r = rgb[0] as i32;
        let mut g = rgb[1] as i32;
        let mut b = rgb[2] as i32;
        let red = ui.add(egui::Slider::new(&mut r, 0..=255).text("R"));
        let green = ui.add(egui::Slider::new(&mut g, 0..=255).text("G"));
        let blue = ui.add(egui::Slider::new(&mut b, 0..=255).text("B"));
        if red.changed() || green.changed() || blue.changed() {
            *rgb = [r as u8, g as u8, b as u8];
        }
        red.lab_hover_tip("赤チャンネルです。");
        green.lab_hover_tip("緑チャンネルです。");
        blue.lab_hover_tip("青チャンネルです。");
    });
    RgbColorControlResponse {
        changed: *rgb != before,
        start_pick,
        cancel_pick,
    }
}

fn merge_rgb_color_response(
    response: RgbColorControlResponse,
    changed: &mut bool,
    start_rgb_pick: &mut Option<RgbPickTarget>,
    cancel_rgb_pick: &mut bool,
) {
    *changed |= response.changed;
    if response.cancel_pick {
        *cancel_rgb_pick = true;
    }
    if response.start_pick.is_some() {
        *start_rgb_pick = response.start_pick;
    }
}

fn duotone_preset_label(preset: DuotonePreset) -> &'static str {
    match preset {
        DuotonePreset::None => "選択してください",
        DuotonePreset::SepiaInk => "セピアインク",
        DuotonePreset::Cyanotype => "青写真",
        DuotonePreset::BlackRed => "黒赤",
        DuotonePreset::PurpleGold => "紫金",
        DuotonePreset::TealCream => "ティールクリーム",
        DuotonePreset::SunsetTritone => "夕暮れ3色",
        DuotonePreset::ComicTritone => "コミック3色",
        DuotonePreset::NoirTritone => "ノワール3色",
    }
}

fn preset_button(ui: &mut egui::Ui, label: &str) -> bool {
    ui.add(egui::Button::new(label).small()).clicked()
}

fn draw_tone_curve_preview(ui: &mut egui::Ui, params: ToneCurveParams) {
    draw_curve_preview_lines(
        ui,
        &[(
            params.points,
            Color32::from_rgb(120, 210, 255),
            egui::Stroke::new(2.0, Color32::from_rgb(120, 210, 255)),
        )],
    );
}

fn draw_rgb_tone_curve_preview(ui: &mut egui::Ui, params: RgbToneCurveParams) {
    draw_curve_preview_lines(
        ui,
        &[
            (
                params.master,
                Color32::from_rgb(230, 230, 230),
                egui::Stroke::new(1.5, Color32::from_rgb(230, 230, 230)),
            ),
            (
                params.red,
                Color32::from_rgb(255, 95, 115),
                egui::Stroke::new(2.0, Color32::from_rgb(255, 95, 115)),
            ),
            (
                params.green,
                Color32::from_rgb(95, 220, 120),
                egui::Stroke::new(2.0, Color32::from_rgb(95, 220, 120)),
            ),
            (
                params.blue,
                Color32::from_rgb(110, 150, 255),
                egui::Stroke::new(2.0, Color32::from_rgb(110, 150, 255)),
            ),
        ],
    );
}

fn draw_curve_preview_lines(ui: &mut egui::Ui, curves: &[([f32; 5], Color32, egui::Stroke)]) {
    let desired = egui::vec2(ui.available_width().min(220.0), 120.0);
    let (rect, _) = ui.allocate_exact_size(desired, Sense::hover());
    let painter = ui.painter_at(rect);
    painter.rect_filled(rect, 4.0, Color32::from_gray(24));
    painter.rect_stroke(
        rect,
        4.0,
        egui::Stroke::new(1.0, Color32::from_gray(70)),
        egui::StrokeKind::Inside,
    );
    for i in 1..4 {
        let t = i as f32 / 4.0;
        let x = egui::lerp(rect.left()..=rect.right(), t);
        let y = egui::lerp(rect.bottom()..=rect.top(), t);
        painter.line_segment(
            [Pos2::new(x, rect.top()), Pos2::new(x, rect.bottom())],
            egui::Stroke::new(1.0, Color32::from_gray(42)),
        );
        painter.line_segment(
            [Pos2::new(rect.left(), y), Pos2::new(rect.right(), y)],
            egui::Stroke::new(1.0, Color32::from_gray(42)),
        );
    }
    painter.line_segment(
        [rect.left_bottom(), rect.right_top()],
        egui::Stroke::new(1.0, Color32::from_gray(68)),
    );
    for &(points, _color, stroke) in curves {
        let mut prev = None;
        for i in 0..=64 {
            let x01 = i as f32 / 64.0;
            let y01 = preview_tone_curve_value(x01, points);
            let p = Pos2::new(
                egui::lerp(rect.left()..=rect.right(), x01),
                egui::lerp(rect.bottom()..=rect.top(), y01),
            );
            if let Some(prev) = prev {
                painter.line_segment([prev, p], stroke);
            }
            prev = Some(p);
        }
    }
}

fn preview_tone_curve_value(x: f32, points: [f32; 5]) -> f32 {
    let x = x.clamp(0.0, 1.0);
    let seg = ((x * 4.0).floor() as usize).min(3);
    let x0 = seg as f32 * 0.25;
    let t = ((x - x0) * 4.0).clamp(0.0, 1.0);
    points[seg].clamp(0.0, 1.0)
        + (points[seg + 1].clamp(0.0, 1.0) - points[seg].clamp(0.0, 1.0)) * t
}

fn draw_curve_point_sliders(ui: &mut egui::Ui, points: &mut [f32; 5]) -> bool {
    let mut changed = false;
    for (idx, label) in ["黒", "暗部", "中間", "明部", "白"].iter().enumerate() {
        let response = ui.add(egui::Slider::new(&mut points[idx], 0.0..=1.0).text(*label));
        changed |= response.changed();
        response.lab_hover_tip("左ほど暗部、右ほど明部の出力明るさです。");
    }
    changed
}

fn draw_color_balance_range_sliders(ui: &mut egui::Ui, range: &mut ColorBalanceRange) -> bool {
    let mut changed = false;
    let cyan_red =
        ui.add(egui::Slider::new(&mut range.cyan_red, -100.0..=100.0).text("シアン / 赤"));
    changed |= cyan_red.changed();
    cyan_red.lab_hover_tip("負の値でシアン寄り、正の値で赤寄りにします。");
    let magenta_green =
        ui.add(egui::Slider::new(&mut range.magenta_green, -100.0..=100.0).text("マゼンタ / 緑"));
    changed |= magenta_green.changed();
    magenta_green.lab_hover_tip("負の値でマゼンタ寄り、正の値で緑寄りにします。");
    let yellow_blue =
        ui.add(egui::Slider::new(&mut range.yellow_blue, -100.0..=100.0).text("黄 / 青"));
    changed |= yellow_blue.changed();
    yellow_blue.lab_hover_tip("負の値で黄寄り、正の値で青寄りにします。");
    changed
}

fn draw_color_grade_wheel_sliders(ui: &mut egui::Ui, wheel: &mut ColorGradeWheel) -> bool {
    let mut changed = false;
    let hue = ui.add(
        egui::Slider::new(&mut wheel.hue_degrees, 0.0..=360.0)
            .text("色相")
            .suffix("°"),
    );
    changed |= hue.changed();
    hue.lab_hover_tip("この明るさ帯に足す色味です。彩度が0のときは色相だけでは変化しません。");
    let saturation = ui.add(egui::Slider::new(&mut wheel.saturation, 0.0..=100.0).text("彩度"));
    changed |= saturation.changed();
    saturation.lab_hover_tip("色相で選んだ色味をどれだけ足すかを調整します。");
    let luminance = ui.add(egui::Slider::new(&mut wheel.luminance, -100.0..=100.0).text("明るさ"));
    changed |= luminance.changed();
    luminance.lab_hover_tip("この明るさ帯だけを明るく、または暗くします。");
    changed
}

fn draw_channel_coeff_sliders(ui: &mut egui::Ui, coeffs: &mut [f32; 3]) -> bool {
    let mut changed = false;
    let red = ui.add(egui::Slider::new(&mut coeffs[0], -200.0..=200.0).text("赤"));
    changed |= red.changed();
    red.lab_hover_tip("元画像の赤チャンネルをどれだけ混ぜるかです。100で等倍、0で不使用です。");
    let green = ui.add(egui::Slider::new(&mut coeffs[1], -200.0..=200.0).text("緑"));
    changed |= green.changed();
    green.lab_hover_tip("元画像の緑チャンネルをどれだけ混ぜるかです。");
    let blue = ui.add(egui::Slider::new(&mut coeffs[2], -200.0..=200.0).text("青"));
    changed |= blue.changed();
    blue.lab_hover_tip("元画像の青チャンネルをどれだけ混ぜるかです。負の値も使えます。");
    changed
}

#[derive(Debug, Default)]
pub(crate) struct EffectParamResponse {
    pub(crate) changed: bool,
    pub(crate) load_cube_lut: bool,
    pub(crate) start_selective_color_pick: bool,
    pub(crate) cancel_selective_color_pick: bool,
    pub(crate) start_rgb_pick: Option<RgbPickTarget>,
    pub(crate) cancel_rgb_pick: bool,
    pub(crate) set_effect_position_handles_visible: Option<bool>,
    pub(crate) copy_effect: bool,
    pub(crate) paste_effect: bool,
    pub(crate) reset_effect: bool,
}

fn draw_effect_position_handle_toggle(ui: &mut egui::Ui, visible: bool) -> Option<bool> {
    let mut show_handles = visible;
    let response = ui.checkbox(&mut show_handles, "画像ハンドルを表示");
    let changed = response.changed();
    response.lab_hover_tip("ONの間、画像上の位置ハンドルをドラッグして中心位置を調整できます。");
    changed.then_some(show_handles)
}

fn draw_effect_center_controls(
    ui: &mut egui::Ui,
    center: &mut [f32; 2],
    x_tip: impl Into<egui::WidgetText>,
    y_tip: impl Into<egui::WidgetText>,
    effect_position_handles_visible: bool,
    set_effect_position_handles_visible: &mut Option<bool>,
) -> bool {
    if let Some(visible) = draw_effect_position_handle_toggle(ui, effect_position_handles_visible) {
        *set_effect_position_handles_visible = Some(visible);
    }

    let mut changed = false;
    let center_x = ui.add(egui::Slider::new(&mut center[0], 0.0..=1.0).text("中心 X"));
    changed |= center_x.changed();
    center_x.lab_hover_tip(x_tip);
    let center_y = ui.add(egui::Slider::new(&mut center[1], 0.0..=1.0).text("中心 Y"));
    changed |= center_y.changed();
    center_y.lab_hover_tip(y_tip);
    changed
}

macro_rules! tone_detail_effect_patterns {
    () => {
        LocalEffect::Tone(_)
            | LocalEffect::ToneCurve(_)
            | LocalEffect::RgbToneCurve(_)
            | LocalEffect::ColorBalance(_)
            | LocalEffect::PhotoFilter(_)
            | LocalEffect::ThreeWayColorGrading(_)
            | LocalEffect::SelectiveColor(_)
            | LocalEffect::PartColor(_)
            | LocalEffect::ChannelMixer(_)
            | LocalEffect::MonochromeMixer(_)
            | LocalEffect::Clarity(_)
            | LocalEffect::Texture(_)
            | LocalEffect::HighPass(_)
            | LocalEffect::FrequencySeparation(_)
            | LocalEffect::HighlightsShadows(_)
            | LocalEffect::Dehaze(_)
    };
}

fn is_tone_detail_effect(effect: &LocalEffect) -> bool {
    matches!(effect, tone_detail_effect_patterns!())
}

fn draw_tone_detail_effect_params(
    ui: &mut egui::Ui,
    effect: &mut LocalEffect,
    selective_color_pick_active: bool,
    rgb_pick_active: Option<RgbPickTarget>,
) -> EffectParamResponse {
    let mut changed = false;
    let mut start_selective_color_pick = false;
    let mut cancel_selective_color_pick = false;
    let mut start_rgb_pick = None;
    let mut cancel_rgb_pick = false;

    match effect {
        LocalEffect::Tone(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "明るく") {
                    *params = ToneParams {
                        brightness: 12.0,
                        contrast: 4.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "鮮やか") {
                    *params = ToneParams {
                        saturation: 18.0,
                        vibrance: 32.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "自然な彩度+") {
                    *params = ToneParams {
                        vibrance: 45.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかく") {
                    *params = ToneParams {
                        contrast: -10.0,
                        vibrance: 12.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "暖かく") {
                    *params = ToneParams {
                        temperature: 35.0,
                        vibrance: 12.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "緑かぶり補正") {
                    *params = ToneParams {
                        tint: 28.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "マゼンタ補正") {
                    *params = ToneParams {
                        tint: -28.0,
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.brightness, -100.0..=100.0).text("明るさ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.contrast, -100.0..=100.0).text("コントラスト"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.gamma, 0.2..=5.0).text("ガンマ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.saturation, -100.0..=100.0).text("彩度"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.vibrance, -100.0..=100.0).text("自然な彩度"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.temperature, -100.0..=100.0).text("色温度"))
                .changed();
            let tint_response = ui.add(
                egui::Slider::new(&mut params.tint, -100.0..=100.0)
                    .text("色かぶり補正")
                    .custom_formatter(|v, _| {
                        if v.abs() < 0.5 {
                            "0".to_string()
                        } else if v > 0.0 {
                            format!("マゼンタ {:.0}", v)
                        } else {
                            format!("緑 {:.0}", -v)
                        }
                    }),
            );
            changed |= tint_response.changed();
            tint_response.lab_hover_tip(
                "緑-マゼンタ方向の色かぶりを補正します。右へ動かすとマゼンタ寄り、左へ動かすと緑寄りになります。",
            );
        }
        LocalEffect::ToneCurve(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "S字") {
                    *params = ToneCurveParams {
                        points: [0.0, 0.18, 0.50, 0.82, 1.0],
                    };
                    changed = true;
                }
                if preset_button(ui, "明るく") {
                    *params = ToneCurveParams {
                        points: [0.0, 0.34, 0.62, 0.86, 1.0],
                    };
                    changed = true;
                }
                if preset_button(ui, "暗く") {
                    *params = ToneCurveParams {
                        points: [0.0, 0.16, 0.40, 0.68, 1.0],
                    };
                    changed = true;
                }
                if preset_button(ui, "フェード") {
                    *params = ToneCurveParams {
                        points: [0.08, 0.28, 0.52, 0.76, 0.96],
                    };
                    changed = true;
                }
            });
            draw_tone_curve_preview(ui, *params);
            ui.label(
                egui::RichText::new(
                    "RGB共通の簡易カーブです。色チャンネルは RGBカーブ で調整します。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            for (idx, label) in ["黒", "暗部", "中間", "明部", "白"].iter().enumerate() {
                changed |= ui
                    .add(egui::Slider::new(&mut params.points[idx], 0.0..=1.0).text(*label))
                    .changed();
            }
        }
        LocalEffect::RgbToneCurve(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "暖色") {
                    *params = RgbToneCurveParams {
                        red: [0.0, 0.30, 0.58, 0.82, 1.0],
                        blue: [0.0, 0.20, 0.44, 0.70, 1.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "寒色") {
                    *params = RgbToneCurveParams {
                        red: [0.0, 0.20, 0.44, 0.70, 1.0],
                        blue: [0.0, 0.31, 0.60, 0.84, 1.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "フィルム") {
                    *params = RgbToneCurveParams {
                        master: [0.06, 0.24, 0.50, 0.76, 0.96],
                        red: [0.0, 0.25, 0.53, 0.82, 1.0],
                        blue: [0.06, 0.30, 0.52, 0.72, 0.95],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "影を青く") {
                    *params = RgbToneCurveParams {
                        red: [0.0, 0.18, 0.46, 0.75, 1.0],
                        blue: [0.08, 0.34, 0.54, 0.76, 1.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "明部を暖かく") {
                    *params = RgbToneCurveParams {
                        red: [0.0, 0.25, 0.52, 0.84, 1.0],
                        green: [0.0, 0.25, 0.51, 0.78, 1.0],
                        blue: [0.0, 0.25, 0.48, 0.66, 0.94],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "クロス") {
                    *params = RgbToneCurveParams {
                        master: [0.04, 0.22, 0.50, 0.78, 0.98],
                        red: [0.0, 0.20, 0.48, 0.82, 1.0],
                        green: [0.0, 0.27, 0.52, 0.74, 1.0],
                        blue: [0.08, 0.34, 0.54, 0.72, 0.94],
                    };
                    changed = true;
                }
            });
            draw_rgb_tone_curve_preview(ui, *params);
            ui.label(
                egui::RichText::new(
                    "白い線が全体、赤/緑/青の線が各チャンネルです。全体カーブ後に各チャンネルを適用します。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.collapsing("全体", |ui| {
                changed |= draw_curve_point_sliders(ui, &mut params.master);
            });
            ui.collapsing("赤", |ui| {
                changed |= draw_curve_point_sliders(ui, &mut params.red);
            });
            ui.collapsing("緑", |ui| {
                changed |= draw_curve_point_sliders(ui, &mut params.green);
            });
            ui.collapsing("青", |ui| {
                changed |= draw_curve_point_sliders(ui, &mut params.blue);
            });
        }
        LocalEffect::ColorBalance(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "影を青く") {
                    *params = ColorBalanceParams {
                        shadows: ColorBalanceRange {
                            yellow_blue: 42.0,
                            cyan_red: -10.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "影を青緑") {
                    *params = ColorBalanceParams {
                        shadows: ColorBalanceRange {
                            cyan_red: -28.0,
                            magenta_green: 12.0,
                            yellow_blue: 30.0,
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "明部を暖かく") {
                    *params = ColorBalanceParams {
                        highlights: ColorBalanceRange {
                            cyan_red: 24.0,
                            yellow_blue: -34.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "夕景") {
                    *params = ColorBalanceParams {
                        shadows: ColorBalanceRange {
                            yellow_blue: 16.0,
                            ..Default::default()
                        },
                        midtones: ColorBalanceRange {
                            cyan_red: 12.0,
                            yellow_blue: -14.0,
                            ..Default::default()
                        },
                        highlights: ColorBalanceRange {
                            cyan_red: 30.0,
                            yellow_blue: -42.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "緑かぶり補正") {
                    *params = ColorBalanceParams {
                        midtones: ColorBalanceRange {
                            magenta_green: -26.0,
                            ..Default::default()
                        },
                        highlights: ColorBalanceRange {
                            magenta_green: -12.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "シネマ") {
                    *params = ColorBalanceParams {
                        shadows: ColorBalanceRange {
                            cyan_red: -30.0,
                            yellow_blue: 26.0,
                            ..Default::default()
                        },
                        highlights: ColorBalanceRange {
                            cyan_red: 24.0,
                            yellow_blue: -24.0,
                            ..Default::default()
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明るさの帯ごとに色を寄せます。RGBカーブより直感的に色かぶりや空気感を調整できます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.collapsing("シャドウ", |ui| {
                changed |= draw_color_balance_range_sliders(ui, &mut params.shadows);
            });
            ui.collapsing("中間", |ui| {
                changed |= draw_color_balance_range_sliders(ui, &mut params.midtones);
            });
            ui.collapsing("ハイライト", |ui| {
                changed |= draw_color_balance_range_sliders(ui, &mut params.highlights);
            });
            let preserve = ui.checkbox(&mut params.preserve_luma, "明るさを保つ");
            changed |= preserve.changed();
            preserve.lab_hover_tip(
                "色だけを寄せたいときに使います。オフにすると色変更による明るさ変化も残します。",
            );
        }
        LocalEffect::PhotoFilter(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "Warm 85") {
                    *params = PhotoFilterParams {
                        preset: PhotoFilterPreset::Warm85,
                        density: 0.35,
                        strength: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "Cool 80") {
                    *params = PhotoFilterParams {
                        preset: PhotoFilterPreset::Cool80,
                        density: 0.32,
                        strength: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "セピア") {
                    *params = PhotoFilterParams {
                        preset: PhotoFilterPreset::Sepia,
                        density: 0.42,
                        strength: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "夕景") {
                    *params = PhotoFilterParams {
                        preset: PhotoFilterPreset::Sunset,
                        density: 0.45,
                        strength: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "水中") {
                    *params = PhotoFilterParams {
                        preset: PhotoFilterPreset::Underwater,
                        density: 0.38,
                        strength: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "カスタム") {
                    *params = PhotoFilterParams {
                        preset: PhotoFilterPreset::Custom,
                        density: 0.35,
                        strength: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            let before_preset = params.preset;
            lab_combo_box(
                ui,
                "photo_filter_preset",
                photo_filter_preset_label(params.preset),
                |ui| {
                    for preset in [
                        PhotoFilterPreset::Custom,
                        PhotoFilterPreset::Warm85,
                        PhotoFilterPreset::Warm81,
                        PhotoFilterPreset::Cool80,
                        PhotoFilterPreset::Cool82,
                        PhotoFilterPreset::Sepia,
                        PhotoFilterPreset::Sunset,
                        PhotoFilterPreset::Underwater,
                        PhotoFilterPreset::Magenta,
                        PhotoFilterPreset::Green,
                    ] {
                        ui.selectable_value(
                            &mut params.preset,
                            preset,
                            photo_filter_preset_label(preset),
                        );
                    }
                },
            );
            if params.preset != before_preset {
                if params.strength <= f32::EPSILON {
                    params.strength = 1.0;
                }
                changed = true;
            }
            ui.label(
                egui::RichText::new(
                    "色付きフィルターをかぶせる感覚で、暖色・寒色・セピアなどの色かぶりや雰囲気を足します。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            if params.preset == PhotoFilterPreset::Custom {
                merge_rgb_color_response(
                    draw_rgb_color_control(
                        ui,
                        "フィルター色",
                        &mut params.color_rgb,
                        RgbPickTarget::PhotoFilterColor,
                        rgb_pick_active,
                    ),
                    &mut changed,
                    &mut start_rgb_pick,
                    &mut cancel_rgb_pick,
                );
            }
            let density = ui.add(egui::Slider::new(&mut params.density, 0.0..=1.0).text("濃度"));
            changed |= density.changed();
            density.lab_hover_tip("フィルター色をどれだけ強くかぶせるかです。");
            let preserve = ui.checkbox(&mut params.preserve_luminosity, "明るさを保つ");
            changed |= preserve.changed();
            preserve.lab_hover_tip(
                "ONにすると、色味だけを変えて元の明るさに近づけます。OFFではフィルター色の明暗も反映します。",
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からフォトフィルター後の色へどれだけ近づけるかです。");
        }
        LocalEffect::ThreeWayColorGrading(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "シネマ") {
                    *params = ThreeWayColorGradingParams {
                        shadows: ColorGradeWheel {
                            hue_degrees: 205.0,
                            saturation: 42.0,
                            luminance: -8.0,
                        },
                        highlights: ColorGradeWheel {
                            hue_degrees: 36.0,
                            saturation: 36.0,
                            luminance: 6.0,
                        },
                        balance: 0.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "夕焼け") {
                    *params = ThreeWayColorGradingParams {
                        shadows: ColorGradeWheel {
                            hue_degrees: 250.0,
                            saturation: 22.0,
                            luminance: -4.0,
                        },
                        midtones: ColorGradeWheel {
                            hue_degrees: 18.0,
                            saturation: 18.0,
                            luminance: 2.0,
                        },
                        highlights: ColorGradeWheel {
                            hue_degrees: 42.0,
                            saturation: 48.0,
                            luminance: 8.0,
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "月明かり") {
                    *params = ThreeWayColorGradingParams {
                        shadows: ColorGradeWheel {
                            hue_degrees: 220.0,
                            saturation: 34.0,
                            luminance: -6.0,
                        },
                        midtones: ColorGradeWheel {
                            hue_degrees: 210.0,
                            saturation: 16.0,
                            luminance: -2.0,
                        },
                        highlights: ColorGradeWheel {
                            hue_degrees: 190.0,
                            saturation: 12.0,
                            luminance: 5.0,
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "桜色") {
                    *params = ThreeWayColorGradingParams {
                        midtones: ColorGradeWheel {
                            hue_degrees: 335.0,
                            saturation: 20.0,
                            luminance: 3.0,
                        },
                        highlights: ColorGradeWheel {
                            hue_degrees: 350.0,
                            saturation: 24.0,
                            luminance: 8.0,
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "サイバー") {
                    *params = ThreeWayColorGradingParams {
                        shadows: ColorGradeWheel {
                            hue_degrees: 270.0,
                            saturation: 36.0,
                            luminance: -4.0,
                        },
                        midtones: ColorGradeWheel {
                            hue_degrees: 190.0,
                            saturation: 18.0,
                            luminance: 0.0,
                        },
                        highlights: ColorGradeWheel {
                            hue_degrees: 315.0,
                            saturation: 34.0,
                            luminance: 7.0,
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "淡い光") {
                    *params = ThreeWayColorGradingParams {
                        shadows: ColorGradeWheel {
                            hue_degrees: 225.0,
                            saturation: 12.0,
                            luminance: 4.0,
                        },
                        midtones: ColorGradeWheel {
                            hue_degrees: 32.0,
                            saturation: 10.0,
                            luminance: 6.0,
                        },
                        highlights: ColorGradeWheel {
                            hue_degrees: 48.0,
                            saturation: 18.0,
                            luminance: 12.0,
                        },
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "カラーバランスより演出的な仕上げ向けです。色相と彩度で足す色を選び、明るさで帯ごとの持ち上げ/締めを調整します。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.collapsing("シャドウ", |ui| {
                changed |= draw_color_grade_wheel_sliders(ui, &mut params.shadows);
            });
            ui.collapsing("中間", |ui| {
                changed |= draw_color_grade_wheel_sliders(ui, &mut params.midtones);
            });
            ui.collapsing("ハイライト", |ui| {
                changed |= draw_color_grade_wheel_sliders(ui, &mut params.highlights);
            });
            let balance =
                ui.add(egui::Slider::new(&mut params.balance, -100.0..=100.0).text("バランス"));
            changed |= balance.changed();
            balance.lab_hover_tip(
                "負の値でシャドウ寄り、正の値でハイライト寄りに効果範囲をずらします。",
            );
        }
        LocalEffect::SelectiveColor(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "赤を桜色") {
                    *params = SelectiveColorParams {
                        target_hue_degrees: 0.0,
                        range_degrees: 18.0,
                        feather_degrees: 18.0,
                        hue_degrees: 18.0,
                        saturation: -12.0,
                        lightness: 10.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "肌を明るく") {
                    *params = SelectiveColorParams {
                        target_hue_degrees: 28.0,
                        range_degrees: 24.0,
                        feather_degrees: 24.0,
                        saturation: 4.0,
                        lightness: 12.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "空を青く") {
                    *params = SelectiveColorParams {
                        target_hue_degrees: 205.0,
                        range_degrees: 28.0,
                        feather_degrees: 24.0,
                        saturation: 30.0,
                        lightness: -8.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "緑を鮮やか") {
                    *params = SelectiveColorParams {
                        target_hue_degrees: 120.0,
                        range_degrees: 28.0,
                        feather_degrees: 24.0,
                        saturation: 34.0,
                        lightness: 4.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "青を紫へ") {
                    *params = SelectiveColorParams {
                        target_hue_degrees: 235.0,
                        range_degrees: 26.0,
                        feather_degrees: 22.0,
                        hue_degrees: 35.0,
                        saturation: 12.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "黄を橙へ") {
                    *params = SelectiveColorParams {
                        target_hue_degrees: 58.0,
                        range_degrees: 24.0,
                        feather_degrees: 18.0,
                        hue_degrees: -18.0,
                        saturation: 12.0,
                        lightness: -2.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "対象色相に近い色だけを補正します。色が広く変わりすぎる場合は範囲やぼかしを小さくしてください。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let swatch = hsl_swatch_color(params.target_hue_degrees, 0.8, 0.55);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), Sense::hover());
                ui.painter().rect_filled(rect, 4.0, swatch);
                ui.painter().rect_stroke(
                    rect,
                    4.0,
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 110)),
                    egui::StrokeKind::Inside,
                );
                let label = if selective_color_pick_active {
                    "スポイト解除"
                } else {
                    "スポイトで対象色を取得"
                };
                let response = ui.button(label);
                if response.clicked() {
                    if selective_color_pick_active {
                        cancel_selective_color_pick = true;
                    } else {
                        start_selective_color_pick = true;
                    }
                }
                response.lab_hover_tip(
                    "画像上をクリックしたピクセルの色相を、対象色相として設定します。",
                );
            });
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "対象: 赤") {
                    params.target_hue_degrees = 0.0;
                    changed = true;
                }
                if preset_button(ui, "対象: 肌") {
                    params.target_hue_degrees = 28.0;
                    changed = true;
                }
                if preset_button(ui, "対象: 黄") {
                    params.target_hue_degrees = 58.0;
                    changed = true;
                }
                if preset_button(ui, "対象: 緑") {
                    params.target_hue_degrees = 120.0;
                    changed = true;
                }
                if preset_button(ui, "対象: 空") {
                    params.target_hue_degrees = 205.0;
                    changed = true;
                }
                if preset_button(ui, "対象: 青") {
                    params.target_hue_degrees = 235.0;
                    changed = true;
                }
                if preset_button(ui, "対象: 紫") {
                    params.target_hue_degrees = 285.0;
                    changed = true;
                }
            });
            let target = ui.add(
                egui::Slider::new(&mut params.target_hue_degrees, 0.0..=360.0)
                    .text("対象色相")
                    .suffix("°"),
            );
            changed |= target.changed();
            target.lab_hover_tip(
                "補正したい色の中心です。赤は0°、黄は60°、緑は120°、青は240°付近です。",
            );
            let range =
                ui.add(egui::Slider::new(&mut params.range_degrees, 1.0..=90.0).text("範囲"));
            changed |= range.changed();
            range.lab_hover_tip("この角度以内の色は強く補正します。小さいほど一点狙いになります。");
            let feather =
                ui.add(egui::Slider::new(&mut params.feather_degrees, 0.0..=90.0).text("ぼかし"));
            changed |= feather.changed();
            feather.lab_hover_tip("範囲の外側へ、どれだけなだらかに効果を弱めるかです。");
            let hue = ui.add(
                egui::Slider::new(&mut params.hue_degrees, -180.0..=180.0)
                    .text("色相補正")
                    .suffix("°"),
            );
            changed |= hue.changed();
            hue.lab_hover_tip("対象色だけ色相をずらします。");
            let saturation =
                ui.add(egui::Slider::new(&mut params.saturation, -100.0..=100.0).text("彩度"));
            changed |= saturation.changed();
            saturation.lab_hover_tip("対象色だけ鮮やかさを増減します。");
            let lightness =
                ui.add(egui::Slider::new(&mut params.lightness, -100.0..=100.0).text("明度"));
            changed |= lightness.changed();
            lightness.lab_hover_tip("対象色だけ明るさを増減します。");
        }
        LocalEffect::PartColor(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "赤だけ") {
                    *params = PartColorParams {
                        target_rgb: [220, 40, 40],
                        range_degrees: 20.0,
                        feather_degrees: 18.0,
                        gray_strength: 1.0,
                        selected_saturation: 18.0,
                        selected_lightness: 2.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "肌だけ") {
                    *params = PartColorParams {
                        target_rgb: [230, 150, 105],
                        range_degrees: 30.0,
                        feather_degrees: 28.0,
                        gray_strength: 0.86,
                        selected_saturation: 8.0,
                        selected_lightness: 5.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "青だけ") {
                    *params = PartColorParams {
                        target_rgb: [50, 115, 230],
                        range_degrees: 26.0,
                        feather_degrees: 24.0,
                        gray_strength: 1.0,
                        selected_saturation: 22.0,
                        selected_lightness: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "緑だけ") {
                    *params = PartColorParams {
                        target_rgb: [58, 170, 82],
                        range_degrees: 28.0,
                        feather_degrees: 24.0,
                        gray_strength: 0.95,
                        selected_saturation: 20.0,
                        selected_lightness: 2.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡く残す") {
                    *params = PartColorParams {
                        target_rgb: [220, 80, 160],
                        range_degrees: 34.0,
                        feather_degrees: 34.0,
                        gray_strength: 0.70,
                        selected_saturation: 6.0,
                        selected_lightness: 4.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "対象色に近い色だけを残し、それ以外を白黒へ寄せます。色が抜けすぎる場合は範囲やぼかしを広げてください。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let response = draw_rgb_color_control(
                ui,
                "対象色",
                &mut params.target_rgb,
                RgbPickTarget::PartColorTarget,
                rgb_pick_active,
            );
            merge_rgb_color_response(
                response,
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let hue = hue_degrees_from_rgb(params.target_rgb);
            ui.label(
                egui::RichText::new(format!("対象色相: {hue:.0}°"))
                    .size(10.0)
                    .color(Color32::from_gray(160)),
            );
            let range =
                ui.add(egui::Slider::new(&mut params.range_degrees, 1.0..=120.0).text("残す範囲"));
            changed |= range.changed();
            range.lab_hover_tip("対象色として強く残す色相範囲です。小さいほど一点狙いになります。");
            let feather = ui.add(
                egui::Slider::new(&mut params.feather_degrees, 0.0..=120.0).text("境界ぼかし"),
            );
            changed |= feather.changed();
            feather.lab_hover_tip("対象色の外側へ、色残しをどれだけなだらかに弱めるかです。");
            let gray =
                ui.add(egui::Slider::new(&mut params.gray_strength, 0.0..=1.0).text("グレー化"));
            changed |= gray.changed();
            gray.lab_hover_tip("対象色から外れた色を白黒へ寄せる強さです。");
            let saturation = ui.add(
                egui::Slider::new(&mut params.selected_saturation, -80.0..=100.0)
                    .text("対象色の彩度"),
            );
            changed |= saturation.changed();
            saturation.lab_hover_tip("残した対象色だけ、鮮やかさを少し整えます。");
            let lightness = ui.add(
                egui::Slider::new(&mut params.selected_lightness, -50.0..=50.0)
                    .text("対象色の明度"),
            );
            changed |= lightness.changed();
            lightness.lab_hover_tip("残した対象色だけ、明るさを少し整えます。");
        }
        LocalEffect::ChannelMixer(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "白黒標準") {
                    *params = ChannelMixerParams {
                        monochrome: true,
                        mono_output: [30.0, 59.0, 11.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "赤フィルター") {
                    *params = ChannelMixerParams {
                        monochrome: true,
                        mono_output: [75.0, 25.0, 0.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "緑フィルター") {
                    *params = ChannelMixerParams {
                        monochrome: true,
                        mono_output: [15.0, 75.0, 10.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "青フィルター") {
                    *params = ChannelMixerParams {
                        monochrome: true,
                        mono_output: [5.0, 35.0, 60.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "赤青入替") {
                    *params = ChannelMixerParams {
                        red_output: [0.0, 0.0, 100.0],
                        green_output: [0.0, 100.0, 0.0],
                        blue_output: [100.0, 0.0, 0.0],
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "暖色ブースト") {
                    *params = ChannelMixerParams {
                        red_output: [115.0, 8.0, 0.0],
                        green_output: [0.0, 100.0, 0.0],
                        blue_output: [0.0, 0.0, 82.0],
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "白黒化では元画像の赤/緑/青をどれだけ明度へ混ぜるかを調整します。カラー時は各出力チャンネルの混合率を直接編集します。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let mono = ui.checkbox(&mut params.monochrome, "白黒化");
            changed |= mono.changed();
            mono.lab_hover_tip("オンにすると、赤/緑/青の寄与率から1枚のグレー画像を作ります。");
            if params.monochrome {
                ui.collapsing("白黒の寄与率", |ui| {
                    changed |= draw_channel_coeff_sliders(ui, &mut params.mono_output);
                });
            } else {
                ui.collapsing("赤出力", |ui| {
                    changed |= draw_channel_coeff_sliders(ui, &mut params.red_output);
                });
                ui.collapsing("緑出力", |ui| {
                    changed |= draw_channel_coeff_sliders(ui, &mut params.green_output);
                });
                ui.collapsing("青出力", |ui| {
                    changed |= draw_channel_coeff_sliders(ui, &mut params.blue_output);
                });
            }
        }
        LocalEffect::MonochromeMixer(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "標準白黒") {
                    *params = MonochromeMixerParams {
                        strength: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "赤フィルター") {
                    *params = MonochromeMixerParams {
                        red: 65.0,
                        yellow: 28.0,
                        cyan: -18.0,
                        blue: -70.0,
                        contrast: 10.0,
                        strength: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "緑フィルター") {
                    *params = MonochromeMixerParams {
                        red: -12.0,
                        green: 58.0,
                        cyan: 18.0,
                        magenta: -28.0,
                        contrast: 8.0,
                        strength: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "青空濃く") {
                    *params = MonochromeMixerParams {
                        red: 24.0,
                        yellow: 22.0,
                        cyan: -25.0,
                        blue: -58.0,
                        contrast: 16.0,
                        strength: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "セピア") {
                    *params = MonochromeMixerParams {
                        red: 12.0,
                        yellow: 8.0,
                        blue: -16.0,
                        tint_rgb: [196, 132, 68],
                        tint_strength: 0.42,
                        contrast: 6.0,
                        strength: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "色ごとの明度を調整して白黒化します。赤フィルターは肌や赤を明るく、青空を暗くするような使い方に向いています。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let mut activates_effect = false;
            let red = ui.add(egui::Slider::new(&mut params.red, -100.0..=100.0).text("赤の明るさ"));
            changed |= red.changed();
            activates_effect |= red.changed();
            red.lab_hover_tip("赤系の色を白黒変換したときの明るさです。");
            let yellow =
                ui.add(egui::Slider::new(&mut params.yellow, -100.0..=100.0).text("黄の明るさ"));
            changed |= yellow.changed();
            activates_effect |= yellow.changed();
            yellow.lab_hover_tip("黄系の色を白黒変換したときの明るさです。");
            let green =
                ui.add(egui::Slider::new(&mut params.green, -100.0..=100.0).text("緑の明るさ"));
            changed |= green.changed();
            activates_effect |= green.changed();
            green.lab_hover_tip("緑系の色を白黒変換したときの明るさです。");
            let cyan =
                ui.add(egui::Slider::new(&mut params.cyan, -100.0..=100.0).text("シアンの明るさ"));
            changed |= cyan.changed();
            activates_effect |= cyan.changed();
            cyan.lab_hover_tip("シアン系の色を白黒変換したときの明るさです。");
            let blue =
                ui.add(egui::Slider::new(&mut params.blue, -100.0..=100.0).text("青の明るさ"));
            changed |= blue.changed();
            activates_effect |= blue.changed();
            blue.lab_hover_tip("青系の色を白黒変換したときの明るさです。");
            let magenta = ui.add(
                egui::Slider::new(&mut params.magenta, -100.0..=100.0).text("マゼンタの明るさ"),
            );
            changed |= magenta.changed();
            activates_effect |= magenta.changed();
            magenta.lab_hover_tip("マゼンタ系の色を白黒変換したときの明るさです。");
            let contrast = ui
                .add(egui::Slider::new(&mut params.contrast, -100.0..=100.0).text("コントラスト"));
            changed |= contrast.changed();
            activates_effect |= contrast.changed();
            contrast.lab_hover_tip("白黒化した明暗のコントラストを調整します。");
            let tint =
                ui.add(egui::Slider::new(&mut params.tint_strength, 0.0..=1.0).text("色調を足す"));
            changed |= tint.changed();
            activates_effect |= tint.changed();
            tint.lab_hover_tip("白黒画像へセピアなどの色味を加える量です。");
            let tint_response = draw_rgb_color_control(
                ui,
                "色調",
                &mut params.tint_rgb,
                RgbPickTarget::MonochromeMixerTint,
                rgb_pick_active,
            );
            if tint_response.changed {
                activates_effect = true;
                if params.tint_strength <= f32::EPSILON {
                    params.tint_strength = 0.35;
                }
            }
            merge_rgb_color_response(
                tint_response,
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            if activates_effect && params.strength <= f32::EPSILON {
                params.strength = 1.0;
                changed = true;
            }
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からモノクロミキサー結果へ切り替える強さです。");
        }
        LocalEffect::Clarity(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "くっきり") {
                    *params = ClarityParams {
                        amount: 0.35,
                        radius_px: 18.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかく") {
                    *params = ClarityParams {
                        amount: -0.35,
                        radius_px: 20.0,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.amount, -1.0..=1.0).text("量"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.radius_px, 1.0..=80.0).text("半径"))
                .changed();
        }
        LocalEffect::Texture(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "質感+") {
                    *params = TextureParams {
                        amount: 0.45,
                        radius_px: 10.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "質感-") {
                    *params = TextureParams {
                        amount: -0.45,
                        radius_px: 10.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "塗り面なめらか") {
                    *params = TextureParams {
                        amount: -0.65,
                        radius_px: 7.0,
                    };
                    changed = true;
                }
            });
            let amount = ui.add(egui::Slider::new(&mut params.amount, -1.0..=1.0).text("量"));
            changed |= amount.changed();
            amount.lab_hover_tip("正で中くらいの細部を強め、負でざらつきを抑えます。");
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 2.0..=40.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip("拾う質感の大きさです。大きい値ほど広めの凹凸を対象にします。");
        }
        LocalEffect::HighPass(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱く") {
                    *params = HighPassParams {
                        amount: 0.45,
                        radius_px: 8.0,
                        contrast: 1.0,
                        detail_only: false,
                    };
                    changed = true;
                }
                if preset_button(ui, "くっきり") {
                    *params = HighPassParams {
                        amount: 0.85,
                        radius_px: 6.0,
                        contrast: 1.2,
                        detail_only: false,
                    };
                    changed = true;
                }
                if preset_button(ui, "線/細部") {
                    *params = HighPassParams {
                        amount: 1.2,
                        radius_px: 3.0,
                        contrast: 1.6,
                        detail_only: false,
                    };
                    changed = true;
                }
                if preset_button(ui, "抽出表示") {
                    *params = HighPassParams {
                        amount: 0.0,
                        radius_px: 8.0,
                        contrast: 1.4,
                        detail_only: true,
                    };
                    changed = true;
                }
            });
            let detail_only = ui.checkbox(&mut params.detail_only, "抽出だけ表示");
            changed |= detail_only.changed();
            detail_only.lab_hover_tip(
                "ONにすると、元画像に合成せず中間グレー上のディテール抽出結果を表示します。",
            );
            let amount = ui.add_enabled(
                !params.detail_only,
                egui::Slider::new(&mut params.amount, 0.0..=2.0).text("量"),
            );
            changed |= amount.changed();
            amount.lab_hover_tip("ハイパス抽出をオーバーレイ合成する強さです。");
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 1.0..=60.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip("大きい値ほど広い輪郭、小さい値ほど細部を抽出します。");
            let contrast = ui
                .add(egui::Slider::new(&mut params.contrast, 0.25..=4.0).text("抽出コントラスト"));
            changed |= contrast.changed();
            contrast.lab_hover_tip("抽出したディテールを中間グレーからどれだけ離すかです。");
        }
        LocalEffect::FrequencySeparation(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "塗り面なめらか") {
                    *params = FrequencySeparationParams {
                        radius_px: 14.0,
                        low_smoothing: 0.24,
                        detail_amount: 0.55,
                        detail_contrast: 1.0,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "色むら均し") {
                    *params = FrequencySeparationParams {
                        radius_px: 26.0,
                        low_smoothing: 0.42,
                        detail_amount: 1.0,
                        detail_contrast: 1.0,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "質感だけ抑える") {
                    *params = FrequencySeparationParams {
                        radius_px: 10.0,
                        low_smoothing: 0.0,
                        detail_amount: 0.35,
                        detail_contrast: 1.0,
                        strength: 0.9,
                    };
                    changed = true;
                }
                if preset_button(ui, "細部くっきり") {
                    *params = FrequencySeparationParams {
                        radius_px: 8.0,
                        low_smoothing: 0.0,
                        detail_amount: 1.35,
                        detail_contrast: 1.25,
                        strength: 0.75,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "低周波の色/明暗と高周波の質感を分けて再合成します。肌や塗り面のざらつき、スキャンの色むら補修に使います。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 1.0..=80.0)
                    .text("分離半径")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius.lab_hover_tip(
                "低周波と質感を分ける大きさです。小さい値は細部、大きい値は広い色むらに効きます。",
            );
            let low_smoothing =
                ui.add(egui::Slider::new(&mut params.low_smoothing, 0.0..=1.0).text("色むら均し"));
            changed |= low_smoothing.changed();
            low_smoothing.lab_hover_tip(
                "低周波レイヤーをさらに均して、広めの色むらや明暗むらを目立ちにくくします。",
            );
            let detail_amount =
                ui.add(egui::Slider::new(&mut params.detail_amount, 0.0..=2.0).text("質感量"));
            changed |= detail_amount.changed();
            detail_amount
                .lab_hover_tip("1.0で元の質感、下げると細かい質感を抑え、上げると細部を強めます。");
            let detail_contrast = ui.add(
                egui::Slider::new(&mut params.detail_contrast, 0.25..=2.0).text("質感コントラスト"),
            );
            changed |= detail_contrast.changed();
            detail_contrast.lab_hover_tip("抽出した質感の濃淡をどれだけ強く再合成するかです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("周波数分離で再合成した結果へ近づける強さです。");
        }
        LocalEffect::HighlightsShadows(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "シャドウを強調") {
                    *params = HighlightsShadowsParams {
                        shadows: -35.0,
                        highlights: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "シャドウを明るく") {
                    *params = HighlightsShadowsParams {
                        shadows: 45.0,
                        highlights: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ハイライトを強調") {
                    *params = HighlightsShadowsParams {
                        shadows: 0.0,
                        highlights: -30.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ハイライトを暗く") {
                    *params = HighlightsShadowsParams {
                        shadows: 0.0,
                        highlights: 35.0,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.shadows, -100.0..=100.0).text("シャドウ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.highlights, -100.0..=100.0).text("ハイライト"))
                .changed();
        }
        LocalEffect::Dehaze(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱く") {
                    *params = DehazeParams {
                        amount: 0.25,
                        radius_px: 10.0,
                        min_transmission: 0.38,
                        saturation: 4.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "標準") {
                    *params = DehazeParams {
                        amount: 0.45,
                        radius_px: 14.0,
                        min_transmission: 0.32,
                        saturation: 8.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "強く") {
                    *params = DehazeParams {
                        amount: 0.70,
                        radius_px: 20.0,
                        min_transmission: 0.25,
                        saturation: 10.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "写真向けの霧・白っぽさ低減です。AI絵では弱めから確認してください。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            changed |= ui
                .add(egui::Slider::new(&mut params.amount, 0.0..=1.0).text("量"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.radius_px, 0.0..=48.0).text("半径"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.min_transmission, 0.10..=0.90).text("最小透過率"),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.saturation, -50.0..=50.0).text("彩度補正"))
                .changed();
        }
        _ => {
            debug_assert!(
                !is_tone_detail_effect(effect),
                "tone/detail effect dispatch is out of sync"
            );
        }
    }

    EffectParamResponse {
        changed,
        start_selective_color_pick,
        cancel_selective_color_pick,
        start_rgb_pick,
        cancel_rgb_pick,
        ..Default::default()
    }
}

macro_rules! focus_motion_effect_patterns {
    () => {
        LocalEffect::Blur(_)
            | LocalEffect::MotionBlur(_)
            | LocalEffect::Wind(_)
            | LocalEffect::SpeedLines(_)
            | LocalEffect::RadialFlash(_)
            | LocalEffect::TiltShift(_)
            | LocalEffect::LensBlur(_)
            | LocalEffect::BokehSprite(_)
            | LocalEffect::LensDirt(_)
    };
}

fn is_focus_motion_effect(effect: &LocalEffect) -> bool {
    matches!(effect, focus_motion_effect_patterns!())
}

fn draw_focus_motion_effect_params(
    ui: &mut egui::Ui,
    effect: &mut LocalEffect,
    rgb_pick_active: Option<RgbPickTarget>,
    effect_position_handles_visible: bool,
) -> EffectParamResponse {
    let mut changed = false;
    let mut start_rgb_pick = None;
    let mut cancel_rgb_pick = false;
    let mut set_effect_position_handles_visible = None;

    match effect {
        LocalEffect::Blur(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱く") {
                    params.radius_px = 6.0;
                    changed = true;
                }
                if preset_button(ui, "背景ぼかし") {
                    params.radius_px = 18.0;
                    changed = true;
                }
                if preset_button(ui, "強く") {
                    params.radius_px = 40.0;
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.radius_px, 0.0..=80.0).text("半径"))
                .changed();
        }
        LocalEffect::MotionBlur(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "横") {
                    *params = MotionBlurParams {
                        distance_px: 24.0,
                        angle_degrees: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "縦") {
                    *params = MotionBlurParams {
                        distance_px: 24.0,
                        angle_degrees: 90.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "斜め") {
                    *params = MotionBlurParams {
                        distance_px: 30.0,
                        angle_degrees: -35.0,
                        strength: 0.9,
                    };
                    changed = true;
                }
                if preset_button(ui, "高速感") {
                    *params = MotionBlurParams {
                        distance_px: 56.0,
                        angle_degrees: 0.0,
                        strength: 0.85,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "指定方向へ画像を流すぼかしです。背景やエフェクトに部分適用すると動きの表現に使えます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let distance =
                ui.add(egui::Slider::new(&mut params.distance_px, 0.0..=160.0).text("距離"));
            changed |= distance.changed();
            distance
                .lab_hover_tip("ぼかしを伸ばす長さです。値を大きくすると流れる幅が長くなります。");
            let angle = ui.add(
                egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                    .text("角度")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip("ぼかしの方向です。0°で横方向、90°で縦方向になります。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から移動ぼかし結果へどれだけ近づけるかです。");
        }
        LocalEffect::Wind(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "右へ") {
                    *params = WindParams {
                        direction: WindDirection::Right,
                        source: WindSource::Bright,
                        distance_px: 34.0,
                        threshold: 0.42,
                        softness: 0.16,
                        turbulence: 0.08,
                        strength: 0.85,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "左へ") {
                    *params = WindParams {
                        direction: WindDirection::Left,
                        source: WindSource::Bright,
                        distance_px: 34.0,
                        threshold: 0.42,
                        softness: 0.16,
                        turbulence: 0.08,
                        strength: 0.85,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "強風") {
                    *params = WindParams {
                        direction: WindDirection::Right,
                        source: WindSource::Edge,
                        distance_px: 62.0,
                        threshold: 0.18,
                        softness: 0.14,
                        turbulence: 0.22,
                        strength: 0.95,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "暗線") {
                    *params = WindParams {
                        direction: WindDirection::Right,
                        source: WindSource::Dark,
                        distance_px: 42.0,
                        threshold: 0.46,
                        softness: 0.12,
                        turbulence: 0.04,
                        strength: 0.8,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "指定した起点を片方向へ引きずります。明部は光の尾、暗部は漫画的な暗線、輪郭は速度感の強い流線に向いています。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let right = params.direction == WindDirection::Right;
                if ui.selectable_label(right, "右へ").clicked() && !right {
                    params.direction = WindDirection::Right;
                    changed = true;
                }
                let left = params.direction == WindDirection::Left;
                if ui.selectable_label(left, "左へ").clicked() && !left {
                    params.direction = WindDirection::Left;
                    changed = true;
                }
                let down = params.direction == WindDirection::Down;
                if ui.selectable_label(down, "下へ").clicked() && !down {
                    params.direction = WindDirection::Down;
                    changed = true;
                }
                let up = params.direction == WindDirection::Up;
                if ui.selectable_label(up, "上へ").clicked() && !up {
                    params.direction = WindDirection::Up;
                    changed = true;
                }
            });
            ui.horizontal_wrapped(|ui| {
                let bright = params.source == WindSource::Bright;
                if ui.selectable_label(bright, "明部").clicked() && !bright {
                    params.source = WindSource::Bright;
                    changed = true;
                }
                let dark = params.source == WindSource::Dark;
                if ui.selectable_label(dark, "暗部").clicked() && !dark {
                    params.source = WindSource::Dark;
                    changed = true;
                }
                let edge = params.source == WindSource::Edge;
                if ui.selectable_label(edge, "輪郭").clicked() && !edge {
                    params.source = WindSource::Edge;
                    changed = true;
                }
            });
            let distance = ui.add(
                egui::Slider::new(&mut params.distance_px, 0.0..=160.0)
                    .text("距離")
                    .suffix("px"),
            );
            changed |= distance.changed();
            distance.lab_hover_tip("流線を伸ばす長さです。");
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.0..=1.0).text("しきい値"));
            changed |= threshold.changed();
            threshold.lab_hover_tip("流線の起点として拾う明るさ・暗さ・輪郭の強さです。");
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.001..=0.5).text("柔らかさ"));
            changed |= softness.changed();
            softness.lab_hover_tip("しきい値付近の起点をどれだけなだらかに拾うかです。");
            let turbulence =
                ui.add(egui::Slider::new(&mut params.turbulence, 0.0..=1.0).text("乱れ"));
            changed |= turbulence.changed();
            turbulence.lab_hover_tip("流線の横揺れです。上げるほど風のムラが出ます。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から風/スピード結果へどれだけ近づけるかです。");
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("乱れのパターンを変えます。");
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::SpeedLines(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "白集中") {
                    *params = SpeedLinesParams {
                        mode: SpeedLinesMode::Radial,
                        center: [0.5, 0.5],
                        angle_degrees: 0.0,
                        line_count: 96,
                        line_width_px: 2.4,
                        length: 0.92,
                        inner_radius: 0.18,
                        outer_radius: 1.0,
                        softness: 0.25,
                        strength: 0.82,
                        color_rgb: [255, 255, 255],
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "黒集中") {
                    *params = SpeedLinesParams {
                        mode: SpeedLinesMode::Radial,
                        center: [0.5, 0.5],
                        angle_degrees: 0.0,
                        line_count: 72,
                        line_width_px: 2.0,
                        length: 0.86,
                        inner_radius: 0.22,
                        outer_radius: 1.0,
                        softness: 0.18,
                        strength: 0.78,
                        color_rgb: [0, 0, 0],
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "横流れ") {
                    *params = SpeedLinesParams {
                        mode: SpeedLinesMode::Parallel,
                        center: [0.5, 0.5],
                        angle_degrees: 0.0,
                        line_count: 44,
                        line_width_px: 2.2,
                        length: 0.90,
                        inner_radius: 0.08,
                        outer_radius: 1.0,
                        softness: 0.30,
                        strength: 0.68,
                        color_rgb: [255, 255, 255],
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "斜め流れ") {
                    *params = SpeedLinesParams {
                        mode: SpeedLinesMode::Parallel,
                        center: [0.5, 0.5],
                        angle_degrees: -28.0,
                        line_count: 58,
                        line_width_px: 1.8,
                        length: 0.72,
                        inner_radius: 0.04,
                        outer_radius: 1.0,
                        softness: 0.22,
                        strength: 0.74,
                        color_rgb: [255, 255, 255],
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "放射状の集中線、または指定方向へ流れる平行スピード線を自動生成します。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let radial = params.mode == SpeedLinesMode::Radial;
                if ui.selectable_label(radial, "放射").clicked() && !radial {
                    params.mode = SpeedLinesMode::Radial;
                    changed = true;
                }
                let parallel = params.mode == SpeedLinesMode::Parallel;
                if ui.selectable_label(parallel, "平行").clicked() && !parallel {
                    params.mode = SpeedLinesMode::Parallel;
                    changed = true;
                }
            });
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "放射では集中点、平行では線の基準位置です。",
                "放射では集中点、平行では線の基準位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            if params.mode != SpeedLinesMode::Radial {
                let angle = ui.add(
                    egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                        .text("角度")
                        .suffix("°"),
                );
                changed |= angle.changed();
                angle.lab_hover_tip("スピード線が流れる方向です。0°で横方向、90°で縦方向です。");
            }
            let mut line_count = params.line_count as i32;
            let line_count_response =
                ui.add(egui::Slider::new(&mut line_count, 4..=240).text("線数"));
            changed |= line_count_response.changed();
            line_count_response.lab_hover_tip("生成する線の本数です。");
            params.line_count = line_count.clamp(4, 240) as u32;
            let line_width =
                ui.add(egui::Slider::new(&mut params.line_width_px, 0.25..=24.0).text("線幅"));
            changed |= line_width.changed();
            line_width.lab_hover_tip("1本あたりの太さです。");
            let length = ui.add(egui::Slider::new(&mut params.length, 0.05..=1.0).text("線長"));
            changed |= length.changed();
            length.lab_hover_tip("線をどれだけ長く伸ばすかです。");
            let inner =
                ui.add(egui::Slider::new(&mut params.inner_radius, 0.0..=0.98).text("中心抜き"));
            changed |= inner.changed();
            inner.lab_hover_tip("放射では中央の空白、平行では中央付近の弱まりを調整します。");
            let outer =
                ui.add(egui::Slider::new(&mut params.outer_radius, 0.02..=1.0).text("外側範囲"));
            changed |= outer.changed();
            outer.lab_hover_tip("線が出る外側の範囲です。");
            if params.outer_radius < params.inner_radius {
                params.outer_radius = (params.inner_radius + 0.02).min(1.0);
            }
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.0..=1.0).text("柔らかさ"));
            changed |= softness.changed();
            softness.lab_hover_tip("線の縁をぼかします。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から線色へどれだけ近づけるかです。");
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "線色",
                    &mut params.color_rgb,
                    RgbPickTarget::SpeedLinesColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("線のばらつきパターンを変えます。");
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::RadialFlash(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "白黒衝撃") {
                    *params = RadialFlashParams {
                        center: [0.5, 0.5],
                        ray_count: 36,
                        rotation_degrees: -6.0,
                        inner_radius: 0.06,
                        outer_radius: 1.0,
                        softness: 0.14,
                        white_amount: 0.92,
                        black_amount: 0.70,
                        invert: false,
                        strength: 0.90,
                    };
                    changed = true;
                }
                if preset_button(ui, "反転") {
                    *params = RadialFlashParams {
                        center: [0.5, 0.5],
                        ray_count: 34,
                        rotation_degrees: 8.0,
                        inner_radius: 0.03,
                        outer_radius: 1.0,
                        softness: 0.12,
                        white_amount: 0.72,
                        black_amount: 0.92,
                        invert: true,
                        strength: 0.88,
                    };
                    changed = true;
                }
                if preset_button(ui, "細かい") {
                    *params = RadialFlashParams {
                        center: [0.5, 0.5],
                        ray_count: 80,
                        rotation_degrees: 0.0,
                        inner_radius: 0.12,
                        outer_radius: 1.0,
                        softness: 0.24,
                        white_amount: 0.86,
                        black_amount: 0.56,
                        invert: false,
                        strength: 0.76,
                    };
                    changed = true;
                }
                if preset_button(ui, "中心抜き") {
                    *params = RadialFlashParams {
                        center: [0.5, 0.5],
                        ray_count: 44,
                        rotation_degrees: -12.0,
                        inner_radius: 0.28,
                        outer_radius: 1.0,
                        softness: 0.20,
                        white_amount: 0.92,
                        black_amount: 0.58,
                        invert: false,
                        strength: 0.82,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "中心から白黒のくさび形フラッシュを放射します。漫画的な衝撃や強い視線誘導向けです。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "フラッシュの中心位置です。",
                "フラッシュの中心位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            let mut ray_count = params.ray_count as i32;
            let ray_count_response =
                ui.add(egui::Slider::new(&mut ray_count, 4..=240).text("分割数"));
            changed |= ray_count_response.changed();
            ray_count_response.lab_hover_tip(
                "白黒に分ける放射方向の数です。多いほど細かいフラッシュになります。",
            );
            params.ray_count = ray_count.clamp(4, 240) as u32;
            let rotation = ui.add(
                egui::Slider::new(&mut params.rotation_degrees, -180.0..=180.0)
                    .text("回転")
                    .suffix("°"),
            );
            changed |= rotation.changed();
            rotation.lab_hover_tip("白黒フラッシュの角度を回します。");
            let inner =
                ui.add(egui::Slider::new(&mut params.inner_radius, 0.0..=0.98).text("中心抜き"));
            changed |= inner.changed();
            inner.lab_hover_tip("中心付近を効果なしで残す範囲です。");
            let outer =
                ui.add(egui::Slider::new(&mut params.outer_radius, 0.02..=1.0).text("外側範囲"));
            changed |= outer.changed();
            outer.lab_hover_tip("フラッシュを出す外側の範囲です。");
            if params.outer_radius < params.inner_radius {
                params.outer_radius = (params.inner_radius + 0.02).min(1.0);
            }
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.0..=1.0).text("柔らかさ"));
            changed |= softness.changed();
            softness.lab_hover_tip("白黒の境界と内外のフェードを柔らかくします。");
            let white =
                ui.add(egui::Slider::new(&mut params.white_amount, 0.0..=1.0).text("白の強さ"));
            changed |= white.changed();
            white.lab_hover_tip("白いくさびで明るくする量です。");
            let black =
                ui.add(egui::Slider::new(&mut params.black_amount, 0.0..=1.0).text("黒の強さ"));
            changed |= black.changed();
            black.lab_hover_tip("黒いくさびで暗くする量です。");
            let invert = ui.checkbox(&mut params.invert, "白黒を反転");
            changed |= invert.changed();
            invert.lab_hover_tip("白と黒の配置を入れ替えます。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からフラッシュ結果へどれだけ近づけるかです。");
        }
        LocalEffect::TiltShift(params) => {
            if !params.range_initialized && !params.mode_selected {
                params.mode_selected = true;
            }
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "奥ぼかし") {
                    *params = TiltShiftParams {
                        mode: TiltShiftMode::Linear,
                        mode_selected: true,
                        range_initialized: true,
                        center: [0.5, 0.58],
                        angle_degrees: -90.0,
                        focus_width: 0.10,
                        falloff: 0.34,
                        radius: [0.32, 0.32],
                        max_radius_px: 24.0,
                        strength: 1.0,
                        far_only: true,
                    };
                    changed = true;
                }
                if preset_button(ui, "ミニチュア") {
                    *params = TiltShiftParams {
                        mode: TiltShiftMode::Linear,
                        mode_selected: true,
                        range_initialized: true,
                        center: [0.5, 0.52],
                        angle_degrees: -90.0,
                        focus_width: 0.08,
                        falloff: 0.22,
                        radius: [0.32, 0.32],
                        max_radius_px: 34.0,
                        strength: 1.0,
                        far_only: false,
                    };
                    changed = true;
                }
                if preset_button(ui, "円形") {
                    *params = TiltShiftParams {
                        mode: TiltShiftMode::Radial,
                        mode_selected: true,
                        range_initialized: true,
                        center: [0.5, 0.5],
                        angle_degrees: -90.0,
                        focus_width: 0.12,
                        falloff: 0.34,
                        radius: [0.32, 0.32],
                        max_radius_px: 28.0,
                        strength: 1.0,
                        far_only: false,
                    };
                    changed = true;
                }
                if preset_button(ui, "斜め") {
                    *params = TiltShiftParams {
                        mode: TiltShiftMode::Linear,
                        mode_selected: true,
                        range_initialized: true,
                        center: [0.5, 0.5],
                        angle_degrees: -35.0,
                        focus_width: 0.10,
                        falloff: 0.28,
                        radius: [0.32, 0.32],
                        max_radius_px: 26.0,
                        strength: 0.9,
                        far_only: false,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "焦点帯または焦点円を残し、外側だけをぼかします。背景だけに使う場合は線形の奥ぼかしから試してください。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let linear_create_active = !params.range_initialized
                    && params.mode_selected
                    && params.mode == TiltShiftMode::Linear;
                if ui
                    .selectable_label(linear_create_active, "線形範囲を作成")
                    .clicked()
                {
                    params.mode = TiltShiftMode::Linear;
                    params.mode_selected = true;
                    params.range_initialized = false;
                    changed = true;
                }
                let radial_create_active = !params.range_initialized
                    && params.mode_selected
                    && params.mode == TiltShiftMode::Radial;
                if ui
                    .selectable_label(radial_create_active, "円形範囲を作成")
                    .clicked()
                {
                    params.mode = TiltShiftMode::Radial;
                    params.mode_selected = true;
                    params.range_initialized = false;
                    changed = true;
                }
                if ui.button("範囲クリア").clicked() {
                    params.range_initialized = false;
                    params.mode_selected = true;
                    changed = true;
                }
            });
            if params.range_initialized {
                let center_x =
                    ui.add(egui::Slider::new(&mut params.center[0], 0.0..=1.0).text("中心 X"));
                changed |= center_x.changed();
                center_x.lab_hover_tip("焦点帯または焦点円の中心位置です。");
                let center_y =
                    ui.add(egui::Slider::new(&mut params.center[1], 0.0..=1.0).text("中心 Y"));
                changed |= center_y.changed();
                center_y.lab_hover_tip("焦点帯または焦点円の中心位置です。");
                if params.mode == TiltShiftMode::Linear {
                    let angle = ui.add(
                        egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                            .text("奥行き方向")
                            .suffix("°"),
                    );
                    changed |= angle.changed();
                    angle.lab_hover_tip("ぼかしが強くなる方向です。-90°は上側を奥として扱います。");
                    let far_only = ui.checkbox(&mut params.far_only, "奥だけぼかす");
                    changed |= far_only.changed();
                    far_only.lab_hover_tip("ONにすると、焦点帯より奥側だけをぼかします。OFFでは手前と奥の両側をぼかします。");
                    let focus_width = ui
                        .add(egui::Slider::new(&mut params.focus_width, 0.0..=0.5).text("焦点幅"));
                    changed |= focus_width.changed();
                    focus_width.lab_hover_tip("線形モードで、シャープに残す帯の幅です。");
                } else {
                    let rx = ui
                        .add(egui::Slider::new(&mut params.radius[0], 0.02..=1.0).text("焦点 横"));
                    changed |= rx.changed();
                    rx.lab_hover_tip("円形モードで、シャープに残す範囲の横半径です。");
                    let ry = ui
                        .add(egui::Slider::new(&mut params.radius[1], 0.02..=1.0).text("焦点 縦"));
                    changed |= ry.changed();
                    ry.lab_hover_tip("円形モードで、シャープに残す範囲の縦半径です。");
                }
                let falloff =
                    ui.add(egui::Slider::new(&mut params.falloff, 0.02..=1.0).text("ぼかし境界"));
                changed |= falloff.changed();
                falloff
                    .lab_hover_tip("焦点範囲の外側で、ぼかしがどれだけなだらかに強くなるかです。");
            } else {
                ui.label(
                    egui::RichText::new(
                        "範囲未設定です。アクティブな作成ボタンの形で、画像上をドラッグして範囲を作成します。",
                    )
                    .size(10.0)
                    .color(Color32::from_gray(190)),
                );
            }
            let max_radius =
                ui.add(egui::Slider::new(&mut params.max_radius_px, 0.0..=80.0).text("最大半径"));
            changed |= max_radius.changed();
            max_radius.lab_hover_tip("最もぼける場所で使うぼかし半径です。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からチルトシフト結果へどれだけ近づけるかです。");
        }
        LocalEffect::LensBlur(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱") {
                    *params = LensBlurParams {
                        radius_px: 10.0,
                        aperture: LensBlurAperture::Circular,
                        rotation_degrees: 0.0,
                        highlight_threshold: 0.94,
                        highlight_boost: 0.3,
                        strength: 0.55,
                    };
                    changed = true;
                }
                if preset_button(ui, "背景ぼかし") {
                    *params = LensBlurParams {
                        radius_px: 24.0,
                        aperture: LensBlurAperture::Circular,
                        rotation_degrees: 0.0,
                        highlight_threshold: 0.94,
                        highlight_boost: 0.5,
                        strength: 0.9,
                    };
                    changed = true;
                }
                if preset_button(ui, "玉ボケ") {
                    *params = LensBlurParams {
                        radius_px: 34.0,
                        aperture: LensBlurAperture::Circular,
                        rotation_degrees: 0.0,
                        highlight_threshold: 0.86,
                        highlight_boost: 1.2,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "6角光") {
                    *params = LensBlurParams {
                        radius_px: 32.0,
                        aperture: LensBlurAperture::Hexagon,
                        rotation_degrees: 30.0,
                        highlight_threshold: 0.88,
                        highlight_boost: 1.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "絞り形状で画像を集めるぼかしです。明るい点がある背景に使うと、通常のぼかしよりレンズらしい玉ボケになります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal(|ui| {
                let circular = params.aperture == LensBlurAperture::Circular;
                if ui.selectable_label(circular, "円形").clicked() && !circular {
                    params.aperture = LensBlurAperture::Circular;
                    changed = true;
                }
                let hexagon = params.aperture == LensBlurAperture::Hexagon;
                if ui.selectable_label(hexagon, "6角").clicked() && !hexagon {
                    params.aperture = LensBlurAperture::Hexagon;
                    changed = true;
                }
                let octagon = params.aperture == LensBlurAperture::Octagon;
                if ui.selectable_label(octagon, "8角").clicked() && !octagon {
                    params.aperture = LensBlurAperture::Octagon;
                    changed = true;
                }
            });
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 0.0..=64.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip("ぼかしの大きさです。値を大きくすると玉ボケも大きくなります。");
            if params.aperture != LensBlurAperture::Circular {
                let rotation = ui.add(
                    egui::Slider::new(&mut params.rotation_degrees, -180.0..=180.0)
                        .text("絞り回転")
                        .suffix("°"),
                );
                changed |= rotation.changed();
                rotation.lab_hover_tip("6角・8角の絞り形状の向きを回転します。");
            }
            let threshold = ui.add(
                egui::Slider::new(&mut params.highlight_threshold, 0.50..=0.995)
                    .text("明部しきい値"),
            );
            changed |= threshold.changed();
            threshold.lab_hover_tip(
                "玉ボケとして膨らませる明るさのしきい値です。低いほど多くの明部が強調されます。",
            );
            let boost = ui.add(
                egui::Slider::new(&mut params.highlight_boost, 0.0..=3.0).text("明部ブースト"),
            );
            changed |= boost.changed();
            boost.lab_hover_tip("しきい値を超えた明るい点を、ぼかし内でどれだけ強く扱うかです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からレンズぼかし結果へどれだけ近づけるかです。");
        }
        LocalEffect::BokehSprite(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "丸い光") {
                    *params = BokehSpriteParams {
                        shape: BokehSpriteShape::Circle,
                        threshold: 0.94,
                        density: 0.35,
                        size_px: 18.0,
                        softness: 0.55,
                        brightness: 1.0,
                        color_strength: 0.45,
                        seed: 1,
                        strength: 0.65,
                    };
                    changed = true;
                }
                if preset_button(ui, "星きらめき") {
                    *params = BokehSpriteParams {
                        shape: BokehSpriteShape::Star,
                        threshold: 0.96,
                        density: 0.45,
                        size_px: 16.0,
                        softness: 0.35,
                        brightness: 1.4,
                        color_strength: 0.35,
                        seed: 7,
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "ハート") {
                    *params = BokehSpriteParams {
                        shape: BokehSpriteShape::Heart,
                        threshold: 0.95,
                        density: 0.38,
                        size_px: 20.0,
                        softness: 0.45,
                        brightness: 1.2,
                        color_strength: 0.60,
                        seed: 11,
                        strength: 0.72,
                    };
                    changed = true;
                }
                if preset_button(ui, "細かく") {
                    *params = BokehSpriteParams {
                        shape: BokehSpriteShape::Circle,
                        threshold: 0.92,
                        density: 0.75,
                        size_px: 10.0,
                        softness: 0.45,
                        brightness: 0.85,
                        color_strength: 0.35,
                        seed: 19,
                        strength: 0.55,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明るい点から形状付きの玉ボケ粒子を作ります。レンズぼかしの上に重ねる装飾や、夜景・魔法光の仕上げに向いています。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal(|ui| {
                for (shape, label) in [
                    (BokehSpriteShape::Circle, "丸"),
                    (BokehSpriteShape::Star, "星"),
                    (BokehSpriteShape::Heart, "ハート"),
                ] {
                    let selected = params.shape == shape;
                    if ui.selectable_label(selected, label).clicked() && !selected {
                        params.shape = shape;
                        changed = true;
                    }
                }
            });
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.50..=0.995).text("明部しきい値"));
            changed |= threshold.changed();
            threshold.lab_hover_tip(
                "粒子を発生させる明るさのしきい値です。低いほど多くの点から出ます。",
            );
            let density = ui.add(egui::Slider::new(&mut params.density, 0.0..=1.0).text("密度"));
            changed |= density.changed();
            density.lab_hover_tip("候補セルの間隔を変え、粒子が出る数を調整します。");
            let size = ui.add(egui::Slider::new(&mut params.size_px, 2.0..=96.0).text("サイズ"));
            changed |= size.changed();
            size.lab_hover_tip("粒子ひとつあたりの大きさです。");
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.0..=1.0).text("柔らかさ"));
            changed |= softness.changed();
            softness.lab_hover_tip("粒子の輪郭をどれだけ柔らかくするかです。");
            let brightness =
                ui.add(egui::Slider::new(&mut params.brightness, 0.0..=2.0).text("明るさ"));
            changed |= brightness.changed();
            brightness.lab_hover_tip("生成した粒子の明るさです。");
            let color_strength =
                ui.add(egui::Slider::new(&mut params.color_strength, 0.0..=1.0).text("元色反映"));
            changed |= color_strength.changed();
            color_strength.lab_hover_tip("0では白い粒子、1では発生元の色を強く反映します。");
            let seed = ui.add(egui::Slider::new(&mut params.seed, 0..=9999).text("seed"));
            changed |= seed.changed();
            seed.lab_hover_tip("粒子の揺らぎを変える値です。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像に対して玉ボケ粒子をどれだけ重ねるかです。");
        }
        LocalEffect::LensDirt(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "ほこり") {
                    *params = LensDirtParams {
                        mode: LensDirtMode::Dust,
                        density: 0.58,
                        size_px: 10.0,
                        opacity: 0.46,
                        softness: 0.38,
                        highlight_response: 0.72,
                        distortion_px: 0.0,
                        seed: 3,
                        strength: 0.72,
                    };
                    changed = true;
                }
                if preset_button(ui, "水滴") {
                    *params = LensDirtParams {
                        mode: LensDirtMode::WaterDrops,
                        density: 0.48,
                        size_px: 34.0,
                        opacity: 0.72,
                        softness: 0.48,
                        highlight_response: 0.88,
                        distortion_px: 10.0,
                        seed: 5,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "曇り指紋") {
                    *params = LensDirtParams {
                        mode: LensDirtMode::Smudges,
                        density: 0.58,
                        size_px: 74.0,
                        opacity: 0.62,
                        softness: 0.78,
                        highlight_response: 0.62,
                        distortion_px: 2.0,
                        seed: 7,
                        strength: 0.72,
                    };
                    changed = true;
                }
                if preset_button(ui, "薄く") {
                    *params = LensDirtParams {
                        mode: LensDirtMode::Dust,
                        density: 0.28,
                        size_px: 12.0,
                        opacity: 0.26,
                        softness: 0.52,
                        highlight_response: 0.42,
                        distortion_px: 0.0,
                        seed: 11,
                        strength: 0.45,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "レンズ表面のほこり、水滴、曇りを重ねます。光源や逆光のある絵で、レンズ越しの質感を足す用途に向いています。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal(|ui| {
                for (mode, label) in [
                    (LensDirtMode::Dust, "ダスト"),
                    (LensDirtMode::WaterDrops, "水滴"),
                    (LensDirtMode::Smudges, "曇り"),
                ] {
                    let selected = params.mode == mode;
                    if ui.selectable_label(selected, label).clicked() && !selected {
                        params.mode = mode;
                        changed = true;
                    }
                }
            });
            let density = ui.add(egui::Slider::new(&mut params.density, 0.0..=1.0).text("密度"));
            changed |= density.changed();
            density.lab_hover_tip("汚れや水滴がどれだけ多く出るかです。");
            let size = ui.add(egui::Slider::new(&mut params.size_px, 2.0..=128.0).text("サイズ"));
            changed |= size.changed();
            size.lab_hover_tip("ほこりの粒、水滴、曇り筋の大きさです。");
            let opacity = ui.add(egui::Slider::new(&mut params.opacity, 0.0..=1.0).text("濃さ"));
            changed |= opacity.changed();
            opacity.lab_hover_tip("生成したレンズ汚れをどれだけ見せるかです。");
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.0..=1.0).text("柔らかさ"));
            changed |= softness.changed();
            softness.lab_hover_tip("汚れや水滴の輪郭をどれだけ柔らかくするかです。");
            let highlight =
                ui.add(egui::Slider::new(&mut params.highlight_response, 0.0..=1.0).text("光反応"));
            changed |= highlight.changed();
            highlight.lab_hover_tip(
                "明るい場所で汚れを目立たせる量です。値を上げると逆光や光源付近で出やすくなります。",
            );
            let distortion = ui.add_enabled(
                params.mode != LensDirtMode::Dust,
                egui::Slider::new(&mut params.distortion_px, 0.0..=32.0).text("屈折"),
            );
            changed |= distortion.changed();
            distortion
                .lab_hover_tip("水滴や曇りで下の画像を少しずらす量です。ダストでは使いません。");
            let seed = ui.add(egui::Slider::new(&mut params.seed, 0..=9999).text("seed"));
            changed |= seed.changed();
            seed.lab_hover_tip("汚れの配置やノイズを変える値です。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像に対してレンズ汚れをどれだけ重ねるかです。");
        }
        _ => {
            debug_assert!(
                !is_focus_motion_effect(effect),
                "focus/motion effect dispatch is out of sync"
            );
        }
    }

    EffectParamResponse {
        changed,
        start_rgb_pick,
        cancel_rgb_pick,
        set_effect_position_handles_visible,
        ..Default::default()
    }
}

macro_rules! distort_effect_patterns {
    () => {
        LocalEffect::RadialBlur(_)
            | LocalEffect::WaveDistortion(_)
            | LocalEffect::HeatHaze(_)
            | LocalEffect::PinchSpherize(_)
            | LocalEffect::Twirl(_)
            | LocalEffect::PolarCoordinates(_)
            | LocalEffect::GlassDisplacement(_)
            | LocalEffect::LensCorrection(_)
    };
}

fn is_distort_effect(effect: &LocalEffect) -> bool {
    matches!(effect, distort_effect_patterns!())
}

fn draw_distort_effect_params(
    ui: &mut egui::Ui,
    effect: &mut LocalEffect,
    effect_position_handles_visible: bool,
) -> EffectParamResponse {
    let mut changed = false;
    let mut set_effect_position_handles_visible = None;

    match effect {
        LocalEffect::RadialBlur(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "ズーム弱") {
                    *params = RadialBlurParams {
                        mode: RadialBlurMode::Zoom,
                        center: [0.5, 0.5],
                        zoom_px: 28.0,
                        spin_degrees: 0.0,
                        samples: 21,
                        strength: 0.65,
                    };
                    changed = true;
                }
                if preset_button(ui, "集中") {
                    *params = RadialBlurParams {
                        mode: RadialBlurMode::Zoom,
                        center: [0.5, 0.5],
                        zoom_px: 78.0,
                        spin_degrees: 0.0,
                        samples: 33,
                        strength: 0.95,
                    };
                    changed = true;
                }
                if preset_button(ui, "回転") {
                    *params = RadialBlurParams {
                        mode: RadialBlurMode::Spin,
                        center: [0.5, 0.5],
                        zoom_px: 0.0,
                        spin_degrees: 24.0,
                        samples: 25,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "強回転") {
                    *params = RadialBlurParams {
                        mode: RadialBlurMode::Spin,
                        center: [0.5, 0.5],
                        zoom_px: 0.0,
                        spin_degrees: 64.0,
                        samples: 41,
                        strength: 0.9,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "中心から外へ伸びるズームぼかし、または中心周りに回るぼかしです。集中線的な速度感や渦巻き感に使えます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal(|ui| {
                let zoom = params.mode == RadialBlurMode::Zoom;
                if ui.selectable_label(zoom, "ズーム").clicked() && !zoom {
                    params.mode = RadialBlurMode::Zoom;
                    changed = true;
                }
                let spin = params.mode == RadialBlurMode::Spin;
                if ui.selectable_label(spin, "回転").clicked() && !spin {
                    params.mode = RadialBlurMode::Spin;
                    changed = true;
                }
            });
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "ぼかしの中心位置です。ズームでは集中点、回転では回転中心になります。",
                "ぼかしの中心位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            match params.mode {
                RadialBlurMode::Zoom => {
                    let zoom =
                        ui.add(egui::Slider::new(&mut params.zoom_px, 0.0..=160.0).text("距離"));
                    changed |= zoom.changed();
                    zoom.lab_hover_tip("画像の端でどれだけ外向きにサンプルを伸ばすかです。");
                }
                RadialBlurMode::Spin => {
                    let spin = ui.add(
                        egui::Slider::new(&mut params.spin_degrees, -180.0..=180.0)
                            .text("回転角")
                            .suffix("°"),
                    );
                    changed |= spin.changed();
                    spin.lab_hover_tip("画像の端でどれだけ回転方向にサンプルを広げるかです。符号で回転方向が変わります。");
                }
            }
            let samples = ui.add(egui::Slider::new(&mut params.samples, 3..=65).text("サンプル数"));
            changed |= samples.changed();
            samples.lab_hover_tip(
                "ぼかしの滑らかさです。大きいほど滑らかですが再合成は重くなります。",
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から放射/回転ぼかし結果へどれだけ近づけるかです。");
        }
        LocalEffect::WaveDistortion(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "水面横波") {
                    *params = WaveDistortionParams {
                        mode: WaveDistortionMode::Horizontal,
                        amplitude_px: 12.0,
                        wavelength_px: 72.0,
                        phase_degrees: 0.0,
                        center: [0.5, 0.5],
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "縦ゆらぎ") {
                    *params = WaveDistortionParams {
                        mode: WaveDistortionMode::Vertical,
                        amplitude_px: 10.0,
                        wavelength_px: 64.0,
                        phase_degrees: 0.0,
                        center: [0.5, 0.5],
                        strength: 0.9,
                    };
                    changed = true;
                }
                if preset_button(ui, "さざ波") {
                    *params = WaveDistortionParams {
                        mode: WaveDistortionMode::Ripple,
                        amplitude_px: 8.0,
                        wavelength_px: 36.0,
                        phase_degrees: 0.0,
                        center: [0.5, 0.5],
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "ジグザグ") {
                    *params = WaveDistortionParams {
                        mode: WaveDistortionMode::Zigzag,
                        amplitude_px: 14.0,
                        wavelength_px: 48.0,
                        phase_degrees: 0.0,
                        center: [0.5, 0.5],
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "画像を波の形にサンプルし直します。反射、水面、熱気、背景の揺らぎに使えます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let horizontal = params.mode == WaveDistortionMode::Horizontal;
                if ui.selectable_label(horizontal, "横波").clicked() && !horizontal {
                    params.mode = WaveDistortionMode::Horizontal;
                    changed = true;
                }
                let vertical = params.mode == WaveDistortionMode::Vertical;
                if ui.selectable_label(vertical, "縦波").clicked() && !vertical {
                    params.mode = WaveDistortionMode::Vertical;
                    changed = true;
                }
                let ripple = params.mode == WaveDistortionMode::Ripple;
                if ui.selectable_label(ripple, "さざ波").clicked() && !ripple {
                    params.mode = WaveDistortionMode::Ripple;
                    changed = true;
                }
                let zigzag = params.mode == WaveDistortionMode::Zigzag;
                if ui.selectable_label(zigzag, "ジグザグ").clicked() && !zigzag {
                    params.mode = WaveDistortionMode::Zigzag;
                    changed = true;
                }
            });
            let amplitude =
                ui.add(egui::Slider::new(&mut params.amplitude_px, -80.0..=80.0).text("振幅"));
            changed |= amplitude.changed();
            amplitude.lab_hover_tip(
                "どれだけ大きく画素をずらすかです。符号を変えると揺れの向きが反転します。",
            );
            let wavelength =
                ui.add(egui::Slider::new(&mut params.wavelength_px, 4.0..=240.0).text("波長"));
            changed |= wavelength.changed();
            wavelength
                .lab_hover_tip("波の間隔です。小さい値ほど細かく、大きい値ほどゆったり揺れます。");
            let phase = ui.add(
                egui::Slider::new(&mut params.phase_degrees, -180.0..=180.0)
                    .text("位相")
                    .suffix("°"),
            );
            changed |= phase.changed();
            phase.lab_hover_tip(
                "波の開始位置をずらします。アニメーション用ではなく、静止画の位置合わせ用です。",
            );
            if params.mode == WaveDistortionMode::Ripple {
                changed |= draw_effect_center_controls(
                    ui,
                    &mut params.center,
                    "さざ波の中心位置です。",
                    "さざ波の中心位置です。",
                    effect_position_handles_visible,
                    &mut set_effect_position_handles_visible,
                );
            }
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からゆがみ結果へどれだけ近づけるかです。");
        }
        LocalEffect::HeatHaze(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "炎のゆらぎ") {
                    *params = HeatHazeParams {
                        amplitude_px: 16.0,
                        wavelength_px: 42.0,
                        rise_px: 10.0,
                        turbulence: 0.82,
                        blur_px: 1.2,
                        phase_degrees: 20.0,
                        strength: 0.92,
                    };
                    changed = true;
                }
                if preset_button(ui, "夏空") {
                    *params = HeatHazeParams {
                        amplitude_px: 7.0,
                        wavelength_px: 96.0,
                        rise_px: 4.0,
                        turbulence: 0.42,
                        blur_px: 0.7,
                        phase_degrees: 0.0,
                        strength: 0.58,
                    };
                    changed = true;
                }
                if preset_button(ui, "排熱") {
                    *params = HeatHazeParams {
                        amplitude_px: 22.0,
                        wavelength_px: 58.0,
                        rise_px: 18.0,
                        turbulence: 1.0,
                        blur_px: 1.8,
                        phase_degrees: -35.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "水面反射") {
                    *params = HeatHazeParams {
                        amplitude_px: 10.0,
                        wavelength_px: 34.0,
                        rise_px: 0.0,
                        turbulence: 0.25,
                        blur_px: 0.5,
                        phase_degrees: 90.0,
                        strength: 0.74,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "上昇する空気のように横へ揺らし、必要に応じて下側の画素を少し引き上げます。マスクで炎や排熱の範囲を切ると使いやすい効果です。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let amplitude =
                ui.add(egui::Slider::new(&mut params.amplitude_px, -80.0..=80.0).text("揺れ幅"));
            changed |= amplitude.changed();
            amplitude.lab_hover_tip(
                "横方向へどれだけ大きく揺らすかです。符号で揺れの向きが反転します。",
            );
            let wavelength =
                ui.add(egui::Slider::new(&mut params.wavelength_px, 4.0..=240.0).text("揺れ間隔"));
            changed |= wavelength.changed();
            wavelength.lab_hover_tip(
                "揺れの間隔です。小さい値ほど細かく、大きい値ほどゆったり揺れます。",
            );
            let rise = ui.add(
                egui::Slider::new(&mut params.rise_px, -80.0..=80.0)
                    .text("上昇")
                    .suffix("px"),
            );
            changed |= rise.changed();
            rise.lab_hover_tip("正の値で下側の画素を上へ引き上げるように見せます。");
            let turbulence =
                ui.add(egui::Slider::new(&mut params.turbulence, 0.0..=1.0).text("乱れ"));
            changed |= turbulence.changed();
            turbulence.lab_hover_tip("揺れに斜め方向の細かな変化を混ぜます。");
            let blur = ui.add(
                egui::Slider::new(&mut params.blur_px, 0.0..=12.0)
                    .text("にじみ")
                    .suffix("px"),
            );
            changed |= blur.changed();
            blur.lab_hover_tip("熱で輪郭が少しぼけるような柔らかさを加えます。");
            let phase = ui.add(
                egui::Slider::new(&mut params.phase_degrees, -180.0..=180.0)
                    .text("位相")
                    .suffix("°"),
            );
            changed |= phase.changed();
            phase.lab_hover_tip("揺れの開始位置をずらします。静止画の位置合わせ用です。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から陽炎結果へどれだけ近づけるかです。");
        }
        LocalEffect::PinchSpherize(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "魚眼") {
                    *params = PinchSpherizeParams {
                        amount: 0.72,
                        radius_px: 0.0,
                        center: [0.5, 0.5],
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ふくらむ") {
                    *params = PinchSpherizeParams {
                        amount: 0.45,
                        radius_px: 260.0,
                        center: [0.5, 0.5],
                        strength: 0.9,
                    };
                    changed = true;
                }
                if preset_button(ui, "つまむ") {
                    *params = PinchSpherizeParams {
                        amount: -0.65,
                        radius_px: 260.0,
                        center: [0.5, 0.5],
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "小顔/圧縮") {
                    *params = PinchSpherizeParams {
                        amount: -0.35,
                        radius_px: 180.0,
                        center: [0.5, 0.45],
                        strength: 0.75,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "中心からの距離を変えて、魚眼レンズのようなふくらみや、内側へつまむ変形を作ります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let amount = ui.add(egui::Slider::new(&mut params.amount, -1.0..=1.0).text("変形量"));
            changed |= amount.changed();
            amount.lab_hover_tip("正で魚眼/ふくらみ、負で中心へつまむ変形になります。");
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 0.0..=800.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip("効果の範囲です。0 のときは中心から画像の角までを使います。");
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "変形の中心位置です。",
                "変形の中心位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から変形結果へどれだけ近づけるかです。");
        }
        LocalEffect::Twirl(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "渦弱") {
                    *params = TwirlParams {
                        angle_degrees: 120.0,
                        radius_px: 0.0,
                        center: [0.5, 0.5],
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "渦強") {
                    *params = TwirlParams {
                        angle_degrees: 360.0,
                        radius_px: 0.0,
                        center: [0.5, 0.5],
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "逆回転") {
                    *params = TwirlParams {
                        angle_degrees: -260.0,
                        radius_px: 0.0,
                        center: [0.5, 0.5],
                        strength: 0.95,
                    };
                    changed = true;
                }
                if preset_button(ui, "魔法陣") {
                    *params = TwirlParams {
                        angle_degrees: 540.0,
                        radius_px: 320.0,
                        center: [0.5, 0.5],
                        strength: 0.9,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "中心に近いほど強く回転させ、外側へ自然に弱まる渦巻き変形を作ります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let angle = ui.add(
                egui::Slider::new(&mut params.angle_degrees, -720.0..=720.0)
                    .text("回転量")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip("中心で最大になる回転量です。符号を変えると渦の向きが反転します。");
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 0.0..=800.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip("効果の範囲です。0 のときは中心から画像の角までを使います。");
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "渦巻きの中心位置です。",
                "渦巻きの中心位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から渦巻き結果へどれだけ近づけるかです。");
        }
        LocalEffect::PolarCoordinates(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "Tiny planet") {
                    *params = PolarCoordinatesParams {
                        mode: PolarCoordinatesMode::RectToPolar,
                        center: [0.5, 0.5],
                        radius_px: 0.0,
                        angle_offset_degrees: -90.0,
                        invert_radius: true,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "円形構図") {
                    *params = PolarCoordinatesParams {
                        mode: PolarCoordinatesMode::RectToPolar,
                        center: [0.5, 0.5],
                        radius_px: 0.0,
                        angle_offset_degrees: 0.0,
                        invert_radius: false,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "パノラマ展開") {
                    *params = PolarCoordinatesParams {
                        mode: PolarCoordinatesMode::PolarToRect,
                        center: [0.5, 0.5],
                        radius_px: 0.0,
                        angle_offset_degrees: 0.0,
                        invert_radius: false,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "内外反転") {
                    params.invert_radius = !params.invert_radius;
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "横方向を角度、縦方向を半径として扱い、画像を円形に巻いたり横長へ展開したりします。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let rect_to_polar = params.mode == PolarCoordinatesMode::RectToPolar;
                if ui.selectable_label(rect_to_polar, "矩形→円形").clicked() && !rect_to_polar
                {
                    params.mode = PolarCoordinatesMode::RectToPolar;
                    changed = true;
                }
                let polar_to_rect = params.mode == PolarCoordinatesMode::PolarToRect;
                if ui.selectable_label(polar_to_rect, "円形→矩形").clicked() && !polar_to_rect
                {
                    params.mode = PolarCoordinatesMode::PolarToRect;
                    changed = true;
                }
            });
            let invert = ui.checkbox(&mut params.invert_radius, "内外反転");
            changed |= invert.changed();
            invert.lab_hover_tip(
                "半径方向の対応を反転します。Tiny planet では地面側を中心へ寄せる用途に使います。",
            );
            let radius =
                ui.add(egui::Slider::new(&mut params.radius_px, 0.0..=1200.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip(
                "円形変換に使う半径です。0 のときは中心から画像の角までを使います。",
            );
            let angle = ui.add(
                egui::Slider::new(&mut params.angle_offset_degrees, -180.0..=180.0)
                    .text("角度オフセット")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip("巻き始めの角度を回します。継ぎ目や上方向の位置合わせに使います。");
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "円形変換の中心位置です。",
                "円形変換の中心位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から極座標変換結果へどれだけ近づけるかです。");
        }
        LocalEffect::GlassDisplacement(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "すりガラス") {
                    *params = GlassDisplacementParams {
                        mode: GlassDisplacementMode::Frosted,
                        displacement_px: 7.0,
                        scale_px: 28.0,
                        detail: 0.7,
                        angle_degrees: 0.0,
                        seed: params.seed,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "水面ガラス") {
                    *params = GlassDisplacementParams {
                        mode: GlassDisplacementMode::Ripple,
                        displacement_px: 14.0,
                        scale_px: 64.0,
                        detail: 0.45,
                        angle_degrees: 0.0,
                        seed: params.seed,
                        strength: 0.9,
                    };
                    changed = true;
                }
                if preset_button(ui, "面ガラス") {
                    *params = GlassDisplacementParams {
                        mode: GlassDisplacementMode::Faceted,
                        displacement_px: 18.0,
                        scale_px: 46.0,
                        detail: 0.0,
                        angle_degrees: 0.0,
                        seed: params.seed,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "細かい歪み") {
                    *params = GlassDisplacementParams {
                        mode: GlassDisplacementMode::Frosted,
                        displacement_px: 4.0,
                        scale_px: 12.0,
                        detail: 1.0,
                        angle_degrees: 18.0,
                        seed: params.seed,
                        strength: 0.8,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "ノイズや波形を変位マップとして使い、元画像のサンプル位置をずらします。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let frosted = params.mode == GlassDisplacementMode::Frosted;
                if ui.selectable_label(frosted, "すりガラス").clicked() && !frosted {
                    params.mode = GlassDisplacementMode::Frosted;
                    changed = true;
                }
                let ripple = params.mode == GlassDisplacementMode::Ripple;
                if ui.selectable_label(ripple, "波ガラス").clicked() && !ripple {
                    params.mode = GlassDisplacementMode::Ripple;
                    changed = true;
                }
                let faceted = params.mode == GlassDisplacementMode::Faceted;
                if ui.selectable_label(faceted, "面ガラス").clicked() && !faceted {
                    params.mode = GlassDisplacementMode::Faceted;
                    changed = true;
                }
            });
            let displacement = ui.add(
                egui::Slider::new(&mut params.displacement_px, 0.0..=64.0)
                    .text("変位量")
                    .suffix("px"),
            );
            changed |= displacement.changed();
            displacement.lab_hover_tip("サンプル位置を最大でどれだけずらすかです。");
            let scale = ui
                .add(egui::Slider::new(&mut params.scale_px, 2.0..=240.0).text("テクスチャサイズ"));
            changed |= scale.changed();
            scale.lab_hover_tip(
                "変位マップの大きさです。小さいほど細かく、大きいほど大きく歪みます。",
            );
            let detail =
                ui.add(egui::Slider::new(&mut params.detail, 0.0..=1.0).text("ディテール"));
            changed |= detail.changed();
            detail.lab_hover_tip(
                "すりガラスでは細かいノイズ量、波ガラスでは交差方向の波量として働きます。",
            );
            let angle = ui.add(
                egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                    .text("角度")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip(
                "変位マップの向きを回します。波や面ガラスの流れを合わせるときに使います。",
            );
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("ノイズや面ガラスの模様を変えます。");
            params.seed = seed.max(0) as u32;
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からガラス変位結果へどれだけ近づけるかです。");
        }
        LocalEffect::LensCorrection(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "樽型補正") {
                    *params = LensCorrectionParams {
                        distortion: 0.35,
                        zoom: 0.06,
                        center: [0.5, 0.5],
                        vignette_correction: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "糸巻き補正") {
                    *params = LensCorrectionParams {
                        distortion: -0.35,
                        zoom: 0.03,
                        center: [0.5, 0.5],
                        vignette_correction: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "広角強め") {
                    *params = LensCorrectionParams {
                        distortion: 0.62,
                        zoom: 0.14,
                        center: [0.5, 0.5],
                        vignette_correction: 0.12,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "周辺減光補正") {
                    *params = LensCorrectionParams {
                        distortion: 0.0,
                        zoom: 0.0,
                        center: [0.5, 0.5],
                        vignette_correction: 0.48,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "中心から外側へ向かうレンズ歪みを補正します。ズームは補正後の端の伸びやにじみを切るために使います。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let distortion =
                ui.add(egui::Slider::new(&mut params.distortion, -1.0..=1.0).text("歪み補正"));
            changed |= distortion.changed();
            distortion.lab_hover_tip("正で樽型歪みの補正、負で糸巻き歪みの補正です。");
            let vignette = ui.add(
                egui::Slider::new(&mut params.vignette_correction, -1.0..=1.0).text("周辺減光補正"),
            );
            changed |= vignette.changed();
            vignette.lab_hover_tip(
                "正で周辺を持ち上げ、負で周辺を落とします。写真補正では正側を使います。",
            );
            let zoom =
                ui.add(egui::Slider::new(&mut params.zoom, 0.0..=0.5).text("ズーム/切り抜き"));
            changed |= zoom.changed();
            zoom.lab_hover_tip("歪み補正で端が伸びるとき、少し拡大して端を切ります。");
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "レンズ補正の中心位置です。",
                "レンズ補正の中心位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からレンズ補正結果へどれだけ近づけるかです。");
        }
        _ => {
            debug_assert!(
                !is_distort_effect(effect),
                "distort effect dispatch is out of sync"
            );
        }
    }

    EffectParamResponse {
        changed,
        set_effect_position_handles_visible,
        ..Default::default()
    }
}

pub(crate) fn draw_effect_params(
    ui: &mut egui::Ui,
    layer: &mut LocalAdjustmentLayer,
    image_dims: (usize, usize),
    selective_color_pick_active: bool,
    rgb_pick_active: Option<RgbPickTarget>,
    effect_clipboard_available: bool,
    effect_position_handles_visible: bool,
) -> EffectParamResponse {
    let mut changed = false;
    let mut load_cube_lut = false;
    let mut start_selective_color_pick = false;
    let mut cancel_selective_color_pick = false;
    let mut start_rgb_pick = None;
    let mut cancel_rgb_pick = false;
    let mut set_effect_position_handles_visible = None;
    let has_effect = !matches!(&layer.effect, LocalEffect::None);
    let mut copy_effect = false;
    let mut paste_effect = false;
    let mut reset_effect = false;
    ui.horizontal(|ui| {
        ui.label(
            egui::RichText::new("加工パラメータ")
                .size(14.0)
                .strong()
                .color(Color32::WHITE),
        );
        ui.add_space(6.0);
        let copy_response = ui.button("コピー");
        let copy_clicked = copy_response.clicked();
        copy_response.lab_hover_tip("現在の効果種類と加工パラメータをコピーします。");
        if copy_clicked {
            copy_effect = true;
        }
        let paste_response =
            ui.add_enabled(effect_clipboard_available, egui::Button::new("ペースト"));
        let paste_clicked = paste_response.clicked();
        paste_response
            .lab_hover_tip("コピー済みの効果種類と加工パラメータをこのレイヤーへ貼り付けます。");
        if paste_clicked {
            paste_effect = true;
        }
        let reset_response = ui.add_enabled(has_effect, egui::Button::new("リセット"));
        let reset_clicked = reset_response.clicked();
        reset_response.lab_hover_tip("現在の効果を標準値に戻します。");
        if reset_clicked {
            reset_effect = true;
        }
    });
    match &mut layer.effect {
        LocalEffect::None => {
            ui.label("加工内容を選ぶと、このレイヤーの効果が有効になります。");
        }
        tone_detail_effect_patterns!() => {
            let tone_detail_response = draw_tone_detail_effect_params(
                ui,
                &mut layer.effect,
                selective_color_pick_active,
                rgb_pick_active,
            );
            changed |= tone_detail_response.changed;
            start_selective_color_pick |= tone_detail_response.start_selective_color_pick;
            cancel_selective_color_pick |= tone_detail_response.cancel_selective_color_pick;
            cancel_rgb_pick |= tone_detail_response.cancel_rgb_pick;
            if tone_detail_response.start_rgb_pick.is_some() {
                start_rgb_pick = tone_detail_response.start_rgb_pick;
            }
        }
        focus_motion_effect_patterns!() => {
            let focus_motion_response = draw_focus_motion_effect_params(
                ui,
                &mut layer.effect,
                rgb_pick_active,
                effect_position_handles_visible,
            );
            changed |= focus_motion_response.changed;
            cancel_rgb_pick |= focus_motion_response.cancel_rgb_pick;
            if focus_motion_response.start_rgb_pick.is_some() {
                start_rgb_pick = focus_motion_response.start_rgb_pick;
            }
            if focus_motion_response
                .set_effect_position_handles_visible
                .is_some()
            {
                set_effect_position_handles_visible =
                    focus_motion_response.set_effect_position_handles_visible;
            }
        }
        distort_effect_patterns!() => {
            let distort_response =
                draw_distort_effect_params(ui, &mut layer.effect, effect_position_handles_visible);
            changed |= distort_response.changed;
            if distort_response
                .set_effect_position_handles_visible
                .is_some()
            {
                set_effect_position_handles_visible =
                    distort_response.set_effect_position_handles_visible;
            }
        }
        LocalEffect::LineExtract(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "白地黒線") {
                    *params = LineExtractParams {
                        mode: LineExtractMode::BlackOnWhite,
                        threshold: 0.18,
                        softness: 0.1,
                        thickness_px: 1.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "黒地白線") {
                    *params = LineExtractParams {
                        mode: LineExtractMode::WhiteOnBlack,
                        threshold: 0.18,
                        softness: 0.1,
                        thickness_px: 1.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "元画像に黒線") {
                    *params = LineExtractParams {
                        mode: LineExtractMode::DarkenOriginal,
                        threshold: 0.16,
                        softness: 0.12,
                        thickness_px: 1.0,
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "太線") {
                    *params = LineExtractParams {
                        mode: LineExtractMode::BlackOnWhite,
                        threshold: 0.12,
                        softness: 0.08,
                        thickness_px: 2.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "Sobel エッジから線を作ります。しきい値を下げるほど薄い差も線になり、柔らかさで境界をなじませます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let black_on_white = params.mode == LineExtractMode::BlackOnWhite;
                if ui.selectable_label(black_on_white, "白地黒線").clicked() && !black_on_white
                {
                    params.mode = LineExtractMode::BlackOnWhite;
                    changed = true;
                }
                let white_on_black = params.mode == LineExtractMode::WhiteOnBlack;
                if ui.selectable_label(white_on_black, "黒地白線").clicked() && !white_on_black
                {
                    params.mode = LineExtractMode::WhiteOnBlack;
                    changed = true;
                }
                let darken_original = params.mode == LineExtractMode::DarkenOriginal;
                if ui
                    .selectable_label(darken_original, "元画像に黒線")
                    .clicked()
                    && !darken_original
                {
                    params.mode = LineExtractMode::DarkenOriginal;
                    changed = true;
                }
                let lighten_original = params.mode == LineExtractMode::LightenOriginal;
                if ui
                    .selectable_label(lighten_original, "元画像に白線")
                    .clicked()
                    && !lighten_original
                {
                    params.mode = LineExtractMode::LightenOriginal;
                    changed = true;
                }
            });
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.0..=1.0).text("しきい値"));
            changed |= threshold.changed();
            threshold
                .lab_hover_tip("線として拾うエッジの強さです。低いほど細かい差も線になります。");
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.001..=0.5).text("柔らかさ"));
            changed |= softness.changed();
            softness.lab_hover_tip("しきい値付近の線をどれだけなだらかに出すかです。");
            let thickness = ui.add(
                egui::Slider::new(&mut params.thickness_px, 1.0..=8.0)
                    .text("太さ")
                    .suffix("px"),
            );
            changed |= thickness.changed();
            thickness.lab_hover_tip("検出したエッジを周囲へ広げる量です。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から線画抽出結果へどれだけ近づけるかです。");
        }
        LocalEffect::ColorTrace(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "柔らかく") {
                    *params = ColorTraceParams {
                        strength: 0.65,
                        line_threshold: 0.34,
                        softness: 0.18,
                        sample_radius_px: 7.0,
                        darkness: 0.50,
                        saturation: 0.10,
                    };
                    changed = true;
                }
                if preset_button(ui, "濃いめ") {
                    *params = ColorTraceParams {
                        strength: 0.85,
                        line_threshold: 0.40,
                        softness: 0.10,
                        sample_radius_px: 5.0,
                        darkness: 0.68,
                        saturation: 0.18,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡色線") {
                    *params = ColorTraceParams {
                        strength: 0.75,
                        line_threshold: 0.32,
                        softness: 0.16,
                        sample_radius_px: 10.0,
                        darkness: 0.35,
                        saturation: 0.24,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "暗い線画を拾い、線を除外してぼかした周辺色を暗めにして線色へ混ぜます。黒線を下地になじませる仕上げ向けです。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip(
                "元の線色から、周辺色を使った色トレス結果へどれだけ近づけるかです。",
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.line_threshold, 0.02..=0.95).text("線の暗さ"));
            changed |= threshold.changed();
            threshold.lab_hover_tip(
                "線として拾う暗さの上限です。上げると濃い中間色も線として扱います。",
            );
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.001..=0.6).text("判定ぼかし"));
            changed |= softness.changed();
            softness.lab_hover_tip("線検出の境界をどれだけなだらかにするかです。");
            let sample_radius = ui.add(
                egui::Slider::new(&mut params.sample_radius_px, 1.0..=64.0)
                    .text("色サンプル半径")
                    .suffix("px"),
            );
            changed |= sample_radius.changed();
            sample_radius
                .lab_hover_tip("線の周囲から下地色を拾う範囲です。大きいほど広い色になじみます。");
            let darkness =
                ui.add(egui::Slider::new(&mut params.darkness, 0.0..=1.0).text("線の濃さ"));
            changed |= darkness.changed();
            darkness.lab_hover_tip(
                "周辺色をどれだけ暗くして線色に使うかです。高いほど暗い線になります。",
            );
            let saturation =
                ui.add(egui::Slider::new(&mut params.saturation, -1.0..=2.0).text("色の鮮やかさ"));
            changed |= saturation.changed();
            saturation
                .lab_hover_tip("色トレス後の線色の彩度です。負で控えめ、正で鮮やかになります。");
        }
        LocalEffect::ArtisticMedia(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "水彩") {
                    *params = ArtisticMediaParams {
                        mode: ArtisticMediaMode::Watercolor,
                        radius_px: 5.0,
                        edge_strength: 0.35,
                        texture: 0.24,
                        color_amount: 0.85,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡彩") {
                    *params = ArtisticMediaParams {
                        mode: ArtisticMediaMode::Watercolor,
                        radius_px: 8.0,
                        edge_strength: 0.18,
                        texture: 0.12,
                        color_amount: 0.55,
                        strength: 0.85,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "色鉛筆") {
                    *params = ArtisticMediaParams {
                        mode: ArtisticMediaMode::ColoredPencil,
                        radius_px: 2.0,
                        edge_strength: 0.55,
                        texture: 0.48,
                        color_amount: 0.95,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "鉛筆画") {
                    *params = ArtisticMediaParams {
                        mode: ArtisticMediaMode::PencilSketch,
                        radius_px: 1.0,
                        edge_strength: 0.75,
                        texture: 0.55,
                        color_amount: 0.0,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "色をなじませ、輪郭と紙目を足して絵画調に寄せます。鉛筆画では色量を上げると淡い色付きになります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let watercolor = params.mode == ArtisticMediaMode::Watercolor;
                if ui.selectable_label(watercolor, "水彩").clicked() && !watercolor {
                    params.mode = ArtisticMediaMode::Watercolor;
                    changed = true;
                }
                let colored_pencil = params.mode == ArtisticMediaMode::ColoredPencil;
                if ui.selectable_label(colored_pencil, "色鉛筆").clicked() && !colored_pencil {
                    params.mode = ArtisticMediaMode::ColoredPencil;
                    changed = true;
                }
                let pencil = params.mode == ArtisticMediaMode::PencilSketch;
                if ui.selectable_label(pencil, "鉛筆画").clicked() && !pencil {
                    params.mode = ArtisticMediaMode::PencilSketch;
                    changed = true;
                }
            });
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 0.0..=24.0)
                    .text("なじませ")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius.lab_hover_tip(
                "色を面としてなじませる量です。水彩では大きめ、鉛筆では小さめが向いています。",
            );
            let edge = ui.add(egui::Slider::new(&mut params.edge_strength, 0.0..=1.0).text("輪郭"));
            changed |= edge.changed();
            edge.lab_hover_tip("輪郭や筆致をどれだけ強調するかです。");
            let texture =
                ui.add(egui::Slider::new(&mut params.texture, 0.0..=1.0).text("紙目/筆致"));
            changed |= texture.changed();
            texture.lab_hover_tip("紙目ノイズや鉛筆のハッチング量です。");
            let color = ui.add(egui::Slider::new(&mut params.color_amount, 0.0..=1.0).text("色量"));
            changed |= color.changed();
            color.lab_hover_tip("色の残し方です。鉛筆画では 0 にすると白黒寄りになります。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から絵画調結果へどれだけ近づけるかです。");
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("紙目や鉛筆線のパターンを変えます。");
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::BrushStroke(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "ドライブラシ") {
                    *params = BrushStrokeParams {
                        mode: BrushStrokeMode::DryBrush,
                        length_px: 14.0,
                        radius_px: 1.0,
                        angle_degrees: -12.0,
                        texture: 0.72,
                        edge_strength: 0.45,
                        color_amount: 0.85,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "塗料") {
                    *params = BrushStrokeParams {
                        mode: BrushStrokeMode::PaintDaubs,
                        length_px: 18.0,
                        radius_px: 3.0,
                        angle_degrees: -24.0,
                        texture: 0.34,
                        edge_strength: 0.28,
                        color_amount: 1.0,
                        strength: 0.92,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "ナイフ") {
                    *params = BrushStrokeParams {
                        mode: BrushStrokeMode::PaletteKnife,
                        length_px: 34.0,
                        radius_px: 2.0,
                        angle_degrees: 0.0,
                        texture: 0.48,
                        edge_strength: 0.55,
                        color_amount: 0.9,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "斜め筆致") {
                    *params = BrushStrokeParams {
                        mode: BrushStrokeMode::DryBrush,
                        length_px: 24.0,
                        radius_px: 2.0,
                        angle_degrees: -38.0,
                        texture: 0.58,
                        edge_strength: 0.42,
                        color_amount: 0.9,
                        strength: 0.9,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "方向のあるサンプルで色を引き、筆跡・厚塗り・ナイフ跡のテクスチャを重ねます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let dry = params.mode == BrushStrokeMode::DryBrush;
                if ui.selectable_label(dry, "ドライ").clicked() && !dry {
                    params.mode = BrushStrokeMode::DryBrush;
                    changed = true;
                }
                let paint = params.mode == BrushStrokeMode::PaintDaubs;
                if ui.selectable_label(paint, "塗料").clicked() && !paint {
                    params.mode = BrushStrokeMode::PaintDaubs;
                    changed = true;
                }
                let knife = params.mode == BrushStrokeMode::PaletteKnife;
                if ui.selectable_label(knife, "ナイフ").clicked() && !knife {
                    params.mode = BrushStrokeMode::PaletteKnife;
                    changed = true;
                }
            });
            let length = ui.add(
                egui::Slider::new(&mut params.length_px, 0.0..=72.0)
                    .text("ストローク長")
                    .suffix("px"),
            );
            changed |= length.changed();
            length.lab_hover_tip("筆跡として色を引く長さです。");
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 0.0..=12.0)
                    .text("幅")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius.lab_hover_tip("筆跡の横方向の揺れや幅です。");
            let angle = ui.add(
                egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                    .text("角度")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip("筆跡の方向です。");
            let texture = ui.add(egui::Slider::new(&mut params.texture, 0.0..=1.0).text("筆致"));
            changed |= texture.changed();
            texture.lab_hover_tip("ドライ感、塗料の凹凸、ナイフ跡の強さです。");
            let edge = ui.add(egui::Slider::new(&mut params.edge_strength, 0.0..=1.0).text("輪郭"));
            changed |= edge.changed();
            edge.lab_hover_tip("輪郭やストロークの硬さをどれだけ出すかです。");
            let color = ui.add(egui::Slider::new(&mut params.color_amount, 0.0..=1.0).text("色量"));
            changed |= color.changed();
            color.lab_hover_tip("元の色の鮮やかさをどれだけ残すかです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から筆致結果へどれだけ近づけるかです。");
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("筆致テクスチャのパターンを変えます。");
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::Cutout(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "フラット") {
                    *params = CutoutParams {
                        levels: 5,
                        radius_px: 6.0,
                        edge_strength: 0.22,
                        color_amount: 0.9,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ポスター") {
                    *params = CutoutParams {
                        levels: 4,
                        radius_px: 3.0,
                        edge_strength: 0.12,
                        color_amount: 1.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかめ") {
                    *params = CutoutParams {
                        levels: 6,
                        radius_px: 10.0,
                        edge_strength: 0.08,
                        color_amount: 0.75,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "輪郭強め") {
                    *params = CutoutParams {
                        levels: 5,
                        radius_px: 5.0,
                        edge_strength: 0.55,
                        color_amount: 0.85,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "色面をなじませて階調を減らし、切り絵やフラットなベクター調に寄せます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let mut levels = params.levels as i32;
            let levels_response = ui.add(egui::Slider::new(&mut levels, 2..=12).text("階調"));
            changed |= levels_response.changed();
            levels_response.lab_hover_tip("色面の明るさを何段階にまとめるかです。");
            params.levels = levels.clamp(2, 12) as u8;
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 0.0..=24.0)
                    .text("面のなじませ")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius
                .lab_hover_tip("階調化の前に色をなじませる量です。大きいほど大きな面になります。");
            let edge = ui.add(egui::Slider::new(&mut params.edge_strength, 0.0..=1.0).text("輪郭"));
            changed |= edge.changed();
            edge.lab_hover_tip("面の境界や元画像のエッジをどれだけ締めるかです。");
            let color = ui.add(egui::Slider::new(&mut params.color_amount, 0.0..=1.0).text("色量"));
            changed |= color.changed();
            color.lab_hover_tip("元の色の鮮やかさをどれだけ残すかです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から切り絵結果へどれだけ近づけるかです。");
        }
        LocalEffect::ToonShade(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "セル塗り") {
                    *params = ToonShadeParams {
                        bands: 4,
                        softness: 0.06,
                        preserve_hue: true,
                        shadow_tint_rgb: [92, 116, 210],
                        shadow_tint_strength: 0.18,
                        light_tint_rgb: [255, 226, 176],
                        light_tint_strength: 0.12,
                        outline_strength: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "硬い影") {
                    *params = ToonShadeParams {
                        bands: 3,
                        softness: 0.0,
                        preserve_hue: true,
                        shadow_tint_rgb: [72, 92, 190],
                        shadow_tint_strength: 0.28,
                        light_tint_rgb: [255, 228, 182],
                        light_tint_strength: 0.08,
                        outline_strength: 0.12,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかめ") {
                    *params = ToonShadeParams {
                        bands: 5,
                        softness: 0.34,
                        preserve_hue: true,
                        shadow_tint_rgb: [100, 128, 220],
                        shadow_tint_strength: 0.12,
                        light_tint_rgb: [255, 236, 202],
                        light_tint_strength: 0.10,
                        outline_strength: 0.0,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "境界線") {
                    *params = ToonShadeParams {
                        bands: 4,
                        softness: 0.02,
                        preserve_hue: true,
                        shadow_tint_rgb: [84, 104, 198],
                        shadow_tint_strength: 0.16,
                        light_tint_rgb: [255, 228, 188],
                        light_tint_strength: 0.10,
                        outline_strength: 0.55,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明度だけを段階化して色相を保ち、セル画風の影面と光面を作ります。影色・光色を少し混ぜるとアニメ塗りらしくなります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let mut bands = params.bands as i32;
            let bands_response = ui.add(egui::Slider::new(&mut bands, 2..=8).text("階調数"));
            changed |= bands_response.changed();
            bands_response.lab_hover_tip(
                "明るさを何段階の面にまとめるかです。少ないほどセル塗りが強くなります。",
            );
            params.bands = bands.clamp(2, 8) as u8;
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.0..=1.0).text("境界の柔らかさ"));
            changed |= softness.changed();
            softness.lab_hover_tip("0で硬いセル影、上げるほど帯境界をなめらかにつなぎます。");
            let preserve_hue = ui.checkbox(&mut params.preserve_hue, "色相を維持");
            changed |= preserve_hue.changed();
            preserve_hue.lab_hover_tip(
                "ONでは明度だけを段階化します。OFFではRGBも段階化し、よりグラフィックになります。",
            );
            let shadow_strength =
                ui.add(egui::Slider::new(&mut params.shadow_tint_strength, 0.0..=1.0).text("影色"));
            changed |= shadow_strength.changed();
            shadow_strength.lab_hover_tip("暗い帯へ指定した影色を混ぜる量です。");
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "影色",
                    &mut params.shadow_tint_rgb,
                    RgbPickTarget::ToonShadeShadowTint,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let light_strength =
                ui.add(egui::Slider::new(&mut params.light_tint_strength, 0.0..=1.0).text("光色"));
            changed |= light_strength.changed();
            light_strength.lab_hover_tip("明るい帯へ指定した光色を混ぜる量です。");
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "光色",
                    &mut params.light_tint_rgb,
                    RgbPickTarget::ToonShadeLightTint,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let outline =
                ui.add(egui::Slider::new(&mut params.outline_strength, 0.0..=1.0).text("段差線"));
            changed |= outline.changed();
            outline.lab_hover_tip("明度帯の境界をどれだけ暗く締めるかです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からトゥーンシェード結果へどれだけ近づけるかです。");
        }
        LocalEffect::Emboss(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "浅い") {
                    *params = EmbossParams {
                        angle_degrees: 135.0,
                        depth: 0.7,
                        contrast: 0.12,
                        color_amount: 0.0,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "くっきり") {
                    *params = EmbossParams {
                        angle_degrees: 135.0,
                        depth: 1.35,
                        contrast: 0.45,
                        color_amount: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "金属調") {
                    *params = EmbossParams {
                        angle_degrees: 120.0,
                        depth: 1.65,
                        contrast: 0.8,
                        color_amount: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "色付き") {
                    *params = EmbossParams {
                        angle_degrees: 135.0,
                        depth: 1.0,
                        contrast: 0.35,
                        color_amount: 0.55,
                        strength: 0.9,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明るさの傾きから陰影を作り、紙や金属の浮き彫りのように見せます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let angle = ui.add(
                egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                    .text("角度")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip("光が当たる方向です。180度変えると凹凸の向きが反転します。");
            let depth = ui.add(egui::Slider::new(&mut params.depth, 0.0..=4.0).text("深さ"));
            changed |= depth.changed();
            depth.lab_hover_tip("明暗差をどれだけ浮き彫りの陰影へ変換するかです。");
            let contrast =
                ui.add(egui::Slider::new(&mut params.contrast, -1.0..=1.0).text("コントラスト"));
            changed |= contrast.changed();
            contrast.lab_hover_tip("エンボス陰影の硬さです。高いほど金属的に締まります。");
            let color = ui.add(egui::Slider::new(&mut params.color_amount, 0.0..=1.0).text("色量"));
            changed |= color.changed();
            color.lab_hover_tip("0ではモノクロ、上げると元画像の色を浮き彫りに残します。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からエンボス結果へどれだけ近づけるかです。");
        }
        LocalEffect::PixelStylize(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "結晶化") {
                    *params = PixelStylizeParams {
                        mode: PixelStylizeMode::Crystallize,
                        cell_px: 16.0,
                        edge_strength: 0.35,
                        color_amount: 0.9,
                        randomness: 0.65,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "点描") {
                    *params = PixelStylizeParams {
                        mode: PixelStylizeMode::Pointillize,
                        cell_px: 11.0,
                        edge_strength: 0.08,
                        color_amount: 1.0,
                        randomness: 0.55,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "Facet") {
                    *params = PixelStylizeParams {
                        mode: PixelStylizeMode::Facet,
                        cell_px: 18.0,
                        edge_strength: 0.25,
                        color_amount: 0.95,
                        randomness: 0.35,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "メゾチント") {
                    *params = PixelStylizeParams {
                        mode: PixelStylizeMode::Mezzotint,
                        cell_px: 3.0,
                        edge_strength: 0.0,
                        color_amount: 0.0,
                        randomness: 0.8,
                        strength: 1.0,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "セルや粒で色を再構成します。結晶化/Facet は面、点描/メゾチントは粒の表現に向いています。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let crystallize = params.mode == PixelStylizeMode::Crystallize;
                if ui.selectable_label(crystallize, "結晶化").clicked() && !crystallize {
                    params.mode = PixelStylizeMode::Crystallize;
                    changed = true;
                }
                let pointillize = params.mode == PixelStylizeMode::Pointillize;
                if ui.selectable_label(pointillize, "点描").clicked() && !pointillize {
                    params.mode = PixelStylizeMode::Pointillize;
                    changed = true;
                }
                let facet = params.mode == PixelStylizeMode::Facet;
                if ui.selectable_label(facet, "Facet").clicked() && !facet {
                    params.mode = PixelStylizeMode::Facet;
                    changed = true;
                }
                let mezzotint = params.mode == PixelStylizeMode::Mezzotint;
                if ui.selectable_label(mezzotint, "メゾチント").clicked() && !mezzotint {
                    params.mode = PixelStylizeMode::Mezzotint;
                    changed = true;
                }
            });
            let size = ui.add(
                egui::Slider::new(&mut params.cell_px, 1.0..=48.0)
                    .text("サイズ")
                    .suffix("px"),
            );
            changed |= size.changed();
            size.lab_hover_tip("結晶や点の大きさです。メゾチントでは粒の粗さとして働きます。");
            let edge = ui.add(egui::Slider::new(&mut params.edge_strength, 0.0..=1.0).text("輪郭"));
            changed |= edge.changed();
            edge.lab_hover_tip("面や粒の境界、元画像の輪郭をどれだけ締めるかです。");
            let color = ui.add(egui::Slider::new(&mut params.color_amount, 0.0..=1.0).text("色量"));
            changed |= color.changed();
            color.lab_hover_tip("0ではモノクロ寄り、上げると元画像の色を強く残します。");
            let randomness =
                ui.add(egui::Slider::new(&mut params.randomness, 0.0..=1.0).text("ばらつき"));
            changed |= randomness.changed();
            randomness.lab_hover_tip("セル位置や粒のランダムさです。下げると規則的になります。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から粒状スタイル結果へどれだけ近づけるかです。");
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("セルや粒の配置パターンを変えます。");
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::Solarize(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "標準") {
                    *params = SolarizeParams {
                        threshold: 0.55,
                        softness: 0.08,
                        inversion: 1.0,
                        contrast: 0.05,
                        color_amount: 1.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかい") {
                    *params = SolarizeParams {
                        threshold: 0.52,
                        softness: 0.22,
                        inversion: 0.85,
                        contrast: -0.05,
                        color_amount: 0.85,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "白黒") {
                    *params = SolarizeParams {
                        threshold: 0.50,
                        softness: 0.08,
                        inversion: 1.0,
                        contrast: 0.25,
                        color_amount: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ハイライト") {
                    *params = SolarizeParams {
                        threshold: 0.68,
                        softness: 0.06,
                        inversion: 1.0,
                        contrast: 0.15,
                        color_amount: 1.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "しきい値より明るいトーンを反転します。ネガより部分的で、境目の色ずれや暗室風の効果を作れます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.0..=1.0).text("しきい値"));
            changed |= threshold.changed();
            threshold
                .lab_hover_tip("反転を始める明るさです。上げるほどハイライトだけが反転します。");
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.0..=0.5).text("柔らかさ"));
            changed |= softness.changed();
            softness.lab_hover_tip("しきい値前後の反転をどれだけなだらかにするかです。");
            let inversion =
                ui.add(egui::Slider::new(&mut params.inversion, 0.0..=1.0).text("反転量"));
            changed |= inversion.changed();
            inversion.lab_hover_tip("明るいトーンを反対側へ折り返す量です。");
            let color = ui.add(egui::Slider::new(&mut params.color_amount, 0.0..=1.0).text("色量"));
            changed |= color.changed();
            color.lab_hover_tip(
                "0では白黒のトーン反転、上げるとRGBチャンネルごとの色ずれを残します。",
            );
            let contrast =
                ui.add(egui::Slider::new(&mut params.contrast, -1.0..=1.0).text("コントラスト"));
            changed |= contrast.changed();
            contrast.lab_hover_tip("反転後の明暗差を締めたり、柔らかくしたりします。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からソラリゼーション結果へどれだけ近づけるかです。");
        }
        LocalEffect::GlowingEdges(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "シアン") {
                    *params = GlowingEdgesParams {
                        threshold: 0.18,
                        softness: 0.10,
                        edge_width_px: 1.0,
                        glow_radius_px: 8.0,
                        edge_brightness: 1.20,
                        glow_strength: 0.90,
                        hue_degrees: 190.0,
                        color_amount: 0.90,
                        background_amount: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "紫ネオン") {
                    *params = GlowingEdgesParams {
                        threshold: 0.15,
                        softness: 0.12,
                        edge_width_px: 2.0,
                        glow_radius_px: 12.0,
                        edge_brightness: 1.25,
                        glow_strength: 1.05,
                        hue_degrees: 285.0,
                        color_amount: 0.95,
                        background_amount: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "元画像") {
                    *params = GlowingEdgesParams {
                        threshold: 0.20,
                        softness: 0.12,
                        edge_width_px: 1.0,
                        glow_radius_px: 7.0,
                        edge_brightness: 0.95,
                        glow_strength: 0.75,
                        hue_degrees: 200.0,
                        color_amount: 0.65,
                        background_amount: 0.65,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "細線") {
                    *params = GlowingEdgesParams {
                        threshold: 0.28,
                        softness: 0.04,
                        edge_width_px: 1.0,
                        glow_radius_px: 3.0,
                        edge_brightness: 1.55,
                        glow_strength: 0.35,
                        hue_degrees: 145.0,
                        color_amount: 1.0,
                        background_amount: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明るさの輪郭を抽出し、黒背景または元画像上にネオン色の線と光彩を重ねます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.0..=1.0).text("しきい値"));
            changed |= threshold.changed();
            threshold.lab_hover_tip("光らせる輪郭の強さです。上げるほど強い輪郭だけが残ります。");
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.0..=0.5).text("柔らかさ"));
            changed |= softness.changed();
            softness.lab_hover_tip("しきい値付近の輪郭をどれだけなだらかに出すかです。");
            let edge_width = ui.add(
                egui::Slider::new(&mut params.edge_width_px, 1.0..=12.0)
                    .text("線幅")
                    .suffix("px"),
            );
            changed |= edge_width.changed();
            edge_width.lab_hover_tip("抽出した輪郭を広げる幅です。");
            let glow_radius = ui.add(
                egui::Slider::new(&mut params.glow_radius_px, 0.0..=80.0)
                    .text("光彩半径")
                    .suffix("px"),
            );
            changed |= glow_radius.changed();
            glow_radius.lab_hover_tip("輪郭の周囲へ広げる発光の大きさです。");
            let edge_brightness = ui
                .add(egui::Slider::new(&mut params.edge_brightness, 0.0..=3.0).text("線の明るさ"));
            changed |= edge_brightness.changed();
            edge_brightness.lab_hover_tip("輪郭線そのものの明るさです。");
            let glow_strength =
                ui.add(egui::Slider::new(&mut params.glow_strength, 0.0..=3.0).text("光彩"));
            changed |= glow_strength.changed();
            glow_strength.lab_hover_tip("ぼかした発光をどれだけ加えるかです。");
            ui.horizontal_wrapped(|ui| {
                let hue = ui.add(
                    egui::Slider::new(&mut params.hue_degrees, 0.0..=360.0)
                        .text("色相")
                        .suffix("°"),
                );
                changed |= hue.changed();
                hue.lab_hover_tip("ネオン色の色相です。");
                let swatch = hsl_swatch_color(params.hue_degrees, 1.0, 0.55);
                let (rect, _) = ui.allocate_exact_size(egui::vec2(22.0, 22.0), Sense::hover());
                ui.painter().rect_filled(rect, 4.0, swatch);
                ui.painter().rect_stroke(
                    rect,
                    4.0,
                    egui::Stroke::new(1.0, Color32::from_rgba_unmultiplied(255, 255, 255, 110)),
                    egui::StrokeKind::Inside,
                );
            });
            let color = ui.add(egui::Slider::new(&mut params.color_amount, 0.0..=1.0).text("色量"));
            changed |= color.changed();
            color.lab_hover_tip("0では元画像の輪郭色、上げると指定したネオン色へ寄せます。");
            let background = ui.add(
                egui::Slider::new(&mut params.background_amount, 0.0..=1.0).text("背景を残す"),
            );
            changed |= background.changed();
            background.lab_hover_tip("0では黒背景、上げると元画像を背景として残します。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からエッジ光彩結果へどれだけ近づけるかです。");
        }
        LocalEffect::OilPaint(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "標準") {
                    *params = OilPaintParams {
                        radius_px: 5.0,
                        saturation: 0.08,
                        contrast: 0.04,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "厚塗り") {
                    *params = OilPaintParams {
                        radius_px: 8.0,
                        saturation: 0.18,
                        contrast: 0.14,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかめ") {
                    *params = OilPaintParams {
                        radius_px: 6.0,
                        saturation: -0.04,
                        contrast: -0.08,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "細部残し") {
                    *params = OilPaintParams {
                        radius_px: 3.0,
                        saturation: 0.04,
                        contrast: 0.0,
                        strength: 0.75,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "4象限の輝度分散が小さい領域を選んで平均色に置き換える Kuwahara 系の油彩化です。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 1.0..=12.0)
                    .text("半径")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius.lab_hover_tip("色面をなじませる範囲です。大きいほど厚塗り風になります。");
            let saturation =
                ui.add(egui::Slider::new(&mut params.saturation, -1.0..=1.0).text("彩度"));
            changed |= saturation.changed();
            saturation.lab_hover_tip("油彩化した色面の鮮やかさです。");
            let contrast =
                ui.add(egui::Slider::new(&mut params.contrast, -1.0..=1.0).text("コントラスト"));
            changed |= contrast.changed();
            contrast.lab_hover_tip("油彩化した色面の明暗差を締めたり柔らかくしたりします。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から油彩結果へどれだけ近づけるかです。");
        }
        LocalEffect::SoftFocus(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "淡く") {
                    *params = SoftFocusParams {
                        radius_px: 16.0,
                        strength: 0.25,
                    };
                    changed = true;
                }
                if preset_button(ui, "発光") {
                    *params = SoftFocusParams {
                        radius_px: 28.0,
                        strength: 0.45,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.radius_px, 0.0..=80.0).text("半径"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"))
                .changed();
        }
        LocalEffect::Orton(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "自然") {
                    *params = OrtonParams {
                        radius_px: 24.0,
                        strength: 0.32,
                        brightness: 0.28,
                        contrast: 0.12,
                        saturation: 0.08,
                    };
                    changed = true;
                }
                if preset_button(ui, "夢幻") {
                    *params = OrtonParams {
                        radius_px: 42.0,
                        strength: 0.58,
                        brightness: 0.48,
                        contrast: 0.18,
                        saturation: 0.18,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡い逆光") {
                    *params = OrtonParams {
                        radius_px: 64.0,
                        strength: 0.44,
                        brightness: 0.62,
                        contrast: -0.12,
                        saturation: -0.08,
                    };
                    changed = true;
                }
                if preset_button(ui, "高彩度") {
                    *params = OrtonParams {
                        radius_px: 30.0,
                        strength: 0.50,
                        brightness: 0.40,
                        contrast: 0.28,
                        saturation: 0.35,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "ぼかして明るくしたコピーをスクリーン合成し、柔らかい光と少し濃い色を足します。Soft Focus よりルック寄りの仕上げです。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 0.0..=160.0)
                    .text("半径")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius.lab_hover_tip(
                "重ねるボケコピーの半径です。大きいほど全体に柔らかく回り込みます。",
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からオートン効果へどれだけ近づけるかです。");
            let brightness =
                ui.add(egui::Slider::new(&mut params.brightness, 0.0..=1.0).text("明るさ"));
            changed |= brightness.changed();
            brightness.lab_hover_tip("ボケコピーをどれだけ明るく持ち上げるかです。");
            let contrast =
                ui.add(egui::Slider::new(&mut params.contrast, -1.0..=1.0).text("コントラスト"));
            changed |= contrast.changed();
            contrast
                .lab_hover_tip("ボケコピー側の明暗差です。正で締まり、負でさらに淡くなります。");
            let saturation =
                ui.add(egui::Slider::new(&mut params.saturation, -1.0..=1.0).text("彩度"));
            changed |= saturation.changed();
            saturation.lab_hover_tip("ボケコピー側の鮮やかさです。");
        }
        LocalEffect::Mosaic(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "長辺0.5倍") {
                    params.tile_mode = MosaicTileMode::LongEdgeRatio(0.5);
                    params.clear_legacy_block_px();
                    changed = true;
                }
                if preset_button(ui, "長辺1倍") {
                    params.tile_mode = MosaicTileMode::LongEdgeRatio(1.0);
                    params.clear_legacy_block_px();
                    changed = true;
                }
                if preset_button(ui, "長辺2倍") {
                    params.tile_mode = MosaicTileMode::LongEdgeRatio(2.0);
                    params.clear_legacy_block_px();
                    changed = true;
                }
            });
            let long_edge = image_dims.0.max(image_dims.1) as u32;
            let mut mode = params.effective_tile_mode();
            ui.horizontal(|ui| {
                let ratio_selected = matches!(mode, MosaicTileMode::LongEdgeRatio(_));
                if ui.selectable_label(ratio_selected, "長辺比率").clicked() && !ratio_selected
                {
                    let multiplier = match mode {
                        MosaicTileMode::LongEdgeRatio(value) => value,
                        MosaicTileMode::FixedPx(_) => 1.0,
                    };
                    mode = MosaicTileMode::LongEdgeRatio(multiplier);
                    params.tile_mode = mode;
                    params.clear_legacy_block_px();
                    changed = true;
                }
                let fixed_selected = matches!(mode, MosaicTileMode::FixedPx(_));
                if ui.selectable_label(fixed_selected, "固定px").clicked() && !fixed_selected {
                    let fixed_px = compute_mosaic_tile_size(long_edge, mode).max(1);
                    mode = MosaicTileMode::FixedPx(fixed_px);
                    params.tile_mode = mode;
                    params.clear_legacy_block_px();
                    changed = true;
                }
            });
            match mode {
                MosaicTileMode::LongEdgeRatio(multiplier) => {
                    let mut value = multiplier;
                    let response = ui.add(
                        egui::Slider::new(&mut value, 0.25..=5.0)
                            .step_by(0.25)
                            .text("長辺比率"),
                    );
                    if response.changed() {
                        params.tile_mode = MosaicTileMode::LongEdgeRatio(value);
                        params.clear_legacy_block_px();
                        mode = params.tile_mode;
                        changed = true;
                    }
                }
                MosaicTileMode::FixedPx(px) => {
                    let mut value = px as i32;
                    let response = ui.add(egui::Slider::new(&mut value, 1..=200).text("固定px"));
                    if response.changed() {
                        params.tile_mode = MosaicTileMode::FixedPx(value.max(1) as u32);
                        params.clear_legacy_block_px();
                        mode = params.tile_mode;
                        changed = true;
                    }
                }
            }
            let actual_px = compute_mosaic_tile_size(long_edge, mode);
            ui.label(
                egui::RichText::new(format!("実タイルサイズ: {actual_px}px"))
                    .size(11.0)
                    .color(Color32::from_gray(170)),
            );

            ui.separator();
            let before_boundary = params.boundary;
            lab_combo_box(
                ui,
                "mosaic_boundary",
                mosaic_boundary_label(params.boundary),
                |ui| {
                    for boundary in [
                        MosaicBoundary::Opaque,
                        MosaicBoundary::Translucent,
                        MosaicBoundary::MaskShape,
                    ] {
                        ui.selectable_value(
                            &mut params.boundary,
                            boundary,
                            mosaic_boundary_label(boundary),
                        );
                    }
                },
            );
            if params.boundary != before_boundary {
                changed = true;
            }
            ui.label(
                egui::RichText::new(params.boundary.process_description())
                    .size(10.0)
                    .color(Color32::from_gray(170)),
            );
            if matches!(params.boundary, MosaicBoundary::Opaque) {
                ui.label(
                    egui::RichText::new(
                        "隠蔽加工と同じく、マスクに触れたタイル全体へ効果が広がります。",
                    )
                    .size(10.0)
                    .color(Color32::from_gray(170)),
                );
            }
        }
        LocalEffect::Sharpen(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱く") {
                    *params = SharpenParams {
                        amount: 0.35,
                        radius_px: 1.0,
                        threshold: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "くっきり") {
                    *params = SharpenParams {
                        amount: 0.7,
                        radius_px: 1.0,
                        threshold: 4.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "線強調") {
                    *params = SharpenParams {
                        amount: 0.55,
                        radius_px: 2.0,
                        threshold: 8.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ノイズ抑制") {
                    *params = SharpenParams {
                        amount: 0.95,
                        radius_px: 1.5,
                        threshold: 12.0,
                    };
                    changed = true;
                }
            });
            let amount = ui.add(egui::Slider::new(&mut params.amount, 0.0..=2.0).text("量"));
            changed |= amount.changed();
            amount
                .lab_hover_tip("輪郭へ足し戻す強さです。上げすぎると白黒の縁が出やすくなります。");
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 0.0..=12.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip(
                "輪郭として扱う幅です。小さい値は細部、大きい値は太い輪郭に効きます。",
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.0..=64.0).text("しきい値"));
            changed |= threshold.changed();
            threshold.lab_hover_tip(
                "小さな明暗差を無視する量です。値を上げるとノイズや微妙なざらつきに効きにくくなります。",
            );
        }
        LocalEffect::SmartSharpen(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "自然") {
                    *params = SmartSharpenParams {
                        amount: 0.65,
                        radius_px: 2.0,
                        edge_threshold: 0.08,
                        halo_suppression: 0.65,
                    };
                    changed = true;
                }
                if preset_button(ui, "細部") {
                    *params = SmartSharpenParams {
                        amount: 0.95,
                        radius_px: 1.2,
                        edge_threshold: 0.05,
                        halo_suppression: 0.45,
                    };
                    changed = true;
                }
                if preset_button(ui, "輪郭") {
                    *params = SmartSharpenParams {
                        amount: 1.15,
                        radius_px: 3.0,
                        edge_threshold: 0.12,
                        halo_suppression: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "フチ抑制") {
                    *params = SmartSharpenParams {
                        amount: 1.2,
                        radius_px: 2.4,
                        edge_threshold: 0.08,
                        halo_suppression: 1.0,
                    };
                    changed = true;
                }
            });
            let amount = ui.add(egui::Slider::new(&mut params.amount, 0.0..=2.0).text("量"));
            changed |= amount.changed();
            amount.lab_hover_tip(
                "輪郭に足し戻す強さです。通常のシャープよりエッジを選んで効きます。",
            );
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 0.0..=16.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip(
                "復元する輪郭の幅です。細部は小さく、太い線や境界は大きめにします。",
            );
            let edge_threshold =
                ui.add(egui::Slider::new(&mut params.edge_threshold, 0.0..=0.5).text("エッジ判定"));
            changed |= edge_threshold.changed();
            edge_threshold.lab_hover_tip(
                "どれだけ明暗差がある場所を輪郭として扱うかです。上げると平坦部に効きにくくなります。",
            );
            let halo =
                ui.add(egui::Slider::new(&mut params.halo_suppression, 0.0..=1.0).text("フチ抑制"));
            changed |= halo.changed();
            halo.lab_hover_tip("明るいフチや暗いフチが立ちすぎる方向の強調を抑えます。");
        }
        LocalEffect::Hsl(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "赤へ") {
                    *params = HslParams {
                        hue_degrees: -25.0,
                        saturation: 10.0,
                        lightness: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "青へ") {
                    *params = HslParams {
                        hue_degrees: 70.0,
                        saturation: 8.0,
                        lightness: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "緑へ") {
                    *params = HslParams {
                        hue_degrees: 120.0,
                        saturation: 8.0,
                        lightness: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "彩度+") {
                    *params = HslParams {
                        saturation: 25.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "淡く") {
                    *params = HslParams {
                        saturation: -25.0,
                        lightness: 8.0,
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new("カラー範囲マスクと組み合わせると、髪や服だけ色替えできます。")
                    .size(10.0)
                    .color(Color32::from_gray(170)),
            );
            changed |= ui
                .add(egui::Slider::new(&mut params.hue_degrees, -180.0..=180.0).text("色相"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.saturation, -100.0..=100.0).text("彩度"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.lightness, -100.0..=100.0).text("明度"))
                .changed();
        }
        LocalEffect::ColorMixer(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "空を濃く") {
                    *params = ColorMixerParams::default();
                    params.bands[4].saturation = 18.0;
                    params.bands[4].lightness = -6.0;
                    params.bands[5].saturation = 26.0;
                    params.bands[5].lightness = -10.0;
                    changed = true;
                }
                if preset_button(ui, "緑を鮮やか") {
                    *params = ColorMixerParams::default();
                    params.bands[3].saturation = 32.0;
                    params.bands[3].lightness = 4.0;
                    changed = true;
                }
                if preset_button(ui, "肌を明るく") {
                    *params = ColorMixerParams::default();
                    params.bands[1].saturation = 8.0;
                    params.bands[1].lightness = 12.0;
                    params.bands[2].lightness = 4.0;
                    changed = true;
                }
                if preset_button(ui, "赤を桜色") {
                    *params = ColorMixerParams::default();
                    params.bands[0].hue_degrees = 16.0;
                    params.bands[0].saturation = -8.0;
                    params.bands[0].lightness = 8.0;
                    params.bands[7].lightness = 6.0;
                    changed = true;
                }
                if preset_button(ui, "青を紫へ") {
                    *params = ColorMixerParams::default();
                    params.bands[5].hue_degrees = 32.0;
                    params.bands[5].saturation = 10.0;
                    params.bands[6].saturation = 12.0;
                    changed = true;
                }
                if preset_button(ui, "黄を橙へ") {
                    *params = ColorMixerParams::default();
                    params.bands[2].hue_degrees = -18.0;
                    params.bands[2].saturation = 12.0;
                    params.bands[1].saturation = 10.0;
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "色相ごとに補正します。カラー範囲マスクなしでも、近い色だけをまとめて調整できます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let range_response =
                ui.add(egui::Slider::new(&mut params.range_degrees, 8.0..=90.0).text("色帯の広さ"));
            changed |= range_response.changed();
            range_response.lab_hover_tip("大きくすると隣の色にもなだらかに効果が広がります。");
            for (idx, band) in params.bands.iter_mut().enumerate() {
                ui.collapsing(color_mixer_band_label(idx), |ui| {
                    let hue = ui
                        .add(egui::Slider::new(&mut band.hue_degrees, -180.0..=180.0).text("色相"));
                    changed |= hue.changed();
                    hue.lab_hover_tip("この色帯だけ色相をずらします。");
                    let saturation = ui
                        .add(egui::Slider::new(&mut band.saturation, -100.0..=100.0).text("彩度"));
                    changed |= saturation.changed();
                    saturation.lab_hover_tip("この色帯だけ鮮やかさを増減します。");
                    let lightness =
                        ui.add(egui::Slider::new(&mut band.lightness, -100.0..=100.0).text("明度"));
                    changed |= lightness.changed();
                    lightness.lab_hover_tip("この色帯だけ明るさを増減します。");
                });
            }
        }
        LocalEffect::Look(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "夕焼け") {
                    *params = LookParams {
                        preset: LookPreset::Sunset,
                        strength: 0.7,
                    };
                    changed = true;
                }
                if preset_button(ui, "夜景") {
                    *params = LookParams {
                        preset: LookPreset::Night,
                        strength: 0.7,
                    };
                    changed = true;
                }
                if preset_button(ui, "明るい日光") {
                    *params = LookParams {
                        preset: LookPreset::BrightSun,
                        strength: 0.65,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡色") {
                    *params = LookParams {
                        preset: LookPreset::Pale,
                        strength: 0.75,
                    };
                    changed = true;
                }
            });
            let before = params.preset;
            lab_combo_box(ui, "look_preset", look_preset_label(params.preset), |ui| {
                for preset in [
                    LookPreset::None,
                    LookPreset::Sunset,
                    LookPreset::Night,
                    LookPreset::BrightSun,
                    LookPreset::Pale,
                    LookPreset::Cool,
                    LookPreset::Warm,
                    LookPreset::RetroFilm,
                    LookPreset::TealOrange,
                    LookPreset::CherryBlossom,
                    LookPreset::FreshGreen,
                    LookPreset::Moonlight,
                    LookPreset::HighKey,
                    LookPreset::LowKey,
                    LookPreset::Sepia,
                    LookPreset::Cyberpunk,
                ] {
                    ui.selectable_value(&mut params.preset, preset, look_preset_label(preset));
                }
            });
            if params.preset != before {
                if params.preset != LookPreset::None && params.strength <= f32::EPSILON {
                    params.strength = 1.0;
                }
                changed = true;
            }
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
        }
        LocalEffect::CubeLut(params) => {
            ui.label(egui::RichText::new("LUTファイル").color(Color32::from_gray(190)));
            if params.is_loaded() {
                ui.label(format!("読み込み済み: {} ({}^3)", params.name, params.size));
            } else {
                ui.label("未読み込みです。`.cube` ファイルを選択してください。");
            }
            if ui.button("LUTファイルを選択").clicked() {
                load_cube_lut = true;
            }
            ui.label(
                egui::RichText::new(
                    "3D LUT は RGB の組み合わせごとに色を変換する外部カラープリセットです。読み込んだ LUT データは設定ファイルにも保存されます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から LUT 変換後の色へどれだけ近づけるかです。");
        }
        LocalEffect::Posterize(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱 16段") {
                    *params = PosterizeParams {
                        levels: 16,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "中 8段") {
                    *params = PosterizeParams {
                        levels: 8,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "強 5段") {
                    *params = PosterizeParams {
                        levels: 5,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "超強 3段") {
                    *params = PosterizeParams {
                        levels: 3,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "RGB各チャンネルの階調を指定段数へ丸めます。色数を減らしたポスター調やレトロ調に使います。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let levels = ui.add(egui::Slider::new(&mut params.levels, 2..=256).text("階調数"));
            changed |= levels.changed();
            levels.lab_hover_tip("値を小さくすると、使われる明るさの段数が減ってフラットになります。256でほぼ無加工です。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から階調を減らした色へどれだけ近づけるかです。");
        }
        LocalEffect::RetroPalette(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "1bit") {
                    *params = RetroPaletteParams {
                        mode: RetroPaletteMode::Dither1Bit,
                        dither: 0.70,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "GameBoy") {
                    *params = RetroPaletteParams {
                        mode: RetroPaletteMode::GameBoy,
                        dither: 0.28,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ファミコン") {
                    *params = RetroPaletteParams {
                        mode: RetroPaletteMode::Famicom,
                        dither: 0.30,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "MSX2+") {
                    *params = RetroPaletteParams {
                        mode: RetroPaletteMode::Msx2Plus,
                        dither: 0.16,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "PC-98") {
                    *params = RetroPaletteParams {
                        mode: RetroPaletteMode::Pc98,
                        dither: 0.18,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ゲームギア") {
                    *params = RetroPaletteParams {
                        mode: RetroPaletteMode::GameGear,
                        dither: 0.14,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "メガドラ") {
                    *params = RetroPaletteParams {
                        mode: RetroPaletteMode::MegaDrive,
                        dither: 0.14,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "SFC") {
                    *params = RetroPaletteParams {
                        mode: RetroPaletteMode::Sfc,
                        dither: 0.06,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "ポスタリゼーションと違い、実機風の固定パレットや画像に合わせた適応パレットへ色を寄せます。ディザを上げると階調は滑らかになりますが、網目感が増えます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let before_mode = params.mode;
            lab_combo_box(
                ui,
                "retro_palette_mode",
                retro_palette_mode_label(params.mode),
                |ui| {
                    for mode in [
                        RetroPaletteMode::Dither1Bit,
                        RetroPaletteMode::GameBoy,
                        RetroPaletteMode::Famicom,
                        RetroPaletteMode::Msx2Plus,
                        RetroPaletteMode::Pc98,
                        RetroPaletteMode::GameGear,
                        RetroPaletteMode::MegaDrive,
                        RetroPaletteMode::Sfc,
                    ] {
                        ui.selectable_value(&mut params.mode, mode, retro_palette_mode_label(mode));
                    }
                },
            );
            if params.mode != before_mode {
                if params.strength <= f32::EPSILON {
                    params.strength = 1.0;
                }
                changed = true;
            }
            let dither = ui.add(egui::Slider::new(&mut params.dither, 0.0..=1.0).text("ディザ"));
            changed |= dither.changed();
            dither.lab_hover_tip(
                "Bayer ディザの強さです。0で硬い減色、上げると階調が網点で補われます。",
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からレトロ減色後の色へどれだけ近づけるかです。");
        }
        LocalEffect::CrtDisplay(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "控えめ") {
                    *params = CrtDisplayParams::preset(CrtDisplayMode::Simple);
                    changed = true;
                }
                if preset_button(ui, "フル") {
                    *params = CrtDisplayParams::preset(CrtDisplayMode::Full);
                    changed = true;
                }
                if preset_button(ui, "アーケード") {
                    *params = CrtDisplayParams::preset(CrtDisplayMode::Arcade);
                    changed = true;
                }
                if preset_button(ui, "黒線強め") {
                    *params = CrtDisplayParams {
                        scanline_spacing_px: 3.0,
                        scanline_depth: 0.72,
                        mask_strength: 0.20,
                        curvature: 0.03,
                        bloom: 0.12,
                        horizontal_blur: 0.28,
                        brightness: 1.55,
                        strength: 0.92,
                        ..CrtDisplayParams::preset(CrtDisplayMode::Arcade)
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "スキャンライン、RGBマスク、ビームにじみ、明部グローを同じ画像サイズのまま重ねます。レトロ減色と組み合わせると実機表示風に寄せられます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let before_mode = params.mode;
            lab_combo_box(
                ui,
                "crt_display_mode",
                crt_display_mode_label(params.mode),
                |ui| {
                    for mode in [
                        CrtDisplayMode::Simple,
                        CrtDisplayMode::Full,
                        CrtDisplayMode::Arcade,
                    ] {
                        ui.selectable_value(&mut params.mode, mode, crt_display_mode_label(mode));
                    }
                },
            );
            if params.mode != before_mode {
                let strength = params.strength;
                *params = CrtDisplayParams {
                    strength: strength.max(0.8),
                    ..CrtDisplayParams::preset(params.mode)
                };
                changed = true;
            }
            let mut activates_effect = false;
            let spacing = ui.add(
                egui::Slider::new(&mut params.scanline_spacing_px, 2.0..=24.0)
                    .text("走査線間隔")
                    .suffix("px"),
            );
            changed |= spacing.changed();
            activates_effect |= spacing.changed();
            spacing.lab_hover_tip(
                "暗い走査線の周期です。小さいほど細かく、値を大きくすると粗く見えます。",
            );
            let scanline =
                ui.add(egui::Slider::new(&mut params.scanline_depth, 0.0..=1.0).text("走査線"));
            changed |= scanline.changed();
            activates_effect |= scanline.changed();
            scanline.lab_hover_tip("走査線で暗く落とす強さです。");
            let mask =
                ui.add(egui::Slider::new(&mut params.mask_strength, 0.0..=1.0).text("RGBマスク"));
            changed |= mask.changed();
            activates_effect |= mask.changed();
            mask.lab_hover_tip("赤・緑・青のアパーチャマスクを重ねる強さです。");
            let blur = ui.add(
                egui::Slider::new(&mut params.horizontal_blur, 0.0..=1.0).text("ビームにじみ"),
            );
            changed |= blur.changed();
            activates_effect |= blur.changed();
            blur.lab_hover_tip("水平方向へ少しにじませ、ブラウン管のビーム感を足します。");
            let bloom = ui.add(egui::Slider::new(&mut params.bloom, 0.0..=1.0).text("グロー"));
            changed |= bloom.changed();
            activates_effect |= bloom.changed();
            bloom.lab_hover_tip("明るい部分を拾って、蛍光体のにじみのように足します。");
            let curvature =
                ui.add(egui::Slider::new(&mut params.curvature, 0.0..=0.25).text("曲面歪み"));
            changed |= curvature.changed();
            activates_effect |= curvature.changed();
            curvature
                .lab_hover_tip("画面をわずかに樽型へ曲げます。大きい値では四隅が黒くなります。");
            let brightness =
                ui.add(egui::Slider::new(&mut params.brightness, 0.25..=2.5).text("明るさ補正"));
            changed |= brightness.changed();
            activates_effect |= brightness.changed();
            brightness.lab_hover_tip("走査線やマスクで落ちた明るさを補います。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からCRT表示風の結果へどれだけ近づけるかです。");
            if activates_effect && params.strength <= f32::EPSILON {
                params.strength = 0.8;
            }
        }
        LocalEffect::Threshold(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "標準") {
                    *params = ThresholdParams {
                        threshold: 0.50,
                        invert: false,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "明るめ") {
                    *params = ThresholdParams {
                        threshold: 0.40,
                        invert: false,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "暗め") {
                    *params = ThresholdParams {
                        threshold: 0.62,
                        invert: false,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "反転") {
                    *params = ThresholdParams {
                        threshold: 0.50,
                        invert: true,
                        strength: 1.0,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "輝度がしきい値以上なら白、それ未満なら黒にします。線画確認やモノクロ風の加工に使えます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.0..=1.0).text("しきい値"));
            changed |= threshold.changed();
            threshold.lab_hover_tip(
                "白にする明るさの境目です。値を大きくすると、より明るい部分だけが白になります。",
            );
            let invert = ui.checkbox(&mut params.invert, "反転");
            changed |= invert.changed();
            invert.lab_hover_tip("黒と白を入れ替えます。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から黒白化した結果へどれだけ近づけるかです。");
        }
        LocalEffect::Invert(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "ネガ") {
                    *params = InvertParams { strength: 1.0 };
                    changed = true;
                }
                if preset_button(ui, "薄め") {
                    *params = InvertParams { strength: 0.35 };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "RGBの明暗を反転します。強度を下げると元画像とネガを混ぜた特殊な色味になります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から反転後の色へどれだけ近づけるかです。");
        }
        LocalEffect::Duotone(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "セピア") {
                    *params = DuotoneParams {
                        preset: DuotonePreset::SepiaInk,
                        strength: 0.8,
                        contrast: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "青写真") {
                    *params = DuotoneParams {
                        preset: DuotonePreset::Cyanotype,
                        strength: 0.85,
                        contrast: 8.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "黒赤") {
                    *params = DuotoneParams {
                        preset: DuotonePreset::BlackRed,
                        strength: 0.9,
                        contrast: 12.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "紫金") {
                    *params = DuotoneParams {
                        preset: DuotonePreset::PurpleGold,
                        strength: 0.85,
                        contrast: 6.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "夕暮れ3色") {
                    *params = DuotoneParams {
                        preset: DuotonePreset::SunsetTritone,
                        strength: 0.85,
                        contrast: 5.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "コミック3色") {
                    *params = DuotoneParams {
                        preset: DuotonePreset::ComicTritone,
                        strength: 0.8,
                        contrast: 18.0,
                    };
                    changed = true;
                }
            });
            let before = params.preset;
            lab_combo_box(
                ui,
                "duotone_preset",
                duotone_preset_label(params.preset),
                |ui| {
                    for preset in [
                        DuotonePreset::None,
                        DuotonePreset::SepiaInk,
                        DuotonePreset::Cyanotype,
                        DuotonePreset::BlackRed,
                        DuotonePreset::PurpleGold,
                        DuotonePreset::TealCream,
                        DuotonePreset::SunsetTritone,
                        DuotonePreset::ComicTritone,
                        DuotonePreset::NoirTritone,
                    ] {
                        ui.selectable_value(
                            &mut params.preset,
                            preset,
                            duotone_preset_label(preset),
                        );
                    }
                },
            );
            if params.preset != before {
                if params.preset != DuotonePreset::None && params.strength <= f32::EPSILON {
                    params.strength = 1.0;
                }
                changed = true;
            }
            ui.label(
                egui::RichText::new(
                    "明るさを元に2色または3色のインク風カラーへ置き換えます。グラデーションマップより印刷・ポスター調に寄せた効果です。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像の色からダブルトーンの色へどれだけ近づけるかです。");
            let contrast = ui.add(
                egui::Slider::new(&mut params.contrast, -100.0..=100.0).text("明暗コントラスト"),
            );
            changed |= contrast.changed();
            contrast.lab_hover_tip("色を割り当てる前に明暗差を締めたり広げたりします。");
        }
        LocalEffect::Equalize(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱") {
                    *params = EqualizeParams {
                        strength: 0.35,
                        preserve_color: true,
                    };
                    changed = true;
                }
                if preset_button(ui, "中") {
                    *params = EqualizeParams {
                        strength: 0.65,
                        preserve_color: true,
                    };
                    changed = true;
                }
                if preset_button(ui, "強") {
                    *params = EqualizeParams {
                        strength: 1.0,
                        preserve_color: true,
                    };
                    changed = true;
                }
                if preset_button(ui, "白黒") {
                    *params = EqualizeParams {
                        strength: 1.0,
                        preserve_color: false,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "画像全体の明暗分布を広げます。色を保つと元の色味をなるべく残し、白黒にすると輝度だけで階調を整えます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength
                .lab_hover_tip("元画像からヒストグラム平坦化した結果へどれだけ近づけるかです。");
            let preserve = ui.checkbox(&mut params.preserve_color, "色を保つ");
            changed |= preserve.changed();
            preserve.lab_hover_tip("ONにすると、明るさだけを広げて元の色相をなるべく維持します。OFFにすると白黒の平坦化になります。");
        }
        LocalEffect::GradientMap(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "夕焼け") {
                    *params = GradientMapParams {
                        preset: GradientMapPreset::Sunset,
                        strength: 0.65,
                        contrast: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "薄暮") {
                    *params = GradientMapParams {
                        preset: GradientMapPreset::Twilight,
                        strength: 0.65,
                        contrast: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "ティール") {
                    *params = GradientMapParams {
                        preset: GradientMapPreset::TealOrange,
                        strength: 0.65,
                        contrast: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "桜色") {
                    *params = GradientMapParams {
                        preset: GradientMapPreset::Cherry,
                        strength: 0.55,
                        contrast: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "炎") {
                    *params = GradientMapParams {
                        preset: GradientMapPreset::Fire,
                        strength: 0.70,
                        contrast: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "氷") {
                    *params = GradientMapParams {
                        preset: GradientMapPreset::Ice,
                        strength: 0.65,
                        contrast: 0.0,
                    };
                    changed = true;
                }
            });
            let before = params.preset;
            lab_combo_box(
                ui,
                "gradient_map_preset",
                gradient_map_preset_label(params.preset),
                |ui| {
                    for preset in [
                        GradientMapPreset::None,
                        GradientMapPreset::Mono,
                        GradientMapPreset::Sepia,
                        GradientMapPreset::Sunset,
                        GradientMapPreset::Twilight,
                        GradientMapPreset::TealOrange,
                        GradientMapPreset::Cherry,
                        GradientMapPreset::Forest,
                        GradientMapPreset::Fire,
                        GradientMapPreset::Ice,
                    ] {
                        ui.selectable_value(
                            &mut params.preset,
                            preset,
                            gradient_map_preset_label(preset),
                        );
                    }
                },
            );
            if params.preset != before {
                if params.preset != GradientMapPreset::None && params.strength <= f32::EPSILON {
                    params.strength = 1.0;
                }
                changed = true;
            }
            ui.label(
                egui::RichText::new(
                    "輝度をグラデーション色に置き換えます。マスクや強度を弱めると色味だけを乗せる用途にも使えます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let strength_response =
                ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength_response.changed();
            strength_response.lab_hover_tip("元の色からグラデーション色へ置き換える強さです。");
            let contrast_response = ui.add(
                egui::Slider::new(&mut params.contrast, -100.0..=100.0).text("明暗コントラスト"),
            );
            changed |= contrast_response.changed();
            contrast_response
                .lab_hover_tip("色を割り当てる前に、明るさの差を締めたり広げたりします。");
        }
        LocalEffect::ColorFill(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "白背景") {
                    *params = ColorFillParams {
                        shape: ColorOverlayShape::Solid,
                        start_rgb: [255, 255, 255],
                        opacity: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "黒背景") {
                    *params = ColorFillParams {
                        shape: ColorOverlayShape::Solid,
                        start_rgb: [18, 18, 20],
                        opacity: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "淡色") {
                    *params = ColorFillParams {
                        shape: ColorOverlayShape::Solid,
                        start_rgb: [246, 238, 224],
                        opacity: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "青グラデ") {
                    *params = ColorFillParams {
                        shape: ColorOverlayShape::Linear,
                        start_rgb: [232, 242, 255],
                        end_rgb: [128, 170, 245],
                        angle_degrees: -18.0,
                        softness: 0.45,
                        opacity: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "3色背景") {
                    *params = ColorFillParams {
                        shape: ColorOverlayShape::Radial,
                        start_rgb: [255, 247, 230],
                        middle_rgb: [255, 206, 222],
                        end_rgb: [170, 195, 255],
                        middle_enabled: true,
                        midpoint: 0.48,
                        center: [0.46, 0.34],
                        radius: 0.92,
                        softness: 0.70,
                        opacity: 1.0,
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            let before_shape = params.shape;
            lab_combo_box(
                ui,
                "color_fill_shape",
                color_overlay_shape_label(params.shape),
                |ui| {
                    for shape in [
                        ColorOverlayShape::Unselected,
                        ColorOverlayShape::Solid,
                        ColorOverlayShape::Linear,
                        ColorOverlayShape::Radial,
                    ] {
                        ui.selectable_value(
                            &mut params.shape,
                            shape,
                            color_overlay_shape_label(shape),
                        );
                    }
                },
            );
            if params.shape != before_shape {
                if params.shape != ColorOverlayShape::Unselected && params.opacity <= f32::EPSILON {
                    params.opacity = 1.0;
                }
                changed = true;
            }
            ui.label(
                egui::RichText::new(
                    "マスク範囲の元画像RGBを、指定した色またはグラデーション色へ置き換えます。被写体切り抜きの背景作成や確認用に向きます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            if params.shape != ColorOverlayShape::Unselected {
                let color_label = if params.shape == ColorOverlayShape::Solid {
                    "塗り色"
                } else {
                    "開始色"
                };
                merge_rgb_color_response(
                    draw_rgb_color_control(
                        ui,
                        color_label,
                        &mut params.start_rgb,
                        RgbPickTarget::ColorFillStart,
                        rgb_pick_active,
                    ),
                    &mut changed,
                    &mut start_rgb_pick,
                    &mut cancel_rgb_pick,
                );
                if params.shape != ColorOverlayShape::Solid {
                    let middle = ui.checkbox(&mut params.middle_enabled, "中間色を使う");
                    changed |= middle.changed();
                    middle.lab_hover_tip(
                        "ONにすると、開始色・中間色・終了色の3色グラデーションになります。",
                    );
                    if params.middle_enabled {
                        merge_rgb_color_response(
                            draw_rgb_color_control(
                                ui,
                                "中間色",
                                &mut params.middle_rgb,
                                RgbPickTarget::ColorFillMiddle,
                                rgb_pick_active,
                            ),
                            &mut changed,
                            &mut start_rgb_pick,
                            &mut cancel_rgb_pick,
                        );
                        let midpoint = ui.add(
                            egui::Slider::new(&mut params.midpoint, 0.01..=0.99).text("中間位置"),
                        );
                        changed |= midpoint.changed();
                        midpoint.lab_hover_tip("グラデーション内で中間色が出る位置です。");
                    }
                    merge_rgb_color_response(
                        draw_rgb_color_control(
                            ui,
                            "終了色",
                            &mut params.end_rgb,
                            RgbPickTarget::ColorFillEnd,
                            rgb_pick_active,
                        ),
                        &mut changed,
                        &mut start_rgb_pick,
                        &mut cancel_rgb_pick,
                    );
                }
                let opacity =
                    ui.add(egui::Slider::new(&mut params.opacity, 0.0..=1.0).text("不透明度"));
                changed |= opacity.changed();
                opacity.lab_hover_tip("元画像から塗りつぶし色へどれだけ置き換えるかです。");
                if params.shape == ColorOverlayShape::Linear {
                    let angle = ui.add(
                        egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                            .text("角度")
                            .suffix("°"),
                    );
                    if angle.changed() {
                        params.linear_points_enabled = false;
                        changed = true;
                    }
                    angle.lab_hover_tip(
                        "線形グラデーションの方向です。0°で左から右へ色が変わります。画像上をドラッグすると開始点と終了点も設定できます。",
                    );
                }
                if params.shape == ColorOverlayShape::Radial {
                    let center_x =
                        ui.add(egui::Slider::new(&mut params.center[0], 0.0..=1.0).text("中心X"));
                    changed |= center_x.changed();
                    center_x.lab_hover_tip("円形グラデーション中心の横位置です。");
                    let center_y =
                        ui.add(egui::Slider::new(&mut params.center[1], 0.0..=1.0).text("中心Y"));
                    changed |= center_y.changed();
                    center_y.lab_hover_tip("円形グラデーション中心の縦位置です。");
                    let radius =
                        ui.add(egui::Slider::new(&mut params.radius, 0.02..=2.0).text("半径"));
                    changed |= radius.changed();
                    radius.lab_hover_tip(
                        "中心色から終了色へ変わる範囲です。画像上をドラッグすると中心と半径を設定できます。",
                    );
                }
                if params.shape != ColorOverlayShape::Solid {
                    let softness = ui
                        .add(egui::Slider::new(&mut params.softness, 0.0..=1.0).text("なめらかさ"));
                    changed |= softness.changed();
                    softness.lab_hover_tip(
                        "グラデーションの変化を直線的にするか、なだらかにするかです。",
                    );
                }
            }
        }
        LocalEffect::Frame(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "黒枠") {
                    *params = FrameParams {
                        mode: FrameMode::Border,
                        color_rgb: [0, 0, 0],
                        opacity: 1.0,
                        width_px: 36.0,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "白枠") {
                    *params = FrameParams {
                        mode: FrameMode::Border,
                        color_rgb: [255, 255, 255],
                        line_rgb: [210, 210, 210],
                        opacity: 1.0,
                        width_px: 48.0,
                        line_width_px: 1.0,
                        line_opacity: 0.8,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "映画黒帯") {
                    *params = FrameParams {
                        mode: FrameMode::Letterbox,
                        color_rgb: [0, 0, 0],
                        opacity: 1.0,
                        aspect_ratio: 2.35,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "角丸白") {
                    *params = FrameParams {
                        mode: FrameMode::RoundedMatte,
                        color_rgb: [255, 255, 255],
                        opacity: 1.0,
                        width_px: 0.0,
                        corner_radius_px: 36.0,
                        softness_px: 2.0,
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            let before_mode = params.mode;
            lab_combo_box(ui, "frame_mode", frame_mode_label(params.mode), |ui| {
                for mode in [
                    FrameMode::Border,
                    FrameMode::Letterbox,
                    FrameMode::RoundedMatte,
                ] {
                    ui.selectable_value(&mut params.mode, mode, frame_mode_label(mode));
                }
            });
            if params.mode != before_mode {
                match params.mode {
                    FrameMode::Border if params.width_px <= f32::EPSILON => {
                        params.width_px = 36.0;
                    }
                    FrameMode::Letterbox => {
                        params.aspect_ratio = params.aspect_ratio.max(2.35);
                    }
                    FrameMode::RoundedMatte if params.corner_radius_px <= f32::EPSILON => {
                        params.corner_radius_px = 36.0;
                    }
                    _ => {}
                }
                changed = true;
            }
            ui.label(
                egui::RichText::new(
                    "画像サイズは変えず、画像の内側に枠や黒帯を描きます。外側キャンバスを広げる余白追加とは別の仕上げ効果です。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "フレーム色",
                    &mut params.color_rgb,
                    RgbPickTarget::FrameColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let opacity =
                ui.add(egui::Slider::new(&mut params.opacity, 0.0..=1.0).text("不透明度"));
            changed |= opacity.changed();
            opacity.lab_hover_tip("元画像からフレーム色へ置き換える強さです。");
            match params.mode {
                FrameMode::Border => {
                    let individual =
                        ui.checkbox(&mut params.use_individual_widths, "辺ごとに幅を指定");
                    changed |= individual.changed();
                    individual.lab_hover_tip("ONにすると上下左右のフレーム幅を個別に指定します。");
                    if params.use_individual_widths {
                        let top =
                            ui.add(egui::Slider::new(&mut params.top_px, 0.0..=300.0).text("上"));
                        let right =
                            ui.add(egui::Slider::new(&mut params.right_px, 0.0..=300.0).text("右"));
                        let bottom = ui
                            .add(egui::Slider::new(&mut params.bottom_px, 0.0..=300.0).text("下"));
                        let left =
                            ui.add(egui::Slider::new(&mut params.left_px, 0.0..=300.0).text("左"));
                        changed |=
                            top.changed() || right.changed() || bottom.changed() || left.changed();
                    } else {
                        let width =
                            ui.add(egui::Slider::new(&mut params.width_px, 0.0..=300.0).text("幅"));
                        changed |= width.changed();
                        width.lab_hover_tip("四辺に描くフレームの内側幅です。");
                    }
                    let softness = ui.add(
                        egui::Slider::new(&mut params.softness_px, 0.0..=80.0).text("ぼかし境界"),
                    );
                    changed |= softness.changed();
                    softness.lab_hover_tip("フレーム内側の境界をどれだけ柔らかくするかです。");
                }
                FrameMode::Letterbox => {
                    ui.horizontal_wrapped(|ui| {
                        if preset_button(ui, "1.85") {
                            params.aspect_ratio = 1.85;
                            changed = true;
                        }
                        if preset_button(ui, "2.35") {
                            params.aspect_ratio = 2.35;
                            changed = true;
                        }
                        if preset_button(ui, "2.39") {
                            params.aspect_ratio = 2.39;
                            changed = true;
                        }
                    });
                    let aspect = ui.add(
                        egui::Slider::new(&mut params.aspect_ratio, 1.0..=3.0).text("アスペクト"),
                    );
                    changed |= aspect.changed();
                    aspect.lab_hover_tip(
                        "残したい中央領域の横:縦比です。現在の画像より横長なら上下、縦長なら左右に帯を描きます。",
                    );
                    let softness = ui.add(
                        egui::Slider::new(&mut params.softness_px, 0.0..=80.0).text("ぼかし境界"),
                    );
                    changed |= softness.changed();
                    softness.lab_hover_tip("黒帯の境界をどれだけ柔らかくするかです。");
                }
                FrameMode::RoundedMatte => {
                    let inset = ui
                        .add(egui::Slider::new(&mut params.width_px, 0.0..=200.0).text("内側余白"));
                    changed |= inset.changed();
                    inset.lab_hover_tip("角丸で残す中央領域を内側へ縮める幅です。");
                    let radius = ui.add(
                        egui::Slider::new(&mut params.corner_radius_px, 0.0..=300.0).text("角丸"),
                    );
                    changed |= radius.changed();
                    radius.lab_hover_tip("中央領域の角丸半径です。");
                    let softness = ui.add(
                        egui::Slider::new(&mut params.softness_px, 0.0..=80.0).text("ぼかし境界"),
                    );
                    changed |= softness.changed();
                    softness.lab_hover_tip("角丸マット境界をどれだけ柔らかくするかです。");
                }
            }
            let before_line_width = params.line_width_px;
            let line_width =
                ui.add(egui::Slider::new(&mut params.line_width_px, 0.0..=32.0).text("内側ライン"));
            if line_width.changed() {
                if before_line_width <= f32::EPSILON
                    && params.line_width_px > f32::EPSILON
                    && params.line_opacity <= f32::EPSILON
                {
                    params.line_opacity = 1.0;
                }
                changed = true;
            }
            line_width
                .lab_hover_tip("フレームや黒帯の内側境界に細いラインを重ねます。0で無効です。");
            if params.line_width_px > f32::EPSILON {
                merge_rgb_color_response(
                    draw_rgb_color_control(
                        ui,
                        "ライン色",
                        &mut params.line_rgb,
                        RgbPickTarget::FrameLineColor,
                        rgb_pick_active,
                    ),
                    &mut changed,
                    &mut start_rgb_pick,
                    &mut cancel_rgb_pick,
                );
                let line_opacity = ui.add(
                    egui::Slider::new(&mut params.line_opacity, 0.0..=1.0).text("ライン不透明度"),
                );
                changed |= line_opacity.changed();
                line_opacity.lab_hover_tip("内側ラインの濃さです。");
            }
        }
        LocalEffect::OutlineStroke(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "黒フチ") {
                    *params = OutlineStrokeParams {
                        placement: OutlineStrokePlacement::Outside,
                        width_px: 4.0,
                        softness_px: 1.0,
                        opacity: 1.0,
                        color_rgb: [0, 0, 0],
                    };
                    changed = true;
                }
                if preset_button(ui, "白ステッカー") {
                    *params = OutlineStrokeParams {
                        placement: OutlineStrokePlacement::Outside,
                        width_px: 8.0,
                        softness_px: 2.0,
                        opacity: 0.95,
                        color_rgb: [255, 255, 255],
                    };
                    changed = true;
                }
                if preset_button(ui, "内側色線") {
                    *params = OutlineStrokeParams {
                        placement: OutlineStrokePlacement::Inside,
                        width_px: 3.0,
                        softness_px: 1.0,
                        opacity: 0.85,
                        color_rgb: [80, 170, 255],
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "マスク境界をもとに色枠を描きます。初期状態では前ON/後OFFなので、外側の縁取りがマスクの外へ出ます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let before_placement = params.placement;
            lab_combo_box(
                ui,
                "outline_stroke_placement",
                outline_stroke_placement_label(params.placement),
                |ui| {
                    for placement in [
                        OutlineStrokePlacement::Outside,
                        OutlineStrokePlacement::Inside,
                        OutlineStrokePlacement::Center,
                    ] {
                        ui.selectable_value(
                            &mut params.placement,
                            placement,
                            outline_stroke_placement_label(placement),
                        );
                    }
                },
            );
            changed |= params.placement != before_placement;
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "線色",
                    &mut params.color_rgb,
                    RgbPickTarget::OutlineStrokeColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let width = ui.add(
                egui::Slider::new(&mut params.width_px, 0.0..=64.0)
                    .text("幅")
                    .suffix("px"),
            );
            changed |= width.changed();
            width.lab_hover_tip("マスク境界から作る線の太さです。0pxでは無効です。");
            let softness = ui.add(
                egui::Slider::new(&mut params.softness_px, 0.0..=16.0)
                    .text("ぼかし")
                    .suffix("px"),
            );
            changed |= softness.changed();
            softness.lab_hover_tip(
                "線の縁を柔らかくします。ステッカー風は低め、発光前の下地は高めが向きます。",
            );
            let opacity =
                ui.add(egui::Slider::new(&mut params.opacity, 0.0..=1.0).text("不透明度"));
            changed |= opacity.changed();
            opacity.lab_hover_tip("縁取り色を元画像へ重ねる強さです。");
        }
        LocalEffect::RimLight(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "右リム") {
                    *params = RimLightParams {
                        light_angle_degrees: 0.0,
                        width_px: 8.0,
                        falloff: 0.42,
                        strength: 0.85,
                        color_rgb: [210, 238, 255],
                        wrap: 0.12,
                    };
                    changed = true;
                }
                if preset_button(ui, "右上光") {
                    *params = RimLightParams {
                        light_angle_degrees: -35.0,
                        width_px: 10.0,
                        falloff: 0.48,
                        strength: 0.90,
                        color_rgb: [255, 244, 210],
                        wrap: 0.18,
                    };
                    changed = true;
                }
                if preset_button(ui, "寒色輪郭") {
                    *params = RimLightParams {
                        light_angle_degrees: 160.0,
                        width_px: 12.0,
                        falloff: 0.55,
                        strength: 0.78,
                        color_rgb: [138, 205, 255],
                        wrap: 0.24,
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかめ") {
                    *params = RimLightParams {
                        light_angle_degrees: -120.0,
                        width_px: 20.0,
                        falloff: 0.78,
                        strength: 0.58,
                        color_rgb: [255, 238, 218],
                        wrap: 0.38,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "マスク境界の光方向に向いた側だけを照らします。初期状態では前ON/後OFFなので、輪郭光がマスク外へ広がります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "光色",
                    &mut params.color_rgb,
                    RgbPickTarget::RimLightColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let angle = ui.add(
                egui::Slider::new(&mut params.light_angle_degrees, -180.0..=180.0)
                    .text("光方向")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip("0°で右側、90°で下側、-90°で上側の縁が光ります。");
            let width = ui.add(
                egui::Slider::new(&mut params.width_px, 0.0..=64.0)
                    .text("幅")
                    .suffix("px"),
            );
            changed |= width.changed();
            width.lab_hover_tip("境界から作るリムライトの太さです。0pxでは無効です。");
            let falloff = ui.add(egui::Slider::new(&mut params.falloff, 0.0..=1.0).text("減衰"));
            changed |= falloff.changed();
            falloff.lab_hover_tip("リムライトの縁をどれだけなめらかに消すかです。");
            let wrap = ui.add(egui::Slider::new(&mut params.wrap, 0.0..=1.0).text("回り込み"));
            changed |= wrap.changed();
            wrap.lab_hover_tip("光方向から外れた境界にも、どれだけ光を回り込ませるかです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=2.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("輪郭光を元画像へ重ねる強さです。");
        }
        LocalEffect::ContactShadow(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "内側AO") {
                    *params = ContactShadowParams {
                        radius_px: 8.0,
                        softness_px: 4.0,
                        strength: 0.32,
                        color_rgb: [18, 16, 20],
                        direction_degrees: 90.0,
                        directionality: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "下側影") {
                    *params = ContactShadowParams {
                        radius_px: 12.0,
                        softness_px: 6.0,
                        strength: 0.45,
                        color_rgb: [30, 25, 44],
                        direction_degrees: 90.0,
                        directionality: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "薄い締め") {
                    *params = ContactShadowParams {
                        radius_px: 5.0,
                        softness_px: 3.0,
                        strength: 0.18,
                        color_rgb: [22, 20, 24],
                        direction_degrees: 90.0,
                        directionality: 0.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "硬い陰") {
                    *params = ContactShadowParams {
                        radius_px: 4.0,
                        softness_px: 1.0,
                        strength: 0.55,
                        color_rgb: [12, 10, 14],
                        direction_degrees: 90.0,
                        directionality: 0.70,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "マスク境界の内側だけを暗くします。初期状態では前ON/後ONなので、簡易AOがマスク内に収まります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "影色",
                    &mut params.color_rgb,
                    RgbPickTarget::ContactShadowColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 0.0..=64.0)
                    .text("幅")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius.lab_hover_tip("境界の内側へ入る影の幅です。0pxでは無効です。");
            let softness = ui.add(
                egui::Slider::new(&mut params.softness_px, 0.0..=32.0)
                    .text("ぼかし")
                    .suffix("px"),
            );
            changed |= softness.changed();
            softness.lab_hover_tip("影の縁をなめらかにします。硬いセル影では低めにします。");
            let angle = ui.add(
                egui::Slider::new(&mut params.direction_degrees, -180.0..=180.0)
                    .text("影方向")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip("0°で右側、90°で下側、-90°で上側の境界に影を寄せます。");
            let directionality =
                ui.add(egui::Slider::new(&mut params.directionality, 0.0..=1.0).text("方向性"));
            changed |= directionality.changed();
            directionality.lab_hover_tip("0で全周AO、1で指定方向の境界だけを強く暗くします。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("影色へ寄せる強さです。");
        }
        LocalEffect::ColorOverlay(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "夕焼け") {
                    *params = ColorOverlayParams {
                        shape: ColorOverlayShape::Linear,
                        blend_mode: ColorOverlayBlendMode::SoftLight,
                        start_rgb: [255, 132, 48],
                        end_rgb: [78, 124, 255],
                        angle_degrees: -24.0,
                        softness: 0.65,
                        opacity: 0.58,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "暖色塗り") {
                    *params = ColorOverlayParams {
                        shape: ColorOverlayShape::Solid,
                        blend_mode: ColorOverlayBlendMode::SoftLight,
                        start_rgb: [255, 170, 92],
                        opacity: 0.36,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "影色乗算") {
                    *params = ColorOverlayParams {
                        shape: ColorOverlayShape::Linear,
                        blend_mode: ColorOverlayBlendMode::Multiply,
                        start_rgb: [76, 84, 148],
                        end_rgb: [255, 180, 98],
                        angle_degrees: 34.0,
                        softness: 0.45,
                        opacity: 0.32,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "光の中心") {
                    *params = ColorOverlayParams {
                        shape: ColorOverlayShape::Radial,
                        blend_mode: ColorOverlayBlendMode::Screen,
                        start_rgb: [255, 236, 186],
                        end_rgb: [255, 114, 44],
                        center: [0.50, 0.36],
                        radius: 0.72,
                        softness: 0.82,
                        opacity: 0.44,
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            let before_shape = params.shape;
            lab_combo_box(
                ui,
                "color_overlay_shape",
                color_overlay_shape_label(params.shape),
                |ui| {
                    for shape in [
                        ColorOverlayShape::Solid,
                        ColorOverlayShape::Linear,
                        ColorOverlayShape::Radial,
                    ] {
                        ui.selectable_value(
                            &mut params.shape,
                            shape,
                            color_overlay_shape_label(shape),
                        );
                    }
                },
            );
            changed |= params.shape != before_shape;
            let before_blend = params.blend_mode;
            lab_combo_box(
                ui,
                "color_overlay_blend_mode",
                color_overlay_blend_mode_label(params.blend_mode),
                |ui| {
                    for mode in [
                        ColorOverlayBlendMode::Normal,
                        ColorOverlayBlendMode::Multiply,
                        ColorOverlayBlendMode::Screen,
                        ColorOverlayBlendMode::Overlay,
                        ColorOverlayBlendMode::SoftLight,
                        ColorOverlayBlendMode::Color,
                    ] {
                        ui.selectable_value(
                            &mut params.blend_mode,
                            mode,
                            color_overlay_blend_mode_label(mode),
                        );
                    }
                },
            );
            changed |= params.blend_mode != before_blend;
            ui.label(
                egui::RichText::new(
                    "画像の明るさではなく画面上の位置を基準に、単色またはグラデーションの色面を合成します。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let color_label = if params.shape == ColorOverlayShape::Solid {
                "塗り色"
            } else {
                "開始色"
            };
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    color_label,
                    &mut params.start_rgb,
                    RgbPickTarget::ColorOverlayStart,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            if params.shape != ColorOverlayShape::Solid {
                merge_rgb_color_response(
                    draw_rgb_color_control(
                        ui,
                        "終了色",
                        &mut params.end_rgb,
                        RgbPickTarget::ColorOverlayEnd,
                        rgb_pick_active,
                    ),
                    &mut changed,
                    &mut start_rgb_pick,
                    &mut cancel_rgb_pick,
                );
            }
            let opacity =
                ui.add(egui::Slider::new(&mut params.opacity, 0.0..=1.0).text("不透明度"));
            changed |= opacity.changed();
            opacity.lab_hover_tip("色面を合成した結果へどれだけ近づけるかです。");
            if params.shape == ColorOverlayShape::Linear {
                let angle = ui.add(
                    egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                        .text("角度")
                        .suffix("°"),
                );
                if angle.changed() {
                    params.linear_points_enabled = false;
                    changed = true;
                }
                angle.lab_hover_tip(
                    "線形グラデーションの方向です。0°で左から右へ色が変わります。画像上をドラッグすると開始点と終了点も設定できます。",
                );
            }
            if params.shape == ColorOverlayShape::Radial {
                let center_x =
                    ui.add(egui::Slider::new(&mut params.center[0], 0.0..=1.0).text("中心X"));
                changed |= center_x.changed();
                center_x.lab_hover_tip("円形グラデーション中心の横位置です。");
                let center_y =
                    ui.add(egui::Slider::new(&mut params.center[1], 0.0..=1.0).text("中心Y"));
                changed |= center_y.changed();
                center_y.lab_hover_tip("円形グラデーション中心の縦位置です。");
                let radius = ui.add(egui::Slider::new(&mut params.radius, 0.02..=2.0).text("半径"));
                changed |= radius.changed();
                radius.lab_hover_tip(
                    "中心色から終了色へ変わる範囲です。画像上をドラッグすると中心と半径を設定できます。",
                );
            }
            if params.shape != ColorOverlayShape::Solid {
                let softness =
                    ui.add(egui::Slider::new(&mut params.softness, 0.0..=1.0).text("なめらかさ"));
                changed |= softness.changed();
                softness
                    .lab_hover_tip("グラデーションの変化を直線的にするか、なだらかにするかです。");
            }
        }
        LocalEffect::NeonGlow(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "シアン管") {
                    *params = NeonGlowParams {
                        threshold: 0.72,
                        by_saturation: true,
                        inner_radius_px: 5.0,
                        outer_radius_px: 34.0,
                        strength: 0.95,
                        inner_amount: 0.95,
                        outer_amount: 0.85,
                        glow_saturation: 0.85,
                        tint_rgb: [0, 220, 255],
                        tint_strength: 0.28,
                        screen_blend: true,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "マゼンタ") {
                    *params = NeonGlowParams {
                        threshold: 0.68,
                        by_saturation: true,
                        inner_radius_px: 4.0,
                        outer_radius_px: 26.0,
                        strength: 0.90,
                        inner_amount: 1.0,
                        outer_amount: 0.72,
                        glow_saturation: 0.95,
                        tint_rgb: [255, 58, 210],
                        tint_strength: 0.38,
                        screen_blend: true,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "広いハロー") {
                    *params = NeonGlowParams {
                        threshold: 0.50,
                        by_saturation: true,
                        inner_radius_px: 7.0,
                        outer_radius_px: 64.0,
                        strength: 0.70,
                        inner_amount: 0.62,
                        outer_amount: 1.15,
                        glow_saturation: 0.55,
                        tint_strength: 0.0,
                        screen_blend: true,
                        ..Default::default()
                    };
                    changed = true;
                }
                if preset_button(ui, "色指定") {
                    *params = NeonGlowParams {
                        threshold: 0.36,
                        by_saturation: true,
                        inner_radius_px: 5.0,
                        outer_radius_px: 24.0,
                        strength: 0.85,
                        inner_amount: 0.85,
                        outer_amount: 0.75,
                        glow_saturation: 0.45,
                        source_color_enabled: true,
                        source_rgb: [0, 220, 255],
                        source_tolerance: 0.24,
                        source_feather: 0.12,
                        tint_rgb: [0, 220, 255],
                        tint_strength: 0.18,
                        screen_blend: true,
                        ..Default::default()
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "輝度だけでなく高彩度の色も発光源として拾い、芯のにじみと広いハローを二段で重ねます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.05..=0.999).text("発光しきい値"));
            changed |= threshold.changed();
            threshold.lab_hover_tip(
                "どの明るさ/鮮やかさから発光源として拾うかです。低いほど広く光ります。",
            );
            let by_saturation = ui.checkbox(&mut params.by_saturation, "鮮やかな色も拾う");
            changed |= by_saturation.changed();
            by_saturation.lab_hover_tip(
                "ONにすると、白くないシアンやマゼンタのネオン色も発光源になります。",
            );
            let source_color = ui.checkbox(&mut params.source_color_enabled, "発光色を指定する");
            changed |= source_color.changed();
            source_color
                .lab_hover_tip("ONにすると、指定色に近い線や面だけを発光源として拾います。");
            if params.source_color_enabled {
                merge_rgb_color_response(
                    draw_rgb_color_control(
                        ui,
                        "発光源の色",
                        &mut params.source_rgb,
                        RgbPickTarget::NeonGlowSource,
                        rgb_pick_active,
                    ),
                    &mut changed,
                    &mut start_rgb_pick,
                    &mut cancel_rgb_pick,
                );
                let tolerance = ui
                    .add(egui::Slider::new(&mut params.source_tolerance, 0.0..=1.0).text("色許容"));
                changed |= tolerance.changed();
                tolerance
                    .lab_hover_tip("発光源として拾う色の近さです。低いほど指定色だけに絞ります。");
                let feather = ui.add(
                    egui::Slider::new(&mut params.source_feather, 0.001..=1.0).text("色ぼかし"),
                );
                changed |= feather.changed();
                feather.lab_hover_tip("指定色の範囲境界をどれだけなだらかにするかです。");
            }
            let inner_radius = ui.add(
                egui::Slider::new(&mut params.inner_radius_px, 0.0..=96.0)
                    .text("芯の半径")
                    .suffix("px"),
            );
            changed |= inner_radius.changed();
            inner_radius.lab_hover_tip("光源の近くに出る強いにじみの半径です。");
            let outer_radius = ui.add(
                egui::Slider::new(&mut params.outer_radius_px, 0.0..=180.0)
                    .text("ハロー半径")
                    .suffix("px"),
            );
            changed |= outer_radius.changed();
            outer_radius.lab_hover_tip("周囲へ広く漂う外側の光の半径です。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=2.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("内側グローと外側ハローを元画像へ重ねる強さです。");
            let inner_amount =
                ui.add(egui::Slider::new(&mut params.inner_amount, 0.0..=2.0).text("芯の強さ"));
            changed |= inner_amount.changed();
            inner_amount.lab_hover_tip("光源近くの強いグローの量です。");
            let outer_amount =
                ui.add(egui::Slider::new(&mut params.outer_amount, 0.0..=2.0).text("ハロー量"));
            changed |= outer_amount.changed();
            outer_amount.lab_hover_tip("外側へ広がる柔らかいハローの量です。");
            let glow_saturation =
                ui.add(egui::Slider::new(&mut params.glow_saturation, -1.0..=2.0).text("光の彩度"));
            changed |= glow_saturation.changed();
            glow_saturation.lab_hover_tip("光輪の色をどれだけ鮮やかにするかです。");
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "着色",
                    &mut params.tint_rgb,
                    RgbPickTarget::NeonGlowTint,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let tint_strength =
                ui.add(egui::Slider::new(&mut params.tint_strength, 0.0..=1.0).text("着色量"));
            changed |= tint_strength.changed();
            tint_strength.lab_hover_tip("元の発光色から、着色で指定した色へどれだけ寄せるかです。");
            let screen_blend = ui.checkbox(&mut params.screen_blend, "スクリーン合成");
            changed |= screen_blend.changed();
            screen_blend.lab_hover_tip("ONにすると、加算より白飛びを抑えながら発光感を出します。");
        }
        LocalEffect::DiffuseGlow(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "夢幻") {
                    *params = DiffuseGlowParams {
                        threshold: 0.48,
                        radius_px: 28.0,
                        strength: 0.75,
                        white_mix: 0.55,
                        grain: 0.28,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡く") {
                    *params = DiffuseGlowParams {
                        threshold: 0.62,
                        radius_px: 18.0,
                        strength: 0.42,
                        white_mix: 0.35,
                        grain: 0.12,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "粒状") {
                    *params = DiffuseGlowParams {
                        threshold: 0.42,
                        radius_px: 22.0,
                        strength: 0.85,
                        white_mix: 0.45,
                        grain: 0.75,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明るい部分を白く拡散し、粒状ノイズで光のにじみにムラを作ります。Bloom より柔らかい写真効果向けです。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.0..=0.98).text("明部しきい値"));
            changed |= threshold.changed();
            threshold
                .lab_hover_tip("光彩として拾う明るさです。低いほど広い範囲へ白い拡散が乗ります。");
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 0.0..=120.0)
                    .text("拡散半径")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius.lab_hover_tip("抽出した明部をどれだけ広くにじませるかです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=2.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像へ拡散光彩を重ねる強さです。");
            let white_mix =
                ui.add(egui::Slider::new(&mut params.white_mix, 0.0..=1.0).text("白さ"));
            changed |= white_mix.changed();
            white_mix.lab_hover_tip("光彩をどれだけ白く漂わせるかです。");
            let grain = ui.add(egui::Slider::new(&mut params.grain, 0.0..=1.0).text("粒状感"));
            changed |= grain.changed();
            grain.lab_hover_tip("光彩と明部に加える粒状ノイズの量です。");
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("粒状ノイズのパターンを変えます。");
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::Bloom(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "弱い光") {
                    *params = BloomParams {
                        threshold: 0.75,
                        radius_px: 18.0,
                        strength: 0.25,
                    };
                    changed = true;
                }
                if preset_button(ui, "瞳/光源") {
                    *params = BloomParams {
                        threshold: 0.82,
                        radius_px: 10.0,
                        strength: 0.55,
                    };
                    changed = true;
                }
                if preset_button(ui, "強いにじみ") {
                    *params = BloomParams {
                        threshold: 0.65,
                        radius_px: 32.0,
                        strength: 0.65,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.threshold, 0.0..=0.98).text("明部しきい値"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.radius_px, 0.0..=120.0).text("半径"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=2.0).text("強さ"))
                .changed();
        }
        LocalEffect::Halation(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "アニメ光") {
                    *params = HalationParams {
                        threshold: 0.58,
                        radius_px: 34.0,
                        strength: 0.65,
                        warmth: 0.65,
                        tint_rgb: [255, 232, 196],
                        edge_bias: 0.45,
                        screen_blend: true,
                    };
                    changed = true;
                }
                if preset_button(ui, "逆光にじみ") {
                    *params = HalationParams {
                        threshold: 0.48,
                        radius_px: 52.0,
                        strength: 0.85,
                        warmth: 0.75,
                        tint_rgb: [255, 220, 176],
                        edge_bias: 0.25,
                        screen_blend: true,
                    };
                    changed = true;
                }
                if preset_button(ui, "輪郭白浮き") {
                    *params = HalationParams {
                        threshold: 0.62,
                        radius_px: 22.0,
                        strength: 0.70,
                        warmth: 0.45,
                        tint_rgb: [255, 238, 210],
                        edge_bias: 0.85,
                        screen_blend: true,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡く") {
                    *params = HalationParams {
                        threshold: 0.70,
                        radius_px: 18.0,
                        strength: 0.35,
                        warmth: 0.50,
                        tint_rgb: [255, 236, 210],
                        edge_bias: 0.35,
                        screen_blend: true,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明るい部分を暖色の白へ寄せて柔らかくにじませます。エッジ寄せを上げると明暗境界の白浮きが強くなります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.05..=0.98).text("明部しきい値"));
            changed |= threshold.changed();
            threshold.lab_hover_tip(
                "ハレーションの元になる明るさです。低いほど広い範囲からにじみます。",
            );
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 0.0..=180.0)
                    .text("にじみ半径")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius.lab_hover_tip("暖色の白浮きをどれだけ広げるかです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=2.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像へハレーションを重ねる強さです。");
            let warmth = ui.add(egui::Slider::new(&mut params.warmth, 0.0..=1.0).text("暖色寄せ"));
            changed |= warmth.changed();
            warmth.lab_hover_tip("光をどれだけ指定した暖色へ寄せるかです。");
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "暖色",
                    &mut params.tint_rgb,
                    RgbPickTarget::HalationTint,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let edge_bias =
                ui.add(egui::Slider::new(&mut params.edge_bias, 0.0..=1.0).text("エッジ寄せ"));
            changed |= edge_bias.changed();
            edge_bias
                .lab_hover_tip("0で明部全体、1に近づけるほど明暗境界を優先して白浮きを作ります。");
            let screen_blend = ui.checkbox(&mut params.screen_blend, "スクリーン合成");
            changed |= screen_blend.changed();
            screen_blend.lab_hover_tip("ONにすると、加算より白飛びを抑えながら発光感を出します。");
        }
        LocalEffect::ColorDodgeGlow(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "魔法光") {
                    *params = ColorDodgeGlowParams {
                        threshold: 0.36,
                        radius_px: 34.0,
                        strength: 0.88,
                        dodge_amount: 0.72,
                        color_rgb: [82, 190, 255],
                        color_strength: 0.68,
                    };
                    changed = true;
                }
                if preset_button(ui, "暖色発光") {
                    *params = ColorDodgeGlowParams {
                        threshold: 0.42,
                        radius_px: 42.0,
                        strength: 0.75,
                        dodge_amount: 0.55,
                        color_rgb: [255, 178, 80],
                        color_strength: 0.58,
                    };
                    changed = true;
                }
                if preset_button(ui, "強い覆い焼き") {
                    *params = ColorDodgeGlowParams {
                        threshold: 0.26,
                        radius_px: 24.0,
                        strength: 1.05,
                        dodge_amount: 1.0,
                        color_rgb: [255, 255, 255],
                        color_strength: 0.08,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡い感情光") {
                    *params = ColorDodgeGlowParams {
                        threshold: 0.18,
                        radius_px: 72.0,
                        strength: 0.46,
                        dodge_amount: 0.34,
                        color_rgb: [255, 124, 212],
                        color_strength: 0.78,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明るい部分を発光源にして、スクリーンと覆い焼きを混ぜた色付きの強い光を重ねます。初期状態では前ON/後OFFなので光がマスク外へ広がります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 0.0..=0.995).text("発光しきい値"));
            changed |= threshold.changed();
            threshold.lab_hover_tip("どの明るさから発光源として拾うかです。低いほど広く光ります。");
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 0.0..=180.0)
                    .text("光の半径")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius.lab_hover_tip("色付きの光をどれだけ周囲へ広げるかです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=2.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("発光を元画像へ重ねる強さです。");
            let dodge =
                ui.add(egui::Slider::new(&mut params.dodge_amount, 0.0..=1.0).text("覆い焼き量"));
            changed |= dodge.changed();
            dodge.lab_hover_tip("0で柔らかいスクリーン寄り、1で強い覆い焼き寄りになります。");
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "光色",
                    &mut params.color_rgb,
                    RgbPickTarget::ColorDodgeGlowColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let color_strength =
                ui.add(egui::Slider::new(&mut params.color_strength, 0.0..=1.0).text("着色量"));
            changed |= color_strength.changed();
            color_strength.lab_hover_tip("元の明部色から、指定した光色へどれだけ寄せるかです。");
        }
        LocalEffect::GodRays(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "木漏れ日") {
                    *params = GodRaysParams {
                        center: [0.38, 0.06],
                        threshold: 0.74,
                        length_px: 150.0,
                        decay: 0.88,
                        strength: 0.95,
                        warm_tint: 0.28,
                    };
                    changed = true;
                }
                if preset_button(ui, "舞台光") {
                    *params = GodRaysParams {
                        center: [0.50, 0.00],
                        threshold: 0.68,
                        length_px: 220.0,
                        decay: 0.82,
                        strength: 1.25,
                        warm_tint: 0.12,
                    };
                    changed = true;
                }
                if preset_button(ui, "夕日") {
                    *params = GodRaysParams {
                        center: [0.12, 0.22],
                        threshold: 0.70,
                        length_px: 190.0,
                        decay: 0.90,
                        strength: 1.10,
                        warm_tint: 0.55,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明るい部分を拾い、光源中心から外側へ伸びる放射状の光芒を作ります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "光が差し込む中心の横位置です。",
                "光が差し込む中心の縦位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.threshold, 0.0..=0.98)
                        .text("明部しきい値")
                        .fixed_decimals(3),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.length_px, 1.0..=360.0).text("光芒長"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.decay, 0.0..=1.0).text("減衰"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=3.0).text("強さ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.warm_tint, 0.0..=1.0).text("暖色"))
                .changed();
        }
        LocalEffect::LensFlare(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "逆光") {
                    *params = LensFlareParams {
                        center: [0.78, 0.20],
                        radius_px: 120.0,
                        strength: 0.90,
                        core_strength: 1.0,
                        halo_strength: 0.85,
                        ghost_strength: 0.75,
                        streak_strength: 0.35,
                        warm_tint: 0.45,
                    };
                    changed = true;
                }
                if preset_button(ui, "シネマ") {
                    *params = LensFlareParams {
                        center: [0.12, 0.36],
                        radius_px: 150.0,
                        strength: 0.80,
                        core_strength: 0.75,
                        halo_strength: 0.45,
                        ghost_strength: 0.65,
                        streak_strength: 1.10,
                        warm_tint: 0.08,
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかい") {
                    *params = LensFlareParams {
                        center: [0.66, 0.18],
                        radius_px: 170.0,
                        strength: 0.55,
                        core_strength: 0.55,
                        halo_strength: 1.15,
                        ghost_strength: 0.35,
                        streak_strength: 0.10,
                        warm_tint: 0.30,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "指定した光源から、にじみ、薄いリング、レンズ内反射のゴーストを作ります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "フレア光源の横位置です。",
                "フレア光源の縦位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            changed |= ui
                .add(egui::Slider::new(&mut params.radius_px, 4.0..=420.0).text("範囲"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=3.0).text("強さ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.core_strength, 0.0..=2.0).text("コア"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.halo_strength, 0.0..=2.0).text("ハロー"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.ghost_strength, 0.0..=2.0).text("ゴースト"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.streak_strength, 0.0..=2.0).text("光条"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.warm_tint, 0.0..=1.0).text("暖色"))
                .changed();
        }
        LocalEffect::AnamorphicFlare(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "シネマ青") {
                    *params = AnamorphicFlareParams {
                        threshold: 0.62,
                        length_px: 220.0,
                        thickness_px: 3.0,
                        strength: 0.85,
                        color_rgb: [70, 150, 255],
                        color_strength: 0.92,
                    };
                    changed = true;
                }
                if preset_button(ui, "強い横光") {
                    *params = AnamorphicFlareParams {
                        threshold: 0.35,
                        length_px: 320.0,
                        thickness_px: 5.0,
                        strength: 1.25,
                        color_rgb: [90, 180, 255],
                        color_strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡い青") {
                    *params = AnamorphicFlareParams {
                        threshold: 0.72,
                        length_px: 180.0,
                        thickness_px: 2.0,
                        strength: 0.48,
                        color_rgb: [120, 190, 255],
                        color_strength: 0.70,
                    };
                    changed = true;
                }
                if preset_button(ui, "暖色") {
                    *params = AnamorphicFlareParams {
                        threshold: 0.58,
                        length_px: 180.0,
                        thickness_px: 4.0,
                        strength: 0.62,
                        color_rgb: [255, 190, 95],
                        color_strength: 0.65,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明るい部分を拾い、横方向へ色付きのストリークを伸ばします。初期状態では前ON/後OFFなので光がマスク外へ広がります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let threshold = ui.add(
                egui::Slider::new(&mut params.threshold, 0.0..=0.98)
                    .text("明部しきい値")
                    .fixed_decimals(3),
            );
            changed |= threshold.changed();
            threshold.lab_hover_tip(
                "どの明るさからフレアの発生源として拾うかです。低いほど広く反応します。",
            );
            let length = ui.add(
                egui::Slider::new(&mut params.length_px, 1.0..=480.0)
                    .text("長さ")
                    .suffix("px"),
            );
            changed |= length.changed();
            length.lab_hover_tip("水平ストリークをどれだけ長く伸ばすかです。");
            let thickness = ui.add(
                egui::Slider::new(&mut params.thickness_px, 0.0..=48.0)
                    .text("太さ")
                    .suffix("px"),
            );
            changed |= thickness.changed();
            thickness.lab_hover_tip("横光を上下方向にどれだけぼかして太くするかです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=3.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("横光を元画像へ重ねる強さです。");
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "色",
                    &mut params.color_rgb,
                    RgbPickTarget::AnamorphicFlareColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let color_strength =
                ui.add(egui::Slider::new(&mut params.color_strength, 0.0..=1.0).text("着色量"));
            changed |= color_strength.changed();
            color_strength.lab_hover_tip("元の明部色から指定したフレア色へどれだけ寄せるかです。");
        }
        LocalEffect::LightLeak(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "左上") {
                    *params = LightLeakParams {
                        center: [0.06, 0.08],
                        color_rgb: [255, 146, 72],
                        radius: 0.72,
                        intensity: 0.90,
                        falloff: 2.3,
                        haze: 0.30,
                        streak_strength: 0.25,
                        streak_angle_degrees: -28.0,
                        strength: 0.78,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "フィルム端") {
                    *params = LightLeakParams {
                        center: [0.0, 0.50],
                        color_rgb: [255, 92, 70],
                        radius: 0.58,
                        intensity: 1.30,
                        falloff: 1.4,
                        haze: 0.18,
                        streak_strength: 0.62,
                        streak_angle_degrees: -6.0,
                        strength: 0.85,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "斜め筋") {
                    *params = LightLeakParams {
                        center: [0.20, 0.02],
                        color_rgb: [255, 180, 92],
                        radius: 0.90,
                        intensity: 0.95,
                        falloff: 1.7,
                        haze: 0.24,
                        streak_strength: 0.85,
                        streak_angle_degrees: -36.0,
                        strength: 0.82,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかい") {
                    *params = LightLeakParams {
                        center: [0.78, 0.18],
                        color_rgb: [255, 205, 145],
                        radius: 1.05,
                        intensity: 0.62,
                        falloff: 3.2,
                        haze: 0.48,
                        streak_strength: 0.10,
                        streak_angle_degrees: -18.0,
                        strength: 0.58,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "指定位置からスクリーン合成の暖色光、薄いヘイズ、斜めの漏れ筋を重ねます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "光漏れが始まる横位置です。",
                "光漏れが始まる縦位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "光色",
                    &mut params.color_rgb,
                    RgbPickTarget::LightLeakColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let radius = ui.add(egui::Slider::new(&mut params.radius, 0.05..=1.6).text("範囲"));
            changed |= radius.changed();
            radius.lab_hover_tip("光漏れが広がる範囲です。画像の対角線に対する比率です。");
            let intensity =
                ui.add(egui::Slider::new(&mut params.intensity, 0.0..=2.0).text("明るさ"));
            changed |= intensity.changed();
            intensity.lab_hover_tip("光漏れ自体の明るさです。高いほど白く飛びやすくなります。");
            let falloff = ui.add(egui::Slider::new(&mut params.falloff, 0.35..=6.0).text("減衰"));
            changed |= falloff.changed();
            falloff.lab_hover_tip("中心から離れたときの弱まり方です。低いほど広く残ります。");
            let haze = ui.add(egui::Slider::new(&mut params.haze, 0.0..=1.0).text("ヘイズ"));
            changed |= haze.changed();
            haze.lab_hover_tip("広い薄いかぶり光を足す量です。");
            let streak =
                ui.add(egui::Slider::new(&mut params.streak_strength, 0.0..=1.0).text("漏れ筋"));
            changed |= streak.changed();
            streak.lab_hover_tip("斜めのフィルム漏れ筋を足す量です。");
            let angle = ui.add(
                egui::Slider::new(&mut params.streak_angle_degrees, -180.0..=180.0)
                    .text("筋角度")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip("漏れ筋の向きです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からライトリーク結果へどれだけ近づけるかです。");
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("漏れ筋や細かな揺らぎの乱数です。");
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::BacklightHaze(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "夕逆光") {
                    *params = BacklightHazeParams {
                        center: [0.54, 0.10],
                        color_rgb: [255, 214, 156],
                        radius: 0.92,
                        falloff: 1.45,
                        haze: 0.46,
                        glow: 0.58,
                        shadow_lift: 0.32,
                        contrast_fade: 0.22,
                        saturation_fade: 0.08,
                        strength: 0.82,
                    };
                    changed = true;
                }
                if preset_button(ui, "朝もや") {
                    *params = BacklightHazeParams {
                        center: [0.42, 0.04],
                        color_rgb: [255, 238, 205],
                        radius: 1.08,
                        falloff: 2.0,
                        haze: 0.54,
                        glow: 0.22,
                        shadow_lift: 0.28,
                        contrast_fade: 0.34,
                        saturation_fade: 0.18,
                        strength: 0.72,
                    };
                    changed = true;
                }
                if preset_button(ui, "柔らかい") {
                    *params = BacklightHazeParams {
                        center: [0.68, 0.18],
                        color_rgb: [255, 226, 186],
                        radius: 0.82,
                        falloff: 2.6,
                        haze: 0.30,
                        glow: 0.32,
                        shadow_lift: 0.18,
                        contrast_fade: 0.16,
                        saturation_fade: 0.08,
                        strength: 0.55,
                    };
                    changed = true;
                }
                if preset_button(ui, "青い空気") {
                    *params = BacklightHazeParams {
                        center: [0.50, 0.00],
                        color_rgb: [190, 220, 255],
                        radius: 1.20,
                        falloff: 1.75,
                        haze: 0.42,
                        glow: 0.28,
                        shadow_lift: 0.18,
                        contrast_fade: 0.26,
                        saturation_fade: 0.22,
                        strength: 0.68,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "光源方向の薄い空気かぶり、グロー、影の持ち上げをまとめて足します。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "逆光や空気かぶりの光源横位置です。",
                "逆光や空気かぶりの光源縦位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "光色",
                    &mut params.color_rgb,
                    RgbPickTarget::BacklightHazeColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let radius = ui.add(egui::Slider::new(&mut params.radius, 0.05..=1.6).text("範囲"));
            changed |= radius.changed();
            radius.lab_hover_tip("逆光ヘイズが広がる範囲です。画像の対角線に対する比率です。");
            let falloff = ui.add(egui::Slider::new(&mut params.falloff, 0.35..=5.0).text("減衰"));
            changed |= falloff.changed();
            falloff.lab_hover_tip("光源から離れたときの弱まり方です。低いほど広く残ります。");
            let haze = ui.add(egui::Slider::new(&mut params.haze, 0.0..=1.0).text("ヘイズ"));
            changed |= haze.changed();
            haze.lab_hover_tip("薄い空気かぶりを足す量です。");
            let glow = ui.add(egui::Slider::new(&mut params.glow, 0.0..=2.0).text("グロー"));
            changed |= glow.changed();
            glow.lab_hover_tip("明るい部分を中心にスクリーン合成で発光感を足す量です。");
            let shadow_lift =
                ui.add(egui::Slider::new(&mut params.shadow_lift, 0.0..=1.0).text("影持ち上げ"));
            changed |= shadow_lift.changed();
            shadow_lift
                .lab_hover_tip("暗部を光色で持ち上げ、逆光で黒つぶれしすぎないようにします。");
            let contrast = ui.add(
                egui::Slider::new(&mut params.contrast_fade, 0.0..=1.0).text("コントラスト低下"),
            );
            changed |= contrast.changed();
            contrast.lab_hover_tip("光源付近のコントラストを少し浅くして、空気感を出します。");
            let saturation =
                ui.add(egui::Slider::new(&mut params.saturation_fade, 0.0..=1.0).text("彩度低下"));
            changed |= saturation.changed();
            saturation
                .lab_hover_tip("光源付近の彩度を少し落として、もやの中にある見た目へ寄せます。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から逆光ヘイズ結果へどれだけ近づけるかです。");
        }
        LocalEffect::CloudFog(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "薄霧") {
                    *params = CloudFogParams {
                        mode: CloudFogMode::Fog,
                        scale_px: 220.0,
                        detail: 0.35,
                        density: 0.42,
                        contrast: 0.16,
                        height_fade: 0.35,
                        opacity: 0.30,
                        color_rgb: [235, 244, 255],
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "濃霧") {
                    *params = CloudFogParams {
                        mode: CloudFogMode::Fog,
                        scale_px: 150.0,
                        detail: 0.48,
                        density: 0.78,
                        contrast: 0.10,
                        height_fade: 0.08,
                        opacity: 0.58,
                        color_rgb: [232, 238, 246],
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "雲") {
                    *params = CloudFogParams {
                        mode: CloudFogMode::Clouds,
                        scale_px: 96.0,
                        detail: 0.78,
                        density: 0.66,
                        contrast: 0.62,
                        height_fade: 0.0,
                        opacity: 0.72,
                        color_rgb: [255, 255, 255],
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "夕霧") {
                    *params = CloudFogParams {
                        mode: CloudFogMode::Fog,
                        scale_px: 190.0,
                        detail: 0.42,
                        density: 0.55,
                        contrast: 0.22,
                        height_fade: 0.22,
                        opacity: 0.42,
                        color_rgb: [255, 220, 176],
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "手続き型のノイズで霧や雲を重ねます。マスクと組み合わせて遠景や背景に大気感を足せます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                let fog = params.mode == CloudFogMode::Fog;
                if ui.selectable_label(fog, "霧").clicked() && !fog {
                    params.mode = CloudFogMode::Fog;
                    changed = true;
                }
                let clouds = params.mode == CloudFogMode::Clouds;
                if ui.selectable_label(clouds, "雲").clicked() && !clouds {
                    params.mode = CloudFogMode::Clouds;
                    changed = true;
                }
            });
            let scale =
                ui.add(egui::Slider::new(&mut params.scale_px, 8.0..=640.0).text("スケール"));
            changed |= scale.changed();
            scale.lab_hover_tip("ノイズの大きさです。大きいほど広くなだらかな霧になります。");
            let detail = ui.add(egui::Slider::new(&mut params.detail, 0.0..=1.0).text("細部"));
            changed |= detail.changed();
            detail.lab_hover_tip("細かい揺らぎを足します。雲では高め、薄霧では低めが向きます。");
            let density = ui.add(egui::Slider::new(&mut params.density, 0.0..=1.0).text("密度"));
            changed |= density.changed();
            density.lab_hover_tip("霧や雲が画面を覆う量です。");
            let contrast =
                ui.add(egui::Slider::new(&mut params.contrast, 0.0..=1.0).text("コントラスト"));
            changed |= contrast.changed();
            contrast.lab_hover_tip("雲の濃淡差です。霧では低めにすると自然です。");
            let height_fade =
                ui.add(egui::Slider::new(&mut params.height_fade, -1.0..=1.0).text("上下フェード"));
            changed |= height_fade.changed();
            height_fade.lab_hover_tip("正の値で上側、負の値で下側に霧や雲を寄せます。");
            let opacity =
                ui.add(egui::Slider::new(&mut params.opacity, 0.0..=1.0).text("不透明度"));
            changed |= opacity.changed();
            opacity.lab_hover_tip("元画像から霧/雲の色へどれだけ近づけるかです。");
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "色",
                    &mut params.color_rgb,
                    RgbPickTarget::CloudFogColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("霧や雲のパターンを変えます。");
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::WaterCaustics(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "水面") {
                    *params = WaterCausticsParams {
                        scale_px: 48.0,
                        intensity: 0.75,
                        contrast: 0.70,
                        tint: 0.45,
                        depth: 0.18,
                        phase: params.phase,
                        seed: params.seed,
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "水中") {
                    *params = WaterCausticsParams {
                        scale_px: 36.0,
                        intensity: 1.10,
                        contrast: 0.85,
                        tint: 0.75,
                        depth: 0.25,
                        phase: params.phase,
                        seed: params.seed,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "強い光網") {
                    *params = WaterCausticsParams {
                        scale_px: 22.0,
                        intensity: 1.55,
                        contrast: 1.0,
                        tint: 0.55,
                        depth: 0.35,
                        phase: params.phase,
                        seed: params.seed,
                        strength: 0.90,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡い背景") {
                    *params = WaterCausticsParams {
                        scale_px: 76.0,
                        intensity: 0.45,
                        contrast: 0.45,
                        tint: 0.35,
                        depth: 0.10,
                        phase: params.phase,
                        seed: params.seed,
                        strength: 0.45,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "水面越しの揺らぐ光網を重ねます。背景や水中、プールの反射光に向きます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let scale = ui.add(
                egui::Slider::new(&mut params.scale_px, 8.0..=240.0)
                    .text("スケール")
                    .suffix("px"),
            );
            changed |= scale.changed();
            scale.lab_hover_tip("光網の大きさです。小さいほど細かい波紋になります。");
            let intensity =
                ui.add(egui::Slider::new(&mut params.intensity, 0.0..=2.0).text("光量"));
            changed |= intensity.changed();
            intensity.lab_hover_tip("光網の明るさです。暗い場所ほど効果が見えやすくなります。");
            let contrast =
                ui.add(egui::Slider::new(&mut params.contrast, 0.0..=1.0).text("網のコントラスト"));
            changed |= contrast.changed();
            contrast.lab_hover_tip("光網の線をどれだけ細く強く出すかです。");
            let tint = ui.add(egui::Slider::new(&mut params.tint, 0.0..=1.0).text("水色"));
            changed |= tint.changed();
            tint.lab_hover_tip("光を白から水色へ寄せる量です。");
            let depth = ui.add(egui::Slider::new(&mut params.depth, 0.0..=1.0).text("陰影"));
            changed |= depth.changed();
            depth.lab_hover_tip("光網の隙間を少し暗くして、水中の奥行きを足します。");
            let phase = ui.add(egui::Slider::new(&mut params.phase, 0.0..=1.0).text("位相"));
            changed |= phase.changed();
            phase.lab_hover_tip("光網の揺らぎ位置を変えます。静止画では模様違いとして使えます。");
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("光網パターンの乱数を変えます。");
            params.seed = seed.max(0) as u32;
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から水中コースティクス結果へどれだけ近づけるかです。");
        }
        LocalEffect::ParticleOverlay(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "雨") {
                    *params = ParticleOverlayParams {
                        mode: ParticleOverlayMode::Rain,
                        density: 0.58,
                        size_px: 1.4,
                        length_px: 38.0,
                        angle_degrees: 105.0,
                        opacity: 0.45,
                        color_rgb: [210, 230, 255],
                        seed: params.seed,
                        strength: 0.78,
                    };
                    changed = true;
                }
                if preset_button(ui, "強い雨") {
                    *params = ParticleOverlayParams {
                        mode: ParticleOverlayMode::Rain,
                        density: 0.86,
                        size_px: 2.0,
                        length_px: 62.0,
                        angle_degrees: 108.0,
                        opacity: 0.62,
                        color_rgb: [220, 238, 255],
                        seed: params.seed,
                        strength: 0.90,
                    };
                    changed = true;
                }
                if preset_button(ui, "雪") {
                    *params = ParticleOverlayParams {
                        mode: ParticleOverlayMode::Snow,
                        density: 0.46,
                        size_px: 4.2,
                        length_px: 0.0,
                        angle_degrees: 92.0,
                        opacity: 0.74,
                        color_rgb: [255, 255, 255],
                        seed: params.seed,
                        strength: 0.74,
                    };
                    changed = true;
                }
                if preset_button(ui, "花びら") {
                    *params = ParticleOverlayParams {
                        mode: ParticleOverlayMode::Petals,
                        density: 0.34,
                        size_px: 5.5,
                        length_px: 0.0,
                        angle_degrees: 112.0,
                        opacity: 0.76,
                        color_rgb: [255, 166, 206],
                        seed: params.seed,
                        strength: 0.78,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "雨、雪、花びらを手続き型の粒子として重ねます。seedで配置を変えられます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                changed |= ui
                    .selectable_value(&mut params.mode, ParticleOverlayMode::Rain, "雨")
                    .changed();
                changed |= ui
                    .selectable_value(&mut params.mode, ParticleOverlayMode::Snow, "雪")
                    .changed();
                changed |= ui
                    .selectable_value(&mut params.mode, ParticleOverlayMode::Petals, "花びら")
                    .changed();
            });
            let density = ui.add(egui::Slider::new(&mut params.density, 0.0..=1.0).text("密度"));
            changed |= density.changed();
            density.lab_hover_tip("粒子の間隔です。高いほど画面内の粒子が増えます。");
            let size = ui.add(
                egui::Slider::new(&mut params.size_px, 0.5..=24.0)
                    .text("サイズ")
                    .suffix("px"),
            );
            changed |= size.changed();
            size.lab_hover_tip("雨筋の太さ、雪や花びらの大きさです。");
            let length = ui.add(
                egui::Slider::new(&mut params.length_px, 0.0..=160.0)
                    .text("長さ")
                    .suffix("px"),
            );
            changed |= length.changed();
            length.lab_hover_tip("雨筋の長さです。雪や花びらでは配置セルの向きだけに影響します。");
            let angle = ui.add(
                egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                    .text("角度")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip("粒子が落ちる方向です。90°で真下方向になります。");
            let opacity =
                ui.add(egui::Slider::new(&mut params.opacity, 0.0..=1.0).text("不透明度"));
            changed |= opacity.changed();
            opacity.lab_hover_tip("粒子自体の濃さです。");
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "色",
                    &mut params.color_rgb,
                    RgbPickTarget::ParticleOverlayColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("粒子の配置を変えます。");
            params.seed = seed.max(0) as u32;
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から粒子オーバーレイ結果へどれだけ近づけるかです。");
        }
        LocalEffect::Aurora(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "緑紫") {
                    *params = AuroraParams {
                        band_count: 5.0,
                        scale_px: 120.0,
                        height: 0.72,
                        waviness: 0.58,
                        softness: 0.46,
                        brightness: 0.92,
                        color_rgb: [80, 255, 170],
                        secondary_rgb: [150, 105, 255],
                        phase: params.phase,
                        seed: params.seed,
                        strength: 0.78,
                    };
                    changed = true;
                }
                if preset_button(ui, "青緑") {
                    *params = AuroraParams {
                        band_count: 6.0,
                        scale_px: 100.0,
                        height: 0.68,
                        waviness: 0.70,
                        softness: 0.42,
                        brightness: 1.05,
                        color_rgb: [70, 230, 255],
                        secondary_rgb: [90, 255, 160],
                        phase: params.phase,
                        seed: params.seed,
                        strength: 0.82,
                    };
                    changed = true;
                }
                if preset_button(ui, "強い光") {
                    *params = AuroraParams {
                        band_count: 8.0,
                        scale_px: 72.0,
                        height: 0.86,
                        waviness: 0.86,
                        softness: 0.34,
                        brightness: 1.45,
                        color_rgb: [90, 255, 145],
                        secondary_rgb: [210, 95, 255],
                        phase: params.phase,
                        seed: params.seed,
                        strength: 0.92,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡い空気") {
                    *params = AuroraParams {
                        band_count: 4.0,
                        scale_px: 170.0,
                        height: 0.58,
                        waviness: 0.42,
                        softness: 0.72,
                        brightness: 0.58,
                        color_rgb: [130, 255, 205],
                        secondary_rgb: [135, 170, 255],
                        phase: params.phase,
                        seed: params.seed,
                        strength: 0.58,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "縦に揺れる発光カーテンをスクリーン合成します。夜空や幻想的な背景に向きます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let bands =
                ui.add(egui::Slider::new(&mut params.band_count, 1.0..=12.0).text("カーテン数"));
            changed |= bands.changed();
            bands.lab_hover_tip("横方向に並ぶ光の帯の数です。");
            let scale = ui.add(
                egui::Slider::new(&mut params.scale_px, 24.0..=480.0)
                    .text("幅")
                    .suffix("px"),
            );
            changed |= scale.changed();
            scale.lab_hover_tip("光の帯と揺らぎの大きさです。大きいほどゆったりします。");
            let height = ui.add(egui::Slider::new(&mut params.height, 0.08..=1.0).text("高さ"));
            changed |= height.changed();
            height.lab_hover_tip("上側からどこまで光を伸ばすかです。");
            let waviness =
                ui.add(egui::Slider::new(&mut params.waviness, 0.0..=1.0).text("揺らぎ"));
            changed |= waviness.changed();
            waviness.lab_hover_tip("光の帯をどれだけ波打たせるかです。");
            let softness =
                ui.add(egui::Slider::new(&mut params.softness, 0.0..=1.0).text("柔らかさ"));
            changed |= softness.changed();
            softness.lab_hover_tip("帯の境界と縦方向フェードを柔らかくします。");
            let brightness =
                ui.add(egui::Slider::new(&mut params.brightness, 0.0..=2.0).text("明るさ"));
            changed |= brightness.changed();
            brightness.lab_hover_tip("オーロラの発光量です。暗い背景ほど見えやすくなります。");
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "主色",
                    &mut params.color_rgb,
                    RgbPickTarget::AuroraColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "副色",
                    &mut params.secondary_rgb,
                    RgbPickTarget::AuroraSecondaryColor,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let phase = ui.add(egui::Slider::new(&mut params.phase, 0.0..=1.0).text("位相"));
            changed |= phase.changed();
            phase.lab_hover_tip("光の揺らぎ位置を変えます。静止画では模様違いとして使えます。");
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("光カーテンの乱数を変えます。");
            params.seed = seed.max(0) as u32;
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からオーロラ合成結果へどれだけ近づけるかです。");
        }
        LocalEffect::Spotlight(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "主役ライト") {
                    *params = SpotlightParams {
                        center: [0.50, 0.42],
                        radius: 0.24,
                        feather: 0.36,
                        light_strength: 0.75,
                        shadow_strength: 0.38,
                        tint_rgb: [255, 238, 200],
                        tint_strength: 0.22,
                    };
                    changed = true;
                }
                if preset_button(ui, "舞台") {
                    *params = SpotlightParams {
                        center: [0.50, 0.24],
                        radius: 0.18,
                        feather: 0.30,
                        light_strength: 1.05,
                        shadow_strength: 0.62,
                        tint_rgb: [255, 244, 220],
                        tint_strength: 0.18,
                    };
                    changed = true;
                }
                if preset_button(ui, "夕光") {
                    *params = SpotlightParams {
                        center: [0.28, 0.32],
                        radius: 0.30,
                        feather: 0.42,
                        light_strength: 0.65,
                        shadow_strength: 0.30,
                        tint_rgb: [255, 190, 118],
                        tint_strength: 0.42,
                    };
                    changed = true;
                }
                if preset_button(ui, "暗転") {
                    *params = SpotlightParams {
                        center: [0.50, 0.50],
                        radius: 0.26,
                        feather: 0.28,
                        light_strength: 0.20,
                        shadow_strength: 0.78,
                        tint_rgb: [230, 240, 255],
                        tint_strength: 0.08,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "指定中心を照らし、周辺を落として視線誘導や舞台照明のような局所光を作ります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            changed |= draw_effect_center_controls(
                ui,
                &mut params.center,
                "ライト中心の横位置です。",
                "ライト中心の縦位置です。",
                effect_position_handles_visible,
                &mut set_effect_position_handles_visible,
            );
            let radius = ui.add(egui::Slider::new(&mut params.radius, 0.0..=1.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip("明るい中心部の大きさです。");
            let feather =
                ui.add(egui::Slider::new(&mut params.feather, 0.001..=1.0).text("ぼかし"));
            changed |= feather.changed();
            feather.lab_hover_tip("中心から外側へのなだらかさです。");
            let light = ui
                .add(egui::Slider::new(&mut params.light_strength, -1.0..=2.0).text("中心明るさ"));
            changed |= light.changed();
            light.lab_hover_tip("正の値で中心を明るく、負の値で中心を暗くします。");
            let shadow =
                ui.add(egui::Slider::new(&mut params.shadow_strength, 0.0..=1.0).text("周辺影"));
            changed |= shadow.changed();
            shadow.lab_hover_tip("スポット外側を暗く落とす強さです。");
            let tint = ui.add(egui::Slider::new(&mut params.tint_strength, 0.0..=1.0).text("光色"));
            changed |= tint.changed();
            tint.lab_hover_tip("中心部へ指定色をどれだけ混ぜるかです。");
            merge_rgb_color_response(
                draw_rgb_color_control(
                    ui,
                    "光色",
                    &mut params.tint_rgb,
                    RgbPickTarget::SpotlightTint,
                    rgb_pick_active,
                ),
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
        }
        LocalEffect::Vignette(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "周辺を暗く") {
                    *params = VignetteParams {
                        strength: 0.35,
                        radius: 0.52,
                        feather: 0.36,
                    };
                    changed = true;
                }
                if preset_button(ui, "周辺を明るく") {
                    *params = VignetteParams {
                        strength: -0.25,
                        radius: 0.50,
                        feather: 0.38,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, -1.0..=1.0).text("強さ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.radius, 0.0..=1.0).text("開始半径"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.feather, 0.01..=1.0).text("ぼかし幅"))
                .changed();
        }
        LocalEffect::FilmGrain(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "微量") {
                    *params = FilmGrainParams {
                        amount: 0.08,
                        size_px: 1,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "フィルム") {
                    *params = FilmGrainParams {
                        amount: 0.18,
                        size_px: 2,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.amount, 0.0..=1.0).text("量"))
                .changed();
            let mut size = params.size_px as i32;
            changed |= ui
                .add(egui::Slider::new(&mut size, 1..=12).text("粒サイズ(px)"))
                .changed();
            params.size_px = size.max(1) as u32;
            let mut seed = params.seed as i32;
            changed |= ui
                .add(egui::Slider::new(&mut seed, 0..=9999).text("seed"))
                .changed();
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::Noise(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "微量") {
                    *params = NoiseParams {
                        amount: 0.08,
                        distribution: NoiseDistribution::Uniform,
                        monochrome: true,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "ガウス") {
                    *params = NoiseParams {
                        amount: 0.22,
                        distribution: NoiseDistribution::Gaussian,
                        monochrome: true,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "カラー") {
                    *params = NoiseParams {
                        amount: 0.18,
                        distribution: NoiseDistribution::Uniform,
                        monochrome: false,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.amount, 0.0..=1.0).text("量"))
                .changed();
            let previous_distribution = params.distribution;
            ComboBox::from_label("分布")
                .selected_text(noise_distribution_label(params.distribution))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut params.distribution,
                        NoiseDistribution::Uniform,
                        "均一",
                    );
                    ui.selectable_value(
                        &mut params.distribution,
                        NoiseDistribution::Gaussian,
                        "ガウス",
                    );
                });
            changed |= params.distribution != previous_distribution;
            changed |= ui.checkbox(&mut params.monochrome, "単色ノイズ").changed();
            let mut seed = params.seed as i32;
            changed |= ui
                .add(egui::Slider::new(&mut seed, 0..=9999).text("seed"))
                .changed();
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::ChromaticAberration(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "微量") {
                    params.offset_px = 1.2;
                    changed = true;
                }
                if preset_button(ui, "演出") {
                    params.offset_px = 3.0;
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.offset_px, 0.0..=24.0).text("ずれ(px)"))
                .changed();
        }
        LocalEffect::Anaglyph3d(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "赤シアン") {
                    *params = AnaglyphParams {
                        mode: AnaglyphMode::RedCyan,
                        disparity_px: 8.0,
                        angle_degrees: 0.0,
                        luma_mix: 0.55,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "立体弱め") {
                    *params = AnaglyphParams {
                        mode: AnaglyphMode::RedCyan,
                        disparity_px: 4.0,
                        angle_degrees: 0.0,
                        luma_mix: 0.70,
                        strength: 0.65,
                    };
                    changed = true;
                }
                if preset_button(ui, "琥珀青") {
                    *params = AnaglyphParams {
                        mode: AnaglyphMode::AmberBlue,
                        disparity_px: 7.0,
                        angle_degrees: 0.0,
                        luma_mix: 0.50,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "RGB分離") {
                    *params = AnaglyphParams {
                        mode: AnaglyphMode::RgbSplit,
                        disparity_px: 10.0,
                        angle_degrees: 0.0,
                        luma_mix: 0.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "斜めズレ") {
                    *params = AnaglyphParams {
                        mode: AnaglyphMode::RgbSplit,
                        disparity_px: 8.0,
                        angle_degrees: -25.0,
                        luma_mix: 0.0,
                        strength: 0.85,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "左右にずらした画像を色チャンネルへ割り当てます。赤シアンは立体視風、RGB分離はグリッチ寄りの色ズレに向きます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let mut activates_effect = false;
            let previous_mode = params.mode;
            ComboBox::from_label("方式")
                .selected_text(anaglyph_mode_label(params.mode))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut params.mode,
                        AnaglyphMode::RedCyan,
                        anaglyph_mode_label(AnaglyphMode::RedCyan),
                    );
                    ui.selectable_value(
                        &mut params.mode,
                        AnaglyphMode::GreenMagenta,
                        anaglyph_mode_label(AnaglyphMode::GreenMagenta),
                    );
                    ui.selectable_value(
                        &mut params.mode,
                        AnaglyphMode::AmberBlue,
                        anaglyph_mode_label(AnaglyphMode::AmberBlue),
                    );
                    ui.selectable_value(
                        &mut params.mode,
                        AnaglyphMode::RgbSplit,
                        anaglyph_mode_label(AnaglyphMode::RgbSplit),
                    );
                });
            if params.mode != previous_mode {
                changed = true;
                activates_effect = true;
            }
            let disparity = ui.add(
                egui::Slider::new(&mut params.disparity_px, 0.0..=96.0)
                    .text("視差")
                    .suffix("px"),
            );
            changed |= disparity.changed();
            activates_effect |= disparity.changed();
            disparity
                .lab_hover_tip("左右画像の総ずれ量です。大きいほど色分離と立体感が強くなります。");
            let angle = ui.add(
                egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                    .text("方向")
                    .suffix("°"),
            );
            changed |= angle.changed();
            activates_effect |= angle.changed();
            angle.lab_hover_tip(
                "視差をかける方向です。通常の3D風は0°、演出色ズレは斜めも使えます。",
            );
            let luma_mix =
                ui.add(egui::Slider::new(&mut params.luma_mix, 0.0..=1.0).text("明度化"));
            changed |= luma_mix.changed();
            activates_effect |= luma_mix.changed();
            luma_mix.lab_hover_tip(
                "左右画像を明度へ寄せてから色チャンネルへ割り当てます。色の暴れを抑えます。",
            );
            if activates_effect && params.strength <= f32::EPSILON {
                params.strength = 1.0;
                changed = true;
            }
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からアナグリフ結果へ切り替える強さです。");
        }
        LocalEffect::Defringe(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "軽く") {
                    *params = DefringeParams {
                        radius_px: 1.0,
                        edge_threshold: 0.08,
                        color_threshold: 0.18,
                        neutralize: 0.55,
                        strength: 0.55,
                    };
                    changed = true;
                }
                if preset_button(ui, "紫/緑フチ") {
                    *params = DefringeParams {
                        radius_px: 1.0,
                        edge_threshold: 0.05,
                        color_threshold: 0.12,
                        neutralize: 0.86,
                        strength: 0.78,
                    };
                    changed = true;
                }
                if preset_button(ui, "強め") {
                    *params = DefringeParams {
                        radius_px: 2.0,
                        edge_threshold: 0.04,
                        color_threshold: 0.08,
                        neutralize: 1.0,
                        strength: 0.90,
                    };
                    changed = true;
                }
                if preset_button(ui, "広め") {
                    *params = DefringeParams {
                        radius_px: 3.0,
                        edge_threshold: 0.06,
                        color_threshold: 0.14,
                        neutralize: 0.75,
                        strength: 0.72,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "周辺より彩度が高いエッジ上の色フチだけを検出し、局所的に彩度を落とします。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let radius = ui.add(
                egui::Slider::new(&mut params.radius_px, 1.0..=8.0)
                    .text("検出半径")
                    .suffix("px"),
            );
            changed |= radius.changed();
            radius.lab_hover_tip("周辺色と比較する距離です。太いフチには大きめが向きます。");
            let edge = ui.add(
                egui::Slider::new(&mut params.edge_threshold, 0.0..=1.0).text("エッジしきい値"),
            );
            changed |= edge.changed();
            edge.lab_hover_tip("どれだけ明暗差がある場所を色フチ候補にするかです。");
            let color = ui.add(
                egui::Slider::new(&mut params.color_threshold, 0.0..=1.0).text("色フチしきい値"),
            );
            changed |= color.changed();
            color.lab_hover_tip("周辺より彩度がどれだけ高い場合に補正するかです。");
            let neutralize =
                ui.add(egui::Slider::new(&mut params.neutralize, 0.0..=1.0).text("中和"));
            changed |= neutralize.changed();
            neutralize.lab_hover_tip("検出した色フチをどれだけ無彩色へ寄せるかです。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から色フチ除去結果へどれだけ近づけるかです。");
        }
        LocalEffect::ScanlineGlitch(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "ホログラム") {
                    *params = ScanlineGlitchParams {
                        line_spacing_px: 4.0,
                        line_strength: 0.42,
                        jitter_px: 1.5,
                        rgb_shift_px: 1.2,
                        block_strength: 0.28,
                        noise: 0.12,
                        seed: params.seed,
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "走査線") {
                    *params = ScanlineGlitchParams {
                        line_spacing_px: 3.0,
                        line_strength: 0.65,
                        jitter_px: 0.0,
                        rgb_shift_px: 0.4,
                        block_strength: 0.0,
                        noise: 0.04,
                        seed: params.seed,
                        strength: 0.70,
                    };
                    changed = true;
                }
                if preset_button(ui, "破損") {
                    *params = ScanlineGlitchParams {
                        line_spacing_px: 5.0,
                        line_strength: 0.55,
                        jitter_px: 10.0,
                        rgb_shift_px: 4.0,
                        block_strength: 0.75,
                        noise: 0.35,
                        seed: params.seed,
                        strength: 0.95,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡いSF") {
                    *params = ScanlineGlitchParams {
                        line_spacing_px: 6.0,
                        line_strength: 0.25,
                        jitter_px: 0.8,
                        rgb_shift_px: 0.8,
                        block_strength: 0.12,
                        noise: 0.06,
                        seed: params.seed,
                        strength: 0.45,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new("横走査線、行ごとのずれ、RGBずれを重ねるデジタル演出です。")
                    .size(10.0)
                    .color(Color32::from_gray(170)),
            );
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.line_spacing_px, 2.0..=64.0)
                        .text("走査線間隔(px)"),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.line_strength, 0.0..=1.0).text("走査線"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.jitter_px, 0.0..=48.0).text("行ずれ(px)"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.rgb_shift_px, 0.0..=24.0).text("RGBずれ(px)"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.block_strength, 0.0..=1.0).text("破損行"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.noise, 0.0..=1.0).text("ノイズ"))
                .changed();
            let mut seed = params.seed as i32;
            changed |= ui
                .add(egui::Slider::new(&mut seed, 0..=9999).text("seed"))
                .changed();
            params.seed = seed.max(0) as u32;
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
        }
        LocalEffect::Vhs(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "VHS標準") {
                    *params = VhsParams {
                        chroma_bleed_px: 4.0,
                        chroma_shift_px: 1.5,
                        ghost_offset_px: 5.0,
                        ghost_strength: 0.22,
                        tracking_strength: 0.25,
                        scanline_strength: 0.25,
                        noise: 0.08,
                        desaturation: 0.25,
                        seed: params.seed,
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "古いテープ") {
                    *params = VhsParams {
                        chroma_bleed_px: 8.0,
                        chroma_shift_px: 2.0,
                        ghost_offset_px: 10.0,
                        ghost_strength: 0.35,
                        tracking_strength: 0.55,
                        scanline_strength: 0.45,
                        noise: 0.18,
                        desaturation: 0.45,
                        seed: params.seed,
                        strength: 0.90,
                    };
                    changed = true;
                }
                if preset_button(ui, "色にじみ") {
                    *params = VhsParams {
                        chroma_bleed_px: 12.0,
                        chroma_shift_px: 3.0,
                        ghost_offset_px: 2.0,
                        ghost_strength: 0.10,
                        tracking_strength: 0.10,
                        scanline_strength: 0.10,
                        noise: 0.04,
                        desaturation: 0.15,
                        seed: params.seed,
                        strength: 0.70,
                    };
                    changed = true;
                }
                if preset_button(ui, "トラッキング") {
                    *params = VhsParams {
                        chroma_bleed_px: 3.0,
                        chroma_shift_px: 1.0,
                        ghost_offset_px: 4.0,
                        ghost_strength: 0.16,
                        tracking_strength: 0.85,
                        scanline_strength: 0.35,
                        noise: 0.22,
                        desaturation: 0.30,
                        seed: params.seed,
                        strength: 0.90,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "輝度は残し、色成分だけを横ににじませるアナログビデオ風の演出です。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.chroma_bleed_px, 0.0..=32.0).text("色にじみ(px)"),
                )
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.chroma_shift_px, -24.0..=24.0).text("色ずれ(px)"),
                )
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.ghost_offset_px, 0.0..=64.0)
                        .text("ゴースト距離(px)"),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.ghost_strength, 0.0..=1.0).text("ゴースト"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.tracking_strength, 0.0..=1.0)
                        .text("トラッキング"),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.scanline_strength, 0.0..=1.0).text("走査線"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.noise, 0.0..=1.0).text("ノイズ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.desaturation, 0.0..=1.0).text("退色"))
                .changed();
            let mut seed = params.seed as i32;
            changed |= ui
                .add(egui::Slider::new(&mut seed, 0..=9999).text("seed"))
                .changed();
            params.seed = seed.max(0) as u32;
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
        }
        LocalEffect::DataMosh(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "軽い破綻") {
                    *params = DataMoshParams {
                        block_size_px: 18.0,
                        displacement_px: 7.0,
                        direction_degrees: 0.0,
                        low_threshold: 0.08,
                        high_threshold: 0.96,
                        freeze: 0.25,
                        smear: 0.12,
                        rgb_shift_px: 0.9,
                        noise: 0.05,
                        seed: params.seed,
                        strength: 0.55,
                    };
                    changed = true;
                }
                if preset_button(ui, "ブロック") {
                    *params = DataMoshParams {
                        block_size_px: 24.0,
                        displacement_px: 22.0,
                        direction_degrees: 0.0,
                        low_threshold: 0.05,
                        high_threshold: 0.98,
                        freeze: 0.65,
                        smear: 0.35,
                        rgb_shift_px: 0.5,
                        noise: 0.08,
                        seed: params.seed,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "RGB崩れ") {
                    *params = DataMoshParams {
                        block_size_px: 12.0,
                        displacement_px: 8.0,
                        direction_degrees: 0.0,
                        low_threshold: 0.0,
                        high_threshold: 1.0,
                        freeze: 0.35,
                        smear: 0.15,
                        rgb_shift_px: 6.0,
                        noise: 0.16,
                        seed: params.seed,
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "強い") {
                    *params = DataMoshParams {
                        block_size_px: 10.0,
                        displacement_px: 34.0,
                        direction_degrees: -8.0,
                        low_threshold: 0.0,
                        high_threshold: 1.0,
                        freeze: 0.90,
                        smear: 0.75,
                        rgb_shift_px: 9.0,
                        noise: 0.28,
                        seed: params.seed,
                        strength: 0.95,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "ブロック単位のずれ、フリーズ、RGB分離、ノイズを重ねるデジタル破損演出です。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let block_size = ui.add(
                egui::Slider::new(&mut params.block_size_px, 2.0..=128.0)
                    .text("ブロック")
                    .suffix("px"),
            );
            changed |= block_size.changed();
            block_size.lab_hover_tip(
                "破損をまとめるブロックの大きさです。大きいほど粗い崩れになります。",
            );
            let displacement = ui.add(
                egui::Slider::new(&mut params.displacement_px, 0.0..=128.0)
                    .text("ずれ")
                    .suffix("px"),
            );
            changed |= displacement.changed();
            displacement.lab_hover_tip("ブロックが指定方向へ引きずられる距離です。");
            let direction = ui.add(
                egui::Slider::new(&mut params.direction_degrees, -180.0..=180.0)
                    .text("方向")
                    .suffix("°"),
            );
            changed |= direction.changed();
            direction.lab_hover_tip("ブロックずれとRGB分離の基準方向です。");
            let low =
                ui.add(egui::Slider::new(&mut params.low_threshold, 0.0..=1.0).text("明るさ下限"));
            changed |= low.changed();
            low.lab_hover_tip(
                "この明るさ以上を破損対象にします。上限より大きい場合は内部で入れ替えます。",
            );
            let high =
                ui.add(egui::Slider::new(&mut params.high_threshold, 0.0..=1.0).text("明るさ上限"));
            changed |= high.changed();
            high.lab_hover_tip(
                "この明るさ以下を破損対象にします。明部だけ、暗部だけの崩し分けに使えます。",
            );
            let freeze = ui.add(egui::Slider::new(&mut params.freeze, 0.0..=1.0).text("フリーズ"));
            changed |= freeze.changed();
            freeze.lab_hover_tip(
                "ブロック単位のずれが出る量です。上げるほど破損ブロックが増えます。",
            );
            let smear = ui.add(egui::Slider::new(&mut params.smear, 0.0..=1.0).text("スメア"));
            changed |= smear.changed();
            smear.lab_hover_tip("ずれた方向へ色を引きずる量です。");
            let rgb = ui.add(
                egui::Slider::new(&mut params.rgb_shift_px, 0.0..=32.0)
                    .text("RGBずれ")
                    .suffix("px"),
            );
            changed |= rgb.changed();
            rgb.lab_hover_tip("赤と青のチャンネルを逆方向へずらします。");
            let noise = ui.add(egui::Slider::new(&mut params.noise, 0.0..=1.0).text("ノイズ"));
            changed |= noise.changed();
            noise.lab_hover_tip("デジタル破損らしい細かな色ノイズを足します。");
            let mut seed = params.seed as i32;
            let seed_response = ui.add(egui::Slider::new(&mut seed, 0..=9999).text("seed"));
            changed |= seed_response.changed();
            seed_response.lab_hover_tip("破損パターンを切り替えます。");
            params.seed = seed.max(0) as u32;
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像からデータモッシュ結果へどれだけ近づけるかです。");
        }
        LocalEffect::PixelSort(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "横流れ") {
                    *params = PixelSortParams {
                        direction: PixelSortDirection::Horizontal,
                        order: PixelSortOrder::LightToDark,
                        low_threshold: 0.25,
                        high_threshold: 0.95,
                        max_segment_px: 220,
                        strength: 0.80,
                    };
                    changed = true;
                }
                if preset_button(ui, "縦流れ") {
                    *params = PixelSortParams {
                        direction: PixelSortDirection::Vertical,
                        order: PixelSortOrder::LightToDark,
                        low_threshold: 0.30,
                        high_threshold: 0.90,
                        max_segment_px: 160,
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "明部だけ") {
                    *params = PixelSortParams {
                        direction: PixelSortDirection::Horizontal,
                        order: PixelSortOrder::LightToDark,
                        low_threshold: 0.62,
                        high_threshold: 1.0,
                        max_segment_px: 260,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "暗部だけ") {
                    *params = PixelSortParams {
                        direction: PixelSortDirection::Horizontal,
                        order: PixelSortOrder::DarkToLight,
                        low_threshold: 0.0,
                        high_threshold: 0.42,
                        max_segment_px: 180,
                        strength: 0.80,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "指定した明るさ帯の連続画素だけを行または列方向に並べ替えます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );

            let previous_direction = params.direction;
            ComboBox::from_label("方向")
                .selected_text(pixel_sort_direction_label(params.direction))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut params.direction,
                        PixelSortDirection::Horizontal,
                        "横方向",
                    );
                    ui.selectable_value(
                        &mut params.direction,
                        PixelSortDirection::Vertical,
                        "縦方向",
                    );
                });
            changed |= params.direction != previous_direction;

            let previous_order = params.order;
            ComboBox::from_label("並び順")
                .selected_text(pixel_sort_order_label(params.order))
                .show_ui(ui, |ui| {
                    ui.selectable_value(
                        &mut params.order,
                        PixelSortOrder::LightToDark,
                        "明るい→暗い",
                    );
                    ui.selectable_value(
                        &mut params.order,
                        PixelSortOrder::DarkToLight,
                        "暗い→明るい",
                    );
                });
            changed |= params.order != previous_order;

            changed |= ui
                .add(egui::Slider::new(&mut params.low_threshold, 0.0..=1.0).text("明るさ下限"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.high_threshold, 0.0..=1.0).text("明るさ上限"))
                .changed();
            let mut max_segment = params.max_segment_px as i32;
            changed |= ui
                .add(egui::Slider::new(&mut max_segment, 2..=512).text("最大長(px)"))
                .changed();
            params.max_segment_px = max_segment.max(2) as u32;
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
        }
        LocalEffect::OldFilm(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "古写真") {
                    *params = OldFilmParams {
                        sepia: 0.75,
                        fade: 0.55,
                        vignette: 0.45,
                        grain: 0.20,
                        dust: 0.10,
                        scratches: 0.06,
                        seed: params.seed,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "傷フィルム") {
                    *params = OldFilmParams {
                        sepia: 0.30,
                        fade: 0.35,
                        vignette: 0.35,
                        grain: 0.25,
                        dust: 0.30,
                        scratches: 0.55,
                        seed: params.seed,
                        strength: 0.90,
                    };
                    changed = true;
                }
                if preset_button(ui, "退色") {
                    *params = OldFilmParams {
                        sepia: 0.45,
                        fade: 0.80,
                        vignette: 0.20,
                        grain: 0.10,
                        dust: 0.08,
                        scratches: 0.05,
                        seed: params.seed,
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "白黒古フィルム") {
                    *params = OldFilmParams {
                        sepia: 0.05,
                        fade: 0.95,
                        vignette: 0.45,
                        grain: 0.25,
                        dust: 0.20,
                        scratches: 0.35,
                        seed: params.seed,
                        strength: 0.85,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new("セピア調の退色、周辺落ち、粒子、ホコリ、縦傷を重ねます。")
                    .size(10.0)
                    .color(Color32::from_gray(170)),
            );
            changed |= ui
                .add(egui::Slider::new(&mut params.sepia, 0.0..=1.0).text("セピア"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.fade, 0.0..=1.0).text("退色"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.vignette, 0.0..=1.0).text("ビネット"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.grain, 0.0..=1.0).text("粒子"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.dust, 0.0..=1.0).text("ホコリ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.scratches, 0.0..=1.0).text("縦傷"))
                .changed();
            let mut seed = params.seed as i32;
            changed |= ui
                .add(egui::Slider::new(&mut seed, 0..=9999).text("seed"))
                .changed();
            params.seed = seed.max(0) as u32;
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
        }
        LocalEffect::Halftone(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "細かい") {
                    *params = HalftoneParams {
                        cell_px: 6,
                        strength: 0.35,
                    };
                    changed = true;
                }
                if preset_button(ui, "漫画風") {
                    *params = HalftoneParams {
                        cell_px: 10,
                        strength: 0.70,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "印刷網点風の演出です。背景や効果線、漫画調の質感付け向けです。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let mut cell = params.cell_px as i32;
            changed |= ui
                .add(egui::Slider::new(&mut cell, 2..=96).text("セル(px)"))
                .changed();
            params.cell_px = cell.max(2) as u32;
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
        }
        LocalEffect::ScreenTone(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "網点") {
                    *params = ScreenToneParams {
                        mode: ScreenToneMode::Dots,
                        cell_px: 8.0,
                        angle_degrees: 45.0,
                        density: 0.60,
                        gradation: 0.60,
                        softness: 0.08,
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "細線") {
                    *params = ScreenToneParams {
                        mode: ScreenToneMode::Lines,
                        cell_px: 6.0,
                        angle_degrees: -35.0,
                        density: 0.34,
                        gradation: 0.35,
                        softness: 0.03,
                        strength: 0.70,
                    };
                    changed = true;
                }
                if preset_button(ui, "カケアミ") {
                    *params = ScreenToneParams {
                        mode: ScreenToneMode::CrossHatch,
                        cell_px: 8.0,
                        angle_degrees: 30.0,
                        density: 0.55,
                        gradation: 0.45,
                        softness: 0.02,
                        strength: 0.80,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡い背景") {
                    *params = ScreenToneParams {
                        mode: ScreenToneMode::Dots,
                        cell_px: 12.0,
                        angle_degrees: 45.0,
                        density: 0.26,
                        gradation: 0.0,
                        softness: 0.10,
                        strength: 0.55,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "漫画用のトーンです。階調追従を下げると均一なトーン、上げると元画像の明暗に沿ったトーンになります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                changed |= ui
                    .selectable_value(&mut params.mode, ScreenToneMode::Dots, "網点")
                    .changed();
                changed |= ui
                    .selectable_value(&mut params.mode, ScreenToneMode::Lines, "線")
                    .changed();
                changed |= ui
                    .selectable_value(&mut params.mode, ScreenToneMode::CrossHatch, "カケアミ")
                    .changed();
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.cell_px, 2.0..=128.0).text("セル(px)"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0).text("角度"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.density, 0.0..=1.0).text("濃度"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.gradation, 0.0..=1.0).text("階調追従"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.softness, 0.0..=1.0).text("柔らかさ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
        }
        LocalEffect::ColorHalftone(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "ポップ") {
                    *params = ColorHalftoneParams {
                        cell_px: 8.0,
                        angle_offset_degrees: 0.0,
                        dot_gain: 0.10,
                        black_generation: 0.55,
                        softness: 0.03,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "粗い印刷") {
                    *params = ColorHalftoneParams {
                        cell_px: 16.0,
                        angle_offset_degrees: 0.0,
                        dot_gain: 0.06,
                        black_generation: 0.80,
                        softness: 0.0,
                        strength: 0.80,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡いCMYK") {
                    *params = ColorHalftoneParams {
                        cell_px: 11.0,
                        angle_offset_degrees: 0.0,
                        dot_gain: -0.08,
                        black_generation: 0.45,
                        softness: 0.10,
                        strength: 0.60,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "CMYKの4版を角度違いのドットにします。ドット増減を上げるとインクが太り、印刷物らしい粗さが出ます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            changed |= ui
                .add(egui::Slider::new(&mut params.cell_px, 3.0..=160.0).text("セル(px)"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.angle_offset_degrees, -180.0..=180.0)
                        .text("角度オフセット"),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.dot_gain, -0.5..=0.5).text("ドット増減"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.black_generation, 0.0..=1.0).text("黒版量"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.softness, 0.0..=1.0).text("柔らかさ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
        }
        LocalEffect::CmykPlateShift(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "軽い版ズレ") {
                    *params = CmykPlateShiftParams {
                        offset_px: 1.6,
                        angle_degrees: 0.0,
                        black_offset_px: 0.3,
                        black_generation: 0.70,
                        ink_gain: 0.0,
                        strength: 0.85,
                    };
                    changed = true;
                }
                if preset_button(ui, "リソグラフ") {
                    *params = CmykPlateShiftParams {
                        offset_px: 4.2,
                        angle_degrees: -18.0,
                        black_offset_px: 1.0,
                        black_generation: 0.45,
                        ink_gain: 0.08,
                        strength: 0.90,
                    };
                    changed = true;
                }
                if preset_button(ui, "粗い印刷") {
                    *params = CmykPlateShiftParams {
                        offset_px: 7.0,
                        angle_degrees: 12.0,
                        black_offset_px: -1.8,
                        black_generation: 0.82,
                        ink_gain: 0.12,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡いズレ") {
                    *params = CmykPlateShiftParams {
                        offset_px: 2.4,
                        angle_degrees: 35.0,
                        black_offset_px: 0.0,
                        black_generation: 0.55,
                        ink_gain: -0.05,
                        strength: 0.55,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "CMYKの各色版を少し違う位置から重ね直します。カラーハーフトーンや紙目と組み合わせると印刷物らしいズレが出ます。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let offset = ui.add(
                egui::Slider::new(&mut params.offset_px, 0.0..=32.0)
                    .text("色版ずれ")
                    .suffix("px"),
            );
            changed |= offset.changed();
            offset.lab_hover_tip("シアン、マゼンタ、イエロー版をどれだけずらすかです。");
            let angle = ui.add(
                egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                    .text("ずれ方向")
                    .suffix("°"),
            );
            changed |= angle.changed();
            angle.lab_hover_tip("シアン版がずれる方向です。ほかの色版は別方向へ散らします。");
            let black_offset = ui.add(
                egui::Slider::new(&mut params.black_offset_px, -16.0..=16.0)
                    .text("黒版ずれ")
                    .suffix("px"),
            );
            changed |= black_offset.changed();
            black_offset.lab_hover_tip("黒版だけの追加ずれです。小さめにすると輪郭が締まります。");
            let black =
                ui.add(egui::Slider::new(&mut params.black_generation, 0.0..=1.0).text("黒版量"));
            changed |= black.changed();
            black.lab_hover_tip("暗部を黒版へどれだけ分担させるかです。");
            let gain =
                ui.add(egui::Slider::new(&mut params.ink_gain, -0.35..=0.35).text("インク増減"));
            changed |= gain.changed();
            gain.lab_hover_tip("各版のインク量を増減します。正で濃く、負で淡くなります。");
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から版ズレ再合成結果へどれだけ近づけるかです。");
        }
        LocalEffect::Lithograph(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "ピンク×シアン") {
                    *params = LithographParams {
                        ink_a_rgb: [238, 64, 95],
                        ink_b_rgb: [32, 163, 197],
                        paper_rgb: [248, 238, 210],
                        ink_density: 0.92,
                        posterization: 0.50,
                        grain: 0.36,
                        misregistration_px: 2.2,
                        angle_degrees: -12.0,
                        paper_texture: 0.28,
                        strength: 0.88,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "青×黄") {
                    *params = LithographParams {
                        ink_a_rgb: [34, 87, 180],
                        ink_b_rgb: [245, 190, 42],
                        paper_rgb: [248, 240, 220],
                        ink_density: 0.86,
                        posterization: 0.42,
                        grain: 0.32,
                        misregistration_px: 1.6,
                        angle_degrees: 18.0,
                        paper_texture: 0.24,
                        strength: 0.82,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "赤×黒") {
                    *params = LithographParams {
                        ink_a_rgb: [210, 35, 45],
                        ink_b_rgb: [34, 32, 30],
                        paper_rgb: [246, 234, 205],
                        ink_density: 1.08,
                        posterization: 0.62,
                        grain: 0.48,
                        misregistration_px: 1.2,
                        angle_degrees: 0.0,
                        paper_texture: 0.36,
                        strength: 0.90,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡色") {
                    *params = LithographParams {
                        ink_a_rgb: [240, 115, 135],
                        ink_b_rgb: [72, 180, 170],
                        paper_rgb: [250, 242, 220],
                        ink_density: 0.62,
                        posterization: 0.28,
                        grain: 0.24,
                        misregistration_px: 0.8,
                        angle_degrees: 8.0,
                        paper_texture: 0.18,
                        strength: 0.68,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "元画像を2色のスポットインクと紙色へ寄せ、少しの版ズレと粒状感を足します。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let ink_a = draw_rgb_color_control(
                ui,
                "インク1",
                &mut params.ink_a_rgb,
                RgbPickTarget::LithographInkA,
                rgb_pick_active,
            );
            merge_rgb_color_response(
                ink_a,
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let ink_b = draw_rgb_color_control(
                ui,
                "インク2",
                &mut params.ink_b_rgb,
                RgbPickTarget::LithographInkB,
                rgb_pick_active,
            );
            merge_rgb_color_response(
                ink_b,
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            let paper = draw_rgb_color_control(
                ui,
                "紙色",
                &mut params.paper_rgb,
                RgbPickTarget::LithographPaper,
                rgb_pick_active,
            );
            merge_rgb_color_response(
                paper,
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            changed |= ui
                .add(egui::Slider::new(&mut params.ink_density, 0.0..=1.6).text("インク濃度"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.posterization, 0.0..=1.0).text("階調の荒さ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.grain, 0.0..=1.0).text("粒状感"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.misregistration_px, 0.0..=32.0)
                        .text("版ズレ")
                        .suffix("px"),
                )
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                        .text("ズレ方向")
                        .suffix("°"),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.paper_texture, 0.0..=1.0).text("紙目"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
            let mut seed = params.seed as i32;
            changed |= ui
                .add(egui::Slider::new(&mut seed, 0..=9999).text("seed"))
                .changed();
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::Engraving(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "古典") {
                    *params = EngravingParams {
                        ink_rgb: [42, 35, 28],
                        paper_rgb: [247, 238, 216],
                        line_spacing_px: 7.0,
                        line_width: 0.62,
                        angle_degrees: -18.0,
                        crosshatch: 0.35,
                        contour_strength: 0.32,
                        tone_levels: 7.0,
                        ink_density: 0.92,
                        paper_texture: 0.30,
                        strength: 0.86,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "細密") {
                    *params = EngravingParams {
                        ink_rgb: [28, 25, 22],
                        paper_rgb: [246, 240, 224],
                        line_spacing_px: 4.0,
                        line_width: 0.54,
                        angle_degrees: -28.0,
                        crosshatch: 0.25,
                        contour_strength: 0.42,
                        tone_levels: 10.0,
                        ink_density: 0.98,
                        paper_texture: 0.18,
                        strength: 0.82,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "クロス") {
                    *params = EngravingParams {
                        ink_rgb: [35, 30, 26],
                        paper_rgb: [248, 236, 210],
                        line_spacing_px: 5.0,
                        line_width: 0.70,
                        angle_degrees: -35.0,
                        crosshatch: 0.82,
                        contour_strength: 0.26,
                        tone_levels: 6.0,
                        ink_density: 1.08,
                        paper_texture: 0.34,
                        strength: 0.90,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡色") {
                    *params = EngravingParams {
                        ink_rgb: [84, 70, 56],
                        paper_rgb: [250, 242, 224],
                        line_spacing_px: 8.0,
                        line_width: 0.48,
                        angle_degrees: -12.0,
                        crosshatch: 0.15,
                        contour_strength: 0.18,
                        tone_levels: 5.0,
                        ink_density: 0.62,
                        paper_texture: 0.20,
                        strength: 0.68,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明暗を線の太さとクロスハッチに置き換え、紙色とインクで古典挿絵の線彫り感を作ります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let ink = draw_rgb_color_control(
                ui,
                "インク",
                &mut params.ink_rgb,
                RgbPickTarget::EngravingInk,
                rgb_pick_active,
            );
            merge_rgb_color_response(ink, &mut changed, &mut start_rgb_pick, &mut cancel_rgb_pick);
            let paper = draw_rgb_color_control(
                ui,
                "紙色",
                &mut params.paper_rgb,
                RgbPickTarget::EngravingPaper,
                rgb_pick_active,
            );
            merge_rgb_color_response(
                paper,
                &mut changed,
                &mut start_rgb_pick,
                &mut cancel_rgb_pick,
            );
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.line_spacing_px, 2.0..=48.0)
                        .text("線間隔")
                        .suffix("px"),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.line_width, 0.05..=1.0).text("線幅"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.angle_degrees, -180.0..=180.0)
                        .text("線角度")
                        .suffix("°"),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.crosshatch, 0.0..=1.0).text("クロス線"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.contour_strength, 0.0..=1.0).text("等高線"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.tone_levels, 2.0..=16.0).text("階調数"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.ink_density, 0.0..=1.8).text("インク濃度"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.paper_texture, 0.0..=1.0).text("紙目"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
            let mut seed = params.seed as i32;
            changed |= ui
                .add(egui::Slider::new(&mut seed, 0..=9999).text("seed"))
                .changed();
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::NewspaperPrint(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "新聞紙") {
                    *params = NewspaperPrintParams {
                        cell_px: 9.0,
                        dot_gain: 0.05,
                        ink_bleed: 0.18,
                        paper_age: 0.38,
                        paper_texture: 0.34,
                        contrast: 0.22,
                        fade: 0.15,
                        strength: 0.82,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "古新聞") {
                    *params = NewspaperPrintParams {
                        cell_px: 11.0,
                        dot_gain: 0.03,
                        ink_bleed: 0.36,
                        paper_age: 0.82,
                        paper_texture: 0.62,
                        contrast: 0.08,
                        fade: 0.48,
                        strength: 0.88,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "粗い印刷") {
                    *params = NewspaperPrintParams {
                        cell_px: 16.0,
                        dot_gain: 0.16,
                        ink_bleed: 0.55,
                        paper_age: 0.52,
                        paper_texture: 0.45,
                        contrast: 0.45,
                        fade: 0.20,
                        strength: 0.95,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "淡い紙面") {
                    *params = NewspaperPrintParams {
                        cell_px: 12.0,
                        dot_gain: -0.07,
                        ink_bleed: 0.20,
                        paper_age: 0.60,
                        paper_texture: 0.40,
                        contrast: -0.10,
                        fade: 0.36,
                        strength: 0.65,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "元画像の明るさを粗い網点に置き換え、黄ばんだ紙色と紙目、少しにじんだインクで新聞・古印刷物風にします。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            changed |= ui
                .add(egui::Slider::new(&mut params.cell_px, 3.0..=96.0).text("網点セル(px)"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.dot_gain, -0.35..=0.45).text("ドット増減"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.ink_bleed, 0.0..=1.0).text("インクにじみ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.paper_age, 0.0..=1.0).text("紙の古さ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.paper_texture, 0.0..=1.0).text("紙目"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.contrast, -1.0..=1.0).text("印刷コントラスト"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.fade, 0.0..=1.0).text("退色"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
            let mut seed = params.seed as i32;
            changed |= ui
                .add(egui::Slider::new(&mut seed, 0..=9999).text("seed"))
                .changed();
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::Textureizer(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "紙目") {
                    *params = TextureizerParams {
                        mode: TextureizerMode::Paper,
                        scale_px: 9.0,
                        depth: 0.55,
                        contrast: 1.05,
                        warmth: 0.22,
                        strength: 0.60,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "キャンバス") {
                    *params = TextureizerParams {
                        mode: TextureizerMode::Canvas,
                        scale_px: 7.0,
                        depth: 0.60,
                        contrast: 1.20,
                        warmth: 0.10,
                        strength: 0.65,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "リネン") {
                    *params = TextureizerParams {
                        mode: TextureizerMode::Linen,
                        scale_px: 8.0,
                        depth: 0.50,
                        contrast: 1.15,
                        warmth: 0.16,
                        strength: 0.58,
                        seed: params.seed,
                    };
                    changed = true;
                }
                if preset_button(ui, "冷たい紙目") {
                    *params = TextureizerParams {
                        mode: TextureizerMode::Paper,
                        scale_px: 12.0,
                        depth: 0.42,
                        contrast: 0.90,
                        warmth: -0.22,
                        strength: 0.50,
                        seed: params.seed,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "手続き型の紙目や織り目をソフトライトで重ねます。フィルム粒子より大きな面の質感向けです。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            ui.horizontal_wrapped(|ui| {
                changed |= ui
                    .selectable_value(&mut params.mode, TextureizerMode::Paper, "紙目")
                    .changed();
                changed |= ui
                    .selectable_value(&mut params.mode, TextureizerMode::Canvas, "キャンバス")
                    .changed();
                changed |= ui
                    .selectable_value(&mut params.mode, TextureizerMode::Linen, "リネン")
                    .changed();
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.scale_px, 2.0..=96.0).text("スケール(px)"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.depth, 0.0..=1.0).text("凹凸"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.contrast, 0.0..=2.0).text("コントラスト"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.warmth, -1.0..=1.0).text("紙色"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強度"))
                .changed();
            let mut seed = params.seed as i32;
            changed |= ui
                .add(egui::Slider::new(&mut seed, 0..=9999).text("seed"))
                .changed();
            params.seed = seed.max(0) as u32;
        }
        LocalEffect::StarGlow(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "クロス弱") {
                    *params = StarGlowParams {
                        ray_count: 4,
                        rotation_degrees: 0.0,
                        threshold: 0.997,
                        length_px: 36.0,
                        strength: 0.45,
                    };
                    changed = true;
                }
                if preset_button(ui, "クロス強") {
                    *params = StarGlowParams {
                        ray_count: 4,
                        rotation_degrees: 0.0,
                        threshold: 0.993,
                        length_px: 72.0,
                        strength: 0.80,
                    };
                    changed = true;
                }
                if preset_button(ui, "X字") {
                    *params = StarGlowParams {
                        ray_count: 4,
                        rotation_degrees: 45.0,
                        threshold: 0.995,
                        length_px: 64.0,
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "6本") {
                    *params = StarGlowParams {
                        ray_count: 6,
                        rotation_degrees: 0.0,
                        threshold: 0.996,
                        length_px: 56.0,
                        strength: 0.70,
                    };
                    changed = true;
                }
                if preset_button(ui, "8本") {
                    *params = StarGlowParams {
                        ray_count: 8,
                        rotation_degrees: 0.0,
                        threshold: 0.997,
                        length_px: 56.0,
                        strength: 0.65,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new("明るい点を抽出し、レンズのクロス/スター光条風に伸ばします。")
                    .size(10.0)
                    .color(Color32::from_gray(170)),
            );
            let mut ray_count = params.ray_count as i32;
            changed |= ui
                .add(egui::Slider::new(&mut ray_count, 2..=12).text("光線本数"))
                .changed();
            if changed {
                let mut normalized = ray_count.clamp(2, 12) as u32;
                if normalized % 2 != 0 {
                    normalized += 1;
                }
                params.ray_count = normalized.clamp(2, 12);
            }
            changed |= ui
                .add(egui::Slider::new(&mut params.rotation_degrees, -180.0..=180.0).text("回転"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.threshold, 0.90..=0.9999)
                        .text("明部しきい値")
                        .fixed_decimals(4)
                        .smart_aim(false),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.length_px, 1.0..=240.0).text("光線長"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=3.0).text("強さ"))
                .changed();
        }
        LocalEffect::DiffractionStarburst(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "6羽スター") {
                    *params = DiffractionStarburstParams {
                        blade_count: 6,
                        rotation_degrees: 0.0,
                        threshold: 0.995,
                        length_px: 96.0,
                        width_px: 1.4,
                        halo_radius_px: 12.0,
                        chromatic_shift: 0.20,
                        strength: 0.80,
                    };
                    changed = true;
                }
                if preset_button(ui, "5羽きらめき") {
                    *params = DiffractionStarburstParams {
                        blade_count: 5,
                        rotation_degrees: -8.0,
                        threshold: 0.996,
                        length_px: 76.0,
                        width_px: 1.1,
                        halo_radius_px: 8.0,
                        chromatic_shift: 0.35,
                        strength: 0.72,
                    };
                    changed = true;
                }
                if preset_button(ui, "長い光条") {
                    *params = DiffractionStarburstParams {
                        blade_count: 8,
                        rotation_degrees: 12.0,
                        threshold: 0.993,
                        length_px: 150.0,
                        width_px: 1.8,
                        halo_radius_px: 18.0,
                        chromatic_shift: 0.30,
                        strength: 0.90,
                    };
                    changed = true;
                }
                if preset_button(ui, "点光源だけ") {
                    *params = DiffractionStarburstParams {
                        blade_count: 7,
                        rotation_degrees: 0.0,
                        threshold: 0.998,
                        length_px: 104.0,
                        width_px: 0.9,
                        halo_radius_px: 6.0,
                        chromatic_shift: 0.18,
                        strength: 0.85,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "明るい点から絞り羽根状の細い光条を伸ばします。奇数羽根では光条が倍になります。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let mut blade_count = params.blade_count as i32;
            changed |= ui
                .add(egui::Slider::new(&mut blade_count, 3..=12).text("絞り羽根数"))
                .changed();
            params.blade_count = blade_count.clamp(3, 12) as u32;
            changed |= ui
                .add(egui::Slider::new(&mut params.rotation_degrees, -180.0..=180.0).text("回転"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.threshold, 0.90..=0.9999)
                        .text("明部しきい値")
                        .fixed_decimals(4)
                        .smart_aim(false),
                )
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.length_px, 1.0..=360.0).text("光条長"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.width_px, 0.4..=12.0).text("光条幅"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.halo_radius_px, 0.0..=96.0).text("点光源ハロー"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.chromatic_shift, 0.0..=1.0).text("色ズレ"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=3.0).text("強さ"))
                .changed();
        }
        LocalEffect::EdgeSmooth(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "背景なじませ") {
                    *params = EdgeSmoothParams {
                        radius_px: 3.0,
                        strength: 0.35,
                        edge_threshold: 28.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "強め") {
                    *params = EdgeSmoothParams {
                        radius_px: 5.0,
                        strength: 0.55,
                        edge_threshold: 45.0,
                    };
                    changed = true;
                }
            });
            changed |= ui
                .add(egui::Slider::new(&mut params.radius_px, 0.0..=8.0).text("半径"))
                .changed();
            changed |= ui
                .add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"))
                .changed();
            changed |= ui
                .add(
                    egui::Slider::new(&mut params.edge_threshold, 1.0..=120.0).text("境界しきい値"),
                )
                .changed();
        }
        LocalEffect::Despeckle(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "点ゴミ") {
                    *params = DespeckleParams {
                        radius_px: 1.0,
                        threshold: 42.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "スキャン補修") {
                    *params = DespeckleParams {
                        radius_px: 2.0,
                        threshold: 34.0,
                        strength: 0.75,
                    };
                    changed = true;
                }
                if preset_button(ui, "控えめ") {
                    *params = DespeckleParams {
                        radius_px: 1.0,
                        threshold: 70.0,
                        strength: 0.55,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "周囲から大きく外れた孤立点だけを中央値へ寄せます。通常のメディアンより線や面を残しやすい点ゴミ除去です。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 1.0..=4.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip(
                "周囲を調べる範囲です。1px は白点・黒点、2px 以上は小さなゴミ向けです。",
            );
            let threshold =
                ui.add(egui::Slider::new(&mut params.threshold, 1.0..=160.0).text("検出しきい値"));
            changed |= threshold.changed();
            threshold.lab_hover_tip(
                "中心画素が周囲の中央値からどれだけ外れたら補修するかです。小さいほど多く補修します。",
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から補修後の色へどれだけ近づけるかです。");
        }
        LocalEffect::Median(params) => {
            ui.label(egui::RichText::new("プリセット").color(Color32::from_gray(190)));
            ui.horizontal_wrapped(|ui| {
                if preset_button(ui, "点ノイズ") {
                    *params = MedianParams {
                        radius_px: 1.0,
                        strength: 1.0,
                    };
                    changed = true;
                }
                if preset_button(ui, "弱く") {
                    *params = MedianParams {
                        radius_px: 1.0,
                        strength: 0.45,
                    };
                    changed = true;
                }
                if preset_button(ui, "強め") {
                    *params = MedianParams {
                        radius_px: 2.0,
                        strength: 0.85,
                    };
                    changed = true;
                }
            });
            ui.label(
                egui::RichText::new(
                    "周囲の中央値に置き換えることで、孤立した白点・黒点や細かいゴミを落とします。線や細部も丸まりやすいので小さめの半径から試してください。",
                )
                .size(10.0)
                .color(Color32::from_gray(170)),
            );
            let radius = ui.add(egui::Slider::new(&mut params.radius_px, 0.0..=8.0).text("半径"));
            changed |= radius.changed();
            radius.lab_hover_tip(
                "中央値を取る範囲です。1pxは点ノイズ除去向け、大きい値は細部も消えやすくなります。",
            );
            let strength = ui.add(egui::Slider::new(&mut params.strength, 0.0..=1.0).text("強さ"));
            changed |= strength.changed();
            strength.lab_hover_tip("元画像から中央値処理後の色へどれだけ近づけるかです。");
        }
    }
    EffectParamResponse {
        changed,
        load_cube_lut,
        start_selective_color_pick,
        cancel_selective_color_pick,
        start_rgb_pick,
        cancel_rgb_pick,
        set_effect_position_handles_visible,
        copy_effect,
        paste_effect,
        reset_effect,
    }
}

use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock};

pub const USER_TEXT_FAMILY_NAME: &str = "miv-user-text";
/// 動画 / 音楽 HUD の固定サイズ control label 用。任意 UI フォントの字幅・字面に
/// 影響されないよう、従来の既定日本語フォントを無補正で使う。
pub const HUD_TEXT_FAMILY_NAME: &str = "miv-hud-text";
/// ツールバー専用フォント family。Yu Gothic/Meiryo の glyph が egui の line box 内で
/// 視覚的に上寄りに見える問題 (Codex 助言 2026-05) を `FontTweak.y_offset` で補正する。
/// ComboBox / Button などのツールバー widget 内 text を 1px 下に寄せて、widget の
/// 縦中央に視覚的に揃える。
pub const TOOLBAR_TEXT_FAMILY_NAME: &str = "miv-toolbar-text";
/// ツールバー用日本語フォントの y_offset (line box 内で文字を下に寄せる、ピクセル単位)。
/// 正の値で下方向。実機調整 (Yu Gothic Medium, 2026-05):
///   1.5 → まだ上寄り
///   3.5 → 中央寄り (現在採用値)
/// 環境やフォントによってさらに調整が必要なら 1px 単位で動かす。
const TOOLBAR_Y_OFFSET: f32 = 3.5;
const DERIVED_Y_OFFSET_CLAMP: f32 = 0.24;
const MAX_UI_FONT_FILE_BYTES: u64 = 256 * 1024 * 1024;

const JAPANESE_FONT_PATHS: &[&str] = &[
    r"C:\Windows\Fonts\YuGothM.ttc",
    r"C:\Windows\Fonts\meiryo.ttc",
    r"C:\Windows\Fonts\msgothic.ttc",
];

struct FallbackFont {
    name: &'static str,
    path: &'static str,
    scale: f32,
    y_offset: FallbackYOffset,
}

#[derive(Clone, Copy)]
enum FallbackYOffset {
    Fixed(f32),
    AlignGlyphCenter {
        samples: &'static [char],
        fallback: f32,
    },
    AlignRowVisualCenter {
        samples: &'static [char],
        fallback: f32,
    },
}

#[derive(Clone, Copy)]
struct FontAlignmentTarget {
    ascent: f32,
    center_y: f32,
    row_height: f32,
}

#[derive(Clone, Copy)]
struct FontRowMetrics {
    ascent: f32,
    row_height: f32,
}

const USER_TEXT_FALLBACKS: &[FallbackFont] = &[
    // Text-presentation symbols such as ✉ and ⋈ should follow browser-like
    // text fallback instead of being captured by emoji/math fonts.
    FallbackFont {
        name: "text_symbols",
        path: r"C:\Windows\Fonts\meiryo.ttc",
        scale: 1.0,
        // Snapshot-locked to align text symbols with nearby Latin lowercase
        // and separator strokes; this is a visual text-presentation target,
        // not the Yu Gothic ideographic center used for emoji.
        y_offset: FallbackYOffset::Fixed(-0.20),
    },
    // Mathematical alphanumeric symbols such as 𝓈𝒸𝓇𝑒𝒶𝓂 are not covered by
    // Yu Gothic or Meiryo. Keep this before emoji so script letters stay texty.
    FallbackFont {
        name: "math",
        path: r"C:\Windows\Fonts\cambria.ttc",
        scale: 0.98,
        y_offset: FallbackYOffset::AlignGlyphCenter {
            samples: &['𝓈', '𝒸', '𝓇', '𝑒', '𝒶', '𝓂'],
            fallback: -0.12,
        },
    },
    FallbackFont {
        name: "emoji",
        path: r"C:\Windows\Fonts\seguiemj.ttf",
        scale: 0.90,
        // Target: leading emoji in metadata lines should visually sit on the
        // same center line as Japanese body text, not below it. Compute the
        // tweak from real glyph bounds to avoid hand-tuning magic numbers.
        y_offset: FallbackYOffset::AlignGlyphCenter {
            samples: &['🐾', '🧠', '🍧', '💗'],
            fallback: -0.12,
        },
    },
    FallbackFont {
        name: "historic",
        path: r"C:\Windows\Fonts\seguihis.ttf",
        scale: 0.96,
        y_offset: FallbackYOffset::Fixed(0.04),
    },
    FallbackFont {
        name: "symbols",
        path: r"C:\Windows\Fonts\seguisym.ttf",
        scale: 0.96,
        y_offset: FallbackYOffset::Fixed(0.05),
    },
    // CJK ideographs / Hangul that the Japanese primary font (Yu Gothic) lacks.
    // Sidecar metadata and other user-derived text can be Simplified Chinese,
    // Traditional Chinese, or Korean; without these the glyphs render as tofu
    // (looks like mojibake but is a coverage gap — the bytes are read as correct
    // UTF-8). Placed AFTER the symbol/emoji fallbacks so existing symbol routing
    // is unchanged. Han code points shared with Japanese resolve to "japanese"
    // first (kept above), so only Chinese/Korean-only glyphs fall through here.
    //
    // Keep full-em CJK fallbacks visually centered with Yu Gothic. Fixed
    // offsets are too brittle here: a Chinese sentence can mix shared Han
    // glyphs from "japanese" with simplified/traditional-only glyphs from the
    // fallback, making even a 1-2 px mismatch obvious within one word.
    // Simplified Chinese (Microsoft YaHei). Comes before Traditional so that
    // code points shared by both render with simplified shapes.
    FallbackFont {
        name: "cjk_sc",
        path: r"C:\Windows\Fonts\msyh.ttc",
        scale: 1.0,
        y_offset: FallbackYOffset::AlignRowVisualCenter {
            samples: &['这', '哪', '衣', '约', '轮', '苏', '恶', '觉'],
            fallback: 0.0,
        },
    },
    // Traditional Chinese (Microsoft JhengHei).
    FallbackFont {
        name: "cjk_tc",
        path: r"C:\Windows\Fonts\msjh.ttc",
        scale: 1.0,
        y_offset: FallbackYOffset::AlignRowVisualCenter {
            samples: &['這', '哪', '衣', '約', '輪', '蘇', '惡', '覺'],
            fallback: 0.0,
        },
    },
    // Korean (Malgun Gothic). Covers Hangul syllables absent from all the above.
    FallbackFont {
        name: "korean",
        path: r"C:\Windows\Fonts\malgun.ttf",
        scale: 1.0,
        y_offset: FallbackYOffset::AlignRowVisualCenter {
            samples: &['한', '글', '가', '나', '다', '라'],
            fallback: 0.0,
        },
    },
];

pub fn user_text_font(size: f32) -> egui::FontId {
    egui::FontId::new(
        size,
        egui::FontFamily::Name(Arc::<str>::from(USER_TEXT_FAMILY_NAME)),
    )
}

pub fn hud_text_font(size: f32) -> egui::FontId {
    egui::FontId::new(
        size,
        egui::FontFamily::Name(Arc::<str>::from(HUD_TEXT_FAMILY_NAME)),
    )
}

pub fn configure_fonts(ctx: &egui::Context) {
    configure_fonts_with_settings(ctx, &crate::settings::UiFontSettings::default());
}

pub fn configure_fonts_with_settings(
    ctx: &egui::Context,
    settings: &crate::settings::UiFontSettings,
) {
    ctx.set_fonts(base_font_definitions_cached(settings));
}

/// 設定画面のワーカーからフォント定義を先に構築してキャッシュする。
/// `Context::set_fonts` は UI スレッドで行うが、フォントファイルの読み込みと
/// メトリクス計測はここで済ませることで、適用時のフレーム停止を避ける。
pub fn prepare_fonts(settings: &crate::settings::UiFontSettings) {
    let _ = base_font_definitions_cached(settings);
}

fn font_settings_cache_key(settings: &crate::settings::UiFontSettings) -> String {
    let mut normalized = settings.clone();
    normalized.sanitize();
    match &normalized.selection {
        crate::settings::UiFontSelection::Face {
            path, face_index, ..
        } => format!(
            "face:{}:{face_index}:{}",
            path.to_string_lossy().to_lowercase(),
            normalized.vertical_adjust.to_bits()
        ),
        crate::settings::UiFontSelection::Default | crate::settings::UiFontSelection::Unknown => {
            format!("default:{}", normalized.vertical_adjust.to_bits())
        }
    }
}

fn base_font_definitions_cached(
    settings: &crate::settings::UiFontSettings,
) -> egui::FontDefinitions {
    static BASES: OnceLock<Mutex<HashMap<String, egui::FontDefinitions>>> = OnceLock::new();
    let cache = BASES.get_or_init(|| Mutex::new(HashMap::new()));
    let key = font_settings_cache_key(settings);
    if let Some(fonts) = cache.lock().ok().and_then(|cache| cache.get(&key).cloned()) {
        return fonts;
    }
    let fonts = mimageviewer_font_definitions_for(settings);
    if let Ok(mut cache) = cache.lock() {
        // フォント定義は CJK fallback を含みサイズが大きい。微調整 slider の値ごとに
        // 永続保持すると短時間で数百 MiB へ膨らむため、直近 1 設定だけを保持する。
        // 次の設定変更はこの直近値を使い、現在 Context が使う旧定義は egui 側の Arc が
        // 必要な間だけ保持する。
        cache.clear();
        cache.insert(key, fonts.clone());
    }
    fonts
}

fn mimageviewer_font_definitions_for(
    settings: &crate::settings::UiFontSettings,
) -> egui::FontDefinitions {
    let mut fonts = egui::FontDefinitions::default();
    install_mimageviewer_fonts_with_settings(&mut fonts, settings);
    fonts
}

pub fn install_mimageviewer_fonts(fonts: &mut egui::FontDefinitions) {
    install_mimageviewer_fonts_with_settings(fonts, &crate::settings::UiFontSettings::default());
}

pub fn install_mimageviewer_fonts_with_settings(
    fonts: &mut egui::FontDefinitions,
    settings: &crate::settings::UiFontSettings,
) {
    let mut settings = settings.clone();
    settings.sanitize();
    let base_proportional = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let primary = install_primary_fonts(fonts, &settings);
    let primary_names = primary.normal_names;
    let alignment_target = primary.alignment_target;

    let user_text_fallback_names = USER_TEXT_FALLBACKS
        .iter()
        .filter_map(|fallback| {
            install_fallback_font(fonts, fallback, alignment_target)
                .then(|| fallback.name.to_owned())
        })
        .collect::<Vec<_>>();

    for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
        let family_fonts = fonts.families.entry(family).or_default();
        for name in primary_names.iter().rev() {
            remove_font_name(family_fonts, name);
            family_fonts.insert(0, name.clone());
        }
        for name in &user_text_fallback_names {
            remove_font_name(family_fonts, name);
            family_fonts.push(name.clone());
        }
    }

    let mut user_text_fonts = Vec::new();
    extend_unique(&mut user_text_fonts, primary_names.iter().cloned());
    extend_unique(
        &mut user_text_fonts,
        user_text_fallback_names.iter().cloned(),
    );
    extend_unique(&mut user_text_fonts, base_proportional.clone());
    fonts.families.insert(
        egui::FontFamily::Name(Arc::<str>::from(USER_TEXT_FAMILY_NAME)),
        user_text_fonts,
    );

    // HUD の `Norm` / 再生速度 / 時刻等は固定高・固定幅で配置され、従来の既定
    // フォントに合わせた Y 基準を持つ。任意 UI フォントで形や位置を変えない。
    let mut hud_text_fonts = primary.hud_names;
    extend_unique(&mut hud_text_fonts, base_proportional.clone());
    fonts.families.insert(
        egui::FontFamily::Name(Arc::<str>::from(HUD_TEXT_FAMILY_NAME)),
        hud_text_fonts,
    );

    // ツールバー専用 family: 選択した主フォントを実メトリクスで自動補正し、
    // ユーザー微調整を加えた variant を先頭へ置く。選択 face が持たない glyph 用の
    // `japanese_toolbar` にも同じ shift を載せる。記号・絵文字等の低頻度 fallback は
    // 従来同様の相対補正を使う（大きな CJK font data の toolbar 複製を避けるため）。
    //
    // ⚠ Codex P2 (2026-05): Yu Gothic / Meiryo / MS Gothic が見つからない環境では
    // japanese_toolbar が登録されないが、family 自体は ui_main.rs から常に参照される
    // ため、family を登録しないと egui が「family is not bound to any fonts」で panic
    // する。japanese_toolbar が無いときは通常の primary fallback (= "japanese" など)
    // にフォールバックさせて family は **常に**登録する。
    let mut toolbar_fonts = primary.toolbar_names;
    extend_unique(&mut toolbar_fonts, user_text_fallback_names.iter().cloned());
    if toolbar_fonts.is_empty() {
        // UI フォントが 1 つも読めない環境でも named family 自体は必ず登録する。
        extend_unique(&mut toolbar_fonts, primary_names.iter().cloned());
    }
    extend_unique(&mut toolbar_fonts, base_proportional);
    fonts.families.insert(
        egui::FontFamily::Name(Arc::<str>::from(TOOLBAR_TEXT_FAMILY_NAME)),
        toolbar_fonts,
    );
}

struct PrimaryFonts {
    normal_names: Vec<String>,
    toolbar_names: Vec<String>,
    hud_names: Vec<String>,
    alignment_target: Option<FontAlignmentTarget>,
}

fn load_default_japanese_font() -> Option<(Vec<u8>, u32)> {
    for path in JAPANESE_FONT_PATHS {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        if ttf_parser::Face::parse(&data, 0).is_ok() {
            return Some((data, 0));
        }
    }
    None
}

fn load_selected_font(path: &std::path::Path) -> std::io::Result<Vec<u8>> {
    let metadata = std::fs::metadata(path)?;
    if metadata.len() > MAX_UI_FONT_FILE_BYTES {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "UI font exceeds 256 MiB",
        ));
    }
    std::fs::read(path)
}

fn font_data(data: Vec<u8>, index: u32, tweak: egui::FontTweak) -> egui::FontData {
    let mut font = egui::FontData::from_owned(data);
    font.index = index;
    font.tweak = tweak;
    font
}

fn selected_face_is_eligible(data: &[u8], index: u32) -> bool {
    ttf_parser::Face::parse(data, index).is_ok_and(|face| {
        !face.is_italic()
            && !face.is_oblique()
            && ['今', 'あ']
                .into_iter()
                .all(|ch| face.glyph_index(ch).is_some())
    })
}

fn alignment_target(data: &[u8], index: u32) -> Option<FontAlignmentTarget> {
    const SAMPLES: &[char] = &['今', 'あ', 'A', 'a', '0'];
    let metrics = font_row_metrics_at(data, index)?;
    let center_y = font_center_y_for_samples_at(data, index, SAMPLES)
        .or_else(|| font_metric_center_y_at(data, index))?;
    Some(FontAlignmentTarget {
        ascent: metrics.ascent,
        center_y,
        row_height: metrics.row_height,
    })
}

fn visual_centering_factor(target: FontAlignmentTarget) -> f32 {
    let visual_center_from_top = target.ascent - target.center_y;
    (0.5 * target.row_height - visual_center_from_top)
        .clamp(-DERIVED_Y_OFFSET_CLAMP, DERIVED_Y_OFFSET_CLAMP)
}

fn row_alignment_factor(
    data: &[u8],
    index: u32,
    samples: &[char],
    target: FontAlignmentTarget,
) -> f32 {
    let Some(metrics) = font_row_metrics_at(data, index) else {
        return 0.0;
    };
    let Some(center) = font_center_y_for_samples_at(data, index, samples)
        .or_else(|| font_metric_center_y_at(data, index))
    else {
        return 0.0;
    };
    let target_visual_center = target.ascent - target.center_y;
    let fallback_visual_center =
        0.5 * target.row_height + metrics.ascent - 0.5 * metrics.row_height - center;
    (target_visual_center - fallback_visual_center)
        .clamp(-DERIVED_Y_OFFSET_CLAMP, DERIVED_Y_OFFSET_CLAMP)
}

fn clone_font_with_toolbar_shift(
    fonts: &mut egui::FontDefinitions,
    source_name: &str,
    target_name: &str,
    shift_factor: f32,
    shift_points: f32,
) -> bool {
    let Some(source) = fonts.font_data.get(source_name).cloned() else {
        return false;
    };
    let mut data = (*source).clone();
    let scale = data.tweak.scale.max(0.01);
    data.tweak.y_offset_factor += shift_factor / scale;
    data.tweak.y_offset += shift_points;
    fonts
        .font_data
        .insert(target_name.to_owned(), Arc::new(data));
    true
}

fn install_primary_fonts(
    fonts: &mut egui::FontDefinitions,
    settings: &crate::settings::UiFontSettings,
) -> PrimaryFonts {
    let default_face = load_default_japanese_font();
    let default_target = default_face
        .as_ref()
        .and_then(|(data, index)| alignment_target(data, *index));

    let custom_face = match &settings.selection {
        crate::settings::UiFontSelection::Face {
            path, face_index, ..
        } => match load_selected_font(path) {
            Ok(data) if selected_face_is_eligible(&data, *face_index) => Some((data, *face_index)),
            Ok(_) => {
                crate::logger::log(format!(
                    "ui_fonts: selected face is not an eligible Japanese upright font path={} index={face_index}; using default",
                    path.display()
                ));
                None
            }
            Err(err) => {
                crate::logger::log(format!(
                    "ui_fonts: selected font unavailable path={} index={face_index}: {err}; using default",
                    path.display()
                ));
                None
            }
        },
        crate::settings::UiFontSelection::Default | crate::settings::UiFontSelection::Unknown => {
            None
        }
    };

    let custom_selected = custom_face.is_some();
    let selected_target = custom_face
        .as_ref()
        .and_then(|(data, index)| alignment_target(data, *index))
        .or(default_target);
    let default_centering = default_target.map(visual_centering_factor).unwrap_or(0.0);
    let selected_centering = selected_target
        .map(visual_centering_factor)
        .unwrap_or(default_centering);
    let toolbar_shift_factor = if custom_selected {
        selected_centering - default_centering
    } else {
        0.0
    };
    let toolbar_shift_points = TOOLBAR_Y_OFFSET + settings.vertical_adjust;

    let mut normal_names = Vec::new();
    let mut toolbar_names = Vec::new();
    let mut hud_names = Vec::new();
    if let Some((data, index)) = custom_face {
        fonts.font_data.insert(
            "ui_primary".to_owned(),
            Arc::new(font_data(data.clone(), index, egui::FontTweak::default())),
        );
        fonts.font_data.insert(
            "ui_primary_toolbar".to_owned(),
            Arc::new(font_data(
                data,
                index,
                egui::FontTweak {
                    y_offset_factor: toolbar_shift_factor,
                    y_offset: toolbar_shift_points,
                    ..Default::default()
                },
            )),
        );
        normal_names.push("ui_primary".to_owned());
        toolbar_names.push("ui_primary_toolbar".to_owned());

        // 選択 face が持たない日本語 glyph も UI を豆腐化させない。既定日本語
        // フォントを次順位に置き、その見た目中心を選択フォントへ合わせる。
        if let Some((data, index)) = default_face {
            let default_font = Arc::new(font_data(data, index, egui::FontTweak::default()));
            fonts
                .font_data
                .insert("ui_hud_default".to_owned(), default_font.clone());
            hud_names.push("ui_hud_default".to_owned());
            let relative = selected_target
                .map(|target| {
                    row_alignment_factor(default_font.font.as_ref(), index, &['今', 'あ'], target)
                })
                .unwrap_or(0.0);
            let mut japanese = (*default_font).clone();
            japanese.tweak.y_offset_factor = relative;
            fonts
                .font_data
                .insert("japanese".to_owned(), Arc::new(japanese));
            normal_names.push("japanese".to_owned());
            if clone_font_with_toolbar_shift(
                fonts,
                "japanese",
                "japanese_toolbar",
                toolbar_shift_factor,
                toolbar_shift_points,
            ) {
                toolbar_names.push("japanese_toolbar".to_owned());
            }
        }
    } else if let Some((data, index)) = default_face {
        let default_font = Arc::new(font_data(data, index, egui::FontTweak::default()));
        fonts
            .font_data
            .insert("ui_hud_default".to_owned(), default_font.clone());
        fonts.font_data.insert("japanese_toolbar".to_owned(), {
            let mut toolbar = (*default_font).clone();
            toolbar.tweak.y_offset = toolbar_shift_points;
            Arc::new(toolbar)
        });
        fonts.font_data.insert("japanese".to_owned(), default_font);
        normal_names.push("japanese".to_owned());
        toolbar_names.push("japanese_toolbar".to_owned());
        hud_names.push("ui_hud_default".to_owned());
    }

    PrimaryFonts {
        normal_names,
        toolbar_names,
        hud_names,
        alignment_target: selected_target,
    }
}

fn install_fallback_font(
    fonts: &mut egui::FontDefinitions,
    fallback: &FallbackFont,
    alignment_target: Option<FontAlignmentTarget>,
) -> bool {
    let Ok(data) = std::fs::read(fallback.path) else {
        return false;
    };
    let y_offset_factor = fallback_y_offset_factor(fallback, &data, alignment_target);
    fonts.font_data.insert(
        fallback.name.to_owned(),
        Arc::new(egui::FontData::from_owned(data).tweak(egui::FontTweak {
            scale: fallback.scale,
            y_offset_factor,
            ..Default::default()
        })),
    );
    true
}

fn fallback_y_offset_factor(
    fallback: &FallbackFont,
    data: &[u8],
    alignment_target: Option<FontAlignmentTarget>,
) -> f32 {
    match fallback.y_offset {
        FallbackYOffset::Fixed(value) => {
            crate::logger::log(format!(
                "ui_fonts: {} alignment fixed scale={:.3} factor={:.4}",
                fallback.name, fallback.scale, value
            ));
            value
        }
        FallbackYOffset::AlignGlyphCenter {
            samples,
            fallback: fallback_offset,
        } => alignment_target
            .and_then(|target| {
                let fallback_center = font_center_y_for_samples(data, samples)?;
                let scale = fallback.scale.max(0.01);
                // egui 0.33 applies y_offset_factor after multiplying it by
                // FontTweak::scale. Font coordinates are positive-up, so aligning
                // normalized glyph centers yields:
                //   factor * size * scale - fallback_center * size * scale
                //     == -target_center * size
                let factor = (fallback_center - target.center_y / scale)
                    .clamp(-DERIVED_Y_OFFSET_CLAMP, DERIVED_Y_OFFSET_CLAMP);
                crate::logger::log(format!(
                    "ui_fonts: {} alignment derived target_center={:.4} fallback_center={:.4} scale={:.3} factor={:.4} samples={}",
                    fallback.name,
                    target.center_y,
                    fallback_center,
                    scale,
                    factor,
                    samples.iter().collect::<String>()
                ));
                Some(factor)
            })
            .unwrap_or(fallback_offset),
        FallbackYOffset::AlignRowVisualCenter {
            samples,
            fallback: fallback_offset,
        } => alignment_target
            .and_then(|target| {
                let fallback_metrics = font_row_metrics(data)?;
                let fallback_center = font_center_y_for_samples(data, samples)?;
                let scale = fallback.scale.max(0.01);
                // egui lays out fallback glyphs with the fallback font's
                // ascent, then centers the fallback row height inside the
                // primary row height:
                //   baseline = A_f + 0.5 * (H_primary - H_f)
                // ttf-parser glyph centers are positive-up from baseline, so
                // visual center from the row top is `baseline - center`.
                // Convert the resulting point offset back to FontTweak's
                // y_offset_factor, which egui multiplies by `size * scale`.
                let target_visual_center = target.ascent - target.center_y;
                let fallback_visual_center = 0.5 * target.row_height
                    + scale
                        * (fallback_metrics.ascent
                            - 0.5 * fallback_metrics.row_height
                            - fallback_center);
                let factor = ((target_visual_center - fallback_visual_center) / scale)
                    .clamp(-DERIVED_Y_OFFSET_CLAMP, DERIVED_Y_OFFSET_CLAMP);
                crate::logger::log(format!(
                    "ui_fonts: {} row alignment target_center={:.4} fallback_center={:.4} target_ascent={:.4} fallback_ascent={:.4} target_row={:.4} fallback_row={:.4} scale={:.3} factor={:.4} samples={}",
                    fallback.name,
                    target.center_y,
                    fallback_center,
                    target.ascent,
                    fallback_metrics.ascent,
                    target.row_height,
                    fallback_metrics.row_height,
                    scale,
                    factor,
                    samples.iter().collect::<String>()
                ));
                Some(factor)
            })
            .unwrap_or(fallback_offset),
    }
}

fn font_center_y_for_samples(data: &[u8], samples: &[char]) -> Option<f32> {
    font_center_y_for_samples_at(data, 0, samples)
}

fn font_center_y_for_samples_at(data: &[u8], index: u32, samples: &[char]) -> Option<f32> {
    let face = ttf_parser::Face::parse(data, index).ok()?;
    let mut centers = samples
        .iter()
        .filter_map(|sample| glyph_center_y(&face, *sample))
        .collect::<Vec<_>>();
    if centers.is_empty() {
        return None;
    }
    centers.sort_by(|a, b| a.total_cmp(b));
    let mid = centers.len() / 2;
    if centers.len() % 2 == 0 {
        Some((centers[mid - 1] + centers[mid]) * 0.5)
    } else {
        Some(centers[mid])
    }
}

#[cfg(all(test, windows))]
fn font_metric_center_y(data: &[u8]) -> Option<f32> {
    font_metric_center_y_at(data, 0)
}

fn font_metric_center_y_at(data: &[u8], index: u32) -> Option<f32> {
    let face = ttf_parser::Face::parse(data, index).ok()?;
    let units_per_em = f32::from(face.units_per_em());
    let ascender = face.ascender() as f32;
    let descender = face.descender() as f32;
    Some((ascender + descender) * 0.5 / units_per_em)
}

fn font_row_metrics(data: &[u8]) -> Option<FontRowMetrics> {
    font_row_metrics_at(data, 0)
}

fn font_row_metrics_at(data: &[u8], index: u32) -> Option<FontRowMetrics> {
    let face = ttf_parser::Face::parse(data, index).ok()?;
    let units_per_em = f32::from(face.units_per_em());
    let ascent = face.ascender() as f32 / units_per_em;
    let descent = face.descender() as f32 / units_per_em;
    let line_gap = face.line_gap() as f32 / units_per_em;
    Some(FontRowMetrics {
        ascent,
        row_height: ascent - descent + line_gap,
    })
}

fn glyph_center_y(face: &ttf_parser::Face<'_>, sample: char) -> Option<f32> {
    let glyph_id = face.glyph_index(sample)?;
    let units_per_em = f32::from(face.units_per_em());
    if let Some(rect) = face.glyph_bounding_box(glyph_id) {
        return Some((f32::from(rect.y_min) + f32::from(rect.y_max)) * 0.5 / units_per_em);
    }
    let image = face.glyph_raster_image(glyph_id, face.units_per_em())?;
    Some((f32::from(image.y) + f32::from(image.height) * 0.5) / f32::from(image.pixels_per_em))
}

fn remove_font_name(fonts: &mut Vec<String>, name: &str) {
    fonts.retain(|existing| existing != name);
}

fn extend_unique(fonts: &mut Vec<String>, names: impl IntoIterator<Item = String>) {
    for name in names {
        if !fonts.iter().any(|existing| existing == &name) {
            fonts.push(name);
        }
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    struct RecommendedWindowsFont {
        label: &'static str,
        family: &'static str,
        path: &'static str,
        typographic_points: f32,
    }

    const RECOMMENDED_WINDOWS_FONTS: &[RecommendedWindowsFont] = &[
        RecommendedWindowsFont {
            label: "BIZ UDPGothic",
            family: "BIZ UDPGothic",
            path: r"C:\Windows\Fonts\BIZ-UDGothicR.ttc",
            typographic_points: 9.0,
        },
        RecommendedWindowsFont {
            label: "Meiryo",
            family: "Meiryo",
            path: r"C:\Windows\Fonts\meiryo.ttc",
            typographic_points: 10.0,
        },
        RecommendedWindowsFont {
            label: "Meiryo UI",
            family: "Meiryo UI",
            path: r"C:\Windows\Fonts\meiryo.ttc",
            typographic_points: 10.0,
        },
    ];

    fn collection_face_index(path: &str, family: &str) -> u32 {
        let mut db = fontdb::Database::new();
        db.load_font_file(path)
            .unwrap_or_else(|err| panic!("{family} should load from {path}: {err}"));
        db.faces()
            .find(|face| {
                face.families
                    .iter()
                    .any(|(name, _)| name.eq_ignore_ascii_case(family))
            })
            .map(|face| face.index)
            .unwrap_or_else(|| panic!("{path} should contain the {family} face"))
    }

    fn japanese_alignment_target(data: &[u8]) -> FontAlignmentTarget {
        let metrics = font_row_metrics(data).expect("Japanese row metrics should be measurable");
        let center_y = font_center_y_for_samples(data, &['今', 'あ'])
            .or_else(|| font_metric_center_y(data))
            .expect("Japanese glyph center should be measurable");
        FontAlignmentTarget {
            ascent: metrics.ascent,
            center_y,
            row_height: metrics.row_height,
        }
    }

    #[test]
    fn emoji_offset_is_derived_from_real_glyph_centers() {
        let japanese = JAPANESE_FONT_PATHS
            .iter()
            .find_map(|path| std::fs::read(path).ok())
            .expect("Windows Japanese font should be available");
        let emoji = std::fs::read(r"C:\Windows\Fonts\seguiemj.ttf")
            .expect("Segoe UI Emoji should be available");
        let body_text = japanese_alignment_target(&japanese);
        let emoji_font = USER_TEXT_FALLBACKS
            .iter()
            .find(|font| font.name == "emoji")
            .expect("emoji fallback definition should exist");

        let samples = ['🐾', '🧠', '🍧', '💗'];
        let fallback_center =
            font_center_y_for_samples(&emoji, &samples).expect("emoji center should be measurable");
        let offset = fallback_y_offset_factor(emoji_font, &emoji, Some(body_text));
        let expected = (fallback_center - body_text.center_y / emoji_font.scale)
            .clamp(-DERIVED_Y_OFFSET_CLAMP, DERIVED_Y_OFFSET_CLAMP);
        assert!(
            (offset - expected).abs() < f32::EPSILON,
            "emoji offset should follow egui's scaled y_offset formula: {offset} vs {expected}",
        );
        assert!(
            (-0.24..=0.0).contains(&offset),
            "emoji should be nudged upward from measured glyph centers, got {offset}",
        );
    }

    #[test]
    fn math_offset_is_derived_from_script_glyph_centers() {
        let japanese = JAPANESE_FONT_PATHS
            .iter()
            .find_map(|path| std::fs::read(path).ok())
            .expect("Windows Japanese font should be available");
        let math =
            std::fs::read(r"C:\Windows\Fonts\cambria.ttc").expect("Cambria should be available");
        let body_text = japanese_alignment_target(&japanese);
        let math_font = USER_TEXT_FALLBACKS
            .iter()
            .find(|font| font.name == "math")
            .expect("math fallback definition should exist");
        let samples = ['𝓈', '𝒸', '𝓇', '𝑒', '𝒶', '𝓂'];
        let fallback_center = font_center_y_for_samples(&math, &samples)
            .expect("math script center should be measurable");
        let offset = fallback_y_offset_factor(math_font, &math, Some(body_text));
        let expected = (fallback_center - body_text.center_y / math_font.scale)
            .clamp(-DERIVED_Y_OFFSET_CLAMP, DERIVED_Y_OFFSET_CLAMP);
        assert!(
            (offset - expected).abs() < f32::EPSILON,
            "math offset should follow egui's scaled y_offset formula: {offset} vs {expected}",
        );
        assert!(
            offset < 0.0,
            "math script should be nudged upward to sit with Yu Gothic punctuation, got {offset}",
        );
    }

    #[test]
    fn text_symbol_fallback_has_expected_symbols() {
        let data =
            std::fs::read(r"C:\Windows\Fonts\meiryo.ttc").expect("Meiryo should be available");
        let face = ttf_parser::Face::parse(&data, 0).expect("Meiryo should parse");
        for symbol in ['✉', '⋈', '★', '♪', '※', '☎'] {
            assert!(
                face.glyph_index(symbol).is_some(),
                "Meiryo should cover text symbol {symbol}"
            );
        }
    }

    #[test]
    fn user_text_routes_common_metadata_symbols() {
        let mut fonts = egui::FontDefinitions::default();
        install_mimageviewer_fonts(&mut fonts);
        let family = fonts
            .families
            .get(&egui::FontFamily::Name(Arc::<str>::from(
                USER_TEXT_FAMILY_NAME,
            )))
            .expect("user text family should be registered");
        for (ch, expected_font) in [
            ('…', "japanese"),
            ('✉', "text_symbols"),
            ('⋈', "text_symbols"),
            ('★', "japanese"),
            ('♪', "japanese"),
            ('※', "japanese"),
            ('☎', "japanese"),
            ('𝓈', "math"),
            ('💗', "emoji"),
        ] {
            let actual = family.iter().find(|font_name| {
                fonts
                    .font_data
                    .get(*font_name)
                    .and_then(|font| ttf_parser::Face::parse(font.font.as_ref(), font.index).ok())
                    .and_then(|face| face.glyph_index(ch))
                    .is_some()
            });
            println!("ui_fonts route {ch:?} -> {actual:?}");
            assert_eq!(
                actual.map(String::as_str),
                Some(expected_font),
                "unexpected fallback route for {ch:?} in {family:?}",
            );
        }
    }

    /// CJK サイドカーメタデータ (簡体字 / ハングル) が追加 fallback に正しく回り、
    /// 日本語と共有する漢字は引き続き Yu Gothic ("japanese") が拾うことを確認する。
    /// (docs/archive/search-metadata/sidecar-encoding-utf8.md: 読み取りは UTF-8 で正しく、表示はフォント被覆の問題)
    #[test]
    fn user_text_covers_cjk_scripts() {
        let mut fonts = egui::FontDefinitions::default();
        install_mimageviewer_fonts(&mut fonts);
        let family = fonts
            .families
            .get(&egui::FontFamily::Name(Arc::<str>::from(
                USER_TEXT_FAMILY_NAME,
            )))
            .expect("user text family should be registered");
        let route = |ch: char| -> Option<String> {
            family
                .iter()
                .find(|font_name| {
                    fonts
                        .font_data
                        .get(*font_name)
                        .and_then(|font| {
                            ttf_parser::Face::parse(font.font.as_ref(), font.index).ok()
                        })
                        .and_then(|face| face.glyph_index(ch))
                        .is_some()
                })
                .cloned()
        };
        // 共有漢字 '文' は日本語フォントが先に拾う (字形を日本語のまま維持)。
        assert_eq!(
            route('文').as_deref(),
            Some("japanese"),
            "shared Han '文' should stay on the Japanese font"
        );
        // 簡体字専用字 '约' は Yu Gothic に無く簡体字フォント (cjk_sc) に回る。
        assert_eq!(
            route('约').as_deref(),
            Some("cjk_sc"),
            "Simplified-only '约' should route to the Simplified Chinese fallback"
        );
        // ハングル '한' は上記すべてに無く韓国語フォント (korean) に回る。
        assert_eq!(
            route('한').as_deref(),
            Some("korean"),
            "Hangul '한' should route to the Korean fallback"
        );
        // 3 つの CJK fallback が family に登録されている。
        for name in ["cjk_sc", "cjk_tc", "korean"] {
            assert!(
                family.iter().any(|f| f == name),
                "{name} fallback should be registered in the user-text family"
            );
        }
    }

    #[test]
    fn custom_face_index_and_manual_toolbar_adjustment_are_preserved() {
        let path = std::path::PathBuf::from(r"C:\Windows\Fonts\meiryo.ttc");
        let face_index = collection_face_index(
            path.to_str().expect("Windows font path should be UTF-8"),
            "Meiryo UI",
        );
        let settings = crate::settings::UiFontSettings {
            selection: crate::settings::UiFontSelection::Face {
                display_name: "Meiryo UI".to_string(),
                path,
                face_index,
                post_script_name: String::new(),
            },
            vertical_adjust: 1.25,
        };

        let fonts = mimageviewer_font_definitions_for(&settings);
        let primary = fonts
            .font_data
            .get("ui_primary")
            .expect("custom primary should be installed");
        let toolbar = fonts
            .font_data
            .get("ui_primary_toolbar")
            .expect("custom toolbar face should be installed");
        assert_eq!(primary.index, face_index);
        assert_eq!(toolbar.index, face_index);
        assert!((toolbar.tweak.y_offset - (TOOLBAR_Y_OFFSET + 1.25)).abs() < f32::EPSILON);
    }

    /// v2.7.0 の代表候補を実際の Windows font collection から選び、推奨サイズで
    /// 自動補正した視覚中心と fallback の補正値を固定する。9/10pt は 96 DPI の
    /// typographic point を egui logical point (1px at 100%) へ換算して評価する。
    #[test]
    fn recommended_windows_fonts_auto_align_at_preferred_sizes() {
        let (default_data, default_index) =
            load_default_japanese_font().expect("default Japanese font should be available");
        let default_target = alignment_target(&default_data, default_index)
            .expect("default Japanese alignment target should be measurable");
        let default_centering = visual_centering_factor(default_target);

        for candidate in RECOMMENDED_WINDOWS_FONTS {
            let data = std::fs::read(candidate.path)
                .unwrap_or_else(|err| panic!("{} should load: {err}", candidate.label));
            let face_index = collection_face_index(candidate.path, candidate.family);
            let selected_target = alignment_target(&data, face_index)
                .unwrap_or_else(|| panic!("{} metrics should be measurable", candidate.label));
            let selected_centering = visual_centering_factor(selected_target);
            let expected_factor = selected_centering - default_centering;
            let logical_size = candidate.typographic_points * (96.0 / 72.0);

            let settings = crate::settings::UiFontSettings {
                selection: crate::settings::UiFontSelection::Face {
                    display_name: candidate.label.to_string(),
                    path: candidate.path.into(),
                    face_index,
                    post_script_name: String::new(),
                },
                vertical_adjust: 0.0,
            };
            let fonts = mimageviewer_font_definitions_for(&settings);
            let primary = fonts
                .font_data
                .get("ui_primary")
                .unwrap_or_else(|| panic!("{} primary face should be installed", candidate.label));
            let toolbar = fonts
                .font_data
                .get("ui_primary_toolbar")
                .unwrap_or_else(|| panic!("{} toolbar face should be installed", candidate.label));
            let hud_family = fonts
                .families
                .get(&egui::FontFamily::Name(Arc::<str>::from(
                    HUD_TEXT_FAMILY_NAME,
                )))
                .unwrap_or_else(|| panic!("{} HUD family should be installed", candidate.label));
            let hud = fonts
                .font_data
                .get(hud_family.first().expect("HUD family should not be empty"))
                .unwrap_or_else(|| panic!("{} HUD font should be installed", candidate.label));

            assert_eq!(primary.index, face_index, "{} face index", candidate.label);
            assert_eq!(
                toolbar.index, face_index,
                "{} toolbar face index",
                candidate.label
            );
            assert_eq!(
                hud.font.as_ref(),
                default_data.as_slice(),
                "{} must not change the fixed HUD font",
                candidate.label,
            );
            assert_eq!(
                hud.index, default_index,
                "{} HUD face index",
                candidate.label
            );
            assert!(
                (toolbar.tweak.y_offset_factor - expected_factor).abs() < f32::EPSILON,
                "{} automatic factor should follow measured glyph centers: {} vs {}",
                candidate.label,
                toolbar.tweak.y_offset_factor,
                expected_factor,
            );
            assert!(
                (toolbar.tweak.y_offset - TOOLBAR_Y_OFFSET).abs() < f32::EPSILON,
                "{} should retain the calibrated default toolbar offset",
                candidate.label,
            );

            // 選択フォント用の差分補正を引くと、既定フォントと同じ残差になる。
            // 推奨サイズで 0.05 logical pixel 未満を許容し、符号・単位の回帰も検知する。
            let residual_pixels =
                ((selected_centering - toolbar.tweak.y_offset_factor) - default_centering).abs()
                    * logical_size;
            assert!(
                residual_pixels < 0.05,
                "{} residual should be visually negligible at {}pt: {residual_pixels}px",
                candidate.label,
                candidate.typographic_points,
            );

            // 記号・絵文字・CJK fallback も既定フォントではなく、選択した face の
            // 実メトリクスを基準に再計算されていることを確認する。
            for fallback in USER_TEXT_FALLBACKS {
                let Ok(fallback_data) = std::fs::read(fallback.path) else {
                    continue;
                };
                let expected =
                    fallback_y_offset_factor(fallback, &fallback_data, Some(selected_target));
                let actual = fonts
                    .font_data
                    .get(fallback.name)
                    .unwrap_or_else(|| {
                        panic!(
                            "{} fallback {} should be installed",
                            candidate.label, fallback.name
                        )
                    })
                    .tweak
                    .y_offset_factor;
                assert!(
                    (actual - expected).abs() < f32::EPSILON,
                    "{} fallback {} should align to the selected face: {actual} vs {expected}",
                    candidate.label,
                    fallback.name,
                );
            }
        }
    }

    #[test]
    fn non_japanese_and_italic_faces_fall_back_to_default() {
        let mut cases = vec![(
            "Arial",
            std::path::PathBuf::from(r"C:\Windows\Fonts\arial.ttf"),
            collection_face_index(r"C:\Windows\Fonts\arial.ttf", "Arial"),
        )];

        let path = std::path::PathBuf::from(r"C:\Windows\Fonts\meiryo.ttc");
        let mut db = fontdb::Database::new();
        db.load_font_file(&path)
            .expect("Meiryo collection should load");
        let italic_index = db
            .faces()
            .find(|face| {
                face.style == fontdb::Style::Italic
                    && face
                        .families
                        .iter()
                        .any(|(name, _)| name.eq_ignore_ascii_case("Meiryo"))
            })
            .map(|face| face.index)
            .expect("Meiryo Italic should exist");
        cases.push(("Meiryo Italic", path, italic_index));

        for (label, path, face_index) in cases {
            let settings = crate::settings::UiFontSettings {
                selection: crate::settings::UiFontSelection::Face {
                    display_name: label.to_string(),
                    path,
                    face_index,
                    post_script_name: String::new(),
                },
                vertical_adjust: 0.0,
            };
            let fonts = mimageviewer_font_definitions_for(&settings);
            assert!(
                !fonts.font_data.contains_key("ui_primary"),
                "{label} should be rejected as a UI primary"
            );
            assert!(fonts.font_data.contains_key("japanese"));
        }
    }

    #[test]
    fn unavailable_custom_font_falls_back_to_default_primary() {
        let settings = crate::settings::UiFontSettings {
            selection: crate::settings::UiFontSelection::Face {
                display_name: "Missing".to_string(),
                path: std::path::PathBuf::from(r"C:\missing\font.ttf"),
                face_index: 0,
                post_script_name: String::new(),
            },
            vertical_adjust: 0.0,
        };
        let fonts = mimageviewer_font_definitions_for(&settings);
        assert!(!fonts.font_data.contains_key("ui_primary"));
        assert!(fonts.font_data.contains_key("japanese"));
        assert!(fonts.font_data.contains_key("japanese_toolbar"));
    }

    #[test]
    fn cjk_offsets_include_egui_row_metrics() {
        const SC_SAMPLES: &[char] = &['这', '哪', '衣', '约', '轮', '苏', '恶', '觉'];
        const TC_SAMPLES: &[char] = &['這', '哪', '衣', '約', '輪', '蘇', '惡', '覺'];
        const KO_SAMPLES: &[char] = &['한', '글', '가', '나', '다', '라'];

        let japanese = JAPANESE_FONT_PATHS
            .iter()
            .find_map(|path| std::fs::read(path).ok())
            .expect("Windows Japanese font should be available");
        let body_text = japanese_alignment_target(&japanese);

        for (name, path, samples) in [
            ("cjk_sc", r"C:\Windows\Fonts\msyh.ttc", SC_SAMPLES),
            ("cjk_tc", r"C:\Windows\Fonts\msjh.ttc", TC_SAMPLES),
            ("korean", r"C:\Windows\Fonts\malgun.ttf", KO_SAMPLES),
        ] {
            let data = std::fs::read(path).unwrap_or_else(|e| panic!("{name} should load: {e}"));
            let font = USER_TEXT_FALLBACKS
                .iter()
                .find(|font| font.name == name)
                .expect("fallback definition should exist");
            let fallback_center = font_center_y_for_samples(&data, samples)
                .unwrap_or_else(|| panic!("{name} center should be measurable"));
            let fallback_metrics = font_row_metrics(&data)
                .unwrap_or_else(|| panic!("{name} row metrics should be measurable"));
            let offset = fallback_y_offset_factor(font, &data, Some(body_text));
            let scale = font.scale.max(0.01);
            let target_visual_center = body_text.ascent - body_text.center_y;
            let fallback_visual_center = 0.5 * body_text.row_height
                + scale
                    * (fallback_metrics.ascent
                        - 0.5 * fallback_metrics.row_height
                        - fallback_center);
            let expected = ((target_visual_center - fallback_visual_center) / scale)
                .clamp(-DERIVED_Y_OFFSET_CLAMP, DERIVED_Y_OFFSET_CLAMP);
            assert!(
                (offset - expected).abs() < f32::EPSILON,
                "{name} offset should include egui row metrics: {offset} vs {expected}",
            );
        }
    }

    #[test]
    fn user_text_prefers_text_symbols_before_emoji() {
        let mut fonts = egui::FontDefinitions::default();
        install_mimageviewer_fonts(&mut fonts);
        let family = fonts
            .families
            .get(&egui::FontFamily::Name(Arc::<str>::from(
                USER_TEXT_FAMILY_NAME,
            )))
            .expect("user text family should be registered");
        let text_symbols = family
            .iter()
            .position(|name| name == "text_symbols")
            .expect("text symbol fallback should be registered");
        let emoji = family
            .iter()
            .position(|name| name == "emoji")
            .expect("emoji fallback should be registered");
        let math = family
            .iter()
            .position(|name| name == "math")
            .expect("math fallback should be registered");
        assert!(
            text_symbols < math && math < emoji,
            "text-presentation symbols should be tried before math and emoji: {family:?}",
        );
    }
}

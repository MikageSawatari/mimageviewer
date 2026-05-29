use std::sync::Arc;

pub const USER_TEXT_FAMILY_NAME: &str = "miv-user-text";
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

pub fn configure_fonts(ctx: &egui::Context) {
    let mut fonts = egui::FontDefinitions::default();
    install_mimageviewer_fonts(&mut fonts);
    ctx.set_fonts(fonts);
}

pub fn install_mimageviewer_fonts(fonts: &mut egui::FontDefinitions) {
    let base_proportional = fonts
        .families
        .get(&egui::FontFamily::Proportional)
        .cloned()
        .unwrap_or_default();
    let mut primary_names: Vec<String> = Vec::new();
    let alignment_target = install_japanese_font(fonts);
    if alignment_target.is_some() {
        primary_names.push("japanese".to_owned());
    }

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

    // ツールバー専用 family: 先頭に y_offset 補正済みの "japanese_toolbar"、
    // 残りは通常の fallback で埋める (= 絵文字や記号の挙動は通常 family と同じ)。
    //
    // ⚠ Codex P2 (2026-05): Yu Gothic / Meiryo / MS Gothic が見つからない環境では
    // japanese_toolbar が登録されないが、family 自体は ui_main.rs から常に参照される
    // ため、family を登録しないと egui が「family is not bound to any fonts」で panic
    // する。japanese_toolbar が無いときは通常の primary fallback (= "japanese" など)
    // にフォールバックさせて family は **常に**登録する。
    let mut toolbar_fonts = Vec::new();
    if fonts.font_data.contains_key("japanese_toolbar") {
        toolbar_fonts.push("japanese_toolbar".to_owned());
    } else {
        // 日本語フォント未登録環境: primary (= 通常 family の先頭) にフォールバック
        extend_unique(&mut toolbar_fonts, primary_names.iter().cloned());
    }
    extend_unique(&mut toolbar_fonts, user_text_fallback_names.iter().cloned());
    extend_unique(&mut toolbar_fonts, base_proportional);
    fonts.families.insert(
        egui::FontFamily::Name(Arc::<str>::from(TOOLBAR_TEXT_FAMILY_NAME)),
        toolbar_fonts,
    );
}

fn install_japanese_font(fonts: &mut egui::FontDefinitions) -> Option<FontAlignmentTarget> {
    for path in JAPANESE_FONT_PATHS {
        let Ok(data) = std::fs::read(path) else {
            continue;
        };
        let alignment_target = font_row_metrics(&data).and_then(|metrics| {
            font_center_y_for_samples(&data, &['今', 'あ'])
                .or_else(|| font_metric_center_y(&data))
                .map(|center_y| FontAlignmentTarget {
                    ascent: metrics.ascent,
                    center_y,
                    row_height: metrics.row_height,
                })
        });
        // ツールバー専用は同じデータに `y_offset` だけ載せたバリアント。
        // 元データは普通の "japanese" として登録、tweak 版は "japanese_toolbar"。
        fonts.font_data.insert(
            "japanese_toolbar".to_owned(),
            Arc::new(
                egui::FontData::from_owned(data.clone()).tweak(egui::FontTweak {
                    y_offset: TOOLBAR_Y_OFFSET,
                    ..Default::default()
                }),
            ),
        );
        fonts.font_data.insert(
            "japanese".to_owned(),
            Arc::new(egui::FontData::from_owned(data)),
        );
        return alignment_target;
    }
    None
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
    let face = ttf_parser::Face::parse(data, 0).ok()?;
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

fn font_metric_center_y(data: &[u8]) -> Option<f32> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
    let units_per_em = f32::from(face.units_per_em());
    let ascender = face.ascender() as f32;
    let descender = face.descender() as f32;
    Some((ascender + descender) * 0.5 / units_per_em)
}

fn font_row_metrics(data: &[u8]) -> Option<FontRowMetrics> {
    let face = ttf_parser::Face::parse(data, 0).ok()?;
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
    /// (docs/sidecar-encoding-utf8.md: 読み取りは UTF-8 で正しく、表示はフォント被覆の問題)
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

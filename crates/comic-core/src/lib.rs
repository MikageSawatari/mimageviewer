//! `comic-core` — pure (egui/eframe-free) data model + layout + rasterizer for
//! the speech-bubble / text-annotation feature.
//!
//! Phase 1 scope (see `docs/speech-bubble-text-tool-plan.md` §9):
//!   - data model (AnnotationObject / BubbleObject / Tail / TextBlock),
//!   - a shared text layout engine (horizontal + vertical/縦書き + minimal
//!     auto-縦中横),
//!   - shape tessellation (ellipse / round-rect + straight tail),
//!   - a CPU rasterizer that bakes objects into an RGBA8 overlay (袋文字 via
//!     dilated halo), and
//!   - TTF/OTF/TTC font loading + OpenType vertical shaping via `rustybuzz`,
//!     glyph coverage via `ab_glyph_rasterizer` (see `font.rs`).
//!
//! The lab (`tools/comic_lab`) drives the interactive UI; the eventual main-app
//! integration reuses this crate unchanged.

pub mod font;
pub mod layout;
pub mod model;
pub mod raster;
pub mod tessellate;

pub use font::{FontSet, GlyphBitmap, LoadedFont, rotate_cw};
pub use layout::{GlyphForm, GlyphPlacement, TextLayout, layout_text, layout_text_wrapped};
pub use model::{
    AnnotationKind, AnnotationObject, BubbleObject, BubbleShape, DecoKind, DecoPlacement,
    DecorationLayer, FillMode, FrameStyle, IndicatorKind, InlineDir, Insets, MarkupRule,
    MessageWindowObject, NamePlate, NamePlateMode, Orientation, PortraitSide, PortraitSlot, Rgba,
    ShadowStyle, SizeMode, StampObject, StampSource, StrokeStyle, Tail, TailKind, TextAlign,
    TextBlock, VAnchor, WindowPosition, default_markup_rules, markup_rules_angle,
    markup_rules_brackets, markup_rules_white,
};
pub use raster::{
    RgbaOverlay, StampImages, bake_overlay, bake_overlay_with_stamps, composite_stamp_sticker,
    effective_bubble_shape, effective_window_half_extents, message_window_overflows,
};
pub use tessellate::{
    BubbleGeometry, auto_base_t, bubble_geometry, fit_bubble_shape, nearest_base_t,
    resolve_tail_base, shape_is_mergeable, shape_renders_tail, tessellate_bubble, tessellate_tail,
};

#[cfg(test)]
mod integration_tests {
    use super::*;

    /// Candidate Windows Japanese fonts to drive font-dependent tests.
    const FONT_CANDIDATES: &[&str] = &[
        r"C:\Windows\Fonts\YuGothM.ttc",
        r"C:\Windows\Fonts\meiryo.ttc",
        r"C:\Windows\Fonts\msgothic.ttc",
        r"C:\Windows\Fonts\YuGothR.ttc",
    ];

    fn load_test_font() -> Option<LoadedFont> {
        for path in FONT_CANDIDATES {
            if let Ok(bytes) = std::fs::read(path) {
                if let Ok(font) = LoadedFont::from_bytes("test", bytes) {
                    return Some(font);
                }
            }
        }
        None
    }

    fn text_block(text: &str, orientation: Orientation) -> TextBlock {
        TextBlock {
            text: text.to_string(),
            size_px: 48.0,
            orientation,
            ..TextBlock::default()
        }
    }

    #[test]
    fn vertical_columns_advance_right_to_left() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        // Two columns separated by '\n'. The first column must be to the RIGHT
        // of the second (縦書き advances right-to-left).
        let block = text_block("あい\nうえ", Orientation::Vertical);
        let layout = layout_text(&block, &font);
        // Column 0 chars: あ, い ; Column 1 chars: う, え.
        let x_first_col: f32 = layout
            .glyphs
            .iter()
            .filter(|g| g.ch == 'あ' || g.ch == 'い')
            .map(|g| g.x)
            .fold(f32::NEG_INFINITY, f32::max);
        let x_second_col: f32 = layout
            .glyphs
            .iter()
            .filter(|g| g.ch == 'う' || g.ch == 'え')
            .map(|g| g.x)
            .fold(f32::INFINITY, f32::min);
        assert!(
            x_first_col > x_second_col,
            "first column ({x_first_col}) should be right of second ({x_second_col})"
        );
    }

    #[test]
    fn vertical_stacks_top_to_bottom() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        let block = text_block("あい", Orientation::Vertical);
        let layout = layout_text(&block, &font);
        let y_a = layout.glyphs.iter().find(|g| g.ch == 'あ').unwrap().y;
        let y_i = layout.glyphs.iter().find(|g| g.ch == 'い').unwrap().y;
        assert!(y_a < y_i, "first char should be above second in a column");
    }

    #[test]
    fn repeated_bangs_stack_vertical() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        // "びっくり!!!!" in Vertical: pure punctuation runs stack upright now
        // (no split into !!!! + !), so the four '!' have distinct, increasing y
        // and full body size.
        let block = text_block("びっくり!!!!", Orientation::Vertical);
        let layout = layout_text(&block, &font);
        let mut bangs: Vec<&GlyphPlacement> =
            layout.glyphs.iter().filter(|g| g.ch == '!').collect();
        assert_eq!(bangs.len(), 4, "expected four '!' glyphs");
        bangs.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap());
        for w in bangs.windows(2) {
            assert!(w[1].y > w[0].y, "stacked '!' should advance vertically");
        }
        assert!(
            bangs.iter().all(|g| (g.size - block.size_px).abs() < 0.01),
            "stacked '!' keep full body size"
        );
    }

    #[test]
    fn mixed_punct_tcy_cluster() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        // "えっ!?" in Vertical: the mixed pair !? becomes one horizontal 縦中横
        // cluster (shared baseline, increasing x, reduced size).
        let block = text_block("えっ!?", Orientation::Vertical);
        let layout = layout_text(&block, &font);
        let marks: Vec<&GlyphPlacement> = layout
            .glyphs
            .iter()
            .filter(|g| g.ch == '!' || g.ch == '?')
            .collect();
        assert_eq!(marks.len(), 2, "expected '!' and '?'");
        assert!(
            (marks[0].y - marks[1].y).abs() < 1.0,
            "縦中横 members share one baseline"
        );
        assert!(
            marks[1].x != marks[0].x,
            "縦中横 members advance horizontally"
        );
        assert!(
            marks.iter().all(|g| (g.size - block.size_px).abs() < 0.01),
            "縦中横 glyphs keep full body size (no shrink)"
        );
    }

    #[test]
    fn vertical_period_sits_in_upper_right_cell() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        let block = text_block("。", Orientation::Vertical);
        let layout = layout_text(&block, &font);
        let g = layout.glyphs.iter().find(|g| g.ch == '。').unwrap();
        let Some((min_x, min_y, w, _h)) = font.glyph_px_bounds_gid(g.glyph_id, g.size) else {
            eprintln!("skip: font has no period bounds");
            return;
        };
        let ink_right = g.x + min_x + w;
        let ink_top = g.y + min_y;
        let cell = font.h_advance('\u{3042}', block.size_px).max(block.size_px);
        assert!(
            ink_right > cell * 0.65,
            "vertical 。 should sit near the right edge: right={ink_right}, cell={cell}"
        );
        assert!(
            ink_top < block.size_px * 0.35,
            "vertical 。 should sit near the top edge: top={ink_top}"
        );
    }

    #[test]
    fn vertical_ellipsis_gets_vertical_form_or_rotation() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        let block = text_block("…", Orientation::Vertical);
        let layout = layout_text(&block, &font);
        let g = layout.glyphs.iter().find(|g| g.ch == '…').unwrap();
        // Top-to-bottom shaping must substitute a vertical form (`vert` feature or
        // the UAX#50 fallback), i.e. a different glyph than the plain horizontal
        // cmap glyph. Verifies the OpenType vertical path is actually engaged.
        assert!(
            g.glyph_id != font.glyph_id('…'),
            "vertical … must shape to a vertical presentation glyph (got the horizontal cmap glyph): {g:?}"
        );
    }

    #[test]
    fn vertical_corner_quotes_get_vertical_form_or_rotation() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        let block = text_block("「」", Orientation::Vertical);
        let layout = layout_text(&block, &font);
        for ch in ['「', '」'] {
            let g = layout.glyphs.iter().find(|g| g.ch == ch).unwrap();
            // Vertical shaping must substitute a vertical bracket form (different
            // glyph than the horizontal cmap glyph) via the font's `vert` feature.
            assert!(
                g.glyph_id != font.glyph_id(ch),
                "vertical quote {ch} must shape to a vertical presentation glyph: {g:?}"
            );
        }
    }

    #[test]
    fn vertical_ivs_stays_in_one_cell() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        // A base kanji + ideographic variation selector (U+E0100) must occupy the
        // SAME single cell as the bare kanji — the selector is shaped into the
        // grapheme, not dropped into a stray empty cell.
        let base = layout_text(&text_block("辻", Orientation::Vertical), &font);
        let ivs = layout_text(&text_block("辻\u{E0100}", Orientation::Vertical), &font);
        assert!(
            (ivs.bounds.1 - base.bounds.1).abs() < 1.0,
            "IVS must not add a second cell: base h={}, ivs h={}",
            base.bounds.1,
            ivs.bounds.1
        );
    }

    #[test]
    fn vertical_decomposed_dakuten_stays_in_one_cell() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        // Decomposed (NFD) か + U+3099 must occupy one cell like precomposed が.
        let precomposed = layout_text(&text_block("が", Orientation::Vertical), &font);
        let decomposed = layout_text(&text_block("か\u{3099}", Orientation::Vertical), &font);
        assert!(
            (decomposed.bounds.1 - precomposed.bounds.1).abs() < 1.0,
            "decomposed dakuten must stay in one cell: precomposed h={}, decomposed h={}",
            precomposed.bounds.1,
            decomposed.bounds.1
        );
    }

    #[test]
    fn vertical_comma_sits_in_upper_right_cell() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        let block = text_block("、", Orientation::Vertical);
        let layout = layout_text(&block, &font);
        let g = layout.glyphs.iter().find(|g| g.ch == '、').unwrap();
        // `vert` must substitute a different glyph than the horizontal cmap glyph.
        assert!(
            g.glyph_id != font.glyph_id('、'),
            "vertical 、 must shape to a vertical presentation glyph: {g:?}"
        );
        let Some((min_x, min_y, w, _h)) = font.glyph_px_bounds_gid(g.glyph_id, g.size) else {
            eprintln!("skip: font has no comma bounds");
            return;
        };
        let ink_right = g.x + min_x + w;
        let ink_top = g.y + min_y;
        let cell = font.h_advance('\u{3042}', block.size_px).max(block.size_px);
        assert!(
            ink_right > cell * 0.5,
            "vertical 、 should sit toward the right edge: right={ink_right}, cell={cell}"
        );
        assert!(
            ink_top < block.size_px * 0.5,
            "vertical 、 should sit toward the top edge: top={ink_top}"
        );
    }

    #[test]
    fn vertical_golden_set_bakes_without_panic() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        let mut fonts = FontSet::new();
        fonts.insert(font);
        // The validation checklist golden strings (docs/comic-lab-validation-
        // checklist.md §2): each must shape + bake vertically without panicking
        // and write opaque pixels through the rustybuzz TTB path.
        let golden = [
            "「……。」",
            "えっ!?",
            "びっくり!!!!",
            "あー、そう……？",
            "（テスト）【重要】《確認》",
            "小さいっゃゅょ、ゎ。",
            "2026年6月4日",
        ];
        for s in golden {
            let tb = TextBlock {
                text: s.to_string(),
                size_px: 40.0,
                color: Rgba::BLACK,
                orientation: Orientation::Vertical,
                markup_enabled: true,
                ..TextBlock::default()
            };
            let obj = AnnotationObject::new_text(1, (10.0, 10.0), tb);
            let ov = bake_overlay(&[obj], 480, 480, &fonts);
            assert!(
                ov.pixels.chunks_exact(4).any(|p| p[3] > 0),
                "golden string {s:?} should bake opaque pixels"
            );
        }
    }

    #[test]
    fn layout_bounds_are_positive() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        let h = layout_text(&text_block("Hello", Orientation::Horizontal), &font);
        assert!(
            h.bounds.0 > 0.0 && h.bounds.1 > 0.0,
            "h bounds: {:?}",
            h.bounds
        );
        let v = layout_text(&text_block("あいう", Orientation::Vertical), &font);
        assert!(
            v.bounds.0 > 0.0 && v.bounds.1 > 0.0,
            "v bounds: {:?}",
            v.bounds
        );
    }

    #[test]
    fn outlined_glyph_mask_is_larger() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        let plain = font
            .rasterize('A', 64.0, 0.0)
            .expect("glyph A should rasterize");
        let outlined = font
            .rasterize('A', 64.0, 4.0)
            .expect("outlined glyph A should rasterize");
        assert!(
            outlined.width > plain.width && outlined.height > plain.height,
            "outlined mask ({}x{}) should be larger than plain ({}x{})",
            outlined.width,
            outlined.height,
            plain.width,
            plain.height
        );
        // The dilated mask must also cover at least as many pixels.
        let plain_cov = plain.coverage.iter().filter(|&&c| c > 0.0).count();
        let out_cov = outlined.coverage.iter().filter(|&&c| c > 0.0).count();
        assert!(out_cov > plain_cov, "outlined coverage should exceed plain");
    }

    #[test]
    fn bake_with_text_writes_pixels() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        let mut fonts = FontSet::new();
        fonts.insert(font);
        let mut tb = TextBlock {
            text: "あ".to_string(),
            size_px: 64.0,
            color: Rgba::BLACK,
            outline: Some(StrokeStyle {
                color: Rgba::WHITE,
                width_px: 3.0,
            }),
            ..TextBlock::default()
        };
        tb.orientation = Orientation::Horizontal;
        let obj = AnnotationObject::new_text(1, (10.0, 10.0), tb);
        let ov = bake_overlay(&[obj], 200, 120, &fonts);
        assert!(
            ov.pixels.chunks_exact(4).any(|p| p[3] > 0),
            "baked text should write opaque pixels"
        );
    }

    /// A vertical TextBlock with marker markup enabled and the default rules.
    fn markup_block(text: &str) -> TextBlock {
        TextBlock {
            text: text.to_string(),
            size_px: 48.0,
            orientation: Orientation::Vertical,
            markup_enabled: true,
            ..TextBlock::default()
        }
    }

    #[test]
    fn tcy_full_size_widens_column_no_overlap() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        // 縦中横 is now laid at FULL body size (no shrink); a wide run widens its
        // column so it doesn't collide with the neighbouring column. Layout:
        // col0 = Tcy([A..G]) (rightmost, wide), col1 = X / Y singles (to its left).
        let block = markup_block("[ABCDEFG]\nXY");
        let layout = layout_text(&block, &font);
        let marked: Vec<&GlyphPlacement> = layout
            .glyphs
            .iter()
            .filter(|g| ('A'..='G').contains(&g.ch))
            .collect();
        assert_eq!(marked.len(), 7, "all 7 chars placed");
        // Full body size (NOT shrunk) and a shared baseline.
        assert!(
            marked.iter().all(|g| (g.size - block.size_px).abs() < 0.01),
            "縦中横 keeps full body size"
        );
        let y0 = marked[0].y;
        assert!(
            marked.iter().all(|g| (g.y - y0).abs() < 1.0),
            "縦中横 members share one baseline"
        );
        // The run is full-size wide (> one cell): the column must have widened.
        let run_left = marked.iter().map(|g| g.x).fold(f32::INFINITY, f32::min);
        let run_right = marked
            .iter()
            .map(|g| g.x + font.h_advance(g.ch, g.size))
            .fold(f32::NEG_INFINITY, f32::max);
        let cell = font.h_advance('\u{3042}', block.size_px);
        assert!(
            run_right - run_left > cell,
            "full-size run is wider than a cell"
        );
        // No overlap: the X/Y column (to the left) must end before the tcy run starts.
        let xy_right = layout
            .glyphs
            .iter()
            .filter(|g| g.ch == 'X' || g.ch == 'Y')
            .map(|g| g.x + font.h_advance(g.ch, g.size))
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            xy_right <= run_left + 1.0,
            "neighbouring column (right edge {xy_right}) must not overlap the 縦中横 run (left {run_left})"
        );
    }

    #[test]
    fn sideways_run_advances_along_column_and_is_flagged() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        // `{LOVE}` -> a Sideways cluster: glyphs flagged sideways, stacked DOWN
        // the column (increasing y) in reading order. (`{}` is the default 横倒し
        // marker now.)
        let block = markup_block("{LOVE}");
        let layout = layout_text(&block, &font);
        let mut side: Vec<&GlyphPlacement> = layout
            .glyphs
            .iter()
            .filter(|g| "LOVE".contains(g.ch))
            .collect();
        assert_eq!(side.len(), 4, "L O V E placed");
        assert!(
            side.iter().all(|g| g.sideways),
            "sideways glyphs flagged sideways"
        );
        side.sort_by(|a, b| a.y.partial_cmp(&b.y).unwrap());
        // Reading order L,O,V,E should advance down the column.
        let order: String = side.iter().map(|g| g.ch).collect();
        assert_eq!(order, "LOVE", "reading order top->bottom");
        for w in side.windows(2) {
            assert!(w[1].y > w[0].y, "sideways glyphs advance down the column");
        }
    }

    #[test]
    fn sideways_glyphs_fit_within_column() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        // A 横倒し run rotates each glyph 90°, so the glyph's ink HEIGHT becomes
        // its across-column width. After fit-scaling, no sideways glyph's ink
        // height (at its placed size) may exceed ~90% of the column cell.
        let block = markup_block("{WMjgyQ}");
        let layout = layout_text(&block, &font);
        let size = block.size_px;
        let cell = font.h_advance('\u{3042}', size).max(size);
        let limit = cell * 0.9;
        let side: Vec<&GlyphPlacement> = layout.glyphs.iter().filter(|g| g.sideways).collect();
        assert!(!side.is_empty(), "the run produced sideways glyphs");
        for g in side {
            let h = font.glyph_height(g.ch, g.size);
            assert!(
                h <= limit + 1.0,
                "sideways glyph '{}' ink height {h} exceeds column limit {limit}",
                g.ch
            );
            assert!(g.size <= size + 0.01, "fit only shrinks, never grows");
        }
    }

    #[test]
    fn markup_literal_when_disabled_in_layout() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        // With markup disabled, the bracket characters are laid out literally.
        let mut block = markup_block("[AI]");
        block.markup_enabled = false;
        let layout = layout_text(&block, &font);
        assert!(
            layout.glyphs.iter().any(|g| g.ch == '['),
            "markers are literal glyphs when markup disabled"
        );
        assert!(
            layout.glyphs.iter().all(|g| !g.sideways),
            "no sideways glyphs when markup disabled"
        );
    }

    #[test]
    fn marker_sets_have_distinct_pairs_and_fixed_dirs() {
        // All three built-in sets follow the same contract: 2 pairs where the
        // first is 縦中横 and the second is 横倒し. The default == set A.
        for rules in [
            markup_rules_brackets(),
            markup_rules_angle(),
            markup_rules_white(),
        ] {
            assert_eq!(rules.len(), 2);
            assert_eq!(rules[0].dir, InlineDir::TateChuYoko);
            assert_eq!(rules[1].dir, InlineDir::Sideways);
            assert_ne!(rules[0].open, rules[1].open, "the two pairs differ");
        }
        assert_eq!(default_markup_rules(), markup_rules_brackets());
        // The opening chars are unique across sets so the lab can detect which
        // set is active by comparing chars.
        let opens: Vec<char> = [
            markup_rules_brackets(),
            markup_rules_angle(),
            markup_rules_white(),
        ]
        .iter()
        .map(|r| r[0].open)
        .collect();
        assert_eq!(opens, vec!['[', '〈', '〚']);
    }

    #[test]
    fn object_clone_eq() {
        // serde Serialize/Deserialize presence is verified at compile time by
        // the `#[derive(...)]` on the model types (and exercised by the lab's
        // serde_json sidecar). comic-core itself doesn't depend on serde_json,
        // so here we just confirm the model is clonable/comparable.
        let obj = AnnotationObject::new_bubble(
            7,
            (1.0, 2.0),
            BubbleObject {
                tail: Some(Tail::default()),
                ..BubbleObject::default()
            },
        );
        assert_eq!(obj.clone(), obj);
    }

    #[test]
    fn preset_link_fields_default_none() {
        // The preset-link fields default to None (so old sidecars that lack the
        // keys keep loading via serde-default) and are clonable along with the
        // rest of the model.
        let tb = TextBlock::default();
        assert_eq!(tb.preset_link, None);
        let b = BubbleObject::default();
        assert_eq!(b.shape_preset_link, None);

        let mut tb2 = TextBlock::default();
        tb2.preset_link = Some("user:foo".to_string());
        let mut b2 = BubbleObject::default();
        b2.shape_preset_link = Some("sys:normal".to_string());
        b2.text = tb2.clone();
        assert_eq!(b2.clone(), b2);
        assert_eq!(b2.text.preset_link.as_deref(), Some("user:foo"));
        assert_eq!(b2.shape_preset_link.as_deref(), Some("sys:normal"));
    }

    // ---- word wrap (kinsoku) ----

    /// Group horizontal glyphs into lines by baseline y, each line sorted by x.
    fn lines_by_y(layout: &TextLayout) -> Vec<Vec<GlyphPlacement>> {
        let mut ys: Vec<f32> = layout.glyphs.iter().map(|g| g.y).collect();
        ys.sort_by(|a, b| a.partial_cmp(b).unwrap());
        ys.dedup_by(|a, b| (*a - *b).abs() < 0.5);
        ys.into_iter()
            .map(|y| {
                let mut line: Vec<GlyphPlacement> = layout
                    .glyphs
                    .iter()
                    .filter(|g| (g.y - y).abs() < 0.5)
                    .copied()
                    .collect();
                line.sort_by(|a, b| a.x.partial_cmp(&b.x).unwrap());
                line
            })
            .collect()
    }

    #[test]
    fn horizontal_wrap_splits_long_line() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        let block = text_block("あいうえおかきくけこ", Orientation::Horizontal);
        let cell = font.h_advance('あ', block.size_px);
        let max_w = cell * 3.0 + 0.5; // ~3 glyphs per line
        let layout = layout_text_wrapped(&block, &font, Some(max_w));
        let lines = lines_by_y(&layout);
        assert!(
            lines.len() >= 2,
            "long line should wrap into >=2 lines, got {}",
            lines.len()
        );
        for line in &lines {
            let w: f32 = line.iter().map(|g| font.h_advance(g.ch, g.size)).sum();
            assert!(
                w <= max_w + 1.0,
                "wrapped line width {w} exceeds max {max_w}"
            );
        }
        // Unwrapped: a single line.
        let flat = layout_text(&block, &font);
        assert_eq!(lines_by_y(&flat).len(), 1);
    }

    #[test]
    fn wrap_keeps_line_head_punct_off_head() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        // Greedy would put 。 at the head of a wrapped line; kinsoku push-out
        // must keep it off any line head.
        let block = text_block("あああああ。あああああ", Orientation::Horizontal);
        let cell = font.h_advance('あ', block.size_px);
        let max_w = cell * 5.0 + 0.5;
        let layout = layout_text_wrapped(&block, &font, Some(max_w));
        for line in lines_by_y(&layout) {
            if let Some(first) = line.first() {
                assert_ne!(
                    first.ch, '。',
                    "no wrapped line may start with 。 (行頭禁則)"
                );
            }
        }
    }

    #[test]
    fn wrap_keeps_open_bracket_off_line_end() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        let block = text_block("あああ「ああああ", Orientation::Horizontal);
        let cell = font.h_advance('あ', block.size_px);
        let max_w = cell * 4.0 + 0.5;
        let layout = layout_text_wrapped(&block, &font, Some(max_w));
        for line in lines_by_y(&layout) {
            if let Some(last) = line.last() {
                assert_ne!(last.ch, '「', "no wrapped line may end with 「 (行末禁則)");
            }
        }
    }

    #[test]
    fn wrap_keeps_latin_word_intact() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        // The Latin word HELLO must not be split across a wrap boundary.
        let block = text_block("ああああHELLOああ", Orientation::Horizontal);
        let cell = font.h_advance('あ', block.size_px);
        let max_w = cell * 5.0 + 0.5;
        let layout = layout_text_wrapped(&block, &font, Some(max_w));
        let ys: Vec<f32> = layout
            .glyphs
            .iter()
            .filter(|g| g.ch.is_ascii_alphabetic())
            .map(|g| g.y)
            .collect();
        assert!(!ys.is_empty(), "expected the Latin word to be laid out");
        let y0 = ys[0];
        assert!(
            ys.iter().all(|y| (y - y0).abs() < 0.5),
            "Latin word was split across lines"
        );
    }

    #[test]
    fn wrap_overlong_latin_word_not_split() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        // A single Latin word wider than the wrap width must NOT be split into
        // one-letter lines — it overflows on one line instead.
        let block = text_block("ABCDEFGHIJ", Orientation::Horizontal);
        let cell = font.h_advance('A', block.size_px);
        let max_w = cell * 2.0; // far narrower than the word
        let layout = layout_text_wrapped(&block, &font, Some(max_w));
        let ys: Vec<f32> = layout.glyphs.iter().map(|g| g.y).collect();
        let y0 = ys[0];
        assert!(
            ys.iter().all(|y| (y - y0).abs() < 0.5),
            "an overlong Latin word must stay on a single (overflowing) line"
        );
        assert_eq!(layout.glyphs.len(), 10, "all letters present, none dropped");
    }

    #[test]
    fn vertical_wrap_splits_into_columns() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        let block = text_block("あいうえおかきくけこ", Orientation::Vertical);
        let glyph_step = block.size_px; // letter_gap default 0
        let max_h = glyph_step * 3.0 + 0.5; // ~3 glyphs per column
        let nowrap = layout_text(&block, &font);
        let wrapped = layout_text_wrapped(&block, &font, Some(max_h));
        // Wrapping a single hard column into several makes the layout WIDER
        // (more columns) and keeps each column within the height limit.
        assert!(
            wrapped.bounds.0 > nowrap.bounds.0 + 1.0,
            "wrapped vertical text should occupy more columns (wider): {} vs {}",
            wrapped.bounds.0,
            nowrap.bounds.0
        );
        assert!(
            wrapped.bounds.1 <= max_h + 0.5,
            "each wrapped column height {} should fit the limit {max_h}",
            wrapped.bounds.1
        );
    }

    // ---- message window ----

    #[test]
    fn message_window_bakes_text_inside_panel() {
        let Some(font) = load_test_font() else {
            eprintln!("skip: no Windows JP font available");
            return;
        };
        // load_test_font registers under the key "test".
        let mut fonts = FontSet::new();
        fonts.insert(font);
        let mut win = MessageWindowObject {
            size_mode: SizeMode::Inset,
            position: WindowPosition::Free,
            half_w: 160.0,
            half_h: 80.0,
            fill: Some(Rgba::new(10, 10, 30, 255)),
            ..MessageWindowObject::default()
        };
        win.text.text = "あいうえお かきくけこ さしすせそ たちつてと なにぬねの".to_string();
        win.text.size_px = 40.0;
        win.text.font_key = "test".to_string();
        win.text.color = Rgba::WHITE;
        let obj = AnnotationObject::new_message_window(1, (200.0, 120.0), win);
        let ov = bake_overlay(&[obj], 400, 240, &fonts);
        // Panel spans x in [40,360], y in [40,200]. Body text (white) must write
        // some near-white pixels inside the panel, and the clip must keep all
        // opaque pixels within the panel bounds (+ a small AA margin).
        let mut white_inside = false;
        for y in 0..240usize {
            for x in 0..400usize {
                let i = (y * 400 + x) * 4;
                if ov.pixels[i + 3] > 0 {
                    assert!(
                        (38..=362).contains(&(x as i32)) && (38..=202).contains(&(y as i32)),
                        "opaque pixel ({x},{y}) escaped the window panel"
                    );
                    if ov.pixels[i] > 200 && ov.pixels[i + 1] > 200 && ov.pixels[i + 2] > 200 {
                        white_inside = true;
                    }
                }
            }
        }
        assert!(white_inside, "expected white body text inside the window");
    }
}

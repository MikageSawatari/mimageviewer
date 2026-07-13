//! Pure, additive scene transforms for the main-app integration.
//!
//! `scale_scene` applies a **uniform similarity scale about the origin** to a
//! whole annotation scene: every absolute length and position is multiplied by
//! `s`, while ratios (`fill_opacity`, `size_ratio`, `jag`, `amp`, `base_t`, …),
//! discrete counts (`spikes`, `sides`, `count`, `points`, `petals`, `seed`) and
//! `rotation_rad` stay unchanged. This is the §5.4 coordinate-transform contract
//! of `docs/comic-integration-plan.md`: annotations are stored in canonical
//! (non-rotated) source-pixel space, and to bake them crisply at the final
//! output resolution we scale the scene by `S = out_long / cropped_source_long`
//! before calling `bake_overlay*`.
//!
//! This is an **additive helper** (it does not change any existing baking or
//! layout behaviour). The crop translate `translate(-crop_origin)` of §5.4 is a
//! separate, trivial concern applied to the positional anchors by the caller;
//! keeping `scale_scene` a pure scale-about-origin keeps it composable and
//! unit-testable in isolation.
//!
//! ## Why `density` is divided by `s`
//!
//! `DecorationLayer::density` is "items per ~100px of outline arc-length"
//! (`tessellate.rs`: `count = round(arclen / 100 * density)`). When the scene is
//! scaled by `s`, the bubble's outline arc-length also grows by `s`, so to keep
//! the **same number** of decorations (a bigger-but-identical picture, the WYSIWYG
//! goal) the per-100px density must shrink by `s`. Every other absolute quantity
//! is multiplied by `s`; `density` is the sole inverse-scaled field.

use crate::model::{
    AnnotationKind, AnnotationObject, BubbleObject, BubbleShape, DecorationLayer, Insets,
    MessageWindowObject, NamePlate, PortraitSlot, ShadowStyle, StampObject, StrokeStyle, Tail,
    TextBlock,
};

/// Uniformly scale a whole annotation scene about the origin by `s`.
///
/// `s` must be finite and strictly positive (it is `out_long /
/// cropped_source_long`, always > 0). If `s` is non-finite or `<= 0` the input
/// is returned unchanged (defensive no-op rather than producing NaN/Inf).
///
/// `s == 1.0` is an exact identity (multiplying/dividing f32 by 1.0 is lossless),
/// so re-baking at the source resolution costs nothing.
pub fn scale_scene(objects: &[AnnotationObject], s: f32) -> Vec<AnnotationObject> {
    if !s.is_finite() || s <= 0.0 {
        return objects.to_vec();
    }
    objects.iter().map(|o| scale_object(o, s)).collect()
}

fn scale_object(o: &AnnotationObject, s: f32) -> AnnotationObject {
    AnnotationObject {
        id: o.id,
        enabled: o.enabled,
        z: o.z,
        pivot: (o.pivot.0 * s, o.pivot.1 * s),
        // Object-local rotation is unaffected by a uniform scale.
        rotation_rad: o.rotation_rad,
        kind: match &o.kind {
            AnnotationKind::Bubble(b) => AnnotationKind::Bubble(scale_bubble(b, s)),
            AnnotationKind::Text(t) => AnnotationKind::Text(scale_text(t, s)),
            AnnotationKind::MessageWindow(w) => AnnotationKind::MessageWindow(scale_window(w, s)),
            AnnotationKind::Stamp(st) => AnnotationKind::Stamp(scale_stamp(st, s)),
        },
    }
}

fn scale_stroke(stroke: &StrokeStyle, s: f32) -> StrokeStyle {
    StrokeStyle {
        color: stroke.color,
        width_px: stroke.width_px * s,
    }
}

fn scale_opt_stroke(stroke: &Option<StrokeStyle>, s: f32) -> Option<StrokeStyle> {
    stroke.as_ref().map(|st| scale_stroke(st, s))
}

fn scale_text(t: &TextBlock, s: f32) -> TextBlock {
    let scale_bg = |bg: &Option<crate::model::TextBackgroundStyle>| {
        bg.as_ref().map(|b| crate::model::TextBackgroundStyle {
            fill: b.fill,
            padding_px: b.padding_px * s,
            corner_px: b.corner_px * s,
        })
    };
    let scale_shadow = |shadow: &Option<crate::model::TextShadowStyle>| {
        shadow.as_ref().map(|sh| crate::model::TextShadowStyle {
            color: sh.color,
            offset: (sh.offset.0 * s, sh.offset.1 * s),
            blur_px: sh.blur_px * s,
            spread_px: sh.spread_px * s,
        })
    };
    let scale_glow = |glow: &Option<crate::model::TextGlowStyle>| {
        glow.as_ref().map(|g| crate::model::TextGlowStyle {
            color: g.color,
            radius_px: g.radius_px * s,
            spread_px: g.spread_px * s,
        })
    };
    let scale_echo = |echo: &Option<crate::model::TextEchoStyle>| {
        echo.as_ref().map(|e| crate::model::TextEchoStyle {
            color: e.color,
            offset: (e.offset.0 * s, e.offset.1 * s),
            count: e.count,
        })
    };
    TextBlock {
        text: t.text.clone(),
        font_key: t.font_key.clone(),
        size_px: t.size_px * s,
        color: t.color,
        orientation: t.orientation,
        align: t.align,
        v_center_ink: t.v_center_ink,
        line_gap: t.line_gap * s,
        letter_gap: t.letter_gap * s,
        outline: scale_opt_stroke(&t.outline, s),
        extra_outlines: t
            .extra_outlines
            .iter()
            .map(|st| scale_stroke(st, s))
            .collect(),
        shadow: scale_shadow(&t.shadow),
        glow: scale_glow(&t.glow),
        background: scale_bg(&t.background),
        echo: scale_echo(&t.echo),
        bold: t.bold,
        italic: t.italic,
        auto_tcy: t.auto_tcy,
        markup_enabled: t.markup_enabled,
        markup_rules: t.markup_rules.clone(),
        preset_link: t.preset_link.clone(),
    }
}

fn scale_shape(shape: BubbleShape, s: f32) -> BubbleShape {
    match shape {
        BubbleShape::Ellipse { rx, ry } => BubbleShape::Ellipse {
            rx: rx * s,
            ry: ry * s,
        },
        BubbleShape::RoundRect {
            half_w,
            half_h,
            corner_px,
        } => BubbleShape::RoundRect {
            half_w: half_w * s,
            half_h: half_h * s,
            corner_px: corner_px * s,
        },
        BubbleShape::Burst {
            rx,
            ry,
            spikes,
            jag,
            shape_seed,
        } => BubbleShape::Burst {
            rx: rx * s,
            ry: ry * s,
            spikes,
            jag, // inner-radius ratio (unitless)
            shape_seed,
        },
        BubbleShape::Cloud {
            rx,
            ry,
            lobes,
            amp,
            shape_seed,
        } => BubbleShape::Cloud {
            rx: rx * s,
            ry: ry * s,
            lobes,
            amp, // bump-depth ratio (unitless)
            shape_seed,
        },
        BubbleShape::Polygon { rx, ry, sides } => BubbleShape::Polygon {
            rx: rx * s,
            ry: ry * s,
            sides,
        },
        BubbleShape::Diamond { half_w, half_h } => BubbleShape::Diamond {
            half_w: half_w * s,
            half_h: half_h * s,
        },
        BubbleShape::Heart { rx, ry } => BubbleShape::Heart {
            rx: rx * s,
            ry: ry * s,
        },
        BubbleShape::Arrow {
            half_w,
            half_h,
            dir_rad,
            head_len_px,
            shaft_half_px,
        } => BubbleShape::Arrow {
            half_w: half_w * s,
            half_h: half_h * s,
            dir_rad, // angle, unchanged
            head_len_px: head_len_px.map(|value| value * s),
            shaft_half_px: shaft_half_px.map(|value| value * s),
        },
        BubbleShape::Soft {
            half_w,
            half_h,
            corner_px,
            shape_seed,
        } => BubbleShape::Soft {
            half_w: half_w * s,
            half_h: half_h * s,
            corner_px: corner_px * s,
            shape_seed,
        },
        BubbleShape::MotionLines {
            rx,
            ry,
            count,
            shape_seed,
        } => BubbleShape::MotionLines {
            rx: rx * s,
            ry: ry * s,
            count,
            shape_seed,
        },
        BubbleShape::SpeedLines {
            half_w,
            half_h,
            dir_rad,
            count,
            shape_seed,
        } => BubbleShape::SpeedLines {
            half_w: half_w * s,
            half_h: half_h * s,
            dir_rad,
            count,
            shape_seed,
        },
        BubbleShape::TextOnly { half_w, half_h } => BubbleShape::TextOnly {
            half_w: half_w * s,
            half_h: half_h * s,
        },
        BubbleShape::Concentration { rx, ry, shape_seed } => BubbleShape::Concentration {
            rx: rx * s,
            ry: ry * s,
            shape_seed,
        },
        BubbleShape::Strokes {
            half_w,
            half_h,
            corner_px,
            shape_seed,
        } => BubbleShape::Strokes {
            half_w: half_w * s,
            half_h: half_h * s,
            corner_px: corner_px * s,
            shape_seed,
        },
        BubbleShape::DoubleStroke {
            half_w,
            half_h,
            corner_px,
            gap_px,
        } => BubbleShape::DoubleStroke {
            half_w: half_w * s,
            half_h: half_h * s,
            corner_px: corner_px * s,
            gap_px: gap_px * s,
        },
    }
}

fn scale_tail(tail: &Tail, s: f32) -> Tail {
    Tail {
        tip: (tail.tip.0 * s, tail.tip.1 * s),
        base_t: tail.base_t, // 0..1 ratio
        base_auto: tail.base_auto,
        width_px: tail.width_px * s,
        kind: tail.kind,
    }
}

fn scale_deco(d: &DecorationLayer, s: f32) -> DecorationLayer {
    DecorationLayer {
        kind: d.kind,
        placement: d.placement,
        // Items per 100px of arc; arc grows by s, so density shrinks by s to keep
        // the decoration count (and spacing) identical (see module docs).
        density: d.density / s,
        size_ratio: d.size_ratio, // ratio of the bubble short side (unitless)
        color: d.color,
        seed: d.seed,
        outline_width: d.outline_width * s,
        outline_color: d.outline_color,
        center_color: d.center_color,
        points: d.points,
        petals: d.petals,
        gradient: d.gradient,
    }
}

fn scale_bubble(b: &BubbleObject, s: f32) -> BubbleObject {
    BubbleObject {
        shape: scale_shape(b.shape, s),
        fill: b.fill,
        fill_opacity: b.fill_opacity, // ratio
        blend: b.blend,
        outline: scale_stroke(&b.outline, s),
        tail: b.tail.as_ref().map(|t| scale_tail(t, s)),
        padding_px: b.padding_px * s,
        decorations: b.decorations.iter().map(|d| scale_deco(d, s)).collect(),
        text: scale_text(&b.text, s),
        auto_size: b.auto_size,
        merge_with_below: b.merge_with_below,
        shape_preset_link: b.shape_preset_link.clone(),
    }
}

fn scale_insets(i: &Insets, s: f32) -> Insets {
    Insets {
        left: i.left * s,
        top: i.top * s,
        right: i.right * s,
        bottom: i.bottom * s,
    }
}

fn scale_shadow(sh: &ShadowStyle, s: f32) -> ShadowStyle {
    ShadowStyle {
        color: sh.color,
        offset: (sh.offset.0 * s, sh.offset.1 * s),
    }
}

fn scale_name_plate(np: &NamePlate, s: f32) -> NamePlate {
    NamePlate {
        mode: np.mode,
        name: scale_text(&np.name, s),
        fill: np.fill,
        outline: scale_stroke(&np.outline, s),
        corner_px: np.corner_px * s,
        padding_px: np.padding_px * s,
        offset: (np.offset.0 * s, np.offset.1 * s),
    }
}

fn scale_portrait(p: &PortraitSlot, s: f32) -> PortraitSlot {
    PortraitSlot {
        side: p.side,
        width_px: p.width_px * s,
        fill: p.fill,
        outline: scale_stroke(&p.outline, s),
        margin_px: p.margin_px * s,
    }
}

fn scale_window(w: &MessageWindowObject, s: f32) -> MessageWindowObject {
    MessageWindowObject {
        size_mode: w.size_mode,
        position: w.position,
        half_w: w.half_w * s,
        half_h: w.half_h * s,
        margin_px: w.margin_px * s,
        corner_px: w.corner_px * s,
        fill_mode: w.fill_mode,
        fill: w.fill,
        fill_opacity: w.fill_opacity, // ratio
        gradient_to: w.gradient_to,
        scrim_dense_side: w.scrim_dense_side,
        frame: w.frame,
        outline: scale_stroke(&w.outline, s),
        frame_gap_px: w.frame_gap_px * s,
        shadow: w.shadow.as_ref().map(|sh| scale_shadow(sh, s)),
        text: scale_text(&w.text, s),
        padding: scale_insets(&w.padding, s),
        v_anchor: w.v_anchor,
        wrap: w.wrap,
        name_plate: scale_name_plate(&w.name_plate, s),
        portrait: scale_portrait(&w.portrait, s),
        indicator: w.indicator,
        indicator_auto: w.indicator_auto,
        style_preset_link: w.style_preset_link.clone(),
    }
}

fn scale_stamp(st: &StampObject, s: f32) -> StampObject {
    StampObject {
        source: st.source.clone(),
        half_w: st.half_w * s,
        half_h: st.half_h * s,
        opacity: st.opacity, // ratio
        flip_h: st.flip_h,
        flip_v: st.flip_v,
        outline: scale_opt_stroke(&st.outline, s),
        style_preset_link: st.style_preset_link.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        AnnotationObject, BubbleObject, DecoKind, DecoPlacement, FillMode, FrameStyle,
        IndicatorKind, NamePlateMode, Orientation, PortraitSide, Rgba, SizeMode, StampSource,
        TailKind, TextAlign, VAnchor, WindowPosition,
    };

    fn rich_bubble() -> AnnotationObject {
        let mut b = BubbleObject {
            shape: BubbleShape::Ellipse {
                rx: 160.0,
                ry: 90.0,
            },
            fill: Some(Rgba::WHITE),
            fill_opacity: 0.5,
            blend: crate::model::FillBlend::Multiply,
            outline: StrokeStyle {
                color: Rgba::BLACK,
                width_px: 4.0,
            },
            tail: Some(Tail {
                tip: (50.0, 70.0),
                base_t: 0.3,
                base_auto: false,
                width_px: 40.0,
                kind: TailKind::Spike,
            }),
            padding_px: 16.0,
            decorations: vec![DecorationLayer {
                kind: DecoKind::Flower,
                placement: DecoPlacement::Outside,
                density: 6.0,
                size_ratio: 0.2,
                color: Rgba::new(255, 200, 80, 255),
                seed: 7,
                outline_width: 2.0,
                outline_color: Rgba::BLACK,
                center_color: Rgba::WHITE,
                points: 4,
                petals: 5,
                gradient: true,
            }],
            text: TextBlock {
                text: "あ".to_string(),
                size_px: 48.0,
                line_gap: 4.0,
                letter_gap: 2.0,
                outline: Some(StrokeStyle {
                    color: Rgba::WHITE,
                    width_px: 3.0,
                }),
                ..TextBlock::default()
            },
            auto_size: false,
            merge_with_below: true,
            shape_preset_link: Some("sys:normal".to_string()),
        };
        b.text.color = Rgba::BLACK;
        AnnotationObject {
            id: 1,
            enabled: true,
            z: 5,
            pivot: (200.0, 120.0),
            rotation_rad: 0.7,
            kind: AnnotationKind::Bubble(b),
        }
    }

    fn rich_window() -> AnnotationObject {
        let w = MessageWindowObject {
            size_mode: SizeMode::Inset,
            position: WindowPosition::Free,
            half_w: 480.0,
            half_h: 120.0,
            margin_px: 48.0,
            corner_px: 14.0,
            fill_mode: FillMode::LinearGradient,
            fill: Some(Rgba::new(18, 22, 48, 235)),
            fill_opacity: 0.9,
            gradient_to: Some(Rgba::BLACK),
            scrim_dense_side: VAnchor::Bottom,
            frame: FrameStyle::DoubleLine,
            outline: StrokeStyle {
                color: Rgba::WHITE,
                width_px: 3.0,
            },
            frame_gap_px: 6.0,
            shadow: Some(ShadowStyle {
                color: Rgba::new(0, 0, 0, 110),
                offset: (6.0, 8.0),
            }),
            text: TextBlock {
                text: "テスト".to_string(),
                size_px: 36.0,
                ..TextBlock::default()
            },
            padding: Insets {
                left: 28.0,
                top: 20.0,
                right: 28.0,
                bottom: 24.0,
            },
            v_anchor: VAnchor::Center,
            wrap: true,
            name_plate: NamePlate {
                mode: NamePlateMode::Above,
                name: TextBlock {
                    text: "名前".to_string(),
                    size_px: 30.0,
                    ..TextBlock::default()
                },
                fill: Some(Rgba::new(30, 32, 44, 255)),
                outline: StrokeStyle {
                    color: Rgba::WHITE,
                    width_px: 2.0,
                },
                corner_px: 6.0,
                padding_px: 8.0,
                offset: (10.0, -40.0),
            },
            portrait: PortraitSlot {
                side: PortraitSide::Left,
                width_px: 200.0,
                fill: Some(Rgba::new(70, 74, 92, 255)),
                outline: StrokeStyle {
                    color: Rgba::BLACK,
                    width_px: 1.5,
                },
                margin_px: 12.0,
            },
            indicator: IndicatorKind::Triangle,
            indicator_auto: true,
            style_preset_link: Some("user:win".to_string()),
        };
        AnnotationObject {
            id: 2,
            enabled: true,
            z: 9,
            pivot: (300.0, 400.0),
            rotation_rad: 0.0,
            kind: AnnotationKind::MessageWindow(w),
        }
    }

    fn rich_stamp() -> AnnotationObject {
        AnnotationObject {
            id: 3,
            enabled: false,
            z: 2,
            pivot: (64.0, 32.0),
            rotation_rad: -0.4,
            kind: AnnotationKind::Stamp(StampObject {
                source: StampSource::Emoji("1f600".to_string()),
                half_w: 96.0,
                half_h: 72.0,
                opacity: 0.8,
                flip_h: true,
                flip_v: false,
                outline: Some(StrokeStyle {
                    color: Rgba::WHITE,
                    width_px: 6.0,
                }),
                style_preset_link: None,
            }),
        }
    }

    fn scene() -> Vec<AnnotationObject> {
        vec![rich_bubble(), rich_window(), rich_stamp()]
    }

    #[test]
    fn arrow_explicit_dimensions_scale_as_absolute_lengths() {
        let scaled = scale_shape(
            BubbleShape::Arrow {
                half_w: 90.0,
                half_h: 10.0,
                dir_rad: 0.25,
                head_len_px: Some(20.0),
                shaft_half_px: Some(2.5),
            },
            2.0,
        );
        assert_eq!(
            scaled,
            BubbleShape::Arrow {
                half_w: 180.0,
                half_h: 20.0,
                dir_rad: 0.25,
                head_len_px: Some(40.0),
                shaft_half_px: Some(5.0),
            }
        );
    }

    #[test]
    fn identity_scale_is_exact() {
        let s = scene();
        // s == 1.0 must be a byte-for-byte identity (no fp drift), so re-baking at
        // the source resolution is free and lossless.
        assert_eq!(scale_scene(&s, 1.0), s);
    }

    #[test]
    fn non_positive_scale_is_noop() {
        let s = scene();
        assert_eq!(scale_scene(&s, 0.0), s);
        assert_eq!(scale_scene(&s, -2.0), s);
        assert_eq!(scale_scene(&s, f32::NAN), s);
        assert_eq!(scale_scene(&s, f32::INFINITY), s);
    }

    #[test]
    fn doubling_scales_every_absolute_length() {
        let scaled = scale_scene(&scene(), 2.0);

        // ---- bubble ----
        let b_obj = &scaled[0];
        assert_eq!(b_obj.id, 1, "id preserved");
        assert!(b_obj.enabled);
        assert_eq!(b_obj.z, 5, "z preserved");
        assert_eq!(b_obj.pivot, (400.0, 240.0), "pivot doubled");
        assert_eq!(b_obj.rotation_rad, 0.7, "rotation unchanged");
        let AnnotationKind::Bubble(b) = &b_obj.kind else {
            panic!("expected bubble");
        };
        let BubbleShape::Ellipse { rx, ry } = b.shape else {
            panic!("expected ellipse");
        };
        assert_eq!((rx, ry), (320.0, 180.0), "shape rx/ry doubled");
        assert_eq!(b.fill_opacity, 0.5, "fill_opacity is a ratio (unchanged)");
        assert_eq!(b.outline.width_px, 8.0, "outline width doubled");
        assert_eq!(b.padding_px, 32.0, "padding doubled");
        assert!(!b.auto_size);
        assert!(b.merge_with_below);
        assert_eq!(b.shape_preset_link.as_deref(), Some("sys:normal"));
        let tail = b.tail.as_ref().unwrap();
        assert_eq!(tail.tip, (100.0, 140.0), "tail tip doubled");
        assert_eq!(tail.width_px, 80.0, "tail width doubled");
        assert_eq!(tail.base_t, 0.3, "tail base_t is a ratio (unchanged)");
        assert!(!tail.base_auto);
        let d = &b.decorations[0];
        assert_eq!(
            d.density, 3.0,
            "decoration density HALVED (÷s) to keep count"
        );
        assert_eq!(d.size_ratio, 0.2, "size_ratio is a ratio (unchanged)");
        assert_eq!(d.outline_width, 4.0, "decoration outline width doubled");
        assert_eq!(d.seed, 7, "seed unchanged");
        assert_eq!(d.points, 4, "points unchanged");
        assert_eq!(d.petals, 5, "petals unchanged");
        assert_eq!(b.text.size_px, 96.0, "text size doubled");
        assert_eq!(b.text.line_gap, 8.0, "line_gap doubled");
        assert_eq!(b.text.letter_gap, 4.0, "letter_gap doubled");
        assert_eq!(
            b.text.outline.as_ref().unwrap().width_px,
            6.0,
            "text outline width doubled"
        );

        // ---- window ----
        let w_obj = &scaled[1];
        assert_eq!(w_obj.pivot, (600.0, 800.0), "window pivot doubled");
        let AnnotationKind::MessageWindow(w) = &w_obj.kind else {
            panic!("expected window");
        };
        assert_eq!(w.half_w, 960.0);
        assert_eq!(w.half_h, 240.0);
        assert_eq!(w.margin_px, 96.0);
        assert_eq!(w.corner_px, 28.0);
        assert_eq!(w.fill_opacity, 0.9, "fill_opacity ratio unchanged");
        assert_eq!(w.outline.width_px, 6.0);
        assert_eq!(w.frame_gap_px, 12.0);
        let sh = w.shadow.as_ref().unwrap();
        assert_eq!(sh.offset, (12.0, 16.0), "shadow offset doubled");
        assert_eq!(w.text.size_px, 72.0);
        assert_eq!(
            (
                w.padding.left,
                w.padding.top,
                w.padding.right,
                w.padding.bottom
            ),
            (56.0, 40.0, 56.0, 48.0),
            "insets doubled per-side"
        );
        assert_eq!(w.name_plate.name.size_px, 60.0);
        assert_eq!(w.name_plate.corner_px, 12.0);
        assert_eq!(w.name_plate.padding_px, 16.0);
        assert_eq!(
            w.name_plate.offset,
            (20.0, -80.0),
            "name plate offset doubled"
        );
        assert_eq!(w.name_plate.outline.width_px, 4.0);
        assert_eq!(w.portrait.width_px, 400.0);
        assert_eq!(w.portrait.margin_px, 24.0);
        assert_eq!(w.portrait.outline.width_px, 3.0);
        assert_eq!(w.style_preset_link.as_deref(), Some("user:win"));
        assert!(w.indicator_auto);

        // ---- stamp ----
        let st_obj = &scaled[2];
        assert!(!st_obj.enabled, "enabled flag preserved");
        assert_eq!(st_obj.pivot, (128.0, 64.0), "stamp pivot doubled");
        assert_eq!(st_obj.rotation_rad, -0.4, "rotation unchanged");
        let AnnotationKind::Stamp(st) = &st_obj.kind else {
            panic!("expected stamp");
        };
        assert_eq!(st.half_w, 192.0);
        assert_eq!(st.half_h, 144.0);
        assert_eq!(st.opacity, 0.8, "opacity ratio unchanged");
        assert!(st.flip_h);
        assert!(!st.flip_v);
        assert_eq!(st.outline.as_ref().unwrap().width_px, 12.0);
        assert_eq!(st.source, StampSource::Emoji("1f600".to_string()));
    }

    #[test]
    fn standalone_text_object_scales() {
        // Direct coverage of the AnnotationKind::Text branch (the other tests only
        // reach TextBlock scaling through embedded bubble/window text).
        let tb = TextBlock {
            text: "標語".to_string(),
            size_px: 40.0,
            line_gap: 6.0,
            letter_gap: 3.0,
            color: Rgba::BLACK,
            align: TextAlign::Center,
            orientation: Orientation::Vertical,
            v_center_ink: true,
            outline: Some(StrokeStyle {
                color: Rgba::WHITE,
                width_px: 5.0,
            }),
            preset_link: Some("user:caption".to_string()),
            ..TextBlock::default()
        };
        let obj = AnnotationObject {
            id: 42,
            enabled: true,
            z: 1,
            pivot: (30.0, 60.0),
            rotation_rad: 0.25,
            kind: AnnotationKind::Text(tb),
        };
        let scaled = scale_scene(&[obj], 3.0);
        let t_obj = &scaled[0];
        assert_eq!(t_obj.id, 42);
        assert_eq!(t_obj.pivot, (90.0, 180.0), "text pivot tripled");
        assert_eq!(t_obj.rotation_rad, 0.25, "rotation unchanged");
        let AnnotationKind::Text(t) = &t_obj.kind else {
            panic!("expected standalone text");
        };
        assert_eq!(t.size_px, 120.0);
        assert_eq!(t.line_gap, 18.0);
        assert_eq!(t.letter_gap, 9.0);
        assert_eq!(t.outline.as_ref().unwrap().width_px, 15.0);
        assert_eq!(t.align, TextAlign::Center, "align enum unchanged");
        assert_eq!(
            t.orientation,
            Orientation::Vertical,
            "orientation unchanged"
        );
        assert_eq!(t.preset_link.as_deref(), Some("user:caption"));
        assert!(t.v_center_ink, "ink-centering opt-in preserved");
        assert_eq!(t.text, "標語", "text content unchanged");
    }

    #[test]
    fn round_trip_returns_to_original() {
        // scale by s then by 1/s reconstructs the original within fp epsilon for
        // every field handled symmetrically (lengths *s then /s; density /s then *s).
        let orig = scene();
        let there = scale_scene(&orig, 4.0);
        let back = scale_scene(&there, 0.25);
        // Compare a representative spread of fields numerically (PartialEq would be
        // too strict for the *4 /4 fp round-trip).
        let approx = |a: f32, b: f32| (a - b).abs() < 1e-3;
        for (o, r) in orig.iter().zip(back.iter()) {
            assert!(approx(o.pivot.0, r.pivot.0) && approx(o.pivot.1, r.pivot.1));
        }
        if let (AnnotationKind::Bubble(o), AnnotationKind::Bubble(r)) =
            (&orig[0].kind, &back[0].kind)
        {
            assert!(approx(o.outline.width_px, r.outline.width_px));
            assert!(approx(o.decorations[0].density, r.decorations[0].density));
            assert!(approx(o.text.size_px, r.text.size_px));
        } else {
            panic!("bubble shape changed across round trip");
        }
    }

    #[test]
    fn every_bubble_shape_scales_its_lengths() {
        // Guards against forgetting a shape variant: every length field of every
        // BubbleShape must double under s=2 while counts/ratios/angles stay.
        let shapes = [
            BubbleShape::Ellipse { rx: 10.0, ry: 20.0 },
            BubbleShape::RoundRect {
                half_w: 10.0,
                half_h: 20.0,
                corner_px: 5.0,
            },
            BubbleShape::Burst {
                rx: 10.0,
                ry: 20.0,
                spikes: 12,
                jag: 0.4,
                shape_seed: 3,
            },
            BubbleShape::Cloud {
                rx: 10.0,
                ry: 20.0,
                lobes: 8,
                amp: 0.3,
                shape_seed: 3,
            },
            BubbleShape::Polygon {
                rx: 10.0,
                ry: 20.0,
                sides: 6,
            },
            BubbleShape::Diamond {
                half_w: 10.0,
                half_h: 20.0,
            },
            BubbleShape::Heart { rx: 10.0, ry: 20.0 },
            BubbleShape::Arrow {
                half_w: 10.0,
                half_h: 20.0,
                dir_rad: 1.2,
                head_len_px: None,
                shaft_half_px: None,
            },
            BubbleShape::Soft {
                half_w: 10.0,
                half_h: 20.0,
                corner_px: 5.0,
                shape_seed: 3,
            },
            BubbleShape::MotionLines {
                rx: 10.0,
                ry: 20.0,
                count: 64,
                shape_seed: 3,
            },
            BubbleShape::SpeedLines {
                half_w: 10.0,
                half_h: 20.0,
                dir_rad: 0.5,
                count: 64,
                shape_seed: 3,
            },
            BubbleShape::TextOnly {
                half_w: 10.0,
                half_h: 20.0,
            },
            BubbleShape::Concentration {
                rx: 10.0,
                ry: 20.0,
                shape_seed: 3,
            },
            BubbleShape::Strokes {
                half_w: 10.0,
                half_h: 20.0,
                corner_px: 5.0,
                shape_seed: 3,
            },
            BubbleShape::DoubleStroke {
                half_w: 10.0,
                half_h: 20.0,
                corner_px: 5.0,
                gap_px: 4.0,
            },
        ];
        for shape in shapes {
            match scale_shape(shape, 2.0) {
                BubbleShape::Ellipse { rx, ry } => assert_eq!((rx, ry), (20.0, 40.0)),
                BubbleShape::RoundRect {
                    half_w,
                    half_h,
                    corner_px,
                } => assert_eq!((half_w, half_h, corner_px), (20.0, 40.0, 10.0)),
                BubbleShape::Burst {
                    rx,
                    ry,
                    spikes,
                    jag,
                    shape_seed,
                } => {
                    assert_eq!((rx, ry), (20.0, 40.0));
                    assert_eq!((spikes, jag, shape_seed), (12, 0.4, 3));
                }
                BubbleShape::Cloud {
                    rx,
                    ry,
                    lobes,
                    amp,
                    shape_seed,
                } => {
                    assert_eq!((rx, ry), (20.0, 40.0));
                    assert_eq!((lobes, amp, shape_seed), (8, 0.3, 3));
                }
                BubbleShape::Polygon { rx, ry, sides } => {
                    assert_eq!((rx, ry), (20.0, 40.0));
                    assert_eq!(sides, 6);
                }
                BubbleShape::Diamond { half_w, half_h } => {
                    assert_eq!((half_w, half_h), (20.0, 40.0))
                }
                BubbleShape::Heart { rx, ry } => assert_eq!((rx, ry), (20.0, 40.0)),
                BubbleShape::Arrow {
                    half_w,
                    half_h,
                    dir_rad,
                    ..
                } => {
                    assert_eq!((half_w, half_h), (20.0, 40.0));
                    assert_eq!(dir_rad, 1.2, "angle unchanged");
                }
                BubbleShape::Soft {
                    half_w,
                    half_h,
                    corner_px,
                    shape_seed,
                } => {
                    assert_eq!((half_w, half_h, corner_px), (20.0, 40.0, 10.0));
                    assert_eq!(shape_seed, 3);
                }
                BubbleShape::MotionLines {
                    rx,
                    ry,
                    count,
                    shape_seed,
                } => {
                    assert_eq!((rx, ry), (20.0, 40.0));
                    assert_eq!((count, shape_seed), (64, 3));
                }
                BubbleShape::SpeedLines {
                    half_w,
                    half_h,
                    dir_rad,
                    count,
                    shape_seed,
                } => {
                    assert_eq!((half_w, half_h), (20.0, 40.0));
                    assert_eq!(dir_rad, 0.5, "angle unchanged");
                    assert_eq!((count, shape_seed), (64, 3));
                }
                BubbleShape::TextOnly { half_w, half_h } => {
                    assert_eq!((half_w, half_h), (20.0, 40.0))
                }
                BubbleShape::Concentration { rx, ry, shape_seed } => {
                    assert_eq!((rx, ry), (20.0, 40.0));
                    assert_eq!(shape_seed, 3);
                }
                BubbleShape::Strokes {
                    half_w,
                    half_h,
                    corner_px,
                    shape_seed,
                } => {
                    assert_eq!((half_w, half_h, corner_px), (20.0, 40.0, 10.0));
                    assert_eq!(shape_seed, 3);
                }
                BubbleShape::DoubleStroke {
                    half_w,
                    half_h,
                    corner_px,
                    gap_px,
                } => assert_eq!((half_w, half_h, corner_px, gap_px), (20.0, 40.0, 10.0, 8.0)),
            }
        }
    }

    #[test]
    fn scaled_bubble_bakes_crisp_at_higher_resolution() {
        // End-to-end: a scene baked at 2x scale into a 2x canvas must produce a
        // comparable opaque-pixel footprint to baking at 1x (i.e. it's the same
        // picture, just larger). Uses a Windows JP font if available.
        use crate::FontSet;
        use crate::raster::bake_overlay;
        let candidates = [
            r"C:\Windows\Fonts\YuGothM.ttc",
            r"C:\Windows\Fonts\meiryo.ttc",
            r"C:\Windows\Fonts\msgothic.ttc",
        ];
        let mut fonts = FontSet::new();
        let mut loaded = false;
        for p in candidates {
            if let Ok(bytes) = std::fs::read(p) {
                if let Ok(f) = crate::LoadedFont::from_bytes("test", bytes) {
                    fonts.insert(f);
                    loaded = true;
                    break;
                }
            }
        }
        if !loaded {
            eprintln!("skip: no Windows JP font available");
            return;
        }
        let mut obj = rich_bubble();
        if let AnnotationKind::Bubble(b) = &mut obj.kind {
            b.text.font_key = "test".to_string();
        }
        let base = vec![obj];
        let ov1 = bake_overlay(&base, 480, 360, &fonts);
        let scaled = scale_scene(&base, 2.0);
        let ov2 = bake_overlay(&scaled, 960, 720, &fonts);
        let cov1 = ov1.pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        let cov2 = ov2.pixels.chunks_exact(4).filter(|p| p[3] > 0).count();
        assert!(cov1 > 0 && cov2 > 0, "both bakes produced ink");
        // 2x linear → ~4x area. Allow a wide band (AA, halo, decorations differ a
        // little) but confirm it really grew roughly quadratically, not stayed 1x.
        let ratio = cov2 as f32 / cov1 as f32;
        assert!(
            (2.5..=6.0).contains(&ratio),
            "2x-scaled bake should cover ~4x the pixels, got {ratio:.2}x ({cov1} -> {cov2})"
        );
    }
}

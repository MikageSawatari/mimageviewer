//! Fixed pixels from the pre-region annotation painter, not a second invocation
//! of today's painter. Capture: extracted `crates/comic-core` from `fce65bb15`
//! using `git show`, made its Cargo.toml a standalone workspace (rust-version
//! literal), copied the repository Cargo.lock to pin dependencies, copied this
//! test into it, changed only the include_bytes path and
//! printed FNV-1a-64(bake_overlay(...).pixels). Ran `cargo test --offline
//! --manifest-path target/t1-rereview/comic-old/Cargo.toml --test legacy_text_pixels
//! -- --nocapture`. Frozen values below were then verified on the current crate.
//! Fixture is the repository's OFL ZenKurenaido-Regular.ttf, so results do not
//! depend on the Windows font version or silently skip when fonts are absent.
use comic_core::*;
fn hash(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf29ce484222325_u64, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x100000001b3)
    })
}
fn fonts() -> FontSet {
    let mut fonts = FontSet::new();
    fonts.insert(
        LoadedFont::from_bytes(
            "fixture",
            include_bytes!("../../../tools/comic_lab/assets/fonts/ZenKurenaido-Regular.ttf")
                .to_vec(),
        )
        .unwrap(),
    );
    fonts
}
fn block(decorated: bool) -> TextBlock {
    let mut b = TextBlock {
        text: "Á 日本語 ABC\n境界 gj".into(),
        font_key: "fixture".into(),
        size_px: 26.25,
        color: Rgba::new(197, 53, 121, 173),
        letter_gap: -0.75,
        line_gap: -3.25,
        ..Default::default()
    };
    if decorated {
        b.outline = Some(StrokeStyle {
            color: Rgba::new(31, 72, 220, 189),
            width_px: 2.25,
        });
        b.extra_outlines = vec![StrokeStyle {
            color: Rgba::new(91, 221, 45, 127),
            width_px: 4.5,
        }];
        b.shadow = Some(TextShadowStyle {
            color: Rgba::new(21, 42, 63, 117),
            offset: (-3.75, 5.25),
            spread_px: 1.25,
            blur_px: 6.5,
        });
        b.echo = Some(TextEchoStyle {
            color: Rgba::new(55, 130, 205, 103),
            offset: (5.25, -2.75),
            count: 3,
        });
        b.glow = Some(TextGlowStyle {
            color: Rgba::new(155, 95, 215, 111),
            radius_px: 5.5,
            spread_px: 1.75,
        });
        b.background = Some(TextBackgroundStyle {
            fill: Rgba::new(42, 85, 127, 137),
            padding_px: 5.25,
            corner_px: 4.75,
        });
    }
    b
}
#[test]
fn legacy_unclipped_annotation_pixels_at_fractional_origin_and_all_edges() {
    let fonts = fonts();
    // Negative/fractional origin, left, top, right, bottom, interior.
    let origins = [
        (-8.75, -13.5),
        (-5.5, 16.25),
        (18.5, -14.75),
        (90.25, 18.5),
        (10.75, 61.5),
        (8.125, 12.75),
    ];
    let expected = [
        [
            0x695f31630a7ae4b1,
            0xab8e78fc8caa214c,
            0xdf1c62c2ee18c23c,
            0xe10a359db981b10e,
            0xaf3ef5c19bf98298,
            0x596a4f255724e607,
        ],
        [
            0x233dbfbc4c4ca4da,
            0x6ffc4f15457bb2c0,
            0x325dbf03f58838f1,
            0xb28c22b36c4980b9,
            0x6ff4efc664fbfa93,
            0x6c250f8c4ef15c33,
        ],
    ];
    for (d, row) in expected.iter().enumerate() {
        for (i, origin) in origins.into_iter().enumerate() {
            let ov = bake_overlay(
                &[AnnotationObject::new_text(1, origin, block(d == 1))],
                128,
                96,
                &fonts,
            );
            let actual = hash(&ov.pixels);
            assert!(ov.pixels.chunks_exact(4).any(|p| p[3] > 0));
            assert_eq!(actual, row[i], "decorated={d}, origin={origin:?}");
        }
    }
}
#[test]
fn legacy_message_window_pixels_with_partial_content_clip_and_translucent_background() {
    let fonts = fonts();
    // Panel and overflowing text cross each overlay edge. wrap=false guarantees
    // glyphs/effects hit clip=Some(content_rect), including fractional bounds.
    let pivots = [
        (61.25, 45.75),
        (10.25, 45.75),
        (61.25, 4.75),
        (119.75, 45.75),
        (61.25, 91.25),
    ];
    let expected = [
        [
            0x740ee42877d9530b,
            0xe7e6e2cbd6846ba0,
            0x49922c5c5de3a503,
            0xa47e74475a166281,
            0x0807e5b88dbc6212,
        ],
        [
            0xc7971f420a3c1c5a,
            0xdb3aeb20a3e8cc0e,
            0x176eaed601a78b1c,
            0x89338560ff8d3bde,
            0x9531ac4b4d914a22,
        ],
    ];
    for (d, row) in expected.iter().enumerate() {
        for (i, pivot) in pivots.into_iter().enumerate() {
            let win = MessageWindowObject {
                size_mode: SizeMode::Inset,
                position: WindowPosition::Free,
                half_w: 40.25,
                half_h: 25.75,
                corner_px: 6.5,
                fill: Some(Rgba::new(15, 37, 59, 101)),
                outline: StrokeStyle {
                    color: Rgba::new(97, 127, 157, 143),
                    width_px: 1.25,
                },
                padding: Insets::uniform(3.25),
                wrap: false,
                v_anchor: VAnchor::Center,
                text: block(d == 1),
                ..Default::default()
            };
            let ov = bake_overlay(
                &[AnnotationObject::new_message_window(1, pivot, win)],
                128,
                96,
                &fonts,
            );
            let actual = hash(&ov.pixels);
            assert!(ov.pixels.chunks_exact(4).any(|p| p[3] > 0));
            assert_eq!(actual, row[i], "decorated={d}, pivot={pivot:?}");
        }
    }
}

#[test]
fn prepared_regions_share_coverage_and_survive_unrelated_overlay_bakes() {
    let fonts = fonts();
    for origin in [(-8.75, -13.5), (8.125, 12.75), (6145.25, 143.125)] {
        let b = block(true);
        let glyph = TextBlock {
            color: Rgba::WHITE,
            outline: None,
            extra_outlines: Vec::new(),
            shadow: None,
            glow: None,
            echo: None,
            background: None,
            ..b.clone()
        };
        let font = fonts.get("fixture").unwrap();
        let layout = layout_text(&b, font);
        let prepared = prepare_text_regions(&layout, font, origin, &[&b, &glyph]);
        // This resets the global generation. Prepared programs own their glyphs
        // and remain valid; replay neither reshapes nor rerasterizes glyphs.
        let _ = bake_overlay(
            &[AnnotationObject::new_text(2, (0.0, 0.0), block(false))],
            32,
            32,
            &fonts,
        );
        for (surface, block) in prepared.iter().zip([&b, &glyph]) {
            assert!(surface.retained_bytes() > 0);
            for xy in [(-16, -16), (0, 0), (60, 32), (6140, 140)] {
                let expected = bake_text_region(block, origin, xy, 97, 67, &fonts);
                let actual = surface.bake_region(xy, 97, 67);
                assert_eq!(
                    actual.pixels, expected.pixels,
                    "origin={origin:?}, region={xy:?}"
                );
            }
        }
    }
}

use super::*;
use image::{Rgba, RgbaImage};

fn options(kind: CombineLayout) -> CombineOptions {
    CombineOptions {
        layout: kind,
        ..Default::default()
    }
}

fn calculate(o: &CombineOptions, dims: &[(u32, u32)]) -> Layout {
    layout(o, dims, o.normalized().labels.band_height()).unwrap()
}

fn close(actual: f64, expected: f64) {
    assert!((actual - expected).abs() < 1e-6, "{actual} != {expected}");
}

fn spec(text: Option<&str>) -> CombineItemSpec {
    CombineItemSpec {
        source: CombineSourceRef::File {
            path: "C:\\combine-test\\source.png".into(),
        },
        label_override: text.map(str::to_owned),
    }
}

fn fonts() -> comic_core::FontSet {
    // The same local Windows font loader used by export_batch watermark tests.
    crate::comic_overlay::load_comic_fonts().expect("Windows annotation font")
}

fn no_labels(count: usize) -> LabelLayout {
    LabelLayout {
        labels: vec![None; count],
        not_drawn: 0,
    }
}

#[test]
fn serde_defaults_normalization_and_recipe_roundtrip() {
    let defaults: CombineOptions = serde_json::from_str("{}").unwrap();
    assert_eq!(defaults, CombineOptions::default());
    assert_eq!(
        serde_json::from_str::<LabelOptions>("{}").unwrap(),
        LabelOptions::default()
    );
    let mut o = defaults;
    o.gap_x = 999;
    o.gap_y = 17;
    o.margin = 999;
    o.border.width_px = 999;
    o.corner_radius = 999;
    o.grid_columns = Some(0);
    o.canvas_ratio = Some((0, 200));
    o.labels.text_size_px = 0;
    o.labels.fill_opacity = 255;
    o.labels.number_start = u32::MAX;
    o.labels.overlay_margin_px = u32::MAX;
    o.output_size = OutputSize::Width(0);
    o.background = Background::Transparent;
    o.format = CombineFormat::Jpeg { quality: 0 };
    let o = o.normalized();
    assert_eq!(
        (
            o.gap_x,
            o.gap_y,
            o.margin,
            o.corner_radius,
            o.border.width_px
        ),
        (500, 17, 500, 500, 100)
    );
    assert_eq!(o.canvas_ratio, Some((1, 100)));
    assert_eq!(o.grid_columns, Some(1));
    assert_eq!(o.labels.text_size_px, 8);
    assert_eq!(o.labels.number_start, 999);
    assert_eq!(o.labels.fill_opacity, 100);
    assert_eq!(o.labels.overlay_margin_px, 500);
    assert_eq!(o.output_size, OutputSize::Width(16));
    assert_eq!(o.background, Background::Color([255; 3]));
    let recipe = CombineRecipe {
        v: 1,
        options: o,
        format: CombineFormat::Png,
        items: vec![
            spec(Some("商品A")),
            CombineItemSpec {
                source: CombineSourceRef::ZipEntry {
                    zip_path: "C:\\test.zip".into(),
                    entry_name: "folder/a.png".into(),
                },
                label_override: None,
            },
            CombineItemSpec {
                source: CombineSourceRef::PdfPage {
                    pdf_path: "C:\\test.pdf".into(),
                    page_num: 2,
                },
                label_override: Some(String::new()),
            },
        ],
    };
    let json = serde_json::to_string(&recipe).unwrap();
    assert!(!json.contains("\"name\""));
    assert_eq!(
        serde_json::from_str::<CombineRecipe>(&json).unwrap(),
        recipe
    );
    assert_eq!(
        serde_json::from_str::<CombineFormat>(r#"{"Jpeg":{}}"#).unwrap(),
        CombineFormat::Jpeg { quality: 95 }
    );
    assert_eq!(
        serde_json::from_str::<CombineFormat>(r#"{"Webp":{}}"#).unwrap(),
        CombineFormat::Webp {
            quality: 90,
            lossless: false
        }
    );
}

#[test]
fn band_height_depends_on_badge_style_and_placement() {
    let mut l = LabelOptions::default();
    assert_eq!(l.band_height(), None);
    l.placement = LabelPlacement::Band;
    l.text_size_px = 13;
    assert_eq!(l.band_height(), Some(21));
    for style in [BadgeStyle::Box, BadgeStyle::Pill] {
        l.style = style;
        assert_eq!(l.band_height(), Some(26));
    }
    l.placement = LabelPlacement::Overlay;
    assert_eq!(l.band_height(), None);
}

#[test]
fn five_layouts_directions_gaps_margins_and_bands_follow_affine_formulas() {
    for kind in [
        CombineLayout::Row,
        CombineLayout::Column,
        CombineLayout::Grid,
        CombineLayout::JustifiedRows,
        CombineLayout::JustifiedColumns,
    ] {
        for direction in [CombineDirection::LeftToRight, CombineDirection::RightToLeft] {
            for (gx, gy, m, t) in [(0, 0, 0, 0), (11, 13, 7, 0), (11, 13, 7, 20)] {
                let mut o = options(kind);
                o.direction = direction;
                o.gap_x = gx;
                o.gap_y = gy;
                o.margin = m;
                o.grid_columns = Some(2);
                o.split = Split::Counts(vec![2, 2]);
                let l = layout(&o, &[(100, 100); 4], Some(t)).unwrap();
                let (w, h) = match kind {
                    CombineLayout::Row => (400 + 3 * gx + 2 * m, 100 + t + 2 * m),
                    CombineLayout::Column => (100 + 2 * m, 400 + 4 * t + 3 * gy + 2 * m),
                    _ => (200 + gx + 2 * m, 200 + 2 * t + gy + 2 * m),
                };
                assert_eq!(l.canvas_size, (w, h), "{kind:?}");
                for (i, card) in l.cards.iter().enumerate() {
                    let (x, y) = match kind {
                        CombineLayout::Row => (m + i as u32 * (100 + gx), m),
                        CombineLayout::Column => (m, m + i as u32 * (100 + t + gy)),
                        CombineLayout::JustifiedColumns => (
                            m + (i / 2) as u32 * (100 + gx),
                            m + (i % 2) as u32 * (100 + t + gy),
                        ),
                        _ => (
                            m + (i % 2) as u32 * (100 + gx),
                            m + (i / 2) as u32 * (100 + t + gy),
                        ),
                    };
                    let x = if direction == CombineDirection::RightToLeft
                        && kind != CombineLayout::Column
                    {
                        w - x - 100
                    } else {
                        x
                    };
                    close(card.card.x, x as f64);
                    close(card.card.y, y as f64);
                    close(card.image.width, 100.0);
                    close(card.image.height, 100.0);
                    close(card.card.height, (100 + t) as f64);
                    assert_eq!(card.band.is_some(), t > 0);
                    if let Some(band) = card.band {
                        close(band.y, card.image.bottom());
                        close(band.height, t as f64);
                    }
                }
            }
        }
    }
}

#[test]
fn band_column_and_unequal_justified_columns_keep_bands_below_images() {
    let mut o = options(CombineLayout::Column);
    o.gap_y = 10;
    o.gap_x = 8;
    o.margin = 5;
    let dims = [(100, 100), (100, 200), (100, 100)];
    let l = layout(&o, &dims, Some(20)).unwrap();
    assert_eq!(l.canvas_size, (110, 490));
    close(l.cards[1].image.y, 135.0);
    close(l.cards[2].image.y, 365.0);
    o.layout = CombineLayout::JustifiedColumns;
    o.split = Split::Counts(vec![1, 2]);
    o.output_size = OutputSize::Height(400);
    let l = layout(&o, &[(100, 100); 3], Some(20)).unwrap();
    // Inner height 390: first width 370, second (390-10-40)/2 = 170.
    assert_eq!(l.canvas_size, (558, 400));
    close(l.cards[0].image.width, 370.0);
    close(l.cards[1].image.width, 170.0);
    close(l.cards[1].image.x, 383.0);
    close(l.cards[2].image.y, 205.0);
    close(l.cards[0].band.unwrap().bottom(), 395.0);
    close(l.cards[2].band.unwrap().bottom(), 395.0);
}

#[test]
fn justified_negative_row_is_rejected_even_when_canvas_height_is_positive() {
    let mut o = options(CombineLayout::JustifiedRows);
    o.split = Split::Counts(vec![2, 1]);
    o.gap_x = 500;
    o.output_size = OutputSize::Width(400);
    assert_eq!(
        layout(&o, &[(100, 100); 3], None),
        Err(LayoutError::NonPositiveImageArea { group: 0 })
    );
}

#[test]
fn output_sizes_and_fixed_ratio_both_hold_for_all_layouts() {
    for kind in [
        CombineLayout::Row,
        CombineLayout::Column,
        CombineLayout::Grid,
        CombineLayout::JustifiedRows,
        CombineLayout::JustifiedColumns,
    ] {
        let mut o = options(kind);
        o.split = Split::Counts(vec![2, 2]);
        o.grid_columns = Some(2);
        o.margin = 5;
        o.gap_x = 10;
        o.gap_y = 10;
        for ratio in [None, Some((2, 3))] {
            o.canvas_ratio = ratio;
            for size in [
                OutputSize::Original,
                OutputSize::Width(500),
                OutputSize::Height(600),
                OutputSize::LongEdge(800),
            ] {
                o.output_size = size;
                let l = layout(&o, &[(100, 100); 4], Some(20)).unwrap();
                match size {
                    OutputSize::Original => {}
                    OutputSize::Width(w) => assert_eq!(l.canvas_size.0, w),
                    OutputSize::Height(h) => assert_eq!(l.canvas_size.1, h),
                    OutputSize::LongEdge(e) => assert_eq!(l.canvas_size.0.max(l.canvas_size.1), e),
                }
                if ratio.is_some() {
                    assert!(
                        (l.canvas_size.0 as f64 - l.canvas_size.1 as f64 * 2.0 / 3.0).abs() <= 1.0
                    );
                }
                for card in l.cards {
                    if let Some(band) = card.band {
                        close(band.height, 20.0);
                    }
                }
            }
        }
    }
}

#[test]
fn original_fixed_ratio_preserves_the_layouts_kept_axis() {
    for kind in [
        CombineLayout::Row,
        CombineLayout::Column,
        CombineLayout::Grid,
        CombineLayout::JustifiedRows,
        CombineLayout::JustifiedColumns,
    ] {
        let mut o = options(kind);
        o.split = Split::Counts(vec![2, 2]);
        o.grid_columns = Some(2);
        o.gap_x = 8;
        o.gap_y = 12;
        o.margin = 7;
        let natural = layout(&o, &[(100, 100); 4], Some(20)).unwrap();
        o.canvas_ratio = Some((2, 3));
        let fixed = layout(&o, &[(100, 100); 4], Some(20)).unwrap();
        if matches!(
            kind,
            CombineLayout::Column | CombineLayout::JustifiedColumns
        ) {
            assert_eq!(natural.canvas_size.1, fixed.canvas_size.1);
        } else {
            assert_eq!(natural.canvas_size.0, fixed.canvas_size.0);
        }
    }
}

#[test]
fn one_square_height_100_ratio_two_to_one_solves_s_and_k() {
    let mut o = options(CombineLayout::Row);
    o.output_size = OutputSize::Height(100);
    o.canvas_ratio = Some((2, 1));
    let l = calculate(&o, &[(100, 100)]);
    assert_eq!(l.canvas_size, (200, 100));
    close(l.s, 200.0);
    close(l.thickness, 0.5);
    assert_eq!(l.cards[0].source_crop, Rect::new(0.0, 25.0, 100.0, 50.0));
    close(l.max_crop_percent, 50.0);
    assert!(l.any_upscaled);
}

#[test]
fn fixed_ratio_crops_both_axes_with_all_nine_anchors() {
    for anchor in Anchor9::ALL {
        let (ax, ay) = anchor.fractions();
        for kind in [
            CombineLayout::Row,
            CombineLayout::Column,
            CombineLayout::JustifiedRows,
            CombineLayout::JustifiedColumns,
        ] {
            let mut o = options(kind);
            o.anchor = anchor;
            for ratio in [(2, 1), (1, 2)] {
                o.canvas_ratio = Some(ratio);
                let l = calculate(&o, &[(100, 100)]);
                let crop = l.cards[0].source_crop;
                if ratio == (2, 1) {
                    close(crop.width, 100.0);
                    close(crop.height, 50.0);
                    close(crop.y, 50.0 * ay);
                } else {
                    close(crop.width, 50.0);
                    close(crop.height, 100.0);
                    close(crop.x, 50.0 * ax);
                }
                close(l.max_crop_percent, 50.0);
            }
        }
    }
}

#[test]
fn automatic_sample_is_two_two_and_larger_groups_are_later() {
    let l = calculate(
        &options(CombineLayout::JustifiedRows),
        &[(300, 400), (400, 300), (300, 400), (300, 400)],
    );
    assert_eq!(l.split, vec![2, 2]);
    for kind in [
        CombineLayout::JustifiedRows,
        CombineLayout::JustifiedColumns,
    ] {
        let l = calculate(&options(kind), &[(100, 100); 5]);
        assert_eq!(l.split, vec![2, 3]);
        let l = calculate(&options(kind), &[(100, 100); 2]);
        assert_eq!(l.split, vec![2]); // equal logarithmic scores: fewer groups
    }
}

#[test]
fn automatic_fixed_ratio_solves_each_candidate_and_skips_invalid_two_two() {
    let mut o = options(CombineLayout::JustifiedRows);
    o.canvas_ratio = Some((3, 5));
    let l = layout(&o, &[(100, 100); 4], Some(180)).unwrap();
    assert_eq!(l.split, vec![4]);
    close(l.thickness, 4.866666666666667);
    o.split = Split::Counts(vec![2, 2]);
    assert_eq!(
        layout(&o, &[(100, 100); 4], Some(180)),
        Err(LayoutError::NonPositiveThickness)
    );
}

#[test]
fn automatic_fixed_ratio_selects_multiple_rows_columns_and_grid_columns() {
    for kind in [
        CombineLayout::JustifiedRows,
        CombineLayout::JustifiedColumns,
        CombineLayout::Grid,
    ] {
        let mut o = options(kind);
        o.canvas_ratio = Some((1, 1));
        let l = calculate(&o, &[(100, 100); 4]);
        assert_eq!(l.split, vec![2, 2], "{kind:?}");
    }
}

#[test]
fn rtl_mirrors_every_destination_rect_and_grid_empty_cells_move_left() {
    for kind in [
        CombineLayout::Row,
        CombineLayout::Column,
        CombineLayout::Grid,
        CombineLayout::JustifiedRows,
        CombineLayout::JustifiedColumns,
    ] {
        let mut o = options(kind);
        o.anchor = Anchor9::TopLeft;
        o.grid_columns = Some(2);
        o.split = Split::Counts(vec![1, 2]);
        o.gap_x = 17;
        o.margin = 9;
        let dims = [(100, 100), (200, 100), (100, 200)];
        let left = layout(&o, &dims, Some(20)).unwrap();
        o.direction = CombineDirection::RightToLeft;
        let right = layout(&o, &dims, Some(20)).unwrap();
        for (a, b) in left.cards.iter().zip(&right.cards) {
            for (a, b) in [
                (a.card, b.card),
                (a.image, b.image),
                (a.band.unwrap(), b.band.unwrap()),
            ] {
                close(
                    b.x,
                    if kind == CombineLayout::Column {
                        a.x
                    } else {
                        left.canvas_size.0 as f64 - a.right()
                    },
                );
                close(a.y, b.y);
                close(a.width, b.width);
                close(a.height, b.height);
            }
            assert_eq!(a.source_crop, b.source_crop);
        }
        if kind == CombineLayout::Grid {
            assert!(right.cards[2].card.x > left.cards[2].card.x);
        }
    }
}

#[test]
fn line_largest_smallest_none_and_cross_axis_alignment() {
    for kind in [CombineLayout::Row, CombineLayout::Column] {
        let mut o = options(kind);
        let dims = [(100, 100), (50, 200)];
        for size in [LineSize::Largest, LineSize::Smallest, LineSize::None] {
            o.line_size = size;
            let l = calculate(&o, &dims);
            let expected = match (kind, size) {
                (CombineLayout::Row, LineSize::Largest) => (250, 200),
                (CombineLayout::Row, LineSize::Smallest) => (125, 100),
                (CombineLayout::Row, LineSize::None) => (150, 200),
                (CombineLayout::Column, LineSize::Largest) => (100, 500),
                (CombineLayout::Column, LineSize::Smallest) => (50, 250),
                _ => (100, 300),
            };
            assert_eq!(l.canvas_size, expected);
        }
        o.line_size = LineSize::None;
        for align in [LineAlign::Start, LineAlign::Center, LineAlign::End] {
            o.line_align = align;
            let l = calculate(&o, &dims);
            if kind == CombineLayout::Row {
                close(l.cards[0].card.y, 100.0 * align.fraction());
            } else {
                close(l.cards[1].card.x, 50.0 * align.fraction());
            }
        }
        o.canvas_ratio = Some((1, 1));
        assert_eq!(
            layout(&o, &dims, None),
            Err(LayoutError::RatioWithUnmatchedLine)
        );
    }
}

#[test]
fn grid_median_area_base_fit_fill_and_nine_anchors() {
    let mut o = options(CombineLayout::Grid);
    o.grid_columns = Some(2);
    o.cell_ratio = CellRatio::Ratio(2, 1);
    let dims = [(100, 50), (100, 100), (200, 100), (200, 200)];
    close(calculate(&o, &dims).s, 200.0);
    o.cell_fit = CellFit::Fill;
    close(calculate(&o, &dims).s, 100.0);
    o.grid_columns = Some(1);
    o.cell_ratio = CellRatio::Ratio(1, 1);
    for anchor in Anchor9::ALL {
        o.anchor = anchor;
        let (ax, ay) = anchor.fractions();
        o.cell_fit = CellFit::Fit;
        let l = calculate(&o, &[(200, 100)]);
        assert_eq!(l.cards[0].image, Rect::new(0.0, 100.0 * ay, 200.0, 100.0));
        o.cell_fit = CellFit::Fill;
        let l = calculate(&o, &[(200, 100)]);
        assert_eq!(
            l.cards[0].source_crop,
            Rect::new(100.0 * ax, 0.0, 100.0, 100.0)
        );
        close(l.max_crop_percent, 50.0);
    }
}

#[test]
fn fixed_pixel_gaps_bands_margin_and_border_do_not_scale_with_output_size() {
    let mut o = options(CombineLayout::Row);
    o.gap_x = 11;
    o.margin = 7;
    o.border.width_px = 4;
    for width in [400, 800] {
        o.output_size = OutputSize::Width(width);
        let l = layout(&o, &[(100, 100); 2], Some(30)).unwrap();
        close(l.cards[1].card.x - l.cards[0].card.right(), 11.0);
        close(l.cards[0].card.x, 7.0);
        close(l.cards[0].band.unwrap().height, 30.0);
        let mut canvas = create_canvas(&l, &o, 1.0).unwrap();
        draw_card(
            &mut canvas,
            &l.cards[0],
            &RgbaImage::from_pixel(100, 100, Rgba([0, 0, 0, 255])),
            &o,
            1.0,
        )
        .unwrap();
        for x in 7..11 {
            assert_eq!(*canvas.get_pixel(x, 20), Rgba([255; 4]));
        }
        assert_eq!(*canvas.get_pixel(11, 20), Rgba([0, 0, 0, 255]));
    }
}

#[test]
fn invalid_dimensions_splits_fixed_parts_and_format_limits_are_distinct() {
    let mut o = options(CombineLayout::Row);
    assert_eq!(layout(&o, &[], None), Err(LayoutError::Empty));
    assert_eq!(
        layout(&o, &[(1, 1); 101], None),
        Err(LayoutError::TooManyImages)
    );
    assert_eq!(
        layout(&o, &[(0, 1)], None),
        Err(LayoutError::InvalidSourceSize { index: 0 })
    );
    assert_eq!(
        layout(&o, &[(20000, 20000)], None),
        Err(LayoutError::CanvasPixelLimit)
    );
    o.format = CombineFormat::Webp {
        quality: 90,
        lossless: false,
    };
    assert_eq!(
        layout(&o, &[(16384, 1)], None),
        Err(LayoutError::WebpSideLimit)
    );
    o.format = CombineFormat::Jpeg { quality: 95 };
    assert_eq!(
        layout(&o, &[(65501, 1)], None),
        Err(LayoutError::JpegSideLimit)
    );
    o.format = CombineFormat::Png;
    o.margin = 500;
    o.output_size = OutputSize::Width(400);
    assert!(layout(&o, &[(100, 100)], None).is_err());
    o.margin = 0;
    o.canvas_ratio = Some((1, 1));
    o.output_size = OutputSize::Height(100);
    assert_eq!(
        layout(&o, &[(100, 100)], Some(180)),
        Err(LayoutError::NonPositiveThickness)
    );
    o = options(CombineLayout::JustifiedRows);
    for counts in [vec![], vec![1, 1], vec![0, 3], vec![101]] {
        o.split = Split::Counts(counts);
        assert_eq!(
            layout(&o, &[(100, 100); 3], None),
            Err(LayoutError::SplitCountMismatch)
        );
    }
    o = options(CombineLayout::Row);
    o.output_size = OutputSize::Width(16);
    assert_eq!(
        layout(&o, &[(1, 10_000_000)], None),
        Err(LayoutError::CanvasPixelLimit)
    );
    o = options(CombineLayout::Row);
    o.output_size = OutputSize::Height(16);
    assert_eq!(
        layout(&o, &[(100, 100), (1, 5000)], None),
        Err(LayoutError::ImageTooSmall { index: 1 })
    );
}

#[test]
fn single_image_and_extreme_one_to_fifty_are_finite_in_every_layout() {
    for kind in [
        CombineLayout::Row,
        CombineLayout::Column,
        CombineLayout::Grid,
        CombineLayout::JustifiedRows,
        CombineLayout::JustifiedColumns,
    ] {
        let o = options(kind);
        let l = calculate(&o, &[(10, 500)]);
        assert_eq!(l.canvas_size, (10, 500));
        assert_eq!(l.cards.len(), 1);
        close(l.max_crop_percent, 0.0);
        assert!(!l.any_upscaled);
        let mut o = o;
        o.canvas_ratio = Some((1, 50));
        assert_eq!(calculate(&o, &[(10, 500)]).canvas_size, (10, 500));
    }
}

#[test]
fn labels_override_number_formats_start_and_alpha_beyond_z() {
    let mut o = LabelOptions::default();
    assert_eq!(resolve_label(&spec(Some("")), 1, &o), "");
    assert_eq!(resolve_label(&spec(Some("商品A")), 1, &o), "商品A");
    assert_eq!(resolve_label(&spec(None), 1, &o), "2");
    o.number_format = NumberFormat::ZeroPadded;
    assert_eq!(resolve_label(&spec(None), 1, &o), "02");
    assert_eq!(resolve_label(&spec(None), 100, &o), "101");
    for (format, z, aa, ab) in [
        (NumberFormat::UpperAlpha, "Z", "AA", "AB"),
        (NumberFormat::LowerAlpha, "z", "aa", "ab"),
    ] {
        o.number_format = format;
        assert_eq!(resolve_label(&spec(None), 25, &o), z);
        assert_eq!(resolve_label(&spec(None), 26, &o), aa);
        assert_eq!(resolve_label(&spec(None), 27, &o), ab);
        assert_eq!(
            resolve_label(&spec(None), 701, &o),
            if format == NumberFormat::UpperAlpha {
                "ZZ"
            } else {
                "zz"
            }
        );
    }
    o.number_start = 0;
    o.number_format = NumberFormat::Arabic;
    assert_eq!(resolve_label(&spec(None), 0, &o), "0");
    o.number_format = NumberFormat::UpperAlpha;
    assert_eq!(resolve_label(&spec(None), 0, &o), "0");
    assert_eq!(resolve_label(&spec(None), 1, &o), "A");
    assert_eq!(resolve_label(&spec(Some("a\nb\rc")), 0, &o), "a b c");
}

#[test]
fn rounded_coverage_is_smooth_and_clamps_radius_to_half_short_side() {
    let rect = Rect::new(0.0, 0.0, 40.0, 40.0);
    close(render::rounded_coverage(rect, 20.0, 20.0, 20.0), 1.0);
    close(render::rounded_coverage(rect, 20.0, 0.5, 0.5), 0.0);
    let mut fractional = 0;
    for y in 0..40 {
        for x in 0..40 {
            let c = render::rounded_coverage(rect, 20.0, x as f64 + 0.5, y as f64 + 0.5);
            assert!((0.0..=1.0).contains(&c));
            if c > 0.0 && c < 1.0 {
                fractional += 1;
            }
            close(
                c,
                render::rounded_coverage(rect, 500.0, x as f64 + 0.5, y as f64 + 0.5),
            );
        }
    }
    assert!(fractional > 20);
}

#[test]
fn transparent_rounded_corners_border_arc_band_and_jpeg_background() {
    let mut o = options(CombineLayout::Row);
    o.background = Background::Transparent;
    o.corner_radius = 20;
    o.border = Border {
        width_px: 4,
        color: [255, 0, 0],
    };
    o.labels.band_color = BandColor::Color([0, 0, 255]);
    let l = layout(&o, &[(40, 40)], Some(20)).unwrap();
    let source = RgbaImage::from_pixel(40, 40, Rgba([0, 255, 0, 255]));
    let f = comic_core::FontSet::default();
    let image = render_all(&l, &[source], &o, &no_labels(1), 1.0, &f).unwrap();
    assert_eq!(image.get_pixel(0, 0)[3], 0);
    assert!(image.pixels().any(|p| p[3] > 0 && p[3] < 255));
    // Width == 2 * radius: the top is an arc, not a straight segment.
    // (20.5, 0.5) is only 0.49359px inside it and must remain antialiased.
    close(
        render::rounded_coverage(l.cards[0].card, 20.0, 20.5, 0.5),
        0.5 + 20.0 - 0.5_f64.hypot(19.5),
    );
    // This center is > 0.5px inside the outer arc and outside the inner arc.
    assert_eq!(*image.get_pixel(20, 1), Rgba([255, 0, 0, 255]));
    assert_eq!(*image.get_pixel(20, 20), Rgba([0, 255, 0, 255]));
    assert_eq!(*image.get_pixel(20, 48), Rgba([0, 0, 255, 255]));
    // Diagonal arc interior has a red border; a rectangular border would miss it.
    assert!(image.get_pixel(6, 6)[0] > image.get_pixel(6, 6)[1]);
    o.format = CombineFormat::Jpeg { quality: 95 };
    let image = render_all(&l, &[RgbaImage::new(40, 40)], &o, &no_labels(1), 1.0, &f).unwrap();
    assert!(image.pixels().all(|p| p[3] == 255));
}

#[test]
fn border_fully_covers_interior_ring_on_all_edges_and_arcs() {
    // Independent piecewise geometry: corner-circle distance in a corner's
    // quadrant, otherwise distance to the nearest straight rectangle edge.
    fn signed_distance(rect: Rect, radius: f64, x: f64, y: f64) -> f64 {
        if rect.width <= 0.0 || rect.height <= 0.0 {
            return f64::INFINITY;
        }
        let radius = radius.min(rect.width.min(rect.height) * 0.5);
        let corner_x = if x < rect.x + radius {
            Some(rect.x + radius)
        } else if x > rect.right() - radius {
            Some(rect.right() - radius)
        } else {
            None
        };
        let corner_y = if y < rect.y + radius {
            Some(rect.y + radius)
        } else if y > rect.bottom() - radius {
            Some(rect.bottom() - radius)
        } else {
            None
        };
        if let (Some(cx), Some(cy)) = (corner_x, corner_y) {
            (x - cx).hypot(y - cy) - radius
        } else {
            -(x - rect.x)
                .min(rect.right() - x)
                .min(y - rect.y)
                .min(rect.bottom() - y)
        }
    }

    for (width, height, band, radius, border) in [
        (40, 40, 20, 20, 4),  // Original failure: top/bottom arcs, image + band.
        (96, 80, 0, 20, 4),   // Four straight sides and four corner arcs.
        (40, 40, 0, 500, 4),  // Radius clamped to a circle.
        (80, 60, 0, 0, 4),    // Square corners.
        (80, 60, 0, 10, 20),  // Inner radius reaches zero.
        (20, 20, 0, 10, 100), // Border consumes the entire card.
    ] {
        let mut o = options(CombineLayout::Row);
        o.background = Background::Transparent;
        o.margin = 3;
        o.corner_radius = radius;
        o.border = Border {
            width_px: border,
            color: [255, 0, 0],
        };
        o.labels.band_color = BandColor::Color([0, 0, 255]);
        let l = layout(&o, &[(width, height)], Some(band)).unwrap();
        let source = RgbaImage::from_pixel(width, height, Rgba([0, 255, 0, 255]));
        let card = &l.cards[0];
        for scale in [1.0, 0.5, 0.37] {
            let mut image = create_canvas(&l, &o, scale).unwrap();
            draw_card(&mut image, card, &source, &o, scale).unwrap();
            let outer = render::scaled_rect(card.card, scale);
            let radius =
                (o.corner_radius as f64).min(card.card.width.min(card.card.height) * 0.5) * scale;
            let border = o.border.width_px as f64 * scale;
            let inner = outer.inset(border, border);
            let mut checked = 0;
            for (x, y, pixel) in image.enumerate_pixels() {
                let (cx, cy) = (x as f64 + 0.5, y as f64 + 0.5);
                let outer_distance = signed_distance(outer, radius, cx, cy);
                let inner_distance = signed_distance(inner, (radius - border).max(0.0), cx, cy);
                if outer_distance <= -0.5 && inner_distance >= 0.5 {
                    assert_eq!(
                        *pixel,
                        Rgba([255, 0, 0, 255]),
                        "{width}x{height}, band {band}, radius {}, border {}, scale {scale}, pixel ({x},{y})",
                        o.corner_radius,
                        o.border.width_px,
                    );
                    checked += 1;
                }
            }
            assert!(checked > 0);
        }
    }
}

#[test]
fn straight_alpha_composite_preserves_transparency_and_color() {
    let mut transparent = Rgba([0, 0, 0, 0]);
    render::composite(&mut transparent, Rgba([200, 50, 0, 128]), 0.5);
    assert_eq!(transparent, Rgba([200, 50, 0, 64]));
    render::composite(&mut transparent, Rgba([0, 100, 200, 128]), 1.0);
    assert_eq!(transparent[3], 160);
    assert!((transparent[0] as i32 - 40).abs() <= 1);
}

#[test]
fn label_boxes_pills_band_alignment_and_descenders_fit_decorated_bounds() {
    let fonts = fonts();
    for style in [BadgeStyle::Plain, BadgeStyle::Box, BadgeStyle::Pill] {
        for align in [HAlign::Left, HAlign::Center, HAlign::Right] {
            let mut o = options(CombineLayout::Row);
            o.labels.placement = LabelPlacement::Band;
            o.labels.style = style;
            o.labels.band_align = align;
            let l = calculate(&o, &[(600, 100)]);
            let labels = label_layout(&l, &[spec(Some("gy 商品A"))], &o, &fonts);
            assert_eq!(labels.not_drawn, 0);
            let label = labels.labels[0].as_ref().unwrap();
            let band = l.cards[0].band.unwrap();
            assert!(label.rect.y >= band.y && label.rect.bottom() <= band.bottom());
            assert!(
                label.text_rect.y >= label.rect.y
                    && label.text_rect.bottom() <= label.rect.bottom()
            );
            match align {
                HAlign::Left => close(label.rect.x, 16.0),
                HAlign::Right => close(label.rect.right(), 584.0),
                HAlign::Center => close(label.rect.x + label.rect.width / 2.0, 300.0),
            }
            if style == BadgeStyle::Pill {
                assert!(label.rect.width >= label.rect.height);
            }
            let raster = label::render_text(label, &o.labels, &fonts);
            assert!(raster.pixels().any(|p| p[3] != 0));
            let output =
                render_all(&l, &[RgbaImage::new(600, 100)], &o, &labels, 1.0, &fonts).unwrap();
            assert_eq!(output.dimensions(), l.canvas_size);
            if style != BadgeStyle::Plain {
                let x = (label.rect.x + label.rect.width / 2.0).round() as u32;
                let y = label.rect.y.round() as u32 + 2;
                assert!(
                    output.get_pixel(x, y)[0] < 200,
                    "badge fill must be drawn inside the band"
                );
            }
        }
    }
}

#[test]
fn label_overlay_nine_anchors_measure_margin_from_inside_border() {
    let fonts = fonts();
    for anchor in Anchor9::ALL {
        let mut o = options(CombineLayout::Row);
        o.border.width_px = 8;
        o.labels.placement = LabelPlacement::Overlay;
        o.labels.overlay_anchor = anchor;
        let l = calculate(&o, &[(500, 400)]);
        let labels = label_layout(&l, &[spec(None)], &o, &fonts);
        let label = labels.labels[0].as_ref().unwrap();
        let (ax, ay) = anchor.fractions();
        close(label.rect.x, 24.0 + (452.0 - label.rect.width) * ax);
        close(label.rect.y, 24.0 + (352.0 - label.rect.height) * ay);
    }
}

#[test]
fn label_shrinks_then_ellipsizes_and_counts_only_nonempty_unplaceable_labels() {
    let fonts = fonts();
    let mut o = options(CombineLayout::Row);
    o.labels.placement = LabelPlacement::Overlay;
    o.labels.overlay_margin_px = 0;
    o.labels.text_size_px = 100;
    let wide = calculate(&o, &[(800, 800)]);
    let original = label_layout(&wide, &[spec(Some("商品ABC"))], &o, &fonts).labels[0]
        .clone()
        .unwrap();
    let width = (original.rect.width * 0.8).floor() as u32;
    let l = calculate(&o, &[(width, 800)]);
    let labels = label_layout(&l, &[spec(Some("商品ABC"))], &o, &fonts);
    let label = labels.labels[0].as_ref().unwrap();
    assert!(label.text_size_px < 100.0 && label.text_size_px >= 60.0);
    assert_eq!(label.text, "商品ABC");
    let long_text = "商品名ABCDEFGHIJKLMN".repeat(8);
    let labels = label_layout(&l, &[spec(Some(&long_text))], &o, &fonts);
    let label = labels.labels[0].as_ref().unwrap();
    close(label.text_size_px, 60.0);
    assert!(label.text.ends_with('…'));
    assert!(label.rect.width <= width as f64);
    let tiny = calculate(&o, &[(10, 10); 3]);
    let labels = label_layout(
        &tiny,
        &[spec(Some("gy")), spec(Some("")), spec(None)],
        &o,
        &fonts,
    );
    assert_eq!(labels.not_drawn, 2);
    assert!(labels.labels.iter().all(Option::is_none));
    o.labels.overlay_margin_px = 100;
    let labels = label_layout(&tiny, &[spec(None), spec(None), spec(None)], &o, &fonts);
    assert_eq!(labels.not_drawn, 3);
}

#[test]
fn badge_height_and_pill_minimum_width_participate_in_fit_decision() {
    let fonts = fonts();
    let mut o = options(CombineLayout::Row);
    o.labels.placement = LabelPlacement::Overlay;
    o.labels.overlay_margin_px = 0;
    o.labels.text_size_px = 80;
    for style in [BadgeStyle::Box, BadgeStyle::Pill] {
        o.labels.style = style;
        let l = calculate(&o, &[(600, 12)]);
        assert_eq!(
            label_layout(&l, &[spec(Some("gy"))], &o, &fonts).not_drawn,
            1
        );
    }
    o.labels.style = BadgeStyle::Pill;
    let l = calculate(&o, &[(20, 600)]);
    assert_eq!(
        label_layout(&l, &[spec(Some("1"))], &o, &fonts).not_drawn,
        1
    );
}

#[test]
fn labels_are_clipped_to_rounded_card_and_band_without_touching_neighbors() {
    let fonts = fonts();
    for placement in [LabelPlacement::Overlay, LabelPlacement::Band] {
        let mut o = options(CombineLayout::Row);
        o.background = Background::Transparent;
        o.corner_radius = 500;
        o.gap_x = 20;
        o.labels.placement = placement;
        o.labels.style = BadgeStyle::Box;
        o.labels.fill_opacity = 100;
        o.labels.fill_color = [255, 0, 0];
        o.labels.overlay_margin_px = 0;
        o.labels.text_size_px = 20;
        let l = calculate(&o, &[(60, 60); 2]);
        let labels = label_layout(&l, &[spec(Some("gy")), spec(Some(""))], &o, &fonts);
        assert_eq!(labels.not_drawn, 0);
        let image = render_all(
            &l,
            &[RgbaImage::new(60, 60), RgbaImage::new(60, 60)],
            &o,
            &labels,
            1.0,
            &fonts,
        )
        .unwrap();
        let card = l.cards[0].card;
        for (x, y, p) in image.enumerate_pixels() {
            if render::rounded_coverage(card, 500.0, x as f64 + 0.5, y as f64 + 0.5) == 0.0 {
                assert_eq!(p[3], 0);
            }
            if placement == LabelPlacement::Band && (y as f64) < l.cards[0].band.unwrap().y {
                assert_eq!(p[3], 0);
            }
        }
    }
}

#[test]
fn preview_and_output_use_identical_edge_rounding_and_label_decisions() {
    let fonts = fonts();
    let mut o = options(CombineLayout::JustifiedRows);
    o.split = Split::Counts(vec![2, 2]);
    o.gap_x = 7;
    o.gap_y = 9;
    o.margin = 13;
    o.labels.placement = LabelPlacement::Band;
    o.labels.style = BadgeStyle::Pill;
    o.corner_radius = 15;
    o.border = Border {
        width_px: 3,
        color: [255, 255, 255],
    };
    let dims = [(140, 200), (200, 140), (140, 200), (140, 200)];
    let l = calculate(&o, &dims);
    let labels = label_layout(
        &l,
        &[spec(None), spec(Some("商品ABC")), spec(None), spec(None)],
        &o,
        &fonts,
    );
    let colors = [
        [255, 0, 0, 255],
        [0, 255, 0, 255],
        [0, 0, 255, 255],
        [255, 0, 255, 255],
    ];
    let sources: Vec<_> = dims
        .iter()
        .zip(colors)
        .map(|(&(w, h), color)| RgbaImage::from_pixel(w, h, Rgba(color)))
        .collect();
    let output = render_all(&l, &sources, &o, &labels, 1.0, &fonts).unwrap();
    let scale = 0.37;
    let small_sources: Vec<_> = sources
        .iter()
        .map(|s| {
            crate::fast_resize::resize_rgba8_exact(
                s,
                (s.width() as f64 * 0.4).round() as u32,
                (s.height() as f64 * 0.4).round() as u32,
                crate::fast_resize::Quality::Lanczos3,
            )
        })
        .collect();
    let preview = render_all(&l, &small_sources, &o, &labels, scale, &fonts).unwrap();
    let reduced = crate::fast_resize::resize_rgba8_exact(
        &output,
        preview.width(),
        preview.height(),
        crate::fast_resize::Quality::Lanczos3,
    );
    // Solid source centers agree; every destination edge (including labels) lies
    // within one pixel of the corresponding full-output edge after reduction.
    for (i, card) in l.cards.iter().enumerate() {
        let target = render::scaled_rect(card.image, scale);
        let (cx, cy) = (
            (target.x + target.width / 2.0) as u32,
            (target.y + target.height / 2.0) as u32,
        );
        assert_eq!(*preview.get_pixel(cx, cy), Rgba(colors[i]));
        assert_eq!(preview.get_pixel(cx, cy), reduced.get_pixel(cx, cy));
        for rect in [
            card.card,
            card.image,
            card.band.unwrap(),
            labels.labels[i].as_ref().unwrap().rect,
        ] {
            let r = render::scaled_rect(rect, scale);
            for (a, b) in [
                (r.x, rect.x * scale),
                (r.y, rect.y * scale),
                (r.right(), rect.right() * scale),
                (r.bottom(), rect.bottom() * scale),
            ] {
                assert!((a - b).abs() <= 0.5 + 1e-9);
            }
        }
    }
    // Exercise actual output/preview boundaries, allowing the antialiasing fringe.
    for (x, y, pixel) in preview.enumerate_pixels() {
        if colors.iter().any(|c| pixel.0 == *c) {
            assert!(
                (y.saturating_sub(1)..=(y + 1).min(reduced.height() - 1)).any(|yy| (x
                    .saturating_sub(1)
                    ..=(x + 1).min(reduced.width() - 1))
                    .any(|xx| reduced
                        .get_pixel(xx, yy)
                        .0
                        .iter()
                        .zip(pixel.0)
                        .all(|(&a, b)| a.abs_diff(b) <= 32)))
            );
        }
    }
}

#[test]
fn preview_sources_crop_in_composed_coordinates_and_streaming_matches_render_all() {
    let mut o = options(CombineLayout::Row);
    o.canvas_ratio = Some((2, 1));
    let l = calculate(&o, &[(100, 100)]);
    let source = RgbaImage::from_fn(20, 20, |_, y| {
        if (5..15).contains(&y) {
            Rgba([255, 0, 0, 255])
        } else {
            Rgba([0, 0, 255, 255])
        }
    });
    let f = comic_core::FontSet::default();
    let image = render_all(
        &l,
        std::slice::from_ref(&source),
        &o,
        &no_labels(1),
        0.5,
        &f,
    )
    .unwrap();
    assert_eq!(
        *image.get_pixel(image.width() / 2, image.height() / 2),
        Rgba([255, 0, 0, 255])
    );
    let mut streaming = create_canvas(&l, &o, 0.5).unwrap();
    draw_card(&mut streaming, &l.cards[0], &source, &o, 0.5).unwrap();
    assert_eq!(streaming, image);
    assert_eq!(
        create_canvas(&l, &o, f64::NAN),
        Err(RenderError::InvalidScale)
    );
    assert_eq!(
        render_all(&l, &[], &o, &no_labels(1), 1.0, &f),
        Err(RenderError::ItemCountMismatch)
    );
}

#[test]
fn text_raster_keeps_outline_negative_bearings_and_descenders() {
    let fonts = fonts();
    let mut o = options(CombineLayout::Row);
    o.labels.placement = LabelPlacement::Band;
    for style in [BadgeStyle::Plain, BadgeStyle::Box, BadgeStyle::Pill] {
        o.labels.style = style;
        let l = calculate(&o, &[(800, 100)]);
        let labels = label_layout(&l, &[spec(Some("gy fj"))], &o, &fonts);
        let placed = labels.labels[0].as_ref().unwrap();
        let tight = label::render_text(placed, &o.labels, &fonts);
        let block = label::text_block(&placed.text, placed.text_size_px, &o.labels);
        let reference = comic_core::bake_overlay(
            &[comic_core::AnnotationObject::new_text(
                0,
                (128.0, 128.0),
                block,
            )],
            1000,
            400,
            &fonts,
        );
        let tight_alpha: u64 = tight.pixels().map(|p| p[3] as u64).sum();
        let reference_alpha: u64 = reference.pixels.chunks_exact(4).map(|p| p[3] as u64).sum();
        assert_eq!(
            tight_alpha, reference_alpha,
            "{style:?}: the tight raster must not clip ink"
        );
    }
}

#[test]
fn preview_badge_and_text_pixel_bounds_match_reduced_output_within_one_pixel() {
    fn bounds(image: &RgbaImage, channel: usize) -> (u32, u32, u32, u32) {
        let mut bounds = (image.width(), image.height(), 0, 0);
        for (x, y, p) in image.enumerate_pixels() {
            if p[3] > 128
                && p[channel] > 128
                && p[(channel + 1) % 3] < 80
                && p[(channel + 2) % 3] < 80
            {
                bounds.0 = bounds.0.min(x);
                bounds.1 = bounds.1.min(y);
                bounds.2 = bounds.2.max(x + 1);
                bounds.3 = bounds.3.max(y + 1);
            }
        }
        assert!(bounds.2 > bounds.0 && bounds.3 > bounds.1);
        bounds
    }
    let fonts = fonts();
    for style in [BadgeStyle::Box, BadgeStyle::Pill] {
        for placement in [LabelPlacement::Overlay, LabelPlacement::Band] {
            let mut o = options(CombineLayout::Row);
            o.background = Background::Transparent;
            o.labels.placement = placement;
            o.labels.style = style;
            o.labels.fill_color = [255, 0, 0];
            o.labels.fill_opacity = 100;
            o.labels.text_color = [0, 255, 0];
            o.labels.overlay_anchor = Anchor9::BottomRight;
            let l = calculate(&o, &[(500, 300)]);
            let labels = label_layout(&l, &[spec(Some("gy ABC"))], &o, &fonts);
            let sources = [RgbaImage::new(50, 30)];
            let full = render_all(&l, &sources, &o, &labels, 1.0, &fonts).unwrap();
            let preview = render_all(&l, &sources, &o, &labels, 0.37, &fonts).unwrap();
            let reduced = crate::fast_resize::resize_rgba8_exact(
                &full,
                preview.width(),
                preview.height(),
                crate::fast_resize::Quality::Lanczos3,
            );
            for channel in [0, 1] {
                let a = bounds(&preview, channel);
                let b = bounds(&reduced, channel);
                for (a, b) in [(a.0, b.0), (a.1, b.1), (a.2, b.2), (a.3, b.3)] {
                    assert!(
                        a.abs_diff(b) <= 1,
                        "{style:?} {placement:?}, channel {channel}: {a} vs {b}"
                    );
                }
            }
        }
    }
}

#[test]
fn fractional_crop_is_preserved_when_using_small_preview_sources() {
    let mut o = options(CombineLayout::Row);
    o.canvas_ratio = Some((1, 3));
    let l = calculate(&o, &[(100, 100)]);
    let source = RgbaImage::from_fn(13, 17, |x, y| {
        Rgba([(x * 19) as u8, (y * 13) as u8, 0, 255])
    });
    let scale = 0.23;
    let actual = render_all(
        &l,
        std::slice::from_ref(&source),
        &o,
        &no_labels(1),
        scale,
        &comic_core::FontSet::default(),
    )
    .unwrap();
    let crop = l.cards[0].source_crop;
    let mut expected = RgbaImage::new(actual.width(), actual.height());
    let opts = fast_image_resize::ResizeOptions::new()
        .resize_alg(crate::fast_resize::Quality::Lanczos3.into())
        .crop(
            crop.x * 13.0 / 100.0,
            crop.y * 17.0 / 100.0,
            crop.width * 13.0 / 100.0,
            crop.height * 17.0 / 100.0,
        );
    fast_image_resize::Resizer::new()
        .resize(&source, &mut expected, &opts)
        .unwrap();
    assert!(
        actual
            .as_raw()
            .iter()
            .zip(expected.as_raw())
            .all(|(&a, &b)| a.abs_diff(b) <= 1)
    );
}

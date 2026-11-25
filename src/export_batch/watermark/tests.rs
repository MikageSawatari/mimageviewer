use super::*;
use image::Rgba;

#[test]
fn watermark_nine_anchors_and_short_edge_margin() {
    let logo = RgbaImage::from_pixel(10, 8, Rgba([200, 10, 20, 255]));
    let size = [100, 80];
    assert_eq!(margin(size, 5), 4);
    for (index, anchor) in WatermarkAnchor::ALL.into_iter().enumerate() {
        let x = [4, 45, 86][index % 3];
        let y = [4, 36, 68][index / 3];
        let mut base = vec![0; 100 * 80 * 4];
        composite(&mut base, size, &logo, anchor, 4, 100);
        for row in 0..80 {
            for column in 0..100 {
                let pixel = &base[(row * 100 + column) * 4..(row * 100 + column + 1) * 4];
                if (x..x + 10).contains(&column) && (y..y + 8).contains(&row) {
                    assert_eq!(pixel, &[200, 10, 20, 255], "{anchor:?}");
                } else {
                    assert_eq!(pixel, &[0; 4], "{anchor:?}");
                }
            }
        }
    }
}

#[test]
fn watermark_logo_size_percent_original_and_fit_inside_margin() {
    assert_eq!(
        logo_size(
            (100, 50),
            [1000, 800],
            16,
            ImageWatermarkSize::PercentOfWidth(20)
        ),
        (200, 100)
    );
    assert_eq!(
        logo_size((100, 50), [1000, 800], 16, ImageWatermarkSize::Original),
        (100, 50)
    );
    assert_eq!(
        logo_size((1000, 500), [100, 80], 4, ImageWatermarkSize::Original),
        (92, 46)
    );
    assert_eq!(
        logo_size(
            (50, 100),
            [100, 80],
            4,
            ImageWatermarkSize::PercentOfWidth(100)
        ),
        (36, 72)
    );
    assert_eq!(
        logo_size((100, 200), [1, 1], 0, ImageWatermarkSize::Original),
        (1, 1)
    );
}

#[test]
fn watermark_text_short_edge_size_and_ink_bounds() {
    let fonts = crate::comic_overlay::load_comic_fonts().expect("Windows annotation font");
    let mut options = BatchWatermark {
        text: "作者 Logo".into(),
        text_size_percent: 10,
        ..Default::default()
    };
    assert_eq!(text_block(&options, [600, 400]).size_px, 40.0);
    assert_eq!(text_block(&options, [400, 600]).size_px, 40.0);
    let small = render_text(&options, [400, 400], &fonts);
    let large = render_text(&options, [800, 800], &fonts);
    assert!(small.height() > 10);
    assert!((large.height() as i64 - small.height() as i64 * 2).abs() <= 4);
    assert!(small.pixels().any(|pixel| pixel[3] > 0 && pixel[0] == 0));
    options.text_outline = false;
    let plain = render_text(&options, [400, 400], &fonts);
    assert!(
        plain
            .pixels()
            .filter(|pixel| pixel[3] > 0)
            .all(|pixel| pixel[0] == 255)
    );
    assert!(plain.height() < small.height());
}

#[test]
fn watermark_straight_src_over_multiplies_source_alpha() {
    for base_alpha in [0, 64, 128, 255] {
        for logo_alpha in [0, 16, 128, 255] {
            for opacity in [0, 30, 60, 100] {
                let original = [20, 40, 80, base_alpha];
                let mut base = original;
                let logo = RgbaImage::from_pixel(1, 1, Rgba([100, 10, 60, logo_alpha]));
                composite(
                    &mut base,
                    [1, 1],
                    &logo,
                    WatermarkAnchor::Center,
                    0,
                    opacity,
                );
                if opacity == 0 || logo_alpha == 0 {
                    assert_eq!(base, original);
                    continue;
                }
                let a = logo_alpha as f64 / 255.0 * opacity as f64 / 100.0;
                let da = base_alpha as f64 / 255.0;
                let out = a + da * (1.0 - a);
                let expected = [100, 10, 60].map(|c| c as f64 * a);
                for c in 0..3 {
                    let expected =
                        ((expected[c] + original[c] as f64 * da * (1.0 - a)) / out).round() as u8;
                    assert!((base[c] as i16 - expected as i16).abs() <= 1);
                }
                assert!((base[3] as f64 - out * 255.0).abs() <= 1.0);
            }
        }
    }
}

#[test]
fn watermark_logo_formats_preserve_alpha_and_jpeg_orientation() {
    let tmp = tempfile::tempdir().unwrap();
    let logo = RgbaImage::from_fn(3, 1, |x, _| Rgba([200, 30, 50, [0, 128, 255][x as usize]]));
    for (ext, format) in [
        ("png", image::ImageFormat::Png),
        ("webp", image::ImageFormat::WebP),
        ("bmp", image::ImageFormat::Bmp),
        ("gif", image::ImageFormat::Gif),
    ] {
        let path = tmp.path().join(format!("logo.{ext}"));
        logo.save_with_format(&path, format).unwrap();
        let decoded = load_logo(&path).unwrap();
        assert_eq!(decoded.dimensions(), (3, 1));
        assert_eq!(decoded.get_pixel(0, 0)[3], 0, "{ext}");
        assert_eq!(decoded.get_pixel(2, 0)[3], 255, "{ext}");
        if ext != "gif" {
            assert_eq!(decoded.get_pixel(1, 0)[3], 128, "{ext}");
        }
        let mut base = [40, 80, 120, 128].repeat(3);
        composite(&mut base, [3, 1], &decoded, WatermarkAnchor::TopLeft, 0, 50);
        assert_eq!(
            &base[..4],
            &[40, 80, 120, 128],
            "{ext}: transparent logo pixel"
        );
        // Fully opaque logo at 50% over a half-transparent base: A~=192, RGB~=147/47/73.
        for (actual, expected) in base[8..12].iter().zip([147, 47, 73, 192]) {
            assert!((*actual as i16 - expected).abs() <= 1, "{ext}: {base:?}");
        }
    }
    let path = tmp.path().join("logo.jpg");
    image::RgbImage::from_pixel(2, 1, image::Rgb([200, 30, 50]))
        .save(&path)
        .unwrap();
    let jpeg = std::fs::read(&path).unwrap();
    // Minimal little-endian TIFF with IFD0 Orientation=6 (90 degrees clockwise).
    let mut exif =
        b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0".to_vec();
    let mut oriented = vec![0xff, 0xd8, 0xff, 0xe1];
    oriented.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    oriented.append(&mut exif);
    oriented.extend_from_slice(&jpeg[2..]);
    std::fs::write(&path, oriented).unwrap();
    let decoded = load_logo(&path).unwrap();
    assert_eq!(decoded.dimensions(), (1, 2));
    assert!(decoded.pixels().all(|pixel| pixel[3] == 255));
    let bmp = tmp.path().join("opaque.bmp");
    image::RgbImage::from_pixel(2, 1, image::Rgb([80, 90, 100]))
        .save(&bmp)
        .unwrap();
    assert!(
        load_logo(&bmp)
            .unwrap()
            .pixels()
            .all(|pixel| pixel[3] == 255)
    );
    let gif = tmp.path().join("animated.gif");
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(std::fs::File::create(&gif).unwrap());
        encoder
            .encode_frame(image::Frame::new(logo.clone()))
            .unwrap();
        encoder
            .encode_frame(image::Frame::new(RgbaImage::from_pixel(
                3,
                1,
                Rgba([0, 255, 0, 255]),
            )))
            .unwrap();
    }
    let first = load_logo(&gif).unwrap();
    assert_eq!(first.get_pixel(0, 0)[3], 0);
    assert_eq!(first.get_pixel(2, 0).0, [200, 30, 50, 255]);
}

#[test]
fn watermark_logo_resize_weights_alpha_and_reuses_last_size() {
    for transparent_color in [[0, 0, 0, 0], [255, 255, 255, 0]] {
        let original = RgbaImage::from_fn(32, 32, |x, y| {
            if (8..24).contains(&x) && (8..24).contains(&y) {
                Rgba([200, 40, 80, 255])
            } else {
                Rgba(transparent_color)
            }
        });
        let resized = resize_logo(&original, (8, 8)).unwrap();
        let mut edge_count = 0;
        for pixel in resized
            .pixels()
            .filter(|pixel| pixel[3] > 24 && pixel[3] < 240)
        {
            edge_count += 1;
            for (channel, expected) in [200, 40, 80].into_iter().enumerate() {
                assert!((pixel[channel] as i16 - expected).abs() <= 8, "{pixel:?}");
            }
        }
        assert!(edge_count > 0);
        let mut assets = WatermarkAssets::Image {
            original,
            resized: None,
        };
        let options = BatchWatermark {
            enabled: true,
            kind: WatermarkKind::Image,
            ..Default::default()
        };
        let base = eframe::egui::ColorImage::filled([40, 40], eframe::egui::Color32::BLACK);
        assets.apply(&base, &options).unwrap();
        let pointer = match &assets {
            WatermarkAssets::Image {
                resized: Some(image),
                ..
            } => image.as_ptr(),
            _ => panic!("resized cache missing"),
        };
        assets.apply(&base, &options).unwrap();
        match &assets {
            WatermarkAssets::Image {
                resized: Some(image),
                ..
            } => assert_eq!(pointer, image.as_ptr()),
            _ => panic!("resized cache missing"),
        }
    }
}

#[test]
fn watermark_stage_a_settings_default_and_roundtrip() {
    let mut json = serde_json::to_value(super::super::BatchExportOptions::default()).unwrap();
    json.as_object_mut().unwrap().remove("watermark");
    let old: super::super::BatchExportOptions = serde_json::from_value(json).unwrap();
    assert_eq!(old.watermark, BatchWatermark::default());
    for kind in [WatermarkKind::Text, WatermarkKind::Image] {
        let watermark = BatchWatermark {
            enabled: true,
            kind,
            text: "作者".into(),
            image_path: Some("C:/logo.png".into()),
            ..Default::default()
        };
        let decoded: BatchWatermark =
            serde_json::from_str(&serde_json::to_string(&watermark).unwrap()).unwrap();
        assert_eq!(decoded, watermark);
    }
}

//! Runtime-generated JPEGs exercise actual TurboJPEG DCT output, rather than
//! approximating decoder scale changes with an ordinary image resize.
use super::*;
use crate::cover_auto_crop::{analysis_raster, cover_auto_crop};
use crate::settings::ReadingDirection;
use std::collections::BTreeSet;

fn jacket_jpeg(binding: ReadingDirection) -> Vec<u8> {
    // Four times the synthetic jacket geometry: at DCT 1/8 the decoded raster
    // is already exactly 256 rows, whereas the full decode is 2048 rows.
    let image = image::RgbImage::from_fn(3568, 2048, |x, y| {
        let x = if binding == ReadingDirection::Ltr {
            3567 - x
        } else {
            x
        } / 4;
        let y = y / 4;
        let pixel = match x {
            0..140 => {
                if y % 73 < 8 {
                    [65, 65, 65]
                } else {
                    [246, 242, 220]
                }
            }
            140..498 => {
                let wave = (y % 80) as u8;
                [35 + wave, 55 + (y % 155) as u8, 200 - wave]
            }
            498..534 => [18, 28, 38],
            _ => {
                if y % 73 < 8 {
                    [75, 75, 75]
                } else {
                    [246, 242, 220]
                }
            }
        };
        image::Rgb(pixel)
    });
    let mut bytes = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut bytes, 90)
        .encode_image(&image)
        .expect("encode synthetic jacket JPEG");
    bytes
}

#[test]
fn auto_crop_boundaries_stay_stable_across_actual_jpeg_dct_scales() {
    for binding in [ReadingDirection::Rtl, ReadingDirection::Ltr] {
        let bytes = jacket_jpeg(binding);
        let (full, stats) =
            decode_jpeg_turbo_scaled_from_bytes(&bytes, 3568).expect("full JPEG decode");
        assert_eq!(stats.scale_num, 8);
        let original_dims = (stats.src_w, stats.src_h);
        let full_raster = analysis_raster(&full);
        let reference = cover_auto_crop(&full_raster, original_dims, Some(0.7), binding)
            .expect("full decode must detect the generated jacket")
            .values();
        let mut actual_scales = BTreeSet::new();
        for target in [128, 512, 1024, 2048] {
            let (decoded, stats) =
                decode_jpeg_turbo_scaled_from_bytes(&bytes, target).expect("scaled JPEG decode");
            actual_scales.insert(stats.scale_num);
            assert_eq!((stats.src_w, stats.src_h), original_dims);
            assert_eq!(decoded.width(), stats.out_w);
            assert_eq!(decoded.height(), stats.out_h);
            let raster = analysis_raster(&decoded);
            assert_eq!(raster.height(), 256);
            let crop = cover_auto_crop(&raster, original_dims, Some(0.7), binding)
                .unwrap_or_else(|reason| {
                    panic!(
                        "{binding:?}, DCT {}/8, target {target}: {reason:?}",
                        stats.scale_num
                    )
                })
                .values();
            for edge in [0, 2] {
                let drift_columns = crop[edge].abs_diff(reference[edge]) as f64
                    * full_raster.width() as f64
                    / 1_000_000.0;
                assert!(
                    drift_columns <= 1.1,
                    "{binding:?}, DCT {}/8, target {target}, edge {edge}: {drift_columns} analysis columns",
                    stats.scale_num,
                );
            }
        }
        assert!(actual_scales.len() >= 3, "DCT scales: {actual_scales:?}");
    }
}

//! Opt-in numeric-name evaluation thumbnails; never writes source identities.
use super::{
    CoverCrop, DynamicImage, Path, Rgb, RgbImage, Rotation, rotated_image, rotated_rect,
    target_output,
};

const MAX_EDGE: u32 = 640;
const JPEG_QUALITY: u8 = 92;

/// A crop is stored before saved rotation; overlays use the rotated image's
/// normalized edge coordinates, rather than pixel-center coordinates.
pub(super) fn visible_crop(crop: CoverCrop, rotation: Rotation) -> [f64; 4] {
    rotated_rect(
        crop.values().map(f64::from),
        (1_000_000, 1_000_000),
        rotation,
    )
    .map(|value| value / 1_000_000.0)
}

fn line_width(width: u32) -> u32 {
    (f64::from(width) / 160.0).round().clamp(2.0, 6.0) as u32
}

fn frame(image: &mut RgbImage, bounds: [f64; 4], color: Rgb<u8>) {
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 {
        return;
    }
    let low = |value: f64, size: u32| (value * f64::from(size)).floor().max(0.0) as u32;
    let high = |value: f64, size: u32| {
        ((value * f64::from(size)).ceil().max(1.0) as u32).saturating_sub(1)
    };
    let [x0, y0, x1, y1] = bounds;
    let left = low(x0, width).min(width - 1);
    let right = high(x1, width).min(width - 1);
    let top = low(y0, height).min(height - 1);
    let bottom = high(y1, height).min(height - 1);
    let thickness = line_width(width);
    for y in top..=bottom {
        for x in left..=right {
            if x - left < thickness
                || right - x < thickness
                || y - top < thickness
                || bottom - y < thickness
            {
                image.put_pixel(x, y, color);
            }
        }
    }
}

/// Rotate first, shrink only when needed, then annotate the visible geometry.
/// A fallback prediction owns the blue frame when both options are supplied.
pub(super) fn render(
    image: &DynamicImage,
    saved: Rotation,
    current: Option<CoverCrop>,
    fallback: Option<CoverCrop>,
) -> RgbImage {
    let image = rotated_image(image, saved);
    let mut image = if image.width().max(image.height()) > MAX_EDGE {
        image.thumbnail(MAX_EDGE, MAX_EDGE).to_rgb8()
    } else {
        image.to_rgb8()
    };
    if let Some((crop, color)) = fallback
        .map(|crop| (crop, Rgb([0, 0, 255])))
        .or_else(|| current.map(|crop| (crop, Rgb([255, 0, 0]))))
    {
        frame(&mut image, visible_crop(crop, saved), color);
    }
    image
}

fn normal_output_file(path: &Path) -> Result<(), String> {
    match std::fs::symlink_metadata(path) {
        Ok(metadata) => {
            #[cfg(windows)]
            let reparse = {
                use std::os::windows::fs::MetadataExt;
                metadata.file_attributes() & 0x400 != 0
            };
            #[cfg(not(windows))]
            let reparse = metadata.file_type().is_symlink();
            if reparse || !metadata.is_file() {
                return Err("evaluation thumbnail output must be a normal file".into());
            }
            Ok(())
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(format!(
            "could not inspect evaluation thumbnail output: {error}"
        )),
    }
}

/// The caller opts in by calling this function. No directory or file is created
/// by render/visible_crop. Return a portable, output-relative numeric path.
pub(super) fn write(
    output: &Path,
    sequence: usize,
    image: &DynamicImage,
    saved: Rotation,
    current: Option<CoverCrop>,
    fallback: Option<CoverCrop>,
) -> Result<String, String> {
    if sequence == 0 {
        return Err("evaluation thumbnail sequence must be one-based".into());
    }
    let directory = target_output(output.join("thumbs")).map_err(|error| error.to_string())?;
    let filename = format!("{sequence:05}.jpg");
    let path = directory.join(&filename);
    normal_output_file(&path)?;
    let rendered = render(image, saved, current, fallback);
    let mut encoded = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut encoded, JPEG_QUALITY)
        .encode_image(&DynamicImage::ImageRgb8(rendered))
        .map_err(|error| format!("could not encode evaluation thumbnail: {error}"))?;
    std::fs::write(&path, encoded)
        .map_err(|error| format!("could not write evaluation thumbnail: {error}"))?;
    Ok(format!("thumbs/{filename}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crop() -> CoverCrop {
        CoverCrop::new([250_000, 125_000, 750_000, 625_000])
            .unwrap()
            .unwrap()
    }

    fn rotations() -> [Rotation; 4] {
        [
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ]
    }

    fn corners() -> DynamicImage {
        DynamicImage::ImageRgb8(RgbImage::from_fn(120, 80, |x, y| match (x < 60, y < 40) {
            (true, true) => Rgb([210, 20, 20]),
            (false, true) => Rgb([20, 210, 20]),
            (true, false) => Rgb([20, 20, 210]),
            (false, false) => Rgb([210, 210, 20]),
        }))
    }

    fn temp_output() -> Result<tempfile::TempDir, Box<dyn std::error::Error>> {
        let target = std::env::current_dir()?.join("target");
        std::fs::create_dir_all(&target)?;
        Ok(tempfile::Builder::new()
            .prefix("cover-eval-thumbs-")
            .tempdir_in(target)?)
    }

    #[test]
    fn evaluation_thumbnails_rotate_source_corners_and_normalized_crop_edges() {
        let image = corners();
        for (rotation, expected, corner) in [
            (Rotation::None, [0.25, 0.125, 0.75, 0.625], [210, 20, 20]),
            (Rotation::Cw90, [0.375, 0.25, 0.875, 0.75], [20, 20, 210]),
            (Rotation::Cw180, [0.25, 0.375, 0.75, 0.875], [210, 210, 20]),
            (Rotation::Cw270, [0.125, 0.25, 0.625, 0.75], [20, 210, 20]),
        ] {
            assert_eq!(visible_crop(crop(), rotation), expected);
            let rendered = render(&image, rotation, None, None);
            assert_eq!(rendered.get_pixel(0, 0).0, corner);
            assert_eq!(rendered, rotated_image(&image, rotation).to_rgb8());
        }
    }

    #[test]
    fn evaluation_thumbnails_shrink_only_and_scale_inward_frames_with_fallback_precedence() {
        for (size, expected) in [
            ((120, 80), (120, 80)),
            ((1280, 640), (640, 320)),
            ((640, 1280), (320, 640)),
        ] {
            let image =
                DynamicImage::ImageRgb8(RgbImage::from_pixel(size.0, size.1, Rgb([70, 80, 90])));
            let unframed = render(&image, Rotation::None, None, None);
            assert_eq!(unframed.dimensions(), expected);
            assert!(unframed.pixels().all(|pixel| pixel.0 == [70, 80, 90]));
        }
        assert_eq!(line_width(120), 2);
        assert_eq!(line_width(640), 4);
        let image = DynamicImage::new_rgb8(640, 320);
        let red = render(&image, Rotation::None, Some(crop()), None);
        let blue = render(&image, Rotation::None, Some(crop()), Some(crop()));
        // x0=160, y0=40, x1=480 exclusive, y1=200 exclusive.
        for offset in 0..4 {
            assert_eq!(red.get_pixel(160 + offset, 100).0, [255, 0, 0]);
            assert_eq!(blue.get_pixel(160 + offset, 100).0, [0, 0, 255]);
            assert_eq!(red.get_pixel(300, 199 - offset).0, [255, 0, 0]);
        }
        assert_eq!(red.get_pixel(159, 100).0, [0, 0, 0]);
        assert_eq!(red.get_pixel(164, 100).0, [0, 0, 0]);
        assert_eq!(red.get_pixel(480, 100).0, [0, 0, 0]);
        assert!(!blue.pixels().any(|pixel| pixel.0 == [255, 0, 0]));
        let edge_crop = CoverCrop::new([500_000, 0, 1_000_000, 1_000_000])
            .unwrap()
            .unwrap();
        let framed = render(&image, Rotation::None, Some(edge_crop), None);
        assert_eq!(framed.get_pixel(639, 319).0, [255, 0, 0]);
    }

    #[test]
    fn evaluation_thumbnails_write_numeric_jpegs_with_rotated_dimensions_and_colored_frames()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = temp_output()?;
        let output = temp.path().join("output");
        let image = DynamicImage::ImageRgb8(RgbImage::from_pixel(1280, 640, Rgb([100, 100, 100])));
        let mut names = Vec::new();
        for (index, rotation) in rotations().into_iter().enumerate() {
            for blue in [false, true] {
                let sequence = index * 2 + usize::from(blue) + 1;
                let relative = write(
                    &output,
                    sequence,
                    &image,
                    rotation,
                    Some(crop()),
                    blue.then(crop),
                )?;
                assert_eq!(relative, format!("thumbs/{sequence:05}.jpg"));
                names.push(format!("{sequence:05}.jpg"));
                let decoded = image::open(output.join(relative))?.to_rgb8();
                let expected = match rotation {
                    Rotation::Cw90 | Rotation::Cw270 => (320, 640),
                    _ => (640, 320),
                };
                assert_eq!(decoded.dimensions(), expected);
                let bounds = visible_crop(crop(), rotation);
                let x = (bounds[0] * f64::from(decoded.width())).floor() as u32 + 1;
                let y =
                    (((bounds[1] + bounds[3]) / 2.0) * f64::from(decoded.height())).round() as u32;
                let [r, g, b] = decoded.get_pixel(x, y).0.map(i32::from);
                if blue {
                    assert!(b > r + 60 && b > g + 60, "{rotation:?}: {:?}", [r, g, b]);
                } else {
                    assert!(r > b + 60 && r > g + 60, "{rotation:?}: {:?}", [r, g, b]);
                }
            }
        }
        // Re-running into the same numeric normal file is supported. Without
        // either prediction, the JPEG contains only the source pixels.
        let relative = write(&output, 1, &image, Rotation::None, None, None)?;
        let unframed = image::open(output.join(relative))?.to_rgb8();
        assert_eq!(unframed.dimensions(), (640, 320));
        assert!(
            unframed
                .pixels()
                .all(|pixel| pixel.0.into_iter().all(|value| value.abs_diff(100) <= 2))
        );
        let mut actual: Vec<_> = std::fs::read_dir(output.join("thumbs"))?
            .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        actual.sort();
        names.sort();
        assert_eq!(actual, names);
        assert_eq!(std::fs::read_dir(&output)?.count(), 1);
        Ok(())
    }

    #[test]
    fn evaluation_thumbnails_reject_non_target_paths_zero_sequence_and_non_files()
    -> Result<(), Box<dyn std::error::Error>> {
        let temp = temp_output()?;
        let image = corners();
        let outside = std::env::current_dir()?.join("not-target-thumb-output");
        assert!(write(&outside, 1, &image, Rotation::None, None, None).is_err());
        let output = temp.path().join("output");
        assert!(write(&output, 0, &image, Rotation::None, None, None).is_err());
        assert!(!output.exists());
        let directory = target_output(output.join("thumbs"))?;
        std::fs::create_dir(directory.join("00001.jpg"))?;
        assert!(write(&output, 1, &image, Rotation::None, None, None).is_err());
        Ok(())
    }
}

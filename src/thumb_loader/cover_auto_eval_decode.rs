//! Evaluation uses exactly the fixed production analysis decode, independently
//! of display/thumb geometry. Production builds omit this adapter.
use super::*;

pub(crate) fn shot_exif(bytes: &[u8]) -> bool {
    super::cover_auto_analysis::shot_exif_from_bytes(bytes)
}

pub(crate) type DecodedCover = CoverAutoAnalysis;

pub(crate) fn decode(
    path: &Path,
    zip_entry: Option<&str>,
    _display_px: u32,
    _thumb_px: u32,
) -> Result<DecodedCover, String> {
    if let Some(entry) = zip_entry {
        let bytes = if crate::rar_loader::is_rar_path(path) {
            crate::rar_loader::read_entry_bytes(path, entry)
        } else {
            crate::zip_loader::read_entry_bytes(path, entry)
        }
        .map_err(|error| error.to_string())?;
        decode_cover_auto_analysis_image(
            crate::image_decode::Source::Bytes {
                bytes: &bytes,
                hint: entry,
            },
            None,
            None,
        )
        .map_err(|error| error.to_string())
    } else {
        decode_cover_auto_analysis_image(crate::image_decode::Source::Path(path), None, None)
            .map_err(|error| error.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn cover_auto_eval_worker_decode_preserves_full_png_pixels() {
        let _data = crate::data_dir::TestDataDirGuard::new();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("cover.png");
        let image = image::RgbImage::from_fn(640, 384, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 239) as u8, 123])
        });
        image.save(&path).unwrap();
        let decoded = decode(&path, None, 256, 512).unwrap();
        assert_eq!(decoded.image.to_rgb8(), image);
        assert_eq!(decoded.native_dims, (640, 384));
        assert_eq!(decoded.dct_scale_num, None);
        assert_eq!(decoded.orientation, 1);
    }

    #[test]
    fn cover_auto_eval_worker_decode_uses_dct_target_then_exif_and_native_dimensions() {
        let _data = crate::data_dir::TestDataDirGuard::new();
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("book.zip");
        let source = image::RgbImage::from_fn(2048, 1024, |x, y| {
            image::Rgb([(x % 251) as u8, (y % 239) as u8, 123])
        });
        let jpeg = turbojpeg::compress_image(&source, 85, turbojpeg::Subsamp::Sub2x2)
            .unwrap()
            .to_vec();
        let payload = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
        let mut bytes = jpeg[..2].to_vec();
        bytes.extend([0xff, 0xe1]);
        bytes.extend(((payload.len() + 2) as u16).to_be_bytes());
        bytes.extend(payload);
        bytes.extend_from_slice(&jpeg[2..]);
        let mut archive = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        archive
            .start_file("cover.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive.write_all(&bytes).unwrap();
        archive.finish().unwrap();
        assert_eq!(crate::settings::Settings::default().thumb_px, 512);
        for display in [256, 512, 1024, 2048] {
            let decoded = decode(&path, Some("cover.jpg"), display, 512).unwrap();
            assert_eq!(decoded.dct_scale_num, Some(4));
            assert_eq!((decoded.image.width(), decoded.image.height()), (512, 1024));
            assert_eq!(decoded.native_dims, (1024, 2048));
            assert_eq!(decoded.orientation, 6);
            let (raw, _) = decode_jpeg_turbo_scaled_with_axis(&bytes, 1024, None, true).unwrap();
            assert_eq!(decoded.image.to_rgb8(), raw.rotate90().to_rgb8());
        }
    }
}

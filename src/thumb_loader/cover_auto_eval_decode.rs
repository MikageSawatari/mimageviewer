//! Evaluation-only tap into the real worker, after EXIF and before cover analysis.
//! No decoder or decoder dispatch is reproduced here; production builds omit this module.
use super::*;
use std::cell::RefCell;
use std::collections::HashMap;

pub(crate) struct DecodedCover {
    pub image: image::DynamicImage,
    pub native_dims: (u32, u32),
    pub dct_scale_num: Option<u32>,
    pub orientation: u16,
}

thread_local! {
    static CAPTURE: RefCell<Option<Option<DecodedCover>>> = const { RefCell::new(None) };
}

pub(super) fn capture(
    image: &image::DynamicImage,
    source_dims: Option<(u32, u32)>,
    dct: Option<ScaleStats>,
    orientation: u16,
) {
    CAPTURE.with_borrow_mut(|slot| {
        if let Some(value) = slot {
            assert!(
                value.is_none(),
                "evaluation expects exactly one source decode"
            );
            *value = Some(DecodedCover {
                image: image.clone(),
                native_dims: source_dims.unwrap_or((image.width(), image.height())),
                dct_scale_num: dct.map(|stats| stats.scale_num),
                orientation,
            });
        }
    });
}

pub(crate) fn decode(
    path: &Path,
    zip_entry: Option<&str>,
    display_px: u32,
    thumb_px: u32,
) -> Result<DecodedCover, String> {
    struct Reset;
    impl Drop for Reset {
        fn drop(&mut self) {
            CAPTURE.with_borrow_mut(|slot| *slot = None);
        }
    }
    CAPTURE.with_borrow_mut(|slot| {
        assert!(slot.is_none(), "evaluation decode must not be nested");
        *slot = Some(None);
    });
    let _reset = Reset;
    let metadata = std::fs::metadata(path).map_err(|error| error.to_string())?;
    let mut request = LoadRequest {
        path: path.to_owned(),
        zip_entry: zip_entry.map(str::to_owned),
        mtime: crate::ui_helpers::mtime_secs(&metadata),
        file_size: metadata.len() as i64,
        source_policy: LoadSourcePolicy::SourceOnly,
        // No manual crop: this is also automatic cropping's initial decode.
        // Auto's JPEG byte retention and decision occur after the capture tap.
        // Leave the instruction unset so evaluation does not run that decision twice.
        // Avoid consulting any configured book directory in an isolated evaluation.
        books_root: Some(path.parent().unwrap().join("unused-evaluation-books")),
        ..Default::default()
    };
    let (tx, _rx) = mpsc::channel();
    process_load_request(
        &mut request,
        &std::sync::RwLock::new(HashMap::new()),
        &tx,
        None::<&Arc<crate::catalog::CatalogDb>>,
        thumb_px,
        crate::settings::Settings::default().thumb_quality,
        display_px,
        CacheDecision::without_thumbnail(),
        &Arc::new(AtomicUsize::new(0)),
        &Arc::new(Mutex::new(crate::stats::ThumbStats::default())),
        None,
        &Arc::new(AtomicUsize::new(0)),
        &Arc::new(AtomicUsize::new(1)),
        None,
        None,
        None,
        None,
        None,
    );
    CAPTURE.with_borrow_mut(|slot| {
        slot.as_mut()
            .and_then(Option::take)
            .ok_or_else(|| format!("worker did not decode {path:?}, leaf={zip_entry:?}"))
    })
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
        for (display, expected_scale, expected_size) in
            [(256, 2, (256, 512)), (2048, 8, (1024, 2048))]
        {
            let decoded = decode(&path, Some("cover.jpg"), display, 512).unwrap();
            assert_eq!(decoded.dct_scale_num, Some(expected_scale));
            assert_eq!(
                (decoded.image.width(), decoded.image.height()),
                expected_size
            );
            assert_eq!(decoded.native_dims, (1024, 2048));
            assert_eq!(decoded.orientation, 6);
            let (raw, _) = decode_jpeg_turbo_scaled_from_bytes(&bytes, display.max(512)).unwrap();
            assert_eq!(decoded.image.to_rgb8(), raw.rotate90().to_rgb8());
        }
    }
}

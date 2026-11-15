//! Orientation and G1 come from one EXIF parse, never another metadata read.
use image::ImageDecoder;
use std::io::Cursor;
use std::path::Path;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct AnalysisExif {
    pub orientation: u16,
    pub shot_exif: bool,
}

impl Default for AnalysisExif {
    fn default() -> Self {
        Self {
            orientation: 1,
            shot_exif: false,
        }
    }
}

fn has_shot_exif(exif: &rexif::ExifData) -> bool {
    // Numeric tags in another IFD must not count as exposure information.
    exif.entries
        .iter()
        .any(|entry| entry.kind == rexif::IfdKind::Exif && matches!(entry.ifd.tag, 33434 | 33437))
}

fn from_rexif(exif: &rexif::ExifData, fallback: impl FnOnce() -> u16) -> AnalysisExif {
    AnalysisExif {
        orientation: exif
            .entries
            .iter()
            .find(|entry| entry.ifd.tag == 274)
            .and_then(crate::thumb_loader::orientation_from_rexif_entry)
            .unwrap_or_else(fallback),
        shot_exif: has_shot_exif(exif),
    }
}

fn from_tiff_payload(bytes: &[u8]) -> AnalysisExif {
    // image decoders return TIFF bytes; tolerate the optional Exif wrapper
    // used by some WebP writers without searching arbitrary image contents.
    let bytes = bytes.strip_prefix(b"Exif\0\0").unwrap_or(bytes);
    rexif::parse_buffer(bytes)
        .ok()
        .as_ref()
        // Decoder metadata extends G1 only. Its orientation is not part of
        // the released bytes decoder's rexif-only coordinate system.
        .map(|exif| AnalysisExif {
            orientation: 1,
            shot_exif: has_shot_exif(exif),
        })
        .unwrap_or_default()
}

pub(super) fn from_decoder(decoder: &mut impl ImageDecoder) -> AnalysisExif {
    decoder
        .exif_metadata()
        .ok()
        .flatten()
        .as_deref()
        .map(from_tiff_payload)
        .unwrap_or_default()
}

pub(super) fn from_bytes(bytes: &[u8]) -> AnalysisExif {
    if let Ok(exif) = rexif::parse_buffer(bytes) {
        return from_rexif(&exif, || 1);
    }
    // PNG eXIf / WebP EXIF are extracted from bytes the analysis already owns.
    // No path reopening or metadata-only full-file read is introduced.
    image::ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .ok()
        .and_then(|reader| reader.into_decoder().ok())
        .map(|mut decoder| from_decoder(&mut decoder))
        .unwrap_or_default()
}

pub(super) fn from_path(path: &Path) -> AnalysisExif {
    let parsed = crate::exif_reader::read_rexif_from_path(path);
    if let Some(exif) = parsed.exif {
        // Match read_exif_orientation_from_file's first-tag rule exactly,
        // including an invalid first tag followed by another orientation.
        return from_rexif(&exif, || {
            crate::thumb_loader::file_orientation_or_wic(None, path)
        });
    }
    // Preserve the existing WIC orientation fallback. G1 is unavailable for a
    // non-JPEG/TIFF path whose full raster was supplied by another caller:
    // rereading that file just to obtain EXIF is intentionally out of scope.
    AnalysisExif {
        orientation: crate::thumb_loader::file_orientation_or_wic(None, path),
        shot_exif: false,
    }
}

/// JPEG analysis already owns these bytes. Preserve the file decoder's WIC
/// fallback without parsing its rexif data or reading its prefix a second time.
pub(super) fn from_path_bytes(path: &Path, bytes: &[u8]) -> AnalysisExif {
    match crate::exif_reader::read_rexif_from_path_bytes(bytes) {
        Some(exif) => from_rexif(&exif, || {
            crate::thumb_loader::file_orientation_or_wic(None, path)
        }),
        None => AnalysisExif {
            orientation: crate::thumb_loader::file_orientation_or_wic(None, path),
            shot_exif: false,
        },
    }
}

#[cfg(test)]
pub(in crate::thumb_loader) mod tests {
    use super::*;
    use image::ImageEncoder;

    pub(in crate::thumb_loader) fn tiff_fixture(
        orientation: u16,
        tags: &[u16],
        shooting_ifd: bool,
    ) -> Vec<u8> {
        // IFD0 holds orientation and the Exif pointer. A deliberately misplaced
        // exposure tag in IFD0 exercises strict IFD ownership.
        let ifd0_count = if shooting_ifd { 2 } else { 1 + tags.len() };
        let exif_offset = 8 + 2 + ifd0_count * 12 + 4;
        let mut bytes = b"II*\0\x08\0\0\0".to_vec();
        bytes.extend_from_slice(&(ifd0_count as u16).to_le_bytes());
        let mut entry = |tag: u16, format: u16, value: u32| {
            bytes.extend_from_slice(&tag.to_le_bytes());
            bytes.extend_from_slice(&format.to_le_bytes());
            bytes.extend_from_slice(&1_u32.to_le_bytes());
            bytes.extend_from_slice(&value.to_le_bytes());
        };
        entry(274, 3, u32::from(orientation));
        if shooting_ifd {
            entry(34665, 4, exif_offset as u32);
        } else {
            for &tag in tags {
                entry(tag, 4, 1);
            }
        }
        bytes.extend_from_slice(&0_u32.to_le_bytes());
        if shooting_ifd {
            bytes.extend_from_slice(&(tags.len() as u16).to_le_bytes());
            let data_offset = exif_offset + 2 + tags.len() * 12 + 4;
            for (index, &tag) in tags.iter().enumerate() {
                bytes.extend_from_slice(&tag.to_le_bytes());
                bytes.extend_from_slice(&5_u16.to_le_bytes());
                bytes.extend_from_slice(&1_u32.to_le_bytes());
                bytes.extend_from_slice(&((data_offset + index * 8) as u32).to_le_bytes());
            }
            bytes.extend_from_slice(&0_u32.to_le_bytes());
            for _ in tags {
                bytes.extend_from_slice(&1_u32.to_le_bytes());
                bytes.extend_from_slice(&125_u32.to_le_bytes());
            }
        }
        bytes
    }

    pub(in crate::thumb_loader) fn jpeg_with_exif(jpeg: &[u8], exif: &[u8]) -> Vec<u8> {
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
        bytes.extend_from_slice(&((exif.len() + 8) as u16).to_be_bytes());
        bytes.extend_from_slice(b"Exif\0\0");
        bytes.extend_from_slice(exif);
        bytes.extend_from_slice(&jpeg[2..]);
        bytes
    }

    pub(in crate::thumb_loader) fn native_with_exif(webp: bool, exif: Vec<u8>) -> Vec<u8> {
        let pixels = image::RgbImage::from_pixel(90, 60, image::Rgb([20, 80, 180]));
        let mut bytes = Vec::new();
        if webp {
            let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut bytes);
            encoder.set_exif_metadata(exif).unwrap();
            encoder
                .write_image(&pixels, 90, 60, image::ExtendedColorType::Rgb8)
                .unwrap();
        } else {
            let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
            encoder.set_exif_metadata(exif).unwrap();
            encoder
                .write_image(&pixels, 90, 60, image::ExtendedColorType::Rgb8)
                .unwrap();
        }
        bytes
    }

    #[test]
    fn cover_auto_analysis_exif_requires_exif_ifd_and_extracts_orientation_in_same_parse() {
        for tags in [&[][..], &[33434][..], &[33437][..], &[33434, 33437][..]] {
            for orientation in 1..=8 {
                assert_eq!(
                    from_bytes(&tiff_fixture(orientation, tags, true)),
                    AnalysisExif {
                        orientation,
                        shot_exif: !tags.is_empty()
                    }
                );
            }
            assert!(!from_bytes(&tiff_fixture(6, tags, false)).shot_exif);
        }
        assert_eq!(from_bytes(b"no EXIF"), AnalysisExif::default());
    }

    #[test]
    fn cover_auto_analysis_legacy_orientation_uses_first_tag_then_fallback() {
        let valid = rexif::parse_buffer(&tiff_fixture(6, &[], true)).unwrap();
        assert_eq!(
            from_rexif(&valid, || panic!("valid rexif orientation wins")).orientation,
            6
        );
        let mut invalid = rexif::parse_buffer(&tiff_fixture(99, &[], true)).unwrap();
        invalid.entries.extend(
            valid
                .entries
                .into_iter()
                .filter(|entry| entry.ifd.tag == 274),
        );
        // The ordinary reader finds the first tag before validating it. A
        // later valid duplicate must not suppress the file's WIC fallback.
        assert_eq!(from_rexif(&invalid, || 7).orientation, 7);
    }

    #[test]
    fn cover_auto_analysis_owned_file_bytes_keep_legacy_jpeg_prefix_limit() {
        let suffix = jpeg_with_exif(
            &[0xff, 0xd8, 0xff, 0xda, 0, 2],
            &tiff_fixture(6, &[33434], true),
        );
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xe0, 0, 16];
        bytes.extend_from_slice(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        for _ in 0..260 {
            bytes.extend([0xff, 0xef, 0xff, 0xff]);
            bytes.resize(bytes.len() + 65_533, 0);
        }
        bytes.extend_from_slice(&suffix[2..]);
        // The legacy bytes reader has no file-prefix bound, while the file
        // reader deliberately stops before this late APP1 segment.
        assert_eq!(
            from_bytes(&bytes),
            AnalysisExif {
                orientation: 6,
                shot_exif: true
            }
        );
        assert!(crate::exif_reader::read_rexif_from_path_bytes(&bytes).is_none());
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("late-exif.jpg");
        std::fs::write(&path, &bytes).unwrap();
        assert_eq!(from_path_bytes(&path, &bytes), from_path(&path));
        assert_eq!(
            from_path_bytes(&path, &bytes).orientation,
            crate::thumb_loader::read_exif_orientation(&path)
        );
        assert!(!from_path_bytes(&path, &bytes).shot_exif);
    }

    #[test]
    fn cover_auto_analysis_jpeg_and_tiff_metadata_bytes_match_existing_path_prefix() {
        let temp = tempfile::tempdir().unwrap();
        for tags in [&[][..], &[33434][..], &[33437][..], &[33434, 33437][..]] {
            let tiff = tiff_fixture(8, tags, true);
            let jpeg = jpeg_with_exif(&[0xff, 0xd8, 0xff, 0xda, 0, 2], &tiff);
            for (name, bytes) in [("cover.tiff", &tiff), ("cover.jpg", &jpeg)] {
                let path = temp.path().join(name);
                std::fs::write(&path, bytes).unwrap();
                assert_eq!(from_path(&path), from_bytes(bytes));
                assert_eq!(from_path_bytes(&path, bytes), from_path(&path));
                assert_eq!(
                    from_path(&path).orientation,
                    crate::thumb_loader::read_exif_orientation(&path)
                );
                assert_eq!(from_path(&path).shot_exif, !tags.is_empty());
            }
        }
    }

    #[test]
    fn cover_auto_analysis_png_and_webp_extract_owned_decoder_exif() {
        for webp in [false, true] {
            for tags in [&[][..], &[33434][..], &[33437][..], &[33434, 33437][..]] {
                assert_eq!(
                    from_bytes(&native_with_exif(webp, tiff_fixture(6, tags, true))),
                    AnalysisExif {
                        orientation: 1,
                        shot_exif: !tags.is_empty()
                    }
                );
            }
        }
    }
}

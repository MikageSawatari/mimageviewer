//! Bounded dimension-only sampling for a representative jacket leaf.
use std::io::Read;
use std::path::{Path, PathBuf};

pub(crate) const MAX_SAMPLE_PAGES: usize = 8;
pub(crate) const MAX_SAMPLE_PREFIX_BYTES: u64 = 256 * 1024;

/// The representative never counts as a sample. Only a final representative
/// samples backwards; other representatives sample the remaining next pages.
pub(crate) fn sample_indices(len: usize, leaf: usize) -> Vec<usize> {
    if leaf >= len {
        return Vec::new();
    }
    if leaf + 1 == len {
        (0..leaf).rev().take(MAX_SAMPLE_PAGES).collect()
    } else {
        (leaf + 1..len).take(MAX_SAMPLE_PAGES).collect()
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PageAspectDecline {
    TooFew,
    Landscape,
    Inconsistent,
}

impl PageAspectDecline {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::TooFew => "too-few",
            Self::Landscape => "landscape",
            Self::Inconsistent => "inconsistent",
        }
    }
}

fn aspect_consensus(ratios: impl Iterator<Item = f64>) -> Result<f64, PageAspectDecline> {
    let ratios: Vec<_> = ratios.filter(|r| r.is_finite() && *r > 0.0).collect();
    if ratios.len() < 3 {
        return Err(PageAspectDecline::TooFew);
    }
    let mut portrait: Vec<_> = ratios.iter().copied().filter(|&r| r < 0.9).collect();
    if portrait.len() * 10 < ratios.len() * 7 {
        return Err(PageAspectDecline::Landscape);
    }
    portrait.sort_unstable_by(f64::total_cmp);
    let count = portrait.len();
    let median = (portrait[(count - 1) / 2] + portrait[count / 2]) * 0.5;
    let consistent = portrait
        .iter()
        .filter(|&&r| (r - median).abs() <= median * 0.06)
        .count();
    if consistent * 10 >= count * 7 {
        Ok(median)
    } else {
        Err(PageAspectDecline::Inconsistent)
    }
}

/// Keep unknown slots until after the leading run is removed: an unknown
/// page stops the run, rather than exposing a later landscape as a new first.
fn aspect_from_slots(
    slots: impl IntoIterator<Item = Option<f64>>,
) -> Result<f64, PageAspectDecline> {
    let slots: Vec<_> = slots
        .into_iter()
        .map(|r| r.filter(|r| r.is_finite() && *r > 0.0))
        .collect();
    let skip = slots
        .iter()
        .take(3)
        .take_while(|r| r.is_some_and(|r| r >= 0.9))
        .count();
    aspect_consensus(slots.into_iter().skip(skip).flatten())
}

pub(crate) fn page_aspect_from_sizes(sizes: &[(f32, f32)], leaf: usize) -> Option<f64> {
    page_aspect_from_sizes_report(sizes, leaf).ok()
}

pub(crate) fn page_aspect_from_sizes_report(
    sizes: &[(f32, f32)],
    leaf: usize,
) -> Result<f64, PageAspectDecline> {
    aspect_from_slots(sample_indices(sizes.len(), leaf).into_iter().map(|index| {
        let (width, height) = sizes[index];
        (width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0)
            .then_some(f64::from(width) / f64::from(height))
    }))
}

/// Read no more than eight 256 KiB prefixes, without nested archive expansion,
/// full-entry fallback, page decoding, or direct-RAR reads.
pub(crate) fn sample_zip_page_aspect(
    path: &Path,
    leaf_entry: &str,
    prefix: Option<&str>,
) -> Option<f64> {
    sample_zip_page_aspect_report(path, leaf_entry, prefix).ok()
}

pub(crate) fn sample_zip_page_aspect_report(
    path: &Path,
    leaf_entry: &str,
    prefix: Option<&str>,
) -> Result<f64, PageAspectDecline> {
    sample_zip_pages(path, leaf_entry, prefix)
}

pub(crate) fn sample_zip_pages(
    path: &Path,
    leaf_entry: &str,
    prefix: Option<&str>,
) -> Result<f64, PageAspectDecline> {
    if crate::rar_loader::is_rar_path(path)
        || crate::zip_loader::split_nested_zip_path(leaf_entry).len() > 1
    {
        return Err(PageAspectDecline::TooFew);
    }
    let prefixes = crate::zip_loader::read_cover_auto_page_prefixes(path, leaf_entry, prefix)
        .unwrap_or_default();
    aspect_from_slots(prefixes.into_iter().map(|(name, bytes)| {
        let (width, height) = dimensions_from_prefix(bytes.as_deref()?, &name)?;
        Some(width / height)
    }))
}

fn dimensions_from_prefix(bytes: &[u8], name: &str) -> Option<(f64, f64)> {
    let (width, height) = crate::app::probe_image_dims_from_bytes(bytes, name)?;
    (width > 0 && height > 0).then_some((f64::from(width), f64::from(height)))
}

fn read_folder_prefix(path: &Path) -> Option<Vec<u8>> {
    let file = std::fs::File::open(path).ok()?;
    let mut bytes = Vec::new();
    file.take(MAX_SAMPLE_PREFIX_BYTES)
        .read_to_end(&mut bytes)
        .ok()?;
    Some(bytes)
}

/// The caller supplies the already enumerated image paths in FileNameSortKey
/// order. Do not enumerate or sort again: this is the representative worker's
/// same direct-child snapshot, and only eight bounded reads are allowed. On the
/// worker only, a missing rexif orientation may query WIC once per sampled file.
pub(crate) fn sample_folder_pages(
    paths: &[PathBuf],
    cover: &Path,
) -> Result<f64, PageAspectDecline> {
    sample_folder_pages_with_wic(paths, cover, |path| {
        crate::thumb_loader::file_orientation_or_wic(None, path)
    })
}

fn sample_folder_pages_with_wic(
    paths: &[PathBuf],
    cover: &Path,
    mut wic_orientation: impl FnMut(&Path) -> u16,
) -> Result<f64, PageAspectDecline> {
    let leaf = paths
        .iter()
        .position(|path| crate::path_key::eq_keep_drive(path, cover));
    let Some(leaf) = leaf else {
        return Err(PageAspectDecline::TooFew);
    };
    aspect_from_slots(sample_indices(paths.len(), leaf).into_iter().map(|index| {
        let path = &paths[index];
        let bytes = read_folder_prefix(path)?;
        let (width, height) = folder_dimensions_from_prefix(&bytes, path, &mut wic_orientation)?;
        Some(width / height)
    }))
}

fn folder_dimensions_from_prefix(
    bytes: &[u8],
    path: &Path,
    wic_orientation: &mut impl FnMut(&Path) -> u16,
) -> Option<(f64, f64)> {
    // This probe has already applied rexif, or native RAW/AVIF orientation.
    // Never apply that transform twice. Reuse the owned prefix for the
    // presence check; no second file read is needed for rexif.
    let (mut width, mut height) = dimensions_from_prefix(bytes, &path.to_string_lossy())?;
    if crate::raw_format::is_raw_path(path)
        || path
            .extension()
            .and_then(|ext| ext.to_str())
            .is_some_and(|ext| ext.eq_ignore_ascii_case("avif"))
        || crate::thumb_loader::read_exif_orientation_from_bytes_if_present(bytes).is_some()
    {
        return Some((width, height));
    }
    if (5..=8).contains(&wic_orientation(path)) {
        std::mem::swap(&mut width, &mut height);
    }
    Some((width, height))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Cursor, Write};

    fn png(width: u32, height: u32) -> Vec<u8> {
        let mut output = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::new(width, height))
            .write_to(&mut output, image::ImageFormat::Png)
            .unwrap();
        output.into_inner()
    }

    fn oriented_image(extension: &str, width: u32, height: u32, orientation: u16) -> Vec<u8> {
        use image::ImageEncoder;
        let mut exif = b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
        exif.extend(orientation.to_le_bytes());
        exif.extend([0; 6]);
        let pixels = image::RgbImage::from_pixel(width, height, image::Rgb([30, 80, 140]));
        let mut bytes = Vec::new();
        match extension {
            "webp" => {
                let mut encoder = image::codecs::webp::WebPEncoder::new_lossless(&mut bytes);
                encoder.set_exif_metadata(exif).unwrap();
                encoder
                    .write_image(&pixels, width, height, image::ExtendedColorType::Rgb8)
                    .unwrap();
            }
            "png" => {
                let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
                encoder.set_exif_metadata(exif).unwrap();
                encoder
                    .write_image(&pixels, width, height, image::ExtendedColorType::Rgb8)
                    .unwrap();
            }
            "jpg" => {
                image::DynamicImage::ImageRgb8(pixels)
                    .write_to(&mut Cursor::new(&mut bytes), image::ImageFormat::Jpeg)
                    .unwrap();
                let mut app1 = vec![0xff, 0xe1];
                app1.extend(((exif.len() + 8) as u16).to_be_bytes());
                app1.extend(b"Exif\0\0");
                app1.extend(exif);
                bytes.splice(2..2, app1);
            }
            _ => unreachable!(),
        }
        bytes
    }

    #[test]
    fn folder_webp_orientation_matches_cover_file_decode_and_zip_stays_bytes_only() {
        let dir = tempfile::tempdir().unwrap();
        let bytes = oriented_image("webp", 900, 600, 6);
        let paths: Vec<_> = (0..11)
            .map(|index| {
                let path = dir.path().join(format!("{index:03}.webp"));
                std::fs::write(&path, &bytes).unwrap();
                path
            })
            .collect();
        assert_eq!(
            crate::app::probe_image_dims_from_bytes(&bytes, "page.webp"),
            Some((900, 600))
        );
        let cover = crate::thumb_loader::decode_cover_auto_analysis_image(
            crate::image_decode::Source::Path(&paths[0]),
            None,
            None,
        )
        .unwrap();
        // This Windows WIC decoder exposes WebP's orientation, unlike rexif.
        #[cfg(windows)]
        assert_eq!(cover.native_dims, (600, 900));
        let sample = sample_folder_pages(&paths, &paths[0]);
        assert_eq!(
            folder_dimensions_from_prefix(&bytes, &paths[1], &mut |path| {
                crate::thumb_loader::file_orientation_or_wic(None, path)
            }),
            Some((
                f64::from(cover.native_dims.0),
                f64::from(cover.native_dims.1)
            ))
        );
        assert_eq!(
            sample,
            aspect_from_slots(
                [Some(f64::from(cover.native_dims.0) / f64::from(cover.native_dims.1)); 8]
            )
        );
        #[cfg(windows)]
        assert_eq!(sample, Ok(2.0 / 3.0));
        let archive = zip((0..11)
            .map(|i| (format!("{i:03}.webp"), bytes.clone()))
            .collect());
        let zip_sample = sample_zip_pages(archive.path(), "000.webp", None);
        assert_eq!(
            dimensions_from_prefix(&bytes, "001.webp"),
            Some((900.0, 600.0))
        );
        assert_eq!(zip_sample, Err(PageAspectDecline::Landscape));
    }

    #[test]
    fn folder_png_and_jpeg_orientation6_keep_released_dimensions() {
        for (extension, stored) in [("png", (70, 100)), ("jpg", (100, 70))] {
            let dir = tempfile::tempdir().unwrap();
            let bytes = oriented_image(extension, stored.0, stored.1, 6);
            let paths: Vec<_> = (0..4)
                .map(|index| {
                    let path = dir.path().join(format!("{index:03}.{extension}"));
                    std::fs::write(&path, &bytes).unwrap();
                    path
                })
                .collect();
            let cover = crate::thumb_loader::decode_cover_auto_analysis_image(
                crate::image_decode::Source::Path(&paths[0]),
                None,
                None,
            )
            .unwrap();
            assert_eq!(cover.native_dims, (70, 100));
            let sample = sample_folder_pages(&paths, &paths[0]);
            assert_eq!(
                folder_dimensions_from_prefix(&bytes, &paths[1], &mut |path| {
                    crate::thumb_loader::file_orientation_or_wic(None, path)
                }),
                Some((70.0, 100.0))
            );
            assert_eq!(sample, Ok(0.7));
        }
    }

    #[test]
    fn folder_wic_fallback_only_for_missing_rexif_and_never_double_rotates() {
        for orientation in [1, 6] {
            let bytes = oriented_image("jpg", 100, 70, orientation);
            let result = folder_dimensions_from_prefix(&bytes, Path::new("page.jpg"), &mut |_| {
                panic!("valid rexif must not open WIC")
            });
            assert_eq!(
                result,
                Some(if orientation == 6 {
                    (70.0, 100.0)
                } else {
                    (100.0, 70.0)
                })
            );
        }
        assert_eq!(
            folder_dimensions_from_prefix(
                b"invalid header",
                Path::new("bad.png"),
                &mut |_| panic!("no dimensions must not open WIC")
            ),
            None
        );
        let mut calls = 0;
        let mut fallback = |_: &Path| {
            calls += 1;
            6
        };
        assert_eq!(
            folder_dimensions_from_prefix(&png(100, 70), Path::new("plain.png"), &mut fallback),
            Some((70.0, 100.0))
        );
        assert_eq!(calls, 1);
    }

    #[test]
    fn folder_wic_budget_keeps_eight_attempts_and_256k_prefixes() {
        let dir = tempfile::tempdir().unwrap();
        let mut bytes = png(100, 70);
        bytes.resize(300 * 1024, 0);
        let paths: Vec<_> = (0..12)
            .map(|i| {
                let path = dir.path().join(format!("{i:03}.png"));
                std::fs::write(&path, &bytes).unwrap();
                path
            })
            .collect();
        assert_eq!(
            read_folder_prefix(&paths[1]).unwrap().len(),
            MAX_SAMPLE_PREFIX_BYTES as usize
        );
        let mut opened = Vec::new();
        let sample = sample_folder_pages_with_wic(&paths, &paths[0], |path| {
            opened.push(path.to_owned());
            6
        });
        assert_eq!(opened, paths[1..=8]);
        assert_eq!(
            folder_dimensions_from_prefix(&bytes, &paths[1], &mut |_| 6),
            Some((70.0, 100.0))
        );
        assert_eq!(sample, Ok(0.7));
    }

    fn zip(entries: Vec<(String, Vec<u8>)>) -> tempfile::NamedTempFile {
        let file = tempfile::NamedTempFile::new().unwrap();
        let mut writer = zip::ZipWriter::new(file.reopen().unwrap());
        let options = zip::write::SimpleFileOptions::default()
            .compression_method(zip::CompressionMethod::Stored);
        for (name, bytes) in entries {
            writer.start_file(name, options).unwrap();
            writer.write_all(&bytes).unwrap();
        }
        writer.finish().unwrap();
        file
    }

    #[test]
    fn pdf_sample_preserves_unknown_slots_until_leading_trim() {
        let sizes = [
            (300.0, 100.0),
            (140.0, 100.0),
            (f32::NAN, 100.0),
            (140.0, 100.0),
            (140.0, 100.0),
            (140.0, 100.0),
            (70.0, 100.0),
            (70.0, 100.0),
            (70.0, 100.0),
        ];
        assert_eq!(
            page_aspect_from_sizes_report(&sizes, 0),
            Err(PageAspectDecline::Landscape)
        );
    }

    #[test]
    fn folder_uses_supplied_name_order_and_eight_attempts_without_rescan() {
        let dir = tempfile::tempdir().unwrap();
        let mut paths = Vec::new();
        for i in [10, 0, 9, 2, 3, 1, 4, 5, 6, 7, 8, 11] {
            let path = dir.path().join(format!("{i:02}.png"));
            let bytes = if i == 1 {
                b"unknown".to_vec()
            } else {
                png(60 + i, 100)
            };
            std::fs::write(&path, bytes).unwrap();
            paths.push(path);
        }
        paths.sort_by_cached_key(|path| {
            crate::filename_sort::FileNameSortKey::new(&path.file_name().unwrap().to_string_lossy())
        });
        assert_eq!(paths[2].file_name().unwrap(), "02.png");
        assert_eq!(paths[10].file_name().unwrap(), "10.png");
        // This direct child is deliberately absent from the supplied snapshot.
        std::fs::write(dir.path().join("00a.png"), png(200, 100)).unwrap();
        let mut opened = Vec::new();
        let sample = sample_folder_pages_with_wic(&paths, &paths[0], |path| {
            opened.push(path.to_owned());
            1
        });
        assert_eq!(sample, Ok(0.65));
        // The unknown first slot consumes an attempt without querying WIC.
        assert_eq!(opened, paths[2..=8]);
        opened.clear();
        let backwards = sample_folder_pages_with_wic(&paths, &paths[11], |path| {
            opened.push(path.to_owned());
            1
        });
        assert_eq!(backwards, Ok((0.66 + 0.67) * 0.5));
        assert_eq!(
            opened,
            paths[3..=10].iter().rev().cloned().collect::<Vec<_>>()
        );
        assert_eq!(
            sample_folder_pages(&paths, &dir.path().join("missing.png")),
            Err(PageAspectDecline::TooFew)
        );
    }

    #[test]
    fn folder_prefix_read_is_bounded_and_failed_reads_remain_slots() {
        let dir = tempfile::tempdir().unwrap();
        let cover = dir.path().join("0.png");
        let large = dir.path().join("1.png");
        let missing = dir.path().join("2.png");
        std::fs::write(&large, vec![0; 300 * 1024]).unwrap();
        assert_eq!(
            read_folder_prefix(&large).unwrap().len(),
            MAX_SAMPLE_PREFIX_BYTES as usize
        );
        let mut paths = vec![cover.clone(), large, missing];
        for i in 3..9 {
            let path = dir.path().join(format!("{i}.png"));
            std::fs::write(&path, png(if i < 6 { 140 } else { 70 }, 100)).unwrap();
            paths.push(path);
        }
        // Neither the unreadable header nor the failed read may expose the
        // later landscape run to leading-slot trimming.
        assert_eq!(
            sample_folder_pages(&paths, &cover),
            Err(PageAspectDecline::Landscape)
        );
    }

    #[test]
    fn zip_sample_preserves_unknown_slots_until_leading_trim() {
        let file = zip(vec![
            ("0.png".into(), png(300, 100)),
            ("1.png".into(), png(140, 100)),
            ("2.png".into(), b"unknown".to_vec()),
            ("3.png".into(), png(140, 100)),
            ("4.png".into(), png(140, 100)),
            ("5.png".into(), png(140, 100)),
            ("6.png".into(), png(70, 100)),
            ("7.png".into(), png(70, 100)),
            ("8.png".into(), png(70, 100)),
        ]);
        let sample = sample_zip_pages(file.path(), "0.png", None);
        assert_eq!(sample, Err(PageAspectDecline::Landscape));
        assert_eq!(
            sample_zip_page_aspect_report(file.path(), "0.png", None),
            sample
        );
    }

    #[test]
    fn next_pages_last_page_and_consensus_boundaries() {
        assert_eq!(sample_indices(12, 0), (1..9).collect::<Vec<_>>());
        assert_eq!(sample_indices(12, 11), (3..11).rev().collect::<Vec<_>>());
        assert_eq!(sample_indices(12, 10), vec![11]);
        assert!(sample_indices(12, 12).is_empty());
        let mut sizes = vec![(70.0, 100.0); 11];
        sizes[0] = (400.0, 100.0);
        assert_eq!(page_aspect_from_sizes(&sizes, 0), Some(0.7));
        assert_eq!(page_aspect_from_sizes(&sizes, 10), Some(0.7));
        assert_eq!(page_aspect_from_sizes(&sizes, 9), None);
        sizes[1] = (200.0, 100.0);
        sizes[2] = (200.0, 100.0);
        assert_eq!(page_aspect_from_sizes(&sizes, 0), Some(0.7)); // 6/8 portrait
        sizes[3] = (200.0, 100.0);
        assert_eq!(page_aspect_from_sizes(&sizes, 0), Some(0.7)); // leading three omitted
        assert_eq!(
            aspect_consensus([0.5, 0.6, 0.7, 0.8].into_iter()),
            Err(PageAspectDecline::Inconsistent)
        );
        assert_eq!(
            aspect_consensus([0.7, 0.7].into_iter()),
            Err(PageAspectDecline::TooFew)
        );
        assert_eq!(
            aspect_consensus([0.9, 0.9, 0.9].into_iter()),
            Err(PageAspectDecline::Landscape)
        );
    }

    #[test]
    fn leading_landscape_slots_pdf_zip_forward_backward_and_typed_reasons() {
        use PageAspectDecline::*;
        let mut cases = Vec::new();
        for count in 1..=3 {
            cases.push((
                vec![Some(1.4); count]
                    .into_iter()
                    .chain(vec![Some(0.7); 8 - count])
                    .collect::<Vec<_>>(),
                Ok(0.7),
            ));
        }
        cases.extend([
            (
                vec![
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(0.7),
                    Some(0.7),
                    Some(0.7),
                    Some(0.7),
                ],
                Ok(0.7),
            ),
            (
                vec![
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(0.7),
                    Some(0.7),
                ],
                Err(Landscape),
            ),
            (
                vec![
                    Some(0.7),
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(0.7),
                    Some(0.7),
                    Some(0.7),
                    Some(0.7),
                ],
                Err(Landscape),
            ),
            (vec![Some(1.4); 8], Err(Landscape)),
            (
                vec![
                    None,
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(0.7),
                    Some(0.7),
                    Some(0.7),
                ],
                Err(Landscape),
            ),
            (
                vec![
                    Some(1.4),
                    None,
                    Some(1.4),
                    Some(1.4),
                    Some(1.4),
                    Some(0.7),
                    Some(0.7),
                    Some(0.7),
                ],
                Err(Landscape),
            ),
            (vec![Some(0.7); 2], Err(TooFew)),
            (
                vec![Some(0.5), Some(0.6), Some(0.7), Some(0.8)],
                Err(Inconsistent),
            ),
        ]);
        for (slots, expected) in cases {
            assert_eq!(aspect_from_slots(slots.clone()), expected, "{slots:?}");
            for backwards in [false, true] {
                let mut values = vec![Some(3.0)];
                values.extend(slots.iter().copied());
                if backwards {
                    values.reverse();
                }
                let leaf = if backwards { values.len() - 1 } else { 0 };
                let sizes: Vec<_> = values
                    .iter()
                    .map(|r| r.map_or((0.0, 0.0), |r| ((r * 100.0) as f32, 100.0)))
                    .collect();
                assert_eq!(
                    page_aspect_from_sizes_report(&sizes, leaf),
                    expected,
                    "PDF {slots:?}/{backwards}"
                );
                let entries = values
                    .iter()
                    .enumerate()
                    .map(|(i, r)| {
                        (
                            format!("{i:03}.png"),
                            r.map_or_else(|| b"unknown".to_vec(), |r| png((r * 100.0) as u32, 100)),
                        )
                    })
                    .collect();
                let file = zip(entries);
                let prefixes = crate::zip_loader::read_cover_auto_page_prefixes(
                    file.path(),
                    &format!("{leaf:03}.png"),
                    None,
                )
                .unwrap();
                assert_eq!(prefixes.len(), slots.len());
                assert_eq!(
                    sample_zip_page_aspect_report(file.path(), &format!("{leaf:03}.png"), None),
                    expected,
                    "ZIP {slots:?}/{backwards}"
                );
            }
        }
    }

    #[test]
    fn zip_prefix_budget_is_eight_attempts_not_eight_successes() {
        let entries = (0..12)
            .map(|i| (format!("{i:03}.png"), vec![0; 300 * 1024]))
            .collect();
        let file = zip(entries);
        let prefixes =
            crate::zip_loader::read_cover_auto_page_prefixes(file.path(), "000.png", None).unwrap();
        assert_eq!(prefixes.len(), 8);
        assert!(
            prefixes
                .iter()
                .all(|(_, bytes)| bytes.as_ref().unwrap().len() == MAX_SAMPLE_PREFIX_BYTES as usize)
        );
        assert_eq!(prefixes.first().unwrap().0, "001.png");
        assert_eq!(prefixes.last().unwrap().0, "008.png");
        assert_eq!(
            prefixes
                .iter()
                .map(|(_, bytes)| bytes.as_ref().unwrap().len())
                .sum::<usize>(),
            8 * 256 * 1024
        );
        assert_eq!(sample_zip_page_aspect(file.path(), "000.png", None), None);
    }

    #[test]
    fn zip_reads_only_selected_prefix_and_skips_nested_and_ignored_entries() {
        let mut entries = vec![
            ("book/000.png".into(), png(300, 100)),
            ("book/001.png".into(), b"invalid header".to_vec()),
            ("book/003.txt".into(), b"text".to_vec()),
            ("book/__MACOSX/004.png".into(), png(200, 100)),
            (
                "book/nested.zip".into(),
                b"not a zip; must not be expanded".to_vec(),
            ),
            ("book/nested.zip/005.png".into(), png(200, 100)),
            ("other/006.png".into(), png(200, 100)),
        ];
        for i in [2, 4, 6, 7] {
            entries.push((format!("book/{i:03}.png"), png(70, 100)));
        }
        let file = zip(entries);
        assert_eq!(
            sample_zip_page_aspect(file.path(), "book/000.png", Some("book/")),
            Some(0.7)
        );
        assert_eq!(
            sample_zip_page_aspect(file.path(), "book/007.png", Some("book")),
            Some(0.7)
        );
        assert_eq!(
            sample_zip_page_aspect(file.path(), "book/nested.zip/005.png", Some("book/")),
            None
        );
        assert_eq!(
            sample_zip_page_aspect(file.path(), "absent.png", None),
            None
        );
    }

    #[test]
    fn zip_probe_applies_exif_orientation_before_consensus() {
        let mut jpeg = Cursor::new(Vec::new());
        image::DynamicImage::ImageRgb8(image::RgbImage::new(100, 70))
            .write_to(&mut jpeg, image::ImageFormat::Jpeg)
            .unwrap();
        // APP1 Exif, little-endian TIFF, one SHORT orientation=6 entry.
        let exif: &[u8] = b"Exif\0\0II\x2a\x00\x08\x00\x00\x00\x01\x00\x12\x01\x03\x00\x01\x00\x00\x00\x06\x00\x00\x00\x00\x00\x00\x00";
        let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
        bytes.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
        bytes.extend_from_slice(exif);
        bytes.extend_from_slice(&jpeg.into_inner()[2..]);
        let file = zip((0..4)
            .map(|i| (format!("{i:03}.jpg"), bytes.clone()))
            .collect());
        assert_eq!(
            sample_zip_page_aspect(file.path(), "000.jpg", None),
            Some(0.7)
        );
        let dir = tempfile::tempdir().unwrap();
        let paths: Vec<_> = (0..4)
            .map(|i| {
                let path = dir.path().join(format!("{i:03}.jpg"));
                std::fs::write(&path, &bytes).unwrap();
                path
            })
            .collect();
        let sample = sample_folder_pages(&paths, &paths[0]);
        assert_eq!(sample, Ok(0.7));
        assert_eq!(
            dimensions_from_prefix(&bytes, "001.jpg"),
            Some((70.0, 100.0))
        );
    }
}

//! Bounded dimension-only sampling for a representative jacket leaf.
use std::path::Path;

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

fn aspect_consensus(ratios: impl Iterator<Item = f64>) -> Option<f64> {
    let ratios: Vec<_> = ratios.filter(|r| r.is_finite() && *r > 0.0).collect();
    if ratios.len() < 3 {
        return None;
    }
    let mut portrait: Vec<_> = ratios.iter().copied().filter(|&r| r < 0.9).collect();
    if portrait.len() * 10 < ratios.len() * 7 {
        return None;
    }
    portrait.sort_unstable_by(f64::total_cmp);
    let count = portrait.len();
    let median = (portrait[(count - 1) / 2] + portrait[count / 2]) * 0.5;
    let consistent = portrait
        .iter()
        .filter(|&&r| (r - median).abs() <= median * 0.06)
        .count();
    (consistent * 10 >= count * 7).then_some(median)
}

pub(crate) fn page_aspect_from_sizes(sizes: &[(f32, f32)], leaf: usize) -> Option<f64> {
    aspect_consensus(
        sample_indices(sizes.len(), leaf)
            .into_iter()
            .filter_map(|index| {
                let (width, height) = sizes[index];
                (width.is_finite() && height.is_finite() && width > 0.0 && height > 0.0)
                    .then_some(f64::from(width) / f64::from(height))
            }),
    )
}

/// Read no more than eight 256 KiB prefixes, without nested archive expansion,
/// full-entry fallback, page decoding, or direct-RAR reads.
pub(crate) fn sample_zip_page_aspect(
    path: &Path,
    leaf_entry: &str,
    prefix: Option<&str>,
) -> Option<f64> {
    if crate::rar_loader::is_rar_path(path)
        || crate::zip_loader::split_nested_zip_path(leaf_entry).len() > 1
    {
        return None;
    }
    let prefixes =
        crate::zip_loader::read_cover_auto_page_prefixes(path, leaf_entry, prefix).ok()?;
    aspect_consensus(prefixes.into_iter().filter_map(|(name, bytes)| {
        let (width, height) = crate::app::probe_image_dims_from_bytes(&bytes, &name)?;
        (width > 0 && height > 0).then_some(f64::from(width) / f64::from(height))
    }))
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
        assert_eq!(page_aspect_from_sizes(&sizes, 0), None); // 5/8 portrait
        assert_eq!(aspect_consensus([0.5, 0.6, 0.7, 0.8].into_iter()), None);
        assert_eq!(aspect_consensus([0.7, 0.7].into_iter()), None);
        assert_eq!(aspect_consensus([0.9, 0.9, 0.9].into_iter()), None);
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
                .all(|(_, bytes)| bytes.len() == MAX_SAMPLE_PREFIX_BYTES as usize)
        );
        assert_eq!(prefixes.first().unwrap().0, "001.png");
        assert_eq!(prefixes.last().unwrap().0, "008.png");
        assert_eq!(
            prefixes.iter().map(|(_, bytes)| bytes.len()).sum::<usize>(),
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
    }
}

use super::tests::{png, zip};
use super::*;

pub(crate) fn fixture_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("testdata/archives/cover-auto-rar")
}

/// Shared with the existing worker regression's synthetic connected jacket.
pub(crate) fn fixture_jacket() -> image::DynamicImage {
    image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(892, 512, |x, y| {
        image::Rgb(if x < 140 {
            if y % 73 < 8 {
                [65, 65, 65]
            } else {
                [246, 242, 220]
            }
        } else if x < 498 {
            [
                35 + (y % 80) as u8,
                55 + (y % 155) as u8,
                200 - (y % 80) as u8,
            ]
        } else if x < 534 {
            [18, 28, 38]
        } else if y % 73 < 8 {
            [75, 75, 75]
        } else {
            [246, 242, 220]
        })
    }))
}

fn raw_samples(
    path: &Path,
    leaf: &str,
    prefix: Option<&str>,
    entry: u64,
    total: u64,
) -> (Vec<(String, SampleRead)>, SampleMetrics) {
    let mut metrics = SampleMetrics::default();
    let slots = crate::rar_loader::read_cover_auto_page_samples(
        path,
        leaf,
        prefix,
        None,
        entry,
        total,
        &mut metrics,
    )
    .unwrap();
    (slots, metrics)
}

#[test]
fn shared_archive_selection_preserves_zip_dot_name_and_prefix_rules() {
    let names = [
        "book/.png",
        "book/001.PNG",
        "book/002.jpg",
        "book/999.png",
        "book/__MACOSX/003.png",
        "book/inner.zip/004.png",
        "book/05.png/readme",
        "book/readme.txt",
        "other/006.png",
        ".png",
    ];
    let selected = select_archive_samples(
        names.into_iter().map(|name| (name.to_owned(), 1)),
        "book/999.png",
        Some("book"),
    );
    let mut names: Vec<_> = selected.into_iter().map(|(name, _)| name).collect();
    names.sort();
    assert_eq!(names, ["book/.png", "book/001.PNG", "book/002.jpg"]);
    assert!(select_archive_samples([("book/999.png".into(), 1)], "absent.png", None).is_empty());
}

#[test]
fn rar_sampler_matches_zip_selection_and_reads_one_processing_scan() {
    let path = fixture_root().join("jacket.rar");
    let before = crate::rar_loader::sample_processing_scans_for_test();
    let (slots, metrics) = raw_samples(&path, "000.png", None, u64::MAX, u64::MAX);
    assert_eq!(
        crate::rar_loader::sample_processing_scans_for_test() - before,
        1
    );
    assert_eq!(metrics.selected, 8);
    assert_eq!(metrics.read, 8);
    assert_eq!(metrics.skipped_limit, 0);
    let names: Vec<_> = slots.iter().map(|(name, _)| name.clone()).collect();
    let entries = crate::rar_loader::enumerate_image_entries_detailed(&path).unwrap();
    assert_eq!(
        names,
        select_archive_samples(
            entries
                .entries
                .into_iter()
                .map(|entry| { (entry.entry_name, entry.uncompressed_size) }),
            "000.png",
            None
        )
        .into_iter()
        .map(|(name, _)| name)
        .collect::<Vec<_>>()
    );
    let zip = zip((0..9)
        .map(|i| (format!("{i:03}.png"), png(70, 100)))
        .collect());
    let mut zip_metrics = SampleMetrics::default();
    let zip_slots = crate::zip_loader::read_cover_auto_page_samples(
        zip.path(),
        "000.png",
        None,
        None,
        &mut zip_metrics,
    )
    .unwrap();
    assert_eq!(
        names,
        zip_slots
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>()
    );
    assert_eq!(sample_zip_pages(&path, "000.png", None), Ok(0.7));
    assert_eq!(sample_zip_pages(&path, "008.png", None), Ok(0.7));
}

#[test]
fn rar_sample_scope_duplicate_names_and_saved_rotation_use_viewer_keys() {
    let scoped = fixture_root().join("zipdir.rar");
    assert_eq!(
        sample_zip_pages(&scoped, "book/000.png", Some("book")),
        Ok(0.7)
    );
    assert_eq!(
        sample_zip_pages(&scoped, "book/000.png", Some("other")),
        Err(PageAspectDecline::TooFew)
    );
    let logical = Path::new("G:/Synthetic/Resolved.RAR");
    let mut keys = Vec::new();
    let sample = sample_container_pages_with_rotations_cancellable(
        &scoped,
        "book/000.png",
        Some("book/"),
        logical,
        |key| {
            keys.push(key.to_owned());
            Rotation::Cw90
        },
        None,
    );
    assert_eq!(sample, Ok(Err(PageAspectDecline::Landscape)));
    assert_eq!(
        keys,
        (1..9)
            .map(|i| crate::adjustment_db::zip_entry_key(logical, &format!("book/{i:03}.png")))
            .collect::<Vec<_>>()
    );
    let duplicate = fixture_root().join("duplicate-names.rar");
    let (slots, _) = raw_samples(&duplicate, "page.png", None, u64::MAX, u64::MAX);
    assert_eq!(slots.len(), 1);
    assert_eq!(slots[0].0, "page (2).png");
    let SampleRead::Read(bytes) = &slots[0].1 else {
        panic!("duplicate must be read")
    };
    assert_eq!(
        *bytes,
        crate::rar_loader::read_entry_bytes(&duplicate, "page (2).png").unwrap()
    );
}

#[test]
fn rar_sampler_limits_are_inclusive_and_fixed_before_extraction() {
    let path = fixture_root().join("jacket.rar");
    let entries = crate::rar_loader::enumerate_image_entries_detailed(&path).unwrap();
    let size = entries
        .entries
        .iter()
        .find(|entry| entry.entry_name == "001.png")
        .unwrap()
        .uncompressed_size;
    let (_, exact) = raw_samples(&path, "000.png", None, size, size * 3);
    assert_eq!((exact.selected, exact.read, exact.skipped_limit), (8, 3, 5));
    assert_eq!(exact.bytes as u64, size * 3);
    let (slots, total_short) = raw_samples(&path, "000.png", None, size, size * 3 - 1);
    assert_eq!((total_short.read, total_short.skipped_limit), (2, 6));
    assert!(
        slots[2..]
            .iter()
            .all(|(_, read)| matches!(read, SampleRead::Unreadable))
    );
    let before = crate::rar_loader::sample_processing_scans_for_test();
    let (_, entry_short) = raw_samples(&path, "000.png", None, size - 1, u64::MAX);
    assert_eq!((entry_short.read, entry_short.skipped_limit), (0, 8));
    assert_eq!(
        crate::rar_loader::sample_processing_scans_for_test(),
        before
    );
}

#[test]
fn rar_sampler_cover_only_and_two_pages_are_judged_declines() {
    for name in ["cover-only.rar", "two-pages.rar"] {
        let path = fixture_root().join(name);
        assert_eq!(
            sample_container_pages_with_rotations_cancellable(
                &path,
                "000.png",
                None,
                &path,
                |_| Rotation::None,
                None
            ),
            Ok(Err(PageAspectDecline::TooFew))
        );
    }
}

#[test]
fn sample_boundaries_keep_os_errors_cancel_and_data_separate() {
    use std::io::{Error, ErrorKind};
    for kind in [
        ErrorKind::InvalidData,
        ErrorKind::UnexpectedEof,
        ErrorKind::Unsupported,
    ] {
        assert_eq!(
            classify_io_error(&Error::new(kind, "synthetic")),
            SampleFailureKind::Data
        );
        assert!(matches!(
            SampleRead::from_io(Err(Error::new(kind, "synthetic")), None),
            SampleRead::Unreadable
        ));
    }
    for kind in [
        ErrorKind::NotFound,
        ErrorKind::PermissionDenied,
        ErrorKind::Other,
        ErrorKind::Interrupted,
    ] {
        assert_eq!(
            classify_io_error(&Error::new(kind, "synthetic")),
            SampleFailureKind::ReadFailed
        );
    }
    for code in [unrar::error::Code::BadData, unrar::error::Code::BadArchive] {
        assert_eq!(classify_unrar_error(code), SampleFailureKind::Data);
    }
    for code in [
        unrar::error::Code::EOpen,
        unrar::error::Code::ERead,
        unrar::error::Code::ECreate,
        unrar::error::Code::NoMemory,
    ] {
        assert_eq!(classify_unrar_error(code), SampleFailureKind::ReadFailed);
    }
    let cancel = AtomicBool::new(true);
    let path = fixture_root().join("jacket.rar");
    assert_eq!(
        sample_container_pages_with_rotations_cancellable(
            &path,
            "000.png",
            None,
            &path,
            |_| Rotation::None,
            Some(&cancel)
        ),
        Err(SampleAbort::Cancelled)
    );
    let missing = fixture_root().join("missing.rar");
    assert!(matches!(
        sample_container_pages_with_rotations_cancellable(
            &missing,
            "000.png",
            None,
            &missing,
            |_| Rotation::None,
            None
        ),
        Err(SampleAbort::ReadFailed(_))
    ));
    let dir = tempfile::tempdir().unwrap();
    let cover = dir.path().join("000.png");
    let body = dir.path().join("001.png");
    assert!(matches!(
        sample_folder_pages_with_rotations_cancellable(
            &[cover.clone(), body.clone()],
            &cover,
            |_| Rotation::None,
            None
        ),
        Err(SampleAbort::ReadFailed(_))
    ));
    std::fs::write(&body, b"broken image header").unwrap();
    assert_eq!(
        sample_folder_pages_with_rotations_cancellable(
            &[cover.clone(), body],
            &cover,
            |_| Rotation::None,
            None
        ),
        Ok(Err(PageAspectDecline::TooFew))
    );
    for kind in [
        ErrorKind::InvalidData,
        ErrorKind::TimedOut,
        ErrorKind::BrokenPipe,
    ] {
        assert!(matches!(
            page_sizes_read_result(Err(Error::new(kind, "worker"))),
            Err(SampleAbort::ReadFailed(_))
        ));
    }
    assert_eq!(
        page_sizes_read_result(Ok(vec![(70.0, 100.0)])),
        Ok(vec![(70.0, 100.0)])
    );
}

#[test]
fn rar_crc_failure_retains_earlier_reads_and_unknown_remaining_slots() {
    let path = fixture_root().join("crc-error.rar");
    let before = crate::rar_loader::sample_processing_scans_for_test();
    let (slots, metrics) = raw_samples(&path, "000.png", None, u64::MAX, u64::MAX);
    assert_eq!(
        crate::rar_loader::sample_processing_scans_for_test() - before,
        1
    );
    assert_eq!(slots.len(), 8);
    assert_eq!(metrics.read, 3);
    assert!(
        slots[..3]
            .iter()
            .all(|(_, read)| matches!(read, SampleRead::Read(_)))
    );
    assert!(
        slots[3..]
            .iter()
            .all(|(_, read)| matches!(read, SampleRead::Unreadable))
    );
    assert_eq!(sample_zip_pages(&path, "000.png", None), Ok(0.7));
}

#[test]
fn rar_header_failure_is_judged_data_and_keeps_earlier_pages() {
    // Locate the file header by the RAR5 metadata, rather than relying on a
    // timestamp-dependent offset in the regenerated fixture.
    fn vint(bytes: &[u8], cursor: &mut usize) -> usize {
        let mut value = 0usize;
        for shift in (0..63).step_by(7) {
            let byte = bytes[*cursor];
            *cursor += 1;
            value |= usize::from(byte & 0x7f) << shift;
            if byte & 0x80 == 0 {
                return value;
            }
        }
        panic!("invalid synthetic RAR vint")
    }
    let mut bytes = std::fs::read(fixture_root().join("jacket.rar")).unwrap();
    assert_eq!(&bytes[..8], b"Rar!\x1a\x07\x01\0");
    let mut block = 8;
    let mut broken = false;
    while block < bytes.len() {
        let mut cursor = block + 4; // header CRC
        let header_size = vint(&bytes, &mut cursor);
        let header_end = cursor + header_size;
        let kind = vint(&bytes, &mut cursor);
        let flags = vint(&bytes, &mut cursor);
        let extra_at = cursor;
        if flags & 1 != 0 {
            vint(&bytes, &mut cursor);
        }
        let data_size = if flags & 2 != 0 {
            vint(&bytes, &mut cursor)
        } else {
            0
        };
        if kind == 2
            && bytes[cursor..header_end]
                .windows(7)
                .any(|name| name == b"004.png")
        {
            assert_ne!(flags & 1, 0);
            assert!(header_size < 127 && bytes[extra_at] < 127);
            // Invalid extra area terminates UnRAR's next-header search. The
            // earlier entries remain real readable pages, not an OS failure.
            bytes[extra_at] = 127;
            broken = true;
            break;
        }
        block = header_end + data_size;
    }
    assert!(broken);
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().join("header-broken.rar");
    std::fs::write(&path, bytes).unwrap();
    let before = crate::rar_loader::sample_processing_scans_for_test();
    let (slots, metrics) = raw_samples(&path, "000.png", None, u64::MAX, u64::MAX);
    assert_eq!((metrics.selected, metrics.read), (3, 3));
    assert_eq!(
        crate::rar_loader::sample_processing_scans_for_test(),
        before + 1
    );
    assert_eq!(
        slots
            .iter()
            .map(|(name, _)| name.as_str())
            .collect::<Vec<_>>(),
        vec!["001.png", "002.png", "003.png"]
    );
    assert_eq!(
        sample_container_pages_with_rotations_cancellable(
            &path,
            "000.png",
            None,
            &path,
            |_| Rotation::None,
            None
        ),
        Ok(Ok(0.7))
    );
}

#[test]
#[ignore = "manual fixture regeneration requires WinRAR Rar.exe"]
fn generate_cover_auto_rar_fixtures() {
    use std::io::Cursor;
    let root = fixture_root();
    std::fs::create_dir_all(&root).unwrap();
    let inputs = tempfile::tempdir().unwrap();
    let mut cover = Cursor::new(Vec::new());
    fixture_jacket()
        .write_to(&mut cover, image::ImageFormat::Png)
        .unwrap();
    for (name, count, prefix) in [
        ("jacket.rar", 8, ""),
        ("cover-only.rar", 0, ""),
        ("two-pages.rar", 2, ""),
        ("zipdir.rar", 8, "book"),
    ] {
        let source = inputs.path().join(name);
        let pages = source.join(prefix);
        std::fs::create_dir_all(&pages).unwrap();
        std::fs::write(pages.join("000.png"), cover.get_ref()).unwrap();
        for index in 1..=count {
            std::fs::write(pages.join(format!("{index:03}.png")), png(70, 100)).unwrap();
        }
        let status = std::process::Command::new(r"C:\Program Files\WinRAR\Rar.exe")
            .current_dir(&source)
            .args(["a", "-s-", "-m0", "-ma5", "-idq", "-ep1", "-r"])
            .arg(root.join(name))
            .arg(if prefix.is_empty() { "*.png" } else { "book" })
            .status()
            .unwrap();
        assert!(status.success());
    }
    std::fs::write(
        root.join("duplicate-names.rar"),
        crate::rar_loader::direct_read_test_fixture_bytes(),
    )
    .unwrap();
    let mut corrupted = std::fs::read(root.join("jacket.rar")).unwrap();
    let offsets: Vec<_> = corrupted
        .windows(8)
        .enumerate()
        .filter_map(|(index, bytes)| (bytes == b"\x89PNG\r\n\x1a\n").then_some(index))
        .collect();
    assert_eq!(offsets.len(), 9);
    // Damage page 004's stored bytes, leaving every RAR header intact. The
    // processing read fails CRC; metadata enumeration still sees all pages.
    corrupted[offsets[4] + 20] ^= 1;
    std::fs::write(root.join("crc-error.rar"), corrupted).unwrap();
}

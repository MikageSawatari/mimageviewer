use super::tests::{oriented_image, png, zip};
use super::*;

#[test]
fn pdf_saved_body_rotation_can_decline_a_landscape_cover_as_not_wide() {
    let mut sizes = vec![(100.0, 85.0); 9];
    sizes[0] = (105.0, 100.0);
    let aspect =
        page_aspect_from_sizes_with_rotations_report(&sizes, 0, Path::new("book.pdf"), |_| {
            Rotation::Cw90
        })
        .unwrap();
    assert_eq!(aspect, 0.85);
    let image = image::DynamicImage::ImageRgb8(image::RgbImage::from_fn(105, 100, |x, y| {
        image::Rgb([(x * 2) as u8, (y * 2) as u8, ((x + y) % 200) as u8])
    }));
    let mut current = None;
    crate::cover_auto_crop::detect_cover_crops_with_photo_detailed_report(
        &image,
        (105, 100),
        Some(aspect),
        false,
        crate::settings::ReadingDirection::Rtl,
        |angle, report| {
            if angle == Rotation::None {
                current = Some(report);
            }
        },
    );
    assert_eq!(
        current.unwrap(),
        Err(crate::cover_auto_crop::AutoCropDecline::NotWide)
    );
}

#[test]
fn pdf_saved_rotations_use_zero_based_viewer_keys_and_keep_sample_budget() {
    let logical = Path::new("G:/Synthetic/Book.PDF");
    let sizes = [(100.0, 70.0); 12];
    for rotation in [Rotation::Cw90, Rotation::Cw270] {
        for leaf in [0, 11] {
            let mut keys = Vec::new();
            let result =
                page_aspect_from_sizes_with_rotations_report(&sizes, leaf, logical, |key| {
                    keys.push(key.to_owned());
                    rotation
                });
            assert_eq!(result, Ok(0.7));
            let indices = if leaf == 0 {
                (1..9).collect::<Vec<_>>()
            } else {
                (3..11).rev().collect::<Vec<_>>()
            };
            assert_eq!(
                keys,
                indices
                    .into_iter()
                    .map(|index| crate::edit_source::page_key_for_pdf(logical, index))
                    .collect::<Vec<_>>()
            );
        }
    }
    for rotation in [Rotation::None, Rotation::Cw180] {
        assert_eq!(
            page_aspect_from_sizes_with_rotations_report(&sizes, 0, logical, |_| rotation),
            page_aspect_from_sizes_report(&sizes, 0)
        );
    }
}

#[test]
fn pdf_leading_landscape_trim_uses_saved_orientation_and_keeps_unknown_slots() {
    let logical = Path::new("synthetic.pdf");
    // The first three sampled pages become portrait after saved rotation. They
    // must count toward the portrait majority rather than being trimmed first.
    let sizes = [
        (200.0, 100.0),
        (100.0, 70.0),
        (100.0, 70.0),
        (100.0, 70.0),
        (70.0, 100.0),
        (70.0, 100.0),
        (70.0, 100.0),
        (140.0, 100.0),
        (140.0, 100.0),
    ];
    assert_eq!(
        page_aspect_from_sizes_report(&sizes, 0),
        Err(PageAspectDecline::Landscape)
    );
    assert_eq!(
        page_aspect_from_sizes_with_rotations_report(&sizes, 0, logical, |key| {
            if (1..=3).any(|index| key == crate::edit_source::page_key_for_pdf(logical, index)) {
                Rotation::Cw90
            } else {
                Rotation::None
            }
        }),
        Ok(0.7)
    );
    let mut unknown = sizes;
    unknown[2] = (f32::NAN, 100.0);
    let mut read_keys = Vec::new();
    let _ = page_aspect_from_sizes_with_rotations_report(&unknown, 0, logical, |key| {
        read_keys.push(key.to_owned());
        Rotation::Cw270
    });
    assert_eq!(read_keys.len(), MAX_SAMPLE_PAGES - 1);
    assert!(!read_keys.contains(&crate::edit_source::page_key_for_pdf(logical, 2)));
}

#[test]
fn zip_saved_rotations_use_logical_archive_keys_and_bounded_prefixes() {
    let mut bytes = png(100, 70);
    bytes.resize(300 * 1024, 0);
    let archive = zip((0..12)
        .map(|index| (format!("Chapter/{index:02}.png"), bytes.clone()))
        .collect());
    let logical = Path::new("G:/Synthetic/Original.CBZ");
    for rotation in [Rotation::Cw90, Rotation::Cw270] {
        let mut keys = Vec::new();
        assert_eq!(
            sample_zip_pages_with_rotations(
                archive.path(),
                "Chapter/00.png",
                Some("Chapter/"),
                logical,
                |key| {
                    keys.push(key.to_owned());
                    rotation
                },
            ),
            Ok(0.7)
        );
        assert_eq!(
            keys,
            (1..=8)
                .map(|index| crate::adjustment_db::zip_entry_key(
                    logical,
                    &format!("Chapter/{index:02}.png")
                ))
                .collect::<Vec<_>>()
        );
    }
    for rotation in [Rotation::None, Rotation::Cw180] {
        assert_eq!(
            sample_zip_pages_with_rotations(
                archive.path(),
                "Chapter/00.png",
                Some("Chapter/"),
                logical,
                |_| rotation,
            ),
            sample_zip_pages(archive.path(), "Chapter/00.png", Some("Chapter/"))
        );
    }
    let prefixes = crate::zip_loader::read_cover_auto_page_prefixes(
        archive.path(),
        "Chapter/00.png",
        Some("Chapter/"),
    )
    .unwrap();
    assert_eq!(prefixes.len(), MAX_SAMPLE_PAGES);
    assert!(
        prefixes.iter().all(|(_, bytes)| {
            bytes.as_ref().unwrap().len() == MAX_SAMPLE_PREFIX_BYTES as usize
        })
    );
}

#[test]
fn folder_saved_rotations_use_viewer_keys_after_existing_orientation() {
    let dir = tempfile::tempdir().unwrap();
    let bytes = png(100, 70);
    let paths: Vec<_> = (0..12)
        .map(|index| {
            let path = dir.path().join(format!("Page{index:02}.png"));
            std::fs::write(&path, &bytes).unwrap();
            path
        })
        .collect();
    for rotation in [Rotation::Cw90, Rotation::Cw270] {
        let mut keys = Vec::new();
        assert_eq!(
            sample_folder_pages_with_rotations(&paths, &paths[0], |key| {
                keys.push(key.to_owned());
                rotation
            }),
            Ok(0.7)
        );
        assert_eq!(
            keys,
            paths[1..=8]
                .iter()
                .map(|path| crate::adjustment_db::normalize_path(path))
                .collect::<Vec<_>>()
        );
    }
    for rotation in [Rotation::None, Rotation::Cw180] {
        assert_eq!(
            sample_folder_pages_with_rotations(&paths, &paths[0], |_| rotation),
            sample_folder_pages(&paths, &paths[0])
        );
    }
}

#[test]
fn saved_quarter_turn_applies_once_after_exif_or_file_wic_but_zip_stays_bytes_only() {
    let dir = tempfile::tempdir().unwrap();
    for extension in ["jpg", "webp"] {
        let bytes = oriented_image(extension, 100, 70, 6);
        let paths: Vec<_> = (0..9)
            .map(|index| {
                let path = dir.path().join(format!("{index:02}.{extension}"));
                std::fs::write(&path, &bytes).unwrap();
                path
            })
            .collect();
        let archive = zip((0..9)
            .map(|index| (format!("{index:02}.{extension}"), bytes.clone()))
            .collect());
        for rotation in [Rotation::Cw90, Rotation::Cw270] {
            let mut wic_calls = 0;
            assert_eq!(
                sample_folder_pages_with_rotations_and_wic(
                    &paths,
                    &paths[0],
                    |_| rotation,
                    |_| {
                        wic_calls += 1;
                        6
                    },
                ),
                Err(PageAspectDecline::Landscape)
            );
            assert_eq!(wic_calls, if extension == "jpg" { 0 } else { 8 });
            assert_eq!(
                sample_zip_pages_with_rotations(
                    archive.path(),
                    &format!("00.{extension}"),
                    None,
                    Path::new("synthetic.zip"),
                    |_| rotation,
                ),
                if extension == "jpg" {
                    Err(PageAspectDecline::Landscape)
                } else {
                    Ok(0.7)
                }
            );
        }
    }
}

#[test]
fn worker_rotation_reader_is_readonly_and_missing_or_bad_db_falls_back() {
    let dir = tempfile::tempdir().unwrap();
    let absent = dir.path().join("absent").join("rotation.db");
    assert_eq!(
        SampleRotations::from_path_worker(None).get("key"),
        Rotation::None
    );
    assert_eq!(
        SampleRotations::from_path_worker(Some(&absent)).get("key"),
        Rotation::None
    );
    assert!(!absent.parent().unwrap().exists());

    let db_path = dir.path().join("rotation.db");
    {
        let db = RotationDb::open_at(&db_path).unwrap();
        for (key, rotation) in [
            ("zero", Rotation::None),
            ("quarter", Rotation::Cw90),
            ("half", Rotation::Cw180),
            ("reverse", Rotation::Cw270),
        ] {
            db.set_key(key, rotation).unwrap();
        }
    }
    let original = std::fs::read(&db_path).unwrap();
    {
        let reader = SampleRotations::from_path_worker(Some(&db_path));
        for (key, expected) in [
            ("missing", Rotation::None),
            ("zero", Rotation::None),
            ("quarter", Rotation::Cw90),
            ("half", Rotation::Cw180),
            ("reverse", Rotation::Cw270),
        ] {
            assert_eq!(reader.get(key), expected);
        }
        assert!(
            reader
                .0
                .as_ref()
                .unwrap()
                .set_key("quarter", Rotation::Cw270)
                .is_err()
        );
    }
    assert_eq!(std::fs::read(&db_path).unwrap(), original);
    assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);

    let bad_schema = dir.path().join("no-table.db");
    drop(rusqlite::Connection::open(&bad_schema).unwrap());
    assert_eq!(
        SampleRotations::from_path_worker(Some(&bad_schema)).get("quarter"),
        Rotation::None
    );
    assert_eq!(std::fs::metadata(&bad_schema).unwrap().len(), 0);
    let corrupt = dir.path().join("invalid.db");
    std::fs::write(&corrupt, b"not a sqlite database").unwrap();
    assert_eq!(
        SampleRotations::from_path_worker(Some(&corrupt)).get("quarter"),
        Rotation::None
    );
    assert_eq!(std::fs::read(&corrupt).unwrap(), b"not a sqlite database");
    assert_eq!(
        SampleRotations::from_path_worker(Some(dir.path())).get("quarter"),
        Rotation::None
    );
}

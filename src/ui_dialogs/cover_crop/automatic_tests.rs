use super::*;
use std::io::{Cursor, Write};
use std::sync::{Mutex, RwLock, atomic::AtomicUsize};

#[test]
fn cover_crop_auto_method_layout_binding_messages_keep_one_typed_result() {
    use crate::cover_auto_crop::{
        AutoCropDecline, AutoCropLayout, AutoCropMethod, AutoCropSuccess,
    };
    let crop = CoverCrop::new([0, 0, 500_000, 1_000_000]).unwrap().unwrap();
    for (binding, binding_label) in [
        (crate::settings::ReadingDirection::Rtl, "右綴じ"),
        (crate::settings::ReadingDirection::Ltr, "左綴じ"),
    ] {
        let mut cases = vec![(
            AutoCropMethod::Current,
            format!("自動の判定: 表表紙を切り出しました (綴じ方向: {binding_label})"),
        )];
        for (layout, layout_label) in [
            (AutoCropLayout::T1, "T1"),
            (AutoCropLayout::T2, "T2"),
            (AutoCropLayout::T3, "T3"),
            (AutoCropLayout::T4, "T4"),
        ] {
            for (method, action) in [
                (
                    AutoCropMethod::LayoutFallback(layout),
                    "寸法の型から表表紙を切り出しました",
                ),
                (
                    AutoCropMethod::LayoutReplaced(layout),
                    "寸法の型に合わせて切り出す位置を変更しました",
                ),
            ] {
                cases.push((
                    method,
                    format!("自動の判定: {action} (型: {layout_label}、綴じ方向: {binding_label})"),
                ));
            }
        }
        assert_eq!(cases.len(), 9);
        for (method, expected_message) in cases {
            let success = AutoCropSuccess::for_test(crop, method);
            for angle in [
                Rotation::None,
                Rotation::Cw90,
                Rotation::Cw180,
                Rotation::Cw270,
            ] {
                let mut reports = [Err(AutoCropDecline::RawAspectGate); 4];
                reports[angle.degrees() as usize / 90] = Ok(success);
                let detection = AutoDetection::Analyzed { binding, reports };
                assert_eq!(detection.message(angle), expected_message);
                assert_eq!(detection.crop_for(angle), Some(crop));
                for other in [
                    Rotation::None,
                    Rotation::Cw90,
                    Rotation::Cw180,
                    Rotation::Cw270,
                ] {
                    let expected = (other == angle).then_some(crop);
                    assert_eq!(detection.crop_for(other), expected);
                    assert_eq!(detection.crops().unwrap().crop_for(other), expected);
                    if other != angle {
                        assert_eq!(
                            detection.message(other),
                            "自動の判定: 切り出していません。表紙がつながった形 (横長) ではありません"
                        );
                    }
                }
            }
        }
    }
}

#[test]
fn cover_crop_auto_from_crops_preserves_each_angle() {
    let first = CoverCrop::new([0, 0, 400_000, 1_000_000]).unwrap().unwrap();
    let last = CoverCrop::new([600_000, 0, 1_000_000, 1_000_000])
        .unwrap()
        .unwrap();
    let crops =
        crate::cover_auto_material::AutoCropResults::new([Some(first), None, Some(last), None]);
    let detection = AutoDetection::from_crops(crops);
    assert_eq!(detection.crops(), Some(crops));
    assert_eq!(detection.crop_for(Rotation::None), Some(first));
    assert_eq!(detection.crop_for(Rotation::Cw90), None);
    assert_eq!(detection.crop_for(Rotation::Cw180), Some(last));
    assert_eq!(detection.crop_for(Rotation::Cw270), None);
}

#[test]
fn cover_crop_worker_and_dialog_match_all_four_angles_for_png_jpeg_and_binding() {
    for photo in [false, true] {
        for folder in [false, true] {
            for jpeg in [false, true] {
                for binding in [
                    crate::settings::ReadingDirection::Rtl,
                    crate::settings::ReadingDirection::Ltr,
                ] {
                    let temp = tempfile::tempdir().unwrap();
                    let zip_path = temp.path().join("book.zip");
                    let image = image::RgbImage::from_fn(892, 512, |x, y| {
                        image::Rgb(if x < 140 {
                            if y % 73 < 8 { [65; 3] } else { [246, 242, 220] }
                        } else if x < 498 {
                            [
                                35 + (y % 80) as u8,
                                55 + (y % 155) as u8,
                                200 - (y % 80) as u8,
                            ]
                        } else if x < 534 {
                            [18, 28, 38]
                        } else if y % 73 < 8 {
                            [75; 3]
                        } else {
                            [246, 242, 220]
                        })
                    });
                    let image = if binding == crate::settings::ReadingDirection::Ltr {
                        image::imageops::flip_horizontal(&image)
                    } else {
                        image
                    };
                    let mut image = image::DynamicImage::ImageRgb8(image);
                    if jpeg {
                        image =
                            image.resize_exact(2676, 1536, image::imageops::FilterType::Nearest);
                    }
                    let leaf = if jpeg { "000.jpg" } else { "000.png" };
                    let mut cover = Cursor::new(Vec::new());
                    image
                        .write_to(
                            &mut cover,
                            if jpeg {
                                image::ImageFormat::Jpeg
                            } else {
                                image::ImageFormat::Png
                            },
                        )
                        .unwrap();
                    let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
                    zip.start_file(leaf, zip::write::SimpleFileOptions::default())
                        .unwrap();
                    zip.write_all(cover.get_ref()).unwrap();
                    // Also exercise the same new sampling rule on both real paths.
                    for i in 1..=8 {
                        let mut bytes = Cursor::new(Vec::new());
                        image::DynamicImage::new_rgb8(
                            if i <= 3 {
                                140
                            } else if photo {
                                image.height()
                            } else {
                                70
                            },
                            if i > 3 && photo { image.width() } else { 100 },
                        )
                        .write_to(&mut bytes, image::ImageFormat::Png)
                        .unwrap();
                        zip.start_file(
                            format!("{i:03}.png"),
                            zip::write::SimpleFileOptions::default(),
                        )
                        .unwrap();
                        zip.write_all(bytes.get_ref()).unwrap();
                    }
                    zip.finish().unwrap();
                    let folder_path = temp.path().join("folder-book");
                    if folder {
                        std::fs::create_dir(&folder_path).unwrap();
                        std::fs::write(folder_path.join(leaf), cover.get_ref()).unwrap();
                        for i in 1..=8 {
                            image::DynamicImage::new_rgb8(
                                if i <= 3 {
                                    140
                                } else if photo {
                                    image.height()
                                } else {
                                    70
                                },
                                if i > 3 && photo { image.width() } else { 100 },
                            )
                            .save(folder_path.join(format!("{i:03}.png")))
                            .unwrap();
                        }
                    }
                    let book_path = if folder { folder_path } else { zip_path };
                    let settings = crate::settings::Settings {
                        thumb_cover_auto_crop: true,
                        thumb_cover_auto_crop_folders: folder,
                        thumb_cover_auto_crop_binding: binding,
                        cache_policy: crate::settings::CachePolicy::Off,
                        ..Default::default()
                    };
                    let env = BookCoverEnv {
                        books_root: Some(temp.path().to_owned()),
                        auto_cover_crop: crate::app::configured_thumbnail_auto_crop(&settings),
                        ..Default::default()
                    };
                    let dialog = prepare_cover(
                        &BookCoverContainer {
                            path: book_path.clone(),
                            kind: if folder {
                                BookKind::Folder
                            } else {
                                BookKind::Zip
                            },
                            zip_dir_prefix: None,
                        },
                        None,
                        &env,
                        73,
                        &Arc::new(AtomicBool::new(false)),
                    )
                    .unwrap();
                    let expected = dialog
                        .auto_detection
                        .crops()
                        .expect("dialog analyzed four directions");
                    if !photo {
                        assert!(expected.crops().iter().any(Option::is_some));
                    }
                    let AutoDetection::Analyzed { reports, .. } = dialog.auto_detection else {
                        unreachable!()
                    };
                    assert!(reports.iter().all(|result| !matches!(
                        result,
                        Err(crate::cover_auto_crop::AutoCropDecline::PhotoLike { .. })
                    )));
                    let mut req = crate::thumb_loader::LoadRequest {
                        path: book_path,
                        zip_entry: (!folder).then(|| leaf.into()),
                        cache_key_override: Some(
                            if folder {
                                "folderthumb:folder-book"
                            } else {
                                "zipthumb:book.zip"
                            }
                            .into(),
                        ),
                        source_policy: crate::thumb_loader::LoadSourcePolicy::SourceOnly,
                        ..Default::default()
                    };
                    req.set_auto_cover_crop(env.auto_cover_crop);
                    let (tx, rx) = mpsc::channel();
                    let done = Arc::new(AtomicUsize::new(0));
                    let catalog = Arc::new(
                        crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path())
                            .unwrap(),
                    );
                    crate::thumb_loader::process_load_request(
                        &mut req,
                        &RwLock::new(Default::default()),
                        &tx,
                        Some(&catalog),
                        64,
                        75,
                        64,
                        crate::thumb_loader::CacheDecision::from_settings(&settings),
                        &done,
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
                    assert_eq!(done.load(Ordering::Relaxed), 1);
                    let messages: Vec<_> = rx.try_iter().collect();
                    let pixels = messages
                        .iter()
                        .find_map(crate::thumb_loader::ThumbMsg::pixels)
                        .expect("worker pixels");
                    assert_eq!(
                        pixels.auto_material,
                        crate::cover_auto_material::AutoMaterial::Automatic(expected),
                        "{jpeg}/{binding:?}"
                    );
                }
            }
        }
    }
}

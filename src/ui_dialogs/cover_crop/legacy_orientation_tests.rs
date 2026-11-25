//! Real decoders, dialog acceptance, pin persistence, and thumbnail workers.
use super::*;
use crate::cover_auto_crop::AutoCropDecline;
use crate::cover_auto_material::AutoMaterial;
use crate::thumb_loader::{LoadRequest, LoadSourcePolicy, ThumbMsg};
use egui_kittest::kittest::Queryable;
use image::{DynamicImage, ImageEncoder, Rgb, RgbImage};
use std::io::Write;

fn jacket() -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(892, 512, |x, y| {
        Rgb(if x < 140 {
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
    }))
}

fn rotate(image: &DynamicImage, angle: Rotation) -> DynamicImage {
    match angle {
        Rotation::None => image.clone(),
        Rotation::Cw90 => image.rotate90(),
        Rotation::Cw180 => image.rotate180(),
        Rotation::Cw270 => image.rotate270(),
    }
}

fn png(image: &DynamicImage, orientation: Option<u16>) -> Vec<u8> {
    let rgb = image.to_rgb8();
    let mut bytes = Vec::new();
    let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
    if let Some(orientation) = orientation {
        // IFD0 Orientation only: no photographic tags, so G1 cannot hide
        // a coordinate-system error by declining the synthetic jacket.
        let mut exif = b"II*\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0".to_vec();
        exif.extend(orientation.to_le_bytes());
        exif.extend([0; 6]);
        encoder.set_exif_metadata(exif).unwrap();
    }
    encoder
        .write_image(
            rgb.as_raw(),
            rgb.width(),
            rgb.height(),
            image::ExtendedColorType::Rgb8,
        )
        .unwrap();
    bytes
}

fn write_book(path: &Path, folder: bool, cover: &[u8], body: &[u8]) {
    if folder {
        std::fs::create_dir(path).unwrap();
        std::fs::write(path.join("000.png"), cover).unwrap();
        for i in 1..=8 {
            std::fs::write(path.join(format!("{i:03}.png")), body).unwrap();
        }
    } else {
        let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
        for i in 0..=8 {
            zip.start_file(
                format!("{i:03}.png"),
                zip::write::SimpleFileOptions::default(),
            )
            .unwrap();
            zip.write_all(if i == 0 { cover } else { body }).unwrap();
        }
        zip.finish().unwrap();
    }
}

fn request(
    path: &Path,
    folder: bool,
    auto: Option<crate::cover_auto_crop::AutoCropIdentity>,
) -> LoadRequest {
    let mut request = LoadRequest {
        path: path.to_owned(),
        zip_entry: (!folder).then(|| "000.png".into()),
        cache_key_override: Some(
            if folder {
                "folderthumb:book"
            } else {
                "zipthumb:book.zip"
            }
            .into(),
        ),
        source_policy: LoadSourcePolicy::SourceOnly,
        force_cache: true,
        ..Default::default()
    };
    request.set_auto_cover_crop(auto);
    request
}

fn prepare(container: &BookCoverContainer, env: &BookCoverEnv) -> PreparedCover {
    prepare_cover(
        container,
        None,
        env,
        2048,
        &Arc::new(AtomicBool::new(false)),
    )
    .unwrap()
}

#[test]
fn cover_crop_png_orientation6_auto_accept_reload_keeps_legacy_manual_pixels_in_zip_and_folder() {
    for folder in [false, true] {
        let mut app = crate::app::setup_app_for_test();
        let root = app.tmp.path().to_owned();
        let book = root.join(if folder { "book" } else { "book.zip" });
        let image = jacket();
        let cover = png(&image, Some(6));
        let body = png(&DynamicImage::new_rgb8(70, 100), Some(6));
        assert_eq!(
            crate::thumb_loader::read_exif_orientation_from_bytes(&cover),
            1
        );
        assert_eq!(
            crate::app::probe_image_dims_from_bytes(&body, "body.png"),
            Some((70, 100))
        );
        write_book(&book, folder, &cover, &body);
        app.settings.thumb_cover_auto_crop = true;
        app.settings.thumb_cover_auto_crop_folders = folder;
        app.settings.book_root = Some(root.clone());
        app.current_folder = Some(root.clone());
        app.items = vec![if folder {
            GridItem::Folder(book.clone())
        } else {
            GridItem::ZipFile(book.clone())
        }];
        app.thumbnails = vec![crate::grid_item::ThumbnailState::Pending];
        app.image_metas = vec![None];
        let pins = Arc::new(
            crate::folder_thumb_pins::FolderThumbPinDb::open_at(&root.join("pins.db")).unwrap(),
        );
        app.folder_thumb_pin_db = Some(pins.clone());
        let container = BookCoverContainer {
            path: book.clone(),
            kind: if folder {
                BookKind::Folder
            } else {
                BookKind::Zip
            },
            zip_dir_prefix: None,
        };
        let env = app.book_cover_env();
        let prepared = prepare(&container, &env);
        let full_image = egui::ColorImage::from_rgb([892, 512], image.to_rgb8().as_raw());
        assert_eq!(
            prepared.image, full_image,
            "automatic preview uses the released PNG orientation"
        );
        let crop = prepared
            .auto_detection
            .crop_for(Rotation::None)
            .expect("positive jacket under legacy orientation");
        let sample = if folder {
            let pages: Vec<_> = (0..=8).map(|i| book.join(format!("{i:03}.png"))).collect();
            crate::cover_auto_sample::sample_folder_pages(&pages, &pages[0])
        } else {
            crate::cover_auto_sample::sample_zip_pages(&book, "000.png", None)
        };
        assert_eq!(sample, Ok(0.7));
        let catalog =
            Arc::new(crate::catalog::CatalogDb::open(&root.join("cache"), &root).unwrap());
        let mut auto_request =
            request(&book, folder, env.cover_auto_crop_scope.identity_for(&book));
        let auto = super::rotation_tests::worker(&mut auto_request, &catalog, &pins);
        assert_eq!(
            auto.iter()
                .find_map(ThumbMsg::pixels)
                .unwrap()
                .auto_material,
            AutoMaterial::Automatic(prepared.auto_detection.crops().unwrap())
        );

        // Accept the actual automatic draft with the ordinary OK handler.
        let (tx, rx) = mpsc::channel();
        app.cover_crop_dialog = Some(CoverCropDialog {
            generation: 1,
            container: book.clone(),
            cancel: Arc::new(AtomicBool::new(false)),
            phase: Phase::Loading(rx),
            placement: WindowPlacement::Initial,
        });
        let mut harness = super::tests::app_harness(app);
        tx.send((1, Ok(prepared))).unwrap();
        harness.run_steps(3);
        harness.get_by_label(OK_LABEL).click();
        harness.run_steps(2);
        assert!(!harness.state().cover_crop_dialog_open());
        let saved = pins.lookup(&book).unwrap();
        assert_eq!(saved.cover_crop, Some(crop));
        let reloaded = prepare(&container, &harness.state().book_cover_env());
        assert_eq!(reloaded.crop, Some(crop));
        assert_eq!(
            reloaded.image, full_image,
            "manual preview keeps the automatic preview coordinates"
        );
        let mut manual = harness.state().cover_crop_tile_request_for_test(0);
        manual.source_policy = LoadSourcePolicy::SourceOnly;
        manual.force_cache = true;
        assert_eq!(manual.cover_crop, Some(crop));
        let manual_key = crate::thumb_loader::cache_key_for_request(&manual)
            .unwrap()
            .into_owned();
        let messages = super::rotation_tests::worker(&mut manual, &catalog, &pins);
        let pixels = messages.iter().find_map(ThumbMsg::pixels).unwrap();
        let (x, y, width, height) = crop.pixel_bounds(image.width(), image.height());
        let expected = image.crop_imm(x, y, width, height);
        assert_eq!(pixels.source_dims, Some((width, height)));
        assert_eq!(
            pixels.image,
            crate::thumb_loader::resize_to_display_color_image(&expected, 64, None)
        );
        assert_eq!(pixels.auto_material, AutoMaterial::NotAutomatic);
        let stored = catalog.load_one(&manual_key).unwrap().unwrap();
        assert_eq!(stored.source_dims, Some((width, height)));
        assert_eq!(stored.auto_material, AutoMaterial::NotAutomatic);
        assert_eq!(stored.cover, pixels.cover);
    }
}

#[test]
fn cover_crop_worker_and_dialog_continue_for_transposed_page_dimensions_at_every_rotation() {
    let upright = jacket().resize_exact(810, 512, image::imageops::FilterType::Nearest);
    let aspect = 512.0 / 810.0;
    assert!(
        crate::cover_auto_crop::detect_cover_crops(
            &upright,
            (810, 512),
            Some(aspect),
            crate::settings::ReadingDirection::Rtl
        )
        .crop_for(Rotation::None)
        .is_some(),
        "fixture must crop without shooting metadata"
    );
    for folder in [false, true] {
        for physical in [
            Rotation::None,
            Rotation::Cw90,
            Rotation::Cw180,
            Rotation::Cw270,
        ] {
            let app = crate::app::setup_app_for_test();
            let root = app.tmp.path();
            let book = root.join(if folder { "book" } else { "book.zip" });
            let image = rotate(&upright, physical);
            write_book(
                &book,
                folder,
                &png(&image, None),
                &png(&DynamicImage::new_rgb8(512, 810), None),
            );
            let settings = crate::settings::Settings {
                thumb_cover_auto_crop: true,
                thumb_cover_auto_crop_folders: folder,
                book_root: Some(root.to_owned()),
                ..Default::default()
            };
            let env = BookCoverEnv {
                books_root: Some(root.to_owned()),
                cover_auto_crop_scope: CoverAutoCropScope::from_settings(&settings),
                ..Default::default()
            };
            let container = BookCoverContainer {
                path: book.clone(),
                kind: if folder {
                    BookKind::Folder
                } else {
                    BookKind::Zip
                },
                zip_dir_prefix: None,
            };
            let prepared = prepare(&container, &env);
            let AutoDetection::Analyzed { reports, .. } = prepared.auto_detection else {
                panic!("analyzed")
            };
            for (index, saved) in [
                Rotation::None,
                Rotation::Cw90,
                Rotation::Cw180,
                Rotation::Cw270,
            ]
            .into_iter()
            .enumerate()
            {
                if (physical.degrees() + saved.degrees()) % 180 == 0 {
                    assert!(
                        !matches!(reports[index], Err(AutoCropDecline::PhotoLike { .. })),
                        "same-sized scan must continue: folder={folder}/{physical:?}/{saved:?}: {:?}",
                        reports[index]
                    );
                } else {
                    assert_eq!(reports[index], Err(AutoCropDecline::RawAspectGate));
                }
            }
            assert!(
                prepared
                    .auto_detection
                    .crop_for(Rotation::from_degrees(-physical.degrees()))
                    .is_some(),
                "corrective saved rotation must crop despite matching page dimensions"
            );
            let pins =
                crate::folder_thumb_pins::FolderThumbPinDb::open_at(&root.join("pins.db")).unwrap();
            let catalog =
                Arc::new(crate::catalog::CatalogDb::open(&root.join("cache"), root).unwrap());
            let mut req = request(&book, folder, env.cover_auto_crop_scope.identity_for(&book));
            let messages = super::rotation_tests::worker(&mut req, &catalog, &pins);
            let pixels = messages.iter().find_map(ThumbMsg::pixels).unwrap();
            assert_eq!(
                pixels.auto_material,
                AutoMaterial::Automatic(prepared.auto_detection.crops().unwrap())
            );
            assert!(
                prepared
                    .auto_detection
                    .crops()
                    .unwrap()
                    .crops()
                    .iter()
                    .any(Option::is_some)
            );
        }
    }
}

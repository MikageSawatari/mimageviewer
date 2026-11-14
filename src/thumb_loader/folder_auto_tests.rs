// These tests use only synthetic images and temporary directories.
fn folder_auto_fixture(folder: &Path) {
    std::fs::create_dir_all(folder).unwrap();
    auto_cover_jacket().save(folder.join("000.png")).unwrap();
    for index in 1..=8 {
        image::DynamicImage::new_rgb8(70, 100)
            .save(folder.join(format!("{index:03}.png")))
            .unwrap();
    }
}

#[test]
fn cover_auto_folder_direct_selection_reuses_listing_and_excludes_descendants_and_seeds() {
    let temp = TempDir::new().unwrap();
    let folder = temp.path().join("book");
    folder_auto_fixture(&folder);
    let identity = crate::cover_auto_crop::AutoCropIdentity {
        binding: crate::settings::ReadingDirection::Rtl,
        folders: true,
    };
    let mut req = LoadRequest {
        path: folder.clone(),
        cache_key_override: Some("folderthumb:book".into()),
        source_policy: LoadSourcePolicy::SourceOnly,
        folder_thumb_depth: 3,
        ..Default::default()
    };
    req.set_auto_cover_crop(Some(identity));
    assert_eq!(
        req.cache_key_override.as_deref(),
        Some("folderthumb:book|autocrop:v2f:rtl")
    );
    let resolved = resolve_read_after_cache_miss(&req, None, None);
    let auto = resolved
        .auto_cover_crop
        .expect("direct image has folder sample");
    let AutoCoverInput::FolderPages(pages) = auto.input else {
        panic!("folder list")
    };
    assert_eq!(pages.len(), 9);
    assert_eq!(pages[0], folder.join("000.png"));
    assert_eq!(
        crate::cover_auto_sample::sample_folder_pages(&pages, &pages[0]),
        Ok(0.7)
    );
    let cache =
        Arc::new(crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path()).unwrap());
    let messages = run_cover_crop_request(&mut req, &cache, None);
    let pixels = messages.iter().find_map(ThumbMsg::pixels).unwrap();
    assert!(
        matches!(pixels.auto_material, crate::cover_auto_material::AutoMaterial::Automatic(results) if results.crops().iter().any(Option::is_some))
    );
    let row = cache
        .load_one(req.cache_key_override.as_deref().unwrap())
        .unwrap()
        .unwrap();
    assert_eq!(row.auto_material, pixels.auto_material);
    req.folder_thumb_provenance = Some(crate::catalog::FolderThumbProvenance::Seeded);
    assert!(
        resolve_read_after_cache_miss(&req, None, None)
            .auto_cover_crop
            .is_err()
    );
    req.folder_thumb_provenance = None;
    req.path = temp.path().to_owned();
    req.cache_key_override = Some("folderthumb:parent|autocrop:v2f:rtl".into());
    assert!(
        resolve_read_after_cache_miss(&req, None, None)
            .auto_cover_crop
            .is_err(),
        "parent must not analyze a descendant"
    );
    req.path = folder;
    req.set_auto_cover_crop(Some(crate::cover_auto_crop::AutoCropIdentity::new(
        identity.binding,
    )));
    assert!(
        resolve_read_after_cache_miss(&req, None, None)
            .auto_cover_crop
            .is_err(),
        "folder switch OFF"
    );
}

#[test]
fn cover_auto_folder_v2f_identity_roundtrips_and_never_uses_v1_migration() {
    let visible = Default::default();
    for literal in [
        "zipdir:book|autocrop:v2f:rtl",
        "zipdir:book|autocrop:v2f:ltr#pin:001",
        "zipthumb:book|autocrop:v2f:rtl",
    ] {
        assert_eq!(
            representative_cache_key_for_policy(literal, &visible),
            literal
        );
        assert!(representative_auto_crop_key_parts(literal).is_none());
    }
    for binding in [
        crate::settings::ReadingDirection::Rtl,
        crate::settings::ReadingDirection::Ltr,
    ] {
        let auto = crate::cover_auto_crop::AutoCropIdentity {
            binding,
            folders: true,
        };
        for key in [
            "folderthumb:book",
            "folderthumb:book#pin:child",
            "zipthumb:book.zip",
            "pdfthumb:book.pdf",
            "zipdir:book/",
        ] {
            let enabled = representative_cache_key_for_policy_with_auto(key, &visible, Some(auto));
            assert_eq!(
                enabled.contains("|autocrop:v2f:"),
                key.starts_with(CACHE_KEY_FOLDER)
            );
            assert_eq!(
                representative_cache_key_for_policy_with_auto(&enabled, &visible, Some(auto)),
                enabled
            );
            assert_eq!(representative_cache_key_for_policy(&enabled, &visible), key);
            assert_eq!(
                legacy_auto_child_keys(&enabled).is_none(),
                key.starts_with(CACHE_KEY_FOLDER)
            );
        }
    }
}

#[test]
fn cover_auto_pdf_transposed_page_dimensions_continue_detection_with_same_lease() {
    let temp = TempDir::new().unwrap();
    let path = temp.path().join("book.epub");
    let read = Some(crate::pdf_loader::generation_target_for_test(
        &path, 17, 4096,
    ));
    let observed = Arc::new(Mutex::new(Vec::new()));
    let jacket = auto_cover_jacket().resize_exact(810, 512, image::imageops::FilterType::Nearest);
    let mut first = cover_only_pdf_result();
    first.image = jacket.resize_exact(224, 128, image::imageops::FilterType::CatmullRom);
    first.page_size_points = crate::pdf_loader::PdfPageSizePoints {
        width: 810.0,
        height: 512.0,
    };
    let mut second = cover_only_pdf_result();
    second.image = jacket.resize_exact(1620, 1024, image::imageops::FilterType::CatmullRom);
    second.page_size_points = first.page_size_points;
    let mut sizes = vec![(512.0, 810.0); 9];
    sizes[0] = (810.0, 512.0);
    AUTO_CROP_PDF_FIXTURE.with(|current| {
        *current.borrow_mut() = Some(AutoCropPdfFixture {
            sizes,
            renders: [first, second].into_iter().collect(),
            observed: Arc::clone(&observed),
        })
    });
    let (tx, rx) = mpsc::channel();
    let done = Arc::new(AtomicUsize::new(0));
    let identity =
        crate::cover_auto_crop::AutoCropIdentity::new(crate::settings::ReadingDirection::Rtl);
    load_one_cached(
        &path,
        None,
        None,
        None,
        Some(0),
        read.as_ref(),
        Some("session password"),
        None,
        0,
        &tx,
        None.into(),
        None,
        7,
        9,
        &done,
        64,
        75,
        256,
        make_decision(CachePolicy::Off, 0, 0),
        &Arc::new(Mutex::new(crate::stats::ThumbStats::default())),
        None,
        true,
        1,
        1,
        ThumbnailRequestId::Registered(109),
        crate::pdf_loader::PdfRenderValidity::Unscoped,
        crate::pdf_loader::CancelWaitPolicy::AbortOnCancel,
        false,
        false,
        None,
        None,
        None,
        LoadRequestSource::Original,
        None,
        None,
        CoverIdentity::Page(crate::adjustment_db::normalize_path(&path)),
        None,
        None,
        Some(ResolvedAutoCover {
            identity,
            input: AutoCoverInput::Container,
        }),
        None,
    );
    assert_eq!(done.load(Ordering::Relaxed), 1);
    let messages: Vec<_> = rx.try_iter().collect();
    let pixels = messages.iter().find_map(ThumbMsg::pixels).unwrap();
    assert_eq!(pixels.source_dims, Some((1620, 1024)));
    assert!(matches!(pixels.auto_material,
        crate::cover_auto_material::AutoMaterial::Automatic(results)
            if results.crop_for(crate::rotation_db::Rotation::None).is_some()));
    AUTO_CROP_PDF_FIXTURE
        .with(|current| assert!(current.borrow_mut().take().unwrap().renders.is_empty()));
    let observed = observed.lock().unwrap();
    assert_eq!(observed.len(), 4);
    assert_eq!(observed[0].render, Some((0, 256)));
    assert_eq!(observed[1].render, None);
    assert_eq!(observed[2].render, None);
    assert_eq!(observed[3].render, Some((0, 1620)));
    for request in observed.iter() {
        assert_eq!(request.password.as_deref(), Some("session password"));
        assert_eq!(request.read_address, observed[0].read_address);
        assert_eq!(request.read.as_ref(), read.as_ref());
    }
}

#[test]
fn cover_auto_photo_shot_exif_reaches_zip_and_folder_workers_with_upright_material() {
    use super::cover_auto_analysis::metadata::tests::{jpeg_with_exif, tiff_fixture};
    use image::ImageEncoder;
    use std::io::Write;
    for extension in ["jpg", "png", "webp"] {
        for orientation in [1, 6] {
            for shot in [false, true] {
                let exif =
                    tiff_fixture(orientation, if shot { &[33434][..] } else { &[][..] }, true);
                let pixels = if extension == "jpg" {
                    auto_cover_jacket()
                        .resize_exact(2676, 1536, image::imageops::FilterType::Nearest)
                        .to_rgb8()
                } else {
                    auto_cover_jacket().to_rgb8()
                };
                let dims = (pixels.width(), pixels.height());
                let bytes = if extension == "jpg" {
                    let jpeg = turbojpeg::compress_image(&pixels, 90, turbojpeg::Subsamp::Sub2x2)
                        .unwrap()
                        .to_vec();
                    jpeg_with_exif(&jpeg, &exif)
                } else {
                    let mut bytes = Vec::new();
                    if extension == "webp" {
                        let mut encoder =
                            image::codecs::webp::WebPEncoder::new_lossless(&mut bytes);
                        encoder.set_exif_metadata(exif).unwrap();
                        encoder
                            .write_image(&pixels, dims.0, dims.1, image::ExtendedColorType::Rgb8)
                            .unwrap();
                    } else {
                        let mut encoder = image::codecs::png::PngEncoder::new(&mut bytes);
                        encoder.set_exif_metadata(exif).unwrap();
                        encoder
                            .write_image(&pixels, dims.0, dims.1, image::ExtendedColorType::Rgb8)
                            .unwrap();
                    }
                    bytes
                };
                for folder in [false, true] {
                    let temp = TempDir::new().unwrap();
                    let book = temp.path().join(if folder { "book" } else { "book.zip" });
                    let leaf = format!("000.{extension}");
                    let mut body = std::io::Cursor::new(Vec::new());
                    image::DynamicImage::new_rgb8(70, 100)
                        .write_to(&mut body, image::ImageFormat::Png)
                        .unwrap();
                    if folder {
                        std::fs::create_dir(&book).unwrap();
                        std::fs::write(book.join(&leaf), &bytes).unwrap();
                        for i in 1..=8 {
                            std::fs::write(book.join(format!("{i:03}.png")), body.get_ref())
                                .unwrap();
                        }
                    } else {
                        let mut zip = zip::ZipWriter::new(std::fs::File::create(&book).unwrap());
                        zip.start_file(&leaf, zip::write::SimpleFileOptions::default())
                            .unwrap();
                        zip.write_all(&bytes).unwrap();
                        for i in 1..=8 {
                            zip.start_file(
                                format!("{i:03}.png"),
                                zip::write::SimpleFileOptions::default(),
                            )
                            .unwrap();
                            zip.write_all(body.get_ref()).unwrap();
                        }
                        zip.finish().unwrap();
                    }
                    let mut req = LoadRequest {
                        path: book,
                        zip_entry: (!folder).then(|| leaf.clone()),
                        cache_key_override: Some(
                            if folder {
                                "folderthumb:book"
                            } else {
                                "zipthumb:book.zip"
                            }
                            .into(),
                        ),
                        source_policy: LoadSourcePolicy::SourceOnly,
                        ..Default::default()
                    };
                    req.set_auto_cover_crop(Some(crate::cover_auto_crop::AutoCropIdentity {
                        binding: crate::settings::ReadingDirection::Rtl,
                        folders: true,
                    }));
                    let catalog = Arc::new(
                        crate::catalog::CatalogDb::open(&temp.path().join("cache"), temp.path())
                            .unwrap(),
                    );
                    let messages = run_cover_crop_request(&mut req, &catalog, None);
                    let pixels = messages.iter().find_map(ThumbMsg::pixels).unwrap();
                    // File decoding retains the legacy WIC fallback (which
                    // supports WebP orientation on Windows); ZIP bytes retain
                    // rexif only. Compare against the released route, not an
                    // assumption that every non-JPEG format ignores its tag.
                    let legacy_orientation = if folder {
                        read_exif_orientation(&req.path.join(&leaf))
                    } else {
                        read_exif_orientation_from_bytes(&bytes)
                    };
                    assert_eq!(
                        pixels.source_dims,
                        Some(if (5..=8).contains(&legacy_orientation) {
                            (dims.1, dims.0)
                        } else {
                            dims
                        }),
                        "{extension}/{orientation}/{folder}"
                    );
                    let crate::cover_auto_material::AutoMaterial::Automatic(results) =
                        pixels.auto_material
                    else {
                        panic!("automatic material")
                    };
                    assert_eq!(
                        results.crops().iter().any(Option::is_some),
                        !shot,
                        "{extension}/{orientation}/{folder}/shot={shot}"
                    );
                }
            }
        }
    }
}

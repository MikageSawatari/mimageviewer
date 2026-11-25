// Real source worker sampling and unchanged cache reuse, disposable data only.
#[test]
fn cover_auto_worker_zip_and_folder_sample_saved_body_rotations_without_cache_invalidation() {
    use crate::cover_auto_material::AutoMaterial;
    use crate::rotation_db::{Rotation, RotationDb};
    use std::io::{Cursor, Write};
    let data = crate::data_dir::TestDataDirGuard::new();
    let folder = data.path().join("folder");
    folder_auto_fixture(&folder);
    let zip_path = data.path().join("converted-cache.zip");
    let mut zip = zip::ZipWriter::new(std::fs::File::create(&zip_path).unwrap());
    for index in 0..=8 {
        let mut bytes = Cursor::new(Vec::new());
        let image = if index == 0 { auto_cover_jacket() } else { image::DynamicImage::new_rgb8(70, 100) };
        image.write_to(&mut bytes, image::ImageFormat::Png).unwrap();
        zip.start_file(format!("{index:03}.png"), zip::write::SimpleFileOptions::default()).unwrap();
        zip.write_all(bytes.get_ref()).unwrap();
    }
    zip.finish().unwrap();
    let rotations = RotationDb::open().unwrap();
    for is_folder in [false, true] {
        let path = if is_folder { &folder } else { &zip_path };
        let cache = Arc::new(crate::catalog::CatalogDb::open(
            &data.path().join(if is_folder { "cache-folder" } else { "cache-zip" }), data.path(),
        ).unwrap());
        let mut req = LoadRequest {
            path: path.to_owned(),
            zip_entry: (!is_folder).then(|| "000.png".into()),
            cache_key_override: Some(if is_folder { "folderthumb:folder" } else { "archivethumb:original.rar" }.into()),
            source_policy: LoadSourcePolicy::SourceOnly,
            force_cache: true, mtime: 7, file_size: 9, ..Default::default()
        };
        req.set_auto_cover_crop(Some(crate::cover_auto_crop::AutoCropIdentity {
            binding: crate::settings::ReadingDirection::Rtl, folders: is_folder,
        }));
        req.cover_auto_crop_scope = crate::cover_auto_crop::CoverAutoCropScope::for_identity(req.auto_cover_crop);
        let key = cache_key_for_request(&req).unwrap().into_owned();
        for angle in [Rotation::None, Rotation::Cw180, Rotation::Cw90, Rotation::Cw270] {
            for page in 1..=8 {
                // Converted archives use the actual viewer's cache-ZIP key,
                // never an invented original.rar::entry identity.
                let page_key = if is_folder {
                    crate::adjustment_db::normalize_path(&folder.join(format!("{page:03}.png")))
                } else {
                    crate::adjustment_db::zip_entry_key(&zip_path, &format!("{page:03}.png"))
                };
                rotations.set_key(&page_key, angle).unwrap();
            }
            if let Some(before) = cache.load_one(&key).unwrap() {
                let mut hit = req.clone();
                hit.source_policy = LoadSourcePolicy::CacheOnly;
                hit.mtime = before.mtime;
                hit.file_size = before.file_size;
                let messages = run_cover_crop_request(&mut hit, &cache, None);
                assert_eq!(messages.iter().find_map(ThumbMsg::pixels).unwrap().auto_material, before.auto_material,
                    "body rotation does not refresh an already cached result");
                let after = cache.load_one(&key).unwrap().unwrap();
                assert_eq!(after.jpeg_data, before.jpeg_data);
                assert_eq!(after.auto_material, before.auto_material);
            }
            req.source_policy = LoadSourcePolicy::SourceOnly;
            let messages = run_cover_crop_request(&mut req, &cache, None);
            let pixels = messages.iter().find_map(ThumbMsg::pixels).unwrap();
            let AutoMaterial::Automatic(results) = pixels.auto_material else { panic!("automatic material") };
            assert_eq!(results.crops().iter().any(Option::is_some), matches!(angle, Rotation::None | Rotation::Cw180),
                "folder={is_folder}, body={angle:?}");
            assert_eq!(cache_key_for_request(&req).unwrap().as_ref(), key);
        }
    }
}

#[test]
fn cover_auto_pdf_worker_samples_body_rotation_from_logical_page_with_same_password_and_lease() {
    use crate::rotation_db::{Rotation, RotationDb};
    let data = crate::data_dir::TestDataDirGuard::new();
    let path = data.path().join("logical.epub");
    let read = crate::pdf_loader::generation_target_for_test(&path, 17, 4096);
    let rotations = RotationDb::open().unwrap();
    for angle in [Rotation::None, Rotation::Cw180, Rotation::Cw90, Rotation::Cw270] {
        for page in 1..=8 {
            rotations.set_key(&crate::edit_source::page_key_for_pdf(&path, page), angle).unwrap();
        }
        let jacket = auto_cover_jacket();
        let mut first = cover_only_pdf_result();
        first.image = jacket.resize_exact(224, 128, image::imageops::FilterType::CatmullRom);
        first.page_size_points = crate::pdf_loader::PdfPageSizePoints { width: 892.0, height: 512.0 };
        let mut analysis = cover_only_pdf_result();
        analysis.image = jacket.resize_exact(1784, 1024, image::imageops::FilterType::CatmullRom);
        analysis.page_size_points = first.page_size_points;
        let observed = Arc::new(Mutex::new(Vec::new()));
        let mut sizes = vec![(70.0, 100.0); 9]; sizes[0] = (892.0, 512.0);
        AUTO_CROP_PDF_FIXTURE.with(|current| *current.borrow_mut() = Some(AutoCropPdfFixture {
            sizes, renders: [first, analysis].into_iter().collect(), observed: Arc::clone(&observed),
        }));
        let (tx, rx) = mpsc::channel();
        let done = Arc::new(AtomicUsize::new(0));
        load_one_cached(None, &path, None, None, None, Some(0), Some(&read), Some("same password"),
            None, 0, &tx, None.into(), None, 7, 9, &done, 64, 75, 256,
            make_decision(CachePolicy::Off, 0, 0),
            &Arc::new(Mutex::new(crate::stats::ThumbStats::default())), None, true, 1, 1,
            ThumbnailRequestId::Registered(773), crate::pdf_loader::PdfRenderValidity::Unscoped,
            crate::pdf_loader::CancelWaitPolicy::AbortOnCancel, false, false, None, None, None,
            LoadRequestSource::Original, None, None,
            CoverIdentity::Page(crate::edit_source::page_key_for_pdf(&path, 0)), None, None,
            Some(ResolvedAutoCover {
                identity: crate::cover_auto_crop::AutoCropIdentity::new(crate::settings::ReadingDirection::Rtl),
                input: AutoCoverInput::Container,
            }), None, None);
        assert_eq!(done.load(Ordering::Relaxed), 1);
        let messages: Vec<_> = rx.try_iter().collect();
        let material = messages.iter().find_map(ThumbMsg::pixels).unwrap().auto_material;
        let crate::cover_auto_material::AutoMaterial::Automatic(results) = material else { panic!("automatic") };
        assert_eq!(results.crops().iter().any(Option::is_some), matches!(angle, Rotation::None | Rotation::Cw180));
        AUTO_CROP_PDF_FIXTURE.with(|current| assert!(current.borrow_mut().take().unwrap().renders.is_empty()));
        let observed = observed.lock().unwrap();
        assert_eq!(observed.len(), 4);
        for request in observed.iter() {
            assert_eq!(request.password.as_deref(), Some("same password"));
            assert_eq!(request.read.as_ref(), Some(&read));
            assert_eq!(request.read_address, observed[0].read_address);
        }
    }
}

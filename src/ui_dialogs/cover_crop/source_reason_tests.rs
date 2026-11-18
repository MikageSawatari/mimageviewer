use super::*;
use crate::folder_thumb_pins::{FileKind, FolderThumbPinDb};
use crate::thumb_loader::{LoadRequest, ReadLocation, resolve_read_after_cache_miss};

fn assert_folder_stop(
    container: &BookCoverContainer,
    source: Option<FolderPin>,
    env: &BookCoverEnv,
    reason: AutoCoverSkip,
    text: &str,
) {
    let BookCover::Readable(resolved) = resolve_logical_book_cover(container, source.clone(), env)
    else {
        panic!("readable synthetic cover");
    };
    assert_eq!(resolved.auto_cover_crop.as_ref().unwrap_err(), &reason);
    let prepared = prepare_cover(
        container,
        source.map(|pin| pin.source),
        env,
        256,
        &Arc::new(AtomicBool::new(false)),
    )
    .unwrap();
    assert_eq!(
        prepared.auto_detection,
        AutoDetection::Stopped(
            AutoStop::for_source(&resolved.auto_cover_crop, env.auto_cover_crop.is_some()).unwrap()
        )
    );
    assert_eq!(
        prepared.auto_detection.message(Rotation::None),
        format!("自動の判定: 切り出していません。{text}")
    );
    if let AutoCoverSkip::Manual {
        auto_eligible_after_clear,
    } = reason
    {
        assert_eq!(
            prepared.auto_detection.enabled(),
            env.auto_cover_crop.is_some() && auto_eligible_after_clear,
            "manual reason must retain the setting for clear-crop guidance"
        );
    }
}

#[test]
fn cover_crop_folder_stops_arrive_from_worker_for_pin_manual_setting_and_descendant() {
    let _data = crate::data_dir::TestDataDirGuard::new();
    let temp = tempfile::tempdir().unwrap();
    let folder = temp.path().join("book");
    std::fs::create_dir(&folder).unwrap();
    image::DynamicImage::new_rgb8(300, 200)
        .save(folder.join("000.png"))
        .unwrap();
    for index in 1..=3 {
        image::DynamicImage::new_rgb8(70, 100)
            .save(folder.join(format!("{index:03}.png")))
            .unwrap();
    }
    let container = BookCoverContainer {
        path: folder.clone(),
        kind: BookKind::Folder,
        zip_dir_prefix: None,
    };
    let pin_db = Arc::new(FolderThumbPinDb::open_at(&temp.path().join("pins.db")).unwrap());
    let mut env = BookCoverEnv {
        books_root: Some(temp.path().to_owned()),
        pin_db: Some(pin_db.clone()),
        auto_cover_crop: Some(crate::cover_auto_crop::AutoCropIdentity {
            binding: crate::settings::ReadingDirection::Rtl,
            folders: true,
        }),
        ..Default::default()
    };
    let source = FolderPinSource::File {
        rel: "000.png".into(),
        kind: FileKind::Image,
    };
    // Entry B supplies its selected leaf before the initial preparation.
    assert_folder_stop(
        &container,
        Some(source.clone().into()),
        &env,
        AutoCoverSkip::PinnedFolder,
        "代表のページを固定しているため、自動の判定は使いません",
    );
    // Reopening that saved pin uses the same worker reason.
    pin_db.set(&folder, &source).unwrap();
    assert_folder_stop(
        &container,
        None,
        &env,
        AutoCoverSkip::PinnedFolder,
        "代表のページを固定しているため、自動の判定は使いません",
    );
    pin_db
        .set_with_cover_crop(
            &folder,
            &source,
            CoverCrop::new([0, 0, 500_000, 1_000_000]).unwrap(),
        )
        .unwrap();
    for enabled in [true, false] {
        if !enabled {
            env.auto_cover_crop = None;
        }
        assert_folder_stop(
            &container,
            None,
            &env,
            AutoCoverSkip::Manual {
                auto_eligible_after_clear: false,
            },
            "手動の範囲を使っています",
        );
    }
    pin_db.remove(&folder).unwrap();
    assert_folder_stop(
        &container,
        None,
        &env,
        AutoCoverSkip::Off,
        "自動の切り出しはオフです",
    );
    env.auto_cover_crop = Some(crate::cover_auto_crop::AutoCropIdentity::new(
        crate::settings::ReadingDirection::Rtl,
    ));
    assert_folder_stop(
        &container,
        None,
        &env,
        AutoCoverSkip::FolderDisabled,
        "画像フォルダの本は自動の対象にしていません (環境設定で変えられます)",
    );
    env.auto_cover_crop.as_mut().unwrap().folders = true;
    let parent = BookCoverContainer {
        path: temp.path().to_owned(),
        ..container.clone()
    };
    assert_folder_stop(
        &parent,
        None,
        &env,
        AutoCoverSkip::ChildRepresentative,
        "代表が子フォルダや中の本の画像なので、自動の判定は使いません",
    );
}

#[test]
fn cover_crop_archive_exclusions_and_fixed_container_pages_use_worker_types() {
    let identity =
        crate::cover_auto_crop::AutoCropIdentity::new(crate::settings::ReadingDirection::Rtl);
    for (path, entry, reason, text) in [
        (
            "book.zip",
            "inner.zip/001.png",
            AutoCoverSkip::NestedZip,
            "ZIP の中の ZIP は自動の対象外です",
        ),
        (
            "book.zip",
            "001.nef",
            AutoCoverSkip::Unsupported,
            "この形式の本は自動の対象外です",
        ),
    ] {
        let mut request = LoadRequest {
            path: path.into(),
            zip_entry: Some(entry.into()),
            cache_key_override: Some("zipthumb:book#pin:selected".into()),
            ..Default::default()
        };
        request.set_auto_cover_crop(Some(identity));
        let resolved = resolve_read_after_cache_miss(&request, None, None);
        assert_eq!(resolved.auto_cover_crop.as_ref().unwrap_err(), &reason);
        let stop = AutoStop::for_source(&resolved.auto_cover_crop, true).unwrap();
        assert_eq!(
            AutoDetection::Stopped(stop).message(Rotation::None),
            format!("自動の判定: 切り出していません。{text}")
        );
    }
    // A container's own pinned ZIP/RAR/PDF page remains eligible, unlike a folder pin.
    for path in ["book.zip", "book.rar", "book.cbr", "book.pdf"] {
        let pdf = path.ends_with(".pdf");
        let mut request = LoadRequest {
            path: path.into(),
            zip_entry: (!pdf).then(|| "001.png".into()),
            pdf_page: pdf.then_some(2),
            cache_key_override: Some(
                if pdf {
                    "pdfthumb:book#pin:selected"
                } else {
                    "zipthumb:book#pin:selected"
                }
                .into(),
            ),
            folder_thumb_provenance: Some(crate::catalog::FolderThumbProvenance::Seeded),
            ..Default::default()
        };
        request.set_auto_cover_crop(Some(identity));
        let resolved = resolve_read_after_cache_miss(&request, None, None);
        assert!(resolved.auto_cover_crop.is_ok());
        assert!(AutoStop::for_source(&resolved.auto_cover_crop, true).is_none());
        assert!(matches!(resolved.location, ReadLocation::PdfPage { page: 2, .. }) == pdf);
        request.cover_crop = CoverCrop::new([0, 0, 500_000, 1_000_000]).unwrap();
        request.set_auto_cover_crop(Some(identity));
        let manual = resolve_read_after_cache_miss(&request, None, None);
        assert_eq!(
            manual.auto_cover_crop.as_ref().unwrap_err(),
            &AutoCoverSkip::Manual {
                auto_eligible_after_clear: true
            }
        );
        for configured in [false, true] {
            let stopped = AutoDetection::Stopped(
                AutoStop::for_source(&manual.auto_cover_crop, configured).unwrap(),
            );
            assert_eq!(stopped.enabled(), configured);
            assert!(
                stopped
                    .message(Rotation::None)
                    .ends_with("手動の範囲を使っています")
            );
        }
    }
}

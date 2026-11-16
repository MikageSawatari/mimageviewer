//! Stage A: preserve a worker's EPUB pin row through ordinary-folder cleanup.
use super::*;
use crate::folder_thumb_pins::{FileKind, FolderPinSource, FolderThumbPinDb};

#[derive(Clone, Copy, Debug)]
enum Route {
    FolderPage,
    FolderFirstPage,
    FolderCascade,
    EpubPage,
    PdfPageControl,
}

fn assert_pin_survives_folder_cleanup(route: Route) {
    let tmp = tempfile::tempdir().unwrap();
    let parent = tmp.path().join("shelf");
    let folder = parent.join("child");
    std::fs::create_dir_all(&folder).unwrap();
    let epub = folder.join(if matches!(route, Route::PdfPageControl) {
        "book.pdf"
    } else {
        "book.epub"
    });
    // No EPUB conversion or PDF renderer is needed: exercise the exact worker
    // stamp helper with an explicitly supplied generation ReadTarget.
    std::fs::write(&epub, b"source attributes, not rendered").unwrap();
    // A real worker resolution also installs the process's immutable generation.
    // Keep it available to memory-only inventory helpers through cleanup.
    let _generation = (!matches!(route, Route::PdfPageControl))
        .then(|| crate::pdf_loader::pin_epub_for_test(&epub, 17, 4096));
    let pin_db = FolderThumbPinDb::open_at(&tmp.path().join("pins.db")).unwrap();
    let (item, source) = match route {
        Route::FolderPage => (
            GridItem::Folder(folder.clone()),
            FolderPinSource::PdfPage {
                pdf_rel: "book.epub".into(),
                page: 2,
            },
        ),
        Route::FolderFirstPage | Route::FolderCascade => {
            if matches!(route, Route::FolderCascade) {
                pin_db
                    .set(
                        &epub,
                        &FolderPinSource::PdfPage {
                            pdf_rel: String::new(),
                            page: 2,
                        },
                    )
                    .unwrap();
            }
            (
                GridItem::Folder(folder.clone()),
                FolderPinSource::File {
                    rel: "book.epub".into(),
                    kind: FileKind::PdfFile,
                },
            )
        }
        Route::EpubPage | Route::PdfPageControl => (
            GridItem::PdfFile(epub.clone()),
            FolderPinSource::PdfPage {
                pdf_rel: String::new(),
                page: 2,
            },
        ),
    };
    let container = match &item {
        GridItem::Folder(path) | GridItem::PdfFile(path) => path,
        _ => unreachable!(),
    };
    pin_db.set(container, &source).unwrap();
    let pins = pin_db.lookup_many([container.as_path()]);
    let req = make_load_request(
        &item,
        0,
        123,
        456,
        false,
        None,
        Some(crate::settings::SortOrder::Numeric),
        3,
        &pins,
        &Default::default(),
        None,
        container.parent(),
        Some(&pin_db),
        None,
        false,
    )
    .expect("a pinned representative must create a request");
    assert_eq!(req.path, epub);
    assert_eq!(
        req.pdf_page,
        Some(if matches!(route, Route::FolderFirstPage) {
            0
        } else {
            2
        })
    );
    let stamped = if matches!(route, Route::PdfPageControl) {
        assert!(matches!(
            req.pdf_stamp_policy,
            crate::thumb_loader::PdfStampPolicy::CallerFileAttributes
        ));
        req.clone()
    } else {
        assert!(matches!(
            req.pdf_stamp_policy,
            crate::thumb_loader::PdfStampPolicy::ResolveInWorker
        ));
        let (stamped, read) = crate::thumb_loader::prepare_load_request_stamp(&req)
            .expect("resolve the fixed test generation")
            .expect("EPUB requests resolve in the worker");
        assert_eq!(read.stamp.generation_catalog_pair(), Some((17, 4096)));
        stamped
    };
    let worker_key = stamped.cache_key_override.as_ref().unwrap();
    let catalog =
        crate::catalog::CatalogDb::open(&tmp.path().join("catalogs"), container.parent().unwrap())
            .unwrap();
    let cover = crate::cover_identity::CoverIdentity::Page(crate::edit_source::page_key_for_pdf(
        &epub,
        stamped.pdf_page.unwrap(),
    ));
    catalog
        .save(
            worker_key,
            stamped.mtime,
            stamped.file_size,
            4,
            4,
            None,
            b"opaque WebP bytes: cleanup never decodes",
            &cover,
            crate::cover_auto_material::AutoMaterial::NotAutomatic,
        )
        .unwrap();
    assert!(catalog.load_one(worker_key).unwrap().is_some());
    let visibility = crate::file_type_visibility::FileTypeVisibility::default();
    assert!(visibility.is_all_visible());
    // Same inventory projection and all-visible gate as load_folder and the
    // thumbnail pool, without launching the app or asynchronous folder scan.
    let existing: HashSet<_> = folder_thumb_existing_keys_for_with_auto_crop(
        &item,
        Some((123, 456)),
        &pins,
        Some(&pin_db),
        Some(crate::settings::SortOrder::Numeric),
        3,
        false,
        &visibility,
        None,
    )
    .into_iter()
    .collect();
    if visibility.is_all_visible() {
        catalog.delete_missing(&existing).unwrap();
    }
    assert!(
        catalog.load_one(worker_key).unwrap().is_some(),
        "{route:?}: ordinary-folder cleanup removed the worker row; worker={worker_key:?}, retained={existing:?}"
    );
}

#[test]
fn section1742_folder_epub_page_pin_survives_cleanup() {
    assert_pin_survives_folder_cleanup(Route::FolderPage);
}

#[test]
fn section1742_folder_epub_first_page_pin_survives_cleanup() {
    assert_pin_survives_folder_cleanup(Route::FolderFirstPage);
}

#[test]
fn section1742_folder_cascaded_epub_page_pin_survives_cleanup() {
    assert_pin_survives_folder_cleanup(Route::FolderCascade);
}

#[test]
fn section1742_epub_self_page_pin_survives_cleanup() {
    assert_pin_survives_folder_cleanup(Route::EpubPage);
}

#[test]
fn section1742_normal_pdf_page_pin_survives_cleanup_control() {
    assert_pin_survives_folder_cleanup(Route::PdfPageControl);
}

#[test]
fn section1742_archive_containers_reject_epub_pin_sources_control() {
    // ZIP/converted-archive containers accept only their own entries/dirs.
    // EPUB members are not PdfPages and cannot enter the generation-stamp path.
    for container in [
        ContainerKindForPin::ZipFile,
        ContainerKindForPin::ConvertibleArchive,
    ] {
        for source in [
            FolderPinSource::File {
                rel: "book.epub".into(),
                kind: FileKind::PdfFile,
            },
            FolderPinSource::PdfPage {
                pdf_rel: "book.epub".into(),
                page: 2,
            },
            FolderPinSource::PdfPage {
                pdf_rel: String::new(),
                page: 2,
            },
        ] {
            assert!(!pin_source_compatible_with_container(container, &source));
        }
    }
}

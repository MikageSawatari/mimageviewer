//! Everything uses the accepted result row as its navigation identity. These tests
//! exercise ordinary handlers and the successful-adoption boundary without IPC/UI input.
use super::*;
use crate::global_search_ui::{
    EverythingChildAdmission, EverythingPosition, EverythingReturn, SearchAccepted,
    SearchOpenOrigin, SearchSource, adopt_everything_for_test, everything_dataset_for_test,
    start_everything_for_test,
};
use std::time::{Duration, Instant};

fn adopt_rows(app: &mut App, rows: Vec<GridItem>) -> u64 {
    adopt_everything_for_test(
        app,
        &egui::Context::default(),
        "list-order",
        everything_dataset_for_test(rows),
    );
    accepted_run(app)
}

fn accepted_run(app: &App) -> u64 {
    let SearchAccepted::Everything { submitted, .. } = &app.global_search.accepted else {
        panic!("expected accepted Everything dataset");
    };
    submitted.run
}

fn assert_child(app: &App, expected: &Path) {
    assert!(matches!(&app.global_search.accepted,
        SearchAccepted::Everything {
            position: EverythingPosition::PhysicalChild { logical_source, return_anchor, .. }, ..
        } if logical_source == expected && return_anchor == expected));
}

fn row_origin(app: &App, dataset_run: u64, row: &Path) -> SearchOpenOrigin {
    let mut origin = app.capture_search_open_origin().unwrap();
    let SearchOpenOrigin::Everything { child, .. } = &mut origin else {
        panic!("expected Everything origin");
    };
    *child = EverythingChildAdmission::ListOrderRow {
        dataset_run,
        row: row.to_path_buf(),
    };
    origin
}

fn commit_row(app: &mut App, row: &Path) {
    let origin = row_origin(app, accepted_run(app), row);
    app.commit_staged_search_navigation_effects(row, false, false, Some(origin));
}

fn settle_search(app: &mut App) {
    let generation = app.items_generation;
    let ctx = egui::Context::default();
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.items_generation == generation || app.everything_search_progress_visible() {
        app.poll_search_page_edit_prepare(&ctx);
        assert!(
            Instant::now() < deadline,
            "Everything reprojection did not finish"
        );
        std::thread::yield_now();
    }
}

#[test]
fn list_order_everything_capture_requires_accepted_root_and_the_result_rows() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let previous = capture_fixture(&mut app, &root);
    let a = root.join("a.zip");
    let b = root.join("b.pdf");
    let run = adopt_rows(
        &mut app,
        vec![GridItem::ZipFile(a.clone()), GridItem::PdfFile(b.clone())],
    );
    assert!(app.items_are_global_search_view);
    // A Root session may survive address-bar navigation into a physical listing.
    app.items_are_global_search_view = false;
    app.capture_main_folder_list_order(0);
    assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &previous));
    app.items_are_global_search_view = true;
    app.capture_main_folder_list_order(0);
    assert_eq!(
        app.list_order.as_ref().unwrap().scope,
        ListOrderScope::Everything { run }
    );
    assert_eq!(captured_paths(&app), vec![a.clone(), b]);
    let snapshot = app.list_order.clone().unwrap();
    assert!(
        app.list_order_position().is_none(),
        "Root has no opened result row"
    );
    app.advance_drilled_current_path(&a);
    app.current_folder = Some(a.clone());
    app.items = vec![GridItem::Folder(root.join("physical-child"))];
    app.visible_indices = vec![0];
    app.items_are_global_search_view = false;
    app.capture_main_folder_list_order(0);
    assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
    // Even incorrectly retained result-surface bits cannot recapture in PhysicalChild.
    app.items_are_global_search_view = true;
    app.capture_main_folder_list_order(0);
    assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
    assert_eq!(app.list_order_position().unwrap().1, 0);
}

#[test]
fn list_order_everything_capture_uses_real_name_date_and_details_sorts() {
    use crate::settings::{DetailsSortKey, GridViewMode, SortOrder};
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let a = root.join("a.zip");
    let b = root.join("b.zip");
    let c = root.join("c.zip");
    let rows = vec![
        GridItem::ZipFile(c.clone()),
        GridItem::ZipFile(a.clone()),
        GridItem::ZipFile(b.clone()),
        GridItem::Image(root.join("picture.jpg")),
        GridItem::Video(root.join("video.mp4")),
    ];
    adopt_rows(&mut app, rows);
    for (sort, expected) in [
        (SortOrder::FileName, vec![a.clone(), b.clone(), c.clone()]),
        (
            SortOrder::FileNameDesc,
            vec![c.clone(), b.clone(), a.clone()],
        ),
        // Prepared metadata timestamps are c=10, a=11, b=12.
        (SortOrder::DateAsc, vec![c.clone(), a.clone(), b.clone()]),
        (SortOrder::DateDesc, vec![b.clone(), a.clone(), c.clone()]),
    ] {
        app.settings.sort_order = sort;
        app.rebuild_items_from_global_search();
        settle_search(&mut app);
        let index = app
            .items
            .iter()
            .position(|row| row == &GridItem::ZipFile(a.clone()))
            .unwrap();
        app.capture_main_folder_list_order(index);
        assert_eq!(captured_paths(&app), expected, "{sort:?}");
    }
    app.settings.grid_view_mode = GridViewMode::Details;
    app.settings.details_show_modified = true;
    for (key, expected) in [
        (DetailsSortKey::Name, vec![a.clone(), b.clone(), c.clone()]),
        (
            DetailsSortKey::Modified,
            vec![c.clone(), a.clone(), b.clone()],
        ),
    ] {
        app.settings.details_sort_key = DetailsSortKey::Toolbar;
        app.set_details_sort_key(key);
        let index = app
            .items
            .iter()
            .position(|row| row == &GridItem::ZipFile(a.clone()))
            .unwrap();
        app.capture_main_folder_list_order(index);
        assert_eq!(captured_paths(&app), expected);
        app.set_details_sort_key(key);
        app.capture_main_folder_list_order(index);
        assert_eq!(
            captured_paths(&app),
            expected.into_iter().rev().collect::<Vec<_>>()
        );
    }
}

#[test]
fn list_order_everything_capture_intersects_rating_tags_facet_and_ctrl_f() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let names = [
        "keep-alpha.zip",
        "keep-beta.zip",
        "other-alpha.zip",
        "keep-alpha-low.zip",
        "keep-alpha-untagged.zip",
    ];
    let paths = names.map(|name| root.join(name));
    let mut tags = crate::tags_db::TagsDb::open_at(&root.join("capture-tags.db")).unwrap();
    for path in paths.iter().take(4) {
        tags.set_item_tags(
            &crate::tags_db::item_key_for_path(path),
            ["chosen"],
            crate::tags_db::source::EDIT,
        )
        .unwrap();
    }
    app.tags_db = Some(tags);
    adopt_rows(
        &mut app,
        paths.iter().cloned().map(GridItem::ZipFile).collect(),
    );
    for index in 0..app.items.len() {
        let rating = if app.items[index].file_operation_path() == Some(paths[3].as_path()) {
            1
        } else {
            5
        };
        app.rating_cache.insert(index, rating);
    }
    app.settings.rating_filter = [false, false, false, false, false, true];
    app.settings.facet_filter.tags.insert("chosen".into());
    app.settings.facet_filter.name_query = "keep".into();
    app.facet_name_tokens = crate::search_query::parse("keep");
    app.facet_name_cache = app
        .items
        .iter()
        .map(|row| row.name().to_lowercase().into_boxed_str())
        .collect();
    app.facet_name_cache_generation = Some(app.items_generation);
    app.search_query = "alpha".into();
    app.search_target =
        crate::fts_index::SearchTarget::Only(vec![crate::fts_index::SourceKind::Filename]);
    let ctx = egui::Context::default();
    app.execute_search(&ctx);
    let deadline = Instant::now() + Duration::from_secs(10);
    while app.search_pending.is_some() {
        app.poll_search(&ctx);
        assert!(Instant::now() < deadline, "local filter did not finish");
        std::thread::yield_now();
    }
    assert_eq!(app.visible_indices.len(), 1);
    let index = app.visible_indices[0];
    assert_eq!(app.items[index], GridItem::ZipFile(paths[0].clone()));
    app.capture_main_folder_list_order(index);
    assert_eq!(captured_paths(&app), vec![paths[0].clone()]);
}

#[test]
fn list_order_everything_capture_preserves_drive_identity_and_omits_single_media() {
    let mut app = setup_app();
    let rows = vec![
        GridItem::ZipFile(PathBuf::from("C:/Books/shared.zip")),
        GridItem::ZipFile(PathBuf::from("D:/Books/shared.zip")),
        GridItem::PdfFile(PathBuf::from("D:/Books/manual.epub")),
        GridItem::ConvertibleArchive {
            path: PathBuf::from("E:/Books/other.7z"),
            format: crate::archive_converter::ArchiveFormat::SevenZ,
        },
        GridItem::Folder(PathBuf::from("E:/Books/images")),
        GridItem::Image(PathBuf::from("C:/Books/photo.jpg")),
        GridItem::Video(PathBuf::from("D:/Books/movie.mp4")),
        GridItem::Audio(PathBuf::from("E:/Books/music.mp3")),
    ];
    adopt_rows(&mut app, rows);
    app.visible_indices = (0..app.items.len()).rev().collect();
    let expected = app
        .current_grid_order()
        .iter()
        .filter_map(|&index| match &app.items[index] {
            GridItem::Folder(path)
            | GridItem::ZipFile(path)
            | GridItem::PdfFile(path)
            | GridItem::ConvertibleArchive { path, .. } => Some(path.clone()),
            _ => None,
        })
        .collect::<Vec<_>>();
    let index = app
        .items
        .iter()
        .position(|row| matches!(row, GridItem::ZipFile(_)))
        .unwrap();
    app.capture_main_folder_list_order(index);
    assert_eq!(captured_paths(&app), expected);
    assert_eq!(expected.len(), 5);
    assert!(expected.contains(&PathBuf::from("C:/Books/shared.zip")));
    assert!(expected.contains(&PathBuf::from("D:/Books/shared.zip")));
}

#[test]
fn list_order_everything_position_uses_adopted_row_and_current_physical_key() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let a = root.join("A");
    let b = a.join("B");
    let inner = b.join("inner.zip");
    let last = root.join("last.zip");
    adopt_rows(
        &mut app,
        vec![
            GridItem::Folder(a.clone()),
            GridItem::Folder(b.clone()),
            GridItem::ZipFile(inner.clone()),
            GridItem::ZipFile(last.clone()),
        ],
    );
    app.visible_indices = (0..app.items.len()).collect();
    let a_index = app
        .items
        .iter()
        .position(|row| row == &GridItem::Folder(a.clone()))
        .unwrap();
    app.capture_main_folder_list_order(a_index);
    let a_position = captured_paths(&app)
        .iter()
        .position(|path| path == &a)
        .unwrap();
    let b_position = captured_paths(&app)
        .iter()
        .position(|path| path == &b)
        .unwrap();
    let inner_position = captured_paths(&app)
        .iter()
        .position(|path| path == &inner)
        .unwrap();
    app.advance_drilled_current_path(&a);
    for presentation in [
        ViewerPresentation::MainWindow,
        ViewerPresentation::Fullscreen,
    ] {
        app.viewer_presentation = presentation;
        for (path, expected) in [
            (a.clone(), Some(a_position)),
            (b.clone(), Some(a_position)),
            (inner.clone(), Some(a_position)),
            (root.join("A-extra/book.zip"), None),
            (last.clone(), None),
        ] {
            app.current_folder = Some(path);
            assert_eq!(app.list_order_position().map(|(_, index)| index), expected);
        }
    }
    // Physical navigation into another dataset row does not replace the original row.
    app.advance_drilled_current_path(&b);
    assert_child(&app, &a);
    app.current_folder = Some(b.clone());
    assert_eq!(app.list_order_position().unwrap().1, a_position);
    commit_row(&mut app, &b);
    assert_child(&app, &b);
    assert_eq!(app.list_order_position().unwrap().1, b_position);
    commit_row(&mut app, &inner);
    app.current_folder = Some(inner.clone());
    assert_child(&app, &inner);
    assert_eq!(app.list_order_position().unwrap().1, inner_position);
    app.current_folder = Some(PathBuf::from(
        inner.to_string_lossy().replace('\\', "/").to_uppercase(),
    ));
    assert_eq!(
        app.list_order_position().unwrap().1,
        inner_position,
        "same physical spelling key"
    );
    app.current_folder = Some(inner.join("not-a-folder-descendant"));
    assert!(app.list_order_position().is_none());
    app.current_folder = Some(inner);
    app.viewer_presentation = ViewerPresentation::DetachedWindow;
    assert!(app.list_order_position().is_none());
    app.viewer_presentation = ViewerPresentation::Fullscreen;
    app.navigation_scope = ViewerNavigationScope::DetachedPhysical;
    assert!(app.list_order_position().is_none());
}

#[test]
fn list_order_everything_scope_uses_accepted_run_through_resubmit_cancel_failure_and_source_switch()
{
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let old_folder = capture_fixture(&mut app, &root);
    let a = root.join("a.zip");
    let run = adopt_rows(&mut app, vec![GridItem::ZipFile(a.clone())]);
    app.advance_drilled_current_path(&a);
    app.current_folder = Some(a.clone());
    assert!(
        app.list_order_position().is_none(),
        "ordinary folder order cannot cross into Everything"
    );
    app.list_order = Some(Arc::new(ListOrderSnapshot {
        owner: app.viewer_context_main(),
        scope: ListOrderScope::Everything { run },
        entries: vec![ListOrderEntry {
            path: a.clone(),
            kind: ListOrderEntryKind::Zip,
        }]
        .into(),
    }));
    assert_eq!(app.list_order_position().unwrap().1, 0);
    let ctx = egui::Context::default();
    start_everything_for_test(&mut app, &ctx, "replacement");
    assert_ne!(app.global_search.run_sequence, run);
    assert_eq!(
        app.list_order_position().unwrap().1,
        0,
        "submission leaves accepted owner intact"
    );
    app.cancel_everything_search();
    assert_eq!(app.list_order_position().unwrap().1, 0);
    app.switch_search_source(SearchSource::Metadata);
    assert_eq!(
        app.list_order_position().unwrap().1,
        0,
        "draft source does not replace accepted source"
    );
    app.switch_search_source(SearchSource::Everything);
    assert_eq!(app.list_order_position().unwrap().1, 0);
    let snapshot = app.list_order.clone().unwrap();
    // A newly accepted dataset, even with the same physical row, ends the old scope.
    app.fullscreen_idx = None;
    adopt_rows(&mut app, vec![GridItem::ZipFile(a.clone())]);
    app.advance_drilled_current_path(&a);
    app.current_folder = Some(a.clone());
    assert!(app.list_order_position().is_none());
    assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
    app.global_search.active = false;
    assert!(app.list_order_position().is_none());
    app.top_level_grid_view
        .replace_surface(TopLevelGridSurface::Folder);
    assert!(
        app.list_order_position().is_none(),
        "Everything snapshot cannot cross into ordinary folder"
    );
    app.list_order = Some(old_folder);
}

#[test]
fn list_order_everything_admission_requires_current_outer_run_and_accepted_dataset_run() {
    let mut app = setup_app();
    let root = app.tmp.path().to_path_buf();
    let a = root.join("a.zip");
    let b = root.join("b.zip");
    let run = adopt_rows(
        &mut app,
        vec![GridItem::ZipFile(a.clone()), GridItem::ZipFile(b.clone())],
    );
    app.advance_drilled_current_path(&a);
    let old_origin = row_origin(&app, run, &b);
    let ctx = egui::Context::default();
    start_everything_for_test(&mut app, &ctx, "cancelled replacement");
    app.cancel_everything_search();
    assert_ne!(app.global_search.run_sequence, run);
    assert!(!app.search_open_origin_is_current(&old_origin));
    app.commit_staged_search_navigation_effects(&b, false, false, Some(old_origin));
    assert_child(&app, &a);
    let current = row_origin(&app, run, &b);
    assert!(app.search_open_origin_is_current(&current));
    assert!(
        matches!(&current, SearchOpenOrigin::Everything { run: outer, child: EverythingChildAdmission::ListOrderRow { dataset_run, .. }, .. } if outer != dataset_run)
    );
    let wrong_dataset = row_origin(&app, app.global_search.run_sequence, &b);
    app.commit_staged_search_navigation_effects(&b, false, false, Some(wrong_dataset));
    assert_child(&app, &a);
    app.commit_staged_search_navigation_effects(&a, false, false, Some(current.clone()));
    assert_child(&app, &a);
    app.commit_staged_search_navigation_effects(&b, false, false, Some(current));
    assert_child(&app, &b);
    let outsider = root.join("outside.zip");
    let origin = row_origin(&app, run, &outsider);
    app.commit_staged_search_navigation_effects(&outsider, false, false, Some(origin));
    assert_child(&app, &b);
    let mut wrong_context = row_origin(&app, run, &a);
    if let SearchOpenOrigin::Everything { context, .. } = &mut wrong_context {
        *context = ViewerContextId::for_test(u64::MAX);
    }
    assert!(!app.search_open_origin_is_current(&wrong_context));
    app.commit_staged_search_navigation_effects(&a, false, false, Some(wrong_context));
    assert_child(&app, &b);
    let inactive = row_origin(&app, run, &a);
    app.global_search.active = false;
    app.commit_staged_search_navigation_effects(&a, false, false, Some(inactive));
    assert_child(&app, &b);
}

#[derive(Clone, Copy, Debug)]
enum Landing {
    Zip,
    ColdPdf,
    WarmPdf,
    Epub,
    Converted,
    UncachedArchive,
    Folder,
    ImageFolder,
    FolderInnerZip,
}

const LANDINGS: [Landing; 8] = [
    Landing::Zip,
    Landing::ColdPdf,
    Landing::WarmPdf,
    Landing::Epub,
    Landing::Converted,
    Landing::Folder,
    Landing::ImageFolder,
    Landing::FolderInnerZip,
];

struct LandingFixture {
    source: PathBuf,
    row: PathBuf,
    destination: PathBuf,
    snapshot: Arc<ListOrderSnapshot>,
    epub_pin: Option<crate::pdf_loader::TestEpubPin>,
    document_stamp: Option<crate::pdf_loader::DocumentStamp>,
}

fn write_page(path: &Path) {
    image::RgbImage::new(2, 2).save(path).unwrap();
}

fn write_zip(path: &Path) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    zip.start_file("001.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    let mut bytes = std::io::Cursor::new(Vec::new());
    image::RgbImage::new(2, 2)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    std::io::Write::write_all(&mut zip, bytes.get_ref()).unwrap();
    zip.finish().unwrap();
}

fn landing_fixture(
    app: &mut tests::phase_c_support::AppTestEnv,
    landing: Landing,
) -> LandingFixture {
    app.active_quick_folder_slot = None;
    app.settings.auto_fullscreen_zip_pdf = true;
    app.settings.auto_fullscreen_image_folders = true;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.sort_order = crate::settings::SortOrder::FileName;
    app.settings.book_nav_resume = crate::settings::ResumeMode::FromStart;
    let root = app.tmp.path().to_path_buf();
    let source = root.join("a-source");
    std::fs::create_dir(&source).unwrap();
    write_page(&source.join("001.png"));
    let row = root.join(match landing {
        Landing::Zip => "b-target.zip",
        Landing::ColdPdf | Landing::WarmPdf => "b-target.pdf",
        Landing::Epub => "b-target.epub",
        Landing::Converted | Landing::UncachedArchive => "b-target.7z",
        _ => "b-target",
    });
    let mut epub_pin = None;
    let mut document_stamp = None;
    let target_item = match landing {
        Landing::Zip => {
            write_zip(&row);
            GridItem::ZipFile(row.clone())
        }
        Landing::ColdPdf | Landing::WarmPdf => {
            std::fs::write(&row, b"%PDF-1.4\n").unwrap();
            if matches!(landing, Landing::WarmPdf) {
                let metadata = std::fs::metadata(&row).unwrap();
                app.get_or_open_catalog(&root)
                    .unwrap()
                    .set_pdf_meta(
                        "b-target.pdf",
                        crate::ui_helpers::mtime_secs(&metadata),
                        metadata.len() as i64,
                        1,
                        false,
                    )
                    .unwrap();
            }
            GridItem::PdfFile(row.clone())
        }
        Landing::Epub => {
            use crate::epub_cache::{EpubCache, GenerationRow, WriteDenyingSource};
            std::fs::write(&row, b"EPUB source fixture").unwrap();
            let mut cache = EpubCache::open_at(&root).unwrap();
            let reserved = cache.reserve_output(&row).unwrap();
            let generated = reserved.final_path().to_path_buf();
            std::fs::create_dir_all(generated.parent().unwrap()).unwrap();
            std::fs::write(&generated, b"generated PDF fixture").unwrap();
            let generation = GenerationRow {
                generation_id: reserved.generation_id(),
                src_path_key: crate::epub_cache::src_key(&row),
                src_path: row.clone(),
                src_state: crate::epub_cache::source_state(&std::fs::metadata(&row).unwrap()),
                src_sha256: "everything-list-order-full".into(),
                src_head_hash: "everything-list-order-head".into(),
                pdf_file: generated.clone(),
                pdf_size: std::fs::metadata(&generated).unwrap().len(),
                page_count: 1,
                direction: "rtl".into(),
                profile: "reflow-v1".into(),
                created_at: 1,
                output_version: crate::epub_cache::CONVERTER_OUTPUT_VERSION,
            };
            let guard = WriteDenyingSource::open(&row).unwrap();
            assert!(matches!(
                cache.publish(&generation, &guard),
                Ok(crate::epub_cache::PublishOutcome::Published)
            ));
            let (pin, target) = crate::pdf_loader::pin_cached_epub_for_test(&row, &root).unwrap();
            document_stamp = Some(target.stamp.clone());
            epub_pin = Some(pin);
            app.settings.epub_file_handling = crate::settings::EpubFileHandling::Convert;
            GridItem::PdfFile(row.clone())
        }
        Landing::Converted => {
            std::fs::write(&row, b"conversion source fixture").unwrap();
            let cached = root.join("conversion-cache.zip");
            write_zip(&cached);
            let metadata = std::fs::metadata(&row).unwrap();
            app.archive_cache_db
                .as_ref()
                .unwrap()
                .record(
                    &row,
                    crate::ui_helpers::mtime_secs(&metadata),
                    metadata.len() as i64,
                    crate::archive_converter::ArchiveFormat::SevenZ,
                    &cached,
                    std::fs::metadata(&cached).unwrap().len() as i64,
                    1,
                    false,
                )
                .unwrap();
            app.settings
                .set_archive_file_handling(crate::settings::ArchiveFileHandling::Convert);
            GridItem::ConvertibleArchive {
                path: row.clone(),
                format: crate::archive_converter::ArchiveFormat::SevenZ,
            }
        }
        Landing::UncachedArchive => {
            let mut bytes = std::io::Cursor::new(Vec::new());
            image::RgbImage::new(2, 2)
                .write_to(&mut bytes, image::ImageFormat::Png)
                .unwrap();
            let mut archive = sevenz_rust2::ArchiveWriter::create(&row).unwrap();
            archive
                .push_archive_entry(
                    sevenz_rust2::ArchiveEntry::new_file("001.png"),
                    Some(std::io::Cursor::new(bytes.into_inner())),
                )
                .unwrap();
            archive.finish().unwrap();
            app.settings
                .set_archive_file_handling(crate::settings::ArchiveFileHandling::Ask);
            GridItem::ConvertibleArchive {
                path: row.clone(),
                format: crate::archive_converter::ArchiveFormat::SevenZ,
            }
        }
        Landing::Folder | Landing::ImageFolder | Landing::FolderInnerZip => {
            std::fs::create_dir(&row).unwrap();
            match landing {
                Landing::FolderInnerZip => {
                    std::fs::write(row.join("audio.mp3"), b"audio").unwrap();
                    write_zip(&row.join("inner.zip"));
                }
                Landing::Folder => {
                    write_page(&row.join("001.png"));
                    // Mixed physical listing, so this is not an image-folder book.
                    std::fs::write(row.join("video.mp4"), b"video").unwrap();
                }
                _ => write_page(&row.join("001.png")),
            }
            GridItem::Folder(row.clone())
        }
    };
    let mut rows = vec![GridItem::Folder(source.clone()), target_item];
    if matches!(landing, Landing::FolderInnerZip) {
        // The inner archive is independently a result row. Opening it as the Folder's
        // first book must retain the Folder identity until a subsequent ListOrder press.
        rows.push(GridItem::ZipFile(row.join("inner.zip")));
    }
    app.current_folder = Some(root);
    let run = adopt_rows(app, rows);
    let source_index = app
        .items
        .iter()
        .position(|item| item == &GridItem::Folder(source.clone()))
        .unwrap();
    app.capture_main_folder_list_order(source_index);
    let snapshot = app.list_order.clone().unwrap();
    assert_eq!(snapshot.scope, ListOrderScope::Everything { run });
    app.load_folder(source.clone());
    app.finish_page_identity_preparation_for_test();
    assert_child(app, &source);
    app.fullscreen_idx = Some(0);
    if matches!(landing, Landing::Zip) {
        // A cancelled query advances request freshness without replacing accepted rows.
        start_everything_for_test(app, &egui::Context::default(), "cancel before navigation");
        app.cancel_everything_search();
        assert_ne!(app.global_search.run_sequence, run);
    }
    let destination = if matches!(landing, Landing::FolderInnerZip) {
        row.join("inner.zip")
    } else {
        row.clone()
    };
    LandingFixture {
        source,
        row,
        destination,
        snapshot,
        epub_pin,
        document_stamp,
    }
}

fn receive_folder_nav(app: &mut App, ctx: &egui::Context) -> FolderNavResult {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        app.poll_sidecar_restore(ctx);
        if let Some(result) = app.poll_folder_nav() {
            return result;
        }
        assert!(
            Instant::now() < deadline,
            "Everything list-order worker did not finish"
        );
        std::thread::yield_now();
    }
}

fn start_landing(app: &mut App, ctx: &egui::Context, fixture: &LandingFixture) {
    app.handle_fullscreen_list_order_nav_context(ctx, 0, true, false);
    assert!(app.folder_nav_pending.is_some());
    let result = receive_folder_nav(app, ctx);
    assert_eq!(
        result.target.as_ref().unwrap().logical_source(),
        fixture.row
    );
    app.apply_folder_nav_result(ctx, result);
    app.settle_open_path_classification_for_test();
}

fn replace_document_preflight(app: &mut App, stamp: Option<crate::pdf_loader::DocumentStamp>) {
    use collection_navigation::{PhysicalHistoryPreflight, PhysicalHistoryPreflightPayload};
    let Some(HistoryNavigationTransition::Physical(request)) =
        app.top_level_grid_view.history_navigation_transition_mut()
    else {
        panic!("document list-order landing must retain physical history owner");
    };
    let PhysicalHistoryPhase::Preflighting { preflight, .. } = &mut request.phase else {
        panic!("document must reach preflight before adoption");
    };
    *preflight = PhysicalHistoryPreflight::ready_for_test(
        PhysicalHistoryPreflightPayload::PdfPages(crate::pdf_loader::PdfEnumerateResult {
            pages: vec![crate::pdf_loader::PdfPageEntry {
                page_num: 0,
                mtime: 1,
                file_size: 1,
            }],
            direction: None,
            stamp,
        }),
    );
}

fn finish_landing(app: &mut App, ctx: &egui::Context) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.top_level_grid_view.open_path_classification().is_some()
        || app
            .top_level_grid_view
            .history_navigation_transition()
            .is_some()
        || app.zip_enumerate_pending.is_some()
        || app.pdf_enumerate_pending.is_some()
        || app.fs_nav_after_pdf_enumerate.is_some()
        || app.sidecar_restore_active()
        || app.fullscreen_idx.is_none()
    {
        app.settle_open_path_classification_for_test();
        app.poll_history_navigation_transition_for_test(ctx);
        app.poll_zip_enumerate();
        app.poll_pdf_enumerate();
        app.poll_sidecar_restore(ctx);
        app.poll_book_composition_service_for_test(ctx);
        app.poll_book_presentation_permission(ctx);
        assert!(
            Instant::now() < deadline,
            "Everything staged landing did not reopen"
        );
        std::thread::yield_now();
    }
    app.finish_page_identity_preparation_for_test();
}

fn land(app: &mut App, ctx: &egui::Context, fixture: &LandingFixture, landing: Landing) {
    start_landing(app, ctx, fixture);
    if matches!(
        landing,
        Landing::Zip | Landing::ColdPdf | Landing::WarmPdf | Landing::Epub | Landing::Converted
    ) {
        assert_child(app, &fixture.source);
        let Some(HistoryNavigationTransition::Physical(request)) =
            app.top_level_grid_view.history_navigation_transition()
        else {
            panic!("virtual ListOrder must stage a physical history transition: {landing:?}");
        };
        let Some(SearchOpenOrigin::Everything {
            run,
            child: EverythingChildAdmission::ListOrderRow { dataset_run, row },
            ..
        }) = &request.global_search_origin
        else {
            panic!("list-order row origin was lost during classification: {landing:?}");
        };
        assert_eq!(*run, app.global_search.run_sequence);
        assert_eq!(*dataset_run, accepted_run(app));
        assert_eq!(row, &fixture.row);
        if matches!(landing, Landing::Zip) {
            assert_ne!(run, dataset_run);
        }
        assert_eq!(app.effective_folder().as_ref(), Some(&fixture.source));
        if matches!(landing, Landing::ColdPdf | Landing::WarmPdf | Landing::Epub) {
            replace_document_preflight(app, fixture.document_stamp.clone());
        }
    } else {
        // The Folder install commits the row before hydration starts the inner book.
        assert_child(app, &fixture.row);
    }
    finish_landing(app, ctx);
    assert!(app.global_search.active);
    assert_eq!(app.effective_folder().as_ref(), Some(&fixture.destination));
    assert_child(app, &fixture.row);
    let expected = fixture
        .snapshot
        .entries
        .iter()
        .position(|entry| entry.path == fixture.row)
        .unwrap();
    assert_eq!(app.list_order_position().unwrap().1, expected);
    assert!(Arc::ptr_eq(
        app.list_order.as_ref().unwrap(),
        &fixture.snapshot
    ));
    if fixture.epub_pin.is_some() {
        assert!(app.display_epub_source.as_ref().unwrap().has_epub_lease());
    }
}

fn finish_results_return(app: &mut App, ctx: &egui::Context, expected: &Path) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.everything_has_physical_child() {
        app.poll_search_page_edit_prepare(ctx);
        assert!(Instant::now() < deadline, "Everything return did not adopt");
        std::thread::yield_now();
    }
    assert!(app.global_search.active);
    assert!(app.items_are_global_search_view);
    assert_eq!(
        app.items[app.selected.unwrap()].drag_source_path(),
        Some(expected)
    );
    assert!(app.grid_scroll.reveal_requested());
    assert!(app.list_order_position().is_none());
}

#[test]
fn list_order_everything_all_landing_transports_commit_row_and_results_selects_destination() {
    for landing in LANDINGS {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let fixture = landing_fixture(&mut app, landing);
        land(&mut app, &ctx, &fixture, landing);
        assert!(app.return_from_everything_child(EverythingReturn::Results));
        finish_results_return(&mut app, &ctx, &fixture.row);
    }
}

#[test]
fn list_order_everything_book_close_returns_the_moved_row_but_nested_folder_book_stays_inside() {
    for landing in LANDINGS {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let fixture = landing_fixture(&mut app, landing);
        land(&mut app, &ctx, &fixture, landing);
        app.release_fs_nav_lock();
        let returns = !matches!(landing, Landing::Folder | Landing::FolderInnerZip);
        assert_eq!(
            app.everything_book_close_return_available(),
            returns,
            "{landing:?}"
        );
        app.handle_fullscreen_close_request_immediate();
        if returns {
            finish_results_return(&mut app, &ctx, &fixture.row);
        } else {
            assert_child(&app, &fixture.row);
            assert_eq!(app.effective_folder().as_ref(), Some(&fixture.destination));
            assert!(app.global_search.pending.is_none());
        }
    }
}

#[test]
fn list_order_everything_parent_uses_the_moved_row_and_nested_book_returns_to_its_folder_first() {
    for landing in LANDINGS {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let fixture = landing_fixture(&mut app, landing);
        land(&mut app, &ctx, &fixture, landing);
        app.release_fs_nav_lock();
        app.close_fullscreen();
        assert!(app.handle_grid_parent_folder_action().is_none());
        if matches!(landing, Landing::FolderInnerZip) {
            app.finish_page_identity_preparation_for_test();
            assert_eq!(app.effective_folder().as_ref(), Some(&fixture.row));
            assert_child(&app, &fixture.row);
            assert_eq!(
                app.items[app.selected.unwrap()].drag_source_path(),
                Some(fixture.destination.as_path())
            );
            assert!(app.handle_grid_parent_folder_action().is_none());
        }
        finish_results_return(&mut app, &ctx, &fixture.row);
    }
}

#[test]
fn list_order_everything_folder_inner_archive_row_becomes_its_own_row_only_on_next_press() {
    let mut app = setup_app();
    let ctx = egui::Context::default();
    let fixture = landing_fixture(&mut app, Landing::FolderInnerZip);
    land(&mut app, &ctx, &fixture, Landing::FolderInnerZip);
    assert_child(&app, &fixture.row);
    app.release_fs_nav_lock();
    app.handle_fullscreen_list_order_nav_context(&ctx, 0, true, false);
    let result = receive_folder_nav(&mut app, &ctx);
    assert_eq!(
        result.target.as_ref().unwrap().logical_source(),
        fixture.destination
    );
    app.apply_folder_nav_result(&ctx, result);
    finish_landing(&mut app, &ctx);
    assert_child(&app, &fixture.destination);
    let index = fixture
        .snapshot
        .entries
        .iter()
        .position(|entry| entry.path == fixture.destination)
        .unwrap();
    assert_eq!(app.list_order_position().unwrap().1, index);
    app.release_fs_nav_lock();
    app.handle_fullscreen_list_order_nav_context(&ctx, 0, true, false);
    let edge = receive_folder_nav(&mut app, &ctx);
    assert!(edge.target.is_none());
    app.apply_folder_nav_result(&ctx, edge);
    assert_child(&app, &fixture.destination);
    assert!(matches!(
        app.fs_boundary_hint,
        Some(crate::ui_fullscreen::FsBoundaryHint::Edge {
            scope: crate::ui_fullscreen::FsBoundaryEdgeScope::ListOrder,
            at_end: true,
            ..
        })
    ));
    assert!(!app.fs_nav_is_locked());
}

#[test]
fn list_order_everything_cancel_scan_failure_and_stale_preflight_keep_the_original_row() {
    use collection_navigation::PhysicalHistoryPreflight;
    for failure in ["cancel", "scan", "preflight", "stale"] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let fixture = landing_fixture(&mut app, Landing::ColdPdf);
        if failure == "cancel" {
            app.handle_fullscreen_list_order_nav_context(&ctx, 0, true, false);
            assert!(app.folder_nav_pending.is_some());
            app.cancel_pending_folder_nav();
            assert!(app.poll_folder_nav().is_none());
        } else if failure == "scan" {
            app.apply_folder_nav_result(
                &ctx,
                FolderNavResult {
                    target: Some(FolderNavTarget {
                        route: FolderNavRoute::FullFeature(fixture.row.clone()),
                        smart_kind: None,
                        scanned: FolderNavScanResult::Failed("test scan failed".into()),
                    }),
                    hit_image_folder: true,
                    forward: true,
                    mode: FolderNavMode::ListOrder {
                        snapshot: fixture.snapshot.clone(),
                        from_index: 0,
                    },
                    queued_steps: 0,
                },
            );
        } else {
            start_landing(&mut app, &ctx, &fixture);
            if failure == "stale" {
                start_everything_for_test(&mut app, &ctx, "supersede staged open");
                app.cancel_everything_search();
                replace_document_preflight(&mut app, None);
            } else {
                let Some(HistoryNavigationTransition::Physical(request)) =
                    app.top_level_grid_view.history_navigation_transition_mut()
                else {
                    panic!("expected physical transition");
                };
                let PhysicalHistoryPhase::Preflighting { preflight, .. } = &mut request.phase
                else {
                    panic!("expected preflight");
                };
                let (controlled, sender) = PhysicalHistoryPreflight::controlled_for_test();
                *preflight = controlled;
                sender.send(Err("test open failed".into())).unwrap();
            }
            app.poll_history_navigation_transition_for_test(&ctx);
            assert!(
                app.top_level_grid_view
                    .history_navigation_transition()
                    .is_none()
            );
        }
        assert_child(&app, &fixture.source);
        assert!(app.global_search.active);
        assert_eq!(app.effective_folder().as_ref(), Some(&fixture.source));
        assert!(Arc::ptr_eq(
            app.list_order.as_ref().unwrap(),
            &fixture.snapshot
        ));
    }
}

/// Inspect the request transported by production; never reconstruct its origin at a modal seam.
fn assert_modal_list_order_owner(app: &App, fixture: &LandingFixture) -> u64 {
    let Some(HistoryNavigationTransition::Physical(request)) =
        app.top_level_grid_view.history_navigation_transition()
    else {
        panic!("modal list-order landing must retain its physical request");
    };
    let Some(SearchOpenOrigin::Everything {
        run,
        context,
        child: EverythingChildAdmission::ListOrderRow { dataset_run, row },
    }) = &request.global_search_origin
    else {
        panic!("modal continuation lost the transported list-order row");
    };
    assert_eq!(*run, app.global_search.run_sequence);
    assert_eq!(*context, app.viewer_context_main());
    assert_eq!(*dataset_run, accepted_run(app));
    assert_ne!(
        run, dataset_run,
        "cancelled query separates request and dataset runs"
    );
    assert_eq!(row, &fixture.row);
    assert_eq!(&request.path, &fixture.row);
    assert_child(app, &fixture.source);
    assert_eq!(app.effective_folder().as_ref(), Some(&fixture.source));
    assert!(Arc::ptr_eq(
        app.list_order.as_ref().unwrap(),
        &fixture.snapshot
    ));
    request.request_id
}

fn assert_modal_cancel_preserves_source(
    app: &App,
    fixture: &LandingFixture,
    source_rows: &[GridItem],
) {
    assert_child(app, &fixture.source);
    assert!(app.global_search.active);
    assert_eq!(app.effective_folder().as_ref(), Some(&fixture.source));
    assert_eq!(app.items.as_slice(), source_rows);
    assert_eq!(app.fullscreen_idx, Some(0));
    assert_eq!(app.list_order_position().unwrap().1, 0);
    assert!(Arc::ptr_eq(
        app.list_order.as_ref().unwrap(),
        &fixture.snapshot
    ));
    assert!(
        app.top_level_grid_view
            .history_navigation_transition()
            .is_none()
    );
}

fn finish_modal_landing_and_return(app: &mut App, ctx: &egui::Context, fixture: &LandingFixture) {
    finish_landing(app, ctx);
    assert_child(app, &fixture.row);
    assert_eq!(app.effective_folder().as_ref(), Some(&fixture.row));
    assert_eq!(app.list_order_position().unwrap().1, 1);
    assert!(Arc::ptr_eq(
        app.list_order.as_ref().unwrap(),
        &fixture.snapshot
    ));
    app.release_fs_nav_lock();
    assert!(app.return_from_everything_child(EverythingReturn::Results));
    finish_results_return(app, ctx, &fixture.row);
}

fn tick_archive_conversion_dialog(app: &mut App, ctx: &egui::Context) {
    // Headless egui rendering consumes the same completion routing as App::update.
    // No native window is opened and no desktop input is sent.
    ctx.run_pass(egui::RawInput::default(), |ctx| {
        app.show_archive_convert_dialog(ctx);
    })
    .drop_without_applying_deltas();
    if let Some(state) = &app.archive_convert {
        if let crate::ui_dialogs::archive_convert::ArchiveConvertPhase::Error { message } =
            &state.phase
        {
            panic!("list-order archive conversion failed: {message}");
        }
    }
}

#[test]
fn list_order_everything_conversion_confirmation_resumes_original_owner_or_cancels() {
    use crate::ui_dialogs::archive_convert::{ArchiveConvertCompletionPolicy, ArchiveConvertPhase};
    for cancel in [true, false] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        // A real 7z with no cache record must scan, ask, and run the converter.
        let fixture = landing_fixture(&mut app, Landing::UncachedArchive);
        let source_rows = app.items.clone();
        start_everything_for_test(&mut app, &ctx, "cancelled query before confirmation");
        app.cancel_everything_search();
        start_landing(&mut app, &ctx, &fixture);
        let request_id = assert_modal_list_order_owner(&app, &fixture);
        let deadline = Instant::now() + Duration::from_secs(30);
        loop {
            app.poll_history_navigation_transition_for_test(&ctx);
            tick_archive_conversion_dialog(&mut app, &ctx);
            assert_eq!(assert_modal_list_order_owner(&app, &fixture), request_id);
            if app
                .archive_convert
                .as_ref()
                .is_some_and(|state| matches!(state.phase, ArchiveConvertPhase::Confirm { .. }))
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "uncached archive did not reach confirmation"
            );
            std::thread::yield_now();
        }
        let state = app.archive_convert.as_ref().unwrap();
        assert_eq!(state.src_path, fixture.row);
        assert_eq!(
            state.completion,
            ArchiveConvertCompletionPolicy::StagedHistory(request_id)
        );
        assert!(!state.suppress_confirm);
        assert!(
            matches!(app.top_level_grid_view.history_navigation_transition(),
            Some(HistoryNavigationTransition::Physical(request))
                if matches!(request.phase, PhysicalHistoryPhase::ArchiveConverting))
        );
        if cancel {
            assert!(
                app.cancel_archive_convert_for_navigation("list_order_confirmation_cancel_test")
            );
            app.poll_history_navigation_transition_for_test(&ctx);
            assert!(app.archive_convert.is_none());
            assert_modal_cancel_preserves_source(&app, &fixture, &source_rows);
            continue;
        }
        // This existing test entry invokes the Confirm button's production handler.
        app.confirm_archive_convert_for_test();
        assert!(matches!(
            app.archive_convert.as_ref().map(|state| &state.phase),
            Some(ArchiveConvertPhase::Converting { .. })
        ));
        assert_eq!(assert_modal_list_order_owner(&app, &fixture), request_id);
        let mut saw_backing_preflight = false;
        let deadline = Instant::now() + Duration::from_secs(30);
        while app.archive_convert.is_some()
            || app
                .top_level_grid_view
                .history_navigation_transition()
                .is_some()
        {
            app.poll_history_navigation_transition_for_test(&ctx);
            tick_archive_conversion_dialog(&mut app, &ctx);
            if let Some(HistoryNavigationTransition::Physical(request)) =
                app.top_level_grid_view.history_navigation_transition()
            {
                saw_backing_preflight |= matches!(
                    request.phase,
                    PhysicalHistoryPhase::ArchivePreflighting { .. }
                );
                assert_eq!(assert_modal_list_order_owner(&app, &fixture), request_id);
            }
            app.poll_sidecar_restore(&ctx);
            assert!(
                Instant::now() < deadline,
                "confirmed archive did not adopt its backing"
            );
            std::thread::yield_now();
        }
        assert!(
            saw_backing_preflight,
            "dialog completion must resume the same staged owner"
        );
        assert_ne!(app.current_folder.as_ref(), Some(&fixture.row));
        assert_eq!(app.archive_source_override.as_ref(), Some(&fixture.row));
        finish_modal_landing_and_return(&mut app, &ctx, &fixture);
    }
}

#[test]
fn list_order_everything_pdf_password_wait_resumes_original_owner_or_cancels() {
    for cancel in [true, false] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let fixture = landing_fixture(&mut app, Landing::ColdPdf);
        let source_rows = app.items.clone();
        // Replace only the external PDF process response. Enqueue, preflight,
        // password-wait creation, Retry/Cancel, and final adoption remain real.
        let pool = crate::pdf_loader::TestPdfEnumeratePool::new(&fixture.row);
        start_everything_for_test(&mut app, &ctx, "cancelled query before password");
        app.cancel_everything_search();
        start_landing(&mut app, &ctx, &fixture);
        let request_id = assert_modal_list_order_owner(&app, &fixture);
        let job = pool.take();
        assert_eq!(job.priority(), crate::pdf_loader::JobPriority::Critical);
        job.fail(crate::pdf_loader::PdfReadError::PasswordRequired);
        let deadline = Instant::now() + Duration::from_secs(30);
        while app.pdf_password_dialog_path().as_ref() != Some(&fixture.row) {
            app.poll_history_navigation_transition_for_test(&ctx);
            app.poll_sidecar_restore(&ctx);
            assert_eq!(assert_modal_list_order_owner(&app, &fixture), request_id);
            assert!(
                Instant::now() < deadline,
                "PDF did not reach password input"
            );
            std::thread::yield_now();
        }
        assert!(
            matches!(app.top_level_grid_view.history_navigation_transition(),
            Some(HistoryNavigationTransition::Physical(request))
                if matches!(request.phase, PhysicalHistoryPhase::PdfPassword))
        );
        assert!(
            matches!(app.pdf_password_request.as_ref().map(|request| &request.owner),
            Some(PdfPasswordRequestOwner::StagedHistory(owner)) if *owner == request_id)
        );
        if cancel {
            assert!(app.cancel_pdf_password_dialog_request());
            assert!(app.pdf_password_dialog_path().is_none());
            assert_modal_cancel_preserves_source(&app, &fixture, &source_rows);
            continue;
        }
        assert!(app.retry_pdf_password_dialog_request("secret".into(), false));
        assert!(app.pdf_password_dialog_path().is_none());
        assert_eq!(assert_modal_list_order_owner(&app, &fixture), request_id);
        assert!(
            matches!(app.top_level_grid_view.history_navigation_transition(),
            Some(HistoryNavigationTransition::Physical(request))
                if matches!(&request.phase, PhysicalHistoryPhase::Preflighting {
                    pdf_password_submission: Some((password, false)), ..
                } if password == "secret"))
        );
        let resumed = pool.take();
        assert_eq!(resumed.priority(), crate::pdf_loader::JobPriority::Critical);
        resumed.complete(1);
        finish_landing(&mut app, &ctx);
        assert_eq!(app.pdf_current_password.as_deref(), Some("secret"));
        finish_modal_landing_and_return(&mut app, &ctx, &fixture);
    }
}

#[cfg(windows)]
struct ClearListOrderInput;

#[cfg(windows)]
impl Drop for ClearListOrderInput {
    fn drop(&mut self) {
        crate::key_input::clear_test_frame();
    }
}

fn open_book_from_grid_for_list_order_repro(app: &mut App, ctx: &egui::Context, path: &Path) {
    let index = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(path))
        .unwrap();
    // Enter/double-click first run shared container admission, which captures
    // the image-folder auto-open intent before returning a navigation value.
    app.selected = Some(index);
    let navigation = app
        .accept_grid_container_open(ctx, index)
        .expect("grid container admission");
    let mut frame = list_order_repro_navigation_frame();
    frame.grid_nav = Some(navigation);
    app.post_render_main_navigation(ctx, frame);
    app.settle_open_path_classification_for_test();
    app.finish_page_identity_preparation_for_test();
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.fullscreen_idx.is_none() || app.sidecar_restore_active() {
        app.post_render_main_navigation(ctx, list_order_repro_navigation_frame());
        app.settle_open_path_classification_for_test();
        app.poll_history_navigation_transition_for_test(ctx);
        app.poll_zip_enumerate();
        app.poll_pdf_enumerate();
        app.poll_sidecar_restore(ctx);
        app.poll_book_composition_service_for_test(ctx);
        app.poll_book_presentation_permission(ctx);
        assert!(
            Instant::now() < deadline,
            "grid book open did not finish: target={path:?}, current={:?}, fullscreen={:?}, auto_open={}, classification={}, history={}, scan={}, zip={}, pdf={}, sidecar={}, deferred_book={}, feedback={:?}",
            app.current_folder,
            app.fullscreen_idx,
            app.pending_auto_fs_open,
            app.top_level_grid_view.open_path_classification().is_some(),
            app.top_level_grid_view
                .history_navigation_transition()
                .is_some(),
            app.folder_pane_open_pending.is_some(),
            app.zip_enumerate_pending.is_some(),
            app.pdf_enumerate_pending.is_some(),
            app.sidecar_restore_active(),
            app.book_deferred_open_wait_active(),
            app.feedback_toasts_for_test
        );
        std::thread::yield_now();
    }
    assert_eq!(app.effective_folder().as_deref(), Some(path));
}

fn list_order_repro_navigation_frame() -> crate::app::update_phases::MainFrameNavigation {
    crate::app::update_phases::MainFrameNavigation {
        fav_nav: None,
        toolbar_fav_nav: None,
        fullscreen_close_nav: None,
        keyboard_nav: None,
        keyboard_nav_from_fullscreen_close: false,
        gamepad_nav: None,
        address_nav: None,
        open_folder_nav: None,
        context_nav: None,
        folder_pane_nav: None,
        grid_nav: None,
    }
}

#[cfg(windows)]
#[test]
fn list_order_everything_folder_dialog_physical_book_reports_unavailable_from_real_key_handler() {
    let _serial = crate::key_input::lock_test_input();
    let _clear = ClearListOrderInput;
    for initial_child in [false, true] {
        for zip_book in [true, false] {
            for forward in [false, true] {
                let mut app = setup_app();
                let ctx = egui::Context::default();
                app.active_quick_folder_slot = None;
                app.settings.auto_fullscreen_zip_pdf = true;
                app.settings.auto_fullscreen_image_folders = true;
                app.settings.detached_viewer_enabled = false;
                app.settings.detached_viewer_open_images_in_window = false;
                let root = app.tmp.path().to_path_buf();
                let result_book = root.join("result-book");
                let favorite = root.join("menubar-favorite");
                std::fs::create_dir_all(&favorite).unwrap();
                std::fs::create_dir_all(&result_book).unwrap();
                write_page(&result_book.join("001.png"));
                let physical_book = favorite.join(if zip_book { "a-book.zip" } else { "a-book" });
                let next_book = favorite.join(if zip_book { "b-book.zip" } else { "b-book" });
                for book in [&physical_book, &next_book] {
                    if zip_book {
                        write_zip(book);
                    } else {
                        std::fs::create_dir_all(book).unwrap();
                        write_page(&book.join("001.png"));
                    }
                }
                app.current_folder = Some(root);
                adopt_rows(&mut app, vec![GridItem::Folder(result_book.clone())]);
                if initial_child {
                    open_book_from_grid_for_list_order_repro(&mut app, &ctx, &result_book);
                    app.close_fullscreen();
                    assert_child(&app, &result_book);
                }
                // The file-open dialog still retains Search; menu favorites now
                // exit it under section 11 and cannot create this retained state.
                let mut frame = list_order_repro_navigation_frame();
                frame.open_folder_nav = Some(favorite.clone());
                app.post_render_main_navigation(&ctx, frame);
                app.settle_open_path_classification_for_test();
                app.finish_page_identity_preparation_for_test();
                assert_eq!(app.current_folder.as_ref(), Some(&favorite));
                assert!(app.global_search.active);
                assert!(!app.items_are_global_search_view);
                open_book_from_grid_for_list_order_repro(&mut app, &ctx, &physical_book);
                assert!(app.global_search.active);
                assert!(app.list_order_position().is_none());
                let before = app.effective_folder();
                let modal = app.viewer_mutation_modal_active();
                let sequence_block = app.fs_navigation_sequence_blocks_new_target();
                let locked = app.fs_nav_is_locked();
                app.fs_feedback_toast = None;
                app.feedback_toasts_for_test.clear();
                crate::key_input::set_test_frame(vec![crate::key_input::KeyEdge {
                    operation_id: crate::key_input::KeyOperationId::unassigned(),
                    source_hwnd: 1,
                    source_viewport: egui::ViewportId::ROOT,
                    virtual_key: if forward { 0x28 } else { 0x26 },
                    scan_code: 0,
                    extended: false,
                    pressed: true,
                    repeat: false,
                    ctrl: true,
                    shift: true,
                    alt: false,
                }]);
                ctx.begin_pass(egui::RawInput {
                    focused: true,
                    ..Default::default()
                });
                let fs_index = app.fullscreen_idx.unwrap();
                let action = app.handle_fs_key_input(&ctx, fs_index, false);
                assert_eq!(
                    action.list_order_nav,
                    Some(forward),
                    "key handler dropped ListOrder: initial_child={initial_child}, zip_book={zip_book}, modal={modal}, sequence_block={sequence_block}, locked={locked}"
                );
                app.handle_fs_navigation_with_list_order_operation(
                    &ctx,
                    action.close,
                    action.close_to_page_list,
                    action.ctrl_nav,
                    action.sibling_nav,
                    action.mouse_nav,
                    action.page_nav,
                    action.jump_to,
                    fs_index,
                    action.list_order_nav,
                    None,
                );
                ctx.end_pass().drop_without_applying_deltas();
                assert_eq!(app.effective_folder(), before);
                assert!(app.folder_nav_pending.is_none());
                assert_eq!(
                    app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
                    Some("一覧から開いた本で使えます"),
                    "silent ListOrder: initial_child={initial_child}, zip_book={zip_book}, modal={modal}, sequence_block={sequence_block}, locked={locked}, position=None, feedback={:?}",
                    app.feedback_toasts_for_test
                );
            }
        }
    }
}

#[cfg(windows)]
fn dispatch_list_order_repro_key(app: &mut App, ctx: &egui::Context, forward: bool) {
    let modal = app.viewer_mutation_modal_active();
    let sequence_block = app.fs_navigation_sequence_blocks_new_target();
    let locked = app.fs_nav_is_locked();
    crate::key_input::set_test_frame(vec![crate::key_input::KeyEdge {
        operation_id: crate::key_input::KeyOperationId::unassigned(),
        source_hwnd: 1,
        source_viewport: egui::ViewportId::ROOT,
        virtual_key: if forward { 0x28 } else { 0x26 },
        scan_code: 0,
        extended: false,
        pressed: true,
        repeat: false,
        ctrl: true,
        shift: true,
        alt: false,
    }]);
    ctx.begin_pass(egui::RawInput {
        focused: true,
        ..Default::default()
    });
    let fs_index = app.fullscreen_idx.unwrap();
    let action = app.handle_fs_key_input(ctx, fs_index, false);
    assert_eq!(
        action.list_order_nav,
        Some(forward),
        "real ListOrder key was not routed: modal={modal}, sequence_block={sequence_block}, locked={locked}"
    );
    app.handle_fs_navigation_with_list_order_operation(
        ctx,
        action.close,
        action.close_to_page_list,
        action.ctrl_nav,
        action.sibling_nav,
        action.mouse_nav,
        action.page_nav,
        action.jump_to,
        fs_index,
        action.list_order_nav,
        None,
    );
    ctx.end_pass().drop_without_applying_deltas();
}

#[cfg(windows)]
fn retained_parent_folder_dialog_repro(
    app: &mut tests::phase_c_support::AppTestEnv,
    ctx: &egui::Context,
    zip_book: bool,
    nested_result_rows: bool,
) -> (PathBuf, PathBuf, PathBuf, Arc<ListOrderSnapshot>) {
    app.active_quick_folder_slot = None;
    app.settings.auto_fullscreen_zip_pdf = true;
    app.settings.auto_fullscreen_image_folders = true;
    app.settings.detached_viewer_enabled = false;
    app.settings.detached_viewer_open_images_in_window = false;
    app.settings.sort_order = crate::settings::SortOrder::FileName;
    let root = app.tmp.path().to_path_buf();
    let parent = root.join("a-parent-result");
    let favorite = parent.join("favorite");
    let book = favorite.join(if zip_book { "b-book.zip" } else { "b-book" });
    let next = favorite.join(if zip_book { "c-book.zip" } else { "c-book" });
    std::fs::create_dir_all(&favorite).unwrap();
    write_page(&parent.join("001.png"));
    for path in [&book, &next] {
        if zip_book {
            write_zip(path);
        } else {
            std::fs::create_dir(path).unwrap();
            write_page(&path.join("001.png"));
        }
    }
    app.current_folder = Some(root);
    let mut rows = vec![GridItem::Folder(parent.clone())];
    if nested_result_rows {
        for path in [&book, &next] {
            rows.push(if zip_book {
                GridItem::ZipFile(path.clone())
            } else {
                GridItem::Folder(path.clone())
            });
        }
    }
    adopt_rows(app, rows);
    let index = app
        .items
        .iter()
        .position(|item| item.drag_source_path() == Some(parent.as_path()))
        .unwrap();
    let navigation = app.accept_grid_container_open(ctx, index).unwrap();
    let mut frame = list_order_repro_navigation_frame();
    frame.grid_nav = Some(navigation);
    app.post_render_main_navigation(ctx, frame);
    let deadline = Instant::now() + Duration::from_secs(30);
    while app.current_folder.as_ref() != Some(&parent)
        || app.folder_pane_open_pending.is_some()
        || app.sidecar_restore_active()
    {
        app.post_render_main_navigation(ctx, list_order_repro_navigation_frame());
        app.settle_open_path_classification_for_test();
        app.poll_sidecar_restore(ctx);
        assert!(
            Instant::now() < deadline,
            "parent result row did not open its mixed folder list"
        );
        std::thread::yield_now();
    }
    app.finish_page_identity_preparation_for_test();
    assert_child(app, &parent);
    assert!(
        app.fullscreen_idx.is_none(),
        "a mixed folder is not an image-folder book"
    );
    let snapshot = app.list_order.clone().unwrap();
    assert_eq!(snapshot.entries[0].path, parent);
    let mut frame = list_order_repro_navigation_frame();
    frame.open_folder_nav = Some(favorite.clone());
    app.post_render_main_navigation(ctx, frame);
    app.settle_open_path_classification_for_test();
    app.finish_page_identity_preparation_for_test();
    assert_eq!(app.current_folder.as_ref(), Some(&favorite));
    assert!(app.global_search.active);
    open_book_from_grid_for_list_order_repro(app, ctx, &book);
    assert_child(app, &parent);
    assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
    assert_eq!(app.list_order_position().unwrap().1, 0);
    assert!(!app.viewer_mutation_modal_active());
    assert!(
        !app.fs_navigation_sequence_blocks_new_target(),
        "initial book is ready, not an unpumped folder-navigation sequence"
    );
    assert!(
        !app.fs_nav_is_locked(),
        "initial book has no unresolved display-wait lock"
    );
    (parent, book, next, snapshot)
}

#[cfg(windows)]
#[test]
fn list_order_everything_retained_parent_folder_dialog_book_reaches_boundary_notice_policy() {
    let _serial = crate::key_input::lock_test_input();
    let _clear = ClearListOrderInput;
    for zip_book in [true, false] {
        for notice_visible in [true, false] {
            for forward in [false, true] {
                let mut app = setup_app();
                let ctx = egui::Context::default();
                let (parent, book, _, snapshot) =
                    retained_parent_folder_dialog_repro(&mut app, &ctx, zip_book, false);
                app.settings.fullscreen_boundary_notice_visible = notice_visible;
                app.fs_feedback_toast = None;
                app.feedback_toasts_for_test.clear();
                dispatch_list_order_repro_key(&mut app, &ctx, forward);
                assert!(
                    app.folder_nav_pending.is_some(),
                    "Some(parent row) must dispatch the existing frozen order"
                );
                let result = receive_folder_nav(&mut app, &ctx);
                assert!(
                    result.target.is_none(),
                    "there is only one captured result row"
                );
                app.apply_folder_nav_result(&ctx, result);
                assert_eq!(app.effective_folder().as_ref(), Some(&book));
                assert_child(&app, &parent);
                assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
                assert!(!app.fs_nav_is_locked());
                assert!(
                    app.feedback_toasts_for_test.is_empty(),
                    "the boundary uses its central hint rather than the unavailable warning"
                );
                let hint = app
                    .fs_boundary_hint
                    .expect("worker exhaustion must publish an edge hint");
                assert!(matches!(hint, crate::ui_fullscreen::FsBoundaryHint::Edge {
                    scope: crate::ui_fullscreen::FsBoundaryEdgeScope::ListOrder, at_end, ..
                } if at_end == forward));
                // This is the same visibility predicate used by draw_boundary_hint.
                // OFF intentionally hides the edge; it is not an admission failure.
                assert_eq!(
                    crate::ui_fullscreen::fullscreen_boundary_hint_visible(
                        app.settings.fullscreen_boundary_notice_visible,
                        hint
                    ),
                    notice_visible
                );
            }
        }
    }
}

#[cfg(windows)]
#[test]
fn list_order_everything_retained_parent_folder_dialog_current_book_reports_its_nested_result_row()
{
    let _serial = crate::key_input::lock_test_input();
    let _clear = ClearListOrderInput;
    for zip_book in [true, false] {
        let mut app = setup_app();
        let ctx = egui::Context::default();
        let (_, book, _, snapshot) =
            retained_parent_folder_dialog_repro(&mut app, &ctx, zip_book, true);
        app.feedback_toasts_for_test.clear();
        dispatch_list_order_repro_key(&mut app, &ctx, true);
        let result = receive_folder_nav(&mut app, &ctx);
        assert_eq!(result.target.as_ref().unwrap().logical_source(), book);
        app.apply_folder_nav_result(&ctx, result);
        finish_landing(&mut app, &ctx);
        // The row advances A -> B although B was already being read under A.
        // The unchanged pixels/path therefore do not imply a dropped operation.
        assert_eq!(app.effective_folder().as_ref(), Some(&book));
        assert_child(&app, &book);
        assert_eq!(app.list_order_position().unwrap().1, 1);
        assert!(Arc::ptr_eq(app.list_order.as_ref().unwrap(), &snapshot));
        assert_eq!(
            app.fs_feedback_toast.as_ref().map(|toast| toast.0.as_str()),
            Some("一覧の移動先は現在表示中の本です"),
            "adopting a different logical row for the already displayed book must explain the unchanged view"
        );
        assert!(
            !app.feedback_toasts_for_test
                .iter()
                .any(|(text, _)| text == "一覧から開いた本で使えます")
        );
    }
}

#[cfg(windows)]
#[test]
fn list_order_regular_folder_repro_fixture_navigates_from_the_real_book_key_handler() {
    let _serial = crate::key_input::lock_test_input();
    let _clear = ClearListOrderInput;
    for initial_child in [false, true] {
        for zip_book in [true, false] {
            let mut app = setup_app();
            let ctx = egui::Context::default();
            app.active_quick_folder_slot = None;
            app.settings.auto_fullscreen_zip_pdf = true;
            app.settings.auto_fullscreen_image_folders = true;
            app.settings.detached_viewer_enabled = false;
            app.settings.detached_viewer_open_images_in_window = false;
            app.settings.sort_order = crate::settings::SortOrder::FileName;
            let folder = app.tmp.path().join("ordinary-list");
            std::fs::create_dir(&folder).unwrap();
            let book = folder.join(if zip_book { "a-book.zip" } else { "a-book" });
            let next = folder.join(if zip_book { "b-book.zip" } else { "b-book" });
            for path in [&book, &next] {
                if zip_book {
                    write_zip(path);
                } else {
                    std::fs::create_dir(path).unwrap();
                    write_page(&path.join("001.png"));
                }
            }
            let origin = app.tmp.path().to_path_buf();
            let result_book = origin.join("search-source-book");
            std::fs::create_dir(&result_book).unwrap();
            write_page(&result_book.join("001.png"));
            app.current_folder = Some(origin);
            adopt_rows(&mut app, vec![GridItem::Folder(result_book.clone())]);
            if initial_child {
                open_book_from_grid_for_list_order_repro(&mut app, &ctx, &result_book);
                app.close_fullscreen();
                assert_child(&app, &result_book);
            }
            let mut address = list_order_repro_navigation_frame();
            address.address_nav = Some(crate::ui_main::AddressBarNav::AddressInput(
                crate::ui_main::LocationJumpTarget::Physical(folder.clone()),
            ));
            app.post_render_main_navigation(&ctx, address);
            let deadline = Instant::now() + Duration::from_secs(30);
            while app.current_folder.as_ref() != Some(&folder)
                || app.folder_pane_open_pending.is_some()
                || app.sidecar_restore_active()
            {
                app.post_render_main_navigation(&ctx, list_order_repro_navigation_frame());
                app.settle_open_path_classification_for_test();
                app.poll_sidecar_restore(&ctx);
                assert!(
                    Instant::now() < deadline,
                    "address search exit did not adopt its physical folder"
                );
                std::thread::yield_now();
            }
            assert!(!app.global_search.active && !app.favsearch.active && !app.tag_view.active);
            assert_eq!(app.current_list_order_scope(), Some(ListOrderScope::Folder));
            app.finish_page_identity_preparation_for_test();
            open_book_from_grid_for_list_order_repro(&mut app, &ctx, &book);
            assert_eq!(
                app.list_order.as_ref().unwrap().scope,
                ListOrderScope::Folder
            );
            assert_eq!(app.list_order_position().unwrap().1, 0);
            assert!(!app.fs_navigation_sequence_blocks_new_target());
            assert!(
                !app.fs_nav_is_locked(),
                "same ordinary fixture has no test-only display lock"
            );
            dispatch_list_order_repro_key(&mut app, &ctx, true);
            let result = receive_folder_nav(&mut app, &ctx);
            assert_eq!(result.target.as_ref().unwrap().logical_source(), next);
            app.apply_folder_nav_result(&ctx, result);
            finish_landing(&mut app, &ctx);
            assert_eq!(app.effective_folder().as_ref(), Some(&next));
            assert_eq!(app.list_order_position().unwrap().1, 1);
        }
    }
}

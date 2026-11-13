//! Headless dialog -> persisted pin -> ordinary tile request -> worker -> draw angle.
//! Only PDF rasterization is injected, using the worker's existing render fixture.
use super::*;
use crate::cover_identity::CoverIdentity;
use crate::cover_source_scope::LoadedCoverIdentity;
use crate::grid_item::ThumbnailState;
use crate::thumb_loader::{LoadRequest, LoadSourcePolicy, ThumbMsg};
use egui_kittest::kittest::Queryable;
use image::{DynamicImage, Rgb, RgbImage};
use std::io::{Cursor, Write};
use std::sync::{Mutex, RwLock, atomic::AtomicUsize};

const SOURCE_RECT: [u32; 4] = [250_000, 125_000, 750_000, 625_000];

fn pattern() -> DynamicImage {
    DynamicImage::ImageRgb8(RgbImage::from_fn(48, 32, |x, y| {
        Rgb([(x * 5) as u8, (y * 7) as u8, (x + y * 3) as u8])
    }))
}

fn zip_leaf(path: &Path, entry: &str, bytes: &[u8]) {
    let mut zip = zip::ZipWriter::new(std::fs::File::create(path).unwrap());
    zip.start_file(entry, zip::write::SimpleFileOptions::default())
        .unwrap();
    zip.write_all(bytes).unwrap();
    zip.finish().unwrap();
}

fn pdf_pages(path: &Path) {
    // Real page enumeration validates page 2 before the raster fixture is used.
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R 4 0 R 5 0 R] /Count 3 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 480 320] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 480 320] >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 480 320] >>",
    ];
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::new();
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        bytes.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
    }
    let xref = bytes.len();
    bytes.extend_from_slice(b"xref\n0 6\n0000000000 65535 f \n");
    for offset in offsets {
        bytes.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
    }
    bytes.extend_from_slice(
        format!("trailer\n<< /Root 1 0 R /Size 6 >>\nstartxref\n{xref}\n%%EOF\n").as_bytes(),
    );
    std::fs::write(path, bytes).unwrap();
}

fn worker(
    request: &mut LoadRequest,
    catalog: &Arc<crate::catalog::CatalogDb>,
    pins: &crate::folder_thumb_pins::FolderThumbPinDb,
) -> Vec<ThumbMsg> {
    let memory = RwLock::new(catalog.load_all().unwrap());
    let (tx, rx) = mpsc::channel();
    let done = Arc::new(AtomicUsize::new(0));
    crate::thumb_loader::process_load_request(
        request,
        &memory,
        &tx,
        Some(catalog),
        64,
        90,
        64,
        crate::thumb_loader::CacheDecision {
            policy: crate::settings::CachePolicy::Always,
            threshold_ms: 0,
            size_threshold: 0,
            webp_always: true,
            pdf_always: true,
            zip_always: true,
            cache_book_cover_only: false,
        },
        &done,
        &Arc::new(Mutex::new(crate::stats::ThumbStats::default())),
        None,
        &Arc::new(AtomicUsize::new(0)),
        &Arc::new(AtomicUsize::new(1)),
        None,
        Some(pins),
        None,
        None,
        None,
    );
    assert_eq!(done.load(Ordering::Relaxed), 1);
    rx.try_iter().collect()
}

fn check_screen_crop(rotation: Rotation, displayed: [u32; 4], pdf: bool, jpeg: bool) {
    let mut app = crate::app::setup_app_for_test();
    let path = app
        .tmp
        .path()
        .join(if pdf { "book.pdf" } else { "book.zip" });
    let entry = if jpeg { "cover.jpg" } else { "cover.png" };
    let mut upright = pattern();
    if pdf {
        // The existing render hook supplies the known page raster below.
        pdf_pages(&path);
    } else {
        let mut bytes = Cursor::new(Vec::new());
        pattern()
            .write_to(
                &mut bytes,
                if jpeg {
                    image::ImageFormat::Jpeg
                } else {
                    image::ImageFormat::Png
                },
            )
            .unwrap();
        let mut bytes = bytes.into_inner();
        if jpeg {
            // A minimal little-endian EXIF IFD with Orientation=6. The expected
            // pixels rotate the raw JPEG independently of the EXIF dispatch.
            let payload =
                b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
            let length = (payload.len() + 2) as u16;
            bytes.splice(
                2..2,
                [
                    vec![0xff, 0xe1],
                    length.to_be_bytes().to_vec(),
                    payload.to_vec(),
                ]
                .concat(),
            );
            let (raw, stats) =
                crate::thumb_loader::decode_jpeg_turbo_scaled_from_bytes(&bytes, 64).unwrap();
            assert_eq!((stats.src_w, stats.src_h), (48, 32));
            upright = raw.rotate90();
        }
        zip_leaf(&path, entry, &bytes);
    }
    let _pdf_backend = pdf.then(|| crate::pdf_loader::RemotePdfTestBackend::for_path(&path));
    let source = if pdf {
        FolderPinSource::PdfPage {
            pdf_rel: String::new(),
            page: 2,
        }
    } else {
        FolderPinSource::ZipEntry {
            zip_rel: String::new(),
            entry: entry.into(),
        }
    };
    let page_key = if pdf {
        crate::edit_source::page_key_for_pdf(&path, 2)
    } else {
        crate::adjustment_db::zip_entry_key(&path, entry)
    };
    app.current_folder = Some(app.tmp.path().to_owned());
    app.items = vec![
        if pdf {
            GridItem::PdfFile(path.clone())
        } else {
            GridItem::ZipFile(path.clone())
        },
        if pdf {
            GridItem::PdfPage {
                pdf_path: path.clone(),
                page_num: 2,
                content_type: None,
            }
        } else {
            GridItem::ZipImage {
                zip_path: path.clone(),
                entry_name: entry.into(),
            }
        },
    ];
    app.thumbnails = vec![ThumbnailState::Pending, ThumbnailState::Pending];
    app.image_metas = vec![None, None];
    app.folder_pin_map.clear();
    app.folder_thumb_pin_db = Some(Arc::new(
        crate::folder_thumb_pins::FolderThumbPinDb::open_at(&app.tmp.path().join("pins.db"))
            .unwrap(),
    ));
    app.rotation_db = Some(
        crate::rotation_db::RotationDb::open_at(&app.tmp.path().join("rotations.db")).unwrap(),
    );
    app.set_image_rotation(1, rotation).unwrap();
    assert_eq!(app.rotation_for_page_key(&page_key), rotation);
    let container = BookCoverContainer {
        path: path.clone(),
        kind: if pdf { BookKind::Pdf } else { BookKind::Zip },
        zip_dir_prefix: None,
    };
    let env = app.book_cover_env();
    let prepared = if pdf {
        let BookCover::Readable(cover) =
            resolve_logical_book_cover(&container, Some(source.clone().into()), &env)
        else {
            panic!("PDF page resolution");
        };
        PreparedCover {
            source: cover.persistent_source,
            rotation_key: cover.rotation_key,
            crop: None,
            auto_crop: None,
            auto_enabled: false,
            image: egui::ColorImage::from_rgb([48, 32], pattern().to_rgb8().as_raw()),
        }
    } else {
        prepare_cover(
            &container,
            Some(source.clone()),
            &env,
            64,
            &Arc::new(AtomicBool::new(false)),
        )
        .unwrap()
    };
    drop(env);
    assert_eq!(prepared.rotation_key.as_deref(), Some(page_key.as_str()));
    assert_eq!(
        prepared.image.size,
        [upright.width() as usize, upright.height() as usize]
    );
    let catalog = Arc::new(
        crate::catalog::CatalogDb::open(&app.tmp.path().join("cache"), app.tmp.path()).unwrap(),
    );
    let (tx, rx) = mpsc::channel();
    app.cover_crop_dialog = Some(CoverCropDialog {
        generation: 1,
        container: path.clone(),
        cancel: Arc::new(AtomicBool::new(false)),
        phase: Phase::Loading(rx),
    });
    let mut h = super::tests::app_harness(app);
    tx.send((1, Ok(prepared))).unwrap();
    h.run_steps(3);
    let Phase::Ready(draft) = &h.state().cover_crop_dialog.as_ref().unwrap().phase else {
        panic!("ready crop dialog");
    };
    assert_eq!(draft.rotation, rotation);
    // Fixed screen-space expectations, not produced by rotate_rect itself.
    // Drag through the production canvas before accepting the actual dialog.
    let (canvas, body) = h
        .ctx
        .data(|data| {
            data.get_temp::<(egui::Rect, egui::Rect)>(egui::Id::new("cover-crop-canvas-geometry"))
        })
        .unwrap();
    assert!(body.contains_rect(canvas));
    let image_rect = canvas.shrink2(CANVAS_PADDING);
    let point = |x, y| {
        egui::pos2(
            image_rect.left() + image_rect.width() * x as f32 / UNIT as f32,
            image_rect.top() + image_rect.height() * y as f32 / UNIT as f32,
        )
    };
    let start = point(displayed[0], displayed[1]);
    let end = point(displayed[2], displayed[3]);
    h.event(egui::Event::PointerMoved(start));
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: start,
        button: egui::PointerButton::Primary,
        pressed: true,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(1);
    h.event(egui::Event::PointerMoved(end));
    h.run_steps(1);
    h.event(egui::Event::PointerButton {
        pos: end,
        button: egui::PointerButton::Primary,
        pressed: false,
        modifiers: egui::Modifiers::NONE,
    });
    h.run_steps(1);
    h.get_by_label(OK_LABEL).click();
    h.run_steps(2);
    assert!(!h.state().cover_crop_dialog_open());
    let pins = h.state().folder_thumb_pin_db.as_ref().unwrap().clone();
    let saved = pins.lookup(&path).unwrap();
    assert_eq!(
        saved,
        FolderPin {
            source,
            cover_crop: CoverCrop::new(SOURCE_RECT).unwrap()
        }
    );
    let mut request = h.state().cover_crop_tile_request_for_test(0);
    let key = crate::thumb_loader::cache_key_for_request(&request)
        .unwrap()
        .into_owned();
    assert_eq!(request.cover_crop, saved.cover_crop);
    if pdf {
        assert_eq!(request.pdf_page, Some(2));
    } else {
        assert_eq!(request.zip_entry.as_deref(), Some(entry));
    }
    request.force_cache = true;
    request.source_policy = LoadSourcePolicy::SourceOnly;
    let mut run = || worker(&mut request, &catalog, &pins);
    let messages = if pdf {
        crate::thumb_loader::with_test_pdf_render_result(
            crate::pdf_loader::RenderResult {
                image: pattern(),
                content_type: crate::pdf_loader::PdfPageContentType::Vector,
                page_count: 3,
                page_size_points: crate::pdf_loader::PdfPageSizePoints {
                    width: 480.0,
                    height: 320.0,
                },
            },
            run,
        )
    } else {
        run()
    };
    let pixels = messages
        .iter()
        .filter_map(ThumbMsg::pixels)
        .last()
        .expect("worker output");
    let expected = upright
        .crop_imm(
            upright.width() / 4,
            upright.height() / 8,
            upright.width() / 2,
            upright.height() / 2,
        )
        .to_rgb8();
    let expected = egui::ColorImage::from_rgb(
        [expected.width() as usize, expected.height() as usize],
        expected.as_raw(),
    );
    assert_eq!(
        pixels.image, expected,
        "worker crops before applying saved page rotation"
    );
    assert_eq!(pixels.cover, CoverIdentity::Page(page_key.clone()));
    assert_eq!(
        pixels.source_dims,
        Some((expected.size[0] as u32, expected.size[1] as u32))
    );
    let row = catalog.load_one(&key).unwrap().unwrap();
    assert_eq!(row.cover, pixels.cover);
    assert_eq!(row.source_dims, pixels.source_dims);
    let texture = h.ctx.load_texture(
        "cropped-cover-worker",
        pixels.image.clone(),
        Default::default(),
    );
    let texture_id = texture.id();
    let mut thumbnail = ThumbnailState::loaded_raster(
        texture,
        pixels.origin,
        pixels.from_edit_preview,
        64,
        pixels.source_coverage,
        pixels.source_dims,
        pixels.layout_dims,
    );
    if let ThumbnailState::Loaded { cover, .. } = &mut thumbnail {
        *cover = LoadedCoverIdentity::from_material(pixels.cover.clone(), None);
    }
    h.state_mut().thumbnails[0] = thumbnail;
    assert_eq!(h.state_mut().grid_tile_rotation(0), rotation);
    assert_eq!(h.state().grid_tile_rotation_memory(0), rotation);
    let revision = catalog.revision_state();
    // The actual page-save path changes the owner; the same cropped material
    // and cache row survive all subsequent angles, including clearing rotation.
    for changed in [
        Rotation::Cw90,
        Rotation::Cw180,
        Rotation::Cw270,
        Rotation::None,
    ] {
        h.state_mut().set_image_rotation(1, changed).unwrap();
        assert_eq!(
            h.state()
                .rotation_db
                .as_ref()
                .unwrap()
                .get_key(&page_key)
                .unwrap_or_default(),
            changed
        );
        assert_eq!(pins.lookup(&path), Some(saved.clone()));
        let next = h.state().cover_crop_tile_request_for_test(0);
        assert_eq!(next.cover_crop, saved.cover_crop);
        assert_eq!(
            crate::thumb_loader::cache_key_for_request(&next).as_deref(),
            Some(key.as_str())
        );
        assert_eq!(catalog.revision_state(), revision);
        let unchanged = catalog.load_one(&key).unwrap().unwrap();
        assert_eq!(unchanged.jpeg_data, row.jpeg_data);
        assert_eq!(unchanged.cover, row.cover);
        assert_eq!(unchanged.source_dims, row.source_dims);
        assert_eq!(unchanged.layout_dims, row.layout_dims);
        assert_eq!(
            (unchanged.mtime, unchanged.file_size),
            (row.mtime, row.file_size)
        );
        assert_eq!(catalog.load_all().unwrap().len(), 1);
        assert!(
            matches!(&h.state().thumbnails[0], ThumbnailState::Loaded { tex, .. } if tex.id() == texture_id)
        );
        assert_eq!(h.state_mut().grid_tile_rotation(0), changed);
        assert_eq!(h.state().grid_tile_rotation_memory(0), changed);
    }
    // CacheOnly cannot re-cut or bake in the new rotation, even offline.
    std::fs::remove_file(&path).unwrap();
    request.source_policy = LoadSourcePolicy::CacheOnly;
    let cached = worker(&mut request, &catalog, &pins);
    assert!(
        cached
            .iter()
            .filter_map(ThumbMsg::pixels)
            .any(|pixels| pixels.cover == row.cover && pixels.source_dims == row.source_dims)
    );
    assert_eq!(catalog.revision_state(), revision);
    // AppTestEnv owns the TempDir: close these extra handles before its drop.
    drop(pins);
    drop(catalog);
}

#[test]
fn cover_crop_saved_rotation_zip_screen_to_pin_worker_and_draw_angle() {
    for (rotation, displayed) in [
        (Rotation::Cw90, [375_000, 250_000, 875_000, 750_000]),
        (Rotation::Cw180, [250_000, 375_000, 750_000, 875_000]),
        (Rotation::Cw270, [125_000, 250_000, 625_000, 750_000]),
    ] {
        check_screen_crop(rotation, displayed, false, false);
    }
}

#[test]
fn cover_crop_saved_rotation_pdf_screen_to_pin_worker_and_draw_angle() {
    for (rotation, displayed) in [
        (Rotation::Cw90, [375_000, 250_000, 875_000, 750_000]),
        (Rotation::Cw180, [250_000, 375_000, 750_000, 875_000]),
        (Rotation::Cw270, [125_000, 250_000, 625_000, 750_000]),
    ] {
        check_screen_crop(rotation, displayed, true, false);
    }
}

#[test]
fn cover_crop_exif6_jpeg_zip_leaf_screen_saves_and_crops_upright_coordinates() {
    check_screen_crop(Rotation::None, SOURCE_RECT, false, true);
}

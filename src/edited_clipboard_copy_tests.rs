use super::*;
use crate::bake_stage::BakeStage;
use crate::export_crop::{CropAspectMode, CropRect, CropSettings};
use crate::grid_item::GridItem;
use std::path::{Path, PathBuf};

fn single(pages: &EditedClipboardPages) -> &EditedClipboardPage {
    match pages {
        EditedClipboardPages::Single(page) => page,
        EditedClipboardPages::Spread { .. } => panic!("expected one page"),
    }
}

fn file_path(page: &EditedClipboardPage) -> &Path {
    match &page.source {
        CompositeSource::File { path } => path,
        source => panic!("expected file source, got {source:?}"),
    }
}

fn render(pages: &EditedClipboardPages) -> egui::ColorImage {
    match render_pages(pages, &Arc::new(AtomicBool::new(false)), None) {
        Ok(image) => image,
        Err(_) => panic!("clipboard rendering failed"),
    }
}

fn install_image(app: &mut App, path: PathBuf) {
    app.install_new_items(vec![GridItem::Image(path)], vec![Some((8, 6))]);
    app.settings.bake_stage_clipboard = BakeStage::Edits;
}

fn crop(source_size: Option<[usize; 2]>, max_x: f32) -> CropSettings {
    CropSettings {
        rect: CropRect {
            min_x: 1.0,
            min_y: 1.0,
            max_x,
            max_y: 5.0,
        },
        aspect_mode: CropAspectMode::Free,
        source_size,
    }
}

fn saved_crop(app: &mut App, settings: CropSettings) -> String {
    let key = app.page_path_key(0).unwrap();
    app.export_crop_db
        .as_ref()
        .unwrap()
        .set(&key, settings)
        .unwrap();
    app.export_crop_page_settings.insert(0, settings);
    app.export_crop_pages.insert(0);
    app.export_crop_page_keys.insert(key.clone());
    key
}

#[test]
fn edited_clipboard_grid_snapshots_only_the_requested_image_zip_or_pdf_page() {
    let mut app = crate::app::setup_app_for_test();
    let image = app.tmp.path().join("one.png");
    let zip = app.tmp.path().join("book.zip");
    let pdf = app.tmp.path().join("book.pdf");
    app.install_new_items(
        vec![
            GridItem::Image(image.clone()),
            GridItem::ZipImage {
                zip_path: zip.clone(),
                entry_name: "pages/two.png".into(),
            },
            GridItem::PdfPage {
                pdf_path: pdf.clone(),
                page_num: 7,
                content_type: None,
            },
        ],
        vec![Some((8, 6)); 3],
    );
    app.settings.bake_stage_clipboard = BakeStage::Ai;
    app.pdf_current_password = Some("current-password".into());
    app.selected = Some(0);
    app.checked.extend([0, 1, 2]);
    app.fullscreen_idx = Some(0);
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    for idx in 0..3 {
        let pages = app.prepare_edited_clipboard_pages(idx, false).unwrap();
        let page = single(&pages);
        assert_eq!(page.edits.stage, BakeStage::Ai);
        assert!(page.edits.crop_legacy_writeback.is_none());
        match (&page.source, idx) {
            (CompositeSource::File { path }, 0) => assert_eq!(path, &image),
            (
                CompositeSource::ZipEntry {
                    zip_path,
                    entry_name,
                },
                1,
            ) => {
                assert_eq!(zip_path, &zip);
                assert_eq!(entry_name, "pages/two.png");
            }
            (
                CompositeSource::PdfPage {
                    pdf_path,
                    page_num,
                    password,
                },
                2,
            ) => {
                assert_eq!(pdf_path, &pdf);
                assert_eq!(*page_num, 7);
                assert_eq!(password.as_deref(), Some("current-password"));
            }
            _ => panic!("requested page was replaced: {idx}"),
        }
    }
    assert_eq!(app.selected, Some(0));
    assert_eq!(app.checked.len(), 3);
}

#[test]
fn edited_clipboard_producer_rejects_non_page_kinds_and_out_of_range_indices() {
    let mut app = crate::app::setup_app_for_test();
    let path = app.tmp.path().join("not-a-page");
    app.install_new_items(
        vec![
            GridItem::Folder(path.clone()),
            GridItem::ZipFile(path.clone()),
            GridItem::PdfFile(path.clone()),
            GridItem::Video(path.clone()),
            GridItem::Audio(path.clone()),
            GridItem::Stack {
                key: "stack".into(),
                representative: path.clone(),
                count: 2,
            },
            GridItem::ConvertibleArchive {
                path: path.clone(),
                format: crate::archive_converter::ArchiveFormat::Rar,
            },
            GridItem::ZipDir {
                zip_path: path.clone(),
                dir_prefix: "chapter/".into(),
                is_archive: false,
                representative: None,
            },
            GridItem::SearchContainer {
                path: path.clone(),
                kind: crate::grid_item::SearchContainerKind::Folder,
                hit_count: 1,
                representative: None,
            },
            GridItem::CollectionPlaceholder {
                path,
                last_known_kind: crate::collection_store::CollectionResolvedKind::Image,
                reason: crate::grid_item::CollectionPlaceholderReason::Missing,
            },
        ],
        vec![None; 10],
    );
    for idx in 0..=10 {
        assert!(
            app.prepare_edited_clipboard_pages(idx, false).is_err(),
            "idx {idx}"
        );
    }
}

fn displayed_page(idx: usize) -> crate::displayed_image_transform::DisplayedImageTransform {
    use crate::displayed_image_transform::*;
    DisplayedImageTransform::resolve(DisplayedImageTransformInput {
        page_idx: idx,
        viewport_rect: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 600.0)),
        source_size: egui::vec2(8.0, 6.0),
        texture_size: egui::vec2(8.0, 6.0),
        rotation: crate::rotation_db::Rotation::None,
        free_rotation_rad: 0.0,
        content_bbox: None,
        fit_mode: crate::fullscreen_specified_scale::FullscreenFitInput::legacy(
            crate::settings::FullscreenFitMode::Page,
        ),
        fit_scale_limits: FullscreenFitScaleLimits::default(),
        pixels_per_point: 1.0,
        placement: ResolvedDisplayPlacement::Normal { zoom_pan: None },
        pixel_fit: RectPixelFit::Texels,
    })
    .unwrap()
}

#[test]
fn edited_clipboard_fullscreen_uses_the_visible_spread_and_grid_keeps_one_page() {
    use crate::displayed_image_transform::FullscreenPageLayoutKind;
    let mut app = crate::app::setup_app_for_test();
    let paths: Vec<_> = (0..3)
        .map(|n| app.tmp.path().join(format!("{n}.png")))
        .collect();
    app.install_new_items(
        paths.iter().cloned().map(GridItem::Image).collect(),
        vec![Some((8, 6)); 3],
    );
    app.settings.bake_stage_clipboard = BakeStage::Edits;
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    app.fullscreen_idx = Some(2);
    app.fullscreen_page_layout
        .begin(FullscreenPageLayoutKind::Spread);
    app.fullscreen_page_layout.push(displayed_page(2));
    app.fullscreen_page_layout.push(displayed_page(0));
    let expected = app.resolve_visible_spread_pair(2);
    let pages = app.prepare_edited_clipboard_pages(2, true).unwrap();
    let EditedClipboardPages::Spread { left, right } = &pages else {
        panic!("visible spread lost")
    };
    let crate::ui_fullscreen::SpreadPair::Double {
        left: left_idx,
        right: right_idx,
    } = expected
    else {
        panic!("visible spread fixture")
    };
    assert_eq!(file_path(left), paths[left_idx]);
    assert_eq!(file_path(right), paths[right_idx]);
    assert_eq!(
        file_path(single(
            &app.prepare_edited_clipboard_pages(2, false).unwrap()
        )),
        paths[2]
    );
}

#[test]
fn edited_clipboard_fullscreen_edit_gates_do_not_reject_the_grid_request() {
    let mut app = crate::app::setup_app_for_test();
    let path = app.tmp.path().join("page.png");
    install_image(&mut app, path);
    for mode in 0..3 {
        app.erase_mode = mode == 0;
        app.adjustment_mode = if mode == 1 {
            crate::ui_helpers::MetadataPanelOpenState::ByPointer
        } else {
            crate::ui_helpers::MetadataPanelOpenState::Closed
        };
        app.analysis_mode = mode == 2;
        assert!(app.prepare_edited_clipboard_pages(0, true).is_err());
        assert!(app.prepare_edited_clipboard_pages(0, false).is_ok());
    }
}

fn pending_erase(app: &mut App, idx: usize, kind: crate::ui_erase::EraseInpaintKind) {
    use crate::ui_erase::*;
    let (_, rx) = mpsc::channel();
    let (_, progress_rx) = mpsc::channel();
    app.erase_inpaint_pending.insert(
        EraseInpaintPendingKey { idx, kind },
        EraseInpaintPending {
            display_basis: crate::material_display_basis::MaterialDisplayBasis::unavailable(
                crate::material_display_basis::MaterialBasisUnavailable::ProducerUnavailable,
            ),
            idx,
            items_generation: app.items_generation,
            path_key: app.page_path_key(idx),
            rx,
            progress_rx,
            progress: EraseInpaintProgress::Preparing,
            cancel: Arc::new(AtomicBool::new(false)),
            started_at: std::time::Instant::now(),
            input_generation: 0,
            mask_generation: 0,
            purpose: EraseInpaintPurpose::Commit {
                origin: "edited-clipboard-producer-test",
            },
        },
    );
}

#[test]
fn edited_clipboard_blocks_commit_on_either_spread_page_but_not_an_unrelated_page_or_preview() {
    use crate::displayed_image_transform::FullscreenPageLayoutKind;
    use crate::ui_erase::EraseInpaintKind;
    let mut app = crate::app::setup_app_for_test();
    let paths: Vec<_> = (0..3)
        .map(|n| app.tmp.path().join(format!("{n}.png")))
        .collect();
    app.install_new_items(
        paths.into_iter().map(GridItem::Image).collect(),
        vec![Some((8, 6)); 3],
    );
    app.settings.bake_stage_clipboard = BakeStage::Edits;
    app.spread_mode = crate::settings::SpreadMode::Ltr;
    app.fullscreen_page_layout
        .begin(FullscreenPageLayoutKind::Spread);
    app.fullscreen_page_layout.push(displayed_page(0));
    app.fullscreen_page_layout.push(displayed_page(1));
    pending_erase(&mut app, 2, EraseInpaintKind::Commit);
    assert!(app.prepare_edited_clipboard_pages(0, true).is_ok());
    for idx in [0, 1] {
        pending_erase(&mut app, idx, EraseInpaintKind::Commit);
        assert!(app.prepare_edited_clipboard_pages(idx, false).is_err());
        assert!(app.prepare_edited_clipboard_pages(0, true).is_err());
        app.erase_inpaint_pending.retain(|key, _| key.idx != idx);
    }
    pending_erase(&mut app, 0, EraseInpaintKind::Preview);
    assert!(app.prepare_edited_clipboard_pages(0, false).is_ok());
    assert!(app.prepare_edited_clipboard_pages(0, true).is_ok());
}

#[test]
fn edited_clipboard_snapshot_read_errors_are_propagated() {
    let mut app = crate::app::setup_app_for_test();
    let path = app.tmp.path().join("page.png");
    install_image(&mut app, path);
    let key = app.page_path_key(0).unwrap();
    app.conceal_db = None;
    app.conceal_page_keys.insert(key.clone());
    assert!(
        app.prepare_edited_clipboard_pages(0, false)
            .err()
            .unwrap()
            .contains("隠蔽加工")
    );
    app.conceal_page_keys.clear();
    app.mask_db = None;
    app.mask_page_keys.insert(key.clone());
    assert!(
        app.prepare_edited_clipboard_pages(0, false)
            .err()
            .unwrap()
            .contains("消しゴム")
    );
    app.mask_page_keys.clear();
    app.local_adjust_db = None;
    app.local_adjust_page_keys.insert(key.clone());
    assert!(
        app.prepare_edited_clipboard_pages(0, false)
            .err()
            .unwrap()
            .contains("補正レイヤー")
    );
    app.local_adjust_page_keys.clear();
    app.comic_db = None;
    app.comic_page_keys.insert(key);
    assert!(
        app.prepare_edited_clipboard_pages(0, false)
            .err()
            .unwrap()
            .contains("テキスト注釈")
    );
}

#[test]
fn edited_clipboard_legacy_crop_without_canonical_size_never_writes_back_or_rolls_back_new_crop() {
    let mut app = crate::app::setup_app_for_test();
    let path = app.tmp.path().join("page.png");
    image::RgbaImage::from_pixel(8, 6, image::Rgba([20, 40, 60, 255]))
        .save(&path)
        .unwrap();
    install_image(&mut app, path.clone());
    let original = std::fs::read(&path).unwrap();
    let legacy = crop(None, 7.0);
    let key = saved_crop(&mut app, legacy);
    let db_path = app.tmp.path().join("export_crop.db");
    let db_before = std::fs::read(&db_path).unwrap();
    let pages = app.prepare_edited_clipboard_pages(0, false).unwrap();
    assert_eq!(single(&pages).edits.export_crop, Some(legacy));
    assert!(single(&pages).edits.crop_legacy_writeback.is_none());
    assert_eq!(render(&pages).size, [6, 4]);
    assert_eq!(app.export_crop_db.as_ref().unwrap().get(&key), Some(legacy));
    assert_eq!(std::fs::read(&db_path).unwrap(), db_before);
    assert_eq!(std::fs::read(&path).unwrap(), original);

    let updated = crop(Some([8, 6]), 4.0);
    app.set_export_crop_for_idx(0, Some(updated), [8, 6]);
    let after_edit = std::fs::read(&db_path).unwrap();
    assert_eq!(render(&pages).size, [6, 4]);
    assert_eq!(
        app.export_crop_db.as_ref().unwrap().get(&key),
        Some(updated)
    );
    assert_eq!(app.export_crop_page_settings.get(&0), Some(&updated));
    assert_eq!(std::fs::read(&db_path).unwrap(), after_edit);
    assert_eq!(std::fs::read(&path).unwrap(), original);
    assert!(!app.tmp.path().join("mimageviewer.dat").exists());
}

#[test]
fn edited_clipboard_legacy_crop_adopts_loaded_canonical_size_only_on_the_ui_side() {
    use crate::fs_animation::{FsCacheEntry, StaticAnimationState};
    let mut app = crate::app::setup_app_for_test();
    let path = app.tmp.path().join("page.png");
    image::RgbaImage::from_pixel(8, 6, image::Rgba([20, 40, 60, 255]))
        .save(&path)
        .unwrap();
    install_image(&mut app, path);
    let key = saved_crop(&mut app, crop(None, 7.0));
    let pixels = Arc::new(egui::ColorImage::filled([8, 6], egui::Color32::WHITE));
    let tex =
        egui::Context::default().load_texture("loaded-page", (*pixels).clone(), Default::default());
    app.fs_cache.insert(
        0,
        FsCacheEntry::Static {
            pdf_metadata: None,
            pdf_rerender_failures: Vec::new(),
            tex,
            pixels,
            source_dims: Some([8, 6]),
            load_seq: 0,
            animation: StaticAnimationState::Still,
        },
    );
    let pages = app.prepare_edited_clipboard_pages(0, true).unwrap();
    let adopted = crop(Some([8, 6]), 7.0);
    assert_eq!(single(&pages).edits.export_crop, Some(adopted));
    assert!(single(&pages).edits.crop_legacy_writeback.is_none());
    assert_eq!(
        app.export_crop_db.as_ref().unwrap().get(&key),
        Some(adopted)
    );
    let db_path = app.tmp.path().join("export_crop.db");
    let before_worker = std::fs::read(&db_path).unwrap();
    assert_eq!(render(&pages).size, [6, 4]);
    assert_eq!(std::fs::read(&db_path).unwrap(), before_worker);
}

#[test]
fn edited_clipboard_animation_is_accepted_and_renders_the_first_frame() {
    let mut app = crate::app::setup_app_for_test();
    let path = app.tmp.path().join("animated.gif");
    {
        let mut encoder =
            image::codecs::gif::GifEncoder::new(std::fs::File::create(&path).unwrap());
        for rgba in [[255, 0, 0, 255], [0, 255, 0, 255]] {
            encoder
                .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                    2,
                    2,
                    image::Rgba(rgba),
                )))
                .unwrap();
        }
    }
    install_image(&mut app, path.clone());
    let bytes = std::fs::read(&path).unwrap();
    let ctx = egui::Context::default();
    let pixels = Arc::new(egui::ColorImage::filled([2, 2], egui::Color32::RED));
    let texture = ctx.load_texture(
        "animation-first-frame",
        (*pixels).clone(),
        Default::default(),
    );
    app.fs_cache.insert(
        0,
        crate::fs_animation::FsCacheEntry::Animated {
            frames: vec![(texture.clone(), 0.1), (texture, 0.1)],
            frame_pixels: vec![Arc::clone(&pixels), pixels],
            current_frame: 1,
            playback: crate::fs_animation::AnimationPlayback::Paused,
            load_seq: 0,
        },
    );
    for fullscreen in [false, true] {
        let pages = app.prepare_edited_clipboard_pages(0, fullscreen).unwrap();
        assert_eq!(render(&pages).pixels, vec![egui::Color32::RED; 4]);
    }
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

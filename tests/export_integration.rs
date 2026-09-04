//! Ctrl+E export worker と source-based compositor の統合テスト。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use eframe::egui;
use image::ImageEncoder;
use mimageviewer::bake_stage::BakeStage;
use mimageviewer::books::{
    BakedEditSnapshot, BookAiResult, BookAiSnapshot, BookConcealSnapshot, BookMaskSnapshot,
    CompositeSource,
};
use mimageviewer::export_dialog::{
    ExportComposite, ExportEntry, ExportEvent, ExportFormat, ExportPageComposite, ExportRequest,
    ExportScale, ExportSource, resolve_session_basename, spawn_export_worker,
};
use mimageviewer::save_with_metadata::SrcFormat;

fn write_png(path: &std::path::Path, width: u32, height: u32, rgba: [u8; 4]) {
    image::RgbaImage::from_pixel(width, height, image::Rgba(rgba))
        .save(path)
        .unwrap();
}

fn jpeg_with_app1(payload: &[u8]) -> Vec<u8> {
    jpeg_with_app1_dims(payload, 4, 4)
}

fn jpeg_with_app1_dims(payload: &[u8], width: u32, height: u32) -> Vec<u8> {
    let image = image::RgbImage::from_pixel(width, height, image::Rgb([120, 80, 40]));
    let jpeg = turbojpeg::compress_image(&image, 90, turbojpeg::Subsamp::Sub2x2).unwrap();
    let length = payload.len() + 2;
    let mut output = Vec::with_capacity(jpeg.len() + payload.len() + 4);
    output.extend_from_slice(&jpeg[..2]);
    output.extend_from_slice(&[0xff, 0xe1, (length >> 8) as u8, length as u8]);
    output.extend_from_slice(payload);
    output.extend_from_slice(&jpeg[2..]);
    output
}

fn encode_png_bytes() -> Vec<u8> {
    let mut output = Vec::new();
    image::codecs::png::PngEncoder::new(&mut output)
        .write_image(&[255, 0, 0, 255], 1, 1, image::ColorType::Rgba8.into())
        .unwrap();
    output
}

fn orientation_exif_payload(value: u16) -> Vec<u8> {
    let mut payload = Vec::new();
    payload.extend_from_slice(b"Exif\0\0");
    payload.extend_from_slice(b"II");
    payload.extend_from_slice(&0x002A_u16.to_le_bytes());
    payload.extend_from_slice(&8_u32.to_le_bytes());
    payload.extend_from_slice(&1_u16.to_le_bytes());
    payload.extend_from_slice(&0x0112_u16.to_le_bytes());
    payload.extend_from_slice(&3_u16.to_le_bytes());
    payload.extend_from_slice(&1_u32.to_le_bytes());
    payload.extend_from_slice(&value.to_le_bytes());
    payload.extend_from_slice(&0_u16.to_le_bytes());
    payload.extend_from_slice(&0_u32.to_le_bytes());
    payload
}

fn jpeg_orientation(jpeg: &[u8]) -> Option<u16> {
    if jpeg.len() < 4 || jpeg[0..2] != [0xff, 0xd8] {
        return None;
    }
    let mut position = 2;
    while position + 4 <= jpeg.len() {
        if jpeg[position] != 0xff {
            position += 1;
            continue;
        }
        let marker = jpeg[position + 1];
        if marker == 0xda {
            return None;
        }
        if marker == 0x00 || marker == 0x01 || (0xd0..=0xd9).contains(&marker) {
            position += 2;
            continue;
        }
        let length = u16::from_be_bytes([jpeg[position + 2], jpeg[position + 3]]) as usize;
        if length < 2 || position + 2 + length > jpeg.len() {
            return None;
        }
        if marker == 0xe1 {
            let payload = &jpeg[position + 4..position + 2 + length];
            if payload.starts_with(b"Exif\0\0") && payload.get(6..14) == Some(b"II*\0\x08\0\0\0") {
                let ifd0 = 14;
                let count = u16::from_le_bytes([payload[ifd0], payload[ifd0 + 1]]) as usize;
                let mut entry = ifd0 + 2;
                for _ in 0..count {
                    if entry + 12 > payload.len() {
                        return None;
                    }
                    let tag = u16::from_le_bytes([payload[entry], payload[entry + 1]]);
                    if tag == 0x0112 {
                        return Some(u16::from_le_bytes([payload[entry + 8], payload[entry + 9]]));
                    }
                    entry += 12;
                }
            }
        }
        position += 2 + length;
    }
    None
}

fn edits(stage: BakeStage) -> BakedEditSnapshot {
    BakedEditSnapshot {
        params: mimageviewer::adjustment::AdjustParams::default(),
        rotation: mimageviewer::rotation_db::Rotation::None,
        conceal: None,
        erase: None,
        local_adjust: None,
        comic: None,
        comic_source_dims: None,
        export_crop: None,
        crop_legacy_writeback: None,
        format: mimageviewer::capture::CaptureFormat::Png,
        jpeg_matte: mimageviewer::capture::JpegMatte::Black,
        stage,
        creative_lut: None,
        ai: None,
    }
}

fn page(
    path: &std::path::Path,
    edits: BakedEditSnapshot,
    predicted_size: [usize; 2],
) -> ExportPageComposite {
    page_from_source(
        CompositeSource::File {
            path: path.to_path_buf(),
        },
        edits,
        predicted_size,
    )
}

fn page_from_source(
    source: CompositeSource,
    edits: BakedEditSnapshot,
    predicted_size: [usize; 2],
) -> ExportPageComposite {
    ExportPageComposite {
        source,
        edits,
        pdf_render_long_edge: 4096,
        predicted_size,
        has_conceal_mask: false,
    }
}

fn entry(label: &str, suffix: u8) -> ExportEntry {
    ExportEntry {
        label: label.to_string(),
        suffix,
        conceal_preset: None,
        crop_override: None,
    }
}

fn request(
    source_path: &std::path::Path,
    output_dir: &std::path::Path,
    basename: &str,
    composite: ExportComposite,
    entries: Vec<ExportEntry>,
) -> ExportRequest {
    ExportRequest {
        source: ExportSource::File {
            path: source_path.to_path_buf(),
        },
        original_format: SrcFormat::Png,
        output_format: ExportFormat::Png,
        output_dir: output_dir.to_path_buf(),
        basename: basename.to_string(),
        composite,
        scale: ExportScale::Full,
        entries,
        include_metadata: false,
        local_ai_activity: None,
    }
}

fn collect(pending: mimageviewer::export_dialog::ExportPending) -> Vec<ExportEvent> {
    let mut events = Vec::new();
    loop {
        let event = pending
            .rx
            .recv_timeout(Duration::from_secs(20))
            .expect("worker event");
        let done = matches!(event, ExportEvent::AllDone | ExportEvent::Cancelled);
        events.push(event);
        if done {
            return events;
        }
    }
}

fn completed(events: &[ExportEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, ExportEvent::Completed(_)))
        .count()
}

fn failed(events: &[ExportEvent]) -> usize {
    events
        .iter()
        .filter(|event| matches!(event, ExportEvent::Failed(_)))
        .count()
}

fn staged_edits(stage: BakeStage) -> BakedEditSnapshot {
    let mut edits = edits(stage);
    edits.params.post_filter = mimageviewer::adjustment::PostFilter::Sepia;
    edits.ai = Some(BookAiSnapshot {
        run: Box::new(|image, _| {
            Ok(BookAiResult {
                image: egui::ColorImage::filled(
                    [image.size[0] * 2, image.size[1] * 2],
                    egui::Color32::from_rgb(20, 180, 70),
                ),
                used_upscale: true,
            })
        }),
    });
    edits
}

#[test]
fn selected_bake_stage_controls_ai_and_display_adjust() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.png");
    write_png(&source, 3, 2, [200, 30, 20, 255]);

    for (name, stage, predicted) in [
        ("edits", BakeStage::Edits, [3, 2]),
        ("ai", BakeStage::Ai, [6, 4]),
        ("display", BakeStage::DisplayAdjust, [6, 4]),
    ] {
        let composite = ExportComposite::Single(page(&source, staged_edits(stage), predicted));
        assert_eq!(composite.render_size().unwrap(), predicted);
        let pending = spawn_export_worker(request(
            &source,
            temp.path(),
            name,
            composite,
            vec![entry("current", 0)],
        ))
        .unwrap();
        assert_eq!(completed(&collect(pending)), 1);
    }

    let edits_image = image::open(temp.path().join("edits_0.png"))
        .unwrap()
        .to_rgba8();
    let ai_image = image::open(temp.path().join("ai_0.png"))
        .unwrap()
        .to_rgba8();
    let display_image = image::open(temp.path().join("display_0.png"))
        .unwrap()
        .to_rgba8();
    assert_eq!(edits_image.dimensions(), (3, 2));
    assert_eq!(ai_image.dimensions(), (6, 4));
    assert_eq!(display_image.dimensions(), (6, 4));
    assert_eq!(ai_image.get_pixel(0, 0).0, [20, 180, 70, 255]);
    assert_ne!(display_image.get_pixel(0, 0), ai_image.get_pixel(0, 0));
}

#[test]
fn source_larger_than_gpu_texture_limit_keeps_its_dimensions_without_ai() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("wide.png");
    write_png(&source, 8193, 1, [7, 8, 9, 255]);
    let composite =
        ExportComposite::Single(page(&source, edits(BakeStage::DisplayAdjust), [8193, 1]));
    let pending = spawn_export_worker(request(
        &source,
        temp.path(),
        "wide",
        composite,
        vec![entry("current", 0)],
    ))
    .unwrap();

    assert_eq!(completed(&collect(pending)), 1);
    assert_eq!(
        image::image_dimensions(temp.path().join("wide_0.png")).unwrap(),
        (8193, 1)
    );
}

#[test]
fn spread_prediction_matches_worker_output_after_rotation_and_crop() {
    let temp = tempfile::tempdir().unwrap();
    let left_path = temp.path().join("left.png");
    let right_path = temp.path().join("right.png");
    write_png(&left_path, 4, 3, [255, 0, 0, 255]);
    write_png(&right_path, 2, 5, [0, 0, 255, 255]);

    let mut left_edits = edits(BakeStage::Edits);
    left_edits.rotation = mimageviewer::rotation_db::Rotation::Cw90;
    let mut right_edits = edits(BakeStage::Edits);
    right_edits.export_crop = Some(mimageviewer::export_crop::CropSettings::authored(
        mimageviewer::export_crop::CropRect {
            min_x: 0.0,
            min_y: 1.0,
            max_x: 2.0,
            max_y: 4.0,
        },
        mimageviewer::export_crop::CropAspectMode::Free,
        [2, 5],
    ));
    let composite = ExportComposite::Spread {
        left: page(&left_path, left_edits, [4, 3]),
        right: page(&right_path, right_edits, [2, 5]),
    };
    let predicted = composite.render_size().unwrap();
    let pending = spawn_export_worker(request(
        &left_path,
        temp.path(),
        "spread",
        composite,
        vec![entry("current", 0)],
    ))
    .unwrap();

    assert_eq!(completed(&collect(pending)), 1);
    assert_eq!(
        image::image_dimensions(temp.path().join("spread_0.png")).unwrap(),
        (predicted[0] as u32, predicted[1] as u32)
    );
}

#[test]
fn conceal_presets_replace_only_the_current_conceal_setting() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("conceal.png");
    write_png(&source, 2, 1, [180, 30, 20, 255]);
    let mut current = mimageviewer::conceal::ConcealPreset::default();
    current.conceal_type = mimageviewer::conceal::ConcealType::BlackFill;
    let mut alternate = current.clone();
    alternate.conceal_type = mimageviewer::conceal::ConcealType::WhiteFill;
    let mut snapshot = edits(BakeStage::Edits);
    snapshot.conceal = Some(BookConcealSnapshot {
        mask: BookMaskSnapshot {
            bitmap: vec![true, true],
            shapes: Vec::new(),
            size: [2, 1],
        },
        preset: current,
    });
    let mut composite_page = page(&source, snapshot, [2, 1]);
    composite_page.has_conceal_mask = true;
    let entries = vec![
        entry("current", 0),
        ExportEntry {
            label: "preset".to_string(),
            suffix: 1,
            conceal_preset: Some(alternate.clone()),
            crop_override: None,
        },
    ];
    let pending = spawn_export_worker(request(
        &source,
        temp.path(),
        "conceal",
        ExportComposite::Single(composite_page),
        entries,
    ))
    .unwrap();
    assert_eq!(completed(&collect(pending)), 2);

    let current = image::open(temp.path().join("conceal_0.png"))
        .unwrap()
        .to_rgba8();
    let alternate_image = image::open(temp.path().join("conceal_1.png"))
        .unwrap()
        .to_rgba8();
    assert_eq!(current.get_pixel(0, 0).0, [0, 0, 0, 255]);
    assert_eq!(alternate_image.get_pixel(0, 0).0, [255, 255, 255, 255]);

    let pending = spawn_export_worker(request(
        &source,
        temp.path(),
        "maskless",
        ExportComposite::Single(page(&source, edits(BakeStage::Edits), [2, 1])),
        vec![ExportEntry {
            label: "preset".to_string(),
            suffix: 1,
            conceal_preset: Some(alternate),
            crop_override: None,
        }],
    ))
    .unwrap();
    assert_eq!(completed(&collect(pending)), 1);
    let maskless = image::open(temp.path().join("maskless_1.png"))
        .unwrap()
        .to_rgba8();
    assert_eq!(maskless.get_pixel(0, 0).0, [180, 30, 20, 255]);
}

#[test]
fn sns_frames_are_scaled_against_the_actual_composed_dimensions() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("sns.png");
    write_png(&source, 5, 2, [40, 80, 120, 255]);
    let mut snapshot = edits(BakeStage::Ai);
    snapshot.ai = Some(BookAiSnapshot {
        run: Box::new(|_, _| {
            Ok(BookAiResult {
                image: egui::ColorImage::filled([10, 4], egui::Color32::from_rgb(40, 80, 120)),
                used_upscale: true,
            })
        }),
    });
    let frames = [
        mimageviewer::export_crop::CropRect {
            min_x: 0.0,
            min_y: 0.0,
            max_x: 2.0,
            max_y: 2.0,
        },
        mimageviewer::export_crop::CropRect {
            min_x: 2.0,
            min_y: 0.0,
            max_x: 5.0,
            max_y: 2.0,
        },
    ];
    let entries = frames
        .into_iter()
        .enumerate()
        .map(|(index, crop)| ExportEntry {
            label: format!("frame {index}"),
            suffix: (index + 1) as u8,
            conceal_preset: None,
            crop_override: Some((crop, [5, 2])),
        })
        .collect();
    let pending = spawn_export_worker(request(
        &source,
        temp.path(),
        "sns",
        ExportComposite::Single(page(&source, snapshot, [10, 4])),
        entries,
    ))
    .unwrap();

    assert_eq!(completed(&collect(pending)), 2);
    assert_eq!(
        image::image_dimensions(temp.path().join("sns_1.png")).unwrap(),
        (4, 4)
    );
    assert_eq!(
        image::image_dimensions(temp.path().join("sns_2.png")).unwrap(),
        (6, 4)
    );
}

#[test]
fn export_scale_is_applied_after_composition() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("scale.png");
    write_png(&source, 8, 6, [1, 2, 3, 255]);
    let mut export = request(
        &source,
        temp.path(),
        "scale",
        ExportComposite::Single(page(&source, edits(BakeStage::Edits), [8, 6])),
        vec![entry("current", 0)],
    );
    export.scale = ExportScale::Half;
    let pending = spawn_export_worker(export).unwrap();

    assert_eq!(completed(&collect(pending)), 1);
    assert_eq!(
        image::image_dimensions(temp.path().join("scale_0.png")).unwrap(),
        (4, 3)
    );
}

#[test]
fn spread_scale_uses_the_rendered_crop_dimensions() {
    let temp = tempfile::tempdir().unwrap();
    let left_path = temp.path().join("scale-left.png");
    let right_path = temp.path().join("scale-right.png");
    write_png(&left_path, 5, 4, [200, 0, 0, 255]);
    write_png(&right_path, 3, 5, [0, 0, 200, 255]);

    let mut left_edits = edits(BakeStage::Edits);
    left_edits.export_crop = Some(mimageviewer::export_crop::CropSettings::authored(
        mimageviewer::export_crop::CropRect {
            min_x: 1.0,
            min_y: 1.0,
            max_x: 5.0,
            max_y: 4.0,
        },
        mimageviewer::export_crop::CropAspectMode::Free,
        [5, 4],
    ));
    let composite = ExportComposite::Spread {
        left: page(&left_path, left_edits, [5, 4]),
        right: page(&right_path, edits(BakeStage::Edits), [3, 5]),
    };
    assert_eq!(composite.render_size().unwrap(), [7, 5]);
    assert_eq!(ExportScale::Half.scaled_size([7, 5]), [4, 3]);
    assert_eq!(ExportScale::Quarter.scaled_size([7, 5]), [2, 1]);

    let mut export = request(
        &left_path,
        temp.path(),
        "scale-spread",
        composite,
        vec![entry("current", 0)],
    );
    export.source = ExportSource::RenderedSpread;
    export.scale = ExportScale::Half;
    let events = collect(spawn_export_worker(export).unwrap());

    assert_eq!(completed(&events), 1);
    assert_eq!(
        image::image_dimensions(temp.path().join("scale-spread_0.png")).unwrap(),
        (4, 3)
    );
}

#[test]
fn spread_conceal_is_composed_independently_for_each_page() {
    let temp = tempfile::tempdir().unwrap();
    let left_path = temp.path().join("conceal-left.png");
    let right_path = temp.path().join("conceal-right.png");
    write_png(&left_path, 1, 1, [255, 255, 255, 255]);
    write_png(&right_path, 1, 1, [10, 20, 30, 255]);

    let mut left_edits = edits(BakeStage::Edits);
    left_edits.conceal = Some(BookConcealSnapshot {
        mask: BookMaskSnapshot {
            bitmap: vec![true],
            shapes: Vec::new(),
            size: [1, 1],
        },
        preset: mimageviewer::conceal::ConcealPreset::default(),
    });
    let composite = ExportComposite::Spread {
        left: page(&left_path, left_edits, [1, 1]),
        right: page(&right_path, edits(BakeStage::Edits), [1, 1]),
    };
    let mut black = mimageviewer::conceal::ConcealPreset::default();
    black.conceal_type = mimageviewer::conceal::ConcealType::BlackFill;
    let preset_entry = ExportEntry {
        label: "black".to_string(),
        suffix: 1,
        conceal_preset: Some(black),
        crop_override: None,
    };
    let mut export = request(
        &left_path,
        temp.path(),
        "conceal-spread",
        composite,
        vec![preset_entry],
    );
    export.source = ExportSource::RenderedSpread;
    let events = collect(spawn_export_worker(export).unwrap());

    assert_eq!(completed(&events), 1);
    let output = image::open(temp.path().join("conceal-spread_1.png"))
        .unwrap()
        .to_rgba8();
    assert_eq!(output.dimensions(), (2, 1));
    assert_eq!(output.get_pixel(0, 0).0, [0, 0, 0, 255]);
    assert_eq!(output.get_pixel(1, 0).0, [10, 20, 30, 255]);
}

#[test]
fn export_cancel_mid_batch() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("cancel-source.png");
    write_png(&source, 2048, 2048, [20, 40, 60, 255]);
    let pending = spawn_export_worker(request(
        &source,
        temp.path(),
        "cancel",
        ExportComposite::Single(page(&source, edits(BakeStage::Edits), [2048, 2048])),
        (0..5)
            .map(|index| entry(&format!("entry{index}"), index))
            .collect(),
    ))
    .unwrap();

    match pending.rx.recv_timeout(Duration::from_secs(20)).unwrap() {
        ExportEvent::Started { .. } => {}
        other => panic!("expected first Started event, got {other:?}"),
    }
    pending.cancel.store(true, Ordering::Relaxed);

    let mut saw_cancel = false;
    while let Ok(event) = pending.rx.recv_timeout(Duration::from_secs(20)) {
        if matches!(event, ExportEvent::Cancelled) {
            saw_cancel = true;
            break;
        }
        if matches!(event, ExportEvent::AllDone) {
            break;
        }
    }
    assert!(saw_cancel, "worker should report cancellation");
    assert!(
        !temp.path().join("cancel_4.png").exists(),
        "entries after cancellation must not be written"
    );
}

#[test]
fn export_fallback_format_for_heic() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("fallback-source.png");
    write_png(&source, 3, 3, [100, 80, 60, 255]);
    let original_format = SrcFormat::Other("heic".to_string());
    let output_format = ExportFormat::from_source(
        &original_format,
        mimageviewer::conceal::ExportFallbackFormat::Jpeg95,
    );
    assert_eq!(output_format, ExportFormat::Jpeg95);

    let mut export = request(
        &source,
        temp.path(),
        "fallback",
        ExportComposite::Single(page(&source, edits(BakeStage::Edits), [3, 3])),
        vec![entry("current", 0)],
    );
    export.original_format = original_format;
    export.output_format = output_format;
    export.include_metadata = true;
    let events = collect(spawn_export_worker(export).unwrap());

    assert_eq!(completed(&events), 1);
    assert!(temp.path().join("fallback_0.jpg").exists());
}

#[test]
fn export_zip_source_no_path() {
    let temp = tempfile::tempdir().unwrap();
    let zip_path = temp.path().join("source.zip");
    {
        use std::io::Write;
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("entry.png", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&encode_png_bytes()).unwrap();
        zip.finish().unwrap();
    }
    let composite_source = CompositeSource::ZipEntry {
        zip_path: zip_path.clone(),
        entry_name: "entry.png".to_string(),
    };
    let export = ExportRequest {
        source: ExportSource::ZipEntry {
            zip_path,
            entry_name: "entry.png".to_string(),
        },
        original_format: SrcFormat::Png,
        output_format: ExportFormat::Png,
        output_dir: temp.path().to_path_buf(),
        basename: "zip-entry".to_string(),
        composite: ExportComposite::Single(page_from_source(
            composite_source,
            edits(BakeStage::Edits),
            [1, 1],
        )),
        scale: ExportScale::Full,
        entries: vec![entry("current", 0)],
        include_metadata: true,
        local_ai_activity: None,
    };
    let events = collect(spawn_export_worker(export).unwrap());

    assert_eq!(completed(&events), 1);
    assert!(temp.path().join("zip-entry_0.png").exists());
}

#[test]
fn export_zip_di2_orientation_canonical_after_display_rotation() {
    let temp = tempfile::tempdir().unwrap();
    let zip_path = temp.path().join("rotated-source.zip");
    {
        use std::io::Write;
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        zip.start_file("rotated.jpg", zip::write::SimpleFileOptions::default())
            .unwrap();
        zip.write_all(&jpeg_with_app1_dims(&orientation_exif_payload(6), 2, 4))
            .unwrap();
        zip.finish().unwrap();
    }
    let composite_source = CompositeSource::ZipEntry {
        zip_path: zip_path.clone(),
        entry_name: "rotated.jpg".to_string(),
    };
    let export = ExportRequest {
        source: ExportSource::ZipEntry {
            zip_path,
            entry_name: "rotated.jpg".to_string(),
        },
        original_format: SrcFormat::Jpeg,
        output_format: ExportFormat::Jpeg95,
        output_dir: temp.path().to_path_buf(),
        basename: "zip-di2".to_string(),
        composite: ExportComposite::Single(page_from_source(
            composite_source,
            edits(BakeStage::Edits),
            [4, 2],
        )),
        scale: ExportScale::Full,
        entries: vec![entry("current", 0)],
        include_metadata: true,
        local_ai_activity: None,
    };
    let events = collect(spawn_export_worker(export).unwrap());

    assert_eq!(completed(&events), 1);
    let output = std::fs::read(temp.path().join("zip-di2_0.jpg")).unwrap();
    assert_eq!(jpeg_orientation(&output), Some(1));
    let decoded = image::load_from_memory(&output).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (4, 2));
}

#[test]
fn metadata_copy_still_uses_the_original_source() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("metadata.jpg");
    let marker = b"Exif\0\0ctrl-e-worker-composite";
    std::fs::write(&source, jpeg_with_app1(marker)).unwrap();
    let mut export = ExportRequest {
        source: ExportSource::File {
            path: source.clone(),
        },
        original_format: SrcFormat::Jpeg,
        output_format: ExportFormat::Jpeg95,
        output_dir: temp.path().to_path_buf(),
        basename: "metadata".to_string(),
        composite: ExportComposite::Single(page(&source, edits(BakeStage::Edits), [4, 4])),
        scale: ExportScale::Full,
        entries: vec![entry("current", 0)],
        include_metadata: true,
        local_ai_activity: None,
    };
    export.include_metadata = true;
    let pending = spawn_export_worker(export).unwrap();

    assert_eq!(completed(&collect(pending)), 1);
    let output = std::fs::read(temp.path().join("metadata_0.jpg")).unwrap();
    assert!(output.windows(marker.len()).any(|window| window == marker));
}

#[test]
fn animated_webp_is_rejected_before_source_composition() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("animated.webp");
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&18_u32.to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(b"ANIM");
    bytes.extend_from_slice(&6_u32.to_le_bytes());
    bytes.extend_from_slice(&[0, 0, 0, 0, 1, 0]);
    std::fs::write(&source, bytes).unwrap();

    for (basename, output_format) in [
        ("animated-png", ExportFormat::Png),
        ("animated-webp", ExportFormat::Webp),
    ] {
        let mut export = request(
            &source,
            temp.path(),
            basename,
            ExportComposite::Single(page(&source, edits(BakeStage::Edits), [1, 1])),
            vec![entry("current", 0)],
        );
        export.original_format = SrcFormat::Webp;
        export.output_format = output_format;
        let events = collect(spawn_export_worker(export).unwrap());

        assert_eq!(completed(&events), 0);
        assert_eq!(failed(&events), 1);
        assert!(
            !temp
                .path()
                .join(format!("{basename}_0.{}", output_format.extension()))
                .exists()
        );
    }
}

#[test]
fn export_webp_source_read_failure_fails_all_entries() {
    let temp = tempfile::tempdir().unwrap();
    let missing = temp.path().join("missing.webp");
    let mut export = request(
        &missing,
        temp.path(),
        "missing",
        ExportComposite::Single(page(&missing, edits(BakeStage::Edits), [2, 2])),
        vec![entry("a", 0), entry("b", 1)],
    );
    export.original_format = SrcFormat::Webp;
    let events = collect(spawn_export_worker(export).unwrap());

    assert_eq!(completed(&events), 0);
    assert_eq!(failed(&events), 2);
    assert!(!temp.path().join("missing_0.png").exists());
    assert!(!temp.path().join("missing_1.png").exists());
}

#[test]
fn multi_entry_export_keeps_collision_and_partial_failure_semantics() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("batch.png");
    write_png(&source, 2, 2, [40, 50, 60, 255]);
    std::fs::write(temp.path().join("batch_1.png"), b"existing").unwrap();

    let pending = spawn_export_worker(request(
        &source,
        temp.path(),
        "batch",
        ExportComposite::Single(page(&source, edits(BakeStage::Edits), [2, 2])),
        vec![
            entry("current", 0),
            entry("preset1", 1),
            entry("preset2", 2),
        ],
    ))
    .unwrap();
    let events = collect(pending);

    assert_eq!(completed(&events), 2);
    assert_eq!(failed(&events), 1);
    assert!(temp.path().join("batch_0.png").exists());
    assert!(temp.path().join("batch_2.png").exists());
    assert_eq!(
        std::fs::read(temp.path().join("batch_1.png")).unwrap(),
        b"existing"
    );
}

#[test]
fn session_basename_reserves_every_selected_suffix() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("batch_2.png"), b"existing").unwrap();

    assert_eq!(
        resolve_session_basename(temp.path(), "batch", "png", &[0, 1, 2]).unwrap(),
        "batch_0001"
    );
}

fn batch_edits(
    format: mimageviewer::capture::CaptureFormat,
) -> mimageviewer::books::BakedEditSnapshot {
    let mut edits = edits(BakeStage::DisplayAdjust);
    edits.format = format;
    edits
}

fn batch_item(
    source_path: &std::path::Path,
    filename: &str,
    format: mimageviewer::capture::CaptureFormat,
) -> mimageviewer::export_batch::BatchExportItem {
    mimageviewer::export_batch::BatchExportItem {
        filename: filename.to_string(),
        dirname: "src".to_string(),
        source: CompositeSource::File {
            path: source_path.to_path_buf(),
        },
        edits: batch_edits(format),
    }
}

#[test]
fn batch_export_still_uses_the_shared_composite_writer() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.png");
    let output = temp.path().join("output");
    write_png(&source, 8, 4, [10, 20, 30, 255]);

    let pending = mimageviewer::export_batch::spawn_batch_export_worker(
        mimageviewer::export_batch::BatchExportRequest {
            output_dir: output.clone(),
            template: "<dirname>_<filename>".to_string(),
            scale: ExportScale::Half,
            items: vec![
                batch_item(&source, "page", mimageviewer::capture::CaptureFormat::Png),
                batch_item(&source, "page", mimageviewer::capture::CaptureFormat::Png),
            ],
            local_ai_activity: mimageviewer::LocalAiActivityLease::new(Arc::new(
                std::sync::atomic::AtomicUsize::new(0),
            )),
        },
    )
    .unwrap();
    let events = collect(pending);

    assert_eq!(completed(&events), 2);
    assert_eq!(
        image::image_dimensions(output.join("src_page.png")).unwrap(),
        (4, 2)
    );
    assert_eq!(
        image::image_dimensions(output.join("src_page_1.png")).unwrap(),
        (4, 2)
    );
}

#[test]
fn batch_export_keeps_going_after_a_missing_source() {
    let temp = tempfile::tempdir().unwrap();
    let source = temp.path().join("source.png");
    let missing = temp.path().join("missing.png");
    let output = temp.path().join("output");
    write_png(&source, 4, 4, [200, 100, 50, 255]);

    let pending = mimageviewer::export_batch::spawn_batch_export_worker(
        mimageviewer::export_batch::BatchExportRequest {
            output_dir: output.clone(),
            template: "<filename>".to_string(),
            scale: ExportScale::Full,
            items: vec![
                batch_item(
                    &missing,
                    "missing",
                    mimageviewer::capture::CaptureFormat::Jpeg85,
                ),
                batch_item(
                    &source,
                    "source",
                    mimageviewer::capture::CaptureFormat::Jpeg85,
                ),
            ],
            local_ai_activity: mimageviewer::LocalAiActivityLease::new(Arc::new(
                std::sync::atomic::AtomicUsize::new(0),
            )),
        },
    )
    .unwrap();
    let events = collect(pending);

    assert_eq!(completed(&events), 1);
    assert_eq!(failed(&events), 1);
    assert!(output.join("source.jpg").exists());
    assert!(!output.join("missing.jpg").exists());
}

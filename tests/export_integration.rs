//! Ctrl+E export worker と save_with_metadata の統合テスト。

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Duration;

use eframe::egui;
use image::ImageEncoder;
use mimageviewer::export_dialog::{
    ExportEntry, ExportEvent, ExportFormat, ExportPagePixels, ExportPixels, ExportRequest,
    ExportSource, resolve_session_basename, spawn_export_worker,
};
use mimageviewer::save_with_metadata::{SaveOptions, SrcFormat, save_image_with_metadata};

fn solid_image(w: usize, h: usize, color: egui::Color32) -> Arc<egui::ColorImage> {
    Arc::new(egui::ColorImage::new([w, h], vec![color; w * h]))
}

fn single_pixels(base_pixels: Arc<egui::ColorImage>) -> ExportPixels {
    ExportPixels::Single(ExportPagePixels {
        base_pixels,
        conceal_mask: None,
        crop: None,
    })
}

fn entry(label: &str, suffix: u8) -> ExportEntry {
    ExportEntry {
        label: label.to_string(),
        suffix,
        conceal_preset: None,
    }
}

fn collect_events(
    pending: mimageviewer::export_dialog::ExportPending,
    timeout_secs: u64,
) -> Vec<ExportEvent> {
    let mut events = Vec::new();
    loop {
        let event = pending
            .rx
            .recv_timeout(Duration::from_secs(timeout_secs))
            .expect("export worker should send an event");
        let done = matches!(event, ExportEvent::AllDone | ExportEvent::Cancelled);
        events.push(event);
        if done {
            break;
        }
    }
    events
}

fn completed_count(events: &[ExportEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, ExportEvent::Completed(_)))
        .count()
}

fn failed_count(events: &[ExportEvent]) -> usize {
    events
        .iter()
        .filter(|e| matches!(e, ExportEvent::Failed(_)))
        .count()
}

fn encode_png_bytes() -> Vec<u8> {
    let mut out = Vec::new();
    image::codecs::png::PngEncoder::new(&mut out)
        .write_image(&[255, 0, 0, 255], 1, 1, image::ColorType::Rgba8.into())
        .unwrap();
    out
}

fn jpeg_with_app1_payload(payload: &[u8]) -> Vec<u8> {
    let rgb = vec![128u8; 8 * 8 * 3];
    let image = image::RgbImage::from_raw(8, 8, rgb).unwrap();
    let jpeg = turbojpeg::compress_image(&image, 90, turbojpeg::Subsamp::Sub2x2).unwrap();
    let jpeg = jpeg.as_ref();
    let len = payload.len() + 2;
    let mut out = Vec::with_capacity(jpeg.len() + payload.len() + 4);
    out.extend_from_slice(&jpeg[..2]);
    out.extend_from_slice(&[0xFF, 0xE1]);
    out.push((len >> 8) as u8);
    out.push((len & 0xFF) as u8);
    out.extend_from_slice(payload);
    out.extend_from_slice(&jpeg[2..]);
    out
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

fn app1_payloads(jpeg: &[u8]) -> Vec<&[u8]> {
    let mut out = Vec::new();
    if jpeg.len() < 4 || jpeg[0] != 0xFF || jpeg[1] != 0xD8 {
        return out;
    }
    let mut pos = 2;
    while pos + 4 <= jpeg.len() {
        if jpeg[pos] != 0xFF {
            pos += 1;
            continue;
        }
        let marker = jpeg[pos + 1];
        if marker == 0xDA {
            break;
        }
        if marker == 0x00 || marker == 0x01 || (0xD0..=0xD9).contains(&marker) {
            pos += 2;
            continue;
        }
        let len = u16::from_be_bytes([jpeg[pos + 2], jpeg[pos + 3]]) as usize;
        if len < 2 || pos + 2 + len > jpeg.len() {
            break;
        }
        if marker == 0xE1 {
            out.push(&jpeg[pos + 4..pos + 2 + len]);
        }
        pos += 2 + len;
    }
    out
}

fn jpeg_orientation(jpeg: &[u8]) -> Option<u16> {
    let payload = app1_payloads(jpeg)
        .into_iter()
        .find(|p| p.starts_with(b"Exif\0\0"))?;
    let tiff = 6;
    if payload.get(tiff..tiff + 8)? != b"II*\0\x08\0\0\0" {
        return None;
    }
    let ifd0 = tiff + 8;
    let count = u16::from_le_bytes([payload[ifd0], payload[ifd0 + 1]]) as usize;
    let mut entry = ifd0 + 2;
    for _ in 0..count {
        let tag = u16::from_le_bytes([payload[entry], payload[entry + 1]]);
        if tag == 0x0112 {
            return Some(u16::from_le_bytes([payload[entry + 8], payload[entry + 9]]));
        }
        entry += 12;
    }
    None
}

fn animated_webp_bytes() -> Vec<u8> {
    let mut bytes = Vec::new();
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&0_u32.to_le_bytes());
    bytes.extend_from_slice(b"WEBP");
    bytes.extend_from_slice(b"ANIM");
    bytes.extend_from_slice(&6_u32.to_le_bytes());
    bytes.extend_from_slice(&[0, 0, 0, 0, 1, 0]);
    let total = (bytes.len() - 8) as u32;
    bytes[4..8].copy_from_slice(&total.to_le_bytes());
    bytes
}

#[test]
fn export_single_jpeg_with_metadata() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("source.jpg");
    let marker = b"Exif\0\0phase7-export-marker";
    std::fs::write(&src, jpeg_with_app1_payload(marker)).unwrap();

    let pending = spawn_export_worker(ExportRequest {
        source: ExportSource::File { path: src },
        original_format: SrcFormat::Jpeg,
        output_format: ExportFormat::Jpeg95,
        output_dir: temp.path().to_path_buf(),
        basename: "out".to_string(),
        pixels: single_pixels(solid_image(4, 4, egui::Color32::from_rgb(10, 20, 30))),
        entries: vec![entry("current", 0)],
        include_metadata: true,
    })
    .unwrap();
    let events = collect_events(pending, 5);
    assert_eq!(completed_count(&events), 1);

    let out = std::fs::read(temp.path().join("out_0.jpg")).unwrap();
    assert!(
        app1_payloads(&out).iter().any(|payload| *payload == marker),
        "APP1 metadata marker should be copied to output JPEG"
    );
}

#[test]
fn export_batch_no_collision() {
    let temp = tempfile::tempdir().unwrap();
    let pending = spawn_export_worker(ExportRequest {
        source: ExportSource::PdfPage,
        original_format: SrcFormat::Other("pdf".to_string()),
        output_format: ExportFormat::Png,
        output_dir: temp.path().to_path_buf(),
        basename: "batch".to_string(),
        pixels: single_pixels(solid_image(2, 2, egui::Color32::LIGHT_BLUE)),
        entries: vec![
            entry("current", 0),
            entry("preset1", 1),
            entry("preset2", 2),
        ],
        include_metadata: false,
    })
    .unwrap();
    let events = collect_events(pending, 5);
    assert_eq!(completed_count(&events), 3);
    for suffix in 0..=2 {
        assert!(temp.path().join(format!("batch_{suffix}.png")).exists());
    }
}

#[test]
fn export_batch_with_collision_uses_session_number() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("batch_0.png"), b"already here").unwrap();
    let basename = resolve_session_basename(temp.path(), "batch", "png", &[0, 1]).unwrap();
    assert_eq!(basename, "batch_0001");

    let pending = spawn_export_worker(ExportRequest {
        source: ExportSource::PdfPage,
        original_format: SrcFormat::Other("pdf".to_string()),
        output_format: ExportFormat::Png,
        output_dir: temp.path().to_path_buf(),
        basename,
        pixels: single_pixels(solid_image(2, 2, egui::Color32::LIGHT_GREEN)),
        entries: vec![entry("current", 0), entry("preset1", 1)],
        include_metadata: false,
    })
    .unwrap();
    let events = collect_events(pending, 5);
    assert_eq!(completed_count(&events), 2);
    assert!(temp.path().join("batch_0001_0.png").exists());
    assert!(temp.path().join("batch_0001_1.png").exists());
}

#[test]
fn export_batch_partial_failure_continues() {
    let temp = tempfile::tempdir().unwrap();
    std::fs::write(temp.path().join("out_1.png"), b"collision").unwrap();

    let pending = spawn_export_worker(ExportRequest {
        source: ExportSource::PdfPage,
        original_format: SrcFormat::Other("pdf".to_string()),
        output_format: ExportFormat::Png,
        output_dir: temp.path().to_path_buf(),
        basename: "out".to_string(),
        pixels: single_pixels(solid_image(2, 2, egui::Color32::GRAY)),
        entries: vec![
            entry("current", 0),
            entry("preset1", 1),
            entry("preset2", 2),
        ],
        include_metadata: false,
    })
    .unwrap();
    let events = collect_events(pending, 5);
    assert_eq!(completed_count(&events), 2);
    assert_eq!(failed_count(&events), 1);
    assert!(temp.path().join("out_0.png").exists());
    assert!(temp.path().join("out_2.png").exists());
    assert_eq!(
        std::fs::read(temp.path().join("out_1.png")).unwrap(),
        b"collision"
    );
}

#[test]
fn export_cancel_mid_batch() {
    let temp = tempfile::tempdir().unwrap();
    let pending = spawn_export_worker(ExportRequest {
        source: ExportSource::PdfPage,
        original_format: SrcFormat::Other("pdf".to_string()),
        output_format: ExportFormat::Png,
        output_dir: temp.path().to_path_buf(),
        basename: "cancel".to_string(),
        pixels: single_pixels(solid_image(2048, 2048, egui::Color32::from_rgb(20, 40, 60))),
        entries: (0..5).map(|i| entry(&format!("entry{i}"), i)).collect(),
        include_metadata: false,
    })
    .unwrap();

    match pending.rx.recv_timeout(Duration::from_secs(5)).unwrap() {
        ExportEvent::Started { .. } => {}
        other => panic!("expected first Started event, got {other:?}"),
    }
    pending.cancel.store(true, Ordering::Relaxed);

    let mut saw_cancel = false;
    while let Ok(event) = pending.rx.recv_timeout(Duration::from_secs(10)) {
        if matches!(event, ExportEvent::Cancelled) {
            saw_cancel = true;
            break;
        }
        if matches!(event, ExportEvent::AllDone) {
            break;
        }
    }
    assert!(
        saw_cancel,
        "worker should observe cancel before starting all entries"
    );
    assert!(
        !temp.path().join("cancel_4.png").exists(),
        "later entries should not be written after cancellation"
    );
}

#[test]
fn export_fallback_format_for_heic() {
    let temp = tempfile::tempdir().unwrap();
    let pending = spawn_export_worker(ExportRequest {
        source: ExportSource::PdfPage,
        original_format: SrcFormat::Other("heic".to_string()),
        output_format: ExportFormat::Jpeg95,
        output_dir: temp.path().to_path_buf(),
        basename: "fallback".to_string(),
        pixels: single_pixels(solid_image(3, 3, egui::Color32::from_rgb(100, 80, 60))),
        entries: vec![entry("current", 0)],
        include_metadata: true,
    })
    .unwrap();
    let events = collect_events(pending, 5);
    assert_eq!(completed_count(&events), 1);
    assert!(temp.path().join("fallback_0.jpg").exists());
}

#[test]
fn export_zip_source_no_path() {
    let temp = tempfile::tempdir().unwrap();
    let zip_path = temp.path().join("source.zip");
    {
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("entry.png", options).unwrap();
        use std::io::Write;
        zip.write_all(&encode_png_bytes()).unwrap();
        zip.finish().unwrap();
    }

    let pending = spawn_export_worker(ExportRequest {
        source: ExportSource::ZipEntry {
            zip_path,
            entry_name: "entry.png".to_string(),
        },
        original_format: SrcFormat::Png,
        output_format: ExportFormat::Png,
        output_dir: temp.path().to_path_buf(),
        basename: "zip-entry".to_string(),
        pixels: single_pixels(solid_image(2, 2, egui::Color32::WHITE)),
        entries: vec![entry("current", 0)],
        include_metadata: true,
    })
    .unwrap();
    let events = collect_events(pending, 5);
    assert_eq!(completed_count(&events), 1);
    assert!(temp.path().join("zip-entry_0.png").exists());
}

#[test]
fn export_zip_di2_orientation_canonical_after_display_rotation() {
    let temp = tempfile::tempdir().unwrap();
    let zip_path = temp.path().join("rotated-source.zip");
    {
        let file = std::fs::File::create(&zip_path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let options = zip::write::SimpleFileOptions::default();
        zip.start_file("rotated.jpg", options).unwrap();
        use std::io::Write;
        zip.write_all(&jpeg_with_app1_payload(&orientation_exif_payload(6)))
            .unwrap();
        zip.finish().unwrap();
    }

    let pending = spawn_export_worker(ExportRequest {
        source: ExportSource::ZipEntry {
            zip_path,
            entry_name: "rotated.jpg".to_string(),
        },
        original_format: SrcFormat::Jpeg,
        output_format: ExportFormat::Jpeg95,
        output_dir: temp.path().to_path_buf(),
        basename: "zip-di2".to_string(),
        pixels: single_pixels(solid_image(4, 2, egui::Color32::from_rgb(30, 60, 90))),
        entries: vec![entry("current", 0)],
        include_metadata: true,
    })
    .unwrap();
    let events = collect_events(pending, 5);
    assert_eq!(completed_count(&events), 1);

    let out = std::fs::read(temp.path().join("zip-di2_0.jpg")).unwrap();
    assert_eq!(jpeg_orientation(&out), Some(1));
    let decoded = image::load_from_memory(&out).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (4, 2));
}

#[test]
fn export_animated_webp_fails() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("animated.webp");
    std::fs::write(&src, animated_webp_bytes()).unwrap();

    let pending = spawn_export_worker(ExportRequest {
        source: ExportSource::File { path: src },
        original_format: SrcFormat::Webp,
        output_format: ExportFormat::Webp,
        output_dir: temp.path().to_path_buf(),
        basename: "animated".to_string(),
        pixels: single_pixels(solid_image(2, 2, egui::Color32::WHITE)),
        entries: vec![entry("current", 0)],
        include_metadata: false,
    })
    .unwrap();
    let events = collect_events(pending, 5);
    assert_eq!(completed_count(&events), 0);
    assert_eq!(failed_count(&events), 1);
    assert!(!temp.path().join("animated_0.webp").exists());
}

/// 元 WebP がアニメ判定のため file read を試みるが、読み込みに失敗したケース。
/// silent skip すると output PNG/JPEG で animation check が走らずアニメ WebP が
/// 静止画化される穴があるため、read 失敗は全エントリ失敗にする (Codex review P3)。
#[test]
fn export_webp_source_read_failure_fails_all_entries() {
    let temp = tempfile::tempdir().unwrap();
    // 実在しないパスを渡して file read を失敗させる。
    let missing = temp.path().join("missing.webp");
    let pending = spawn_export_worker(ExportRequest {
        source: ExportSource::File {
            path: missing.clone(),
        },
        original_format: SrcFormat::Webp,
        output_format: ExportFormat::Png,
        output_dir: temp.path().to_path_buf(),
        basename: "missing".to_string(),
        pixels: single_pixels(solid_image(2, 2, egui::Color32::WHITE)),
        entries: vec![entry("a", 0), entry("b", 1)],
        include_metadata: false,
    })
    .unwrap();
    let events = collect_events(pending, 5);
    assert_eq!(completed_count(&events), 0);
    assert_eq!(failed_count(&events), 2);
    assert!(!temp.path().join("missing_0.png").exists());
    assert!(!temp.path().join("missing_1.png").exists());
}

/// 元 WebP がアニメーションのとき、出力形式に関係なく export を拒否する。
/// 旧版は output=WebP のときだけ animation check が走り、PNG/JPEG 出力では
/// 単一フレームを silent に書き出していた (Codex review CONFIRMED の修正)。
#[test]
fn export_animated_webp_rejected_when_output_is_png() {
    let temp = tempfile::tempdir().unwrap();
    let src = temp.path().join("animated.webp");
    std::fs::write(&src, animated_webp_bytes()).unwrap();

    let pending = spawn_export_worker(ExportRequest {
        source: ExportSource::File { path: src },
        original_format: SrcFormat::Webp,
        output_format: ExportFormat::Png,
        output_dir: temp.path().to_path_buf(),
        basename: "anim2png".to_string(),
        pixels: single_pixels(solid_image(2, 2, egui::Color32::WHITE)),
        entries: vec![entry("current", 0)],
        include_metadata: false,
    })
    .unwrap();
    let events = collect_events(pending, 5);
    assert_eq!(completed_count(&events), 0);
    assert_eq!(failed_count(&events), 1);
    assert!(!temp.path().join("anim2png_0.png").exists());
}

#[test]
fn export_orientation_canonical() {
    let temp = tempfile::tempdir().unwrap();
    let src = jpeg_with_app1_payload(&orientation_exif_payload(6));
    let dst = temp.path().join("canonical.jpg");
    let pixels = solid_image(8, 8, egui::Color32::from_rgb(30, 60, 90));

    save_image_with_metadata(
        pixels.as_ref(),
        None,
        Some(&src),
        &dst,
        SrcFormat::Jpeg,
        &SaveOptions::default(),
    )
    .unwrap();

    let out = std::fs::read(dst).unwrap();
    assert_eq!(jpeg_orientation(&out), Some(1));
}

#[test]
fn export_orientation_preserved() {
    let temp = tempfile::tempdir().unwrap();
    let src = jpeg_with_app1_payload(&orientation_exif_payload(6));
    let dst = temp.path().join("preserved.jpg");
    let pixels = solid_image(8, 8, egui::Color32::from_rgb(30, 60, 90));
    let options = SaveOptions {
        caller_applied_orientation: false,
        ..Default::default()
    };

    save_image_with_metadata(
        pixels.as_ref(),
        None,
        Some(&src),
        &dst,
        SrcFormat::Jpeg,
        &options,
    )
    .unwrap();

    let out = std::fs::read(dst).unwrap();
    assert_eq!(jpeg_orientation(&out), Some(6));
}

use super::*;
use crate::{
    audio_album_art::tests::{apic, frame, tag},
    catalog::{AudioArtCached, AudioArtCatalogScope, AudioArtSourceStamp, CatalogAccess},
    settings::CachePolicy,
};
use std::cell::Cell;
use std::io::Cursor;

const RED: [u8; 4] = [240, 15, 25, 255];
const BLUE: [u8; 4] = [15, 25, 240, 255];
const GREEN: [u8; 4] = [15, 240, 25, 255];

fn png(color: [u8; 4], width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
        width,
        height,
        image::Rgba(color),
    ))
    .write_to(&mut bytes, image::ImageFormat::Png)
    .unwrap();
    bytes.into_inner()
}

fn mp3(pictures: &[(&[u8], u8)]) -> Vec<u8> {
    let mut frames = Vec::new();
    for (bytes, kind) in pictures {
        frames.extend(frame(&apic(bytes, *kind), 4, 0));
    }
    let mut bytes = tag(&frames, 4, 0);
    bytes.extend(b"audio payload");
    bytes
}

fn options(cache_dir: &Path, policy: CachePolicy) -> AudioThumbnailOptions {
    AudioThumbnailOptions {
        thumb_px: 64,
        thumb_quality: 90.0,
        cache_dir: cache_dir.to_path_buf(),
        cache_decision: CacheDecision {
            policy,
            threshold_ms: 0,
            size_threshold: u64::MAX,
            webp_always: false,
            pdf_always: false,
            zip_always: false,
        },
        raw_executor: None,
    }
}

fn with_sidecar_decoder(mut options: AudioThumbnailOptions) -> AudioThumbnailOptions {
    options.raw_executor = Some(Arc::new(crate::raw::RawDevelopExecutor::new(1).unwrap()));
    options
}

fn generate_pixels(
    path: &Path,
    sidecar: Option<&Path>,
    options: &AudioThumbnailOptions,
    policy: LoadSourcePolicy,
) -> Option<AudioThumbnailPixels> {
    generate_with_options(
        path,
        sidecar,
        options,
        policy,
        CatalogAccess::for_cache_dir(&options.cache_dir).admit(),
        32,
        &|| false,
    )
    .unwrap()
}

fn cached(path: &Path, options: &AudioThumbnailOptions) -> AudioArtCached {
    let scope = AudioArtCatalogScope::new(path.parent().unwrap());
    crate::catalog::lookup_audio_art(
        &options.cache_dir,
        &scope,
        &scope.key_for(path),
        AudioArtSourceStamp::read(path).unwrap(),
        CatalogAccess::for_cache_dir(&options.cache_dir).admit(),
    )
    .unwrap()
}

fn catalog_path(path: &Path, options: &AudioThumbnailOptions) -> PathBuf {
    crate::catalog::db_path_for(&options.cache_dir, path.parent().unwrap())
}

fn assert_color(pixels: &AudioThumbnailPixels, expected: [u8; 4]) {
    assert_eq!(
        pixels.image.pixels[0],
        egui::Color32::from_rgba_unmultiplied(expected[0], expected[1], expected[2], expected[3])
    );
}

#[test]
fn same_name_sidecar_beats_both_embedded_pixels_and_cached_absence() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("song.mp3");
    let sidecar = tmp.path().join("song.png");
    let red = png(RED, 3, 2);
    std::fs::write(&path, mp3(&[(&red, 3)])).unwrap();
    let options = with_sidecar_decoder(options(&tmp.path().join("cache"), CachePolicy::Always));
    let embedded = generate_pixels(&path, None, &options, LoadSourcePolicy::CacheOrSource).unwrap();
    assert_color(&embedded, RED);
    std::fs::write(&sidecar, png(BLUE, 5, 4)).unwrap();
    let selected = generate_pixels(
        &path,
        Some(&sidecar),
        &options,
        LoadSourcePolicy::CacheOrSource,
    )
    .unwrap();
    assert_color(&selected, BLUE);
    assert!(matches!(
        selected.origin,
        ThumbLoadOrigin::SourceGenerated { .. }
    ));

    std::fs::write(&path, mp3(&[])).unwrap();
    assert!(generate_pixels(&path, None, &options, LoadSourcePolicy::SourceOnly).is_none());
    assert!(matches!(cached(&path, &options), AudioArtCached::NoArt));
    let selected = generate_pixels(
        &path,
        Some(&sidecar),
        &options,
        LoadSourcePolicy::CacheOrSource,
    )
    .unwrap();
    assert_color(&selected, BLUE);
    // Sidecar pixels must not replace the embedded-art absence row.
    assert!(matches!(cached(&path, &options), AudioArtCached::NoArt));
}

#[test]
fn corrupted_or_deleted_selected_sidecar_falls_back_to_embedded_art() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("song.mp3");
    let sidecar = tmp.path().join("song.png");
    let red = png(RED, 3, 2);
    std::fs::write(&path, mp3(&[(&red, 3)])).unwrap();
    let options = with_sidecar_decoder(options(&tmp.path().join("cache"), CachePolicy::Off));
    std::fs::write(&sidecar, b"not an image").unwrap();
    let pixels = generate_pixels(
        &path,
        Some(&sidecar),
        &options,
        LoadSourcePolicy::SourceOnly,
    )
    .unwrap();
    assert_color(&pixels, RED);
    std::fs::remove_file(&sidecar).unwrap();
    let pixels = generate_pixels(
        &path,
        Some(&sidecar),
        &options,
        LoadSourcePolicy::SourceOnly,
    )
    .unwrap();
    assert_color(&pixels, RED);
    assert!(!catalog_path(&path, &options).exists());
}

#[test]
fn sidecar_changes_are_visible_with_unchanged_audio_stamp() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("song.mp3");
    let sidecar = tmp.path().join("song.png");
    std::fs::write(&path, mp3(&[])).unwrap();
    let original = AudioArtSourceStamp::read(&path).unwrap();
    let options = with_sidecar_decoder(options(&tmp.path().join("cache"), CachePolicy::Always));
    std::fs::write(&sidecar, png(BLUE, 3, 2)).unwrap();
    assert_color(
        &generate_pixels(
            &path,
            Some(&sidecar),
            &options,
            LoadSourcePolicy::CacheOrSource,
        )
        .unwrap(),
        BLUE,
    );
    std::fs::write(&sidecar, png(GREEN, 2, 3)).unwrap();
    assert_color(
        &generate_pixels(
            &path,
            Some(&sidecar),
            &options,
            LoadSourcePolicy::CacheOrSource,
        )
        .unwrap(),
        GREEN,
    );
    assert_eq!(AudioArtSourceStamp::read(&path).unwrap(), original);
    assert!(matches!(cached(&path, &options), AudioArtCached::Miss));
}

#[test]
fn non_mp3_uses_sidecar_but_never_attempts_embedded_id3_or_persists_absence() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("song.flac");
    let sidecar = tmp.path().join("song.png");
    let red = png(RED, 3, 2);
    // Deliberately looks like an MP3 tag: the format contract must still exclude it.
    std::fs::write(&path, mp3(&[(&red, 3)])).unwrap();
    let options = with_sidecar_decoder(options(&tmp.path().join("cache"), CachePolicy::Always));
    assert!(generate_pixels(&path, None, &options, LoadSourcePolicy::CacheOrSource).is_none());
    std::fs::write(&sidecar, png(BLUE, 3, 2)).unwrap();
    assert_color(
        &generate_pixels(
            &path,
            Some(&sidecar),
            &options,
            LoadSourcePolicy::CacheOrSource,
        )
        .unwrap(),
        BLUE,
    );
    assert!(!catalog_path(&path, &options).exists());
}

#[test]
fn real_image_decoding_selects_first_valid_front_then_first_valid_other_picture() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("song.mp3");
    let red = png(RED, 3, 2);
    let blue = png(BLUE, 4, 3);
    let green = png(GREEN, 2, 4);
    let options = options(&tmp.path().join("cache"), CachePolicy::Off);
    std::fs::write(
        &path,
        mp3(&[(&red, 4), (b"corrupted PNG", 3), (&blue, 3), (&green, 3)]),
    )
    .unwrap();
    let pixels = generate_pixels(&path, None, &options, LoadSourcePolicy::SourceOnly).unwrap();
    assert_color(&pixels, BLUE);
    assert_eq!(pixels.source_dims, (4, 3));
    std::fs::write(&path, mp3(&[(b"invalid", 3), (&red, 4), (&green, 0)])).unwrap();
    assert_color(
        &generate_pixels(&path, None, &options, LoadSourcePolicy::SourceOnly).unwrap(),
        RED,
    );
}

#[test]
fn cache_policy_controls_both_positive_and_negative_storage() {
    for policy in [CachePolicy::Off, CachePolicy::Auto, CachePolicy::Always] {
        for has_art in [false, true] {
            let tmp = tempfile::tempdir().unwrap();
            let path = tmp.path().join("song.mp3");
            let red = png(RED, 3, 2);
            let data = if has_art { mp3(&[(&red, 3)]) } else { mp3(&[]) };
            std::fs::write(&path, data).unwrap();
            // Auto threshold is deliberately zero; this case has a deterministic decision.
            let options = options(&tmp.path().join("cache"), policy);
            let result = generate_pixels(&path, None, &options, LoadSourcePolicy::CacheOrSource);
            assert_eq!(result.is_some(), has_art);
            match (policy, has_art, cached(&path, &options)) {
                (CachePolicy::Off, _, AudioArtCached::Miss)
                | (CachePolicy::Auto | CachePolicy::Always, true, AudioArtCached::Pixels(_))
                | (CachePolicy::Auto | CachePolicy::Always, false, AudioArtCached::NoArt) => {}
                _ => panic!("cache policy persisted the wrong terminal result"),
            }
            if policy == CachePolicy::Off {
                assert!(!catalog_path(&path, &options).exists());
            }
        }
    }
}

#[test]
fn auto_not_admitted_keeps_no_art_terminal_without_creating_a_catalog() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("song.mp3");
    std::fs::write(&path, mp3(&[])).unwrap();
    let mut options = options(&tmp.path().join("cache"), CachePolicy::Auto);
    options.cache_decision.threshold_ms = u32::MAX;
    options.cache_decision.size_threshold = u64::MAX;
    assert!(generate_pixels(&path, None, &options, LoadSourcePolicy::CacheOrSource).is_none());
    assert!(matches!(cached(&path, &options), AudioArtCached::Miss));
    assert!(!catalog_path(&path, &options).exists());
}

#[test]
fn cache_hit_preserves_source_dimensions_and_size_only_changes_invalidate_it() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("song.mp3");
    let red = png(RED, 3, 2);
    let blue = png(BLUE, 7, 4);
    std::fs::write(&path, mp3(&[(&red, 3)])).unwrap();
    let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
    let before = AudioArtSourceStamp::read(&path).unwrap();
    let options = options(&tmp.path().join("cache"), CachePolicy::Always);
    let first = generate_pixels(&path, None, &options, LoadSourcePolicy::CacheOrSource).unwrap();
    assert!(matches!(
        first.origin,
        ThumbLoadOrigin::SourceGenerated { .. }
    ));
    let hit = generate_pixels(&path, None, &options, LoadSourcePolicy::CacheOrSource).unwrap();
    assert!(matches!(hit.origin, ThumbLoadOrigin::UpgradeableCache));
    assert_eq!(hit.source_dims, (3, 2));

    let mut replacement = mp3(&[(&blue, 3)]);
    replacement.extend(vec![0; before.file_size as usize + 1]);
    std::fs::write(&path, replacement).unwrap();
    std::fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    let after = AudioArtSourceStamp::read(&path).unwrap();
    assert_eq!(after.mtime_secs, before.mtime_secs);
    assert_ne!(after.file_size, before.file_size);
    assert!(matches!(cached(&path, &options), AudioArtCached::Miss));
    let fresh = generate_pixels(&path, None, &options, LoadSourcePolicy::CacheOrSource).unwrap();
    assert_color(&fresh, BLUE);
    assert_eq!(fresh.source_dims, (7, 4));
    assert!(matches!(
        fresh.origin,
        ThumbLoadOrigin::SourceGenerated { .. }
    ));
}

#[test]
fn source_only_bypasses_matching_negative_and_replaces_it_with_pixels() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("song.mp3");
    let red = png(RED, 3, 2);
    std::fs::write(&path, mp3(&[(&red, 3)])).unwrap();
    let options = options(&tmp.path().join("cache"), CachePolicy::Always);
    let scope = AudioArtCatalogScope::new(path.parent().unwrap());
    crate::catalog::save_audio_art_absence_with_cancel_check(
        &options.cache_dir,
        &scope,
        &scope.key_for(&path),
        AudioArtSourceStamp::read(&path).unwrap(),
        CatalogAccess::for_cache_dir(&options.cache_dir).admit(),
        &|| false,
    )
    .unwrap();
    assert!(generate_pixels(&path, None, &options, LoadSourcePolicy::CacheOrSource).is_none());
    let pixels = generate_pixels(&path, None, &options, LoadSourcePolicy::SourceOnly).unwrap();
    assert_color(&pixels, RED);
    assert!(matches!(cached(&path, &options), AudioArtCached::Pixels(_)));
}

#[test]
fn cache_only_miss_never_extracts_a_valid_picture_or_creates_schema() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("song.mp3");
    let red = png(RED, 3, 2);
    std::fs::write(&path, mp3(&[(&red, 3)])).unwrap();
    let options = options(&tmp.path().join("cache"), CachePolicy::Always);
    let admission = CatalogAccess::for_cache_dir(&options.cache_dir).admit();
    let result = generate_with_options(
        &path,
        None,
        &options,
        LoadSourcePolicy::CacheOnly,
        admission,
        32,
        &|| false,
    );
    assert!(matches!(result, Err(AudioThumbnailError::Failed(_))));
    assert!(!catalog_path(&path, &options).exists());

    // A released catalog lacking the additional absence table stays unchanged on miss.
    let db = crate::catalog::CatalogDb::open(&options.cache_dir, path.parent().unwrap()).unwrap();
    drop(db);
    let result = generate_with_options(
        &path,
        None,
        &options,
        LoadSourcePolicy::CacheOnly,
        admission,
        32,
        &|| false,
    );
    assert!(matches!(result, Err(AudioThumbnailError::Failed(_))));
    let conn = rusqlite::Connection::open(catalog_path(&path, &options)).unwrap();
    let tables: i64 = conn
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='audio_art_absence'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0);
}

#[test]
fn cancellation_during_extraction_never_persists_a_negative_result() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("song.mp3");
    let noise = vec![0x41; 256 * 1024];
    std::fs::write(&path, mp3(&[(&noise, 3)])).unwrap();
    let options = options(&tmp.path().join("cache"), CachePolicy::Always);
    let checks = Cell::new(0);
    let result = generate_with_options(
        &path,
        None,
        &options,
        LoadSourcePolicy::SourceOnly,
        CatalogAccess::for_cache_dir(&options.cache_dir).admit(),
        32,
        &|| {
            checks.set(checks.get() + 1);
            checks.get() >= 5
        },
    );
    assert!(matches!(result, Err(AudioThumbnailError::Canceled)));
    assert_eq!(checks.get(), 5);
    assert!(!catalog_path(&path, &options).exists());
}

#[test]
fn deleted_cache_rejects_old_and_display_only_writes_but_keeps_display_results() {
    let tmp = tempfile::tempdir().unwrap();
    let path = tmp.path().join("song.mp3");
    let no_art = tmp.path().join("empty.mp3");
    let red = png(RED, 3, 2);
    std::fs::write(&path, mp3(&[(&red, 3)])).unwrap();
    std::fs::write(&no_art, mp3(&[])).unwrap();
    let options = options(&tmp.path().join("cache"), CachePolicy::Always);
    let access = CatalogAccess::for_cache_dir(&options.cache_dir);
    let old = access.admit();
    assert!(generate_pixels(&path, None, &options, LoadSourcePolicy::SourceOnly).is_some());
    let deleting = access
        .begin_delete(crate::catalog::CatalogDeleteOperation::All)
        .unwrap();
    let display_only = access.admit();
    deleting.retire_connections();
    assert_eq!(
        crate::catalog::delete_all_cache_under_delete(&options.cache_dir).deleted,
        1
    );
    assert!(
        generate_with_options(
            &no_art,
            None,
            &options,
            LoadSourcePolicy::SourceOnly,
            display_only,
            32,
            &|| false,
        )
        .unwrap()
        .is_none()
    );
    assert!(!catalog_path(&path, &options).exists());
    drop(deleting);
    let visible = generate_with_options(
        &path,
        None,
        &options,
        LoadSourcePolicy::SourceOnly,
        old,
        32,
        &|| false,
    )
    .unwrap()
    .unwrap();
    assert_color(&visible, RED);
    assert!(!catalog_path(&path, &options).exists());
    assert!(generate_pixels(&path, None, &options, LoadSourcePolicy::SourceOnly).is_some());
    assert!(matches!(cached(&path, &options), AudioArtCached::Pixels(_)));
}

#[test]
fn jpeg_bytes_and_exif_orientation_are_decoded_without_trusting_apic_mime() {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        12,
        8,
        image::Rgb([20, 100, 220]),
    ))
    .write_to(&mut bytes, image::ImageFormat::Jpeg)
    .unwrap();
    let jpeg = bytes.into_inner();
    let mut oriented = jpeg[..2].to_vec();
    let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    oriented.extend([0xff, 0xe1]);
    oriented.extend(((exif.len() + 2) as u16).to_be_bytes());
    oriented.extend(exif);
    oriented.extend(&jpeg[2..]);
    let (image, dims) = decode_picture(&oriented, 64).unwrap();
    assert_eq!(dims, (8, 12));
    assert_eq!((image.width(), image.height()), (8, 12));
}

#[test]
fn non_jpeg_png_picture_is_not_a_thumbnail_candidate() {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::ImageRgb8(image::RgbImage::from_pixel(
        3,
        2,
        image::Rgb([20, 100, 220]),
    ))
    .write_to(&mut bytes, image::ImageFormat::Bmp)
    .unwrap();
    assert!(decode_picture(&bytes.into_inner(), 64).is_none());
}

#[test]
fn oversized_png_dimensions_are_rejected_before_pixel_decode() {
    let mut bytes = png(RED, 8, 8);
    bytes[16..20].copy_from_slice(&40_000_001u32.to_be_bytes());
    bytes[20..24].copy_from_slice(&1u32.to_be_bytes());
    let mut crc = u32::MAX;
    for byte in &bytes[12..29] {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb88320u32.wrapping_mul(crc & 1));
        }
    }
    bytes[29..33].copy_from_slice(&(!crc).to_be_bytes());
    assert!(decode_picture(&bytes, 64).is_none());
}

use super::*;
use std::io::Write;

fn item(path: &Path, subresource: Option<&str>) -> ItemRef {
    ItemRef {
        path: path.to_string_lossy().into_owned(),
        subresource: subresource.map(str::to_owned),
    }
}

fn context(fixture: &super::super::tests::Fixture) -> ReadContext {
    ReadContext::new(
        &fixture.handlers,
        (
            PermissionSnapshot {
                level: AgentLevel::ReadOnly,
                thumbnails: true,
                gps: false,
                ai_captions: false,
            },
            0,
        ),
        Arc::new(AtomicBool::new(false)),
    )
}

fn call(
    fixture: &super::super::tests::Fixture,
    items: Vec<ItemRef>,
    side: Option<u32>,
) -> ThumbnailResponse {
    let Payload::Thumbnails(response) = fixture
        .handlers
        .get_thumbnail(items, side, None, &context(fixture))
        .unwrap()
    else {
        panic!("expected thumbnails");
    };
    response
}

fn row(response: &ThumbnailResponse, index: usize) -> &ThumbnailItem {
    match &response.page.items[index] {
        PageRow::Value(value) => value,
        PageRow::TooLong(_) => panic!("normal identity must fit"),
    }
}

fn png(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(width, height)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn zip_bytes(name: &str, bytes: &[u8]) -> Vec<u8> {
    let mut zip = zip::ZipWriter::new(Cursor::new(Vec::new()));
    zip.start_file(
        name,
        zip::write::SimpleFileOptions::default().compression_method(zip::CompressionMethod::Stored),
    )
    .unwrap();
    zip.write_all(bytes).unwrap();
    zip.finish().unwrap().into_inner()
}

fn cp932_zip(placeholder: &str, actual: &str, bytes: &[u8]) -> (Vec<u8>, String) {
    let mut bytes = zip_bytes(placeholder, bytes);
    let encoded = encoding_rs::SHIFT_JIS.encode(actual).0;
    assert_eq!(encoded.len(), placeholder.len());
    let indices: Vec<_> = bytes
        .windows(placeholder.len())
        .enumerate()
        .filter_map(|(i, b)| (b == placeholder.as_bytes()).then_some(i))
        .collect();
    assert_eq!(indices.len(), 2);
    for i in indices {
        bytes[i..i + placeholder.len()].copy_from_slice(&encoded);
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let legacy = archive.by_index_raw(0).unwrap().name().replace('\\', "/");
    (bytes, legacy)
}

fn jpeg_with_orientation() -> Vec<u8> {
    let mut jpeg = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(80, 40)
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
    // One little-endian TIFF IFD: Orientation=6 (clockwise quarter-turn).
    let exif = b"Exif\0\0II\x2a\0\x08\0\0\0\x01\0\x12\x01\x03\0\x01\0\0\0\x06\0\0\0\0\0\0\0";
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
    bytes.extend_from_slice(&((exif.len() + 2) as u16).to_be_bytes());
    bytes.extend_from_slice(exif);
    bytes.extend_from_slice(&jpeg.into_inner()[2..]);
    bytes
}

#[test]
fn jpeg_exif_is_applied_once_then_readonly_user_rotation() {
    let fixture = super::super::tests::Fixture::new();
    let path = fixture._dir.path().join("oriented.jpg");
    let jpeg = jpeg_with_orientation();
    let decoded = decode_bytes(&jpeg, "oriented.jpg", 256, &AtomicBool::new(false)).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (40, 80));
    std::fs::write(&path, jpeg).unwrap();
    crate::rotation_db::RotationDb::open()
        .unwrap()
        .set(&path, crate::rotation_db::Rotation::Cw90)
        .unwrap();
    let before = std::fs::read(crate::rotation_db::RotationDb::db_path()).unwrap();
    let response = call(&fixture, vec![item(&path, None)], None);
    assert_eq!(row(&response, 0).status, ThumbnailStatus::Ok);
    assert_eq!(
        row(&response, 0).dimensions,
        Some(Dimensions {
            width: 80,
            height: 40,
            unit: DimensionUnit::Pixels,
        })
    );
    assert_eq!(response.images[0].item_index, 0);
    let image = image::load_from_memory(&response.images[0].webp).unwrap();
    assert_eq!((image.width(), image.height()), (80, 40));
    assert_eq!(
        std::fs::read(crate::rotation_db::RotationDb::db_path()).unwrap(),
        before
    );
}

#[test]
fn large_jpeg_file_and_exact_zip_member_use_bounded_dct_before_full_size_estimate() {
    fn segment(jpeg: &mut Vec<u8>, marker: u8, data: &[u8]) {
        jpeg.extend_from_slice(&[0xff, marker]);
        jpeg.extend_from_slice(&((data.len() + 2) as u16).to_be_bytes());
        jpeg.extend_from_slice(data);
    }

    // A complete 12,000 x 8,000 baseline grayscale JPEG, with no full-size
    // raster allocation while creating the fixture. Each constant-gray block
    // has DC difference zero and AC end-of-block, each encoded by a zero bit.
    let mut jpeg = vec![0xff, 0xd8];
    let mut quantization = [1; 65];
    quantization[0] = 0;
    segment(&mut jpeg, 0xdb, &quantization);
    let mut frame = vec![8];
    frame.extend_from_slice(&8000_u16.to_be_bytes());
    frame.extend_from_slice(&12000_u16.to_be_bytes());
    frame.extend_from_slice(&[1, 1, 0x11, 0]);
    segment(&mut jpeg, 0xc0, &frame);
    let mut huffman = Vec::new();
    for table in [0, 0x10] {
        huffman.extend_from_slice(&[table, 1]);
        huffman.extend_from_slice(&[0; 15]);
        huffman.push(0);
    }
    segment(&mut jpeg, 0xc4, &huffman);
    segment(&mut jpeg, 0xda, &[1, 1, 0, 0, 63, 0]);
    jpeg.resize(jpeg.len() + (12000 / 8) * (8000 / 8) / 4, 0);
    jpeg.extend_from_slice(&[0xff, 0xd9]);
    assert!(jpeg.len() < 512 * 1024);
    assert_eq!(
        super::super::item_info::bytes_dimensions(&jpeg, "large.jpg"),
        Some((12000, 8000))
    );
    assert!(12000_u64 * 8000 * 4 > MAX_DECODED_BYTES);

    let fixture = super::super::tests::Fixture::new();
    let path = fixture._dir.path().join("large.jpg");
    let archive = fixture._dir.path().join("large.cbz");
    std::fs::write(&path, &jpeg).unwrap();
    std::fs::write(&archive, zip_bytes("pages/large.jpg", &jpeg)).unwrap();
    let response = call(
        &fixture,
        vec![item(&path, None), item(&archive, Some("pages/large.jpg"))],
        None,
    );
    assert_eq!(response.images.len(), 2);
    for (index, attachment) in response.images.iter().enumerate() {
        let value = row(&response, index);
        assert_eq!(value.status, ThumbnailStatus::Ok);
        assert_eq!(attachment.item_index, index);
        let dimensions = value.dimensions.as_ref().unwrap();
        assert!((1..=DEFAULT_THUMBNAIL_SIDE).contains(&dimensions.width));
        assert!((1..=DEFAULT_THUMBNAIL_SIDE).contains(&dimensions.height));
        assert!((f64::from(dimensions.width) / f64::from(dimensions.height) - 1.5).abs() < 0.01);
        let decoded = image::load_from_memory(&attachment.webp).unwrap();
        assert_eq!(
            (decoded.width(), decoded.height()),
            (dimensions.width, dimensions.height)
        );
    }
}

#[test]
fn png_file_and_exact_zip_member_do_not_read_catalog_or_create_metadata_stores() {
    fn inventory(root: &Path) -> Vec<(std::path::PathBuf, Option<Vec<u8>>)> {
        fn visit(
            root: &Path,
            directory: &Path,
            rows: &mut Vec<(std::path::PathBuf, Option<Vec<u8>>)>,
        ) {
            for entry in std::fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                let relative = path.strip_prefix(root).unwrap().to_owned();
                if entry.file_type().unwrap().is_dir() {
                    rows.push((relative, None));
                    visit(root, &path, rows);
                } else {
                    rows.push((relative, Some(std::fs::read(path).unwrap())));
                }
            }
        }
        let mut rows = Vec::new();
        visit(root, root, &mut rows);
        rows.sort_by(|a, b| a.0.cmp(&b.0));
        rows
    }
    let fixture = super::super::tests::Fixture::new();
    let path = fixture._dir.path().join("page.png");
    let archive = fixture._dir.path().join("pages.cbz");
    let pixels = png(700, 350);
    std::fs::write(&path, &pixels).unwrap();
    std::fs::write(&archive, zip_bytes("pages/exact.png", &pixels)).unwrap();
    let cache = crate::catalog::default_cache_dir();
    assert_eq!(cache, fixture._dir.path().join("cache"));
    assert!(!cache.exists());
    // With no cache present, neither source may create a catalog or even its
    // parent cache directory.
    let response = call(
        &fixture,
        vec![item(&path, None), item(&archive, Some("pages/exact.png"))],
        None,
    );
    assert!(!cache.exists());
    assert_eq!(response.images.len(), 2);
    for (index, image) in response.images.iter().enumerate() {
        assert_eq!(image.item_index, index);
        assert_eq!(row(&response, index).status, ThumbnailStatus::Ok);
        assert_eq!(
            row(&response, index).dimensions,
            Some(Dimensions {
                width: 256,
                height: 128,
                unit: DimensionUnit::Pixels,
            })
        );
    }
    // Poison the real candidate catalog locations the normal image/ZIP loader
    // would consult. Pure path derivation must not open/create a catalog.
    let catalogs = [
        crate::catalog::db_path_for(&cache, path.parent().unwrap()),
        crate::catalog::db_path_for(&cache, &archive),
    ];
    assert_ne!(catalogs[0], catalogs[1]);
    for catalog in &catalogs {
        std::fs::create_dir_all(catalog.parent().unwrap()).unwrap();
        std::fs::write(catalog, b"not a sqlite catalog").unwrap();
    }
    let before = inventory(&cache);
    let uncached = call(
        &fixture,
        vec![item(&path, None), item(&archive, Some("pages/exact.png"))],
        None,
    );
    assert_eq!(uncached, response);
    for db in [
        "conceal.db",
        "rotation.db",
        "pdf_meta.db",
        "adjustment.db",
        "similar.db",
    ] {
        assert!(!fixture._dir.path().join(db).exists(), "created {db}");
    }
    let clamped = call(&fixture, vec![item(&path, None)], Some(10000));
    assert_eq!(row(&clamped, 0).dimensions.as_ref().unwrap().width, 512);
    // Includes directory entries, exact file bytes, and unexpected siblings
    // such as WAL/SHM files, replacement/quarantine files, or new catalogs.
    assert_eq!(inventory(&cache), before);
}

#[test]
fn conceal_file_and_cp932_nested_legacy_keys_are_fail_closed_without_migration() {
    let fixture = super::super::tests::Fixture::new();
    let path = fixture._dir.path().join("page.png");
    std::fs::write(&path, png(7, 5)).unwrap();
    let archive = fixture._dir.path().join("legacy.zip");
    let (inner, legacy_leaf) = cp932_zip("xxxx.png", "日本.png", &png(7, 5));
    let (outer, legacy_outer) = cp932_zip("xxxx.zip", "日本.zip", &inner);
    std::fs::write(&archive, outer).unwrap();
    let legacy_name = format!("{legacy_outer}/{legacy_leaf}");
    let legacy_key = crate::adjustment_db::zip_entry_key(&archive, &legacy_name);
    let current_key = crate::adjustment_db::zip_entry_key(&archive, "日本.zip/日本.png");
    assert_ne!(legacy_key, current_key);
    let conceal = crate::conceal_db::ConcealDb::open().unwrap();
    conceal
        .set(
            &crate::adjustment_db::normalize_path(&path),
            &[true],
            &[],
            1,
            1,
        )
        .unwrap();
    conceal.set(&legacy_key, &[true], &[], 1, 1).unwrap();
    drop(conceal);
    let before = std::fs::read(crate::conceal_db::ConcealDb::db_path()).unwrap();
    let response = call(
        &fixture,
        vec![item(&path, None), item(&archive, Some("日本.zip/日本.png"))],
        None,
    );
    assert!(response.images.is_empty());
    for index in 0..2 {
        assert_eq!(row(&response, index).status, ThumbnailStatus::HiddenByEdit);
    }
    assert_eq!(
        std::fs::read(crate::conceal_db::ConcealDb::db_path()).unwrap(),
        before
    );
    let db = crate::conceal_db::ConcealDb::open_readonly(&crate::conceal_db::ConcealDb::db_path())
        .unwrap();
    assert!(db.get_full_checked(&legacy_key).unwrap().is_some());
    assert!(db.get_full_checked(&current_key).unwrap().is_none());
    // A failed target lookup cannot establish its current/legacy relationship.
    let missing = call(
        &fixture,
        vec![item(&archive, Some("日本.zip/missing.png"))],
        None,
    );
    assert_eq!(row(&missing, 0).status, ThumbnailStatus::Unavailable);
    assert!(missing.images.is_empty());
}

#[test]
fn current_zip_rotation_has_priority_over_cp932_legacy_rotation() {
    let fixture = super::super::tests::Fixture::new();
    let archive = fixture._dir.path().join("legacy.zip");
    let (bytes, legacy) = cp932_zip("xxxx.png", "日本.png", &png(70, 50));
    std::fs::write(&archive, bytes).unwrap();
    let current = crate::adjustment_db::zip_entry_key(&archive, "日本.png");
    let legacy = crate::adjustment_db::zip_entry_key(&archive, &legacy);
    let db = crate::rotation_db::RotationDb::open().unwrap();
    db.set_key(&legacy, crate::rotation_db::Rotation::Cw90)
        .unwrap();
    let response = call(&fixture, vec![item(&archive, Some("日本.png"))], None);
    assert_eq!(row(&response, 0).dimensions.as_ref().unwrap().height, 70);
    db.set_key(&current, crate::rotation_db::Rotation::Cw180)
        .unwrap();
    let response = call(&fixture, vec![item(&archive, Some("日本.png"))], None);
    assert_eq!(row(&response, 0).dimensions.as_ref().unwrap().width, 70);
}

#[test]
fn unreadable_or_corrupt_conceal_database_rejects_entire_request() {
    let fixture = super::super::tests::Fixture::new();
    let path = fixture._dir.path().join("page.png");
    std::fs::write(&path, png(7, 5)).unwrap();
    let conceal = crate::conceal_db::ConcealDb::db_path();
    std::fs::write(&conceal, b"invalid private database").unwrap();
    let result =
        fixture
            .handlers
            .get_thumbnail(vec![item(&path, None)], None, None, &context(&fixture));
    assert_eq!(result.unwrap_err().code, ErrorCode::Internal);
    assert_eq!(std::fs::read(conceal).unwrap(), b"invalid private database");
}

#[test]
fn invalid_arguments_permission_and_cancellation_are_checked_before_io() {
    let fixture = super::super::tests::Fixture::new();
    let input = item(&fixture._dir.path().join("missing.png"), None);
    let mut ctx = context(&fixture);
    ctx.granted.0.thumbnails = false;
    assert_eq!(
        fixture
            .handlers
            .get_thumbnail(vec![input.clone()], None, None, &ctx)
            .unwrap_err()
            .code,
        ErrorCode::Disabled
    );
    ctx.granted.0.thumbnails = true;
    for (items, side) in [
        (vec![], None),
        (vec![input.clone(); 21], None),
        (vec![input.clone()], Some(255)),
    ] {
        assert_eq!(
            fixture
                .handlers
                .get_thumbnail(items, side, None, &ctx)
                .unwrap_err()
                .code,
            ErrorCode::BadRequest
        );
    }
    ctx.cancel.store(true, Ordering::Release);
    assert_eq!(
        fixture
            .handlers
            .get_thumbnail(vec![input], None, None, &ctx)
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    assert!(!crate::conceal_db::ConcealDb::db_path().exists());
}

#[test]
fn unsupported_containers_media_and_custom_formats_never_resolve_representatives() {
    let fixture = super::super::tests::Fixture::new();
    let inputs = [
        "directory",
        "a.zip",
        "a.pdf",
        "a.mp4",
        "a.mp3",
        "a.epub",
        "a.spi_only",
    ]
    .map(|name| item(&fixture._dir.path().join(name), None));
    let response = call(&fixture, inputs.to_vec(), None);
    assert!(response.images.is_empty());
    assert!(
        response
            .page
            .items
            .iter()
            .all(|r| matches!(r, PageRow::Value(v) if v.status == ThumbnailStatus::Unsupported))
    );
    let bad_container = call(
        &fixture,
        vec![item(&fixture._dir.path().join("a.rar"), Some("page.png"))],
        None,
    );
    assert_eq!(row(&bad_container, 0).status, ThumbnailStatus::Unsupported);
}

#[test]
fn animation_uses_first_frame_and_raw_never_falls_back_to_regular_decoder() {
    let mut gif = Vec::new();
    {
        let mut encoder = image::codecs::gif::GifEncoder::new(&mut gif);
        for rgba in [[255, 0, 0, 255], [0, 0, 255, 255]] {
            encoder
                .encode_frame(image::Frame::new(image::RgbaImage::from_pixel(
                    7,
                    5,
                    image::Rgba(rgba),
                )))
                .unwrap();
        }
    }
    let decoded = decode_bytes(&gif, "animated.gif", 256, &AtomicBool::new(false)).unwrap();
    assert_eq!(
        *decoded.to_rgba8().get_pixel(0, 0),
        image::Rgba([255, 0, 0, 255])
    );
    // A valid ordinary image disguised as RAW must not enter image/WIC/Susie
    // fallback or a full-RAW development executor when no preview is available.
    assert_eq!(
        decode_bytes(&png(7, 5), "no-preview.dng", 256, &AtomicBool::new(false)).unwrap_err(),
        ThumbnailStatus::Unavailable
    );
}

#[test]
fn file_zip_input_caps_and_known_header_raster_budget_are_too_large() {
    let fixture = super::super::tests::Fixture::new();
    let large = fixture._dir.path().join("large.png");
    std::fs::File::create(&large)
        .unwrap()
        .set_len(MAX_FILE_BYTES + 1)
        .unwrap();
    let mut archive = zip_bytes("large.png", b"not decoded");
    let central = archive.windows(4).position(|b| b == b"PK\x01\x02").unwrap();
    archive[central + 24..central + 28].copy_from_slice(&(64 * 1024 * 1024 + 1_u32).to_le_bytes());
    archive[22..26].copy_from_slice(&(64 * 1024 * 1024 + 1_u32).to_le_bytes());
    let zip = fixture._dir.path().join("large.zip");
    std::fs::write(&zip, archive).unwrap();
    let response = call(
        &fixture,
        vec![item(&large, None), item(&zip, Some("large.png"))],
        None,
    );
    for index in 0..2 {
        assert_eq!(row(&response, index).status, ThumbnailStatus::TooLarge);
    }
    assert!(response.images.is_empty());
    let mut header = Vec::new();
    let mut encoder = png::Encoder::new(&mut header, 10000, 10000);
    encoder.set_color(png::ColorType::Rgba);
    drop(encoder.write_header().unwrap());
    // IHDR alone is not enough for image::PngDecoder: construction reads up to
    // the first IDAT. This incomplete stream cannot supply known dimensions.
    assert!(super::super::item_info::bytes_dimensions(&header, "large.png").is_none());
    assert_eq!(
        decode_bytes(&header, "large.png", 256, &AtomicBool::new(false)).unwrap_err(),
        ThumbnailStatus::Unavailable
    );
    // Preserve a tiny ordinary PNG's valid chunk sequence, including IDAT, but
    // advertise a large raster in IHDR and recompute its checksum. The header
    // probe can now establish the bound without allocating a 400 MB raster.
    let mut header = png(1, 1);
    assert_eq!(&header[12..16], b"IHDR");
    header[16..20].copy_from_slice(&10000_u32.to_be_bytes());
    header[20..24].copy_from_slice(&10000_u32.to_be_bytes());
    let mut crc = u32::MAX;
    for byte in &header[12..29] {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & (0_u32.wrapping_sub(crc & 1)));
        }
    }
    header[29..33].copy_from_slice(&(!crc).to_be_bytes());
    assert_eq!(
        super::super::item_info::bytes_dimensions(&header, "large.png"),
        Some((10000, 10000))
    );
    assert_eq!(
        decode_bytes(&header, "large.png", 256, &AtomicBool::new(false)).unwrap_err(),
        ThumbnailStatus::TooLarge
    );
    let header_file = fixture._dir.path().join("large-header.png");
    let header_zip = fixture._dir.path().join("header.zip");
    std::fs::write(&header_file, &header).unwrap();
    std::fs::write(&header_zip, zip_bytes("large-header.png", &header)).unwrap();
    let response = call(
        &fixture,
        vec![
            item(&header_file, None),
            item(&header_zip, Some("large-header.png")),
        ],
        None,
    );
    assert!(response.images.is_empty());
    for index in 0..2 {
        assert_eq!(row(&response, index).status, ThumbnailStatus::TooLarge);
    }
    assert_eq!(
        decode_bytes(b"broken", "broken.png", 256, &AtomicBool::new(false)).unwrap_err(),
        ThumbnailStatus::Unavailable
    );
}

fn metadata(name: String) -> ThumbnailItem {
    ThumbnailItem {
        item: ItemRef {
            path: name,
            subresource: None,
        },
        status: ThumbnailStatus::Ok,
        dimensions: Some(Dimensions {
            width: 256,
            height: 256,
            unit: DimensionUnit::Pixels,
        }),
    }
}

#[test]
fn image_and_metadata_budgets_keep_attachments_in_sync_and_cursor_advancing() {
    let args = serde_json::json!(["fixture"]);
    let mut builder = ThumbnailBuilder::new(7, args.clone(), 0);
    for n in 0..4 {
        assert!(
            builder
                .push(
                    metadata(format!("C:/fixture/{n}.png")),
                    Some(vec![0; MAX_THUMBNAIL_IMAGE_BYTES])
                )
                .unwrap()
        );
    }
    assert!(
        !builder
            .push(metadata("C:/fixture/next.png".into()), Some(vec![0]))
            .unwrap()
    );
    let response = builder.response(true).unwrap();
    assert_eq!(response.images.len(), 4);
    assert_eq!(
        decode_cursor(response.page.cursor.as_deref(), "get_thumbnail", &args, 0).unwrap(),
        11
    );
    assert_eq!(
        response.images.iter().map(|i| i.webp.len()).sum::<usize>(),
        MAX_THUMBNAIL_TOTAL_BYTES
    );
    let mut oversized = ThumbnailBuilder::new(0, args.clone(), 0);
    assert!(
        oversized
            .push(
                metadata("C:/fixture/large.png".into()),
                Some(vec![0; MAX_THUMBNAIL_IMAGE_BYTES + 1])
            )
            .unwrap()
    );
    let response = oversized.response(true).unwrap();
    assert_eq!(row(&response, 0).status, ThumbnailStatus::TooLarge);
    assert!(response.images.is_empty());
    assert_eq!(
        decode_cursor(response.page.cursor.as_deref(), "get_thumbnail", &args, 0).unwrap(),
        1
    );
    let mut bounded = ThumbnailBuilder::new(0, args.clone(), 0);
    let mut accepted = 0;
    for n in 0..20 {
        if !bounded
            .push(
                metadata(format!("C:/{}-{n}.png", "escaped\\\"".repeat(300))),
                Some(vec![1]),
            )
            .unwrap()
        {
            break;
        }
        accepted += 1;
    }
    assert!(accepted > 0 && accepted < 20);
    let response = bounded.response(true).unwrap();
    assert_eq!(response.images.len(), accepted);
    assert!(mcp_content_chars(&response.page).unwrap() <= MAX_PAGE_CHARS);
    assert_eq!(
        decode_cursor(response.page.cursor.as_deref(), "get_thumbnail", &args, 0).unwrap(),
        accepted
    );
    let mut too_long = ThumbnailBuilder::new(3, args.clone(), 0);
    assert!(
        too_long
            .push(
                metadata(format!("C:/{}.png", "x".repeat(MAX_ITEM_CHARS))),
                Some(vec![1])
            )
            .unwrap()
    );
    let response = too_long.response(true).unwrap();
    assert!(matches!(response.page.items[0], PageRow::TooLong(_)));
    assert!(response.images.is_empty());
    assert_eq!(
        decode_cursor(response.page.cursor.as_deref(), "get_thumbnail", &args, 0).unwrap(),
        4
    );
}

#[test]
fn continuation_starts_at_input_offset_without_reopening_earlier_sources() {
    let fixture = super::super::tests::Fixture::new();
    let first = fixture._dir.path().join("first.png");
    let second = fixture._dir.path().join("second.png");
    std::fs::write(&second, png(7, 5)).unwrap();
    let inputs = vec![item(&first, None), item(&second, None)];
    let normalized: Vec<_> = inputs
        .iter()
        .map(|i| {
            serde_json::json!([
                super::super::input_path::cursor_key(&i.path).unwrap(),
                i.subresource.as_ref().map(|s| s.replace('\\', "/"))
            ])
        })
        .collect();
    let args = serde_json::json!([normalized, DEFAULT_THUMBNAIL_SIDE, "input_order"]);
    let cursor = encode_cursor("get_thumbnail", args, 1, 0);
    let Payload::Thumbnails(response) = fixture
        .handlers
        .get_thumbnail(
            inputs.clone(),
            None,
            Some(cursor.clone()),
            &context(&fixture),
        )
        .unwrap()
    else {
        panic!();
    };
    assert_eq!(response.page.items.len(), 1);
    assert_eq!(row(&response, 0).item, inputs[1]);
    assert_eq!(row(&response, 0).status, ThumbnailStatus::Ok);
    assert!(response.page.cursor.is_none());
    assert!(
        fixture
            .handlers
            .get_thumbnail(inputs, Some(512), Some(cursor), &context(&fixture))
            .is_err()
    );
}

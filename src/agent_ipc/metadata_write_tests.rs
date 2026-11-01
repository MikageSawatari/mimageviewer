use super::*;
use crate::agent_ipc::{ReadContext, tests::Fixture};
use mimageviewer_agent_ipc::{AgentLevel, PermissionSnapshot};
use std::io::{Cursor, Write};
use std::sync::{
    Arc,
    atomic::{AtomicBool, AtomicUsize, Ordering},
};
use std::time::{Duration, Instant};

fn context(fixture: &Fixture) -> ReadContext {
    ReadContext::new(
        &fixture.handlers,
        PermissionSnapshot {
            level: AgentLevel::Organize,
            thumbnails: false,
            gps: false,
        },
        Arc::new(AtomicBool::new(false)),
    )
}

fn item(path: &Path, member: Option<&str>) -> ItemRef {
    ItemRef {
        path: path.to_string_lossy().into_owned(),
        subresource: member.map(str::to_owned),
    }
}

fn zip_bytes(name: &str, bytes: &[u8]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file(
            name,
            zip::write::SimpleFileOptions::default()
                .compression_method(zip::CompressionMethod::Stored),
        )
        .unwrap();
    writer.write_all(bytes).unwrap();
    writer.finish().unwrap().into_inner()
}

fn cp932_zip(placeholder: &str, actual: &str, content: &[u8]) -> (Vec<u8>, String) {
    let mut bytes = zip_bytes(placeholder, content);
    let encoded = encoding_rs::SHIFT_JIS.encode(actual).0;
    assert_eq!(encoded.len(), placeholder.len());
    let offsets: Vec<_> = bytes
        .windows(placeholder.len())
        .enumerate()
        .filter_map(|(offset, value)| (value == placeholder.as_bytes()).then_some(offset))
        .collect();
    for offset in offsets {
        bytes[offset..offset + placeholder.len()].copy_from_slice(&encoded);
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let legacy = archive.by_index_raw(0).unwrap().name().to_owned();
    (bytes, legacy)
}

#[test]
fn metadata_write_prevalidates_tag_names_and_all_call_bounds() {
    let fixture = Fixture::new();
    let ctx = context(&fixture);
    let input = ItemRef {
        path: "relative/invalid".into(),
        subresource: None,
    };
    for tags in [
        vec![],
        vec!["a".into(); 11],
        vec!["valid".into(), "bad name".into()],
        vec!["x".repeat(65)],
        vec!["##".into()],
    ] {
        let error = prepare(
            Command::AddTags {
                items: vec![input.clone()],
                tags,
            },
            &ctx,
            "client".into(),
        )
        .unwrap_err();
        assert_eq!(error.code, ErrorCode::BadRequest);
    }
    for items in [vec![], vec![input.clone(); 501]] {
        assert_eq!(
            prepare(
                Command::SetRating { items, stars: 3 },
                &ctx,
                "client".into()
            )
            .unwrap_err()
            .code,
            ErrorCode::BadRequest
        );
    }
    assert_eq!(
        prepare(
            Command::SetRating {
                items: vec![input],
                stars: 6
            },
            &ctx,
            "client".into()
        )
        .unwrap_err()
        .code,
        ErrorCode::BadRequest
    );
    assert!(!is_write(&Command::Status));
    assert!(is_write(&Command::AddTags {
        items: vec![],
        tags: vec![]
    }));
}

#[test]
fn metadata_write_prepares_supported_physical_types_and_precise_skips() {
    let fixture = Fixture::new();
    let ctx = context(&fixture);
    let root = fixture._dir.path();
    let expected = [
        ("image.png", RatingItemKind::Image),
        ("movie.mp4", RatingItemKind::Video),
        ("song.mp3", RatingItemKind::Audio),
        ("book.cbz", RatingItemKind::ZipFile),
        ("book.pdf", RatingItemKind::PdfFile),
        ("book.epub", RatingItemKind::PdfFile),
        ("book.rar", RatingItemKind::ConvertibleArchive),
    ];
    let mut inputs = Vec::new();
    for (name, _) in expected {
        let path = root.join(name);
        std::fs::write(&path, b"fixture").unwrap();
        inputs.push(item(&path, None));
    }
    let folder = root.join("folder");
    std::fs::create_dir(&folder).unwrap();
    inputs.push(item(&folder, None));
    inputs.push(item(&root.join("missing.png"), None));
    let unsupported = root.join("file.txt");
    std::fs::write(&unsupported, b"fixture").unwrap();
    inputs.push(item(&unsupported, None));
    inputs.push(ItemRef {
        path: "relative".into(),
        subresource: None,
    });
    let PreparedWrite::Ratings(prepared) = prepare(
        Command::SetRating {
            items: inputs,
            stars: 4,
        },
        &ctx,
        "client".into(),
    )
    .unwrap() else {
        panic!("ratings expected")
    };
    for (index, (_, kind)) in expected.iter().enumerate() {
        let target = prepared.items[index].target.as_ref().unwrap();
        assert_eq!(target.meta.kind, *kind);
        assert_eq!(
            target.key,
            crate::adjustment_db::normalize_path(Path::new(&prepared.items[index].input.path))
        );
        assert!(target.legacy_key.is_none());
        assert_eq!(target.xmp_target.is_some(), *kind == RatingItemKind::Image);
    }
    assert_eq!(
        prepared.items[6]
            .target
            .as_ref()
            .unwrap()
            .meta
            .archive_format
            .as_deref(),
        Some("rar")
    );
    assert_eq!(
        prepared.items[7].target.as_ref().unwrap().meta.kind,
        RatingItemKind::Folder
    );
    assert_eq!(
        prepared.items[8].target.as_ref().unwrap_err(),
        &MetadataSkipReason::NotFound
    );
    assert_eq!(
        prepared.items[9].target.as_ref().unwrap_err(),
        &MetadataSkipReason::UnsupportedType
    );
    assert_eq!(
        prepared.items[10].target.as_ref().unwrap_err(),
        &MetadataSkipReason::InvalidPath
    );
    assert_eq!(prepared.stars, 4);
    assert_eq!(prepared.client_name, "client");
}

#[test]
fn metadata_write_tags_normalize_and_preserve_compiled_book_sidecar_policy() {
    let fixture = Fixture::new();
    let root = fixture._dir.path();
    let books = root.join("books");
    let book = books.join("one");
    std::fs::create_dir_all(&book).unwrap();
    crate::settings_db::with_db_result(|db| {
        let mut settings = db.load_into_settings()?;
        settings.book_root = Some(books.clone());
        db.save_full(&settings)
    })
    .unwrap();
    let compiled = book.join("page.png");
    let regular = root.join("normal.png");
    for path in [&compiled, &regular] {
        std::fs::write(path, b"fixture").unwrap();
    }
    let PreparedWrite::Tags(prepared) = prepare(
        Command::AddTags {
            items: vec![
                item(&compiled, None),
                item(&regular, None),
                item(&book, None),
                item(&regular, Some("page_0")),
            ],
            tags: vec!["  #ＦＡＴＥ ".into(), "猫".into()],
        },
        &context(&fixture),
        "client".into(),
    )
    .unwrap() else {
        panic!("tags expected")
    };
    assert_eq!(prepared.tags, ["FATE", "猫"]);
    assert!(prepared.add);
    assert!(
        prepared.items[0]
            .target
            .as_ref()
            .unwrap()
            .tag_sidecar
            .is_none()
    );
    assert!(
        prepared.items[1]
            .target
            .as_ref()
            .unwrap()
            .tag_sidecar
            .is_some()
    );
    assert!(
        prepared.items[2]
            .target
            .as_ref()
            .unwrap()
            .tag_sidecar
            .is_none()
    );
    assert_eq!(
        prepared.items[3].target.as_ref().unwrap_err(),
        &MetadataSkipReason::UnsupportedSubresource
    );
    let PreparedWrite::Ratings(prepared) = prepare(
        Command::SetRating {
            items: vec![item(&compiled, None)],
            stars: 1,
        },
        &context(&fixture),
        "client".into(),
    )
    .unwrap() else {
        panic!("ratings expected")
    };
    let target = prepared.items[0].target.as_ref().unwrap();
    assert!(target.compiled_book_page);
    assert!(target.xmp_target.is_none());
    assert!(!root.join("mimageviewer.dat").exists());
}

#[test]
fn metadata_write_zip_preparation_is_bounded_and_keeps_both_keys_without_migration() {
    let fixture = Fixture::new();
    let ctx = context(&fixture);
    let root = fixture._dir.path();
    let path = root.join("legacy.zip");
    let (inner, legacy_leaf) = cp932_zip("xxxx.png", "日本.png", b"image");
    let (outer, legacy_outer) = cp932_zip("xxxx.zip", "日本.zip", &inner);
    std::fs::write(&path, &outer).unwrap();
    let canonical = crate::adjustment_db::zip_entry_key(&path, "日本.zip/日本.png");
    let legacy =
        crate::adjustment_db::zip_entry_key(&path, &format!("{legacy_outer}/{legacy_leaf}"));
    let db = crate::rating_db::RatingDb::open().unwrap();
    db.set(&legacy, 5).unwrap();
    let PreparedWrite::Ratings(prepared) = prepare(
        Command::SetRating {
            items: vec![
                item(&path, Some("日本.zip/日本.png")),
                item(&path, Some("missing.png")),
                item(&path, Some("../bad.png")),
                item(&path, Some("file.txt")),
            ],
            stars: 0,
        },
        &ctx,
        "client".into(),
    )
    .unwrap() else {
        panic!("ratings expected")
    };
    let target = prepared.items[0].target.as_ref().unwrap();
    assert_eq!(target.key, canonical);
    assert_eq!(target.legacy_key.as_deref(), Some(legacy.as_str()));
    assert_eq!(target.meta.kind, RatingItemKind::ZipImage);
    assert_eq!(target.meta.entry_name.as_deref(), Some("日本.zip/日本.png"));
    assert_eq!(db.get(&legacy), 5);
    assert_eq!(db.get(&canonical), 0);
    assert_eq!(std::fs::read(&path).unwrap(), outer);
    assert_eq!(
        prepared.items[1].target.as_ref().unwrap_err(),
        &MetadataSkipReason::NotFound
    );
    assert_eq!(
        prepared.items[2].target.as_ref().unwrap_err(),
        &MetadataSkipReason::UnsupportedSubresource
    );
    assert_eq!(
        prepared.items[3].target.as_ref().unwrap_err(),
        &MetadataSkipReason::UnsupportedSubresource
    );
    let mut large = zip_bytes("large.png", b"small");
    let central = large
        .windows(4)
        .position(|value| value == b"PK\x01\x02")
        .unwrap();
    let declared = (64_u32 * 1024 * 1024 + 1).to_le_bytes();
    large[central + 24..central + 28].copy_from_slice(&declared);
    large[22..26].copy_from_slice(&declared);
    let path = root.join("large.zip");
    std::fs::write(&path, large).unwrap();
    let identity = prepare_rating(&item(&path, Some("large.png")), root, &ctx)
        .unwrap()
        .unwrap();
    assert_eq!(identity.meta.entry_name.as_deref(), Some("large.png"));
    assert!(
        crate::agent_ipc::item_info::read_zip(&path, "large.png", &ctx.cancel)
            .unwrap()
            .bytes
            .is_none(),
        "normal payload reads retain the 64 MiB leaf bound"
    );
    // Corrupt payload bytes remain unread by the identity path. The same name
    // lookup on the ordinary read path must attempt them and fail CRC checking.
    let mut unread = zip_bytes("unread.png", b"payload");
    let payload = unread
        .windows(7)
        .position(|value| value == b"payload")
        .unwrap();
    unread[payload] ^= 1;
    let unread_path = root.join("unread.zip");
    std::fs::write(&unread_path, unread).unwrap();
    assert!(
        prepare_rating(&item(&unread_path, Some("unread.png")), root, &ctx)
            .unwrap()
            .is_ok()
    );
    assert!(
        crate::agent_ipc::item_info::read_zip(&unread_path, "unread.png", &ctx.cancel).is_err()
    );
    let mut outer = zip_bytes("inner.zip", b"small");
    let central = outer
        .windows(4)
        .position(|value| value == b"PK\x01\x02")
        .unwrap();
    outer[central + 24..central + 28].copy_from_slice(&declared);
    outer[22..26].copy_from_slice(&declared);
    let outer_path = root.join("large-nested.zip");
    std::fs::write(&outer_path, outer).unwrap();
    assert_eq!(
        prepare_rating(&item(&outer_path, Some("inner.zip/image.png")), root, &ctx)
            .unwrap()
            .unwrap_err(),
        MetadataSkipReason::Unavailable
    );
}

#[test]
fn metadata_write_pdf_uses_exact_page_numbers_range_and_actual_completion_checkpoint() {
    let fixture = Fixture::new();
    let ctx = context(&fixture);
    let root = fixture._dir.path();
    let path = root.join("book.pdf");
    std::fs::write(&path, b"fixture").unwrap();
    for invalid in [
        "page_01",
        "page_-1",
        "page_+1",
        "page_",
        "page_4294967296",
        "page:1",
    ] {
        assert_eq!(pdf_page_number(invalid), None);
    }
    let target = prepare_rating_with_pdf_sizes(&item(&path, Some("page_1")), root, &ctx, |_| {
        Ok(vec![(10., 20.); 2])
    })
    .unwrap()
    .unwrap();
    assert_eq!(target.key, crate::edit_source::page_key_for_pdf(&path, 1));
    assert_eq!(target.meta.page_num, Some(1));
    assert_eq!(target.meta.kind, RatingItemKind::PdfPage);
    assert_eq!(
        prepare_rating_with_pdf_sizes(&item(&path, Some("page_2")), root, &ctx, |_| Ok(vec![
            (
                10., 20.
            );
            2
        ]))
        .unwrap()
        .unwrap_err(),
        MetadataSkipReason::UnsupportedSubresource
    );
    let error = prepare_rating_with_pdf_sizes(&item(&path, Some("page_0")), root, &ctx, |_| {
        ctx.cancel.store(true, Ordering::Release);
        Ok(vec![(10., 20.)])
    })
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Cancelled);
}

#[test]
fn metadata_write_cancel_and_deadline_stop_before_preparation() {
    let mut fixture = Fixture::new();
    let ctx = context(&fixture);
    ctx.cancel.store(true, Ordering::Release);
    let command = Command::SetRating {
        items: vec![ItemRef {
            path: "relative".into(),
            subresource: None,
        }],
        stars: 1,
    };
    assert_eq!(
        prepare(command.clone(), &ctx, "client".into())
            .unwrap_err()
            .code,
        ErrorCode::Cancelled
    );
    let elapsed = Arc::new(AtomicUsize::new(0));
    let clock_elapsed = elapsed.clone();
    let start = Instant::now();
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks =
        Some(Arc::new(crate::agent_ipc::ReadHooks {
            now: Arc::new(move || {
                start + Duration::from_secs(clock_elapsed.load(Ordering::Acquire) as u64)
            }),
            execute: Box::new(|_, _| unreachable!()),
        }));
    let ctx = context(&fixture);
    elapsed.store(20, Ordering::Release);
    assert_eq!(
        prepare(command, &ctx, "client".into()).unwrap_err().code,
        ErrorCode::Timeout
    );
}

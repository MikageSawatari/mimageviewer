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
        (
            PermissionSnapshot {
                level: AgentLevel::Organize,
                thumbnails: false,
                gps: false,
                ai_captions: false,
            },
            0,
        ),
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
fn metadata_write_zip_directories_match_ui_rating_keys_without_reading_leaf_payloads() {
    // The two fixtures each own the serialized data-directory override; finish
    // worker preparation before constructing the App used for pure UI checks.
    let targets = {
        let fixture = Fixture::new();
        let ctx = context(&fixture);
        let root = fixture._dir.path();
        let path = root.join("folders.zip");
        let (folder_bytes, legacy_leaf) = cp932_zip("xxxx/page.png", "日本/page.png", b"image");
        std::fs::write(&path, &folder_bytes).unwrap();
        let legacy_folder = format!("{}/", legacy_leaf.split('/').next().unwrap());
        let legacy_key = zip_directory_rating_key(&path, &legacy_folder);
        let db = crate::rating_db::RatingDb::open().unwrap();
        db.set(&legacy_key, 5).unwrap();
        let folder = prepare_rating(&item(&path, Some("日本/")), root, &ctx)
            .unwrap()
            .unwrap();
        assert_eq!(folder.meta.dir_prefix.as_deref(), Some("日本/"));
        assert_eq!(folder.meta.zipdir_is_archive, Some(false));
        assert_eq!(folder.legacy_key.as_deref(), Some(legacy_key.as_str()));
        let old_spelling = prepare_rating(&item(&path, Some(&legacy_folder)), root, &ctx)
            .unwrap()
            .unwrap();
        assert_eq!(old_spelling.key, folder.key);
        assert_eq!(old_spelling.legacy_key, folder.legacy_key);
        assert_eq!(
            db.get(&legacy_key),
            5,
            "preparation must not migrate ratings"
        );
        assert_eq!(db.get(&folder.key), 0);
        assert_eq!(std::fs::read(&path).unwrap(), folder_bytes);
        assert_eq!(
            prepare_rating(&item(&path, Some("missing/")), root, &ctx)
                .unwrap()
                .unwrap_err(),
            MetadataSkipReason::NotFound
        );
        for invalid in ["/", "日本//", "日本/../", "../日本/"] {
            assert_eq!(
                prepare_rating(&item(&path, Some(invalid)), root, &ctx)
                    .unwrap()
                    .unwrap_err(),
                MetadataSkipReason::UnsupportedSubresource
            );
        }
        // Every nesting level retains both its decoded and old ZIP spelling.
        let nested_path = root.join("nested.zip");
        let (outer_bytes, legacy_outer) = cp932_zip("xxxx.zip", "日本.zip", &folder_bytes);
        std::fs::write(&nested_path, &outer_bytes).unwrap();
        let book = prepare_rating(&item(&nested_path, Some("日本.zip/")), root, &ctx)
            .unwrap()
            .unwrap();
        assert_eq!(book.meta.dir_prefix.as_deref(), Some("日本.zip/"));
        assert_eq!(book.meta.zipdir_is_archive, Some(true));
        assert_eq!(
            book.legacy_key,
            Some(zip_directory_rating_key(
                &nested_path,
                &format!("{legacy_outer}/")
            ))
        );
        let nested_folder = prepare_rating(&item(&nested_path, Some("日本.zip/日本/")), root, &ctx)
            .unwrap()
            .unwrap();
        assert_eq!(
            nested_folder.meta.dir_prefix.as_deref(),
            Some("日本.zip/日本/")
        );
        assert_eq!(nested_folder.meta.zipdir_is_archive, Some(false));
        assert_eq!(
            nested_folder.legacy_key,
            Some(zip_directory_rating_key(
                &nested_path,
                &format!("{legacy_outer}/{legacy_folder}")
            ))
        );
        assert_eq!(
            prepare_rating(&item(&nested_path, Some("日本.zip/missing/")), root, &ctx)
                .unwrap()
                .unwrap_err(),
            MetadataSkipReason::NotFound
        );
        assert_eq!(std::fs::read(&nested_path).unwrap(), outer_bytes);
        let old_nested = format!("{legacy_outer}/{legacy_folder}");
        assert_eq!(
            prepare_rating(&item(&nested_path, Some(&old_nested)), root, &ctx)
                .unwrap()
                .unwrap()
                .key,
            nested_folder.key
        );
        let explicit_path = root.join("explicit.zip");
        let mut explicit = zip::ZipWriter::new(Cursor::new(Vec::new()));
        explicit
            .add_directory("empty/", zip::write::SimpleFileOptions::default())
            .unwrap();
        std::fs::write(&explicit_path, explicit.finish().unwrap().into_inner()).unwrap();
        let empty_folder = prepare_rating(&item(&explicit_path, Some("empty/")), root, &ctx)
            .unwrap()
            .unwrap();
        // Corrupt terminal ZIP payloads prove book identity lookup never opens
        // that payload, even after reading a required intermediate ZIP.
        let mut unread = zip_bytes("book.cbz", b"payload");
        let offset = unread
            .windows(7)
            .position(|value| value == b"payload")
            .unwrap();
        unread[offset] ^= 1;
        let deep_path = root.join("deep.zip");
        std::fs::write(&deep_path, zip_bytes("inner.zip", &unread)).unwrap();
        let deep_book = prepare_rating(&item(&deep_path, Some("inner.zip/book.cbz/")), root, &ctx)
            .unwrap()
            .unwrap();
        assert_eq!(deep_book.meta.zipdir_is_archive, Some(true));
        assert!(deep_book.legacy_key.is_none());
        assert!(!root.join("mimageviewer.dat").exists());
        vec![folder, book, nested_folder, empty_folder, deep_book]
    };
    let mut app = crate::app::setup_app_for_test();
    for target in targets {
        app.items = vec![crate::grid_item::GridItem::ZipDir {
            zip_path: target.source.clone(),
            dir_prefix: target.meta.dir_prefix.clone().unwrap(),
            is_archive: target.meta.zipdir_is_archive.unwrap(),
            representative: None,
        }];
        assert_eq!(app.rating_path_key(0).as_deref(), Some(target.key.as_str()));
        assert_eq!(app.rating_meta_for_idx(0), Some(target.meta));
        assert!(target.xmp_target.is_none());
        assert!(!target.compiled_book_page);
    }
}

#[test]
fn metadata_write_zip_directory_cp932_backslash_trails_preserve_raw_boundaries() {
    let fixture = Fixture::new();
    let ctx = context(&fixture);
    let root = fixture._dir.path();
    assert_eq!(
        encoding_rs::SHIFT_JIS.encode("表").0.as_ref(),
        &[0x95, 0x5c]
    );
    for (placeholder, name, prefix) in [
        ("xx/inner/page.png", "表/inner/page.png", "表/inner/"),
        ("xxinner/page.png", "表inner/page.png", "表inner/"),
    ] {
        let (bytes, legacy_name) = cp932_zip(placeholder, name, b"payload");
        // Derive the old prefix from the actual archive name at the filename
        // boundary, retaining the CP437 backslash produced by the trail byte.
        let legacy_prefix = legacy_name
            .strip_suffix("page.png")
            .unwrap()
            .replace('\\', "/");
        let path = root.join(format!("{}.zip", prefix.replace('/', "-")));
        std::fs::write(&path, &bytes).unwrap();
        let target = prepare_rating(&item(&path, Some(prefix)), root, &ctx)
            .unwrap()
            .unwrap();
        assert_eq!(target.meta.dir_prefix.as_deref(), Some(prefix));
        assert_eq!(target.key, zip_directory_rating_key(&path, prefix));
        assert_eq!(
            target.legacy_key,
            Some(zip_directory_rating_key(&path, &legacy_prefix))
        );
        if crate::book_scope::validate_relative_reference(legacy_prefix.strip_suffix('/').unwrap())
            .is_ok()
        {
            let old_input = prepare_rating(&item(&path, Some(&legacy_prefix)), root, &ctx)
                .unwrap()
                .unwrap();
            assert_eq!(old_input.key, target.key);
            assert_eq!(old_input.legacy_key, target.legacy_key);
            // The apparent legacy separator inside 表 must not invent a
            // directory half-way through a CP932 character.
            let apparent_parent = format!("{}/", legacy_prefix.split('/').next().unwrap());
            assert_eq!(
                prepare_rating(&item(&path, Some(&apparent_parent)), root, &ctx)
                    .unwrap()
                    .unwrap_err(),
                MetadataSkipReason::NotFound
            );
        }
        let outer_path = path.with_extension("nested.zip");
        let (outer, legacy_outer) = cp932_zip("xx.zip", "表.zip", &bytes);
        std::fs::write(&outer_path, &outer).unwrap();
        let nested_prefix = format!("表.zip/{prefix}");
        let nested = prepare_rating(&item(&outer_path, Some(&nested_prefix)), root, &ctx)
            .unwrap()
            .unwrap();
        assert_eq!(
            nested.meta.dir_prefix.as_deref(),
            Some(nested_prefix.as_str())
        );
        assert_eq!(
            nested.key,
            zip_directory_rating_key(&outer_path, &nested_prefix)
        );
        assert_eq!(
            nested.legacy_key,
            Some(zip_directory_rating_key(
                &outer_path,
                &format!("{}/{legacy_prefix}", legacy_outer.replace('\\', "/"))
            ))
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        assert_eq!(std::fs::read(&outer_path).unwrap(), outer);
    }
}

#[test]
fn metadata_write_zip_directory_intermediates_keep_size_and_cancellation_bounds() {
    let fixture = Fixture::new();
    let ctx = context(&fixture);
    let root = fixture._dir.path();
    let path = root.join("large-nested.zip");
    let mut bytes = zip_bytes("inner.zip", b"small");
    let central = bytes
        .windows(4)
        .position(|value| value == b"PK\x01\x02")
        .unwrap();
    let declared = (64_u32 * 1024 * 1024 + 1).to_le_bytes();
    bytes[central + 24..central + 28].copy_from_slice(&declared);
    bytes[22..26].copy_from_slice(&declared);
    std::fs::write(&path, bytes).unwrap();
    assert!(
        prepare_rating(&item(&path, Some("inner.zip/")), root, &ctx)
            .unwrap()
            .is_ok(),
        "terminal book identity needs no payload"
    );
    assert_eq!(
        prepare_rating(&item(&path, Some("inner.zip/folder/")), root, &ctx)
            .unwrap()
            .unwrap_err(),
        MetadataSkipReason::Unavailable
    );
    ctx.cancel.store(true, Ordering::Release);
    assert_eq!(
        super::super::item_info::read_zip_directory_identity(&path, "folder/", &ctx.cancel)
            .err()
            .unwrap()
            .kind(),
        std::io::ErrorKind::Interrupted
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

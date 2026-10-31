use super::*;
use crate::agent_ipc::tests::Fixture;
use crate::agent_ipc::{ReadContext, encode_cursor};
use mimageviewer_agent_ipc::*;
use std::io::{Cursor, Write};
use std::sync::{Arc, atomic::AtomicBool};

#[test]
fn aliases_are_rejected_before_the_long_name_lookup() {
    for path in [
        "relative.png",
        r"C:relative.png",
        r"\rooted.png",
        "C:/images/./a.png",
        r"C:\images\..\x\..\a.png",
        r"\\?\C:\images\a.png",
        r"\\.\C:\images\a.png",
        r"\??\C:\images\a.png",
        "//?/UNC/server/share/a.png",
        "C:/images/a.png:stream",
        "C:/images./a.png",
        "C:/images /a.png",
        "C:/images/a.png.",
        "C:/images/a.png ",
        "C:/images/a\0.png",
        "C:/images//a.png",
        r"\\server\",
        r"\\server\\share\a.png",
    ] {
        let result = validate_with(path, |_| panic!("invalid path reached filesystem"));
        let error = result.unwrap_err();
        assert_eq!(error.code, ErrorCode::BadRequest);
        assert_eq!(error.message, MESSAGE);
    }
}

#[test]
fn short_names_are_rejected_by_mocked_long_name_lookup() {
    // This does not depend on the volume having 8.3 name creation enabled.
    for (short, long) in [
        (r"C:\IMAGES~1\photo.png", r"C:\Images long\photo.png"),
        (r"C:\Images\PHOTO~1.PNG", r"C:\Images\Photograph.png"),
        (
            r"\\server\share\BOOKNA~1.ZIP",
            r"\\server\share\Book name.zip",
        ),
    ] {
        let error = validate_with(short, |_| Ok(Some(long.into()))).unwrap_err();
        assert_eq!(error.code, ErrorCode::BadRequest);
        assert_eq!(error.message, MESSAGE);
    }
}

#[test]
fn normal_paths_keep_case_separator_roots_and_link_identity() {
    for (input, long) in [
        (r"C:\", r"C:\"),
        ("c:/Images/PHOTO.png", r"C:\Images\photo.png"),
        (r"C:\画像\写真.png", r"C:\画像\写真.png"),
        (r"C:\Ä\Ö.png", r"C:\ä\ö.png"),
        (r"\\server\share\a.png", r"\\server\share\a.png"),
        // A long-name lookup retains the link name; it does not open the final
        // target or turn a junction into the target's unrelated edit key.
        (r"C:\junction\a.png", r"C:\junction\a.png"),
    ] {
        assert_eq!(
            validate_with(input, |_| Ok(Some(long.into()))).unwrap(),
            Path::new(input)
        );
    }
    assert_ne!(
        key(Path::new(r"C:\junction\a.png")),
        key(Path::new(r"D:\target\a.png"))
    );
    assert_eq!(
        validate_with(r"C:\missing\a.png", |_| Ok(None)).unwrap(),
        Path::new(r"C:\missing\a.png")
    );
    let error = validate_with(r"C:\images\a.png", |_| {
        Err(std::io::Error::from_raw_os_error(5))
    })
    .unwrap_err();
    assert_eq!(error.code, ErrorCode::Internal);
}

fn context(fixture: &Fixture) -> ReadContext {
    ReadContext::new(
        &fixture.handlers,
        PermissionSnapshot {
            level: AgentLevel::ReadOnly,
            thumbnails: true,
            gps: false,
        },
        Arc::new(AtomicBool::new(false)),
    )
}

fn png() -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(7, 5)
        .write_to(&mut bytes, image::ImageFormat::Png)
        .unwrap();
    bytes.into_inner()
}

fn assert_path_error<T>(result: Result<T, AgentError>) {
    let error = result.err().expect("alias must not return data or pixels");
    assert_eq!(error.code, ErrorCode::BadRequest);
    assert_eq!(error.message, MESSAGE);
}

#[test]
fn concealed_file_and_virtual_items_reject_aliases_in_all_three_handlers() {
    let fixture = Fixture::new();
    let root = fixture._dir.path();
    let image = root.join("concealed.png");
    std::fs::write(&image, png()).unwrap();
    let archive = root.join("concealed.zip");
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    writer
        .start_file("page.png", zip::write::SimpleFileOptions::default())
        .unwrap();
    writer.write_all(&png()).unwrap();
    std::fs::write(&archive, writer.finish().unwrap().into_inner()).unwrap();
    let pdf = root.join("concealed.pdf");
    // Conceal must be checked before any PDF decode is attempted.
    std::fs::write(&pdf, b"not a PDF").unwrap();
    let db = crate::conceal_db::ConcealDb::open().unwrap();
    for key in [
        crate::adjustment_db::normalize_path(&image),
        crate::adjustment_db::zip_entry_key(&archive, "page.png"),
        crate::edit_source::page_key_for_grid_item(&crate::grid_item::GridItem::PdfPage {
            pdf_path: pdf.clone(),
            page_num: 0,
            content_type: None,
        })
        .unwrap(),
    ] {
        db.set(&key, &[true], &[], 1, 1).unwrap();
    }
    drop(db);
    let ctx = context(&fixture);
    for (path, member) in [
        (&image, None),
        (&archive, Some("page.png")),
        (&pdf, Some("page_0")),
    ] {
        let item = ItemRef {
            path: path.to_string_lossy().into_owned(),
            subresource: member.map(str::to_owned),
        };
        let Payload::Thumbnails(hidden) = fixture
            .handlers
            .get_thumbnail(vec![item.clone()], None, None, &ctx)
            .unwrap()
        else {
            panic!("expected thumbnail metadata")
        };
        assert!(hidden.images.is_empty());
        assert!(
            matches!(&hidden.page.items[0], PageRow::Value(row) if row.status == ThumbnailStatus::HiddenByEdit)
        );
        let root = root.to_string_lossy();
        let name = path.file_name().unwrap().to_string_lossy();
        for alias in [
            format!("{root}\\.\\{name}"),
            format!(
                "{root}\\..\\x\\..\\{}\\{name}",
                path.parent()
                    .unwrap()
                    .file_name()
                    .unwrap()
                    .to_string_lossy()
            ),
            format!("\\\\?\\{}", item.path),
            format!("{}.", item.path),
            format!("{} ", item.path),
            format!("{}:stream", item.path),
        ] {
            let input = ItemRef {
                path: alias.clone(),
                subresource: item.subresource.clone(),
            };
            assert_path_error(fixture.handlers.get_thumbnail(
                vec![input.clone()],
                None,
                None,
                &ctx,
            ));
            assert_path_error(
                fixture
                    .handlers
                    .item_info(vec![input.clone()], None, None, &ctx),
            );
            assert_path_error(crate::agent_ipc::item_info::read(
                &input,
                ctx.permission,
                &ctx.cancel,
            ));
            assert_path_error(
                fixture
                    .handlers
                    .list_folder(alias.clone(), None, None, None, &ctx),
            );
            if member.is_some() {
                assert_path_error(fixture.handlers.list_folder(
                    alias,
                    Some("inner/".into()),
                    None,
                    None,
                    &ctx,
                ));
            }
        }
    }
}

#[test]
fn cursors_use_only_validated_spelling_and_validate_even_skipped_inputs() {
    let fixture = Fixture::new();
    let path = fixture._dir.path().join("missing.png");
    let normal = path.to_string_lossy().into_owned();
    assert_eq!(
        cursor_key(&normal).unwrap(),
        cursor_key(&normal.replace('\\', "/").to_uppercase()).unwrap()
    );
    let ctx = context(&fixture);
    let good = ItemRef {
        path: normal.clone(),
        subresource: None,
    };
    let bad = ItemRef {
        path: format!("{}\\.\\missing.png", fixture._dir.path().display()),
        subresource: None,
    };
    for tool in ["get_item_info", "get_thumbnail"] {
        let normalized = serde_json::json!([
            [key(Path::new(&bad.path)), null],
            [key(Path::new(&good.path)), null]
        ]);
        let args = if tool == "get_item_info" {
            serde_json::json!([normalized, "input_order"])
        } else {
            serde_json::json!([normalized, DEFAULT_THUMBNAIL_SIDE, "input_order"])
        };
        let cursor = encode_cursor(tool, args, 1);
        let inputs = vec![bad.clone(), good.clone()];
        if tool == "get_item_info" {
            assert_path_error(fixture.handlers.item_info(inputs, None, Some(cursor), &ctx));
        } else {
            assert_path_error(
                fixture
                    .handlers
                    .get_thumbnail(inputs, None, Some(cursor), &ctx),
            );
        }
    }
    // A normally spelled missing path still has each tool's existing outcome.
    let Payload::Thumbnails(missing) = fixture
        .handlers
        .get_thumbnail(vec![good.clone()], None, None, &ctx)
        .unwrap()
    else {
        panic!()
    };
    assert!(
        matches!(&missing.page.items[0], PageRow::Value(item) if item.status == ThumbnailStatus::Unavailable)
    );
    assert!(
        fixture
            .handlers
            .item_info(vec![good], None, None, &ctx)
            .is_ok()
    );
}

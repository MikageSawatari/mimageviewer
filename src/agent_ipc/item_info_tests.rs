use super::*;
use std::io::Write;

fn permission(gps: bool) -> PermissionSnapshot {
    PermissionSnapshot {
        level: AgentLevel::ReadOnly,
        thumbnails: false,
        gps,
        ai_captions: false,
    }
}

fn seed_caption(path: &Path, failed: bool) {
    use crate::ai_caption_db::*;
    let request = SaveRequest {
        rotation: crate::rotation_db::Rotation::None,
        item_path: path.to_owned(),
        signature_path: path.to_owned(),
        signature: FileSignature::from_metadata(&std::fs::metadata(path).unwrap()).unwrap(),
        cover_ref: String::new(),
        model_id: "caption-model".into(),
        prompt_version: 1,
        lang: "ja".into(),
        result: if failed {
            CaptionResult::Failed
        } else {
            CaptionResult::Ok {
                caption: "海辺の本".into(),
                text: "連絡先 123".into(),
                keywords: vec!["sea".into()],
            }
        },
    };
    assert_eq!(
        CaptionDb::open().unwrap().save(&request).unwrap(),
        SaveOutcome::Written
    );
}

#[test]
fn ai_caption_permission_rows_and_image_staleness() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("caption.png");
    std::fs::write(&path, png(&[])).unwrap();
    let allowed = PermissionSnapshot {
        ai_captions: true,
        ..permission(false)
    };
    let read_image =
        |permission| read(&item(&path, None), permission, &AtomicBool::new(false)).unwrap();
    assert_eq!(read_image(allowed).ai_caption, CaptionField::None);
    assert!(!crate::ai_caption_db::CaptionDb::db_path().exists());
    seed_caption(&path, true);
    assert_eq!(read_image(allowed).ai_caption, CaptionField::None);
    seed_caption(&path, false);
    let output = read_image(allowed);
    let CaptionField::Present(info) = output.ai_caption else {
        panic!("caption missing")
    };
    assert_eq!(info.caption.text, "海辺の本");
    assert_eq!(info.visible_text.text, "連絡先 123");
    assert_eq!(info.keywords, ["sea"]);
    assert_eq!(info.model, "caption-model");
    assert!(info.created_at > 0);
    assert_eq!(info.stale, Some(false));
    assert!(output.container_ai_caption.is_none());
    assert_eq!(
        read_image(permission(false)).ai_caption,
        CaptionField::NotPermitted
    );
    std::fs::write(&path, [png(&[]), vec![0]].concat()).unwrap();
    assert!(matches!(
        read_image(allowed).ai_caption,
        CaptionField::Present(AiCaptionInfo {
            stale: Some(true),
            ..
        })
    ));
}

#[test]
fn ai_caption_rotation_stale_compares_saved_generation_angle() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("caption.png");
    std::fs::write(&path, png(&[])).unwrap();
    seed_caption(&path, false);
    let metadata = std::fs::metadata(&path).unwrap();
    let read = || {
        read_caption(
            &crate::ai_caption_db::CaptionDb::db_path(),
            &path,
            Some(&metadata),
            true,
        )
        .unwrap()
    };
    assert!(matches!(
        read(),
        CaptionField::Present(AiCaptionInfo {
            stale: Some(false),
            ..
        })
    ));
    let rotations = crate::rotation_db::RotationDb::open().unwrap();
    rotations
        .set(&path, crate::rotation_db::Rotation::Cw180)
        .unwrap();
    assert!(matches!(
        read(),
        CaptionField::Present(AiCaptionInfo {
            stale: Some(true),
            ..
        })
    ));
    let db = crate::ai_caption_db::CaptionDb::open().unwrap();
    let mut row = db.get(&path.to_string_lossy()).unwrap().unwrap();
    db.save(&crate::ai_caption_db::SaveRequest {
        item_path: path.clone(),
        signature_path: path.clone(),
        signature: row.signature,
        rotation: crate::rotation_db::Rotation::Cw180,
        cover_ref: row.cover_ref,
        model_id: row.model_id,
        prompt_version: row.prompt_version,
        lang: row.lang,
        result: crate::ai_caption_db::CaptionResult::Ok {
            caption: std::mem::take(&mut row.caption),
            text: row.visible_text,
            keywords: row.keywords,
        },
    })
    .unwrap();
    assert!(matches!(
        read(),
        CaptionField::Present(AiCaptionInfo {
            stale: Some(false),
            ..
        })
    ));
}

#[test]
fn ai_caption_rotation_book_stale_uses_current_folder_or_zip_cover() {
    use crate::ai_caption_db::{CaptionDb, CaptionResult, FileSignature, SaveRequest};
    use crate::rotation_db::{Rotation, RotationDb};
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let allowed = PermissionSnapshot {
        ai_captions: true,
        ..permission(false)
    };
    for zip in [false, true] {
        let book = directory
            .path()
            .join(if zip { "book.zip" } else { "folder" });
        let (signature_path, cover_ref, rotation_key) = if zip {
            std::fs::write(&book, zip_bytes(&[("cover.png", &png(&[]))])).unwrap();
            (
                book.clone(),
                "#entry=cover.png".to_string(),
                crate::edit_source::page_key_for_grid_item(&GridItem::ZipImage {
                    zip_path: book.clone(),
                    entry_name: "cover.png".into(),
                })
                .unwrap(),
            )
        } else {
            std::fs::create_dir(&book).unwrap();
            let cover = book.join("cover.png");
            std::fs::write(&cover, png(&[])).unwrap();
            (
                cover.clone(),
                "cover.png".to_string(),
                crate::edit_source::page_key_for_grid_item(&GridItem::Image(cover)).unwrap(),
            )
        };
        let db = CaptionDb::open().unwrap();
        db.save(&SaveRequest {
            item_path: book.clone(),
            signature: FileSignature::from_metadata(&std::fs::metadata(&signature_path).unwrap())
                .unwrap(),
            signature_path,
            rotation: Rotation::None,
            cover_ref,
            model_id: "fixture".into(),
            prompt_version: 1,
            lang: "ja".into(),
            result: CaptionResult::Ok {
                caption: "book".into(),
                text: String::new(),
                keywords: Vec::new(),
            },
        })
        .unwrap();
        let read_book = || {
            read(&item(&book, None), allowed, &AtomicBool::new(false))
                .unwrap()
                .ai_caption
        };
        assert!(matches!(
            read_book(),
            CaptionField::Present(AiCaptionInfo {
                stale: Some(false),
                ..
            })
        ));
        RotationDb::open()
            .unwrap()
            .set_key(&rotation_key, Rotation::Cw90)
            .unwrap();
        assert!(matches!(
            read_book(),
            CaptionField::Present(AiCaptionInfo {
                stale: Some(true),
                ..
            })
        ));
        if zip {
            let output = read(
                &item(&book, Some("cover.png")),
                allowed,
                &AtomicBool::new(false),
            )
            .unwrap();
            assert!(matches!(
                output.container_ai_caption,
                Some(CaptionField::Present(AiCaptionInfo {
                    stale: Some(true),
                    ..
                }))
            ));
        }
    }
}

#[test]
fn ai_caption_books_and_partial_items_use_outer_book_and_current_cover_staleness() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let allowed = PermissionSnapshot {
        ai_captions: true,
        ..permission(false)
    };
    let cancel = AtomicBool::new(false);
    for name in ["book.pdf", "book.epub", "book.zip", "book.rar", "folder"] {
        let path = directory.path().join(name);
        if name == "folder" {
            std::fs::create_dir(&path).unwrap();
        } else if name == "book.zip" {
            std::fs::write(&path, zip_bytes(&[("chapter/page.png", &png(&[]))])).unwrap();
        } else {
            std::fs::write(&path, b"fixture").unwrap();
        }
        seed_caption(&path, false);
        let mut output = read(&item(&path, None), allowed, &cancel).unwrap();
        let CaptionField::Present(info) = &output.ai_caption else {
            panic!("{name}")
        };
        assert_eq!(
            info.stale,
            if name == "book.zip" { Some(true) } else { None },
            "{name}"
        );
        assert!(output.container_ai_caption.is_none());
        // PDF projection uses the same enrichment boundary without launching
        // the PDF helper just to verify the caption's container ownership.
        if name == "book.pdf" {
            output.item.subresource = Some("page_0".into());
            output.ai_caption = CaptionField::None;
            enrich_caption(
                &mut output,
                &path,
                Some(&std::fs::metadata(&path).unwrap()),
                true,
            )
            .unwrap();
            assert_eq!(output.ai_caption, CaptionField::None);
            assert!(matches!(
                output.container_ai_caption,
                Some(CaptionField::Present(AiCaptionInfo { stale: None, .. }))
            ));
        }
        if name == "book.zip" {
            for member in ["chapter/page.png", "chapter/"] {
                let output = read(&item(&path, Some(member)), allowed, &cancel).unwrap();
                assert_eq!(output.ai_caption, CaptionField::None);
                assert!(matches!(
                    output.container_ai_caption,
                    Some(CaptionField::Present(AiCaptionInfo {
                        stale: Some(true),
                        ..
                    }))
                ));
                let hidden = read(&item(&path, Some(member)), permission(false), &cancel).unwrap();
                assert_eq!(hidden.ai_caption, CaptionField::NotPermitted);
                assert_eq!(
                    hidden.container_ai_caption,
                    Some(CaptionField::NotPermitted)
                );
            }
        }
    }
}

#[test]
fn ai_caption_off_does_not_open_store_and_read_failure_is_not_none() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let db_path = crate::ai_caption_db::CaptionDb::db_path();
    std::fs::write(&db_path, b"unreadable caption database").unwrap();
    let path = directory.path().join("image.png");
    std::fs::write(&path, png(&[])).unwrap();
    assert_eq!(
        read(
            &item(&path, None),
            permission(false),
            &AtomicBool::new(false)
        )
        .unwrap()
        .ai_caption,
        CaptionField::NotPermitted
    );
    let allowed = PermissionSnapshot {
        ai_captions: true,
        ..permission(false)
    };
    assert_eq!(
        read(&item(&path, None), allowed, &AtomicBool::new(false))
            .unwrap_err()
            .code,
        ErrorCode::Internal
    );
    assert_eq!(
        std::fs::read(&db_path).unwrap(),
        b"unreadable caption database"
    );
    let version_zero = directory.path().join("unpublished.db");
    drop(rusqlite::Connection::open(&version_zero).unwrap());
    assert_eq!(
        read_caption(&version_zero, &path, None, true).unwrap(),
        CaptionField::None
    );
}

#[test]
fn ai_caption_text_budget_trims_after_generation_and_marks_each_text() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("caption.png");
    std::fs::write(&path, png(&[])).unwrap();
    seed_caption(&path, false);
    let mut output = read(
        &item(&path, None),
        PermissionSnapshot {
            ai_captions: true,
            ..permission(false)
        },
        &AtomicBool::new(false),
    )
    .unwrap();
    output.generation = InfoField::Present(GenerationInfo {
        prompt: Some(text("生成情報".repeat(5000))),
        ..Default::default()
    });
    let CaptionField::Present(info) = &mut output.ai_caption else {
        panic!()
    };
    info.caption = text("説明文\\\"".repeat(5000));
    info.visible_text = text("画像内文字".repeat(5000));
    trim_descriptions(&mut output).unwrap();
    assert!(mcp_content_chars(&output).unwrap() <= MAX_ITEM_CHARS);
    let InfoField::Present(generation) = output.generation else {
        panic!()
    };
    assert_eq!(
        generation.prompt.unwrap(),
        TextValue {
            text: String::new(),
            truncated: true
        }
    );
    let CaptionField::Present(info) = output.ai_caption else {
        panic!()
    };
    assert!(info.caption.truncated && info.visible_text.truncated);
    assert_eq!(info.keywords, ["sea"]);
    assert_eq!(info.model, "caption-model");
}

#[test]
fn ai_caption_item_info_is_withheld_when_permission_changes_during_read() {
    use crate::agent_ipc::{ReadHooks, tests::Fixture};
    use std::sync::Arc;
    let mut fixture = Fixture::new();
    let path = fixture._dir.path().join("caption.png");
    std::fs::write(&path, png(&[])).unwrap();
    seed_caption(&path, false);
    let set_allowed = |allowed| {
        crate::settings_db::with_db_result(|db| {
            let mut settings = db.load_into_settings()?;
            settings.agent_show_ai_captions = allowed;
            db.save_full(&settings)
        })
        .unwrap()
    };
    set_allowed(true);
    let (started_send, started) = crossbeam_channel::bounded(1);
    let (release, gate) = crossbeam_channel::bounded(1);
    Arc::get_mut(&mut fixture.handlers).unwrap().read_hooks = Some(Arc::new(ReadHooks {
        now: Arc::new(std::time::Instant::now),
        execute: Box::new(move |command, ctx| {
            let Command::GetItemInfo { items, .. } = command else {
                panic!("unexpected command")
            };
            let output = read(&items[0], ctx.granted.0, &ctx.cancel)?;
            assert!(matches!(output.ai_caption, CaptionField::Present(_)));
            started_send.send(ctx.granted).unwrap();
            gate.recv_timeout(std::time::Duration::from_secs(10))
                .unwrap();
            Ok(Payload::Info(InfoPage {
                items: vec![output.into()],
                cursor: None,
                truncated: false,
            }))
        }),
    }));
    let connection = fixture.connection();
    let (send, recv) = crossbeam_channel::bounded(1);
    let worker = connection
        .request(
            Request {
                id: 1,
                command: Command::GetItemInfo {
                    items: vec![item(&path, None)],
                    limit: None,
                    cursor: None,
                },
            },
            send,
        )
        .unwrap();
    let admitted = started
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert!(admitted.0.ai_captions);
    set_allowed(false);
    release.send(()).unwrap();
    let reply = recv
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    worker.join().unwrap();
    assert_eq!(
        reply.wire().result.unwrap_err().code,
        ErrorCode::PermissionChanged
    );
}

fn item(path: &Path, subresource: Option<&str>) -> ItemRef {
    ItemRef {
        path: path.to_string_lossy().into_owned(),
        subresource: subresource.map(str::to_owned),
    }
}

fn png(chunks: &[(&str, &str)]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = png::Encoder::new(&mut bytes, 7, 5);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    for (key, value) in chunks {
        encoder
            .add_text_chunk((*key).to_owned(), (*value).to_owned())
            .unwrap();
    }
    encoder
        .write_header()
        .unwrap()
        .write_image_data(&[0; 7 * 5 * 3])
        .unwrap();
    bytes
}

fn zip_bytes(members: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    let date = zip::DateTime::from_date_and_time(2020, 2, 3, 4, 5, 6).unwrap();
    for (name, bytes) in members {
        writer
            .start_file(
                *name,
                zip::write::SimpleFileOptions::default()
                    .compression_method(zip::CompressionMethod::Stored)
                    .last_modified_time(date),
            )
            .unwrap();
        writer.write_all(bytes).unwrap();
    }
    writer.finish().unwrap().into_inner()
}

fn seed_tags(path: &Path, tag: &str) {
    crate::tags_db::TagsDb::open()
        .unwrap()
        .set_item_tags(&crate::tags_db::item_key_for_path(path), [tag], "fixture")
        .unwrap();
}

#[test]
fn physical_archives_and_extension_like_folders_have_consistent_kinds() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    for extension in ["zip", "cbz", "rar", "cbr", "7z", "cb7", "lzh", "lha"] {
        let path = directory.path().join(format!("book.{extension}"));
        std::fs::write(&path, b"identity-only fixture").unwrap();
        let output = read(
            &item(&path, None),
            permission(false),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(output.kind, ItemKind::Archive, "{extension}");
    }
    for name in ["book-folder", "book-folder.zip", "book-folder.epub"] {
        let path = directory.path().join(name);
        std::fs::create_dir(&path).unwrap();
        let output = read(
            &item(&path, None),
            permission(false),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(output.kind, ItemKind::Folder, "{name}");
    }
}

#[test]
fn png_projection_bounds_prompt_and_preserves_file_tags_and_rating() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("long.png");
    let parameters = format!(
        "{}\nNegative prompt: bad\nSteps: 25, Sampler: Euler, CFG scale: 7, Seed: 42, Size: 7x5, Model: example",
        "cat\\\"".repeat(12000)
    );
    std::fs::write(
        &path,
        png(&[
            ("parameters", &parameters),
            ("UnknownSecret", "UNKNOWN_CHUNK_SECRET"),
            ("workflow", "WORKFLOW_SECRET"),
        ]),
    )
    .unwrap();
    seed_tags(&path, "file-tag");
    crate::rating_db::RatingDb::open()
        .unwrap()
        .set(&crate::adjustment_db::normalize_path(&path), 4)
        .unwrap();
    let output = read(
        &item(&path, None),
        permission(false),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(
        output.dimensions,
        InfoField::Present(Dimensions {
            width: 7,
            height: 5,
            unit: DimensionUnit::Pixels
        })
    );
    assert_eq!(output.rating, 4);
    assert_eq!(
        output.tags,
        InfoField::Present(vec!["#file-tag".to_owned()])
    );
    assert!(output.container_tags.is_empty());
    assert!(mcp_content_chars(&output).unwrap() <= MAX_ITEM_CHARS);
    let (rows, cursor, truncated) = crate::agent_ipc::bounded_rows(
        vec![output.clone()],
        0,
        false,
        "get_item_info",
        serde_json::json!([item(&path, None)]),
        "items",
        0,
    )
    .unwrap();
    let page = InfoPage {
        items: rows,
        cursor,
        truncated,
    };
    assert_eq!(page.items.len(), 1);
    assert!(page.cursor.is_none());
    assert!(mcp_content_chars(&page).unwrap() <= MAX_PAGE_CHARS);
    let InfoField::Present(generation) = &output.generation else {
        panic!("missing generation");
    };
    assert!(generation.prompt.as_ref().unwrap().truncated);
    assert_eq!(generation.seed.as_ref().unwrap().text, "42");
    let json = serde_json::to_string(&output).unwrap();
    for secret in [
        "UNKNOWN_CHUNK_SECRET",
        "WORKFLOW_SECRET",
        "\"raw\"",
        "prompt_json",
        "workflow_json",
        "UserComment",
        "sampler_name",
    ] {
        assert!(!json.contains(secret), "leaked {secret}");
    }
}

#[test]
fn unknown_chunks_and_unresolved_workflow_never_become_generation_fields() {
    let unknown = project_generation(crate::png_metadata::extract_metadata_from_bytes_for_agent(
        &png(&[("Secret", "SECRET_CHUNK")]),
    ));
    assert_eq!(unknown, InfoField::Unavailable);
    let prompt = r#"{"1":{"class_type":"CLIPTextEncode","inputs":{"text":"safe positive"}},"2":{"class_type":"CLIPTextEncode","inputs":{"text":"safe negative"}},"3":{"class_type":"KSampler","inputs":{"positive":["1",0],"negative":["2",0],"seed":12,"steps":20,"sampler_name":"euler","scheduler":"UNKNOWN_SCHEDULER_SECRET"}},"4":{"class_type":"UnknownSecret","inputs":{"text":"UNKNOWN_NODE_SECRET"}}}"#;
    let source = crate::png_metadata::extract_metadata_from_bytes_for_agent(&png(&[
        ("prompt", prompt),
        ("workflow", r#"{"secret":"WORKFLOW_SECRET"}"#),
    ]));
    let InfoField::Present(generation) = project_generation(source) else {
        panic!("missing generation");
    };
    assert_eq!(generation.prompt.as_ref().unwrap().text, "safe positive");
    assert_eq!(
        generation.negative_prompt.as_ref().unwrap().text,
        "safe negative"
    );
    assert_eq!(generation.sampler.as_ref().unwrap().text, "euler");
    let json = serde_json::to_string(&generation).unwrap();
    assert!(!json.contains("SECRET"));
    assert!(!json.contains("class_type"));
    assert!(!json.contains("prompt_json"));
}

fn scalar_generation(chunk: &str, json: &serde_json::Value) -> InfoField<GenerationInfo> {
    project_generation(crate::png_metadata::extract_metadata_from_bytes_for_agent(
        &png(&[(chunk, &json.to_string())]),
    ))
}

fn assert_no_nested_generation(source: &InfoField<GenerationInfo>) {
    let InfoField::Present(generation) = source else {
        return;
    };
    let mut values = vec![generation.tool.as_deref(), generation.model.as_deref()];
    values.extend(
        [
            &generation.prompt,
            &generation.negative_prompt,
            &generation.seed,
            &generation.steps,
            &generation.sampler,
            &generation.cfg,
            &generation.dimensions,
        ]
        .into_iter()
        .map(|field| field.as_ref().map(|value| value.text.as_str())),
    );
    for value in values.into_iter().flatten() {
        for secret in ["gps", "latitude", "workflow_json", "secret", "{"] {
            assert!(!value.contains(secret), "leaked {secret}: {value}");
        }
    }
}

#[test]
fn nested_generation_seed_is_absent_for_file_and_same_png_in_zip() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("nested.png");
    let archive = directory.path().join("nested.zip");
    let image = png(&[(
        "parameters",
        r#"{"sui_image_params":{"prompt":"cat","seed":{"gps":{"latitude":35.5},"workflow_json":{"secret":"x"}}}}"#,
    )]);
    std::fs::write(&path, &image).unwrap();
    std::fs::write(&archive, zip_bytes(&[("nested.png", &image)])).unwrap();
    for input in [item(&path, None), item(&archive, Some("nested.png"))] {
        let output = read(&input, permission(false), &AtomicBool::new(false)).unwrap();
        assert_no_nested_generation(&output.generation);
        let InfoField::Present(generation) = output.generation else {
            panic!("missing safe generation");
        };
        assert_eq!(generation.prompt.as_ref().unwrap().text, "cat");
        assert!(generation.seed.is_none());
    }
}

fn generation_from_file_and_zip(bytes: &[u8]) -> [GenerationInfo; 2] {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("structured.png");
    let archive = directory.path().join("structured.zip");
    std::fs::write(&path, bytes).unwrap();
    std::fs::write(&archive, zip_bytes(&[("structured.png", bytes)])).unwrap();
    [item(&path, None), item(&archive, Some("structured.png"))].map(|input| {
        let output = read(&input, permission(false), &AtomicBool::new(false)).unwrap();
        assert_no_nested_generation(&output.generation);
        let InfoField::Present(generation) = output.generation else {
            panic!("known generation structure must retain its independent fields");
        };
        generation
    })
}

fn replace_json_chunk(
    image: &[u8],
    name: &str,
    update: impl FnOnce(&mut serde_json::Value),
) -> Vec<u8> {
    let mut chunks = crate::png_metadata::read_png_text_chunks_from_bytes(image).unwrap();
    let (_, text) = chunks.iter_mut().find(|(key, _)| key == name).unwrap();
    let mut source = serde_json::from_str(text).unwrap();
    update(&mut source);
    *text = serde_json::to_string(&source).unwrap();
    png(&chunks
        .iter()
        .map(|(key, value)| (key.as_str(), value.as_str()))
        .collect::<Vec<_>>())
}

fn invalid_text_leaves() -> Vec<serde_json::Value> {
    use serde_json::json;
    vec![
        json!({"gps":{"latitude":35.5},"workflow_json":{"secret":"x"}}),
        json!(["secret"]),
        json!(true),
        json!(42),
        json!(null),
    ]
}

#[test]
fn known_model_object_retains_scalar_name_for_file_and_zip() {
    let image = include_bytes!("../../tests/fixtures/ai_metadata/invokeai_new.png");
    let baseline = generation_from_file_and_zip(image);
    assert_eq!(baseline[0], baseline[1]);
    assert_eq!(baseline[0].model.as_deref(), Some("juggernautXL_v9"));
    assert!(baseline[0].prompt.is_some());
    assert!(baseline[0].negative_prompt.is_some());
    for bad in invalid_text_leaves() {
        let changed = replace_json_chunk(image, "invokeai_metadata", |value| {
            value["model"]["model_name"] = bad;
        });
        let mut expected = baseline[0].clone();
        expected.model = None;
        for actual in generation_from_file_and_zip(&changed) {
            assert_eq!(actual, expected, "only the non-scalar model name is absent");
        }
    }
}

#[test]
fn known_legacy_prompt_array_retains_scalar_leaves_for_file_and_zip() {
    let image = include_bytes!("../../tests/fixtures/ai_metadata/invokeai_legacy_sdmeta.png");
    let baseline = generation_from_file_and_zip(image);
    assert_eq!(baseline[0], baseline[1]);
    assert_eq!(baseline[0].model.as_deref(), Some("deliberate_v2"));
    assert_eq!(
        baseline[0].prompt.as_ref().unwrap().text,
        "a calico cat sitting on a windowsill, watercolor, soft morning light"
    );
    for bad in invalid_text_leaves() {
        let changed = replace_json_chunk(image, "sd-metadata", |value| {
            value["image"]["prompt"][0]["prompt"] = bad;
        });
        let mut expected = baseline[0].clone();
        expected.prompt = None;
        for actual in generation_from_file_and_zip(&changed) {
            assert_eq!(
                actual, expected,
                "keep model/params and do not fall back to Dream"
            );
        }
    }
    let mixed = replace_json_chunk(image, "sd-metadata", |value| {
        value["image"]["prompt"]
            .as_array_mut()
            .unwrap()
            .push(serde_json::json!({"prompt":"second scene","weight":1}));
    });
    let mut expected = baseline[0].clone();
    expected
        .prompt
        .as_mut()
        .unwrap()
        .text
        .push_str("\nsecond scene");
    for actual in generation_from_file_and_zip(&mixed) {
        assert_eq!(actual, expected);
    }
    // With no prompt leaf at all, keep the display parser's ordinary fallback.
    let empty = replace_json_chunk(image, "sd-metadata", |value| {
        value["image"]["prompt"] = serde_json::json!([]);
    });
    let InfoField::Present(fallback) =
        project_generation(crate::png_metadata::extract_metadata_from_bytes(&empty))
    else {
        panic!("empty legacy array should use the known Dream fallback");
    };
    assert!(fallback.prompt.is_some());
    for actual in generation_from_file_and_zip(&empty) {
        assert_eq!(actual, fallback);
    }
    let mixed = replace_json_chunk(&mixed, "sd-metadata", |value| {
        value["image"]["prompt"][0]["prompt"] = serde_json::json!({"secret":"x"});
    });
    expected.prompt.as_mut().unwrap().text = "second scene".into();
    for actual in generation_from_file_and_zip(&mixed) {
        assert_eq!(actual, expected);
    }
}

#[test]
fn known_v4_caption_structure_retains_scalar_leaves_for_file_and_zip() {
    let source = serde_json::json!({
        "v4_prompt":{"caption":{"base_caption":"base scene",
            "char_captions":[{"char_caption":"hero character"}]}},
        "v4_negative_prompt":{"caption":{"base_caption":"bad scene"}},
        "seed":45,"steps":20
    });
    // Same v4 shape as png_metadata::tests::test_detect_novelai_v4_caption_shape,
    // with no Description fallback and no raw workflow in the projected fields.
    let image = png(&[("Software", "NovelAI"), ("Comment", &source.to_string())]);
    let baseline = generation_from_file_and_zip(&image);
    assert_eq!(baseline[0], baseline[1]);
    assert_eq!(
        baseline[0].prompt.as_ref().unwrap().text,
        "base scene\nhero character"
    );
    assert_eq!(
        baseline[0].negative_prompt.as_ref().unwrap().text,
        "bad scene"
    );
    let mixed = replace_json_chunk(&image, "Comment", |value| {
        value["v4_prompt"]["caption"]["char_captions"] = serde_json::json!([
            {"char_caption":"hero character"}, "secret", {"unknown":"secret"}
        ]);
    });
    for actual in generation_from_file_and_zip(&mixed) {
        assert_eq!(
            actual, baseline[0],
            "only named caption leaves contribute text"
        );
    }
    for bad in invalid_text_leaves() {
        for (pointer, remaining) in [
            ("/v4_prompt/caption/base_caption", Some("hero character")),
            (
                "/v4_prompt/caption/char_captions/0/char_caption",
                Some("base scene"),
            ),
            ("/v4_negative_prompt/caption/base_caption", None),
        ] {
            let changed = replace_json_chunk(&image, "Comment", |value| {
                *value.pointer_mut(pointer).unwrap() = bad.clone();
            });
            let mut expected = baseline[0].clone();
            if let Some(remaining) = remaining {
                expected.prompt.as_mut().unwrap().text = remaining.into();
            } else {
                expected.negative_prompt = None;
            }
            for actual in generation_from_file_and_zip(&changed) {
                assert_eq!(
                    actual, expected,
                    "only the rejected leaf at {pointer} is absent"
                );
            }
        }
        let changed = replace_json_chunk(&image, "Comment", |value| {
            value["v4_prompt"]["caption"] = serde_json::json!({"base_caption":bad});
            value.as_object_mut().unwrap().remove("v4_negative_prompt");
        });
        let mut expected = baseline[0].clone();
        expected.prompt = None;
        expected.negative_prompt = None;
        for actual in generation_from_file_and_zip(&changed) {
            assert_eq!(
                actual, expected,
                "rejected final text leaf must not hide seed/steps"
            );
        }
    }
}

#[test]
fn every_generation_field_rejects_original_json_objects_and_arrays() {
    use serde_json::json;
    let object = json!({"gps":{"latitude":35.5},"workflow_json":{"secret":"x"}});
    for nested in [object.clone(), json!([object, "secret"])] {
        // All parameter aliases used by project_generation and all model aliases
        // recognized by these JSON formats must retain their original type check.
        for key in [
            "prompt",
            "negativeprompt",
            "model",
            "seed",
            "steps",
            "num_inference_steps",
            "sampler",
            "sampler_name",
            "cfg",
            "cfg scale",
            "cfg_scale",
            "guidance_scale",
            "guidance scale",
            "scale",
            "size",
            "dimensions",
            "width",
            "height",
        ] {
            let mut source = json!({"sui_image_params":{"prompt":"cat","negativeprompt":"bad"}});
            source["sui_image_params"][key] = nested.clone();
            let output = scalar_generation("parameters", &source);
            assert_no_nested_generation(&output);
            let InfoField::Present(generation) = output else {
                panic!("missing safe generation for {key}");
            };
            let field = match key {
                "prompt" => &generation.prompt,
                "negativeprompt" => &generation.negative_prompt,
                "seed" => &generation.seed,
                "steps" | "num_inference_steps" => &generation.steps,
                "sampler" | "sampler_name" => &generation.sampler,
                "size" | "dimensions" | "width" | "height" => &generation.dimensions,
                "model" => {
                    assert!(generation.model.is_none());
                    continue;
                }
                _ => &generation.cfg,
            };
            assert!(field.is_none(), "unexpected {key}: {field:?}");
        }
        for key in [
            "base_model",
            "base_model_name",
            "refiner_model",
            "full_prompt",
            "raw_prompt",
            "real_prompt",
            "Prompt",
            "Full prompt",
            "full_negative_prompt",
            "raw_negative_prompt",
            "real_negative_prompt",
            "Negative",
            "Negative Prompt",
        ] {
            let mut source = json!({"prompt":"cat","negative_prompt":"bad"});
            source[key] = nested.clone();
            assert_no_nested_generation(&scalar_generation("parameters", &source));
            assert_no_nested_generation(&scalar_generation("Comment", &source));
        }
        for key in [
            "positive_prompt",
            "positive",
            "prompt",
            "positive_style_prompt",
            "negative_prompt",
            "negative",
            "negative_style_prompt",
            "model",
            "model_name",
            "model_weights",
            "seed",
            "steps",
            "cfg_scale",
            "width",
            "height",
        ] {
            let mut source = json!({"positive_prompt":"cat","negative_prompt":"bad"});
            source[key] = nested.clone();
            assert_no_nested_generation(&scalar_generation("invokeai_metadata", &source));
        }
        for key in ["prompt", "negative_prompt", "negative"] {
            let mut source = json!({"image":{"prompt":"cat","negative_prompt":"bad"}});
            source["image"][key] = nested.clone();
            assert_no_nested_generation(&scalar_generation("sd-metadata", &source));
        }
        for key in [
            "prompt",
            "uc",
            "seed",
            "steps",
            "scale",
            "sampler",
            "width",
            "height",
            "v4_prompt",
            "v4_negative_prompt",
        ] {
            let mut source = json!({"prompt":"cat","uc":"bad"});
            source[key] = nested.clone();
            assert_no_nested_generation(&scalar_generation("Comment", &source));
        }
        for key in ["text", "ckpt_name", "seed", "steps", "cfg", "sampler_name"] {
            let mut source = json!({
                "1":{"class_type":"CLIPTextEncode","inputs":{"text":"cat"}},
                "2":{"class_type":"KSampler","inputs":{"positive":["1",0]}},
                "3":{"class_type":"CheckpointLoaderSimple","inputs":{}}
            });
            let node = match key {
                "text" => "1",
                "ckpt_name" => "3",
                _ => "2",
            };
            source[node]["inputs"][key] = nested.clone();
            assert_no_nested_generation(&scalar_generation("prompt", &source));
        }
        // RuinedFooocus tool detection and SwarmUI model-name/hash leaves also
        // pass through JSON-to-text conversion, even though tool is a fixed label.
        for key in ["software", "Software"] {
            let mut source = json!({"Prompt":"cat","Negative":"bad"});
            source[key] = nested.clone();
            assert_no_nested_generation(&scalar_generation("parameters", &source));
        }
        for key in ["name", "hash"] {
            let mut source =
                json!({"sui_image_params":{"prompt":"cat"},"sui_models":[{"name":"model"}]});
            source["sui_models"][0][key] = nested.clone();
            assert_no_nested_generation(&scalar_generation("parameters", &source));
        }
    }
}

#[test]
fn generation_preserves_valid_scalar_types_without_reinterpreting_text_as_json() {
    use serde_json::json;
    for seed in [json!(42), json!("42")] {
        let output = scalar_generation(
            "parameters",
            &json!({"sui_image_params":{
                "prompt":"literal {braces} and [arrays]", "negativeprompt":"bad",
                "model":"model.safetensors", "seed":seed, "steps":20, "sampler":"euler",
                "cfg":7.5, "width":7, "height":"5"
            }}),
        );
        let InfoField::Present(generation) = output else {
            panic!("missing generation")
        };
        assert_eq!(generation.tool.as_deref(), Some("SwarmUI"));
        assert_eq!(generation.model.as_deref(), Some("model"));
        assert_eq!(
            generation.prompt.as_ref().unwrap().text,
            "literal {braces} and [arrays]"
        );
        assert_eq!(generation.negative_prompt.as_ref().unwrap().text, "bad");
        assert_eq!(generation.seed.as_ref().unwrap().text, "42");
        assert_eq!(generation.steps.as_ref().unwrap().text, "20");
        assert_eq!(generation.sampler.as_ref().unwrap().text, "euler");
        assert_eq!(generation.cfg.as_ref().unwrap().text, "7.5");
        assert_eq!(generation.dimensions.as_ref().unwrap().text, "7x5");
    }
    // No currently exported generation column is boolean. Boolean flags in
    // metadata must not masquerade as numeric parameters or textual names.
    let output = scalar_generation(
        "parameters",
        &json!({"sui_image_params":{
            "prompt":"cat", "negativeprompt":true, "model":true, "sampler":true,
            "seed":true, "steps":false, "cfg":true, "width":true, "height":false
        }}),
    );
    let InfoField::Present(generation) = output else {
        panic!("missing generation")
    };
    assert!(generation.negative_prompt.is_none());
    assert!(generation.model.is_none());
    assert!(generation.sampler.is_none());
    assert!(generation.seed.is_none());
    assert!(generation.steps.is_none());
    assert!(generation.cfg.is_none());
    assert!(generation.dimensions.is_none());
}

#[test]
fn structured_chunks_never_fall_back_to_raw_generation_prompts() {
    use serde_json::json;
    let object = json!({"gps":{"latitude":35.5},"workflow_json":{"secret":"x"}});
    for source in [
        object.clone(),
        json!([object, "Steps: 20", "Sampler: euler"]),
    ] {
        for chunk in ["parameters", "Description", "Dream"] {
            let output = scalar_generation(chunk, &source);
            assert_eq!(output, InfoField::Unavailable, "{chunk}");
        }
        let output =
            project_generation(crate::png_metadata::extract_metadata_from_bytes_for_agent(
                &png(&[("prompt", "{}"), ("parameters", &source.to_string())]),
            ));
        assert_no_nested_generation(&output);
        let InfoField::Present(generation) = output else {
            panic!("missing ComfyUI")
        };
        assert!(generation.prompt.is_none());
    }
}

#[test]
fn zip_projection_uses_member_values_and_page_rating_with_separate_container_tags() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("book.zip");
    let image = png(&[("parameters", "page prompt\nSteps: 5, Seed: 123")]);
    std::fs::write(&path, zip_bytes(&[("pages/a.png", &image)])).unwrap();
    seed_tags(&path, "container-tag");
    let ratings = crate::rating_db::RatingDb::open().unwrap();
    ratings
        .set(&crate::adjustment_db::normalize_path(&path), 5)
        .unwrap();
    ratings
        .set(
            &crate::adjustment_db::zip_entry_key(&path, "pages/a.png"),
            2,
        )
        .unwrap();
    drop(ratings);
    let output = read(
        &item(&path, Some("pages/a.png")),
        permission(false),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(output.rating, 2);
    assert_eq!(output.size, InfoField::Present(image.len() as u64));
    assert_eq!(output.modified, InfoField::Present(1_580_702_706_000));
    assert_eq!(output.tags, InfoField::NotApplicable);
    assert_eq!(output.container_tags, vec!["#container-tag"]);
    assert_eq!(
        output.container_size,
        Some(std::fs::metadata(&path).unwrap().len())
    );
    assert_eq!(output.kind, ItemKind::Image);
    assert_eq!(
        output.dimensions,
        InfoField::Present(Dimensions {
            width: 7,
            height: 5,
            unit: DimensionUnit::Pixels
        })
    );
    assert!(matches!(output.generation, InfoField::Present(_)));
}

fn corrupt_zip_payload(bytes: &mut [u8], payload: &[u8]) {
    let offset = bytes
        .windows(payload.len())
        .position(|bytes| bytes == payload)
        .unwrap();
    bytes[offset] ^= 1;
}

#[test]
fn list_folder_zip_books_round_trip_through_item_info_and_rating_preparation() {
    use crate::agent_ipc::{ReadContext, metadata_write, tests::Fixture};
    use crate::book_composition::{BookCompositionService, PageDomain};
    use crate::book_scope::BookScopeKey;
    use crate::rating_db::RatingItemKind;
    use std::sync::Arc;
    use std::time::Duration;

    let fixture = Fixture::new();
    let path = fixture._dir.path().join("books.zip");
    let image = png(&[]);
    let inner = zip_bytes(&[("page.png", &image), ("chapter3/page.png", &image)]);
    std::fs::write(
        &path,
        zip_bytes(&[
            ("cover.png", &image),
            ("chapter2/page.png", &image),
            ("inner.zip", &inner),
        ]),
    )
    .unwrap();
    seed_tags(&path, "container-tag");
    let (service, owner) = BookCompositionService::spawn_at(crate::data_dir::get(), None).unwrap();
    service
        .prepare(
            BookScopeKey::document(fixture._dir.path().join("ready.pdf")).unwrap(),
            PageDomain::Document,
        )
        .recv_timeout(Duration::from_secs(5))
        .unwrap()
        .status
        .unwrap();
    fixture.handlers.readers.install_book_composition(service);
    let ctx = ReadContext::new(
        &fixture.handlers,
        (permission(false), 0),
        Arc::new(AtomicBool::new(false)),
    );
    let ratings = crate::rating_db::RatingDb::open().unwrap();
    ratings
        .set(&crate::adjustment_db::normalize_path(&path), 5)
        .unwrap();
    let mut books = Vec::new();
    for prefix in [None, Some("inner.zip/")] {
        let Payload::Folder(page) = fixture
            .handlers
            .list_folder(
                path.to_string_lossy().into_owned(),
                prefix.map(str::to_owned),
                None,
                None,
                &ctx,
            )
            .unwrap()
        else {
            panic!("expected folder page")
        };
        let inputs: Vec<_> = page
            .items
            .into_iter()
            .map(|row| {
                let PageRow::Value(row) = row else {
                    panic!("fixture row must fit")
                };
                ItemRef {
                    path: row.path,
                    subresource: row.subresource,
                }
            })
            .collect();
        let metadata_write::PreparedWrite::Ratings(prepared) = metadata_write::prepare(
            Command::SetRating {
                items: inputs.clone(),
                stars: 2,
            },
            &ctx,
            "fixture".into(),
        )
        .unwrap() else {
            panic!("expected ratings")
        };
        for (input, prepared) in inputs.iter().zip(prepared.items) {
            let target = prepared.target.unwrap();
            ratings.set(&target.key, 2).unwrap();
            let output = read(input, permission(false), &ctx.cancel).unwrap();
            assert_eq!(output.item, *input);
            assert_eq!(
                output.rating, 2,
                "must read the key prepared for set_rating"
            );
            assert_eq!(output.tags, InfoField::NotApplicable);
            assert_eq!(output.container_tags, ["#container-tag"]);
            assert_eq!(
                output.container_size,
                Some(std::fs::metadata(&path).unwrap().len())
            );
            if target.meta.kind == RatingItemKind::ZipDir {
                let prefix = input.subresource.as_deref().unwrap();
                let is_archive = crate::zip_tree::segment_is_archive(
                    crate::grid_item::zipdir_display_name(prefix),
                );
                assert_eq!(
                    target.key,
                    crate::adjustment_db::normalize_path(&crate::app::book_container_key(
                        &path, prefix
                    ),)
                );
                assert_eq!(target.meta.dir_prefix.as_deref(), Some(prefix));
                assert_eq!(target.meta.zipdir_is_archive, Some(is_archive));
                assert_eq!(
                    output.kind,
                    if is_archive {
                        ItemKind::Archive
                    } else {
                        ItemKind::Folder
                    }
                );
                assert_eq!(output.name, crate::grid_item::zipdir_display_name(prefix));
                assert_eq!(output.size, InfoField::NotApplicable);
                assert_eq!(output.modified, InfoField::NotApplicable);
                assert_eq!(output.dimensions, InfoField::NotApplicable);
                assert_eq!(output.exif, InfoField::NotApplicable);
                assert_eq!(output.generation, InfoField::NotApplicable);
                books.push(prefix.to_owned());
            }
        }
    }
    books.sort();
    assert_eq!(books, ["chapter2/", "inner.zip/", "inner.zip/chapter3/"]);
    owner.shutdown_and_join();
}

#[test]
fn zip_folder_info_does_not_read_image_or_terminal_nested_zip_payloads() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("unread.zip");
    let mut image_zip = zip_bytes(&[("chapter2/page.png", b"image-payload")]);
    corrupt_zip_payload(&mut image_zip, b"image-payload");
    std::fs::write(&path, image_zip).unwrap();
    assert!(read_zip(&path, "chapter2/page.png", &AtomicBool::new(false)).is_err());
    let folder = read(
        &item(&path, Some("chapter2/")),
        permission(false),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(folder.kind, ItemKind::Folder);
    assert_eq!(folder.dimensions, InfoField::NotApplicable);

    let mut terminal = zip_bytes(&[("book.cbz", b"archive-payload")]);
    corrupt_zip_payload(&mut terminal, b"archive-payload");
    std::fs::write(&path, &terminal).unwrap();
    assert!(read_zip(&path, "book.cbz", &AtomicBool::new(false)).is_err());
    assert_eq!(
        read(
            &item(&path, Some("book.cbz/")),
            permission(false),
            &AtomicBool::new(false)
        )
        .unwrap()
        .kind,
        ItemKind::Archive
    );
    std::fs::write(&path, zip_bytes(&[("inner.zip", &terminal)])).unwrap();
    assert!(read_zip(&path, "inner.zip/book.cbz", &AtomicBool::new(false)).is_err());
    assert_eq!(
        read(
            &item(&path, Some("inner.zip/book.cbz/")),
            permission(false),
            &AtomicBool::new(false)
        )
        .unwrap()
        .kind,
        ItemKind::Archive
    );
}

#[test]
fn zip_folder_info_preserves_legacy_rating_without_migration_and_prefers_current() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("legacy-folder.zip");
    let mut bytes = zip_bytes(&[("xxxx/page.png", b"image")]);
    let name = encoding_rs::SHIFT_JIS.encode("日本/page.png").0;
    let offsets: Vec<_> = bytes
        .windows(13)
        .enumerate()
        .filter_map(|(offset, bytes)| (bytes == b"xxxx/page.png").then_some(offset))
        .collect();
    for offset in offsets {
        bytes[offset..offset + 13].copy_from_slice(&name);
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
    let legacy_name = archive.by_index_raw(0).unwrap().name().to_owned();
    let legacy_prefix = legacy_name.strip_suffix("page.png").unwrap();
    std::fs::write(&path, &bytes).unwrap();
    let old = super::super::metadata_write::zip_directory_rating_key(&path, legacy_prefix);
    let current = super::super::metadata_write::zip_directory_rating_key(&path, "日本/");
    let db = crate::rating_db::RatingDb::open().unwrap();
    db.set(&old, 5).unwrap();
    let before = std::fs::read(crate::rating_db::RatingDb::db_path()).unwrap();
    for prefix in ["日本/", legacy_prefix] {
        let output = read(
            &item(&path, Some(prefix)),
            permission(false),
            &AtomicBool::new(false),
        )
        .unwrap();
        assert_eq!(output.item.subresource.as_deref(), Some("日本/"));
        assert_eq!(output.rating, 5);
    }
    assert_eq!(
        std::fs::read(crate::rating_db::RatingDb::db_path()).unwrap(),
        before
    );
    db.set(&current, 2).unwrap();
    assert_eq!(
        read(
            &item(&path, Some("日本/")),
            permission(false),
            &AtomicBool::new(false)
        )
        .unwrap()
        .rating,
        2
    );
    assert_eq!(db.get(&old), 5);
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
}

#[test]
fn zip_folder_info_rejects_missing_and_invalid_folders_with_existing_bad_request_contract() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("missing-folder.zip");
    let inner = zip_bytes(&[("page.png", b"image")]);
    std::fs::write(
        &path,
        zip_bytes(&[("chapter2/page.png", b"image"), ("inner.zip", &inner)]),
    )
    .unwrap();
    for prefix in [
        "missing/",
        "inner.zip/missing/",
        "/",
        "chapter2//",
        "chapter2/../",
        "../chapter2/",
    ] {
        assert_eq!(
            read(
                &item(&path, Some(prefix)),
                permission(false),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            ErrorCode::BadRequest
        );
    }
}

#[test]
fn bounded_zip_preserves_literal_nested_and_cp932_name_resolution() {
    let image = png(&[]);
    let nested = zip_bytes(&[("a.png", &image)]);
    let outer = zip_bytes(&[("inner.zip", &nested), ("inner.zip/a.png", &image)]);
    let result = read_zip_member(
        zip::ZipArchive::new(Cursor::new(outer)).unwrap(),
        "inner.zip/a.png",
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.name, "inner.zip/a.png");
    assert_eq!(result.bytes.unwrap(), image);
    let outer = zip_bytes(&[("inner.zip", &nested)]);
    let result = read_zip_member(
        zip::ZipArchive::new(Cursor::new(outer)).unwrap(),
        "inner.zip/a.png",
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.bytes.unwrap(), image);
    let mut cp932 = zip_bytes(&[("xxxx.png", &image)]);
    let encoded = encoding_rs::SHIFT_JIS.encode("日本.png").0;
    let indices = cp932
        .windows(8)
        .enumerate()
        .filter_map(|(i, value)| (value == b"xxxx.png").then_some(i))
        .collect::<Vec<_>>();
    for i in indices {
        cp932[i..i + 8].copy_from_slice(&encoded);
    }
    let result = read_zip_member(
        zip::ZipArchive::new(Cursor::new(cp932)).unwrap(),
        "日本.png",
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(result.name, "日本.png");
    assert_ne!(result.legacy_name, result.name);
    assert_eq!(result.bytes.unwrap(), image);
}

#[test]
fn nested_cp932_zip_rating_reads_legacy_key_without_migration_and_prefers_current() {
    fn cp932_zip(placeholder: &str, actual: &str, content: &[u8]) -> (Vec<u8>, String) {
        let mut bytes = zip_bytes(&[(placeholder, content)]);
        let encoded = encoding_rs::SHIFT_JIS.encode(actual).0;
        assert_eq!(encoded.len(), placeholder.len());
        let offsets: Vec<_> = bytes
            .windows(placeholder.len())
            .enumerate()
            .filter_map(|(i, value)| (value == placeholder.as_bytes()).then_some(i))
            .collect();
        for offset in offsets {
            bytes[offset..offset + placeholder.len()].copy_from_slice(&encoded);
        }
        let mut archive = zip::ZipArchive::new(Cursor::new(&bytes)).unwrap();
        let legacy = archive.by_index_raw(0).unwrap().name().replace('\\', "/");
        (bytes, legacy)
    }
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("legacy.zip");
    let (inner, legacy_leaf) = cp932_zip("xxxx.png", "日本.png", &png(&[]));
    let (outer, legacy_outer) = cp932_zip("xxxx.zip", "日本.zip", &inner);
    std::fs::write(&path, outer).unwrap();
    let legacy =
        crate::adjustment_db::zip_entry_key(&path, &format!("{legacy_outer}/{legacy_leaf}"));
    let current = crate::adjustment_db::zip_entry_key(&path, "日本.zip/日本.png");
    let db = crate::rating_db::RatingDb::open().unwrap();
    db.set(&legacy, 5).unwrap();
    let before = std::fs::read(crate::rating_db::RatingDb::db_path()).unwrap();
    let output = read(
        &item(&path, Some("日本.zip/日本.png")),
        permission(false),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(output.rating, 5);
    assert_eq!(
        output.item.subresource.as_deref(),
        Some("日本.zip/日本.png")
    );
    assert_eq!(
        std::fs::read(crate::rating_db::RatingDb::db_path()).unwrap(),
        before
    );
    assert_eq!(db.get(&legacy), 5);
    assert_eq!(db.get(&current), 0);
    db.set(&current, 2).unwrap();
    let output = read(
        &item(&path, Some("日本.zip/日本.png")),
        permission(false),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(output.rating, 2);
    assert_eq!(db.get(&legacy), 5);
}

#[test]
fn oversized_zip_metadata_is_rejected_before_decompression_and_actual_read_is_capped() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("large.zip");
    let mut archive = zip_bytes(&[("a.png", b"not an image")]);
    let central = archive
        .windows(4)
        .position(|value| value == b"PK\x01\x02")
        .unwrap();
    archive[central + 24..central + 28]
        .copy_from_slice(&((MAX_ZIP_BYTES + 1) as u32).to_le_bytes());
    archive[22..26].copy_from_slice(&((MAX_ZIP_BYTES + 1) as u32).to_le_bytes());
    std::fs::write(&path, archive).unwrap();
    let output = read(
        &item(&path, Some("a.png")),
        permission(false),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(output.size, InfoField::Present(MAX_ZIP_BYTES + 1));
    assert_eq!(output.dimensions, InfoField::TooLarge);
    assert_eq!(output.exif, InfoField::TooLarge);
    assert_eq!(output.generation, InfoField::TooLarge);
    let mut actual = Cursor::new(vec![0_u8; 100]);
    assert!(
        read_capped(&mut actual, 8, &AtomicBool::new(false))
            .unwrap()
            .is_none()
    );
    assert_eq!(
        actual.position(),
        9,
        "at most one cap-detection byte is read"
    );
    assert_eq!(
        read_capped(&mut Cursor::new(vec![0_u8; 8]), 8, &AtomicBool::new(false)).unwrap(),
        Some(vec![0; 8])
    );
}

#[test]
fn pdf_page_rating_and_container_tags_match_ui_without_starting_pdf_worker() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("book.pdf");
    std::fs::write(&path, b"fixture").unwrap();
    seed_tags(&path, "pdf-tag");
    let ratings = crate::rating_db::RatingDb::open().unwrap();
    ratings
        .set(&crate::adjustment_db::normalize_path(&path), 5)
        .unwrap();
    ratings
        .set(&crate::edit_source::page_key_for_pdf(&path, 2), 3)
        .unwrap();
    drop(ratings);
    let mut output = ItemInfo {
        item: item(&path, Some("page_2")),
        name: "Page 3".into(),
        kind: ItemKind::Image,
        size: InfoField::NotApplicable,
        modified: InfoField::NotApplicable,
        dimensions: InfoField::Unavailable,
        rating: 0,
        tags: InfoField::NotApplicable,
        container_size: Some(7),
        container_modified: None,
        container_tags: vec![],
        exif: InfoField::NotApplicable,
        generation: InfoField::NotApplicable,
        ai_caption: CaptionField::None,
        container_ai_caption: None,
    };
    enrich(
        &mut output,
        &GridItem::PdfPage {
            pdf_path: path.clone(),
            page_num: 2,
            content_type: None,
        },
        &path,
        None,
    )
    .unwrap();
    assert_eq!(output.rating, 3);
    assert_eq!(output.tags, InfoField::NotApplicable);
    assert_eq!(output.container_tags, vec!["#pdf-tag"]);
}

fn jpeg_with_gps() -> Vec<u8> {
    // Little-endian TIFF: IFD0 at 8, EXIF at 62, GPS at 92.
    let mut tiff = vec![0_u8; 170];
    tiff[..8].copy_from_slice(b"II*\0\x08\0\0\0");
    fn ifd(bytes: &mut Vec<u8>, offset: usize, entries: &[(u16, u16, u32, Vec<u8>)]) {
        bytes[offset..offset + 2].copy_from_slice(&(entries.len() as u16).to_le_bytes());
        for (index, (tag, format, count, data)) in entries.iter().enumerate() {
            let slot = offset + 2 + index * 12;
            bytes[slot..slot + 2].copy_from_slice(&tag.to_le_bytes());
            bytes[slot + 2..slot + 4].copy_from_slice(&format.to_le_bytes());
            bytes[slot + 4..slot + 8].copy_from_slice(&count.to_le_bytes());
            if data.len() <= 4 {
                bytes[slot + 8..slot + 8 + data.len()].copy_from_slice(data);
            } else {
                let location = bytes.len() as u32;
                bytes[slot + 8..slot + 12].copy_from_slice(&location.to_le_bytes());
                bytes.extend_from_slice(data);
            }
        }
    }
    fn rationals(values: &[u32]) -> Vec<u8> {
        values
            .iter()
            .flat_map(|value| [value.to_le_bytes(), 1_u32.to_le_bytes()].concat())
            .collect()
    }
    ifd(
        &mut tiff,
        8,
        &[
            (0x010f, 2, 9, b"TestMake\0".to_vec()),
            (0x0110, 2, 10, b"TestModel\0".to_vec()),
            (0x8769, 4, 1, 62_u32.to_le_bytes().to_vec()),
            (0x8825, 4, 1, 92_u32.to_le_bytes().to_vec()),
        ],
    );
    let comment = b"ASCII\0\0\0USER_COMMENT_SECRET";
    ifd(
        &mut tiff,
        62,
        &[
            (0x9003, 2, 20, b"2020:02:03 04:05:06\0".to_vec()),
            (0x9286, 7, comment.len() as u32, comment.to_vec()),
        ],
    );
    ifd(
        &mut tiff,
        92,
        &[
            (1, 2, 2, b"N\0".to_vec()),
            (2, 5, 3, rationals(&[35, 30, 0])),
            (3, 2, 2, b"E\0".to_vec()),
            (4, 5, 3, rationals(&[139, 45, 0])),
            (5, 1, 1, vec![1]),
            (6, 5, 1, rationals(&[12])),
        ],
    );
    let mut jpeg = Cursor::new(Vec::new());
    image::DynamicImage::new_rgb8(7, 5)
        .write_to(&mut jpeg, image::ImageFormat::Jpeg)
        .unwrap();
    let jpeg = jpeg.into_inner();
    let mut output = vec![0xff, 0xd8, 0xff, 0xe1];
    output.extend_from_slice(&((tiff.len() + 8) as u16).to_be_bytes());
    output.extend_from_slice(b"Exif\0\0");
    output.extend_from_slice(&tiff);
    output.extend_from_slice(&jpeg[2..]);
    output
}

#[test]
fn jpeg_gps_is_permission_scoped_and_raw_user_comment_is_never_exported() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("gps.jpg");
    std::fs::write(&path, jpeg_with_gps()).unwrap();
    for gps in [false, true] {
        let output = read(&item(&path, None), permission(gps), &AtomicBool::new(false)).unwrap();
        let InfoField::Present(exif) = &output.exif else {
            panic!("missing EXIF");
        };
        assert_eq!(exif.make.as_deref(), Some("TestMake"));
        assert_eq!(exif.model.as_deref(), Some("TestModel"));
        assert_eq!(exif.captured_at.as_deref(), Some("2020:02:03 04:05:06"));
        assert_eq!(
            exif.gps,
            gps.then_some(GpsInfo {
                latitude: 35.5,
                longitude: 139.75,
                altitude: Some(-12.0)
            })
        );
        let json = serde_json::to_string(&output).unwrap();
        assert!(!json.contains("USER_COMMENT_SECRET"));
        assert!(!json.contains("UserComment"));
        if !gps {
            assert!(!json.contains("35.5"));
            assert!(!json.contains("139.75"));
        }
    }
}

#[test]
fn invalid_references_and_pre_cancel_are_rejected_without_heavy_io() {
    let directory = crate::settings_db::DataDirOverrideGuard::new();
    let path = directory.path().join("missing.zip");
    for member in [
        "../a.png",
        "/a.png",
        "a/../../a.png",
        "C:/a.png",
        "a.txt",
        "a//b.png",
    ] {
        assert_eq!(
            read(
                &item(&path, Some(member)),
                permission(false),
                &AtomicBool::new(false)
            )
            .unwrap_err()
            .code,
            ErrorCode::BadRequest
        );
    }
    assert_eq!(
        read(
            &ItemRef {
                path: "relative.png".into(),
                subresource: None
            },
            permission(false),
            &AtomicBool::new(false)
        )
        .unwrap_err()
        .code,
        ErrorCode::BadRequest
    );
    assert_eq!(
        read(
            &item(&path, None),
            permission(false),
            &AtomicBool::new(true)
        )
        .unwrap_err()
        .code,
        ErrorCode::Cancelled
    );
    let missing = read(
        &item(&directory.path().join("missing.png"), None),
        permission(false),
        &AtomicBool::new(false),
    )
    .unwrap();
    assert_eq!(missing.size, InfoField::Unavailable);
    assert_eq!(missing.dimensions, InfoField::Unavailable);
}

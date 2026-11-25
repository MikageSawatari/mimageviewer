use mimageviewer_agent_ipc as current;
use serde_json::{Value, json};
mod v3_wire;

fn hello() -> v3_wire::Request {
    v3_wire::Request {
        id: 1,
        command: v3_wire::Command::Hello {
            version: 3,
            client_name: "fixture".into(),
            client_version: "1".into(),
            request_kind: v3_wire::RequestKind::Tool,
        },
    }
}

#[test]
fn shipped_hello_fixture_has_exact_adapter_bytes() {
    let mut bytes = vec![];
    v3_wire::write_frame(&mut bytes, &hello()).unwrap();
    assert_eq!(bytes, include_bytes!("v3_wire/hello.frame"));
    assert_eq!(v3_wire::AGENT_PROTOCOL_VERSION, 3);
    assert_eq!(current::AGENT_PROTOCOL_VERSION, 7);
    assert_eq!(current::MIN_ADAPTER_PROTOCOL_VERSION, 3);
    for version in [0, 2, 3, 4, 5, 6, 7, 8, u32::MAX] {
        assert_eq!(
            current::supports_adapter_version(version),
            matches!(version, 3 | 4 | 5 | 6 | 7)
        );
        assert_eq!(current::check_version(version).is_ok(), version == 7);
    }
}

#[test]
fn shipped_output_limits_are_preserved() {
    macro_rules! same {
        ($($name:ident),+ $(,)?) => { $(assert_eq!(current::$name, v3_wire::$name, stringify!($name));)+ };
    }
    for version in [3, 4] {
        assert_eq!(
            current::clamp_thumbnail_side(version, 1568),
            v3_wire::MAX_THUMBNAIL_SIDE
        );
        let budget =
            current::thumbnail_budget(version, current::clamp_thumbnail_side(version, 1568));
        assert_eq!(budget.max_image_bytes, v3_wire::MAX_THUMBNAIL_IMAGE_BYTES);
        assert_eq!(budget.max_total_bytes, v3_wire::MAX_THUMBNAIL_TOTAL_BYTES);
        assert_eq!(budget.max_images, v3_wire::MAX_THUMBNAIL_ITEMS);
    }
    same!(
        MAX_FRAME_BYTES,
        DEFAULT_LIMIT,
        MAX_LIMIT,
        MAX_INFO_LIMIT,
        DEFAULT_FOLDER_LIMIT,
        MAX_THUMBNAIL_ITEMS,
        MAX_WRITE_ITEMS,
        MAX_WRITE_TAGS,
        DEFAULT_THUMBNAIL_SIDE,
        MAX_THUMBNAIL_IMAGE_BYTES,
        MAX_THUMBNAIL_TOTAL_BYTES,
        MAX_ITEM_CHARS,
        MCP_ENVELOPE_RESERVE_CHARS,
        MAX_MCP_RESPONSE_CHARS,
        MAX_PAGE_CHARS,
        MAX_CLIENT_NAME_CHARS,
        MAX_CLIENT_VERSION_CHARS
    );
}

#[cfg(windows)]
#[test]
fn pipe_name_is_the_shipped_v3_name_even_with_business_protocol_5() {
    let path = std::path::Path::new("C:/Users/Test/data");
    let name = current::derive_pipe_name(path, "S-1-5-21-123").unwrap();
    assert_eq!(
        name,
        v3_wire::derive_pipe_name(path, "S-1-5-21-123").unwrap()
    );
    assert_eq!(current::AGENT_PIPE_NAMESPACE, "mimageviewer-agent-v3");
    assert!(name.starts_with(r"\\.\pipe\mimageviewer-agent-v3-S-1-5-21-123-"));
    // Current 5 versus frozen 3 exercises a real business protocol bump.
}

fn payload(kind: &str, data: Value) -> current::Payload {
    serde_json::from_value(json!({"type":kind,"data":data})).unwrap()
}

fn check_response(tool: &str, response: current::Response, expected_error: bool) -> Value {
    let mut wire = vec![];
    current::write_frame(&mut wire, &response).unwrap();
    let old: v3_wire::Response = v3_wire::read_frame(&mut wire.as_slice()).unwrap().unwrap();
    // Execute the actual shipped typed conversion and output budget checks.
    let result = v3_wire::output::tool_result(tool, old.result);
    assert_eq!(result["isError"], expected_error, "{tool}: {result}");
    let tools = v3_wire::schema::tools();
    let schema = &tools.iter().find(|entry| entry["name"] == tool).unwrap()["outputSchema"];
    assert!(
        v3_wire::schema_check::accepts_schema(schema, &result["structuredContent"]),
        "{tool}: {result}"
    );
    assert_eq!(
        serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
        result["structuredContent"]
    );
    result
}

fn ok(tool: &str, value: current::Payload) -> Value {
    check_response(
        tool,
        current::Response {
            id: 2,
            result: Ok(value),
        },
        false,
    )
}

#[test]
fn all_shipped_tool_response_families_decode_and_pass_old_structured_output_checks() {
    let status = current::Payload::Status(current::StatusPayload {
        search_scope: vec![],
        search_scope_truncated: false,
        app_version: "fixture".into(),
        permission: current::PermissionSnapshot {
            level: current::AgentLevel::Organize,
            thumbnails: true,
            gps: true,
            ai_captions: true,
            transcripts: true,
            current_view: true,
            settings: true,
        },
        fts_ready: true,
        remote_active: false,
    });
    let result = ok("miv_status", status);
    assert!(
        result["structuredContent"]["permission"]
            .get("transcripts")
            .is_none()
    );
    let marker = json!({"id":"a".repeat(64),"error":"too_long"});
    let items = current::Payload::Items(current::ItemPage {
        items: [
            current::ItemKind::Image,
            current::ItemKind::Pdf,
            current::ItemKind::Video,
            current::ItemKind::Audio,
            current::ItemKind::Archive,
            current::ItemKind::Folder,
            current::ItemKind::Other,
        ]
        .into_iter()
        .map(|kind| {
            current::Item {
                path: "C:/猫.png".into(),
                subresource: None,
                name: "猫".into(),
                kind,
                tags: Some(vec!["tag".into()]),
                container_tags: Some(vec!["book".into()]),
                rating: 3,
            }
            .into()
        })
        .chain([current::PageRow::too_long(&"oversized").unwrap()])
        .collect(),
        cursor: Some("cursor".into()),
        truncated: true,
    });
    for tool in ["search_items", "items_by_tag", "get_collection"] {
        ok(tool, items.clone());
    }
    for (tool, kind, key, row) in [
        ("list_tags", "tags", "tags", json!({"name":"tag","count":3})),
        (
            "list_collections",
            "collections",
            "collections",
            json!({"id":"id","name":"collection","count":3}),
        ),
        (
            "list_library",
            "library",
            "items",
            json!({"path":"C:/猫.png","subresource":null,"name":"猫","kind":"image","id":"id","page":1,"added_at":1,"last_read_at":2,"rated_at":3,"rating":4}),
        ),
    ] {
        let mut page = json!({"cursor":"cursor","truncated":true});
        page[key] = json!([row, marker]);
        ok(tool, payload(kind, page));
    }
    let info = json!({
        "item":{"path":"C:/音声.mp3","subresource":null}, "name":"音声", "kind":"audio",
        "size":{"status":"present","value":42}, "modified":{"status":"present","value":1},
        "dimensions":{"status":"present","value":{"width":10,"height":20,"unit":"pixels"}},
        "rating":3,"tags":{"status":"present","value":["tag"]},
        "container_size":42,"container_modified":1,"container_tags":["book"],
        "exif":{"status":"present","value":{"make":"camera","gps":{"latitude":1.0,"longitude":2.0,"altitude":3.0}}},
        "generation":{"status":"present","value":{"model":"model","prompt":{"text":"猫","truncated":true}}},
        "ai_caption":{"status":"present","value":{"caption":{"text":"猫","truncated":false},"visible_text":{"text":"text","truncated":false},"keywords":["cat"],"model":"model","created_at":1,"stale":false}},
        "container_ai_caption":{"status":"none"},
        "transcript":{"status":"present","value":{"model":"whisper","created_at":1,"audio_stream":1,"duration_secs":5.0,"segment_count":2,"char_count":3,"stale":false}}
    });
    let result = ok(
        "get_item_info",
        payload(
            "info",
            json!({"items":[info,marker],"cursor":null,"truncated":false}),
        ),
    );
    assert!(
        result["structuredContent"]["items"][0]
            .get("transcript")
            .is_none()
    );

    let folder_rows: Vec<_> = [
        current::FolderKind::Folder,
        current::FolderKind::Image,
        current::FolderKind::Video,
        current::FolderKind::Audio,
        current::FolderKind::Zip,
        current::FolderKind::Pdf,
        current::FolderKind::Epub,
        current::FolderKind::Archive,
        current::FolderKind::ZipFolder,
        current::FolderKind::PdfPage,
        current::FolderKind::ZipEntry,
    ]
    .into_iter()
    .map(|kind| {
        current::FolderItem {
            path: "C:/fixture".into(),
            subresource: Some("page".into()),
            name: "fixture".into(),
            kind,
            size: Some(42),
            modified: Some(1),
            rating: Some(3),
            tags: Some(vec!["tag".into()]),
        }
        .into()
    })
    .collect();
    ok(
        "list_folder",
        current::Payload::Folder(current::FolderPage {
            items: folder_rows,
            cursor: Some("cursor".into()),
            truncated: true,
            book_composition_applied: true,
        }),
    );
    ok(
        "open_in_viewer",
        current::Payload::Opened(current::OpenResult {
            status: current::OpenStatus::Accepted,
            message: "開きました".into(),
        }),
    );
    for (tool, data) in [
        (
            "create_collection",
            json!({"operation":"created","id":"id"}),
        ),
        (
            "rename_collection",
            json!({"operation":"renamed","id":"id","changed":true}),
        ),
        (
            "add_to_collection",
            json!({"operation":"items","id":"id","outcomes":[
            {"status":"added","indices":[0]}, {"status":"duplicate","indices":[1]},
            {"status":"removed","indices":[2]}, {"status":"not_present","indices":[3]},
            {"status":"capacity_rejected","indices":[4]},
            {"status":"skipped","indices":[5],"reason":"invalid_path"},
            {"status":"skipped","indices":[6],"reason":"not_found"},
            {"status":"skipped","indices":[7],"reason":"unsupported_subresource"},
            {"status":"skipped","indices":[8],"reason":"unsupported_type"}]}),
        ),
    ] {
        let value = payload("collection_write", data);
        ok(tool, value.clone());
        if tool == "add_to_collection" {
            ok("remove_from_collection", value);
        }
    }
    for (tool, kind, status) in [
        ("add_tags", "tag_write", "queued"),
        ("remove_tags", "tag_write", "queued"),
        ("set_rating", "rating_write", "changed"),
    ] {
        let mut outcomes = vec![json!({"status":status,"indices":[0]})];
        if tool == "set_rating" {
            outcomes.push(json!({"status":"unchanged","indices":[1]}));
        }
        for (i, reason) in [
            "invalid_path",
            "not_found",
            "unsupported_type",
            "unsupported_subresource",
            "unavailable",
        ]
        .into_iter()
        .enumerate()
        {
            outcomes.push(json!({"status":"skipped","indices":[i+2],"reason":reason}));
        }
        ok(tool, payload(kind, json!({"outcomes":outcomes})));
    }
}

#[test]
fn every_old_error_code_passes_typed_decode_and_shipped_error_output() {
    use current::ErrorCode::*;
    for code in [
        Disabled,
        PermissionChanged,
        NotReady,
        BadRequest,
        Conflict,
        VersionMismatch,
        Busy,
        Cancelled,
        Timeout,
        Internal,
    ] {
        // Exhaustive match makes new error variants require compatibility review.
        let _: () = match code {
            Disabled | PermissionChanged | NotReady | BadRequest | Conflict | VersionMismatch
            | Busy | Cancelled | Timeout | Internal => (),
        };
        check_response(
            "miv_status",
            current::Response {
                id: 2,
                result: Err(current::AgentError::new(code, "fixture")),
            },
            true,
        );
    }
}

#[test]
fn maximum_image_budget_survives_old_wire_decoder_and_attachment_validation() {
    let value = current::ThumbnailResponse {
        page: current::ThumbnailPage {
            items: (0..4)
                .map(|_| {
                    current::ThumbnailItem {
                        item: current::ItemRef {
                            path: "C:/猫.png".into(),
                            subresource: None,
                        },
                        status: current::ThumbnailStatus::Ok,
                        dimensions: Some(current::Dimensions {
                            width: 512,
                            height: 512,
                            unit: current::DimensionUnit::Pixels,
                        }),
                    }
                    .into()
                })
                .collect(),
            cursor: None,
            truncated: false,
        },
        images: (0..4)
            .map(|item_index| current::ThumbnailImage {
                item_index,
                webp: vec![255; current::MAX_THUMBNAIL_IMAGE_BYTES],
            })
            .collect(),
    };
    let result = ok("get_thumbnail", current::Payload::Thumbnails(value));
    assert_eq!(result["content"].as_array().unwrap().len(), 5);
}

#[test]
fn existing_field_and_thumbnail_state_variants_remain_readable_to_old_adapter() {
    for status in ["not_applicable", "unavailable", "too_large"] {
        for caption in ["none", "not_permitted"] {
            ok(
                "get_item_info",
                payload(
                    "info",
                    json!({
                        "items":[{
                            "item":{"path":"C:/fixture.png","subresource":null},
                            "name":"fixture", "kind":"image", "rating":0,
                            "size":{"status":status}, "modified":{"status":status},
                            "dimensions":{"status":status}, "tags":{"status":status},
                            "container_size":null, "container_modified":null, "container_tags":[],
                            "exif":{"status":status}, "generation":{"status":status},
                            "ai_caption":{"status":caption}, "container_ai_caption":{"status":caption},
                            "transcript":{"status":"not_permitted"}
                        }], "cursor":null, "truncated":false
                    }),
                ),
            );
        }
    }
    let statuses = [
        current::ThumbnailStatus::TooLarge,
        current::ThumbnailStatus::Unavailable,
        current::ThumbnailStatus::Unsupported,
        current::ThumbnailStatus::HiddenByEdit,
    ];
    let items = (0..current::MAX_THUMBNAIL_ITEMS)
        .map(|i| {
            current::ThumbnailItem {
                item: current::ItemRef {
                    path: "C:/fixture.pdf".into(),
                    subresource: Some(i.to_string()),
                },
                status: statuses[i % statuses.len()],
                dimensions: Some(current::Dimensions {
                    width: 100,
                    height: 200,
                    unit: current::DimensionUnit::Points,
                }),
            }
            .into()
        })
        .collect();
    ok(
        "get_thumbnail",
        current::Payload::Thumbnails(current::ThumbnailResponse {
            page: current::ThumbnailPage {
                items,
                cursor: Some("cursor".into()),
                truncated: true,
            },
            images: vec![],
        }),
    );
}

#[test]
fn enums_reachable_by_old_adapters_keep_their_complete_shipped_variant_sets() {
    // The exhaustive match deliberately fails to compile when a new variant is
    // added: it needs a negotiated-version translation before reaching v3.
    macro_rules! check {
        ($new:ty, $old:ty; $($variant:path),+ $(,)?) => {
            for value in [$($variant),+] {
                let _: $new = value;
                match value { $($variant => (),)+ }
                let wire = serde_json::to_value(value).unwrap();
                let old: $old = serde_json::from_value(wire.clone()).unwrap();
                assert_eq!(serde_json::to_value(old).unwrap(), wire);
            }
        };
    }
    check!(current::ItemKind, v3_wire::ItemKind;
        current::ItemKind::Image, current::ItemKind::Pdf, current::ItemKind::Video,
        current::ItemKind::Audio, current::ItemKind::Archive, current::ItemKind::Folder,
        current::ItemKind::Other);
    check!(current::AgentLevel, v3_wire::AgentLevel;
        current::AgentLevel::Disabled, current::AgentLevel::ReadOnly,
        current::AgentLevel::ReadAndOpen, current::AgentLevel::Organize);
    check!(current::FolderKind, v3_wire::FolderKind;
        current::FolderKind::Folder, current::FolderKind::Image, current::FolderKind::Video,
        current::FolderKind::Audio, current::FolderKind::Zip, current::FolderKind::Pdf,
        current::FolderKind::Epub, current::FolderKind::Archive, current::FolderKind::ZipFolder,
        current::FolderKind::PdfPage, current::FolderKind::ZipEntry);
    check!(current::DimensionUnit, v3_wire::DimensionUnit;
        current::DimensionUnit::Pixels, current::DimensionUnit::Points);
    check!(current::ThumbnailStatus, v3_wire::ThumbnailStatus;
        current::ThumbnailStatus::Ok, current::ThumbnailStatus::TooLarge,
        current::ThumbnailStatus::Unavailable, current::ThumbnailStatus::Unsupported,
        current::ThumbnailStatus::HiddenByEdit);
    check!(current::RowError, v3_wire::RowError; current::RowError::TooLong);
    check!(current::OpenStatus, v3_wire::OpenStatus; current::OpenStatus::Accepted);
    check!(current::CollectionSkipReason, v3_wire::CollectionSkipReason;
        current::CollectionSkipReason::InvalidPath, current::CollectionSkipReason::NotFound,
        current::CollectionSkipReason::UnsupportedSubresource, current::CollectionSkipReason::UnsupportedType);
    check!(current::MetadataSkipReason, v3_wire::MetadataSkipReason;
        current::MetadataSkipReason::InvalidPath, current::MetadataSkipReason::NotFound,
        current::MetadataSkipReason::UnsupportedType, current::MetadataSkipReason::UnsupportedSubresource,
        current::MetadataSkipReason::Unavailable);
}

#[test]
fn shipped_folder_requests_default_to_nonrecursive_unfiltered_expansion() {
    let request = v3_wire::Request {
        id: 17,
        command: v3_wire::Command::ListFolder {
            path: "C:/photos".into(),
            subresource: None,
            min_stars: Some(0),
            max_stars: Some(3),
            kinds: Some(vec![v3_wire::FolderKind::Image]),
            limit: Some(2),
            cursor: None,
        },
    };
    let mut frame = vec![];
    v3_wire::write_frame(&mut frame, &request).unwrap();
    let decoded: current::Request = current::read_frame(&mut frame.as_slice()).unwrap().unwrap();
    assert!(matches!(
        decoded.command,
        current::Command::ListFolder {
            recursive: false,
            max_depth: None,
            name_contains: None,
            extensions: None,
            tagged: None,
            min_stars: Some(0),
            max_stars: Some(3),
            limit: Some(2),
            ..
        }
    ));
    let old: v3_wire::Request =
        serde_json::from_value(serde_json::to_value(decoded).unwrap()).unwrap();
    assert_eq!(old, request);
}

#[test]
fn new_wire_decoder_accepts_one_mib_but_frozen_adapter_keeps_256_kib_cap() {
    for bytes in [
        current::MAX_THUMBNAIL_IMAGE_BYTES,
        current::MAX_THUMBNAIL_IMAGE_BYTES + 1,
        current::MAX_THUMBNAIL_WIRE_IMAGE_BYTES,
        current::MAX_THUMBNAIL_WIRE_IMAGE_BYTES + 1,
    ] {
        let image = current::ThumbnailImage {
            item_index: 0,
            webp: vec![1; bytes],
        };
        let wire = serde_json::to_value(image).unwrap();
        assert_eq!(
            serde_json::from_value::<current::ThumbnailImage>(wire.clone()).is_ok(),
            bytes <= current::MAX_THUMBNAIL_WIRE_IMAGE_BYTES
        );
        assert_eq!(
            serde_json::from_value::<v3_wire::ThumbnailImage>(wire).is_ok(),
            bytes <= v3_wire::MAX_THUMBNAIL_IMAGE_BYTES
        );
    }
}

#[test]
fn search_scope_is_discarded_before_old_adapter_structured_output() {
    let result = ok(
        "miv_status",
        current::Payload::Status(current::StatusPayload {
            app_version: "fixture".into(),
            permission: current::PermissionSnapshot::default(),
            fts_ready: true,
            remote_active: false,
            search_scope: vec!["C:/private-index".into()],
            search_scope_truncated: true,
        }),
    );
    assert!(result["structuredContent"].get("search_scope").is_none());
    assert!(
        result["structuredContent"]
            .get("search_scope_truncated")
            .is_none()
    );
    assert!(!result.to_string().contains("private-index"));
}

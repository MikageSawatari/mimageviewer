use super::*;
use std::{sync::Mutex, time::Duration};
struct Input {
    receiver: mpsc::Receiver<Vec<u8>>,
    current: io::Cursor<Vec<u8>>,
}

#[test]
fn catalog_tools_forward_and_validate_paging_arguments() {
    for name in ["list_tags", "list_collections"] {
        let mut arguments = json!({"limit":17,"cursor":"opaque"});
        if name == "list_tags" {
            arguments["choice"] = json!("all");
        }
        let (_, command) = parse_tool(&json!({"name":name,"arguments":arguments})).unwrap();
        assert_eq!(
            command,
            if name == "list_tags" {
                Command::ListTags {
                    choice: TagChoice::All,
                    limit: Some(17),
                    cursor: Some("opaque".into()),
                }
            } else {
                Command::ListCollections {
                    limit: Some(17),
                    cursor: Some("opaque".into()),
                }
            }
        );
        for bad in [json!(0), json!("50")] {
            arguments["limit"] = bad;
            assert!(parse_tool(&json!({"name":name,"arguments":arguments})).is_err());
        }
        arguments["limit"] = json!(null);
        arguments["cursor"] = json!("x".repeat(8193));
        assert!(parse_tool(&json!({"name":name,"arguments":arguments})).is_err());
        arguments["cursor"] = json!(null);
        assert!(parse_tool(&json!({"name":name,"arguments":arguments})).is_ok());
        let tool = schema::tools()
            .into_iter()
            .find(|tool| tool["name"] == name)
            .unwrap();
        assert_eq!(tool["inputSchema"]["properties"]["limit"]["default"], 50);
        assert_eq!(tool["inputSchema"]["properties"]["limit"]["maximum"], 200);
        assert_eq!(
            tool["inputSchema"]["properties"]["cursor"]["maxLength"],
            8192
        );
    }
}

fn thumbnail_fixture(statuses: &[ThumbnailStatus], image_bytes: usize) -> ThumbnailResponse {
    ThumbnailResponse {
        page: ThumbnailPage {
            items: statuses
                .iter()
                .enumerate()
                .map(|(index, &status)| {
                    ThumbnailItem {
                        item: ItemRef {
                            path: format!("C:/猫-{index}.png"),
                            subresource: None,
                        },
                        status,
                        dimensions: (status == ThumbnailStatus::Ok).then_some(Dimensions {
                            width: 256,
                            height: 128,
                            unit: DimensionUnit::Pixels,
                        }),
                    }
                    .into()
                })
                .collect(),
            cursor: Some("next-input-position".into()),
            truncated: true,
        },
        images: statuses
            .iter()
            .enumerate()
            .filter(|(_, status)| **status == ThumbnailStatus::Ok)
            .map(|(item_index, _)| ThumbnailImage {
                item_index,
                webp: vec![255; image_bytes],
            })
            .collect(),
    }
}

#[test]
fn stage1b_argument_contracts_and_schema() {
    let (_, folder) = parse_tool(&json!({"name":"list_folder","arguments":{"path":"C:/book.zip","subresource":"pages/","limit":999,"cursor":"next"}})).unwrap();
    assert_eq!(
        folder,
        Command::ListFolder {
            path: "C:/book.zip".into(),
            subresource: Some("pages/".into()),
            limit: Some(200),
            cursor: Some("next".into())
        }
    );
    let (_, thumbnail) = parse_tool(&json!({"name":"get_thumbnail","arguments":{"items":[{"path":"C:/book.zip","subresource":"cat.png"}],"long_side":999,"cursor":"next"}})).unwrap();
    assert_eq!(
        thumbnail,
        Command::GetThumbnail {
            items: vec![ItemRef {
                path: "C:/book.zip".into(),
                subresource: Some("cat.png".into())
            }],
            long_side: Some(512),
            cursor: Some("next".into())
        }
    );
    for arguments in [
        json!({"items":[]}),
        json!({"items":[{"path":""}]}),
        json!({"items":vec![json!({"path":"C:/a.png"});21]}),
        json!({"items":[{"path":"C:/a.png"}],"long_side":255}),
        json!({"items":[{"path":"C:/a.png"}],"limit":1}),
        json!({"items":[{"path":"C:/a.png","extra":true}]}),
        json!({"items":[{"path":"C:/a.png"}],"cursor":"x".repeat(8193)}),
    ] {
        assert!(parse_tool(&json!({"name":"get_thumbnail","arguments":arguments})).is_err());
    }
    for arguments in [
        json!({"path":""}),
        json!({"path":"C:/book.zip","limit":0}),
        json!({"path":"C:/book.zip","extra":true}),
    ] {
        assert!(parse_tool(&json!({"name":"list_folder","arguments":arguments})).is_err());
    }
    let tools = schema::tools();
    let folder = tools
        .iter()
        .find(|tool| tool["name"] == "list_folder")
        .unwrap();
    assert_eq!(folder["inputSchema"]["properties"]["limit"]["default"], 100);
    assert!(
        folder["description"]
            .as_str()
            .unwrap()
            .contains("UI search")
    );
    let thumbnail = tools
        .iter()
        .find(|tool| tool["name"] == "get_thumbnail")
        .unwrap();
    assert!(
        thumbnail["inputSchema"]["properties"]
            .get("limit")
            .is_none()
    );
    assert_eq!(
        thumbnail["inputSchema"]["properties"]["long_side"]["default"],
        256
    );
    assert_eq!(
        thumbnail["inputSchema"]["properties"]["long_side"]["maximum"],
        512
    );
    for name in ["list_folder", "get_thumbnail"] {
        let tool = tools.iter().find(|tool| tool["name"] == name).unwrap();
        let error = tool_result(
            name,
            Err(AgentError::new(ErrorCode::PermissionChanged, "changed")),
        );
        assert!(accepts_schema(
            &tool["outputSchema"],
            &error["structuredContent"]
        ));
    }
    let statuses = [
        ThumbnailStatus::Ok,
        ThumbnailStatus::TooLarge,
        ThumbnailStatus::Unavailable,
        ThumbnailStatus::Unsupported,
        ThumbnailStatus::HiddenByEdit,
    ];
    let result = tool_result(
        "get_thumbnail",
        Ok(Payload::Thumbnails(thumbnail_fixture(&statuses, 8))),
    );
    assert!(accepts_schema(
        &thumbnail["outputSchema"],
        &result["structuredContent"]
    ));
    let invalid = json!({"items":[{"item":{"path":"C:/a.png","subresource":null},"status":"hidden","dimensions":null}],"cursor":null,"truncated":false});
    assert!(!accepts_schema(&thumbnail["outputSchema"], &invalid));
    let result = tool_result(
        "list_folder",
        Ok(Payload::Folder(FolderPage {
            items: vec![
                FolderItem {
                    path: "C:/book.zip".into(),
                    subresource: Some("pages/".into()),
                    name: "pages".into(),
                    kind: FolderKind::ZipFolder,
                    size: None,
                    modified: None,
                }
                .into(),
            ],
            cursor: None,
            truncated: false,
            book_composition_applied: true,
        })),
    );
    assert!(accepts_schema(
        &folder["outputSchema"],
        &result["structuredContent"]
    ));
}

#[test]
fn stage1b_images_stay_out_of_metadata_and_have_separate_budgets() {
    let thumbnails = thumbnail_fixture(&[ThumbnailStatus::Ok; 4], MAX_THUMBNAIL_IMAGE_BYTES);
    let page = serde_json::to_value(&thumbnails.page).unwrap();
    let page_chars = mcp_content_chars(&page).unwrap();
    let result = tool_result("get_thumbnail", Ok(Payload::Thumbnails(thumbnails)));
    assert_eq!(result["isError"], false);
    assert_eq!(result["structuredContent"], page);
    assert_eq!(
        serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
        page
    );
    assert_eq!(result["content"].as_array().unwrap().len(), 5);
    for block in result["content"].as_array().unwrap().iter().skip(1) {
        assert_eq!(block["type"], "image");
        assert_eq!(block["mimeType"], "image/webp");
        assert_eq!(
            STANDARD
                .decode(block["data"].as_str().unwrap())
                .unwrap()
                .len(),
            MAX_THUMBNAIL_IMAGE_BYTES
        );
    }
    for modern in [true, false] {
        let envelope = success(json!("id".repeat(128)), result.clone(), modern);
        let serialized = serialize_response(&envelope);
        assert!(serialized.len() > MAX_MCP_RESPONSE_CHARS);
        assert!(
            serde_json::from_str::<Value>(&serialized)
                .unwrap()
                .get("error")
                .is_none()
        );
        let mut stripped = envelope;
        for block in stripped["result"]["content"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .skip(1)
        {
            block["data"] = json!("");
        }
        assert!(stripped.to_string().chars().count() - page_chars <= MCP_ENVELOPE_RESERVE_CHARS);
    }
    for thumbnails in [
        thumbnail_fixture(&[ThumbnailStatus::Ok], MAX_THUMBNAIL_IMAGE_BYTES + 1),
        thumbnail_fixture(&[ThumbnailStatus::Ok; 5], MAX_THUMBNAIL_IMAGE_BYTES),
    ] {
        let result = tool_result("get_thumbnail", Ok(Payload::Thumbnails(thumbnails)));
        assert_eq!(result["isError"], true);
        assert_eq!(result["content"].as_array().unwrap().len(), 1);
    }
    let mut oversized = thumbnail_fixture(&[ThumbnailStatus::Ok], 8);
    let PageRow::Value(row) = &mut oversized.page.items[0] else {
        panic!()
    };
    row.item.path = "猫\\\"".repeat(12_000);
    assert_eq!(
        tool_result("get_thumbnail", Ok(Payload::Thumbnails(oversized)))["content"]
            .as_array()
            .unwrap()
            .len(),
        1
    );
    let too_large = success(
        json!(1),
        json!({"content":[{"type":"image","data":"x".repeat(MAX_FRAME_BYTES)}]}),
        true,
    );
    assert!(
        serde_json::from_str::<Value>(&serialize_response(&too_large))
            .unwrap()
            .get("error")
            .is_some()
    );
}

#[test]
fn stage1b_final_budget_exempts_only_image_content_data() {
    let small = json!({"jsonrpc":"2.0","id":1,"result":{
        "structuredContent":{"data":"metadata","nested":{"type":"image","data":"metadata"}},
        "content":[{"type":"text","text":"metadata","data":"metadata"},{"type":"image","mimeType":"image/webp","data":""}],
        "isError":false
    }});
    let baseline = small.to_string().chars().count();
    let mut response = small.clone();
    response["result"]["content"][1]["data"] = json!("x".repeat(MAX_THUMBNAIL_TOTAL_BYTES));
    assert_eq!(response_non_image_chars(&response), baseline);
    assert!(
        serde_json::from_str::<Value>(&serialize_response(&response))
            .unwrap()
            .get("error")
            .is_none()
    );
    for pointer in [
        "/result/structuredContent/data",
        "/result/structuredContent/nested/data",
        "/result/content/0/text",
        "/result/content/0/data",
    ] {
        let mut oversized = response.clone();
        *oversized.pointer_mut(pointer).unwrap() = json!("x".repeat(MAX_MCP_RESPONSE_CHARS));
        assert!(
            serde_json::from_str::<Value>(&serialize_response(&oversized))
                .unwrap()
                .get("error")
                .is_some(),
            "{pointer}"
        );
    }
    let mut exact = small;
    exact["result"]["content"][1]["data"] = json!("encoded image");
    exact["result"]["structuredContent"]["padding"] = json!("");
    let padding = MAX_MCP_RESPONSE_CHARS - response_non_image_chars(&exact);
    exact["result"]["structuredContent"]["padding"] = json!("x".repeat(padding));
    assert_eq!(response_non_image_chars(&exact), MAX_MCP_RESPONSE_CHARS);
    assert!(
        serde_json::from_str::<Value>(&serialize_response(&exact))
            .unwrap()
            .get("error")
            .is_none()
    );
    exact["result"]["structuredContent"]["padding"] = json!("x".repeat(padding + 1));
    assert!(
        serde_json::from_str::<Value>(&serialize_response(&exact))
            .unwrap()
            .get("error")
            .is_some()
    );
}

#[test]
fn stage1b_attachment_indices_must_match_exactly_the_ok_rows() {
    assert!(!valid_thumbnail_attachments(&thumbnail_fixture(
        &[ThumbnailStatus::Unsupported; 21],
        0
    )));
    let fixture = thumbnail_fixture(
        &[
            ThumbnailStatus::Unsupported,
            ThumbnailStatus::Ok,
            ThumbnailStatus::HiddenByEdit,
            ThumbnailStatus::Ok,
        ],
        8,
    );
    assert!(valid_thumbnail_attachments(&fixture));
    for mutation in 0..6 {
        let mut bad = fixture.clone();
        match mutation {
            0 => {
                bad.images[0].item_index = 0;
            }
            1 => {
                bad.images[1].item_index = 1;
            }
            2 => {
                bad.images.pop();
            }
            3 => {
                bad.images.reverse();
            }
            4 => {
                bad.images[0].webp.clear();
            }
            _ => {
                bad.images.push(bad.images[0].clone());
            }
        }
        assert!(!valid_thumbnail_attachments(&bad));
        let result = tool_result("get_thumbnail", Ok(Payload::Thumbnails(bad)));
        assert_eq!(result["isError"], true);
        assert_eq!(result["content"].as_array().unwrap().len(), 1);
    }
    let mut omitted = fixture;
    omitted.page.items[1] = PageRow::too_long(&"oversized item").unwrap();
    assert!(!valid_thumbnail_attachments(&omitted));
    omitted.images.remove(0);
    assert!(valid_thumbnail_attachments(&omitted));
}

#[test]
fn stage1b_permission_tool_sets_and_thumbnail_changes() {
    for level in [
        AgentLevel::Disabled,
        AgentLevel::ReadOnly,
        AgentLevel::ReadAndOpen,
    ] {
        for thumbnails in [true, false] {
            let permission = PermissionSnapshot {
                level,
                thumbnails,
                gps: true,
            };
            let names = schema::tool_names(permission);
            assert_eq!(
                names.contains(&"list_folder"),
                level != AgentLevel::Disabled
            );
            assert_eq!(
                names.contains(&"get_thumbnail"),
                level != AgentLevel::Disabled && thumbnails
            );
            assert_eq!(schema::tools_for(permission).len(), names.len());
        }
    }
    let (stop, _recv) = mpsc::sync_channel(1);
    let mut observer = PermissionObserver {
        stop,
        revision: Arc::new(AtomicU64::new(0)),
        effective: None,
    };
    for (index, thumbnails) in [false, true, false].into_iter().enumerate() {
        let notice = observer.observe(
            schema::tool_names(PermissionSnapshot {
                level: AgentLevel::ReadOnly,
                thumbnails,
                gps: false,
            }),
            None,
        );
        assert_eq!(notice.is_some(), index > 0);
    }
}

#[test]
fn stage1b_all_protocols_transport_folder_pages_and_webp_blocks() {
    struct Stage1b;
    impl Backend for Stage1b {
        fn call(
            &self,
            command: Command,
            client: &str,
            control: Arc<CallControl>,
        ) -> Result<Payload, AgentError> {
            match command {
                Command::Status => Ok(Payload::Status(StatusPayload {
                    app_version: "fixture".into(),
                    permission: PermissionSnapshot {
                        level: AgentLevel::ReadOnly,
                        thumbnails: true,
                        gps: false,
                    },
                    fts_ready: true,
                    remote_active: false,
                })),
                Command::ListFolder { .. } => Ok(Payload::Folder(FolderPage {
                    items: vec![],
                    cursor: None,
                    truncated: false,
                    book_composition_applied: true,
                })),
                Command::GetThumbnail { .. } => Ok(Payload::Thumbnails(thumbnail_fixture(
                    &[ThumbnailStatus::HiddenByEdit, ThumbnailStatus::Ok],
                    MAX_THUMBNAIL_IMAGE_BYTES,
                ))),
                _ => Fixture { disabled: false }.call(command, client, control),
            }
        }
    }
    for version in VERSIONS {
        let modern_protocol = version == MODERN;
        let mut h = Harness::new(Arc::new(Stage1b));
        if !modern_protocol {
            h.send(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}));
            h.receive();
            h.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        }
        for (id, method, params) in [
            (1, "tools/list", json!({})),
            (
                2,
                "tools/call",
                json!({"name":"list_folder","arguments":{"path":"C:/book.pdf"}}),
            ),
            (
                3,
                "tools/call",
                json!({"name":"get_thumbnail","arguments":{"items":[{"path":"C:/a.png"},{"path":"C:/b.png"}]}}),
            ),
        ] {
            h.send(if modern_protocol {
                modern(id, method, params)
            } else {
                json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
            });
            let response = h.receive();
            assert_eq!(response["id"], id);
            let result = &response["result"];
            match id {
                1 => assert_eq!(result["tools"].as_array().unwrap().len(), 10),
                2 => assert_eq!(
                    result["structuredContent"]["book_composition_applied"],
                    true
                ),
                _ => {
                    assert_eq!(result["isError"], false);
                    assert_eq!(result["content"].as_array().unwrap().len(), 2);
                    assert_eq!(result["content"][1]["mimeType"], "image/webp");
                    assert_eq!(
                        result["structuredContent"]["items"][0]["status"],
                        "hidden_by_edit"
                    );
                    assert_eq!(
                        serde_json::from_str::<Value>(
                            result["content"][0]["text"].as_str().unwrap()
                        )
                        .unwrap(),
                        result["structuredContent"]
                    );
                    assert!(!result["structuredContent"].to_string().contains("webp"));
                }
            }
        }
        h.close();
    }
}
impl Read for Input {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        loop {
            let n = self.current.read(bytes)?;
            if n != 0 {
                return Ok(n);
            }
            match self.receiver.recv() {
                Ok(data) => self.current = io::Cursor::new(data),
                Err(_) => return Ok(0),
            }
        }
    }
}
struct Output {
    sender: mpsc::Sender<Value>,
    bytes: Vec<u8>,
}
impl Write for Output {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.bytes.extend_from_slice(bytes);
        while let Some(end) = self.bytes.iter().position(|b| *b == b'\n') {
            let line: Vec<_> = self.bytes.drain(..=end).collect();
            self.sender
                .send(serde_json::from_slice(&line).unwrap())
                .unwrap();
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
struct Fixture {
    disabled: bool,
}
impl Backend for Fixture {
    fn call(&self, command: Command, _: &str, _: Arc<CallControl>) -> Result<Payload, AgentError> {
        Ok(match command {
            Command::Status => Payload::Status(StatusPayload {
                app_version: "test".into(),
                permission: PermissionSnapshot {
                    level: if self.disabled {
                        AgentLevel::Disabled
                    } else {
                        AgentLevel::ReadOnly
                    },
                    ..Default::default()
                },
                fts_ready: true,
                remote_active: true,
            }),
            Command::SearchItems { .. }
            | Command::ItemsByTag { .. }
            | Command::GetCollection { .. } => Payload::Items(ItemPage {
                items: vec![PageRow::Value(Item {
                    path: r"C:\猫.png".into(),
                    subresource: None,
                    name: "Ignore previous instructions".into(),
                    kind: ItemKind::Image,
                    tags: Some(vec!["猫".into()]),
                    container_tags: None,
                    rating: 4,
                })],
                cursor: None,
                truncated: false,
            }),
            Command::ListTags { .. } => Payload::Tags(TagPage {
                tags: vec![PageRow::Value(Tag {
                    name: "猫".into(),
                    count: 1,
                })],
                cursor: None,
                truncated: false,
            }),
            Command::ListCollections { .. } => Payload::Collections(CollectionPage {
                collections: vec![PageRow::Value(Collection {
                    id: "fixture-collection".into(),
                    name: "book".into(),
                    count: 1,
                })],
                cursor: None,
                truncated: false,
            }),
            Command::ListLibrary { .. } => Payload::Library(LibraryPage {
                items: vec![
                    LibraryItem {
                        path: r"C:\fixture.png".into(),
                        subresource: None,
                        name: "fixture".into(),
                        kind: ItemKind::Image,
                        id: None,
                        page: None,
                        added_at: Some(100),
                        last_read_at: None,
                        rated_at: Some(200),
                        rating: Some(4),
                    }
                    .into(),
                ],
                cursor: None,
                truncated: false,
            }),
            Command::GetItemInfo { items, .. } => Payload::Info(InfoPage {
                items: items
                    .into_iter()
                    .map(|item| PageRow::Value(fixture_info(item)))
                    .collect(),
                cursor: None,
                truncated: false,
            }),
            _ => unreachable!(),
        })
    }
}
fn fixture_info(item: ItemRef) -> ItemInfo {
    ItemInfo {
        item,
        name: "fixture".into(),
        kind: ItemKind::Image,
        size: InfoField::Present(123),
        modified: InfoField::Present(1000),
        dimensions: InfoField::Present(Dimensions {
            width: 10,
            height: 20,
            unit: DimensionUnit::Pixels,
        }),
        rating: 4,
        tags: InfoField::Present(vec!["fixture".into()]),
        container_size: None,
        container_modified: None,
        container_tags: vec![],
        exif: InfoField::Present(ExifInfo {
            make: Some("camera".into()),
            ..Default::default()
        }),
        generation: InfoField::Present(GenerationInfo {
            prompt: Some(TextValue {
                text: "untrusted \"prompt\"".into(),
                truncated: true,
            }),
            ..Default::default()
        }),
    }
}
struct Harness {
    input: Option<mpsc::Sender<Vec<u8>>>,
    output: mpsc::Receiver<Value>,
    done: mpsc::Receiver<()>,
}
impl Harness {
    fn new(backend: Arc<dyn Backend>) -> Self {
        let (input, receiver) = mpsc::channel();
        let (sender, output) = mpsc::channel();
        let (done_send, done) = mpsc::channel();
        std::thread::spawn(move || {
            run_stdio(
                Input {
                    receiver,
                    current: io::Cursor::new(vec![]),
                },
                Output {
                    sender,
                    bytes: vec![],
                },
                backend,
            )
            .unwrap();
            done_send.send(()).unwrap();
        });
        Self {
            input: Some(input),
            output,
            done,
        }
    }
    fn send(&self, value: Value) {
        self.raw(format!("{value}\n").into_bytes());
    }
    fn raw(&self, bytes: Vec<u8>) {
        self.input.as_ref().unwrap().send(bytes).unwrap();
    }
    fn receive(&self) -> Value {
        self.output.recv_timeout(Duration::from_secs(5)).unwrap()
    }
    fn close(&mut self) {
        self.input.take();
        self.done
            .recv_timeout(Duration::from_millis(300))
            .expect("EOF must not wait for blocked workers");
    }
}
fn modern(id: i64, method: &str, mut params: Value) -> Value {
    params["_meta"] = json!({"io.modelcontextprotocol/protocolVersion":MODERN,"io.modelcontextprotocol/clientCapabilities":{}});
    json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
}
#[test]
fn modern_discovery_and_metadata_errors() {
    let mut h = Harness::new(Arc::new(Fixture { disabled: false }));
    h.send(modern(1, "server/discover", json!({})));
    let r = h.receive();
    assert_eq!(r["result"]["resultType"], "complete");
    assert_eq!(r["result"]["supportedVersions"], json!(VERSIONS));
    assert_eq!(r["result"]["ttlMs"], 0);
    h.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
    assert_eq!(h.receive()["error"]["code"], -32602);
    let mut r = modern(3, "tools/list", json!({}));
    r["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] = json!("2099-01-01");
    h.send(r);
    let e = h.receive();
    assert_eq!(e["error"]["code"], -32022);
    assert_eq!(e["error"]["data"]["supported"], json!(VERSIONS));
    h.raw(b"{oops\n".to_vec());
    assert_eq!(h.receive()["error"]["code"], -32700);
    h.send(json!([]));
    assert_eq!(h.receive()["error"]["code"], -32600);
    h.send(modern(4, "unknown", json!({})));
    assert_eq!(h.receive()["error"]["code"], -32601);
    h.close();
}
#[test]
fn legacy_versions_require_handshake_and_keep_legacy_results() {
    for version in &VERSIONS[1..] {
        let mut h = Harness::new(Arc::new(Fixture { disabled: false }));
        h.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}));
        assert_eq!(h.receive()["result"]["protocolVersion"], *version);
        h.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
        assert_eq!(h.receive()["error"]["code"], -32600);
        h.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        h.send(json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}));
        let r = h.receive();
        assert_eq!(r["result"]["tools"].as_array().unwrap().len(), 9);
        assert!(r["result"].get("resultType").is_none());
        h.close();
    }
}
#[test]
fn all_tools_return_matching_structured_and_text_and_validate_inputs() {
    let mut h = Harness::new(Arc::new(Fixture { disabled: false }));
    for (index, (name, args)) in [
        ("miv_status", json!({})),
        (
            "search_items",
            json!({"query":"猫","kind":"image","limit":50}),
        ),
        ("items_by_tag", json!({"tag":"猫"})),
        ("list_tags", json!({"choice":"all"})),
        ("list_collections", json!({})),
        ("get_collection", json!({"name":"fixture"})),
        ("list_library", json!({"kind":"rated","stars":3})),
        (
            "get_item_info",
            json!({"items":[{"path":"C:/fixture.png"}]}),
        ),
    ]
    .into_iter()
    .enumerate()
    {
        h.send(modern(
            index as i64,
            "tools/call",
            json!({"name":name,"arguments":args}),
        ));
        let r = h.receive();
        let result = &r["result"];
        assert_eq!(result["isError"], false);
        assert_eq!(result["resultType"], "complete");
        let text: Value =
            serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
        assert_eq!(text, result["structuredContent"]);
    }
    for args in [
        json!({"query":"q","kind":"unsupported"}),
        json!({"query":"q","unknown":true}),
        json!({"query":""}),
    ] {
        h.send(modern(
            20,
            "tools/call",
            json!({"name":"search_items","arguments":args}),
        ));
        assert_eq!(h.receive()["error"]["code"], -32602);
    }
    h.close();
    let mut h = Harness::new(Arc::new(Fixture { disabled: true }));
    h.send(modern(1, "tools/list", json!({})));
    assert_eq!(h.receive()["result"]["tools"], json!([]));
    h.close();
}
struct Blocked {
    started: mpsc::Sender<Arc<CallControl>>,
    release: Mutex<mpsc::Receiver<()>>,
}
impl Backend for Blocked {
    fn call(&self, _: Command, _: &str, control: Arc<CallControl>) -> Result<Payload, AgentError> {
        self.started.send(control).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        Err(AgentError::new(ErrorCode::Cancelled, "Cancelled."))
    }
}
#[test]
fn cancellation_and_eof_do_not_wait_for_worker() {
    for explicit in [false, true] {
        let (started_send, started) = mpsc::channel();
        let (release, release_recv) = mpsc::channel();
        let (mut h, reads) = tracked_runtime(Arc::new(Blocked {
            started: started_send,
            release: Mutex::new(release_recv),
        }));
        h.send(modern(
            1,
            "tools/call",
            json!({"name":"search_items","arguments":{"query":"q"}}),
        ));
        reads.recv_timeout(Duration::from_secs(5)).unwrap();
        let control = started.recv_timeout(Duration::from_secs(5)).unwrap();
        if explicit {
            h.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":1}}));
            reads.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(control.cancelled.load(Ordering::Acquire));
            h.send(modern(2, "server/discover", json!({})));
            reads.recv_timeout(Duration::from_secs(5)).unwrap();
            assert!(h.output.try_recv().is_err());
            release.send(()).unwrap();
            assert_eq!(h.receive()["id"], 2);
        }
        h.close();
        assert!(control.cancelled.load(Ordering::Acquire));
        if !explicit {
            release.send(()).unwrap();
        }
        assert!(h.output.try_recv().is_err());
    }
}
#[test]
fn actual_output_cap_includes_text_escaping_and_keeps_unicode() {
    let escaped = "\"\\\n\t猫".repeat(2000);
    let page = TagPage {
        tags: vec![PageRow::Value(Tag {
            name: escaped,
            count: 1,
        })],
        cursor: None,
        truncated: false,
    };
    assert!(mcp_content_chars(&page).unwrap() > MAX_PAGE_CHARS);
    let rejected = tool_result("list_tags", Ok(Payload::Tags(page.clone())));
    assert_eq!(rejected["isError"], true);
    // Simulate a broken adapter constructing an unbounded envelope directly;
    // the final serialized cap remains an independent last check.
    let page = json!(page);
    let result = json!({"content":[{"type":"text","text":page.to_string()}],"structuredContent":page,"isError":false});
    let response = success(json!(1), result, true);
    assert!(response.to_string().chars().count() > MAX_MCP_RESPONSE_CHARS);
    let mut bytes = vec![];
    write_response(&mut bytes, &response).unwrap();
    let text = String::from_utf8(bytes).unwrap();
    assert!(text.chars().count() <= MAX_MCP_RESPONSE_CHARS);
    let value: Value = serde_json::from_str(&text).unwrap();
    assert_eq!(value["id"], 1);
    assert_eq!(value["error"]["code"], -32603);
    let result = tool_result(
        "list_tags",
        Ok(Payload::Tags(TagPage {
            tags: vec![PageRow::Value(Tag {
                name: "猫".repeat(1000),
                count: 1,
            })],
            cursor: None,
            truncated: false,
        })),
    );
    let response = success(json!(2), result, true);
    let mut bytes = vec![];
    write_response(&mut bytes, &response).unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(value, response);
}
#[test]
fn tool_errors_remain_structured_and_match_error_schema_branch() {
    let result = tool_result(
        "search_items",
        Err(AgentError::new(
            ErrorCode::Disabled,
            "Enable AI integration in mIV.",
        )),
    );
    assert_eq!(result["isError"], true);
    assert_eq!(result["structuredContent"]["error"]["code"], "disabled");
    let text: Value = serde_json::from_str(result["content"][0]["text"].as_str().unwrap()).unwrap();
    assert_eq!(text, result["structuredContent"]);
    assert!(
        schema::tools()[1]["outputSchema"]["anyOf"][1]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("error"))
    );
}
struct Disabled;
impl Backend for Disabled {
    fn call(&self, _: Command, _: &str, _: Arc<CallControl>) -> Result<Payload, AgentError> {
        Err(AgentError::new(
            ErrorCode::Disabled,
            "AI 連携 (MCP) は無効です",
        ))
    }
}
#[test]
fn disabled_handshake_hides_tools_and_direct_call_returns_tool_error() {
    let mut h = Harness::new(Arc::new(Disabled));
    h.send(modern(1, "tools/list", json!({})));
    let r = h.receive();
    assert_eq!(r["result"]["tools"], json!([]));
    assert_eq!(r["result"]["ttlMs"], 0);
    h.send(modern(
        2,
        "tools/call",
        json!({"name":"search_items","arguments":{"query":"q"}}),
    ));
    let r = h.receive();
    assert_eq!(r["result"]["isError"], true);
    assert_eq!(
        r["result"]["structuredContent"]["error"]["code"],
        "disabled"
    );
    h.close();
}
#[test]
fn input_limits_match_core_and_advertised_schemas() {
    assert!(validate_page(&"猫".repeat(200), None, Some(&"c".repeat(8192))).is_ok());
    assert!(validate_page(&"猫".repeat(201), None, None).is_err());
    assert!(validate_page("tag", None, Some(&"c".repeat(8193))).is_err());
    for tool in schema::tools()
        .into_iter()
        .filter(|tool| matches!(tool["name"].as_str(), Some("search_items" | "items_by_tag")))
    {
        let key = if tool["name"] == "search_items" {
            "query"
        } else {
            "tag"
        };
        assert_eq!(tool["inputSchema"]["properties"][key]["maxLength"], 200);
        assert_eq!(
            tool["inputSchema"]["properties"]["cursor"]["maxLength"],
            8192
        );
    }
}

#[test]
fn stdin_owner_invokes_exit_callback_even_when_stdout_is_blocked() {
    struct BlockedOutput {
        entered: mpsc::Sender<()>,
        release: Option<mpsc::Receiver<()>>,
    }
    impl Write for BlockedOutput {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            if let Some(release) = self.release.take() {
                self.entered.send(()).unwrap();
                release.recv().unwrap();
            }
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    for queued_messages in [0] {
        let (input_send, input_recv) = mpsc::channel();
        let (entered_send, entered) = mpsc::channel();
        let (release_send, release) = mpsc::channel();
        let (eof_send, eof) = mpsc::channel();
        let (done_send, done) = mpsc::channel();
        let thread = std::thread::spawn(move || {
            run_stdio_with_eof(
                Input {
                    receiver: input_recv,
                    current: io::Cursor::new(vec![]),
                },
                BlockedOutput {
                    entered: entered_send,
                    release: Some(release),
                },
                Arc::new(Fixture { disabled: false }),
                move || {
                    eof_send.send(()).unwrap();
                },
            )
            .unwrap();
            done_send.send(()).unwrap();
        });
        input_send
            .send(format!("{}\n", modern(1, "server/discover", json!({}))).into_bytes())
            .unwrap();
        entered.recv_timeout(Duration::from_secs(5)).unwrap();
        for id in 2..2 + queued_messages {
            input_send
                .send(format!("{}\n", modern(id, "server/discover", json!({}))).into_bytes())
                .unwrap();
        }
        drop(input_send);
        eof.recv_timeout(Duration::from_millis(300))
            .expect("stdin EOF must reach its lifetime callback independently of stdout");
        release_send.send(()).unwrap();
        done.recv_timeout(Duration::from_secs(5)).unwrap();
        thread.join().unwrap();
    }
}

struct TrackedInput {
    inner: Input,
    reads: mpsc::Sender<()>,
}
impl Read for TrackedInput {
    fn read(&mut self, bytes: &mut [u8]) -> io::Result<usize> {
        let _ = self.reads.send(());
        self.inner.read(bytes)
    }
}
fn tracked_runtime(backend: Arc<dyn Backend>) -> (Harness, mpsc::Receiver<()>) {
    let (input, receiver) = mpsc::channel();
    let (sender, output) = mpsc::channel();
    let (done_send, done) = mpsc::channel();
    let (read_send, reads) = mpsc::channel();
    std::thread::spawn(move || {
        run_stdio(
            TrackedInput {
                inner: Input {
                    receiver,
                    current: io::Cursor::new(vec![]),
                },
                reads: read_send,
            },
            Output {
                sender,
                bytes: vec![],
            },
            backend,
        )
        .unwrap();
        done_send.send(()).unwrap();
    });
    reads.recv_timeout(Duration::from_secs(5)).unwrap();
    (
        Harness {
            input: Some(input),
            output,
            done,
        },
        reads,
    )
}
struct GateOutput {
    inner: Output,
    entered: mpsc::Sender<()>,
    release: Option<mpsc::Receiver<()>>,
}
impl Write for GateOutput {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        if let Some(release) = self.release.take() {
            self.entered.send(()).unwrap();
            release.recv().unwrap();
        }
        self.inner.write(bytes)
    }
    fn flush(&mut self) -> io::Result<()> {
        self.inner.flush()
    }
}
fn gated_runtime(
    backend: Arc<dyn Backend>,
) -> (
    Harness,
    mpsc::Receiver<()>,
    mpsc::Sender<()>,
    mpsc::Receiver<()>,
    mpsc::Receiver<()>,
) {
    let (input, receiver) = mpsc::channel();
    let (sender, output) = mpsc::channel();
    let (done_send, done) = mpsc::channel();
    let (entered_send, entered) = mpsc::channel();
    let (release_send, release) = mpsc::channel();
    let (eof_send, eof) = mpsc::channel();
    let (read_send, reads) = mpsc::channel();
    std::thread::spawn(move || {
        run_stdio_with_eof(
            TrackedInput {
                inner: Input {
                    receiver,
                    current: io::Cursor::new(vec![]),
                },
                reads: read_send,
            },
            GateOutput {
                inner: Output {
                    sender,
                    bytes: vec![],
                },
                entered: entered_send,
                release: Some(release),
            },
            backend,
            move || {
                let _ = eof_send.send(());
            },
        )
        .unwrap();
        done_send.send(()).unwrap();
    });
    reads.recv_timeout(Duration::from_secs(5)).unwrap();
    (
        Harness {
            input: Some(input),
            output,
            done,
        },
        entered,
        release_send,
        eof,
        reads,
    )
}
pub(crate) fn run_pipe_cancel_with_blocked_stdout(
    backend: Arc<dyn Backend>,
    started: mpsc::Receiver<()>,
    cancel_received: mpsc::Receiver<()>,
) {
    let (mut h, entered, release, _eof, reads) = gated_runtime(backend);
    h.send(modern(1, "server/discover", json!({})));
    reads.recv_timeout(Duration::from_secs(5)).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    h.send(modern(
        2,
        "tools/call",
        json!({"name":"miv_status","arguments":{}}),
    ));
    started.recv_timeout(Duration::from_secs(5)).unwrap();
    h.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}}));
    cancel_received
        .recv_timeout(Duration::from_secs(5))
        .expect("core must receive Cancel while stdout is still blocked");
    h.send(modern(3, "server/discover", json!({})));
    release.send(()).unwrap();
    assert_eq!(h.receive()["id"], 1);
    assert_eq!(h.receive()["id"], 3);
    h.close();
    assert!(h.output.try_recv().is_err());
}

#[test]
fn blocked_stdout_does_not_block_eager_cancellation() {
    let (started_send, started) = mpsc::channel();
    let (release_work, release_recv) = mpsc::channel();
    let (mut h, entered, release_output, _eof, reads) = gated_runtime(Arc::new(Blocked {
        started: started_send,
        release: Mutex::new(release_recv),
    }));
    h.send(modern(1, "server/discover", json!({})));
    reads.recv_timeout(Duration::from_secs(5)).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    h.send(modern(
        2,
        "tools/call",
        json!({"name":"miv_status","arguments":{}}),
    ));
    reads.recv_timeout(Duration::from_secs(5)).unwrap();
    let control = started.recv_timeout(Duration::from_secs(5)).unwrap();
    h.send(json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":2}}));
    reads.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(control.cancelled.load(Ordering::Acquire));
    release_work.send(()).unwrap();
    h.send(modern(3, "server/discover", json!({})));
    release_output.send(()).unwrap();
    assert_eq!(h.receive()["id"], 1);
    assert_eq!(h.receive()["id"], 3);
    h.close();
    assert!(h.output.try_recv().is_err());
}

#[test]
fn queued_response_is_suppressed_when_cancelled_before_writer_commit() {
    let (owner, control) = (Arc::new(Mutex::new(None)), Arc::new(CallControl::default()));
    *owner.lock().unwrap() = Some(Active {
        serial: 2,
        id: json!(2),
        modern: true,
        phase: ActivePhase::ReplyQueued,
        control: control.clone(),
    });
    let (tx, rx) = mpsc::sync_channel(32);
    let (output_send, output) = mpsc::channel();
    let (entered_send, entered) = mpsc::channel();
    let (release_send, release) = mpsc::channel();
    let (writer_events, _writer_event_recv) = mpsc::sync_channel(32);
    let writer_owner = owner.clone();
    let writer = std::thread::spawn(move || {
        let mut output = GateOutput {
            inner: Output {
                sender: output_send,
                bytes: vec![],
            },
            entered: entered_send,
            release: Some(release),
        };
        writer_loop(&mut output, &rx, &writer_owner, &writer_events).unwrap();
    });
    tx.send(Envelope::plain(success(json!(1), json!({}), true)))
        .unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    tx.send(Envelope {
        value: success(json!(2), json!({}), true),
        request: Some((2, control)),
    })
    .unwrap();
    cancel_matching(&owner, Some(&json!(2)));
    release_send.send(()).unwrap();
    drop(tx);
    writer.join().unwrap();
    assert_eq!(
        output.recv_timeout(Duration::from_secs(5)).unwrap()["id"],
        1
    );
    assert!(output.try_recv().is_err());
    assert!(owner.lock().unwrap().is_none());
}

fn input_bytes(line: InputLine) -> Vec<u8> {
    match line {
        InputLine::Request { bytes, .. } | InputLine::Other(bytes) => bytes,
        InputLine::Rejected(value) => panic!("unexpected rejected input: {value}"),
    }
}
fn cancel_line(id: i64) -> Vec<u8> {
    json!({"jsonrpc":"2.0","method":"notifications/cancelled","params":{"requestId":id}})
        .to_string()
        .into_bytes()
}
#[test]
fn queued_cancel_removes_request_before_admission_and_preserves_initialize() {
    let owner = Arc::new(Mutex::new(None));
    let queue = InputQueue::new();
    assert!(queue.push(
        modern(1, "tools/list", json!({})).to_string().into_bytes(),
        &owner
    ));
    assert!(queue.cancel_notification(&owner, &cancel_line(1)));
    assert!(
        queue
            .dispatch_next(&owner, |_| panic!(
                "cancelled queued request must not dispatch"
            ))
            .is_none()
    );
    assert!(
        queue.push(
            json!({"jsonrpc":"2.0","id":2,"method":"initialize"})
                .to_string()
                .into_bytes(),
            &owner
        )
    );
    assert!(queue.cancel_notification(&owner, &cancel_line(2)));
    let bytes = queue.dispatch_next(&owner, input_bytes).unwrap();
    assert_eq!(
        serde_json::from_slice::<Value>(&bytes).unwrap()["method"],
        "initialize"
    );
}
#[test]
fn cancellation_covers_queue_to_active_handoff_under_one_lock() {
    let owner = Arc::new(Mutex::new(None));
    let queue = Arc::new(InputQueue::new());
    let control = Arc::new(CallControl::default());
    assert!(queue.push(
        modern(1, "tools/list", json!({})).to_string().into_bytes(),
        &owner
    ));
    let (entered_send, entered) = mpsc::channel();
    let (release_send, release) = mpsc::channel();
    let handoff_queue = queue.clone();
    let handoff_owner = owner.clone();
    let handoff_control = control.clone();
    let handoff = std::thread::spawn(move || {
        handoff_queue
            .dispatch_next(&handoff_owner, |line| {
                assert_eq!(
                    serde_json::from_slice::<Value>(&input_bytes(line)).unwrap()["id"],
                    1
                );
                entered_send.send(()).unwrap();
                release.recv().unwrap();
                *handoff_owner.lock().unwrap() = Some(Active {
                    serial: 1,
                    id: json!(1),
                    modern: true,
                    phase: ActivePhase::Executing(Action::List),
                    control: handoff_control,
                });
            })
            .unwrap();
    });
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    let (attempt_send, attempted) = mpsc::channel();
    let cancel_queue = queue.clone();
    let cancel_owner = owner.clone();
    let cancel = std::thread::spawn(move || {
        attempt_send.send(()).unwrap();
        assert!(cancel_queue.cancel_notification(&cancel_owner, &cancel_line(1)));
    });
    attempted.recv_timeout(Duration::from_secs(5)).unwrap();
    assert!(owner.lock().unwrap().is_none());
    release_send.send(()).unwrap();
    handoff.join().unwrap();
    cancel.join().unwrap();
    assert!(control.cancelled.load(Ordering::Acquire));
}
#[test]
fn bounded_input_queue_waits_at_32_then_resumes_or_closes_without_sleep() {
    for close in [false, true] {
        let owner = Arc::new(Mutex::new(None));
        let queue = Arc::new(InputQueue::new());
        for id in 0..input::CAPACITY {
            assert!(
                queue.push(
                    modern(id as i64, "server/discover", json!({}))
                        .to_string()
                        .into_bytes(),
                    &owner
                )
            );
        }
        let (start_send, started) = mpsc::channel();
        let (done_send, done) = mpsc::channel();
        let producer_queue = queue.clone();
        let producer_owner = owner.clone();
        let producer = std::thread::spawn(move || {
            start_send.send(()).unwrap();
            done_send
                .send(
                    producer_queue.push(
                        modern(32, "server/discover", json!({}))
                            .to_string()
                            .into_bytes(),
                        &producer_owner,
                    ),
                )
                .unwrap();
        });
        started.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(done.try_recv().is_err());
        if close {
            queue.close();
            assert!(!done.recv_timeout(Duration::from_secs(5)).unwrap());
            assert!(
                queue
                    .dispatch_next(&owner, |_| panic!(
                        "closed queue must not dispatch pending input"
                    ))
                    .is_none()
            );
            assert!(
                !queue.push(
                    modern(33, "server/discover", json!({}))
                        .to_string()
                        .into_bytes(),
                    &owner
                )
            );
        } else {
            let first = queue.dispatch_next(&owner, input_bytes).unwrap();
            assert_eq!(serde_json::from_slice::<Value>(&first).unwrap()["id"], 0);
            assert!(done.recv_timeout(Duration::from_secs(5)).unwrap());
            for id in 1..=32 {
                let bytes = queue.dispatch_next(&owner, input_bytes).unwrap();
                assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap()["id"], id);
            }
            assert!(queue.dispatch_next(&owner, input_bytes).is_none());
        }
        producer.join().unwrap();
    }
}
#[test]
fn duplicate_queued_or_active_ids_are_explicitly_rejected() {
    for active in [false, true] {
        let owner = Arc::new(Mutex::new(None));
        let queue = InputQueue::new();
        let line = modern(1, "tools/list", json!({})).to_string().into_bytes();
        if active {
            *owner.lock().unwrap() = Some(Active {
                serial: 1,
                id: json!(1),
                modern: true,
                phase: ActivePhase::Executing(Action::List),
                control: Arc::new(CallControl::default()),
            });
        } else {
            assert!(queue.push(line.clone(), &owner));
        }
        assert!(queue.push(line, &owner));
        if active {
            owner.lock().unwrap().take();
        } else {
            queue.dispatch_next(&owner, input_bytes).unwrap();
        }
        let response = queue
            .dispatch_next(&owner, |line| match line {
                InputLine::Rejected(value) => value,
                _ => panic!("duplicate must remain a rejection"),
            })
            .unwrap();
        assert_eq!(response["id"], 1);
        assert_eq!(response["error"]["code"], -32600);
    }
}

#[derive(Clone, Copy)]
enum HoldRequest {
    Status,
    Search,
}
struct SerialQueries {
    hold: HoldRequest,
    release: Mutex<mpsc::Receiver<()>>,
    calls: mpsc::Sender<Command>,
    running: std::sync::atomic::AtomicUsize,
    maximum: std::sync::atomic::AtomicUsize,
}
impl Backend for SerialQueries {
    fn call(
        &self,
        command: Command,
        client: &str,
        control: Arc<CallControl>,
    ) -> Result<Payload, AgentError> {
        // Legacy's independent permission observer also calls Status. In that
        // fixture only the two client queries are counted; initial list Status
        // completes before either query is admitted. Modern counts every call.
        if matches!(self.hold, HoldRequest::Search) && matches!(command, Command::Status) {
            return Fixture { disabled: false }.call(command, client, control);
        }
        let running = self.running.fetch_add(1, Ordering::AcqRel) + 1;
        self.maximum.fetch_max(running, Ordering::AcqRel);
        self.calls.send(command.clone()).unwrap();
        if matches!(
            (self.hold, &command),
            (HoldRequest::Status, Command::Status)
                | (HoldRequest::Search, Command::SearchItems { .. })
        ) {
            self.release.lock().unwrap().recv().unwrap();
        }
        let result = Fixture { disabled: false }.call(command, client, control);
        self.running.fetch_sub(1, Ordering::AcqRel);
        result
    }
}
fn version_request(version: &str, id: i64, method: &str, params: Value) -> Value {
    if version == MODERN {
        modern(id, method, params)
    } else {
        json!({"jsonrpc":"2.0","id":id,"method":method,"params":params})
    }
}
fn pipeline_burst(version: &str) -> Vec<u8> {
    let mut messages = vec![];
    if version != MODERN {
        messages.push(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"pipeline","version":"1"}}}));
        messages.push(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
    }
    messages.extend([
        version_request(version, 10, "tools/list", json!({})),
        version_request(
            version,
            11,
            "tools/call",
            json!({"name":"search_items","arguments":{"query":"q"}}),
        ),
        version_request(
            version,
            12,
            "tools/call",
            json!({"name":"list_collections","arguments":{}}),
        ),
    ]);
    messages
        .into_iter()
        .map(|message| format!("{message}\n"))
        .collect::<String>()
        .into_bytes()
}
fn serial_fixture(
    hold: HoldRequest,
) -> (
    Arc<SerialQueries>,
    mpsc::Receiver<Command>,
    mpsc::Sender<()>,
) {
    let (calls_send, calls) = mpsc::channel();
    let (release, recv) = mpsc::channel();
    (
        Arc::new(SerialQueries {
            hold,
            release: Mutex::new(recv),
            calls: calls_send,
            running: std::sync::atomic::AtomicUsize::new(0),
            maximum: std::sync::atomic::AtomicUsize::new(0),
        }),
        calls,
        release,
    )
}
#[test]
fn pipelined_list_and_calls_complete_successfully_in_fifo_for_all_versions() {
    for version in VERSIONS {
        let (backend, calls, release) = serial_fixture(HoldRequest::Search);
        let (mut h, reads) = tracked_runtime(backend.clone());
        h.raw(pipeline_burst(version));
        reads.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            calls.recv_timeout(Duration::from_secs(5)).unwrap(),
            Command::SearchItems { .. }
        ));
        assert_eq!(backend.running.load(Ordering::Acquire), 1);
        assert!(calls.try_recv().is_err());
        release.send(()).unwrap();
        if version != MODERN {
            assert_eq!(h.receive()["id"], 0);
        }
        for id in 10..=12 {
            let response = h.receive();
            assert_eq!(response["id"], id);
            assert!(response.get("error").is_none());
            assert_ne!(response["result"]["isError"], true);
        }
        assert!(matches!(
            calls.recv_timeout(Duration::from_secs(5)).unwrap(),
            Command::ListCollections { .. }
        ));
        assert_eq!(backend.maximum.load(Ordering::Acquire), 1);
        h.close();
    }
}
#[test]
fn ping_bypasses_held_tool_without_reordering_core_requests() {
    for version in VERSIONS {
        let (backend, calls, release) = serial_fixture(HoldRequest::Search);
        let (mut h, reads) = tracked_runtime(backend.clone());
        h.raw(pipeline_burst(version));
        reads.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            calls.recv_timeout(Duration::from_secs(5)).unwrap(),
            Command::SearchItems { .. }
        ));
        if version != MODERN {
            assert_eq!(h.receive()["id"], 0);
        }
        assert_eq!(h.receive()["id"], 10);

        // Collection id=12 is already waiting behind held search id=11.
        h.send(version_request(version, 20, "ping", json!({})));
        let ping = h.receive();
        assert_eq!(ping["id"], 20);
        if version == MODERN {
            // The stateless version has no ping; preserve method validation.
            assert_eq!(ping["error"]["code"], -32601);
        } else {
            assert_eq!(ping["result"], json!({}));
        }
        assert_eq!(backend.running.load(Ordering::Acquire), 1);
        assert!(
            calls.try_recv().is_err(),
            "queued core request must still wait"
        );
        release.send(()).unwrap();
        assert_eq!(h.receive()["id"], 11);
        assert_eq!(h.receive()["id"], 12);
        assert!(matches!(
            calls.recv_timeout(Duration::from_secs(5)).unwrap(),
            Command::ListCollections { .. }
        ));
        assert_eq!(backend.maximum.load(Ordering::Acquire), 1);
        h.close();
    }
}

#[test]
fn modern_pipeline_holds_first_list_until_release_and_dispatches_one_backend_call() {
    let (backend, calls, release) = serial_fixture(HoldRequest::Status);
    let (mut h, reads) = tracked_runtime(backend.clone());
    h.raw(pipeline_burst(MODERN));
    reads.recv_timeout(Duration::from_secs(5)).unwrap();
    assert_eq!(
        calls.recv_timeout(Duration::from_secs(5)).unwrap(),
        Command::Status
    );
    assert!(calls.try_recv().is_err());
    assert!(h.output.try_recv().is_err());
    release.send(()).unwrap();
    for id in 10..=12 {
        let response = h.receive();
        assert_eq!(response["id"], id);
        assert!(response.get("error").is_none());
        assert_ne!(response["result"]["isError"], true);
    }
    assert!(matches!(
        calls.recv_timeout(Duration::from_secs(5)).unwrap(),
        Command::SearchItems { .. }
    ));
    assert!(matches!(
        calls.recv_timeout(Duration::from_secs(5)).unwrap(),
        Command::ListCollections { .. }
    ));
    assert_eq!(backend.maximum.load(Ordering::Acquire), 1);
    h.close();
}
#[test]
fn queued_cancel_has_no_backend_call_or_response_for_all_versions() {
    for version in VERSIONS {
        let (backend, calls, release) = serial_fixture(HoldRequest::Search);
        let (mut h, reads) = tracked_runtime(backend);
        h.raw(pipeline_burst(version));
        reads.recv_timeout(Duration::from_secs(5)).unwrap();
        assert!(matches!(
            calls.recv_timeout(Duration::from_secs(5)).unwrap(),
            Command::SearchItems { .. }
        ));
        h.raw(format!("{}\n", String::from_utf8(cancel_line(12)).unwrap()).into_bytes());
        reads.recv_timeout(Duration::from_secs(5)).unwrap();
        release.send(()).unwrap();
        if version != MODERN {
            assert_eq!(h.receive()["id"], 0);
        }
        assert_eq!(h.receive()["id"], 10);
        assert_eq!(h.receive()["id"], 11);
        h.send(version_request(
            version,
            13,
            if version == MODERN {
                "server/discover"
            } else {
                "ping"
            },
            json!({}),
        ));
        assert_eq!(h.receive()["id"], 13);
        assert!(calls.try_recv().is_err());
        h.close();
        assert!(h.output.try_recv().is_err());
    }
}
struct CoreBusy;
impl Backend for CoreBusy {
    fn call(&self, _: Command, _: &str, _: Arc<CallControl>) -> Result<Payload, AgentError> {
        Err(AgentError::new(ErrorCode::Busy, "core busy fixture"))
    }
}
#[test]
fn actual_core_busy_remains_a_typed_tool_error_for_all_versions() {
    for version in VERSIONS {
        let mut h = Harness::new(Arc::new(CoreBusy));
        if version != MODERN {
            h.send(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}));
            h.receive();
            h.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        }
        h.send(version_request(
            version,
            1,
            "tools/call",
            json!({"name":"search_items","arguments":{"query":"q"}}),
        ));
        let response = h.receive();
        assert!(response.get("error").is_none());
        assert_eq!(response["result"]["isError"], true);
        assert_eq!(
            response["result"]["structuredContent"]["error"]["code"],
            "busy"
        );
        assert_eq!(
            response["result"]["structuredContent"]["error"]["message"],
            "core busy fixture"
        );
        h.close();
    }
}

#[test]
fn full_input_and_output_queues_apply_backpressure_without_exiting_or_losing_ids() {
    let (mut h, entered, release, eof, reads) =
        gated_runtime(Arc::new(Fixture { disabled: false }));
    h.send(modern(1, "server/discover", json!({})));
    reads.recv_timeout(Duration::from_secs(5)).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    for id in 2..=100 {
        h.send(modern(id, "server/discover", json!({})));
    }
    // With the writer stopped, these reads fill both bounded queues and leave
    // stdin open. No queue-full condition is an EOF/process-exit instruction.
    for _ in 0..65 {
        reads.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    assert!(eof.try_recv().is_err());
    release.send(()).unwrap();
    for id in 1..=100 {
        assert_eq!(h.receive()["id"], id);
    }
    assert!(eof.try_recv().is_err());
    h.send(modern(101, "server/discover", json!({})));
    assert_eq!(h.receive()["id"], 101);
    h.close();
}

#[test]
fn oversized_input_and_read_errors_are_not_true_eof() {
    struct FailedInput;
    impl Read for FailedInput {
        fn read(&mut self, _: &mut [u8]) -> io::Result<usize> {
            Err(io::Error::other("fixture"))
        }
    }
    for input in [
        Box::new(FailedInput) as Box<dyn Read + Send>,
        Box::new(io::Cursor::new(vec![b'x'; MAX_LINE_BYTES as usize + 1])),
    ] {
        let (tx, rx) = mpsc::channel();
        run_stdio_with_eof(
            input,
            io::sink(),
            Arc::new(Fixture { disabled: false }),
            move || {
                let _ = tx.send(());
            },
        )
        .unwrap();
        assert!(rx.try_recv().is_err());
    }
}

#[test]
fn legacy_pending_ping_and_capability_contracts() {
    for version in &VERSIONS[1..] {
        let mut h = Harness::new(Arc::new(Fixture { disabled: false }));
        h.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}));
        let r = h.receive();
        assert_eq!(r["result"]["capabilities"]["tools"]["listChanged"], true);
        h.send(json!({"jsonrpc":"2.0","id":2,"method":"ping"}));
        assert_eq!(h.receive()["result"], json!({}));
        h.close();
    }
    let mut h = Harness::new(Arc::new(Fixture { disabled: false }));
    h.send(modern(1, "server/discover", json!({})));
    let r = h.receive();
    assert!(
        r["result"]["capabilities"]["tools"]
            .get("listChanged")
            .is_none()
    );
    h.send(modern(2, "tools/list", json!({})));
    let r = h.receive();
    assert_eq!(r["result"]["ttlMs"], 0);
    assert_eq!(r["result"]["cacheScope"], "private");
    h.close();
}

#[test]
fn legacy_pending_ping_with_version_metadata_uses_normal_version_validation() {
    for version in &VERSIONS[1..] {
        let mut h = Harness::new(Arc::new(Fixture { disabled: false }));
        h.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}));
        h.receive();
        let mut unsupported = modern(2, "ping", json!({}));
        unsupported["params"]["_meta"]["io.modelcontextprotocol/protocolVersion"] =
            json!("2099-01-01");
        h.send(unsupported);
        assert_eq!(h.receive()["error"]["code"], -32022);
        h.send(modern(3, "ping", json!({})));
        assert_eq!(h.receive()["error"]["code"], -32601);
        h.send(json!({"jsonrpc":"2.0","id":4,"method":"ping"}));
        assert_eq!(h.receive()["result"], json!({}));
        h.close();
    }
}

struct LiveStatus {
    enabled: std::sync::atomic::AtomicBool,
    calls: mpsc::Sender<bool>,
}
impl Backend for LiveStatus {
    fn call(&self, command: Command, _: &str, _: Arc<CallControl>) -> Result<Payload, AgentError> {
        assert_eq!(command, Command::Status);
        let enabled = self.enabled.load(Ordering::Acquire);
        let _ = self.calls.send(enabled);
        if enabled {
            Ok(Payload::Status(StatusPayload {
                app_version: "fixture".into(),
                permission: PermissionSnapshot {
                    level: AgentLevel::ReadOnly,
                    ..Default::default()
                },
                fts_ready: true,
                remote_active: false,
            }))
        } else {
            Err(AgentError::new(ErrorCode::Disabled, "disabled"))
        }
    }
}
#[test]
fn idle_legacy_permission_observer_notifies_changes_and_refreshes_lists() {
    for version in &VERSIONS[1..] {
        let (call_send, calls) = mpsc::channel();
        let backend = Arc::new(LiveStatus {
            enabled: std::sync::atomic::AtomicBool::new(false),
            calls: call_send,
        });
        let mut h = Harness::new(backend.clone());
        h.send(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}));
        h.receive();
        h.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        h.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
        assert_eq!(h.receive()["result"]["tools"], json!([]));
        while calls.try_recv().is_ok() {}
        backend.enabled.store(true, Ordering::Release);
        while !calls.recv_timeout(Duration::from_secs(10)).unwrap() {}
        assert_eq!(h.receive()["method"], "notifications/tools/list_changed");
        h.send(json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}));
        assert_eq!(h.receive()["result"]["tools"].as_array().unwrap().len(), 9);
        while calls.try_recv().is_ok() {}
        assert!(calls.recv_timeout(Duration::from_secs(10)).unwrap());
        h.send(json!({"jsonrpc":"2.0","id":4,"method":"ping"}));
        assert_eq!(h.receive()["id"], 4);
        backend.enabled.store(false, Ordering::Release);
        while calls.recv_timeout(Duration::from_secs(10)).unwrap() {}
        assert_eq!(h.receive()["method"], "notifications/tools/list_changed");
        h.send(json!({"jsonrpc":"2.0","id":5,"method":"tools/list"}));
        assert_eq!(h.receive()["result"]["tools"], json!([]));
        h.send(json!({"jsonrpc":"2.0","id":6,"method":"ping"}));
        assert_eq!(h.receive()["id"], 6);
        h.close();
    }
}
#[test]
fn revoked_read_does_not_advertise_disabled_tools_while_status_stays_enabled() {
    struct RevokedRead;
    impl Backend for RevokedRead {
        fn call(
            &self,
            command: Command,
            client: &str,
            control: Arc<CallControl>,
        ) -> Result<Payload, AgentError> {
            if matches!(command, Command::Status) {
                Fixture { disabled: false }.call(command, client, control)
            } else {
                Err(AgentError::new(
                    ErrorCode::PermissionChanged,
                    "AI 連携の許可が変更されました。もう一度お試しください",
                ))
            }
        }
    }
    for version in &VERSIONS[1..] {
        let mut h = Harness::new(Arc::new(RevokedRead));
        h.send(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}));
        h.receive();
        h.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        h.send(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
        assert_eq!(h.receive()["result"]["tools"].as_array().unwrap().len(), 9);
        h.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"search_items","arguments":{"query":"fixture"}}}));
        let denied = h.receive();
        assert_eq!(
            denied["id"], 2,
            "revocation must not emit a spurious list_changed"
        );
        assert_eq!(denied["result"]["isError"], true);
        h.send(json!({"jsonrpc":"2.0","id":3,"method":"ping"}));
        assert_eq!(h.receive()["id"], 3);
        h.close();
        assert!(h.output.try_recv().is_err());
    }
}

#[test]
fn permission_observer_discards_old_poll_after_newer_actual_observations() {
    let (stop, _recv) = mpsc::sync_channel(1);
    let mut observer = PermissionObserver {
        stop,
        revision: Arc::new(AtomicU64::new(0)),
        effective: None,
    };
    assert!(observer.observe(vec![], None).is_none());
    let old = observer.revision.load(Ordering::Acquire);
    assert!(
        observer
            .observe(
                schema::tool_names(PermissionSnapshot {
                    level: AgentLevel::ReadOnly,
                    ..Default::default()
                }),
                None
            )
            .is_some()
    );
    assert!(observer.observe(vec![], None).is_some());
    assert!(
        observer
            .observe(
                schema::tool_names(PermissionSnapshot {
                    level: AgentLevel::ReadOnly,
                    ..Default::default()
                }),
                Some(old)
            )
            .is_none()
    );
    assert_eq!(observer.effective, Some(vec![]));
    let current = observer.revision.load(Ordering::Acquire);
    assert!(observer.observe(vec![], Some(current)).is_none());
    for code in [
        ErrorCode::Busy,
        ErrorCode::NotReady,
        ErrorCode::Internal,
        ErrorCode::PermissionChanged,
    ] {
        assert_eq!(
            permission_tools(&Err(AgentError::new(code, "fixture"))),
            None
        );
    }
}

#[test]
fn stale_status_poll_and_tool_list_never_advertise_disabled_tools() {
    struct StaleStatus {
        calls: mpsc::Sender<usize>,
        count: AtomicU64,
    }
    impl Backend for StaleStatus {
        fn call(
            &self,
            command: Command,
            client: &str,
            control: Arc<CallControl>,
        ) -> Result<Payload, AgentError> {
            assert_eq!(command, Command::Status);
            let call = self.count.fetch_add(1, Ordering::AcqRel) as usize;
            self.calls.send(call).unwrap();
            if matches!(call, 1 | 2) {
                // Core has rejected the result because GPS changed,
                // while the level and therefore the advertised tools stayed put.
                Err(AgentError::new(
                    ErrorCode::PermissionChanged,
                    "AI 連携の許可が変更されました。もう一度お試しください",
                ))
            } else {
                Fixture { disabled: false }.call(command, client, control)
            }
        }
    }
    for version in &VERSIONS[1..] {
        let (send, calls) = mpsc::channel();
        let mut h = Harness::new(Arc::new(StaleStatus {
            calls: send,
            count: AtomicU64::new(0),
        }));
        h.send(json!({"jsonrpc":"2.0","id":0,"method":"initialize","params":{"protocolVersion":version,"capabilities":{},"clientInfo":{"name":"fixture","version":"1"}}}));
        h.receive();
        h.send(json!({"jsonrpc":"2.0","method":"notifications/initialized"}));
        h.send(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}));
        assert_eq!(calls.recv_timeout(Duration::from_secs(10)).unwrap(), 0);
        assert_eq!(h.receive()["result"]["tools"].as_array().unwrap().len(), 9);
        // The real five-second observer poll receives a stale snapshot result.
        assert_eq!(calls.recv_timeout(Duration::from_secs(10)).unwrap(), 1);
        h.send(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}));
        assert_eq!(calls.recv_timeout(Duration::from_secs(10)).unwrap(), 2);
        let stale = h.receive();
        assert_eq!(
            stale["id"], 2,
            "a stale poll/list must not emit list_changed"
        );
        assert!(
            stale.get("error").is_some(),
            "staleness is an error, never an empty successful list"
        );
        assert!(stale.pointer("/result/tools").is_none());
        h.send(json!({"jsonrpc":"2.0","id":3,"method":"tools/list"}));
        assert_eq!(calls.recv_timeout(Duration::from_secs(10)).unwrap(), 3);
        let fresh = h.receive();
        assert_eq!(fresh["id"], 3, "recovery must not emit list_changed either");
        assert_eq!(fresh["result"]["tools"].as_array().unwrap().len(), 9);
        h.close();
        assert!(h.output.try_recv().is_err());
    }
}

#[test]
fn stdout_failure_releases_full_queue_and_cancels_active_work_without_eof() {
    struct FailedOutput {
        entered: mpsc::Sender<()>,
        release: Option<mpsc::Receiver<()>>,
    }
    impl Write for FailedOutput {
        fn write(&mut self, _: &[u8]) -> io::Result<usize> {
            if let Some(release) = self.release.take() {
                self.entered.send(()).unwrap();
                release.recv().unwrap();
            }
            Err(io::Error::new(
                io::ErrorKind::BrokenPipe,
                "fixture stdout closed",
            ))
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    let (input_send, input_recv) = mpsc::channel();
    let (read_send, reads) = mpsc::channel();
    let (entered_send, entered) = mpsc::channel();
    let (release_output, release_recv) = mpsc::channel();
    let (started_send, started) = mpsc::channel();
    let (release_work, release_work_recv) = mpsc::channel();
    let (done_send, done) = mpsc::channel();
    let runtime = std::thread::spawn(move || {
        let result = run_stdio(
            TrackedInput {
                inner: Input {
                    receiver: input_recv,
                    current: io::Cursor::new(vec![]),
                },
                reads: read_send,
            },
            FailedOutput {
                entered: entered_send,
                release: Some(release_recv),
            },
            Arc::new(Blocked {
                started: started_send,
                release: Mutex::new(release_work_recv),
            }),
        );
        done_send.send(result).unwrap();
    });
    reads.recv_timeout(Duration::from_secs(5)).unwrap();
    input_send
        .send(format!("{}\n", modern(1, "server/discover", json!({}))).into_bytes())
        .unwrap();
    reads.recv_timeout(Duration::from_secs(5)).unwrap();
    entered.recv_timeout(Duration::from_secs(5)).unwrap();
    input_send
        .send(
            format!(
                "{}\n",
                modern(2, "tools/call", json!({"name":"miv_status","arguments":{}}))
            )
            .into_bytes(),
        )
        .unwrap();
    reads.recv_timeout(Duration::from_secs(5)).unwrap();
    let control = started.recv_timeout(Duration::from_secs(5)).unwrap();
    for id in 3..=102 {
        input_send
            .send(format!("{}\n", modern(id, "server/discover", json!({}))).into_bytes())
            .unwrap();
    }
    for _ in 0..32 {
        reads.recv_timeout(Duration::from_secs(5)).unwrap();
    }
    release_output.send(()).unwrap();
    assert_eq!(
        done.recv_timeout(Duration::from_secs(5))
            .unwrap()
            .unwrap_err()
            .kind(),
        io::ErrorKind::BrokenPipe
    );
    assert!(control.cancelled.load(Ordering::Acquire));
    release_work.send(()).unwrap();
    runtime.join().unwrap();
    drop(input_send);
}

#[test]
fn stage1a_new_tool_arguments_and_oversized_limits_match_wire_contract() {
    for (name, args) in [
        ("search_items", json!({"query":"q"})),
        ("items_by_tag", json!({"tag":"tag"})),
        ("list_tags", json!({"choice":"all"})),
        ("list_collections", json!({})),
        ("get_collection", json!({"id":"collection-id"})),
        ("list_library", json!({"kind":"rated","stars":0})),
        (
            "get_item_info",
            json!({"items":[{"path":"C:/image.png","subresource":null}]}),
        ),
    ] {
        let mut args = args;
        args["limit"] = json!(1000);
        let (_, command) = parse_tool(&json!({"name":name,"arguments":args})).unwrap();
        let wire = json!(command);
        assert_eq!(wire["limit"], MAX_LIMIT);
        if name == "list_library" {
            assert_eq!(wire["stars"], 1);
        }
        args["limit"] = json!(0);
        assert!(parse_tool(&json!({"name":name,"arguments":args})).is_err());
    }
    for args in [
        json!({}),
        json!({"id":"a","name":"b"}),
        json!({"id":""}),
        json!({"name":"  "}),
    ] {
        assert!(parse_tool(&json!({"name":"get_collection","arguments":args})).is_err());
    }
    for kind in [
        "favorites",
        "reading_history",
        "bookshelf",
        "bookmarks",
        "rated",
    ] {
        assert!(parse_tool(&json!({"name":"list_library","arguments":{"kind":kind}})).is_ok());
    }
    for args in [
        json!({"kind":"unknown"}),
        json!({"kind":"rated","stars":6}),
        json!({"kind":"rated","stars":-1}),
    ] {
        assert!(parse_tool(&json!({"name":"list_library","arguments":args})).is_err());
    }
    assert!(parse_tool(&json!({"name":"get_item_info","arguments":{"items":vec![json!({"path":"C:/image.png"});MAX_INFO_LIMIT as usize]}})).is_ok());
    for items in [
        json!([]),
        json!([{"path":""}]),
        json!([{"path":"C:/image.png","unknown":true}]),
        json!(vec![
            json!({"path":"C:/image.png"});
            MAX_INFO_LIMIT as usize + 1
        ]),
    ] {
        assert!(parse_tool(&json!({"name":"get_item_info","arguments":{"items":items}})).is_err());
    }
}

// The subset of JSON Schema used by these tool definitions. This checks the
// actual serialized wire fixtures, including nested tagged/nullable fields.
fn accepts_schema(schema: &Value, value: &Value) -> bool {
    if schema
        .get("const")
        .is_some_and(|expected| expected != value)
    {
        return false;
    }
    if schema
        .get("enum")
        .and_then(Value::as_array)
        .is_some_and(|values| !values.contains(value))
    {
        return false;
    }
    for (keyword, exactly_one) in [("anyOf", false), ("oneOf", true)] {
        if let Some(choices) = schema.get(keyword).and_then(Value::as_array) {
            let matches = choices
                .iter()
                .filter(|choice| accepts_schema(choice, value))
                .count();
            if matches == 0 || exactly_one && matches != 1 {
                return false;
            }
        }
    }
    let matches_type = |kind: &str| match kind {
        "object" => value.is_object(),
        "array" => value.is_array(),
        "string" => value.is_string(),
        "boolean" => value.is_boolean(),
        "null" => value.is_null(),
        "integer" => value.is_i64() || value.is_u64(),
        "number" => value.is_number(),
        _ => false,
    };
    if let Some(kind) = schema.get("type") {
        if !match kind {
            Value::String(kind) => matches_type(kind),
            Value::Array(kinds) => kinds
                .iter()
                .any(|kind| kind.as_str().is_some_and(matches_type)),
            _ => false,
        } {
            return false;
        }
    }
    if let Some(object) = value.as_object() {
        if schema
            .get("required")
            .and_then(Value::as_array)
            .is_some_and(|keys| {
                keys.iter()
                    .any(|key| !object.contains_key(key.as_str().unwrap()))
            })
        {
            return false;
        }
        for (key, value) in object {
            if let Some(property) = schema.get("properties").and_then(|p| p.get(key)) {
                if !accepts_schema(property, value) {
                    return false;
                }
            } else if schema.get("additionalProperties") == Some(&json!(false)) {
                return false;
            }
        }
    }
    if let Some(array) = value.as_array() {
        if schema
            .get("minItems")
            .and_then(Value::as_u64)
            .is_some_and(|min| array.len() < (min as usize))
            || schema
                .get("maxItems")
                .and_then(Value::as_u64)
                .is_some_and(|max| array.len() > max as usize)
        {
            return false;
        }
        if schema
            .get("items")
            .is_some_and(|item| array.iter().any(|value| !accepts_schema(item, value)))
        {
            return false;
        }
    }
    if let Some(text) = value.as_str() {
        let length = text.chars().count();
        if schema
            .get("minLength")
            .and_then(Value::as_u64)
            .is_some_and(|min| length < min as usize)
            || schema
                .get("maxLength")
                .and_then(Value::as_u64)
                .is_some_and(|max| length > max as usize)
        {
            return false;
        }
        if schema.get("pattern") == Some(&json!("^[0-9a-f]{64}$"))
            && (text.len() != 64
                || !text
                    .bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)))
        {
            return false;
        }
    }
    if let Some(number) = value.as_f64() {
        if schema
            .get("minimum")
            .and_then(Value::as_f64)
            .is_some_and(|min| number < min)
            || schema
                .get("maximum")
                .and_then(Value::as_f64)
                .is_some_and(|max| number > max)
        {
            return false;
        }
    }
    true
}

#[test]
fn stage1a_schema_validates_serialized_payloads_and_explicit_absence_markers() {
    let backend = Fixture { disabled: false };
    for (name, args) in [
        ("miv_status", json!({})),
        ("search_items", json!({"query":"q"})),
        ("list_tags", json!({"choice":"all"})),
        ("items_by_tag", json!({"tag":"t"})),
        ("list_collections", json!({})),
        ("get_collection", json!({"name":"fixture"})),
        ("list_library", json!({"kind":"bookmarks"})),
        (
            "get_item_info",
            json!({"items":[{"path":"C:/image.png","subresource":"entry.png"}]}),
        ),
    ] {
        let schema = schema::tools()
            .into_iter()
            .find(|tool| tool["name"] == name)
            .unwrap();
        assert!(
            accepts_schema(&schema["inputSchema"], &args),
            "input schema: {name}"
        );
        let (_, command) = parse_tool(&json!({"name":name,"arguments":args})).unwrap();
        let result = tool_result(
            name,
            backend.call(command, "fixture", Arc::new(CallControl::default())),
        );
        assert_eq!(result["isError"], false);
        assert!(
            accepts_schema(&schema["outputSchema"], &result["structuredContent"]),
            "output schema: {name}: {result}"
        );
        assert_eq!(
            serde_json::from_str::<Value>(result["content"][0]["text"].as_str().unwrap()).unwrap(),
            result["structuredContent"]
        );
        let failure = tool_result(
            name,
            Err(AgentError::new(ErrorCode::Timeout, "fixture timeout")),
        );
        assert!(accepts_schema(
            &schema["outputSchema"],
            &failure["structuredContent"]
        ));
    }
    let mut info = fixture_info(ItemRef {
        path: "C:/book.pdf".into(),
        subresource: Some("1".into()),
    });
    info.size = InfoField::NotApplicable;
    info.modified = InfoField::Unavailable;
    info.dimensions = InfoField::TooLarge;
    info.tags = InfoField::NotApplicable;
    info.exif = InfoField::NotApplicable;
    info.generation = InfoField::Unavailable;
    let page = InfoPage {
        items: vec![info.into(), PageRow::too_long(&"large-reference").unwrap()],
        cursor: None,
        truncated: true,
    };
    let result = tool_result("get_item_info", Ok(Payload::Info(page)));
    let schema = schema::tools()
        .into_iter()
        .find(|tool| tool["name"] == "get_item_info")
        .unwrap();
    assert!(accepts_schema(
        &schema["outputSchema"],
        &result["structuredContent"]
    ));
    assert_eq!(
        result["structuredContent"]["items"][0]["size"],
        json!({"status":"not_applicable"})
    );
    let mut forbidden = result["structuredContent"].clone();
    forbidden["items"][0]["workflow_json"] = json!("private");
    assert!(!accepts_schema(&schema["outputSchema"], &forbidden));
    let virtual_page = ItemPage {
        items: vec![
            Item {
                path: "C:/book.zip".into(),
                subresource: Some("page.png".into()),
                name: "page.png".into(),
                kind: ItemKind::Image,
                tags: None,
                container_tags: Some(vec!["book-tag".into()]),
                rating: 4,
            }
            .into(),
        ],
        cursor: None,
        truncated: false,
    };
    let result = tool_result("get_collection", Ok(Payload::Items(virtual_page)));
    let schema = schema::tools()
        .into_iter()
        .find(|tool| tool["name"] == "get_collection")
        .unwrap();
    assert!(accepts_schema(
        &schema["outputSchema"],
        &result["structuredContent"]
    ));
    assert!(result["structuredContent"]["items"][0]["tags"].is_null());
    assert_eq!(
        result["structuredContent"]["items"][0]["container_tags"],
        json!(["book-tag"])
    );
}

#[test]
fn stage1a_shared_content_budget_and_final_envelope_include_escaping() {
    let page = InfoPage {
        items: vec![
            fixture_info(ItemRef {
                path: "C:/image.png".into(),
                subresource: None,
            })
            .into(),
            PageRow::too_long(&"omitted").unwrap(),
        ],
        cursor: Some("c".repeat(8192)),
        truncated: true,
    };
    let content_chars = mcp_content_chars(&page).unwrap();
    assert!(content_chars <= MAX_PAGE_CHARS);
    for row in &page.items {
        assert!(mcp_content_chars(row).unwrap() <= MAX_ITEM_CHARS);
    }
    let result = tool_result("get_item_info", Ok(Payload::Info(page)));
    assert_eq!(result["isError"], false);
    let envelope = success(json!("id".repeat(128)), result, true);
    let serialized = serialize_response(&envelope);
    assert!(serialized.chars().count() <= MAX_MCP_RESPONSE_CHARS);
    assert!(serialized.chars().count() - content_chars <= MCP_ENVELOPE_RESERVE_CHARS);
    assert!(
        serde_json::from_str::<Value>(&serialized)
            .unwrap()
            .get("error")
            .is_none()
    );
}

#[test]
fn permission_observer_compares_advertised_tools_rather_than_snapshot_bits() {
    let (stop, _recv) = mpsc::sync_channel(1);
    let mut observer = PermissionObserver {
        stop,
        revision: Arc::new(AtomicU64::new(0)),
        effective: None,
    };
    assert!(
        observer
            .observe(schema::tool_names(PermissionSnapshot::default()), None)
            .is_none()
    );
    assert!(
        observer
            .observe(
                schema::tool_names(PermissionSnapshot {
                    level: AgentLevel::ReadOnly,
                    ..Default::default()
                }),
                None
            )
            .is_some()
    );
    assert!(
        observer
            .observe(
                schema::tool_names(PermissionSnapshot {
                    level: AgentLevel::ReadAndOpen,
                    thumbnails: false,
                    gps: true
                }),
                None
            )
            .is_none()
    );
    assert!(
        observer
            .observe(
                schema::tool_names(PermissionSnapshot {
                    level: AgentLevel::ReadOnly,
                    thumbnails: false,
                    gps: true
                }),
                None
            )
            .is_none()
    );
    assert!(
        observer
            .observe(schema::tool_names(PermissionSnapshot::default()), None)
            .is_some()
    );
}

use mimageviewer_agent_ipc::*;
use serde_json::json;

#[test]
fn favorite_commands_preserve_omitted_and_explicit_index_flags() {
    assert_eq!(AGENT_PROTOCOL_VERSION, 7);
    assert_eq!(MIN_ADAPTER_PROTOCOL_VERSION, 3);
    for (fields, name, item_index, container_index) in [
        (json!({}), None, None, None),
        (
            json!({"name":null,"item_index":null,"container_index":null}),
            None,
            None,
            None,
        ),
        (
            json!({"name":"猫🦊","item_index":true,"container_index":false}),
            Some("猫🦊".into()),
            Some(true),
            Some(false),
        ),
        (
            json!({"item_index":false,"container_index":true}),
            None,
            Some(false),
            Some(true),
        ),
    ] {
        let mut value = json!({"type":"set_favorite","path":"C:/猫🦊"});
        value
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        let command = Command::SetFavorite {
            path: "C:/猫🦊".into(),
            name,
            item_index,
            container_index,
        };
        assert_eq!(serde_json::from_value::<Command>(value).unwrap(), command);
        let request = Request { id: 1, command };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &request).unwrap();
        assert_eq!(
            read_frame::<_, Request>(&mut bytes.as_slice()).unwrap(),
            Some(request)
        );
    }
}

#[test]
fn favorite_results_roundtrip_and_discard_unknown_fields() {
    for added in [false, true] {
        let value = json!({
            "path":"C:/猫🦊", "name":"猫🦊", "added":added,
            "before":if added { serde_json::Value::Null } else { json!({"item_index":false,"container_index":true,"unknown":"discard"}) },
            "after":{"item_index":true,"container_index":false,"unknown":"discard"},
            "message":if added { Some("新しく登録したお気に入りは AI から削除できません。削除するときは mIV の「お気に入り → 編集」を使ってください") } else { None },
            "unknown":"discard"
        });
        let typed: FavoriteWriteResult = serde_json::from_value(value).unwrap();
        assert_eq!(typed.before.is_none(), added);
        let output = serde_json::to_value(&typed).unwrap();
        assert!(output.get("before").is_some());
        assert_eq!(output.get("message").is_some(), added);
        assert!(!output.to_string().contains("discard"));
        let response = Response {
            id: 2,
            result: Ok(Payload::FavoriteWrite(typed)),
        };
        let mut bytes = Vec::new();
        write_frame(&mut bytes, &response).unwrap();
        assert_eq!(
            read_frame::<_, Response>(&mut bytes.as_slice()).unwrap(),
            Some(response.clone())
        );
        assert!(mcp_content_chars(&response).unwrap() <= MAX_PAGE_CHARS);
    }
}

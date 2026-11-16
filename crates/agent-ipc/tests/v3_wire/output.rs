// Frozen from 8d6201dce:crates/mcp-server/src/lib.rs (tool_result + attachment validation).
use super::*;
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Value, json};
pub fn tool_result(name: &str, result: Result<Payload, AgentError>) -> Value {
    let expected = matches!(
        (name, &result),
        ("miv_status", Ok(Payload::Status(_)))
            | (
                "search_items" | "items_by_tag" | "get_collection",
                Ok(Payload::Items(_))
            )
            | ("list_tags", Ok(Payload::Tags(_)))
            | ("list_collections", Ok(Payload::Collections(_)))
            | ("list_library", Ok(Payload::Library(_)))
            | ("get_item_info", Ok(Payload::Info(_)))
            | ("list_folder", Ok(Payload::Folder(_)))
            | ("get_thumbnail", Ok(Payload::Thumbnails(_)))
            | ("open_in_viewer", Ok(Payload::Opened(_)))
            | ("add_tags" | "remove_tags", Ok(Payload::TagWrite(_)))
            | ("set_rating", Ok(Payload::RatingWrite(_)))
            | (
                "create_collection",
                Ok(Payload::CollectionWrite(
                    CollectionWriteResult::Created { .. }
                ))
            )
            | (
                "rename_collection",
                Ok(Payload::CollectionWrite(
                    CollectionWriteResult::Renamed { .. }
                ))
            )
            | (
                "add_to_collection" | "remove_from_collection",
                Ok(Payload::CollectionWrite(
                    CollectionWriteResult::Items { .. }
                ))
            )
    );
    let mut images = vec![];
    let (value, is_error) = match result {
        Ok(payload) if expected => (
            match payload {
                Payload::Status(v) => json!(v),
                Payload::Items(v) => json!(v),
                Payload::Tags(v) => json!(v),
                Payload::Collections(v) => json!(v),
                Payload::Library(v) => json!(v),
                Payload::Info(v) => json!(v),
                Payload::Folder(v) => json!(v),
                Payload::Opened(v) => json!(v),
                Payload::CollectionWrite(v) => json!(v),
                Payload::TagWrite(v) | Payload::RatingWrite(v) => json!(v),
                Payload::Thumbnails(v) => {
                    if !valid_thumbnail_attachments(&v) {
                        return tool_result(
                            name,
                            Err(AgentError::new(
                                ErrorCode::Internal,
                                "mIV の縮小画像の応答を確認できませんでした",
                            )),
                        );
                    }
                    images = v.images;
                    json!(v.page)
                }
                _ => unreachable!(),
            },
            false,
        ),
        Err(error) => (json!({"error":error}), true),
        _ => (
            json!({"error":{"code":"internal","message":"mIV からの応答を確認できませんでした"}}),
            true,
        ),
    };
    let rows_within_budget = value
        .get("items")
        .or_else(|| value.get("tags"))
        .or_else(|| value.get("collections"))
        .and_then(Value::as_array)
        .is_none_or(|rows| {
            rows.iter()
                .all(|row| mcp_content_chars(row).is_ok_and(|chars| chars <= MAX_ITEM_CHARS))
        });
    let (value, is_error) = if rows_within_budget
        && mcp_content_chars(&value).is_ok_and(|chars| chars <= MAX_PAGE_CHARS)
    {
        (value, is_error)
    } else {
        images.clear();
        (
            json!({"error":{"code":"internal","message":"mIV の応答が出力予算を超えました"}}),
            true,
        )
    };
    let mut content = vec![json!({"type":"text","text":value.to_string()})];
    content.extend(images.into_iter().map(|image| {
        json!({
            "type":"image", "mimeType":"image/webp", "data":STANDARD.encode(image.webp)
        })
    }));
    json!({"content":content,"structuredContent":value,"isError":is_error})
}

fn valid_thumbnail_attachments(response: &ThumbnailResponse) -> bool {
    if response.page.items.len() > MAX_THUMBNAIL_ITEMS {
        return false;
    }
    let mut attachments = response.images.iter();
    let mut total = 0usize;
    for (index, row) in response.page.items.iter().enumerate() {
        if matches!(row, PageRow::Value(item) if item.status == ThumbnailStatus::Ok) {
            let Some(image) = attachments.next() else {
                return false;
            };
            if image.item_index != index
                || image.webp.is_empty()
                || image.webp.len() > MAX_THUMBNAIL_IMAGE_BYTES
            {
                return false;
            }
            total += image.webp.len();
            if total > MAX_THUMBNAIL_TOTAL_BYTES {
                return false;
            }
        }
    }
    attachments.next().is_none()
}

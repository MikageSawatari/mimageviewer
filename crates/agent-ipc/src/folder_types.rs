use serde::{Deserialize, Serialize};

use crate::{Dimensions, ItemRef, PageRow};

/// Counts filtered logical rows before pagination or oversized-row replacement.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderSummary {
    pub total: usize,
    pub kinds: std::collections::BTreeMap<FolderKind, usize>,
    pub stars: [usize; 6],
    pub folders: Option<Vec<FolderSummaryFolder>>,
    pub folders_truncated: bool,
    pub book_composition_applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderSummaryFolder {
    pub path: String,
    pub name: String,
    pub total: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderPage {
    pub items: Vec<PageRow<FolderItem>>,
    pub cursor: Option<String>,
    pub truncated: bool,
    pub book_composition_applied: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct FolderItem {
    pub path: String,
    pub subresource: Option<String>,
    pub name: String,
    pub kind: FolderKind,
    pub size: Option<u64>,
    /// UTC Unix milliseconds, when known.
    pub modified: Option<i64>,
    pub rating: Option<u8>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, PartialOrd, Ord)]
#[serde(rename_all = "snake_case")]
pub enum FolderKind {
    Folder,
    Image,
    Video,
    Audio,
    Zip,
    Pdf,
    Epub,
    Archive,
    ZipFolder,
    PdfPage,
    ZipEntry,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThumbnailResponse {
    pub page: ThumbnailPage,
    pub images: Vec<ThumbnailImage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThumbnailPage {
    pub items: Vec<PageRow<ThumbnailItem>>,
    pub cursor: Option<String>,
    pub truncated: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThumbnailItem {
    pub item: ItemRef,
    pub status: ThumbnailStatus,
    pub dimensions: Option<Dimensions>,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum ThumbnailStatus {
    Ok,
    TooLarge,
    Unavailable,
    Unsupported,
    HiddenByEdit,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct ThumbnailImage {
    /// Zero-based row in this response page, including oversized-row markers.
    pub item_index: usize,
    /// Base64 on the IPC JSON wire, never a JSON array of bytes.
    #[serde(with = "webp_base64")]
    pub webp: Vec<u8>,
}

mod webp_base64 {
    use base64::{Engine, engine::general_purpose::STANDARD};
    use serde::{Deserialize, Deserializer, Serializer, de::Error};

    pub fn serialize<S: Serializer>(bytes: &[u8], serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(bytes))
    }

    pub fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Vec<u8>, D::Error> {
        let encoded = String::deserialize(deserializer)?;
        let max = crate::MAX_THUMBNAIL_WIRE_IMAGE_BYTES;
        if encoded.len() > max.div_ceil(3) * 4 {
            return Err(D::Error::custom("thumbnail exceeds image byte budget"));
        }
        let bytes = STANDARD.decode(encoded).map_err(D::Error::custom)?;
        if bytes.len() > max {
            return Err(D::Error::custom("thumbnail exceeds image byte budget"));
        }
        Ok(bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::*;

    #[test]
    fn maximum_large_thumbnail_response_fits_the_existing_frame() {
        let response = ThumbnailResponse {
            page: ThumbnailPage {
                items: (0..2)
                    .map(|_| {
                        ThumbnailItem {
                            item: ItemRef {
                                path: "C:/photo.png".into(),
                                subresource: None,
                            },
                            status: ThumbnailStatus::Ok,
                            dimensions: Some(Dimensions {
                                width: 1568,
                                height: 1568,
                                unit: DimensionUnit::Pixels,
                            }),
                        }
                        .into()
                    })
                    .collect(),
                cursor: Some("next".into()),
                truncated: true,
            },
            images: (0..2)
                .map(|item_index| ThumbnailImage {
                    item_index,
                    webp: vec![255; MAX_THUMBNAIL_WIRE_IMAGE_BYTES],
                })
                .collect(),
        };
        let mut frame = vec![];
        write_frame(&mut frame, &response).unwrap();
        assert!(frame.len() < MAX_FRAME_BYTES);
        assert_eq!(
            read_frame::<_, ThumbnailResponse>(&mut frame.as_slice()).unwrap(),
            Some(response)
        );
    }

    #[test]
    fn thumbnail_budget_changes_only_for_new_protocol_and_large_request() {
        for version in [3, 4, 5] {
            for side in [256, 512, 513, 1568, u32::MAX] {
                let clamped = clamp_thumbnail_side(version, side);
                assert_eq!(clamped, side.min(if version == 5 { 1568 } else { 512 }));
                let budget = thumbnail_budget(version, clamped);
                let large = version == 5 && clamped > 512;
                assert_eq!(
                    budget.max_image_bytes,
                    if large { 1024 * 1024 } else { 256 * 1024 }
                );
                assert_eq!(
                    budget.max_total_bytes,
                    if large { 2 * 1024 * 1024 } else { 1024 * 1024 }
                );
                assert_eq!(budget.max_images, if large { 4 } else { 20 });
            }
        }
    }

    #[test]
    fn filter_omission_and_null_have_identical_wire_defaults() {
        for base in [
            serde_json::json!({"type":"list_folder","path":"C:/photos"}),
            serde_json::json!({"type":"list_library","kind":"rated"}),
        ] {
            let mut explicit = base.clone();
            explicit["max_stars"] = serde_json::Value::Null;
            if base["type"] == "list_folder" {
                explicit["min_stars"] = serde_json::Value::Null;
                explicit["kinds"] = serde_json::Value::Null;
                explicit["recursive"] = serde_json::json!(false);
                for key in ["max_depth", "name_contains", "extensions", "tagged"] {
                    explicit[key] = serde_json::Value::Null;
                }
            }
            let omitted: Command = serde_json::from_value(base).unwrap();
            let null: Command = serde_json::from_value(explicit).unwrap();
            assert_eq!(omitted, null);
            assert_eq!(
                serde_json::to_value(omitted).unwrap(),
                serde_json::to_value(null).unwrap()
            );
        }
        for command in [
            serde_json::json!({"type":"list_folder","path":"C:/photos","min_stars":0,"max_stars":2,"kinds":["image","audio"]}),
            serde_json::json!({"type":"list_library","kind":"rated","stars":2,"max_stars":2}),
        ] {
            let parsed: Command = serde_json::from_value(command.clone()).unwrap();
            let wire = serde_json::to_value(parsed).unwrap();
            for (key, value) in command.as_object().unwrap() {
                assert_eq!(&wire[key], value);
            }
        }
    }

    #[test]
    fn folder_and_thumbnail_commands_and_payloads_roundtrip() {
        let item = ItemRef {
            path: "C:/猫.zip".into(),
            subresource: Some("猫.png".into()),
        };
        for command in [
            Command::ListFolder {
                recursive: false,
                max_depth: None,
                name_contains: None,
                extensions: None,
                tagged: None,
                min_stars: None,
                max_stars: None,
                kinds: None,
                path: item.path.clone(),
                subresource: Some("pages/".into()),
                limit: Some(100),
                cursor: Some("folder".into()),
            },
            Command::GetThumbnail {
                items: vec![item.clone()],
                long_side: Some(512),
                cursor: Some("thumb".into()),
            },
        ] {
            let request = Request { id: 17, command };
            let mut bytes = vec![];
            write_frame(&mut bytes, &request).unwrap();
            assert_eq!(
                read_frame::<_, Request>(&mut bytes.as_slice()).unwrap(),
                Some(request)
            );
        }
        let response = Response {
            id: 17,
            result: Ok(Payload::Thumbnails(ThumbnailResponse {
                page: ThumbnailPage {
                    items: (0..4)
                        .map(|_| {
                            ThumbnailItem {
                                item: item.clone(),
                                status: ThumbnailStatus::Ok,
                                dimensions: Some(Dimensions {
                                    width: 256,
                                    height: 128,
                                    unit: DimensionUnit::Pixels,
                                }),
                            }
                            .into()
                        })
                        .collect(),
                    cursor: None,
                    truncated: false,
                },
                images: (0..4)
                    .map(|item_index| ThumbnailImage {
                        item_index,
                        webp: vec![255; MAX_THUMBNAIL_IMAGE_BYTES],
                    })
                    .collect(),
            })),
        };
        // Four maximum-size images would exceed 4 MiB as JSON byte arrays.
        // Base64 keeps the full 1 MiB decoded budget well within the frame.
        let mut frame = vec![];
        write_frame(&mut frame, &response).unwrap();
        assert!(frame.len() < 2 * 1024 * 1024);
        assert_eq!(
            read_frame::<_, Response>(&mut frame.as_slice()).unwrap(),
            Some(response)
        );
        let image = ThumbnailImage {
            item_index: 0,
            webp: vec![1, 2, 3],
        };
        assert_eq!(serde_json::to_value(image).unwrap()["webp"], "AQID");
        // A decoded overrun can have the same base64 length as the exact cap.
        let oversized = ThumbnailImage {
            item_index: 0,
            webp: vec![255; MAX_THUMBNAIL_WIRE_IMAGE_BYTES + 1],
        };
        assert!(
            serde_json::from_value::<ThumbnailImage>(serde_json::to_value(oversized).unwrap())
                .is_err()
        );
        for webp in [
            serde_json::json!([1, 2, 3]),
            serde_json::json!("?"),
            serde_json::json!("A".repeat(MAX_THUMBNAIL_WIRE_IMAGE_BYTES.div_ceil(3) * 4 + 4)),
        ] {
            assert!(
                serde_json::from_value::<ThumbnailImage>(
                    serde_json::json!({"item_index":0,"webp":webp})
                )
                .is_err()
            );
        }
        let page = FolderPage {
            items: vec![
                FolderItem {
                    rating: Some(0),
                    tags: None,
                    path: "C:/book.pdf".into(),
                    subresource: Some("0".into()),
                    name: "page 1".into(),
                    kind: FolderKind::PdfPage,
                    size: None,
                    modified: None,
                }
                .into(),
            ],
            cursor: None,
            truncated: false,
            book_composition_applied: true,
        };
        let payload = Payload::Folder(page);
        assert_eq!(
            serde_json::from_value::<Payload>(serde_json::to_value(&payload).unwrap()).unwrap(),
            payload
        );
    }
}

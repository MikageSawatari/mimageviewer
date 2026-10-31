use serde::{Deserialize, Serialize};

use crate::{Dimensions, ItemRef, PageRow};

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
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
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
        let max = crate::MAX_THUMBNAIL_IMAGE_BYTES;
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
    fn folder_and_thumbnail_commands_and_payloads_roundtrip() {
        let item = ItemRef {
            path: "C:/猫.zip".into(),
            subresource: Some("猫.png".into()),
        };
        for command in [
            Command::ListFolder {
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
            webp: vec![255; MAX_THUMBNAIL_IMAGE_BYTES + 1],
        };
        assert!(
            serde_json::from_value::<ThumbnailImage>(serde_json::to_value(oversized).unwrap())
                .is_err()
        );
        for webp in [
            serde_json::json!([1, 2, 3]),
            serde_json::json!("?"),
            serde_json::json!("A".repeat(MAX_THUMBNAIL_IMAGE_BYTES.div_ceil(3) * 4 + 4)),
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

use std::collections::HashMap;
use std::fmt::Write as _;
use std::path::PathBuf;
use std::sync::OnceLock;

use mimageviewer_ipc::{
    VideoStreamJumpEntry, VideoStreamJumpEntryId, VideoStreamJumpKind, VideoStreamJumpListPayload,
    VideoStreamJumpSection, VideoStreamJumpThumbnailPayload,
};
use sha2::{Digest, Sha256};

pub(crate) struct VideoJumpCatalogSource {
    path: PathBuf,
    chapters: Vec<crate::video::decoder::Chapter>,
    catalog: OnceLock<VideoJumpCatalog>,
}

impl VideoJumpCatalogSource {
    pub(crate) fn new(path: PathBuf, chapters: Vec<crate::video::decoder::Chapter>) -> Self {
        Self {
            path,
            chapters,
            catalog: OnceLock::new(),
        }
    }

    pub(crate) fn list(&self) -> VideoStreamJumpListPayload {
        self.catalog().list.clone()
    }

    pub(crate) fn thumbnail(&self, token: &str) -> VideoStreamJumpThumbnailPayload {
        self.catalog()
            .thumbnails
            .get(token)
            .cloned()
            .map(|webp_bytes| VideoStreamJumpThumbnailPayload::Found { webp_bytes })
            .unwrap_or(VideoStreamJumpThumbnailPayload::Missing)
    }

    fn catalog(&self) -> &VideoJumpCatalog {
        self.catalog.get_or_init(|| {
            let sources = LoadedJumpSources::load(&self.path);
            build_catalog(&self.chapters, sources)
        })
    }
}

struct VideoJumpCatalog {
    list: VideoStreamJumpListPayload,
    thumbnails: HashMap<String, Vec<u8>>,
}

#[derive(Default)]
struct LoadedJumpSources {
    pin: Option<crate::video_pins::VideoPin>,
    bookmarks: Vec<crate::video_bookmarks::VideoBookmark>,
    chapter_thumbnails: HashMap<i64, Vec<u8>>,
}

impl LoadedJumpSources {
    fn load(path: &std::path::Path) -> Self {
        let pin =
            crate::video_pins::VideoPinDb::open_readonly(&crate::video_pins::VideoPinDb::db_path())
                .ok()
                .and_then(|db| db.lookup(path));
        let bookmarks = crate::video_bookmarks::VideoBookmarkDb::open_readonly()
            .map(|db| db.list(path))
            .unwrap_or_default();
        let chapter_thumbnails = crate::video_chapter_thumbs::VideoChapterThumbDb::open_readonly()
            .map(|db| {
                db.list(path)
                    .into_iter()
                    .map(|chapter| {
                        (
                            crate::video_chapter_thumbs::chapter_start_key(chapter.start_secs),
                            chapter.thumb_webp,
                        )
                    })
                    .collect()
            })
            .unwrap_or_default();
        Self {
            pin,
            bookmarks,
            chapter_thumbnails,
        }
    }
}

struct CatalogEntry {
    id: VideoStreamJumpEntryId,
    kind: VideoStreamJumpKind,
    position_secs: f64,
    title: Option<String>,
    thumbnail: Vec<u8>,
}

fn build_catalog(
    chapters: &[crate::video::decoder::Chapter],
    mut sources: LoadedJumpSources,
) -> VideoJumpCatalog {
    let mut entries = Vec::new();
    if let Some(pin) = sources.pin.take() {
        let position_us = position_key(pin.pin_pts_secs);
        let thumbnail = if pin.thumb_is_current() {
            pin.thumb_webp
        } else {
            Vec::new()
        };
        entries.push(CatalogEntry {
            id: VideoStreamJumpEntryId::Pin { position_us },
            kind: VideoStreamJumpKind::Pin,
            position_secs: pin.pin_pts_secs,
            title: Some("代表フレーム".to_owned()),
            thumbnail,
        });
    }
    entries.extend(sources.bookmarks.into_iter().map(|bookmark| CatalogEntry {
        id: VideoStreamJumpEntryId::Bookmark {
            bookmark_id: bookmark.id,
        },
        kind: VideoStreamJumpKind::Bookmark,
        position_secs: bookmark.pts_secs,
        title: bookmark.title,
        thumbnail: bookmark.thumb_webp,
    }));
    entries.extend(chapters.iter().map(|chapter| {
        let start_us = crate::video_chapter_thumbs::chapter_start_key(chapter.start_secs);
        CatalogEntry {
            id: VideoStreamJumpEntryId::Chapter { start_us },
            kind: VideoStreamJumpKind::Chapter,
            position_secs: chapter.start_secs,
            title: chapter.title.clone(),
            thumbnail: sources
                .chapter_thumbnails
                .remove(&start_us)
                .unwrap_or_default(),
        }
    }));
    entries.retain(|entry| entry.position_secs.is_finite() && entry.position_secs >= 0.0);
    entries.sort_by(|left, right| {
        left.position_secs
            .partial_cmp(&right.position_secs)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| kind_order(left.kind).cmp(&kind_order(right.kind)))
    });

    let positions: Vec<_> = entries.iter().map(|entry| entry.position_secs).collect();
    let mut thumbnails = HashMap::new();
    let sections = [
        (VideoStreamJumpKind::Pin, "ピン留め"),
        (VideoStreamJumpKind::Bookmark, "ブックマーク"),
        (VideoStreamJumpKind::Chapter, "チャプター"),
    ]
    .into_iter()
    .filter_map(|(kind, label)| {
        let section_entries: Vec<_> = entries
            .iter()
            .filter(|entry| entry.kind == kind)
            .map(|entry| {
                let thumbnail_token = (!entry.thumbnail.is_empty()).then(|| {
                    let token = thumbnail_token(entry);
                    thumbnails.insert(token.clone(), entry.thumbnail.clone());
                    token
                });
                VideoStreamJumpEntry {
                    id: entry.id.clone(),
                    position_secs: entry.position_secs,
                    display_time: crate::video_jump::format_jump_entry_time(
                        entry.position_secs,
                        positions.iter().copied(),
                    ),
                    title: entry.title.clone(),
                    thumbnail_token,
                }
            })
            .collect();
        (!section_entries.is_empty()).then(|| VideoStreamJumpSection {
            kind,
            label: label.to_owned(),
            entries: section_entries,
        })
    })
    .collect();

    VideoJumpCatalog {
        list: VideoStreamJumpListPayload { sections },
        thumbnails,
    }
}

fn position_key(position_secs: f64) -> i64 {
    (position_secs.max(0.0) * 1_000_000.0).round() as i64
}

fn kind_order(kind: VideoStreamJumpKind) -> u8 {
    match kind {
        VideoStreamJumpKind::Pin => 0,
        VideoStreamJumpKind::Bookmark => 1,
        VideoStreamJumpKind::Chapter => 2,
    }
}

fn thumbnail_token(entry: &CatalogEntry) -> String {
    let logical_id = match entry.id {
        VideoStreamJumpEntryId::Pin { position_us } => position_us,
        VideoStreamJumpEntryId::Bookmark { bookmark_id } => bookmark_id,
        VideoStreamJumpEntryId::Chapter { start_us } => start_us,
    };
    let mut digest_text = String::with_capacity(64);
    for byte in Sha256::digest(&entry.thumbnail) {
        let _ = write!(&mut digest_text, "{byte:02x}");
    }
    format!(
        "v1:{}:{logical_id}:{digest_text}",
        match entry.kind {
            VideoStreamJumpKind::Pin => "pin",
            VideoStreamJumpKind::Bookmark => "bookmark",
            VideoStreamJumpKind::Chapter => "chapter",
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn catalog_orders_sections_and_uses_global_time_disambiguation() {
        let chapters = vec![crate::video::decoder::Chapter {
            start_secs: 80.04,
            end_secs: 90.0,
            title: Some("Chapter".to_owned()),
        }];
        let sources = LoadedJumpSources {
            pin: Some(crate::video_pins::VideoPin {
                pin_pts_secs: 80.0,
                thumb_webp: vec![1, 2, 3],
                thumb_pts_secs: Some(80.0),
            }),
            bookmarks: vec![crate::video_bookmarks::VideoBookmark {
                id: 7,
                pts_secs: 12.0,
                title: Some("Bookmark".to_owned()),
                thumb_webp: vec![4, 5, 6],
            }],
            chapter_thumbnails: HashMap::from([(
                crate::video_chapter_thumbs::chapter_start_key(80.04),
                vec![7, 8, 9],
            )]),
        };
        let catalog = build_catalog(&chapters, sources);
        assert_eq!(catalog.list.sections.len(), 3);
        assert_eq!(catalog.list.sections[0].kind, VideoStreamJumpKind::Pin);
        assert_eq!(catalog.list.sections[1].kind, VideoStreamJumpKind::Bookmark);
        assert_eq!(catalog.list.sections[2].kind, VideoStreamJumpKind::Chapter);
        assert_eq!(catalog.list.sections[0].entries[0].display_time, "1:20.000");
        assert_eq!(catalog.list.sections[2].entries[0].display_time, "1:20.040");
        assert_eq!(catalog.thumbnails.len(), 3);
    }

    #[test]
    fn stale_pin_thumbnail_is_not_published() {
        let sources = LoadedJumpSources {
            pin: Some(crate::video_pins::VideoPin {
                pin_pts_secs: 20.0,
                thumb_webp: vec![1, 2, 3],
                thumb_pts_secs: Some(10.0),
            }),
            ..LoadedJumpSources::default()
        };
        let catalog = build_catalog(&[], sources);
        assert!(
            catalog.list.sections[0].entries[0]
                .thumbnail_token
                .is_none()
        );
        assert!(catalog.thumbnails.is_empty());
    }
}

//! Shared, read-only rating identity and current/legacy precedence.
use std::collections::HashMap;

use crate::grid_item::GridItem;

pub(super) struct RatingKey {
    current: String,
    legacy: Option<String>,
}

impl RatingKey {
    pub(super) fn for_item(item: &GridItem, legacy: Option<String>) -> Self {
        let current = match item {
            GridItem::ZipDir {
                zip_path,
                dir_prefix,
                ..
            } => super::metadata_write::zip_directory_rating_key(zip_path, dir_prefix),
            _ => crate::edit_source::page_key_for_grid_item(item).unwrap_or_else(|| {
                crate::adjustment_db::normalize_path(
                    &item
                        .drag_source_path()
                        .expect("rating item has a physical path"),
                )
            }),
        };
        Self { current, legacy }
    }

    pub(super) fn keys(&self) -> impl Iterator<Item = &String> {
        std::iter::once(&self.current).chain(self.legacy.iter())
    }

    pub(super) fn resolve(&self, ratings: &HashMap<String, u8>) -> u8 {
        // Presence matters: an explicit current zero overrides legacy stars.
        ratings
            .get(&self.current)
            .copied()
            .or_else(|| {
                self.legacy
                    .as_ref()
                    .and_then(|key| ratings.get(key).copied())
            })
            .unwrap_or(0)
    }
}

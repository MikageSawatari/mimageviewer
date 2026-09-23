//! Exact page-key edit state prepared off the UI thread for cross-folder grids.
//!
//! The keyed snapshot owns values for one accepted item generation. Index maps are only a
//! projection of those values onto that generation's order; absence from an index map does not
//! prove that a page has no durable edit.

use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, Ordering};

use crate::grid_item::GridItem;

#[derive(Clone, Copy, Default)]
pub(crate) struct PageEditAvailability {
    pub adjustment: bool,
    pub export_crop: bool,
    pub view_trim: bool,
    pub mask: bool,
    pub conceal: bool,
    pub comic: bool,
    pub local_adjust: bool,
}

impl PageEditAvailability {
    pub(super) fn for_app(app: &super::App) -> Self {
        Self {
            adjustment: app.adjustment_db.is_some(),
            export_crop: app.export_crop_db.is_some(),
            view_trim: app.view_trim_db.is_some(),
            mask: app.mask_db.is_some(),
            conceal: app.conceal_db.is_some(),
            comic: app.comic_db.is_some(),
            local_adjust: app.local_adjust_db.is_some(),
        }
    }
}

#[derive(Clone, Debug, Default)]
pub(crate) struct PageEditSnapshot {
    pub adjustment: HashMap<String, crate::adjustment::AdjustParams>,
    pub export_crop: HashMap<String, crate::export_crop::CropSettings>,
    pub view_trim: HashMap<String, crate::view_trim::ViewTrimPageOverride>,
    pub mask: HashSet<String>,
    pub conceal: HashSet<String>,
    pub comic: HashSet<String>,
    pub local_adjust: HashSet<String>,
}

#[derive(Debug, Default)]
pub(crate) struct PageEditProjection {
    pub adjustment: HashMap<usize, crate::adjustment::AdjustParams>,
    pub export_crop: HashMap<usize, crate::export_crop::CropSettings>,
    pub view_trim: HashMap<usize, crate::view_trim::ViewTrimPageOverride>,
    pub mask: HashSet<usize>,
    pub conceal: HashSet<usize>,
    pub comic: HashSet<usize>,
    pub local_adjust: HashSet<usize>,
}

impl PageEditSnapshot {
    pub(super) fn load_and_project(
        items: &[GridItem],
        available: PageEditAvailability,
        cancel: &AtomicBool,
    ) -> Result<Option<(Self, PageEditProjection)>, String> {
        let keys = items
            .iter()
            .filter_map(crate::edit_source::page_key_for_grid_item)
            .collect::<Vec<_>>();
        let key_refs = keys.iter().map(String::as_str).collect::<Vec<_>>();
        let mut snapshot = Self::default();
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        if !keys.is_empty() {
            if available.adjustment {
                snapshot.adjustment = crate::adjustment_db::AdjustmentDb::open_readonly()
                    .map_err(|e| format!("補正 DB を読み込めませんでした: {e}"))?
                    .load_page_params_many(&key_refs);
            }
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if available.export_crop {
                snapshot.export_crop = crate::export_crop::CropDb::open_readonly(
                    &crate::export_crop::CropDb::db_path(),
                )
                .map_err(|e| format!("切り取り DB を読み込めませんでした: {e}"))?
                .load_many(&key_refs);
            }
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if available.view_trim {
                snapshot.view_trim = crate::view_trim_db::ViewTrimDb::open_existing_read_only_at(
                    &crate::data_dir::get().join("view_trim.db"),
                )
                .map_err(|e| format!("表示トリミング DB を読み込めませんでした: {e}"))?
                .map(|db| db.load_page_overrides_many(&key_refs))
                .unwrap_or_default();
            }
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if available.mask {
                snapshot.mask = crate::mask_db::MaskDb::open_readonly()
                    .map_err(|e| format!("消しゴム DB を読み込めませんでした: {e}"))?
                    .load_existing_mask_keys(&key_refs);
            }
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if available.conceal {
                snapshot.conceal = crate::conceal_db::ConcealDb::open_readonly(
                    &crate::conceal_db::ConcealDb::db_path(),
                )
                .map_err(|e| format!("隠蔽加工 DB を読み込めませんでした: {e}"))?
                .load_existing_conceal_keys(&key_refs);
            }
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if available.comic {
                snapshot.comic = crate::comic_db::ComicDb::open_readonly()
                    .map_err(|e| format!("注釈 DB を読み込めませんでした: {e}"))?
                    .load_existing_comic_keys(&key_refs);
            }
            if cancel.load(Ordering::Relaxed) {
                return Ok(None);
            }
            if available.local_adjust {
                snapshot.local_adjust = crate::local_adjust_db::LocalAdjustDb::open_readonly(
                    &crate::local_adjust_db::LocalAdjustDb::db_path(),
                )
                .map_err(|e| format!("補正レイヤー DB を読み込めませんでした: {e}"))?
                .load_existing_layer_keys(&keys);
            }
        }
        if cancel.load(Ordering::Relaxed) {
            return Ok(None);
        }
        let projection = snapshot.project(items);
        Ok((!cancel.load(Ordering::Relaxed)).then_some((snapshot, projection)))
    }

    /// Only call for a prepared view. Large projection stays in its prepare worker.
    pub(super) fn project(&self, items: &[GridItem]) -> PageEditProjection {
        let mut projection = PageEditProjection::default();
        for (index, item) in items.iter().enumerate() {
            let Some(key) = crate::edit_source::page_key_for_grid_item(item) else {
                continue;
            };
            if let Some(value) = self.adjustment.get(&key) {
                projection.adjustment.insert(index, value.clone());
            }
            if let Some(value) = self.export_crop.get(&key) {
                projection.export_crop.insert(index, *value);
            }
            if let Some(value) = self.view_trim.get(&key) {
                projection.view_trim.insert(index, *value);
            }
            if self.mask.contains(&key) {
                projection.mask.insert(index);
            }
            if self.conceal.contains(&key) {
                projection.conceal.insert(index);
            }
            if self.comic.contains(&key) {
                projection.comic.insert(index);
            }
            if self.local_adjust.contains(&key) {
                projection.local_adjust.insert(index);
            }
        }
        projection
    }

    fn set_value<T: Clone>(map: &mut HashMap<String, T>, key: &str, value: Option<&T>) {
        if let Some(value) = value {
            map.insert(key.to_owned(), value.clone());
        } else {
            map.remove(key);
        }
    }

    fn set_presence(set: &mut HashSet<String>, key: &str, present: bool) {
        if present {
            set.insert(key.to_owned());
        } else {
            set.remove(key);
        }
    }

    pub(super) fn sync_key(&mut self, key: &str, index: usize, app: &super::App) {
        Self::set_value(
            &mut self.adjustment,
            key,
            app.adjustment_page_params.get(&index),
        );
        Self::set_value(
            &mut self.export_crop,
            key,
            app.export_crop_page_settings.get(&index),
        );
        Self::set_value(
            &mut self.view_trim,
            key,
            app.view_trim_page_overrides.get(&index),
        );
        Self::set_presence(&mut self.mask, key, app.mask_pages.contains(&index));
        Self::set_presence(&mut self.conceal, key, app.conceal_pages.contains(&index));
        Self::set_presence(&mut self.comic, key, app.comic_pages.contains(&index));
        Self::set_presence(
            &mut self.local_adjust,
            key,
            app.local_adjust_pages.contains(&index),
        );
    }
}

//! 同じ物理一覧の並び替えを跨ぐ表示だけの素材。要求・Loaded の所有には参加しない。

use super::*;

#[derive(Clone)]
pub(crate) struct ThumbnailDisplayImage {
    pub tex: egui::TextureHandle,
    pub view: crate::cover_auto_material::AutoCropView,
    pub rotation: crate::rotation_db::Rotation,
}

#[derive(Clone)]
pub(super) struct Standin {
    key: String,
    stamp: (i64, i64),
    image: ThumbnailDisplayImage,
}

#[derive(Clone, Default)]
pub(crate) struct ThumbnailStandins {
    generation: u64,
    images: HashMap<usize, Standin>,
}

impl ThumbnailStandins {
    pub(crate) fn clear(&mut self) {
        self.images.clear();
    }

    pub(crate) fn set_generation(&mut self, generation: u64) {
        if self.generation != generation {
            self.clear();
            self.generation = generation;
        }
    }

    pub(crate) fn remove(&mut self, index: usize) {
        self.images.remove(&index);
    }

    pub(crate) fn retain_keep_set(&mut self, keep: &HashSet<usize>) {
        self.images.retain(|index, _| keep.contains(index));
    }

    pub(super) fn attach(
        &mut self,
        index: usize,
        key: &str,
        stamp: Option<(i64, i64)>,
        captured: &HashMap<String, Standin>,
    ) {
        if let Some(entry) = captured.get(key).filter(|entry| Some(entry.stamp) == stamp) {
            self.images.insert(index, entry.clone());
        }
    }
}

impl App {
    /// 通常画と選択後の再描画・採用前の控えが同じ素材を選ぶ。生成・I/O はしない。
    pub(crate) fn grid_thumbnail_display_image(
        &self,
        index: usize,
    ) -> Option<ThumbnailDisplayImage> {
        let item = self.items.get(index)?;
        if crate::grid_item::container_icon_only(item, self.settings.thumb_container_icon_kinds) {
            return None;
        }
        match self.thumbnails.get(index)? {
            thumb @ ThumbnailState::Loaded { tex, .. } => {
                let adjusted = !self.adjustment_dragging
                    && matches!(
                        item,
                        GridItem::Folder(_)
                            | GridItem::Image(_)
                            | GridItem::ZipImage { .. }
                            | GridItem::PdfPage { .. }
                    );
                let tex = if adjusted {
                    self.thumb_adjust_tex.get(&index).unwrap_or(tex)
                } else {
                    tex
                };
                // ページ・動画の実描画は get_rotation の index cache を優先する。
                // 既に採用した角度だけを peek し、表紙は旧 Loaded.cover の参照元を保つ。
                let rotation = match item {
                    GridItem::Image(_)
                    | GridItem::ZipImage { .. }
                    | GridItem::PdfPage { .. }
                    | GridItem::Video(_) => self
                        .peek_rotation(index)
                        .unwrap_or_else(|| self.grid_tile_rotation_memory(index)),
                    _ => self.grid_tile_rotation_memory(index),
                };
                Some(ThumbnailDisplayImage {
                    tex: tex.clone(),
                    view: thumb.tile_view_for_texture(tex, rotation),
                    rotation,
                })
            }
            ThumbnailState::Pending if !self.thumbnail_source_unavailable(index) => {
                if self.thumbnail_standins.generation != self.items_generation {
                    return None;
                }
                self.thumbnail_standins
                    .images
                    .get(&index)
                    .map(|entry| entry.image.clone())
            }
            _ => None,
        }
    }

    fn thumbnail_source_unavailable(&self, index: usize) -> bool {
        let Some(GridItem::ConvertibleArchive { path, .. }) = self.items.get(index) else {
            return false;
        };
        self.converted_archive_cache_paths
            .get(&crate::path_key::normalize_keep_drive(path))
            .is_some_and(|source| {
                matches!(
                    source.read_source(),
                    ConvertedArchiveSourceState::Unavailable { .. }
                )
            })
    }

    /// 控えるのは保持集合内の表示素材と実測 stamp だけ。元の state は変更しない。
    pub(super) fn capture_thumbnail_standins(&self) -> HashMap<String, Standin> {
        let started = crate::perf::is_enabled().then(std::time::Instant::now);
        let captured: HashMap<_, _> = self
            .keep_set
            .iter()
            .filter_map(|&index| {
                let key = crate::folder_order::physical_item_key(self.items.get(index)?)?;
                let stamp = self.image_metas.get(index).copied().flatten()?;
                let entry = if matches!(self.thumbnails.get(index), Some(ThumbnailState::Pending)) {
                    // 連続した作り直しでも元の identity/stamp を維持する。
                    self.grid_thumbnail_display_image(index)?;
                    let entry = self.thumbnail_standins.images.get(&index)?;
                    if entry.key != key || entry.stamp != stamp {
                        return None;
                    }
                    entry.clone()
                } else {
                    Standin {
                        key: key.clone(),
                        stamp,
                        image: self.grid_thumbnail_display_image(index)?,
                    }
                };
                Some((key, entry))
            })
            .collect();
        if let Some(started) = started {
            crate::perf::event(
                "nav",
                "order_refresh_standin_capture",
                None,
                0,
                &[
                    ("items", serde_json::Value::from(captured.len())),
                    (
                        "ms",
                        serde_json::Value::from(started.elapsed().as_secs_f64() * 1000.0),
                    ),
                ],
            );
        }
        captured
    }

    /// state の終端と source の確定を、表示専用 owner にも反映する。
    pub(super) fn reconcile_thumbnail_standins(&mut self) {
        self.thumbnail_standins
            .set_generation(self.items_generation);
        let retired = self
            .thumbnail_standins
            .images
            .keys()
            .copied()
            .filter(|&index| {
                !matches!(self.thumbnails.get(index), Some(ThumbnailState::Pending))
                    || self.thumbnail_source_unavailable(index)
            })
            .collect::<Vec<_>>();
        for index in retired {
            self.thumbnail_standins.remove(index);
        }
    }
}

#[cfg(test)]
#[path = "thumbnail_standin_tests.rs"]
mod tests;

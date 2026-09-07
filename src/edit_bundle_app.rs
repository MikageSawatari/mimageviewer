//! `PageEditBundle` と App の UI / cache / sidecar 統合。

use eframe::egui;

use crate::app::App;
use crate::edit_bundle::{
    EditBundleApplyPending, EditBundleClipboard, EditBundleCopyPending, EditBundleDbPaths,
    EditBundlePasteRequest, EditMaskSnapshot, PageEditBundle, PreparedPageEditBundle,
};
use crate::grid_item::{GridItem, ThumbnailState};

#[derive(Clone, Copy)]
pub(crate) struct PageEditUndoInvalidation {
    pub(crate) adjustment: bool,
    pub(crate) mask: bool,
    pub(crate) conceal: bool,
    pub(crate) local_adjust: bool,
    pub(crate) comic: bool,
}

impl PageEditUndoInvalidation {
    pub(crate) const fn all() -> Self {
        Self {
            adjustment: true,
            mask: true,
            conceal: true,
            local_adjust: true,
            comic: true,
        }
    }
}

impl App {
    pub(crate) fn is_page_edit_bundle_target(&self, idx: usize) -> bool {
        self.items.get(idx).is_some_and(GridItem::has_page_data)
    }

    pub(crate) fn has_page_edit_bundle_clipboard(&self) -> bool {
        self.edit_bundle_clipboard.is_some()
    }

    pub(crate) fn known_page_source_size(&self, idx: usize) -> Option<[usize; 2]> {
        if let Some((width, height)) = self.source_dims_for_idx(idx) {
            return Some([
                width.round().max(1.0) as usize,
                height.round().max(1.0) as usize,
            ]);
        }
        if let Some(ThumbnailState::Loaded {
            source_dims: Some((width, height)),
            ..
        }) = self.thumbnails.get(idx)
        {
            return Some([(*width).max(1) as usize, (*height).max(1) as usize]);
        }
        None
    }

    pub(crate) fn edit_bundle_databases_available(&self) -> bool {
        self.adjustment_db.is_some()
            && self.mask_db.is_some()
            && self.conceal_db.is_some()
            && self.local_adjust_db.is_some()
            && self.export_crop_db.is_some()
            && self.comic_db.is_some()
    }

    pub(crate) fn copy_page_edit_bundle(&mut self, idx: usize) {
        if self.edit_bundle_bulk_pending.is_some() {
            self.show_feedback_toast("複数ページの編集内容を処理しています".to_string());
            return;
        }
        if !self.is_page_edit_bundle_target(idx) {
            return;
        }
        if !self.edit_bundle_databases_available() {
            self.show_feedback_toast("編集データベースを利用できません".to_string());
            return;
        }
        let Some(key) = self.page_path_key(idx) else {
            return;
        };
        let Some(source_size) = self.known_page_source_size(idx) else {
            self.show_feedback_toast(
                "画像サイズを取得できません。画像を一度フルスクリーンで表示してから再実行してください"
                    .to_string(),
            );
            return;
        };

        let source_label = self
            .items
            .get(idx)
            .map(|item| item.name().to_string())
            .unwrap_or_else(|| key.clone());
        let adjust_override = self.adjustment_page_params.get(&idx).cloned();
        let local_adjust_override = self.local_adjust_page_layers.get(&idx).cloned();
        let crop_override = self.ensure_export_crop_source_size(idx, source_size);
        let comic_override = self.comic_docs.get(&key).cloned();
        let paths = EditBundleDbPaths::default_data_dir();
        let (tx, rx) = std::sync::mpsc::channel();
        let worker_key = key.clone();
        match std::thread::Builder::new()
            .name("edit-bundle-copy".to_string())
            .spawn(move || {
                let result = load_page_edit_bundle(
                    &paths,
                    &worker_key,
                    source_size,
                    adjust_override,
                    local_adjust_override,
                    crop_override,
                    comic_override,
                );
                let _ = tx.send(result);
            }) {
            Ok(_) => {
                // 読み取りだけなので、連続操作時は古い receiver を捨てて最新要求を優先できる。
                self.edit_bundle_clipboard = None;
                self.edit_bundle_copy_pending = Some(EditBundleCopyPending {
                    source_label: source_label.clone(),
                    rx,
                });
                self.show_feedback_toast(format!("編集内容を読み込んでいます: {source_label}"));
            }
            Err(error) => self
                .show_feedback_toast(format!("編集内容の読み込みを開始できませんでした: {error}")),
        }
    }

    pub(crate) fn request_paste_page_edit_bundle(&mut self, idx: usize) {
        if self.edit_bundle_bulk_pending.is_some() {
            self.show_feedback_toast("複数ページの編集内容を処理しています".to_string());
            return;
        }
        if !self.is_page_edit_bundle_target(idx) || self.edit_bundle_clipboard.is_none() {
            return;
        }
        if !self.edit_bundle_databases_available() {
            self.show_feedback_toast("編集データベースを利用できません".to_string());
            return;
        }
        let Some(target_key) = self.page_path_key(idx) else {
            return;
        };
        let Some(target_size) = self.known_page_source_size(idx) else {
            self.show_feedback_toast(
                "画像サイズを取得できません。画像を一度フルスクリーンで表示してから再実行してください"
                    .to_string(),
            );
            return;
        };
        let target_label = self
            .items
            .get(idx)
            .map(|item| item.name().to_string())
            .unwrap_or_else(|| target_key.clone());
        let request = EditBundlePasteRequest {
            target_idx: idx,
            target_key: target_key.clone(),
            target_label,
            target_size,
            items_generation: self.items_generation,
            sidecar_coords: (!self.idx_is_compiled_book_page(idx))
                .then(|| self.sidecar_coords(idx))
                .flatten(),
        };
        if self.page_has_any_bundle_edit(idx, &target_key) {
            self.edit_bundle_paste_pending = Some(request);
        } else {
            self.start_apply_page_edit_bundle(request);
        }
    }

    pub(crate) fn page_has_any_bundle_edit(&self, _idx: usize, key: &str) -> bool {
        self.adjusted_page_keys.contains(key)
            || self.mask_page_keys.contains(key)
            || self.conceal_page_keys.contains(key)
            || self.local_adjust_page_keys.contains(key)
            || self.comic_page_keys.contains(key)
            || self.export_crop_page_keys.contains(key)
    }

    pub(crate) fn start_apply_page_edit_bundle(&mut self, request: EditBundlePasteRequest) {
        if self.edit_bundle_bulk_pending.is_some() {
            self.show_feedback_toast("複数ページの編集内容を処理しています".to_string());
            return;
        }
        if self.edit_bundle_apply_pending.is_some() {
            self.show_feedback_toast("別の編集内容を貼り付けています".to_string());
            return;
        }
        let still_same_target = request.items_generation == self.items_generation
            && self.page_path_key(request.target_idx).as_deref() == Some(&request.target_key);
        if !still_same_target {
            self.show_feedback_toast(
                "一覧が更新されたため、編集内容の貼り付けをキャンセルしました".to_string(),
            );
            return;
        }
        if self.defer_single_page_edit_apply_for_local_adjust(request.clone()) {
            return;
        }
        let Some(clipboard) = self.edit_bundle_clipboard.clone() else {
            return;
        };
        let source_label = clipboard.source_label.clone();
        let source_bundle = clipboard.bundle;
        let worker_request = request.clone();
        let paths = EditBundleDbPaths::default_data_dir();
        let (tx, rx) = std::sync::mpsc::channel();
        match std::thread::Builder::new()
            .name("edit-bundle-paste".to_string())
            .spawn(move || {
                let result = source_bundle
                    .transformed_to(worker_request.target_size)
                    .and_then(|bundle| bundle.prepare())
                    .and_then(|prepared| {
                        prepared
                            .apply_atomic(&paths, &worker_request.target_key)
                            .map(|()| prepared)
                    });
                let _ = tx.send(result);
            }) {
            Ok(_) => {
                self.edit_bundle_apply_pending = Some(EditBundleApplyPending {
                    request,
                    source_label,
                    rx,
                });
            }
            Err(error) => self
                .show_feedback_toast(format!("編集内容の貼り付けを開始できませんでした: {error}")),
        }
    }

    pub(crate) fn commit_page_edit_bundle_to_runtime(
        &mut self,
        idx: Option<usize>,
        key: &str,
        sidecar_coords: Option<&(std::path::PathBuf, String)>,
        prepared: PreparedPageEditBundle,
    ) {
        let old_params = idx.map(|idx| self.effective_params(idx).clone());
        if prepared.adjust.is_some() {
            self.adjusted_page_keys.insert(key.to_string());
        } else {
            self.adjusted_page_keys.remove(key);
        }
        if let Some(idx) = idx {
            if let Some(adjust) = &prepared.adjust {
                self.adjustment_page_params.insert(idx, adjust.clone());
            } else {
                self.adjustment_page_params.remove(&idx);
            }
        }

        set_key_presence(&mut self.mask_page_keys, key, prepared.mask.is_some());
        set_key_presence(&mut self.conceal_page_keys, key, prepared.conceal.is_some());
        if let Some(idx) = idx {
            set_idx_presence(&mut self.mask_pages, idx, prepared.mask.is_some());
            set_idx_presence(&mut self.conceal_pages, idx, prepared.conceal.is_some());
        }

        let layers = prepared.local_adjust_layers.clone().unwrap_or_default();
        set_key_presence(&mut self.local_adjust_page_keys, key, !layers.is_empty());
        if let Some(idx) = idx {
            self.set_local_adjust_layers_for_idx_memory_only(idx, layers);
        }

        set_key_presence(
            &mut self.export_crop_page_keys,
            key,
            prepared.export_crop.is_some(),
        );
        if let Some(idx) = idx {
            if let Some(crop) = prepared.export_crop {
                self.export_crop_page_settings.insert(idx, crop);
                self.export_crop_pages.insert(idx);
            } else {
                self.export_crop_page_settings.remove(&idx);
                self.export_crop_pages.remove(&idx);
            }
        }

        let comic = prepared.comic.clone().unwrap_or_default();
        self.comic_docs.insert(key.to_string(), comic.clone());
        set_key_presence(&mut self.comic_page_keys, key, !comic.is_empty());
        if let Some(idx) = idx {
            set_idx_presence(&mut self.comic_pages, idx, !comic.is_empty());
            if self.fullscreen_idx == Some(idx) {
                self.text_selected = None;
                self.text_selected_ids.clear();
                self.text_list_selection_anchor = None;
                self.text_drag = None;
                self.text_marquee = None;
                self.text_smart_guides = crate::app::TextSmartGuides::default();
            }
        }

        let sidecar_entry = prepared.to_sidecar_entry();
        self.with_sidecar_coords_mut(sidecar_coords, move |sidecar, rel_key| {
            sidecar.replace_edit_bundle(rel_key, sidecar_entry)
        });

        // DB commit 後だけ runtime と派生 cache を切り替える。同じ frame で全系統を
        // invalidate するため、新旧 bundle が混じった表示を作らない。
        self.invalidate_edit_preview_cache_for_key(key);
        if let (Some(idx), Some(old_params)) = (idx, old_params) {
            let new_params = self.effective_params(idx).clone();
            self.invalidate_compare_prepared_for_idx(idx);
            self.clear_caches_for_param_change(idx, &old_params, &new_params);
            self.bump_erase_mask_generation(idx);
            self.bump_conceal_mask_generation(idx);
        }
        // set_local_adjust_layers_for_idx_memory_only が local generation を進める。
        self.mark_comic_dirty();
    }

    /// bundle の全置換は Undo entry を作らない。その代わり、成功前の編集状態を指す
    /// 対象ページ・対象種類の履歴だけを破棄する。★ / tag と別ページの履歴は残す。
    pub(crate) fn invalidate_page_edit_undo_after_bundle_apply(
        &mut self,
        requested_idx: usize,
        current_idx: Option<usize>,
        key: &str,
        kinds: PageEditUndoInvalidation,
    ) {
        let mut indices = vec![requested_idx];
        if let Some(current_idx) = current_idx
            && current_idx != requested_idx
        {
            indices.push(current_idx);
        }
        self.meta_undo.discard_page_edit_changes(
            &indices,
            key,
            kinds.adjustment,
            kinds.local_adjust,
        );

        if kinds.adjustment
            && self
                .adjustment_drag_session
                .as_ref()
                .is_some_and(|session| indices.contains(&session.fs_idx))
        {
            self.adjustment_drag_session = None;
        }

        let is_current_fullscreen = current_idx.is_some() && current_idx == self.fullscreen_idx;
        if is_current_fullscreen && kinds.mask {
            self.erase_undo_stack.clear();
            self.erase_redo_stack.clear();
            self.erase_last_undo_at = None;
        }
        if is_current_fullscreen && kinds.conceal {
            self.conceal_undo_stack.clear();
            self.conceal_redo_stack.clear();
            self.conceal_last_undo_at = None;
        }
        if kinds.comic && self.comic_undo_key.as_deref() == Some(key) {
            self.comic_undo_stack.clear();
            self.comic_redo_stack.clear();
            self.comic_undo_baseline = self.comic_docs.get(key).cloned().unwrap_or_default();
        }
    }

    pub(crate) fn show_edit_bundle_paste_confirm_dialog(&mut self, ctx: &egui::Context) {
        self.poll_edit_bundle_workers(ctx);
        if self.edit_bundle_apply_pending.is_some() {
            egui::Window::new("編集内容を貼り付け")
                .id(egui::Id::new("edit_bundle_paste_progress"))
                .collapsible(false)
                .resizable(false)
                .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
                .show(ctx, |ui| {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("編集内容を変換して貼り付けています…");
                    });
                });
            return;
        }
        let Some(pending) = self.edit_bundle_paste_pending.clone() else {
            return;
        };
        let escape_pressed = self.dialog_escape_pressed(ctx);
        let mut overwrite = false;
        let mut cancel = escape_pressed;
        let source_label = self
            .edit_bundle_clipboard
            .as_ref()
            .map(|clipboard| clipboard.source_label.as_str())
            .unwrap_or("コピー元");
        egui::Window::new("編集内容を貼り付け")
            .id(egui::Id::new("edit_bundle_paste_confirm"))
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_min_width(420.0);
                ui.label(format!(
                    "「{}」には既に編集内容があります。",
                    pending.target_label
                ));
                ui.label(format!(
                    "現在の編集内容をすべて消し、「{source_label}」の編集内容で上書きします。"
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("上書きして貼り付け").clicked() {
                        overwrite = true;
                    }
                    if ui.button("キャンセル").clicked() {
                        cancel = true;
                    }
                });
            });
        if cancel {
            self.edit_bundle_paste_pending = None;
        } else if overwrite {
            self.edit_bundle_paste_pending = None;
            self.start_apply_page_edit_bundle(pending);
        }
    }

    fn poll_edit_bundle_workers(&mut self, ctx: &egui::Context) {
        let copy_result = self
            .edit_bundle_copy_pending
            .as_ref()
            .and_then(|pending| match pending.rx.try_recv() {
                Ok(result) => Some(result),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    Some(Err("編集内容の読み込み worker が終了しました".to_string()))
                }
                Err(std::sync::mpsc::TryRecvError::Empty) => None,
            });
        if let Some(result) = copy_result {
            let pending = self.edit_bundle_copy_pending.take().unwrap();
            match result {
                Ok(bundle) if bundle.has_any() => {
                    self.edit_bundle_clipboard = Some(EditBundleClipboard {
                        source_label: pending.source_label.clone(),
                        bundle,
                    });
                    self.show_feedback_toast(format!(
                        "編集内容をコピーしました: {}",
                        pending.source_label
                    ));
                }
                Ok(_) => {
                    self.edit_bundle_clipboard = None;
                    self.show_feedback_toast(
                        "この画像にはコピーできる編集内容がありません".to_string(),
                    );
                }
                Err(error) => {
                    self.show_feedback_toast(format!("編集内容をコピーできませんでした: {error}"))
                }
            }
        }

        let apply_result =
            self.edit_bundle_apply_pending.as_ref().and_then(|pending| {
                match pending.rx.try_recv() {
                    Ok(result) => Some(result),
                    Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        Some(Err("編集内容の貼り付け worker が終了しました".to_string()))
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => None,
                }
            });
        if let Some(result) = apply_result {
            let pending = self.edit_bundle_apply_pending.take().unwrap();
            match result {
                Ok(prepared) => {
                    let current_idx = if self.page_path_key(pending.request.target_idx).as_deref()
                        == Some(pending.request.target_key.as_str())
                    {
                        Some(pending.request.target_idx)
                    } else {
                        (0..self.items.len()).find(|&idx| {
                            self.page_path_key(idx).as_deref()
                                == Some(pending.request.target_key.as_str())
                        })
                    };
                    self.commit_page_edit_bundle_to_runtime(
                        current_idx,
                        &pending.request.target_key,
                        pending.request.sidecar_coords.as_ref(),
                        prepared,
                    );
                    self.invalidate_page_edit_undo_after_bundle_apply(
                        pending.request.target_idx,
                        current_idx,
                        &pending.request.target_key,
                        PageEditUndoInvalidation::all(),
                    );
                    self.show_feedback_toast(format!(
                        "{} の編集内容を貼り付けました: {}",
                        pending.source_label, pending.request.target_label
                    ));
                }
                Err(error) => {
                    crate::logger::log(format!(
                        "edit_bundle: atomic paste failed key={} error={error}",
                        pending.request.target_key
                    ));
                    self.show_feedback_toast(format!(
                        "編集内容を貼り付けできませんでした: {error}"
                    ));
                }
            }
        }

        if self.edit_bundle_copy_pending.is_some() || self.edit_bundle_apply_pending.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(40));
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn load_page_edit_bundle(
    paths: &EditBundleDbPaths,
    key: &str,
    source_size: [usize; 2],
    adjust_override: Option<crate::adjustment::AdjustParams>,
    local_adjust_override: Option<local_adjust_core::LocalAdjustmentLayers>,
    crop_override: Option<crate::export_crop::CropSettings>,
    comic_override: Option<Vec<comic_core::AnnotationObject>>,
) -> Result<PageEditBundle, String> {
    let adjustment = crate::adjustment_db::AdjustmentDb::open_at(&paths.adjustment)
        .map_err(|e| format!("個別補正DBを開けませんでした: {e}"))?;
    let mask_db = crate::mask_db::MaskDb::open_at(&paths.mask)
        .map_err(|e| format!("消しゴムDBを開けませんでした: {e}"))?;
    let conceal_db = crate::conceal_db::ConcealDb::open_at(&paths.conceal)
        .map_err(|e| format!("隠蔽加工DBを開けませんでした: {e}"))?;
    let local_adjust_db = crate::local_adjust_db::LocalAdjustDb::open_at(&paths.local_adjust)
        .map_err(|e| format!("補正レイヤーDBを開けませんでした: {e}"))?;
    let crop_db = crate::export_crop::CropDb::open_at(&paths.export_crop)
        .map_err(|e| format!("切り取りDBを開けませんでした: {e}"))?;
    let comic_db = crate::comic_db::ComicDb::open_at(&paths.comic)
        .map_err(|e| format!("テキスト注釈DBを開けませんでした: {e}"))?;

    let snapshot_mask = |pixels, shapes| EditMaskSnapshot {
        pixels,
        shapes,
        size: source_size,
    };
    let export_crop = crop_override.or_else(|| crop_db.get(key)).map(|crop| {
        let adopted = crop.with_legacy_source_size(source_size);
        if crop.valid_source_size().is_none()
            && let Err(error) = crop_db.set(key, adopted)
        {
            crate::logger::log(format!(
                "edit_bundle: failed to adopt legacy crop source size key={key} error={error}"
            ));
        }
        adopted
    });
    Ok(PageEditBundle {
        source_size,
        adjust: adjust_override.or_else(|| adjustment.get_page_params(key)),
        mask: mask_db
            .get_full(key, source_size[0], source_size[1])
            .map(|(pixels, shapes)| snapshot_mask(pixels, shapes)),
        conceal: conceal_db
            .get_full(key, source_size[0], source_size[1])
            .map(|(pixels, shapes)| snapshot_mask(pixels, shapes)),
        local_adjust_layers: local_adjust_override
            .or_else(|| {
                local_adjust_db
                    .get_layers(key)
                    .map(local_adjust_core::LocalAdjustmentLayers::new)
            })
            .filter(|layers| !layers.is_empty()),
        export_crop,
        comic: comic_override
            .or_else(|| comic_db.get(key))
            .filter(|objects| !objects.is_empty()),
    })
}

fn set_key_presence(set: &mut std::collections::BTreeSet<String>, key: &str, present: bool) {
    if present {
        set.insert(key.to_string());
    } else {
        set.remove(key);
    }
}

fn set_idx_presence(set: &mut std::collections::HashSet<usize>, idx: usize, present: bool) {
    if present {
        set.insert(idx);
    } else {
        set.remove(&idx);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn worker_loader_reads_the_six_database_snapshot() {
        let dir = tempfile::tempdir().unwrap();
        let paths = EditBundleDbPaths::in_dir(dir.path());
        let _ = crate::adjustment_db::AdjustmentDb::open_at(&paths.adjustment).unwrap();
        let _ = crate::mask_db::MaskDb::open_at(&paths.mask).unwrap();
        let _ = crate::conceal_db::ConcealDb::open_at(&paths.conceal).unwrap();
        let _ = crate::local_adjust_db::LocalAdjustDb::open_at(&paths.local_adjust).unwrap();
        let _ = crate::export_crop::CropDb::open_at(&paths.export_crop).unwrap();
        let _ = crate::comic_db::ComicDb::open_at(&paths.comic).unwrap();

        let mut adjust = crate::adjustment::AdjustParams::default();
        adjust.brightness = 12.0;
        let source = PageEditBundle {
            source_size: [2, 2],
            adjust: Some(adjust.clone()),
            mask: Some(EditMaskSnapshot {
                pixels: vec![true, false, false, true],
                shapes: Vec::new(),
                size: [2, 2],
            }),
            ..PageEditBundle::default()
        };
        source
            .prepare()
            .unwrap()
            .apply_atomic(&paths, "page")
            .unwrap();

        let loaded = load_page_edit_bundle(&paths, "page", [2, 2], None, None, None, None)
            .expect("worker snapshot");
        assert_eq!(loaded.adjust, Some(adjust));
        assert_eq!(loaded.mask.unwrap().pixels, vec![true, false, false, true]);
        assert!(loaded.conceal.is_none());
    }
}

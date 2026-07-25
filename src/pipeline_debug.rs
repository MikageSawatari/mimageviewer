//! Display pipeline debug export.
//!
//! `Ctrl+Alt+Shift+D` snapshots the current fullscreen page(s), then a worker
//! writes stage PNGs and a manifest. The snapshot step captures the current cache
//! topology and lightweight mask metadata; PNG encoding, file I/O, and derived
//! recomposition stages stay off the UI thread.

use std::path::{Path, PathBuf};
use std::sync::{Arc, mpsc};
use std::time::{SystemTime, UNIX_EPOCH};

use eframe::egui;
use image::ImageEncoder;
use serde_json::json;

use crate::app::{App, EraseResultKey, LocalAdjustResultKey};
use crate::fs_animation::FsCacheEntry;
use crate::grid_item::GridItem;

pub(crate) struct PipelineDebugExportPending {
    pub(crate) rx: mpsc::Receiver<Result<PipelineDebugExportComplete, String>>,
}

pub(crate) struct PipelineDebugExportComplete {
    pub(crate) dir: PathBuf,
    pub(crate) manifest_path: PathBuf,
    pub(crate) stage_count: usize,
}

struct PipelineDebugExportWork {
    root_dir: PathBuf,
    created_unix_ms: u128,
    fullscreen_idx: usize,
    items_generation: u64,
    spread_mode: String,
    post_filter_bypassed: bool,
    pages: Vec<PipelineDebugPageWork>,
}

struct PipelineDebugPageWork {
    idx: usize,
    dir_name: String,
    item: String,
    path_key: Option<String>,
    input_generation: u64,
    erase_mask_generation: u64,
    local_adjust_generation: u64,
    conceal_mask_generation: u64,
    current_erase_key: EraseResultKey,
    current_local_adjust_key: LocalAdjustResultKey,
    post_filter: String,
    notes: Vec<String>,
    stages: Vec<PipelineDebugStageWork>,
}

enum PipelineDebugStageWork {
    Image {
        name: String,
        note: String,
        pixels: Arc<egui::ColorImage>,
    },
    Adjustment {
        name: String,
        note: String,
        source: Arc<egui::ColorImage>,
        params: crate::adjustment::AdjustParams,
        include_post_filter: bool,
    },
    EraseInput {
        name: String,
        note: String,
        source: Arc<egui::ColorImage>,
        params: crate::adjustment::AdjustParams,
        force_black_flatten: bool,
    },
    LocalAdjustCompose {
        name: String,
        note: String,
        source: Arc<egui::ColorImage>,
        layers: Vec<local_adjust_core::LocalAdjustmentLayer>,
    },
    ConcealCompose {
        name: String,
        note: String,
        base: Arc<egui::ColorImage>,
        mask: Arc<Vec<bool>>,
        preset: crate::conceal::ConcealPreset,
    },
    Missing {
        name: String,
        reason: String,
    },
}

struct SavedStage {
    name: String,
    file: Option<String>,
    size: Option<[usize; 2]>,
    note: String,
    missing: bool,
}

impl App {
    pub(crate) fn consume_pipeline_debug_shortcut(ctx: &egui::Context) -> bool {
        ctx.input_mut(|i| {
            let mut found = false;
            i.events.retain(|event| {
                let consume = matches!(
                    event,
                    egui::Event::Key {
                        key: egui::Key::D,
                        pressed: true,
                        repeat: false,
                        modifiers,
                        ..
                    } if modifiers.ctrl && modifiers.alt && modifiers.shift
                );
                if consume {
                    found = true;
                }
                !consume
            });
            found
        })
    }

    pub(crate) fn start_pipeline_debug_export(&mut self, ctx: &egui::Context, fs_idx: usize) {
        if self.pipeline_debug_export_pending.is_some() {
            self.show_feedback_toast("デバッグ出力中です".to_string());
            return;
        }
        if matches!(self.items.get(fs_idx), Some(GridItem::Video(_))) {
            self.show_feedback_toast("動画は画像パイプラインデバッグ対象外です".to_string());
            return;
        }

        let page_indices = match self.resolve_visible_spread_pair(fs_idx) {
            crate::ui_fullscreen::SpreadPair::Single => vec![fs_idx],
            crate::ui_fullscreen::SpreadPair::Double { left, right } => vec![left, right],
        };
        let created_unix_ms = unix_ms_now();
        let suffix = uuid::Uuid::new_v4().simple().to_string();
        let root_dir = crate::data_dir::get().join("debug-pipeline").join(format!(
            "{}_idx{:04}_{}",
            created_unix_ms,
            fs_idx,
            &suffix[..8]
        ));

        let mut pages = Vec::new();
        for (page_no, idx) in page_indices.into_iter().enumerate() {
            if self.items.get(idx).is_some_and(|item| item.has_page_data()) {
                pages.push(self.pipeline_debug_page_work(idx, page_no));
            }
        }

        let stage_count = pages
            .iter()
            .map(|page| {
                page.stages
                    .iter()
                    .filter(|stage| !matches!(stage, PipelineDebugStageWork::Missing { .. }))
                    .count()
            })
            .sum::<usize>();
        if pages.is_empty() || stage_count == 0 {
            self.show_feedback_toast("出力できる画像段階がまだありません".to_string());
            return;
        }

        let work = PipelineDebugExportWork {
            root_dir: root_dir.clone(),
            created_unix_ms,
            fullscreen_idx: fs_idx,
            items_generation: self.items_generation,
            spread_mode: format!("{:?}", self.spread_mode),
            post_filter_bypassed: self.post_filter_bypassed,
            pages,
        };
        let (tx, rx) = mpsc::channel();
        let thread = std::thread::Builder::new()
            .name("pipeline-debug-export".to_string())
            .spawn(move || {
                let result = run_pipeline_debug_export(work);
                let _ = tx.send(result);
            });
        match thread {
            Ok(_) => {
                crate::logger::log(format!(
                    "pipeline-debug: export started dir={}",
                    root_dir.display()
                ));
                self.pipeline_debug_export_pending = Some(PipelineDebugExportPending { rx });
                self.show_feedback_toast_with_duration(
                    "画像パイプラインをデバッグ出力中".to_string(),
                    2.0,
                );
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
            }
            Err(err) => {
                self.show_feedback_toast(format!("デバッグ出力 worker を開始できません: {err}"));
            }
        }
    }

    pub(crate) fn poll_pipeline_debug_export_pending(&mut self, ctx: &egui::Context) {
        let Some(pending) = self.pipeline_debug_export_pending.as_ref() else {
            return;
        };
        let result = match pending.rx.try_recv() {
            Ok(result) => result,
            Err(mpsc::TryRecvError::Empty) => {
                ctx.request_repaint_after(std::time::Duration::from_millis(100));
                return;
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                Err("デバッグ出力が中断されました".to_string())
            }
        };
        self.pipeline_debug_export_pending = None;
        match result {
            Ok(done) => {
                crate::logger::log(format!(
                    "pipeline-debug: export completed dir={} stages={}",
                    done.dir.display(),
                    done.stage_count
                ));
                let name = done
                    .dir
                    .file_name()
                    .and_then(|n| n.to_str())
                    .unwrap_or("debug-pipeline");
                #[cfg(windows)]
                self.show_native_video_overlay_toast(
                    format!("デバッグ出力しました: {name}"),
                    false,
                );
                self.fs_feedback_toast = Some((
                    format!("デバッグ出力しました: {name}"),
                    std::time::Instant::now(),
                    crate::ui_fullscreen::FEEDBACK_TOAST_DURATION,
                ));
                self.fs_feedback_toast_reveal_path = Some(done.manifest_path);
                self.fs_feedback_toast_surface = None;
            }
            Err(err) => {
                crate::logger::log(format!("pipeline-debug: export failed: {err}"));
                self.show_feedback_toast(format!("デバッグ出力に失敗: {err}"));
            }
        }
    }

    fn pipeline_debug_page_work(&self, idx: usize, page_no: usize) -> PipelineDebugPageWork {
        let mut page = PipelineDebugPageWork {
            idx,
            dir_name: format!("page_{page_no:02}_idx{idx:04}"),
            item: self
                .items
                .get(idx)
                .map(|item| item.display_path())
                .unwrap_or_else(|| "<missing item>".to_string()),
            path_key: self.page_path_key(idx),
            input_generation: self.input_generation.get(&idx).copied().unwrap_or(0),
            erase_mask_generation: self.erase_mask_generation.get(&idx).copied().unwrap_or(0),
            local_adjust_generation: self.local_adjust_generation.get(&idx).copied().unwrap_or(0),
            conceal_mask_generation: self.conceal_mask_generation.get(&idx).copied().unwrap_or(0),
            current_erase_key: self.current_erase_result_key(idx),
            current_local_adjust_key: self.current_local_adjust_key(idx),
            post_filter: format!("{:?}", self.effective_params(idx).post_filter),
            notes: Vec::new(),
            stages: Vec::new(),
        };

        self.add_raw_stage(idx, &mut page);
        self.add_ai_stages(idx, &mut page);
        self.add_adjustment_stages(idx, &mut page);
        self.add_erase_stages(idx, &mut page);
        self.add_local_adjust_stages(idx, &mut page);
        self.add_conceal_stages(idx, &mut page);
        page
    }

    fn add_raw_stage(&self, idx: usize, page: &mut PipelineDebugPageWork) {
        match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { pixels, .. }) => {
                page.push_image("00_fs_raw", "fs_cache Static pixels", Arc::clone(pixels));
            }
            Some(FsCacheEntry::Animated {
                frame_pixels,
                current_frame,
                ..
            }) => {
                if let Some(pixels) = frame_pixels.get(*current_frame) {
                    page.push_image(
                        "00_fs_raw_current_frame",
                        &format!("fs_cache Animated current_frame={current_frame}"),
                        Arc::clone(pixels),
                    );
                } else {
                    page.push_missing("00_fs_raw", "animated current frame pixels are missing");
                }
            }
            Some(FsCacheEntry::Failed) => {
                page.push_missing("00_fs_raw", "fs_cache is Failed");
            }
            Some(FsCacheEntry::Video { .. }) => {
                page.push_missing("00_fs_raw", "video item has no ColorImage stage");
            }
            None => {
                page.push_missing("00_fs_raw", "fs_cache has no entry");
            }
        }
    }

    fn add_ai_stages(&self, idx: usize, page: &mut PipelineDebugPageWork) {
        let display_bg = self.effective_upscale_bg_mode();
        if self.ai_will_apply_to(idx) {
            if let Some(FsCacheEntry::Static { pixels, .. }) =
                self.ai_upscale_cache.get(&(idx, display_bg))
            {
                page.push_image(
                    &format!("10_ai_display_bg{display_bg}"),
                    "ai_upscale_cache for effective display background",
                    Arc::clone(pixels),
                );
            } else {
                page.push_missing(
                    &format!("10_ai_display_bg{display_bg}"),
                    "ai_upscale_cache missing for effective display background",
                );
            }
        } else {
            page.push_missing(
                "10_ai_display",
                "AI upscale/denoise is not active for this page",
            );
        }

        let erase_bg = self.erase_upscale_bg_mode(idx);
        if erase_bg != display_bg {
            if let Some(FsCacheEntry::Static { pixels, .. }) =
                self.ai_upscale_cache.get(&(idx, erase_bg))
            {
                page.push_image(
                    &format!("11_ai_erase_bg{erase_bg}"),
                    "ai_upscale_cache used by erase input background policy",
                    Arc::clone(pixels),
                );
            } else {
                page.push_missing(
                    &format!("11_ai_erase_bg{erase_bg}"),
                    "AI cache for erase background policy is missing",
                );
            }
        }
    }

    fn add_adjustment_stages(&self, idx: usize, page: &mut PipelineDebugPageWork) {
        if let Some(FsCacheEntry::Static { pixels, .. }) = self.adjustment_cache.get(&idx) {
            page.push_image(
                "20_adjustment_cache_current",
                if self.post_filter_bypassed {
                    "adjustment_cache while post_filter_bypassed=true (usually color-only)"
                } else {
                    "adjustment_cache current display entry"
                },
                Arc::clone(pixels),
            );
        } else {
            page.push_missing(
                "20_adjustment_cache_current",
                "adjustment_cache has no entry",
            );
        }

        if let Some(source) = self.best_pre_adjustment_source(idx) {
            let params = self.effective_params(idx).clone();
            page.stages.push(PipelineDebugStageWork::Adjustment {
                name: "21_adjustment_recomputed_color_only".to_string(),
                note: "recomputed from best pre-adjustment source without post-filter".to_string(),
                source: Arc::clone(&source),
                params: params.clone(),
                include_post_filter: false,
            });
            if params.post_filter != crate::adjustment::PostFilter::None {
                page.stages.push(PipelineDebugStageWork::Adjustment {
                    name: "22_adjustment_recomputed_with_post_filter".to_string(),
                    note: "recomputed from best pre-adjustment source with post-filter".to_string(),
                    source,
                    params,
                    include_post_filter: true,
                });
            } else {
                page.push_missing(
                    "22_adjustment_recomputed_with_post_filter",
                    "post_filter is None",
                );
            }
        } else {
            page.push_missing(
                "21_adjustment_recomputed_color_only",
                "no pre-adjustment source is available",
            );
            page.push_missing(
                "22_adjustment_recomputed_with_post_filter",
                "no pre-adjustment source is available",
            );
        }
    }

    fn add_erase_stages(&self, idx: usize, page: &mut PipelineDebugPageWork) {
        if let Some(pixels) = self.erase_base_cache.get(&idx) {
            page.push_image(
                "29_erase_base_cache",
                "erase_base_cache pre-inpaint work base",
                Arc::clone(pixels),
            );
        } else {
            page.push_missing("29_erase_base_cache", "erase_base_cache has no entry");
        }

        if let Some(stage) = self.debug_erase_input_stage(idx) {
            page.stages.push(stage);
        } else {
            page.push_missing(
                "30_erase_input_exact_current",
                "current erase input resolver would return None",
            );
        }

        if let Some(mask) = self.debug_erase_mask_for_idx(idx) {
            page.push_image(
                "31_erase_mask_current",
                "erase mask as black/white debug image",
                Arc::new(mask_to_color_image(mask.size, &mask.mask)),
            );
        } else {
            page.push_missing("31_erase_mask_current", "erase mask is not loaded or saved");
        }

        if let Some(entry) = self.erase_preview_cache.get(&idx) {
            page.push_image(
                "40_erase_preview_cache",
                "erase preview cache pixels",
                Arc::clone(&entry.pixels),
            );
        } else {
            page.push_missing("40_erase_preview_cache", "erase_preview_cache has no entry");
        }

        let mut erase_entries = self
            .erase_result_cache
            .iter()
            .filter(|(key, _)| key.idx == idx)
            .map(|(key, entry)| (*key, Arc::clone(&entry.pixels)))
            .collect::<Vec<_>>();
        erase_entries.sort_by_key(|(key, _)| (key.input_gen, key.mask_gen));
        if erase_entries.is_empty() {
            page.push_missing(
                "50_erase_result_cache",
                "erase_result_cache has no entry for idx",
            );
        }
        for (key, pixels) in erase_entries {
            let name = if key == page.current_erase_key {
                "50_erase_result_current".to_string()
            } else {
                format!(
                    "51_erase_result_stale_input{}_mask{}",
                    key.input_gen, key.mask_gen
                )
            };
            page.push_image(
                &name,
                &format!(
                    "erase_result_cache key input_gen={} mask_gen={} current={}",
                    key.input_gen,
                    key.mask_gen,
                    key == page.current_erase_key
                ),
                pixels,
            );
        }
    }

    fn add_local_adjust_stages(&self, idx: usize, page: &mut PipelineDebugPageWork) {
        let layers = self
            .local_adjust_page_layers
            .get(&idx)
            .cloned()
            .unwrap_or_default();
        if layers.is_empty() {
            page.push_missing("55_local_adjust_layers", "no local adjustment layers");
        } else {
            let active_count = layers
                .iter()
                .filter(|layer| layer.enabled && layer.opacity > 0.0)
                .count();
            page.push_missing(
                "55_local_adjust_layers",
                &format!("{} layers loaded, {} active", layers.len(), active_count),
            );
        }

        let local_source = self.current_local_adjust_source_pixels(idx);
        if let Some(source) = local_source.as_ref() {
            page.push_image(
                "56_local_adjust_source_current",
                "current source for local adjustment render",
                Arc::clone(source),
            );
        } else if self.has_active_local_adjust_layers(idx) {
            page.push_missing(
                "56_local_adjust_source_current",
                "active local adjustment layers exist but source is not ready",
            );
        } else {
            page.push_missing(
                "56_local_adjust_source_current",
                "no active local adjustment layers",
            );
        }

        if let Some(source) = local_source
            && self.has_active_local_adjust_layers(idx)
        {
            page.stages
                .push(PipelineDebugStageWork::LocalAdjustCompose {
                    name: "57_local_adjust_recomputed".to_string(),
                    note: "worker recomposition from current local adjustment source".to_string(),
                    source,
                    layers: layers.clone(),
                });
        } else {
            page.push_missing(
                "57_local_adjust_recomputed",
                "no active local adjustment source to recompute",
            );
        }

        let mut entries = self
            .local_adjust_cache
            .iter()
            .filter(|(key, _)| key.idx == idx)
            .map(|(key, entry)| (*key, Arc::clone(&entry.pixels)))
            .collect::<Vec<_>>();
        entries.sort_by_key(|(key, _)| (key.input_gen, key.erase_mask_gen, key.local_gen));
        if entries.is_empty() {
            page.push_missing(
                "58_local_adjust_cache",
                "local_adjust_cache has no entry for idx",
            );
        }
        for (key, pixels) in entries {
            let name = if key == page.current_local_adjust_key {
                "58_local_adjust_current".to_string()
            } else {
                format!(
                    "58_local_adjust_stale_input{}_mask{}_local{}",
                    key.input_gen, key.erase_mask_gen, key.local_gen
                )
            };
            page.push_image(
                &name,
                &format!(
                    "local_adjust_cache key input_gen={} erase_mask_gen={} local_gen={} current={}",
                    key.input_gen,
                    key.erase_mask_gen,
                    key.local_gen,
                    key == page.current_local_adjust_key
                ),
                pixels,
            );
        }
    }

    fn add_conceal_stages(&self, idx: usize, page: &mut PipelineDebugPageWork) {
        if let Some(pixels) = self.conceal_base_cache.get(&idx) {
            page.push_image(
                "59_conceal_base_cache",
                "conceal edit entry base cache",
                Arc::clone(pixels),
            );
        } else {
            page.push_missing("59_conceal_base_cache", "conceal_base_cache has no entry");
        }

        let conceal_source = self.current_conceal_source_pixels(idx);
        if let Some((pixels, source_kind)) = conceal_source.as_ref() {
            page.push_image(
                "60_conceal_source_current",
                &format!("current_conceal_source_pixels source={source_kind}"),
                Arc::clone(pixels),
            );
        } else {
            page.push_missing(
                "60_conceal_source_current",
                "current_conceal_source_pixels returned None",
            );
        }

        if let Some(mask) = self
            .debug_conceal_mask_for_idx(idx, conceal_source.as_ref().map(|(pixels, _)| pixels.size))
        {
            page.push_image(
                "61_conceal_mask_current",
                "conceal composite mask as black/white debug image",
                Arc::new(mask_to_color_image(mask.size, &mask.mask)),
            );
            if let Some((base, source_kind)) = conceal_source {
                page.stages.push(PipelineDebugStageWork::ConcealCompose {
                    name: "80_conceal_composed_from_current_source".to_string(),
                    note: format!("worker recomposition from current conceal source={source_kind}"),
                    base,
                    mask: Arc::new(mask.mask),
                    preset: self.current_conceal_preset_from_settings(),
                });
            } else {
                page.push_missing(
                    "80_conceal_composed_from_current_source",
                    "no current conceal source",
                );
            }
        } else {
            page.push_missing(
                "61_conceal_mask_current",
                "conceal mask is not loaded or saved",
            );
            page.push_missing(
                "80_conceal_composed_from_current_source",
                "conceal mask is not loaded or saved",
            );
        }

        if let Some(entry) = self.conceal_cache.get(&idx) {
            let name = if entry.generation == self.conceal_generation {
                "70_conceal_cache_current"
            } else {
                "70_conceal_cache_stale"
            };
            page.push_image(
                name,
                &format!(
                    "conceal_cache generation={} current_generation={}",
                    entry.generation, self.conceal_generation
                ),
                Arc::clone(&entry.pixels),
            );
        } else {
            page.push_missing("70_conceal_cache", "conceal_cache has no entry");
        }
    }

    fn best_pre_adjustment_source(&self, idx: usize) -> Option<Arc<egui::ColorImage>> {
        let bg = self.effective_upscale_bg_mode();
        if self.ai_will_apply_to(idx)
            && let Some(FsCacheEntry::Static { pixels, .. }) = self.ai_upscale_cache.get(&(idx, bg))
        {
            return Some(Arc::clone(pixels));
        }
        match self.fs_cache.get(&idx) {
            Some(FsCacheEntry::Static { pixels, .. }) => Some(Arc::clone(pixels)),
            Some(FsCacheEntry::Animated {
                frame_pixels,
                current_frame,
                ..
            }) => frame_pixels.get(*current_frame).map(Arc::clone),
            _ => None,
        }
    }

    fn debug_erase_input_stage(&self, idx: usize) -> Option<PipelineDebugStageWork> {
        let params = self.effective_params(idx).clone();
        let force_black = self.fs_static_has_alpha(idx);
        let post_filter_active = params.post_filter != crate::adjustment::PostFilter::None;
        let can_reuse_adjustment_cache =
            !force_black && (!self.post_filter_bypassed || !post_filter_active);

        if can_reuse_adjustment_cache
            && let Some(FsCacheEntry::Static { pixels, .. }) = self.adjustment_cache.get(&idx)
        {
            return Some(PipelineDebugStageWork::Image {
                name: "30_erase_input_exact_current".to_string(),
                note: "erase input resolver reused adjustment_cache".to_string(),
                pixels: Arc::clone(pixels),
            });
        }

        let bg = self.erase_upscale_bg_mode(idx);
        let (source, source_kind) = if let Some(FsCacheEntry::Static { pixels, .. }) =
            self.ai_upscale_cache.get(&(idx, bg))
        {
            (Arc::clone(pixels), format!("ai_upscale_cache_bg{bg}"))
        } else if let Some(pixels) = self.erase_base_cache.get(&idx) {
            (Arc::clone(pixels), "erase_base_cache".to_string())
        } else {
            match self.fs_cache.get(&idx) {
                Some(FsCacheEntry::Static { pixels, .. }) => {
                    (Arc::clone(pixels), "fs_cache".to_string())
                }
                _ => return None,
            }
        };

        Some(PipelineDebugStageWork::EraseInput {
            name: "30_erase_input_exact_current".to_string(),
            note: format!(
                "worker recomputation of erase input resolver; source={source_kind}; force_black_flatten={force_black}"
            ),
            source,
            params,
            force_black_flatten: force_black,
        })
    }

    fn debug_erase_mask_for_idx(&self, idx: usize) -> Option<DebugMask> {
        if self.erase_mode && self.fullscreen_idx == Some(idx) {
            let mask = self.erase_mask.as_ref()?.clone();
            return Some(DebugMask {
                size: self.erase_mask_size,
                mask,
            });
        }
        let size = self.best_pre_adjustment_source(idx)?.size;
        let key = self.page_path_key(idx)?;
        let db = self.mask_db.as_ref()?;
        let (mut mask, shapes) = db.get_full(&key, size[0], size[1])?;
        crate::mask_db::rasterize_shapes_into(&mut mask, &shapes, size[0], size[1]);
        Some(DebugMask { size, mask })
    }

    fn debug_conceal_mask_for_idx(
        &self,
        idx: usize,
        source_size: Option<[usize; 2]>,
    ) -> Option<DebugMask> {
        if self.conceal_mode && self.fullscreen_idx == Some(idx) {
            let mut mask = self.composite_conceal_mask()?;
            let mut size = self.conceal_mask_size;
            if let Some(target) = source_size
                && target != size
            {
                mask = crate::mask_db::rescale_mask(&mask, size[0], size[1], target[0], target[1]);
                size = target;
            }
            return Some(DebugMask { size, mask });
        }
        let size = source_size.or_else(|| self.best_pre_adjustment_source(idx).map(|p| p.size))?;
        let key = self.page_path_key(idx)?;
        let db = self.conceal_db.as_ref()?;
        let (mut mask, shapes) = db.get_full(&key, size[0], size[1])?;
        crate::mask_db::rasterize_shapes_into(&mut mask, &shapes, size[0], size[1]);
        Some(DebugMask { size, mask })
    }
}

struct DebugMask {
    size: [usize; 2],
    mask: Vec<bool>,
}

impl PipelineDebugPageWork {
    fn push_image(&mut self, name: &str, note: &str, pixels: Arc<egui::ColorImage>) {
        self.stages.push(PipelineDebugStageWork::Image {
            name: name.to_string(),
            note: note.to_string(),
            pixels,
        });
    }

    fn push_missing(&mut self, name: &str, reason: &str) {
        self.stages.push(PipelineDebugStageWork::Missing {
            name: name.to_string(),
            reason: reason.to_string(),
        });
    }
}

fn run_pipeline_debug_export(
    work: PipelineDebugExportWork,
) -> Result<PipelineDebugExportComplete, String> {
    std::fs::create_dir_all(&work.root_dir)
        .map_err(|err| format!("出力ディレクトリを作成できません: {err}"))?;

    let mut manifest_pages = Vec::new();
    let mut written_count = 0usize;

    for page in work.pages {
        let page_dir = work.root_dir.join(&page.dir_name);
        std::fs::create_dir_all(&page_dir)
            .map_err(|err| format!("ページ出力ディレクトリを作成できません: {err}"))?;
        let mut manifest_stages = Vec::new();

        for stage in page.stages {
            let saved = match stage {
                PipelineDebugStageWork::Image { name, note, pixels } => {
                    let file = format!("{name}.png");
                    save_color_image_png(&pixels, &page_dir.join(&file))?;
                    written_count += 1;
                    SavedStage {
                        name,
                        file: Some(format!("{}/{}", page.dir_name, file)),
                        size: Some(pixels.size),
                        note,
                        missing: false,
                    }
                }
                PipelineDebugStageWork::Adjustment {
                    name,
                    note,
                    source,
                    params,
                    include_post_filter,
                } => {
                    let adjusted = crate::adjustment::apply_adjustments_fast(&source, &params);
                    let colorized = if include_post_filter && params.colorize.is_enabled() {
                        crate::colorize::apply(&adjusted, &params.colorize)
                    } else {
                        adjusted
                    };
                    let output = if include_post_filter
                        && params.post_filter != crate::adjustment::PostFilter::None
                    {
                        crate::post_filter::apply(&colorized, params.post_filter)
                    } else {
                        colorized
                    };
                    let file = format!("{name}.png");
                    save_color_image_png(&output, &page_dir.join(&file))?;
                    written_count += 1;
                    SavedStage {
                        name,
                        file: Some(format!("{}/{}", page.dir_name, file)),
                        size: Some(output.size),
                        note,
                        missing: false,
                    }
                }
                PipelineDebugStageWork::EraseInput {
                    name,
                    note,
                    source,
                    params,
                    force_black_flatten,
                } => {
                    let source = if force_black_flatten || source.pixels.iter().any(|p| p.a() < 255)
                    {
                        App::black_flatten_if_transparent(&source)
                            .map(Arc::new)
                            .unwrap_or(source)
                    } else {
                        source
                    };
                    let adjusted = crate::adjustment::apply_adjustments_fast(&source, &params);
                    let colorized = if params.colorize.is_enabled() {
                        crate::colorize::apply(&adjusted, &params.colorize)
                    } else {
                        adjusted
                    };
                    let output = if params.post_filter != crate::adjustment::PostFilter::None {
                        crate::post_filter::apply(&colorized, params.post_filter)
                    } else {
                        colorized
                    };
                    let file = format!("{name}.png");
                    save_color_image_png(&output, &page_dir.join(&file))?;
                    written_count += 1;
                    SavedStage {
                        name,
                        file: Some(format!("{}/{}", page.dir_name, file)),
                        size: Some(output.size),
                        note,
                        missing: false,
                    }
                }
                PipelineDebugStageWork::LocalAdjustCompose {
                    name,
                    note,
                    source,
                    layers,
                } => {
                    let rgba = crate::capture::color_image_to_rgba(&source);
                    let src = local_adjust_core::RgbaImageRef {
                        width: source.size[0],
                        height: source.size[1],
                        pixels: &rgba,
                    };
                    match local_adjust_core::apply_layers(src, &layers) {
                        Ok(output) => {
                            let image = egui::ColorImage::from_rgba_unmultiplied(
                                [output.width, output.height],
                                &output.pixels,
                            );
                            let file = format!("{name}.png");
                            save_color_image_png(&image, &page_dir.join(&file))?;
                            written_count += 1;
                            SavedStage {
                                name,
                                file: Some(format!("{}/{}", page.dir_name, file)),
                                size: Some(image.size),
                                note,
                                missing: false,
                            }
                        }
                        Err(err) => SavedStage {
                            name,
                            file: None,
                            size: Some(source.size),
                            note: format!("{note}; local adjustment recompute failed: {err}"),
                            missing: true,
                        },
                    }
                }
                PipelineDebugStageWork::ConcealCompose {
                    name,
                    note,
                    base,
                    mask,
                    preset,
                } => {
                    if mask.len() != base.size[0] * base.size[1] {
                        SavedStage {
                            name,
                            file: None,
                            size: Some(base.size),
                            note: format!(
                                "{note}; skipped because mask len {} != {}",
                                mask.len(),
                                base.size[0] * base.size[1]
                            ),
                            missing: true,
                        }
                    } else if !mask.iter().any(|&b| b) {
                        SavedStage {
                            name,
                            file: None,
                            size: Some(base.size),
                            note: format!("{note}; skipped because mask is empty"),
                            missing: true,
                        }
                    } else {
                        let composed =
                            crate::conceal_compose::compose_with_preset(&base, &mask, &preset);
                        let file = format!("{name}.png");
                        save_color_image_png(&composed, &page_dir.join(&file))?;
                        written_count += 1;
                        SavedStage {
                            name,
                            file: Some(format!("{}/{}", page.dir_name, file)),
                            size: Some(composed.size),
                            note,
                            missing: false,
                        }
                    }
                }
                PipelineDebugStageWork::Missing { name, reason } => SavedStage {
                    name,
                    file: None,
                    size: None,
                    note: reason,
                    missing: true,
                },
            };
            manifest_stages.push(json!({
                "name": saved.name,
                "file": saved.file,
                "size": saved.size,
                "note": saved.note,
                "missing": saved.missing,
            }));
        }

        manifest_pages.push(json!({
            "idx": page.idx,
            "item": page.item,
                "path_key": page.path_key,
                "input_generation": page.input_generation,
                "erase_mask_generation": page.erase_mask_generation,
                "local_adjust_generation": page.local_adjust_generation,
                "conceal_mask_generation": page.conceal_mask_generation,
                "current_erase_key": {
                    "idx": page.current_erase_key.idx,
                    "input_gen": page.current_erase_key.input_gen,
                    "mask_gen": page.current_erase_key.mask_gen,
                },
                "current_local_adjust_key": {
                    "idx": page.current_local_adjust_key.idx,
                    "input_gen": page.current_local_adjust_key.input_gen,
                    "erase_mask_gen": page.current_local_adjust_key.erase_mask_gen,
                    "local_gen": page.current_local_adjust_key.local_gen,
                },
                "post_filter": page.post_filter,
                "notes": page.notes,
                "stages": manifest_stages,
        }));
    }

    let manifest = json!({
        "kind": "mimageviewer-pipeline-debug",
        "version": 1,
        "shortcut": "Ctrl+Alt+Shift+D",
        "created_unix_ms": work.created_unix_ms.to_string(),
        "fullscreen_idx": work.fullscreen_idx,
        "items_generation": work.items_generation,
        "spread_mode": work.spread_mode,
        "post_filter_bypassed": work.post_filter_bypassed,
        "stage_count": written_count,
        "pages": manifest_pages,
    });
    let manifest_path = work.root_dir.join("manifest.json");
    let json = serde_json::to_string_pretty(&manifest)
        .map_err(|err| format!("manifest JSON を作成できません: {err}"))?;
    std::fs::write(&manifest_path, json)
        .map_err(|err| format!("manifest を書き込めません: {err}"))?;

    Ok(PipelineDebugExportComplete {
        dir: work.root_dir,
        manifest_path,
        stage_count: written_count,
    })
}

fn save_color_image_png(img: &egui::ColorImage, path: &Path) -> Result<(), String> {
    let [w, h] = img.size;
    if w == 0 || h == 0 {
        return Err(format!("画像サイズが 0 です: {}", path.display()));
    }
    let width = u32::try_from(w).map_err(|_| format!("画像幅が大きすぎます: {w}"))?;
    let height = u32::try_from(h).map_err(|_| format!("画像高さが大きすぎます: {h}"))?;
    let rgba = crate::capture::color_image_to_rgba(img);
    let file = std::fs::File::create(path)
        .map_err(|err| format!("PNG を作成できません {}: {err}", path.display()))?;
    let mut writer = std::io::BufWriter::new(file);
    image::codecs::png::PngEncoder::new(&mut writer)
        .write_image(&rgba, width, height, image::ColorType::Rgba8.into())
        .map_err(|err| format!("PNG を書き込めません {}: {err}", path.display()))
}

fn mask_to_color_image(size: [usize; 2], mask: &[bool]) -> egui::ColorImage {
    let [w, h] = size;
    let expected = w.saturating_mul(h);
    let mut pixels = vec![egui::Color32::BLACK; expected];
    for (i, &masked) in mask.iter().take(expected).enumerate() {
        if masked {
            pixels[i] = egui::Color32::WHITE;
        }
    }
    egui::ColorImage::new(size, pixels)
}

fn unix_ms_now() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mask_to_color_image_uses_black_and_white() {
        let img = mask_to_color_image([2, 1], &[false, true]);
        assert_eq!(img.pixels[0], egui::Color32::BLACK);
        assert_eq!(img.pixels[1], egui::Color32::WHITE);
    }

    #[test]
    fn save_color_image_png_writes_file() {
        let tmp = tempfile::TempDir::new().expect("tempdir");
        let path = tmp.path().join("stage.png");
        let img = egui::ColorImage::new(
            [1, 1],
            vec![egui::Color32::from_rgba_unmultiplied(1, 2, 3, 255)],
        );
        save_color_image_png(&img, &path).expect("save png");
        assert!(path.exists());
        let decoded = image::open(path).expect("decode").to_rgba8();
        assert_eq!(decoded.get_pixel(0, 0).0, [1, 2, 3, 255]);
    }
}

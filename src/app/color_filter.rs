use super::*;

struct ColorScanWorkItem {
    key: crate::color_search::ColorPaletteKey,
    req: LoadRequest,
    pin_source: Option<crate::folder_thumb_pins::FolderPinSource>,
}

const COLOR_SCAN_CONFIRM_MISSING_THRESHOLD: usize = 2_000;
const MAX_COLOR_SCAN_MESSAGES_PER_FRAME: usize = 256;

fn color_filter_item_supported(item: &crate::grid_item::GridItem) -> bool {
    item.has_page_data()
        || matches!(item, crate::grid_item::GridItem::Stack { .. })
        || color_filter_representative_supported(item)
}

fn color_filter_representative_supported(item: &GridItem) -> bool {
    match item {
        GridItem::ZipFile(_) => true,
        // PdfFile also represents converted EPUBs; do not expand the requested scope.
        GridItem::PdfFile(path) => path
            .extension()
            .is_some_and(|ext| ext.eq_ignore_ascii_case("pdf")),
        _ => false,
    }
}

fn color_representative_key(
    item: &GridItem,
    storage_key: &str,
    pin: Option<&crate::folder_thumb_pins::FolderPinSource>,
    pin_stamp: Option<crate::folder_thumb_pins::FolderPinMutationStamp>,
    depth: u32,
    revision: u64,
) -> String {
    // The root selection plus DB revision identifies cascade selections without UI-thread
    // stat/DB lookups. The worker resolves the canonical #pin: storage suffix as usual.
    format!(
        "{}|representative:{storage_key:?}#pin:{pin:?}|cascade:{:?}:{depth}|thumb:{revision}",
        item.perf_key(),
        pin.and(pin_stamp)
    )
}

fn color_scan_cache_decision(req: &LoadRequest, cache_decision: CacheDecision) -> CacheDecision {
    if req.pdf_page.is_none() || color_representative_request(req) {
        return cache_decision;
    }
    CacheDecision {
        policy: crate::settings::CachePolicy::Off,
        threshold_ms: cache_decision.threshold_ms,
        size_threshold: cache_decision.size_threshold,
        webp_always: false,
        pdf_always: false,
        zip_always: false,
    }
}

fn color_representative_request(req: &LoadRequest) -> bool {
    req.cache_key_override
        .as_deref()
        .is_some_and(|key| key.starts_with(CACHE_KEY_ZIP) || key.starts_with(CACHE_KEY_PDF))
}

/// Only parent ZIP/PDF representatives consult catalog on a fresh memory-cache miss.
/// Use a private one-row map so a delayed DB read cannot overwrite a newer grid cache row.
fn color_representative_catalog_cache(
    req: &LoadRequest,
    cache_map: &std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
    catalog: Option<&Arc<crate::catalog::CatalogDb>>,
) -> Option<Arc<std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>>> {
    if !color_representative_request(req) {
        return None;
    }
    let key = crate::thumb_loader::cache_key_for_request(req)?;
    let fresh = |entry: &crate::catalog::CacheEntry| {
        entry.mtime == req.mtime && entry.file_size == req.file_size
    };
    if cache_map
        .read()
        .ok()
        .is_some_and(|map| map.get(key.as_ref()).is_some_and(fresh))
    {
        return None;
    }
    let entry = match catalog?.load_one(key.as_ref()) {
        Ok(entry) => entry.filter(fresh)?,
        Err(error) => {
            crate::logger::log(format!(
                "color_scan: representative catalog read failed: {error}"
            ));
            return None;
        }
    };
    Some(Arc::new(std::sync::RwLock::new(
        std::collections::HashMap::from([(key.into_owned(), entry)]),
    )))
}

impl App {
    pub(crate) fn invalidate_color_representative(&mut self, idx: usize) {
        // Color filtering belongs to the main grid, outside the swapped viewer payload.
        // A thumbnail completion/reload in another owner must not mutate that grid's scan.
        if self.projected_viewer_context_id() != self.viewer_context_main() {
            return;
        }
        let Some(item) = self
            .items
            .get(idx)
            .filter(|item| color_filter_representative_supported(item))
        else {
            return;
        };
        let revision = self
            .color_filter
            .palettes
            .representative_revisions
            .entry(item.perf_key())
            .or_default();
        *revision = revision.wrapping_add(1);
        self.mark_color_filter_scope_dirty();
    }

    pub(crate) fn color_filter_available_in_current_view(&self) -> bool {
        !self.items_are_drive_list
            && !self.items_are_reading_history_view
            && !self.items_are_global_search_view
            && !self.items_are_tag_view
            && !self.items_are_rating_view
            && !self.favsearch.on_results_grid()
            && !self.tag_view.on_results_grid()
    }

    pub(crate) fn clear_color_filter_for_new_items(&mut self) {
        self.color_filter.clear_for_new_items();
        self.color_filter_scope_refresh_pending = false;
    }

    pub(crate) fn refresh_color_filter_for_scope_change(&mut self, ctx: &egui::Context) {
        self.color_filter_scope_refresh_pending = false;
        if self.color_filter.enabled {
            self.color_filter.applied_scope_signature = None;
            self.color_filter.confirmation = None;
            self.color_filter.confirmed_large_scan_scope = None;
            self.ensure_color_scan_for_current_scope(ctx);
        }
    }

    pub(crate) fn mark_color_filter_scope_dirty(&mut self) {
        if !self.color_filter.enabled {
            return;
        }
        self.color_filter.applied_scope_signature = None;
        self.color_filter.confirmation = None;
        self.color_filter.confirmed_large_scan_scope = None;
        self.color_filter_scope_refresh_pending = true;
    }

    pub(crate) fn confirm_large_color_scan(&mut self, ctx: &egui::Context) {
        let Some(confirmation) = self.color_filter.confirmation.take() else {
            return;
        };
        self.color_filter.confirmed_large_scan_scope = Some(confirmation.scope_signature);
        self.ensure_color_scan_for_current_scope(ctx);
    }

    pub(crate) fn cancel_large_color_scan_confirmation(&mut self) {
        self.color_filter.confirmation = None;
        self.color_filter.confirmed_large_scan_scope = None;
        self.color_filter.enabled = false;
        self.color_filter.applied_scope_signature = None;
        self.color_filter_scope_refresh_pending = false;
        self.rebuild_visible_indices();
    }

    pub(crate) fn apply_image_color_filter_from_swatch(
        &mut self,
        rgb: [u8; 3],
        ctx: &egui::Context,
    ) {
        // 集約ビュー (タグ / 全文検索 / お気に入り検索 / 閲覧履歴 / レーティング一覧 /
        // ドライブ一覧) では
        // 画像色フィルタは未対応。スウォッチから有効化しても何も絞れず、不活性な
        // チップだけ残るので、案内トーストを出して有効化しない。
        if !self.color_filter_available_in_current_view() {
            self.show_feedback_toast(
                "このビューでは画像色フィルタを使えません (通常フォルダ / サブ展開 / ZIP / PDF で利用できます)"
                    .to_string(),
            );
            return;
        }
        self.color_filter.set_query_rgb(rgb);
        self.color_filter.enabled = true;
        self.color_filter.applied_scope_signature = None;
        self.color_filter_scope_refresh_pending = false;
        self.ensure_color_scan_for_current_scope(ctx);
        self.show_feedback_toast(format!("[画像色 {}]", crate::color_search::hex_rgb(rgb)));
    }

    pub(crate) fn current_fullscreen_color_palette(
        &mut self,
    ) -> Option<crate::color_search::Palette> {
        let idx = self.fullscreen_idx?;
        let (key, mtime, file_size) = self.color_identity_for_idx(idx)?;
        if let Some(entry) = self
            .color_filter
            .palettes
            .fresh_entry(&key, mtime, file_size)
        {
            return Some(entry.palette.clone());
        }

        let pixels = match self.fs_cache.get(&idx) {
            Some(crate::fs_animation::FsCacheEntry::Static { pixels, .. }) => Arc::clone(pixels),
            _ => return None,
        };

        let t0 = std::time::Instant::now();
        let palette = crate::color_search::extract_palette_from_color_image(&pixels);
        if crate::perf::is_enabled() {
            crate::perf::event(
                "color",
                "fullscreen_palette",
                Some(&key),
                0,
                &[
                    (
                        "ms",
                        serde_json::Value::from(t0.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("colors", serde_json::Value::from(palette.colors.len())),
                    (
                        "pixels",
                        serde_json::Value::from(
                            pixels.size[0].saturating_mul(pixels.size[1]) as u64
                        ),
                    ),
                ],
            );
        }
        self.color_filter.palettes.insert(
            key,
            crate::color_search::PaletteEntry {
                mtime,
                file_size,
                palette: palette.clone(),
            },
        );
        Some(palette)
    }

    pub(crate) fn poll_color_scan(&mut self, ctx: &egui::Context) {
        if self.color_filter_scope_refresh_pending {
            self.color_filter_scope_refresh_pending = false;
            if self.color_filter.enabled {
                self.ensure_color_scan_for_current_scope(ctx);
            }
        }

        let Some(mut pending) = self.color_filter.pending.take() else {
            return;
        };

        let mut finished = None;
        let mut disconnected = false;
        let mut reached_frame_limit = true;
        for _ in 0..MAX_COLOR_SCAN_MESSAGES_PER_FRAME {
            match pending.rx.try_recv() {
                Ok(crate::color_search::ColorScanMessage::Item(item)) => {
                    pending.done = pending.done.saturating_add(1);
                    self.color_filter.palettes.insert(
                        item.key,
                        crate::color_search::PaletteEntry {
                            mtime: item.mtime,
                            file_size: item.file_size,
                            palette: item.palette,
                        },
                    );
                }
                Ok(crate::color_search::ColorScanMessage::Done {
                    scan_id,
                    scope_signature,
                    cancelled,
                }) => {
                    finished = Some((scan_id, scope_signature, cancelled));
                }
                Err(mpsc::TryRecvError::Empty) => {
                    reached_frame_limit = false;
                    break;
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    disconnected = true;
                    reached_frame_limit = false;
                    break;
                }
            }
        }
        let hit_frame_limit = reached_frame_limit && finished.is_none() && !disconnected;

        if let Some((scan_id, scope_signature, cancelled)) = finished {
            let elapsed_ms = pending.started_at.elapsed().as_secs_f64() * 1000.0;
            // current_scope は Drop 以外でのみ必要 (O(N) なので Drop 時は算出しない)。
            let current_scope = if cancelled || !self.color_filter.enabled {
                None
            } else {
                self.color_current_scope_signature()
            };
            let disposition = crate::color_search::scan_result_disposition(
                cancelled,
                self.color_filter.enabled,
                scan_id,
                self.color_filter.palettes.active_scan_id,
                scope_signature,
                current_scope,
            );
            match disposition {
                crate::color_search::ScanDisposition::Drop => {
                    self.color_filter.applied_scope_signature = None;
                    self.rebuild_visible_indices();
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "color",
                            "scan_cancelled",
                            None,
                            0,
                            &[
                                ("scan_id", serde_json::Value::from(scan_id)),
                                ("done", serde_json::Value::from(pending.done)),
                                ("total", serde_json::Value::from(pending.total)),
                                ("ms", serde_json::Value::from(elapsed_ms)),
                            ],
                        );
                    }
                    return;
                }
                crate::color_search::ScanDisposition::Apply => {
                    self.color_filter.applied_scope_signature = Some(scope_signature);
                    self.color_filter.palettes.last_scope_signature = Some(scope_signature);
                    self.rebuild_visible_indices();
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "color",
                            "scan_applied",
                            None,
                            0,
                            &[
                                ("scan_id", serde_json::Value::from(scan_id)),
                                ("done", serde_json::Value::from(pending.done)),
                                ("total", serde_json::Value::from(pending.total)),
                                ("ms", serde_json::Value::from(elapsed_ms)),
                            ],
                        );
                    }
                }
                crate::color_search::ScanDisposition::Restart => {
                    if crate::perf::is_enabled() {
                        crate::perf::event(
                            "color",
                            "scan_stale_scope",
                            None,
                            0,
                            &[
                                ("scan_id", serde_json::Value::from(scan_id)),
                                ("done", serde_json::Value::from(pending.done)),
                                ("total", serde_json::Value::from(pending.total)),
                                ("ms", serde_json::Value::from(elapsed_ms)),
                            ],
                        );
                    }
                    self.ensure_color_scan_for_current_scope(ctx);
                }
            }
            ctx.request_repaint();
        } else if disconnected {
            self.color_filter.applied_scope_signature = None;
            self.rebuild_visible_indices();
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "color",
                    "scan_disconnected",
                    None,
                    0,
                    &[
                        ("done", serde_json::Value::from(pending.done)),
                        ("total", serde_json::Value::from(pending.total)),
                        (
                            "ms",
                            serde_json::Value::from(
                                pending.started_at.elapsed().as_secs_f64() * 1000.0,
                            ),
                        ),
                    ],
                );
            }
        } else if hit_frame_limit {
            ctx.request_repaint();
            self.color_filter.pending = Some(pending);
        } else {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
            self.color_filter.pending = Some(pending);
        }
    }

    pub(crate) fn ensure_color_scan_for_current_scope(&mut self, ctx: &egui::Context) {
        if !self.color_filter.enabled {
            return;
        }
        if !self.color_filter_available_in_current_view() {
            self.color_filter.cancel_pending();
            self.color_filter.confirmation = None;
            self.color_filter.confirmed_large_scan_scope = None;
            self.color_filter.applied_scope_signature = None;
            self.rebuild_visible_indices();
            return;
        }
        let Some(scope_signature) = self.color_current_scope_signature() else {
            self.color_filter.confirmation = None;
            self.color_filter.confirmed_large_scan_scope = None;
            self.color_filter.applied_scope_signature = None;
            self.rebuild_visible_indices();
            return;
        };

        if self
            .color_filter
            .pending
            .as_ref()
            .is_some_and(|pending| pending.scope_signature == scope_signature)
        {
            return;
        }

        self.color_filter.cancel_pending();
        let work_items = self.color_missing_work_items();
        if work_items.is_empty() {
            self.color_filter.confirmation = None;
            self.color_filter.confirmed_large_scan_scope = None;
            self.color_filter.applied_scope_signature = Some(scope_signature);
            self.color_filter.palettes.last_scope_signature = Some(scope_signature);
            if crate::perf::is_enabled() {
                crate::perf::event(
                    "color",
                    "scan_reuse_all",
                    None,
                    0,
                    &[("scope", serde_json::Value::from(scope_signature))],
                );
            }
            self.rebuild_visible_indices();
            return;
        }
        if work_items.len() >= COLOR_SCAN_CONFIRM_MISSING_THRESHOLD
            && self.color_filter.confirmed_large_scan_scope != Some(scope_signature)
        {
            let same_confirmation =
                self.color_filter
                    .confirmation
                    .as_ref()
                    .is_some_and(|confirmation| {
                        confirmation.scope_signature == scope_signature
                            && confirmation.missing == work_items.len()
                    });
            self.color_filter.confirmation = Some(crate::color_search::ColorScanConfirmation {
                scope_signature,
                missing: work_items.len(),
            });
            self.color_filter.applied_scope_signature = None;
            self.rebuild_visible_indices();
            if !same_confirmation {
                self.show_feedback_toast(format!(
                    "画像色: {} 件のスキャン確認が必要です (画像色メニュー)",
                    work_items.len()
                ));
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "color",
                        "scan_confirmation",
                        None,
                        0,
                        &[
                            ("missing", serde_json::Value::from(work_items.len())),
                            ("scope", serde_json::Value::from(scope_signature)),
                        ],
                    );
                }
            }
            ctx.request_repaint();
            return;
        }
        self.color_filter.confirmation = None;
        self.color_filter.confirmed_large_scan_scope = None;

        let Some(cache_map) = self.current_color_cache_map.as_ref().cloned() else {
            self.color_filter.applied_scope_signature = None;
            self.rebuild_visible_indices();
            return;
        };

        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let scan_id = self.color_filter.palettes.active_scan_id.wrapping_add(1);
        self.color_filter.palettes.active_scan_id = scan_id;
        let pending = crate::color_search::ColorScanPending {
            scan_id,
            scope_signature,
            total: work_items.len(),
            done: 0,
            cancel: Arc::clone(&cancel),
            rx,
            started_at: std::time::Instant::now(),
        };
        let catalog = self.current_color_catalog.clone();
        let thumb_px = self.settings.thumb_px;
        let thumb_quality = self.settings.thumb_quality;
        let cache_decision = CacheDecision::from_settings(&self.settings);
        let stats = Arc::clone(&self.stats);
        let pin_db = self.folder_thumb_pin_db.clone();
        // サムネ生成と同じ並列度設定に従う (Auto = cores/2)。デコードを並列化しつつ
        // I/O 競合を抑える。
        let threads = self.settings.parallelism.thread_count();
        let total = pending.total;

        if crate::perf::is_enabled() {
            crate::perf::event(
                "color",
                "scan_start",
                None,
                0,
                &[
                    ("scan_id", serde_json::Value::from(scan_id)),
                    ("total", serde_json::Value::from(total)),
                    ("scope", serde_json::Value::from(scope_signature)),
                ],
            );
        }

        match std::thread::Builder::new()
            .name("color-scan".to_string())
            .spawn(move || {
                run_color_scan_worker(
                    scan_id,
                    scope_signature,
                    work_items,
                    cache_map,
                    catalog,
                    thumb_px,
                    thumb_quality,
                    cache_decision,
                    stats,
                    pin_db,
                    threads,
                    cancel,
                    tx,
                );
            }) {
            Ok(_) => {
                self.color_filter.pending = Some(pending);
                ctx.request_repaint();
            }
            Err(e) => {
                crate::logger::log(format!("color_scan: failed to spawn worker: {e}"));
                self.color_filter.applied_scope_signature = None;
                self.rebuild_visible_indices();
                if crate::perf::is_enabled() {
                    crate::perf::event(
                        "color",
                        "scan_spawn_failed",
                        None,
                        0,
                        &[("total", serde_json::Value::from(total))],
                    );
                }
            }
        }
    }

    pub(crate) fn passes_color_filter_for_scope(
        &self,
        idx: usize,
        scope_signature: Option<crate::color_search::ScanScopeSignature>,
    ) -> bool {
        if !self.color_filter.enabled {
            return true;
        }
        let Some(scope_signature) = scope_signature else {
            return true;
        };
        if self.color_filter.applied_scope_signature != Some(scope_signature) {
            return true;
        }
        let Some((key, mtime, file_size)) = self.color_identity_for_idx(idx) else {
            return false;
        };
        let Some(entry) = self
            .color_filter
            .palettes
            .fresh_entry(&key, mtime, file_size)
        else {
            return false;
        };
        crate::color_search::palette_matches(
            &entry.palette,
            self.color_filter.query_lab(),
            self.color_filter.tolerance,
        )
    }

    pub(crate) fn color_current_scope_signature(
        &mut self,
    ) -> Option<crate::color_search::ScanScopeSignature> {
        let indices = self.color_scan_candidate_indices();
        if indices.is_empty() {
            return None;
        }
        let mut parts = Vec::with_capacity(indices.len());
        for idx in indices {
            let item = self.items.get(idx)?;
            let key = self
                .color_identity_for_idx(idx)
                .map(|identity| identity.0)
                .unwrap_or_else(|| item.perf_key());
            let (mtime, file_size) = self
                .image_metas
                .get(idx)
                .copied()
                .flatten()
                .unwrap_or((0, 0));
            parts.push((key, mtime, file_size));
        }
        Some(crate::color_search::scan_scope_signature(
            self.color_view_kind(),
            parts
                .iter()
                .map(|(key, mtime, file_size)| (key.as_str(), *mtime, *file_size)),
        ))
    }

    fn color_scan_candidate_indices(&mut self) -> Vec<usize> {
        let search_filter = self.search_filter.clone();
        let rating_filter = self.effective_rating_filter();
        let rating_filter_active = !self.items_are_drive_list
            && !self.items_are_reading_history_view
            && !rating_filter.iter().all(|&b| b);
        let mut out = Vec::new();
        for i in 0..self.items.len() {
            if !self.smart_folder_rule_qualifies_index(i) {
                continue;
            }
            if !self.items.get(i).is_some_and(color_filter_item_supported) {
                continue;
            }
            if let Some(ref f) = search_filter
                && !f.contains(&i)
            {
                continue;
            }
            if rating_filter_active {
                let stars = self.get_rating(i);
                if let Some(item) = self.items.get(i)
                    && !passes_rating_filter(item, stars, &rating_filter)
                {
                    continue;
                }
            }
            if !self.items_are_reading_history_view && !self.passes_facet_filter(i, None) {
                continue;
            }
            out.push(i);
        }
        out
    }

    fn color_missing_work_items(&mut self) -> Vec<ColorScanWorkItem> {
        let indices = self.color_scan_candidate_indices();
        let mut work = Vec::new();
        for idx in indices {
            let Some((key, req)) = self.color_work_identity_for_idx(idx) else {
                continue;
            };
            if self
                .color_filter
                .palettes
                .fresh_entry(&key, req.mtime, req.file_size)
                .is_some()
            {
                continue;
            }
            let pin_source = self
                .items
                .get(idx)
                .filter(|item| color_filter_representative_supported(item))
                .and_then(GridItem::container_path)
                .and_then(|path| {
                    self.folder_pin_map
                        .get(&crate::path_key::normalize_keep_drive(path))
                })
                .cloned();
            work.push(ColorScanWorkItem {
                key,
                req,
                pin_source,
            });
        }
        work
    }

    fn color_identity_for_idx(
        &self,
        idx: usize,
    ) -> Option<(crate::color_search::ColorPaletteKey, i64, i64)> {
        let item = self.items.get(idx)?;
        if !color_filter_item_supported(item) {
            return None;
        }
        let (mtime, file_size) = self.image_metas.get(idx).copied().flatten()?;
        let key = if color_filter_representative_supported(item) {
            let storage_key = container_cache_base_key(
                item,
                self.use_full_path_cache_keys(),
                Some(self.settings.folder_thumb_sort),
                self.settings.folder_thumb_depth,
            )?;
            let pin = item.container_path().and_then(|path| {
                self.folder_pin_map
                    .get(&crate::path_key::normalize_keep_drive(path))
            });
            color_representative_key(
                item,
                &storage_key,
                pin,
                self.folder_thumb_pin_db
                    .as_ref()
                    .map(|db| db.mutation_stamp()),
                self.settings.folder_thumb_depth,
                self.color_filter
                    .palettes
                    .representative_revisions
                    .get(&item.perf_key())
                    .copied()
                    .unwrap_or(0),
            )
        } else {
            item.perf_key()
        };
        Some((key, mtime, file_size))
    }

    fn color_work_identity_for_idx(
        &self,
        idx: usize,
    ) -> Option<(crate::color_search::ColorPaletteKey, LoadRequest)> {
        let item = self.items.get(idx)?;
        if !color_filter_item_supported(item) {
            return None;
        }
        let (mtime, file_size) = self.image_metas.get(idx).copied().flatten()?;
        let empty_pins = std::collections::HashMap::new();
        let mut req = make_load_request(
            item,
            idx,
            mtime,
            file_size,
            false,
            self.pdf_current_password.as_deref(),
            Some(self.settings.folder_thumb_sort),
            self.settings.folder_thumb_depth,
            if color_filter_representative_supported(item) {
                &empty_pins
            } else {
                &self.folder_pin_map
            },
            &self.converted_archive_cache_paths,
            self.archive_source_override.as_deref(),
            self.current_folder.as_deref(),
            self.folder_thumb_pin_db.as_deref(),
            self.video_pin_db.as_ref(),
            self.use_full_path_cache_keys(),
        )?;
        req.relative_page_provenance = self.relative_page_provenance_for_idx(idx);
        req.context_epoch = crate::pdf_loader::current_render_context_epoch();
        req.input_seq = self.input_seq;
        Some((self.color_identity_for_idx(idx)?.0, req))
    }

    fn color_view_kind(&self) -> &'static str {
        if self.items_are_global_search_view {
            "global_search"
        } else if self.items_are_tag_view {
            "tag"
        } else if self.items_are_subfolder_expansion_view {
            "subfolder_expansion"
        } else if self.favsearch.on_results_grid() {
            "favsearch"
        } else if self.items_are_reading_history_view {
            "reading_history"
        } else {
            "folder"
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn run_color_scan_worker(
    scan_id: u64,
    scope_signature: crate::color_search::ScanScopeSignature,
    work_items: Vec<ColorScanWorkItem>,
    cache_map: Arc<
        std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
    >,
    catalog: Option<Arc<crate::catalog::CatalogDb>>,
    thumb_px: u32,
    thumb_quality: u8,
    cache_decision: CacheDecision,
    stats: Arc<Mutex<crate::stats::ThumbStats>>,
    pin_db: Option<Arc<crate::folder_thumb_pins::FolderThumbPinDb>>,
    threads: usize,
    cancel: Arc<AtomicBool>,
    tx: mpsc::Sender<crate::color_search::ColorScanMessage>,
) {
    let done = Arc::new(AtomicUsize::new(0));
    let keep_start = Arc::new(AtomicUsize::new(0));
    let keep_end = Arc::new(AtomicUsize::new(usize::MAX));

    // 1 件ぶんの処理。`cache_map` / `catalog` (Mutex<Connection>) / `stats` はいずれも
    // 内部同期されているので複数スレッドから安全に共有できる。`tx` は for_each_with の
    // per-thread clone で渡す (Sender は !Sync のため)。
    let process = |tx: &mut mpsc::Sender<crate::color_search::ColorScanMessage>,
                   mut item: ColorScanWorkItem| {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let mtime = item.req.mtime;
        let file_size = item.req.file_size;
        if let Some(source) = item.pin_source {
            let container = item.req.path.clone();
            let base_key = item
                .req
                .cache_key_override
                .clone()
                .expect("representative cache key");
            let kind = if base_key.starts_with(CACHE_KEY_PDF) {
                ContainerKindForPin::PdfFile
            } else {
                ContainerKindForPin::ZipFile
            };
            let pins = std::collections::HashMap::from([(
                crate::path_key::normalize_keep_drive(&container),
                source,
            )]);
            let password = item.req.pdf_password.clone();
            item.req = apply_folder_thumb_pin(
                item.req,
                &container,
                &base_key,
                false,
                kind,
                &pins,
                &std::collections::HashMap::new(),
                pin_db.as_deref(),
                None,
                password.as_deref(),
            );
        }
        let palette = load_palette_for_request(
            item.req,
            &cache_map,
            catalog.as_ref(),
            thumb_px,
            thumb_quality,
            cache_decision,
            &done,
            &stats,
            &cancel,
            &keep_start,
            &keep_end,
            pin_db.as_deref(),
        )
        .unwrap_or_default();
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let _ = tx.send(crate::color_search::ColorScanMessage::Item(
            crate::color_search::ColorScanItemResult {
                key: item.key,
                mtime,
                file_size,
                palette,
            },
        ));
    };

    let pool = (threads > 1)
        .then(|| {
            rayon::ThreadPoolBuilder::new()
                .num_threads(threads)
                .build()
                .ok()
        })
        .flatten();

    match pool {
        Some(pool) => {
            use rayon::prelude::*;
            // Sender は !Sync なので install クロージャ内で clone せず、事前に clone して move で渡す。
            let tx_pool = tx.clone();
            // pool.install で par_iter を専用プール上で動かす (グローバル rayon プールを
            // 占有してサムネ/補正処理を止めないため)。install は全タスク完了まで block するので、
            // 抜けた時点で Item は全て tx に積まれている (= Done より前)。
            pool.install(move || {
                work_items
                    .into_par_iter()
                    .for_each_with(tx_pool, |tx, item| process(tx, item));
            });
        }
        None => {
            // 並列度 1 / プール生成失敗時は逐次実行にフォールバック。
            let mut tx_seq = tx.clone();
            for item in work_items {
                if cancel.load(Ordering::Relaxed) {
                    break;
                }
                process(&mut tx_seq, item);
            }
        }
    }

    let _ = tx.send(crate::color_search::ColorScanMessage::Done {
        scan_id,
        scope_signature,
        cancelled: cancel.load(Ordering::Relaxed),
    });
}

#[allow(clippy::too_many_arguments)]
fn load_palette_for_request(
    mut req: LoadRequest,
    cache_map: &Arc<
        std::sync::RwLock<std::collections::HashMap<String, crate::catalog::CacheEntry>>,
    >,
    catalog: Option<&Arc<crate::catalog::CatalogDb>>,
    thumb_px: u32,
    thumb_quality: u8,
    cache_decision: CacheDecision,
    done: &Arc<AtomicUsize>,
    stats: &Arc<Mutex<crate::stats::ThumbStats>>,
    cancel: &Arc<AtomicBool>,
    keep_start: &Arc<AtomicUsize>,
    keep_end: &Arc<AtomicUsize>,
    pin_db: Option<&crate::folder_thumb_pins::FolderThumbPinDb>,
) -> Option<crate::color_search::Palette> {
    if cancel.load(Ordering::Relaxed) {
        return None;
    }
    let (tx, rx) = mpsc::channel();
    req.priority = false;
    let cache_decision = color_scan_cache_decision(&req, cache_decision);
    if req.pdf_page.is_some() && cache_decision.policy == crate::settings::CachePolicy::Off {
        req.force_cache = false;
    }
    // Pin resolution has already supplied the canonical #pin: key. This lookup stays on
    // the color worker and leaves ordinary images' released cache_map -> decode path intact.
    let catalog_cache = color_representative_catalog_cache(&req, cache_map, catalog);
    crate::thumb_loader::process_load_request(
        &req,
        catalog_cache.as_ref().unwrap_or(cache_map),
        &tx,
        catalog,
        thumb_px,
        thumb_quality,
        128,
        cache_decision,
        done,
        stats,
        Some(cancel),
        keep_start,
        keep_end,
        None,
        pin_db,
        None,
        None,
    );

    let mut image = None;
    while let Ok(msg) = rx.try_recv() {
        if msg.canceled {
            return None;
        }
        if image.is_none() {
            image = msg.image;
        }
    }
    image
        .as_ref()
        .map(crate::color_search::extract_palette_from_color_image)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};

    fn always_cache_decision() -> CacheDecision {
        CacheDecision {
            policy: crate::settings::CachePolicy::Always,
            threshold_ms: 0,
            size_threshold: 0,
            webp_always: true,
            pdf_always: true,
            zip_always: true,
        }
    }

    fn representative_app() -> AppTestEnvForTest {
        let mut env = setup_app_for_test();
        let root = env.tmp.path().to_path_buf();
        env.items = vec![
            GridItem::ZipFile(root.join("book.zip")),
            GridItem::PdfFile(root.join("book.PDF")),
        ];
        env.image_metas = vec![Some((10, 20)); 2];
        env
    }

    #[test]
    fn color_representatives_share_missing_confirmation_and_pending_visibility() {
        let mut env = representative_app();
        let root = env.tmp.path().to_path_buf();
        env.items = (0..COLOR_SCAN_CONFIRM_MISSING_THRESHOLD)
            .map(|i| {
                if i % 2 == 0 {
                    GridItem::ZipFile(root.join(format!("{i}.zip")))
                } else {
                    GridItem::PdfFile(root.join(format!("{i}.pdf")))
                }
            })
            .collect();
        env.image_metas = vec![Some((10, 20)); env.items.len()];
        env.color_filter.enabled = true;
        let ctx = egui::Context::default();
        let scope = env.color_current_scope_signature();
        assert_eq!(
            env.color_missing_work_items().len(),
            COLOR_SCAN_CONFIRM_MISSING_THRESHOLD
        );
        env.ensure_color_scan_for_current_scope(&ctx);
        assert_eq!(
            env.color_filter.confirmation.as_ref().unwrap().missing,
            COLOR_SCAN_CONFIRM_MISSING_THRESHOLD
        );
        assert!(env.color_filter.pending.is_none());
        assert!(env.passes_color_filter_for_scope(0, scope));
        assert!(env.passes_color_filter_for_scope(1, scope));
        env.cancel_large_color_scan_confirmation();
        assert!(!env.color_filter.enabled);
    }

    #[test]
    fn color_representative_pin_reload_and_file_changes_reject_old_results() {
        let mut env = representative_app();
        let original_scope = env.color_current_scope_signature().unwrap();
        for idx in 0..2 {
            let (key, mtime, file_size) = env.color_identity_for_idx(idx).unwrap();
            env.color_filter.palettes.insert(
                key,
                crate::color_search::PaletteEntry {
                    mtime,
                    file_size,
                    palette: Default::default(),
                },
            );
        }
        assert!(env.color_missing_work_items().is_empty());
        env.invalidate_color_representative(0);
        assert_eq!(env.color_missing_work_items().len(), 1);
        let changed_scope = env.color_current_scope_signature().unwrap();
        assert_eq!(
            crate::color_search::scan_result_disposition(
                false,
                true,
                1,
                1,
                original_scope,
                Some(changed_scope)
            ),
            crate::color_search::ScanDisposition::Restart
        );
        let pdf_path = env.items[1].container_path().unwrap().to_path_buf();
        env.folder_pin_map.insert(
            crate::path_key::normalize_keep_drive(&pdf_path),
            crate::folder_thumb_pins::FolderPinSource::PdfPage {
                pdf_rel: String::new(),
                page: 3,
            },
        );
        assert_eq!(env.color_missing_work_items().len(), 2);
        // UI construction must not stat/open this nonexistent PDF, even when pinned.
        assert_eq!(
            env.color_work_identity_for_idx(1).unwrap().1.pdf_page,
            Some(0)
        );
        let pin_key = env.color_identity_for_idx(1).unwrap().0;
        assert!(pin_key.contains("#pin:"));
        env.folder_pin_map.clear();
        assert_ne!(env.color_identity_for_idx(1).unwrap().0, pin_key);
        assert_eq!(env.color_missing_work_items().len(), 1);
        env.image_metas[1] = Some((11, 20));
        assert_eq!(env.color_missing_work_items().len(), 2);
        env.image_metas[1] = Some((10, 21));
        assert_eq!(env.color_missing_work_items().len(), 2);
        env.clear_color_filter_for_new_items();
        assert!(
            env.color_filter
                .palettes
                .representative_revisions
                .is_empty()
        );
    }

    #[cfg(windows)]
    #[test]
    fn color_representative_invalidation_stays_with_main_grid_owner() {
        let mut env = representative_app();
        let items = env.items.clone();
        let sibling = env.build_window_context_for_test(29_010, |owner| {
            owner.items = items;
            owner.image_metas = vec![Some((10, 20)); 2];
        });
        env.color_filter.enabled = true;
        env.color_filter_scope_refresh_pending = false;
        let scope = env.color_current_scope_signature();
        env.with_viewer_context(sibling, |owner| {
            owner.invalidate_color_representative(0);
            assert!(
                owner
                    .color_filter
                    .palettes
                    .representative_revisions
                    .is_empty()
            );
            assert!(!owner.color_filter_scope_refresh_pending);
        })
        .unwrap();
        assert_eq!(env.color_current_scope_signature(), scope);
        env.invalidate_color_representative(0);
        assert_eq!(env.color_filter.palettes.representative_revisions.len(), 1);
        assert!(env.color_filter_scope_refresh_pending);
    }

    #[test]
    fn color_representative_cascade_pin_stamp_changes_identity_without_io() {
        let tmp = tempfile::tempdir().unwrap();
        let db = crate::folder_thumb_pins::FolderThumbPinDb::open_at(&tmp.path().join("pins.db"))
            .unwrap();
        let item = GridItem::ZipFile(tmp.path().join("book.zip"));
        let source = crate::folder_thumb_pins::FolderPinSource::ZipDir {
            zip_rel: String::new(),
            dir_prefix: "chapter/".into(),
        };
        let key = |pin, stamp| {
            color_representative_key(&item, "zipthumb:book.zip", pin, Some(stamp), 3, 0)
        };
        let before = db.mutation_stamp();
        let pinned = key(Some(&source), before);
        db.set(
            &tmp.path().join("book.zip/chapter"),
            &crate::folder_thumb_pins::FolderPinSource::ZipEntry {
                zip_rel: String::new(),
                entry: "chapter/b.png".into(),
            },
        )
        .unwrap();
        assert_ne!(pinned, key(Some(&source), db.mutation_stamp()));
        assert_eq!(key(None, before), key(None, db.mutation_stamp()));
    }

    fn scan_one(
        req: LoadRequest,
        pin_source: Option<crate::folder_thumb_pins::FolderPinSource>,
        entries: std::collections::HashMap<String, crate::catalog::CacheEntry>,
        catalog: Option<Arc<crate::catalog::CatalogDb>>,
    ) -> crate::color_search::Palette {
        let (tx, rx) = mpsc::channel();
        run_color_scan_worker(
            1,
            2,
            vec![ColorScanWorkItem {
                key: "test".into(),
                req,
                pin_source,
            }],
            Arc::new(std::sync::RwLock::new(entries)),
            catalog,
            128,
            75,
            always_cache_decision(),
            Arc::new(Mutex::new(crate::stats::ThumbStats::new())),
            None,
            1,
            Arc::new(AtomicBool::new(false)),
            tx,
        );
        let crate::color_search::ColorScanMessage::Item(result) = rx.recv().unwrap() else {
            panic!("item before done")
        };
        assert!(matches!(
            rx.recv().unwrap(),
            crate::color_search::ColorScanMessage::Done {
                cancelled: false,
                ..
            }
        ));
        result.palette
    }

    fn cache_entry(rgb: [u8; 3], mtime: i64, file_size: i64) -> crate::catalog::CacheEntry {
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([rgb[0], rgb[1], rgb[2], 255]),
        ));
        let (jpeg_data, _, _) = crate::catalog::encode_thumb_webp(&image, 8, 100.0).unwrap();
        crate::catalog::CacheEntry {
            mtime,
            file_size,
            jpeg_data,
            source_dims: Some((8, 8)),
            layout_dims: None,
            folder_provenance: None,
            selection_proof: None,
        }
    }

    #[test]
    fn color_catalog_fallback_does_not_change_normal_image_pixels() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("image.png");
        image::RgbaImage::from_pixel(8, 8, image::Rgba([255u8, 0, 0, 255]))
            .save(&path)
            .unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let req = LoadRequest {
            path,
            mtime: crate::ui_helpers::mtime_secs(&metadata),
            file_size: metadata.len() as i64,
            ..Default::default()
        };
        let db = Arc::new(crate::catalog::CatalogDb::open(tmp.path(), tmp.path()).unwrap());
        let green = cache_entry([0, 255, 0], req.mtime, req.file_size);
        db.save_thumb_bytes(
            "image.png",
            req.mtime,
            req.file_size,
            green.source_dims,
            &green.jpeg_data,
        )
        .unwrap();
        let palette = scan_one(req, None, Default::default(), Some(db));
        assert!(crate::color_search::palette_matches(
            &palette,
            crate::color_search::srgb_to_lab([255, 0, 0]),
            6.0
        ));
        assert!(!crate::color_search::palette_matches(
            &palette,
            crate::color_search::srgb_to_lab([0, 255, 0]),
            6.0
        ));
    }

    #[test]
    fn color_pdf_pinned_representative_reuses_exact_catalog_key_and_memory_wins() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("book.pdf");
        // Source contents must never be read: only the pinned page's cached thumbnail exists.
        std::fs::write(&path, b"not a PDF").unwrap();
        let metadata = std::fs::metadata(&path).unwrap();
        let req = LoadRequest {
            path: path.clone(),
            pdf_page: Some(0),
            cache_key_override: Some("pdfthumb:book.pdf".into()),
            mtime: crate::ui_helpers::mtime_secs(&metadata),
            file_size: metadata.len() as i64,
            ..Default::default()
        };
        let source = crate::folder_thumb_pins::FolderPinSource::PdfPage {
            pdf_rel: String::new(),
            page: 3,
        };
        let pins = std::collections::HashMap::from([(
            crate::path_key::normalize_keep_drive(&path),
            source.clone(),
        )]);
        let pinned = apply_folder_thumb_pin(
            req.clone(),
            &path,
            "pdfthumb:book.pdf",
            false,
            ContainerKindForPin::PdfFile,
            &pins,
            &Default::default(),
            None,
            None,
            None,
        );
        assert_eq!(pinned.pdf_page, Some(3));
        let key = crate::thumb_loader::cache_key_for_request(&pinned)
            .unwrap()
            .into_owned();
        assert!(key.contains("#pin:"));
        let db = Arc::new(crate::catalog::CatalogDb::open(tmp.path(), tmp.path()).unwrap());
        db.set_pdf_meta("book.pdf", req.mtime, req.file_size, 4, false)
            .unwrap();
        let blue = cache_entry([0, 0, 255], pinned.mtime, pinned.file_size);
        db.save_thumb_bytes(
            &key,
            pinned.mtime,
            pinned.file_size,
            blue.source_dims,
            &blue.jpeg_data,
        )
        .unwrap();
        let palette = scan_one(
            req.clone(),
            Some(source.clone()),
            Default::default(),
            Some(Arc::clone(&db)),
        );
        assert!(crate::color_search::palette_matches(
            &palette,
            crate::color_search::srgb_to_lab([0, 0, 255]),
            6.0
        ));
        let red = cache_entry([255, 0, 0], pinned.mtime, pinned.file_size);
        let memory = scan_one(
            req,
            Some(source),
            std::collections::HashMap::from([(key, red)]),
            Some(db),
        );
        assert!(crate::color_search::palette_matches(
            &memory,
            crate::color_search::srgb_to_lab([255, 0, 0]),
            6.0
        ));
        assert!(!crate::color_search::palette_matches(
            &memory,
            crate::color_search::srgb_to_lab([0, 0, 255]),
            6.0
        ));
    }

    #[test]
    fn color_pdf_representative_reads_memory_and_catalog_cache_and_preserves_failure_behavior() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("nonexistent.pdf");
        let req = LoadRequest {
            path,
            pdf_page: Some(0),
            cache_key_override: Some("pdfthumb:nonexistent.pdf".into()),
            mtime: 10,
            file_size: 20,
            ..Default::default()
        };
        let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([128, 24, 40, 255]),
        ));
        let (bytes, _, _) = crate::catalog::encode_thumb_webp(&image, 8, 100.0).unwrap();
        let entry = crate::catalog::CacheEntry {
            mtime: 10,
            file_size: 20,
            jpeg_data: bytes.clone(),
            source_dims: Some((8, 8)),
            layout_dims: None,
            folder_provenance: None,
            selection_proof: None,
        };
        let memory = scan_one(
            req.clone(),
            None,
            std::collections::HashMap::from([("pdfthumb:nonexistent.pdf".into(), entry)]),
            None,
        );
        assert!(crate::color_search::palette_matches(
            &memory,
            crate::color_search::srgb_to_lab([128, 24, 40]),
            6.0
        ));
        let db = Arc::new(crate::catalog::CatalogDb::open(tmp.path(), tmp.path()).unwrap());
        // Keep this headless cache test out of the PDF metadata catch-up queue.
        db.set_pdf_meta("nonexistent.pdf", 10, 20, 1, false)
            .unwrap();
        assert!(
            db.save_thumb_bytes("pdfthumb:nonexistent.pdf", 10, 20, Some((8, 8)), &bytes)
                .unwrap()
        );
        let catalog = scan_one(req, None, Default::default(), Some(db));
        assert!(crate::color_search::palette_matches(
            &catalog,
            crate::color_search::srgb_to_lab([128, 24, 40]),
            6.0
        ));
        let failed = scan_one(
            LoadRequest {
                path: tmp.path().join("missing.zip"),
                cache_key_override: Some("zipthumb:missing.zip".into()),
                ..Default::default()
            },
            None,
            Default::default(),
            None,
        );
        assert!(failed.colors.is_empty());
        assert!(!crate::color_search::palette_matches(
            &failed, [0.0; 3], 60.0
        ));
    }

    #[test]
    fn color_zip_representative_decodes_only_selected_cover_and_rejects_stale_cache() {
        use std::io::Write;
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("book.zip");
        let mut zip = zip::ZipWriter::new(std::fs::File::create(&path).unwrap());
        for (name, rgb) in [("a.png", [255, 0, 0]), ("b.png", [0, 0, 255])] {
            zip.start_file(name, zip::write::SimpleFileOptions::default())
                .unwrap();
            let image = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
                8,
                8,
                image::Rgba([rgb[0], rgb[1], rgb[2], 255]),
            ));
            let mut png = std::io::Cursor::new(Vec::new());
            image.write_to(&mut png, image::ImageFormat::Png).unwrap();
            zip.write_all(png.get_ref()).unwrap();
        }
        zip.finish().unwrap();
        let meta = std::fs::metadata(&path).unwrap();
        let req = LoadRequest {
            path,
            cache_key_override: Some("zipthumb:book.zip".into()),
            mtime: crate::ui_helpers::mtime_secs(&meta),
            file_size: meta.len() as i64,
            ..Default::default()
        };
        let green = image::DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            8,
            8,
            image::Rgba([0, 255, 0, 255]),
        ));
        let (bytes, _, _) = crate::catalog::encode_thumb_webp(&green, 8, 100.0).unwrap();
        let entry = crate::catalog::CacheEntry {
            mtime: req.mtime - 1,
            file_size: req.file_size,
            jpeg_data: bytes,
            source_dims: None,
            layout_dims: None,
            folder_provenance: None,
            selection_proof: None,
        };
        let db = Arc::new(crate::catalog::CatalogDb::open(tmp.path(), tmp.path()).unwrap());
        db.save_thumb_bytes(
            "zipthumb:book.zip",
            entry.mtime,
            entry.file_size,
            entry.source_dims,
            &entry.jpeg_data,
        )
        .unwrap();
        let cover = scan_one(
            req.clone(),
            None,
            std::collections::HashMap::from([("zipthumb:book.zip".into(), entry)]),
            Some(db),
        );
        let matches = |palette: &crate::color_search::Palette, rgb| {
            crate::color_search::palette_matches(
                palette,
                crate::color_search::srgb_to_lab(rgb),
                6.0,
            )
        };
        assert!(matches(&cover, [255, 0, 0]));
        assert!(!matches(&cover, [0, 0, 255]));
        let pinned = scan_one(
            req,
            Some(crate::folder_thumb_pins::FolderPinSource::ZipEntry {
                zip_rel: String::new(),
                entry: "b.png".into(),
            }),
            Default::default(),
            None,
        );
        assert!(matches(&pinned, [0, 0, 255]));
        assert!(!matches(&pinned, [255, 0, 0]));
    }

    #[test]
    fn color_filter_targets_images_stacks_and_zip_pdf_representatives() {
        assert!(color_filter_item_supported(
            &crate::grid_item::GridItem::Image(PathBuf::from("a.jpg"))
        ));
        assert!(color_filter_item_supported(
            &crate::grid_item::GridItem::ZipImage {
                zip_path: PathBuf::from("book.zip"),
                entry_name: "page.jpg".to_string(),
            }
        ));
        assert!(color_filter_item_supported(
            &crate::grid_item::GridItem::PdfPage {
                pdf_path: PathBuf::from("book.pdf"),
                page_num: 0,
                content_type: None,
            }
        ));
        assert!(color_filter_item_supported(
            &crate::grid_item::GridItem::Stack {
                key: "a".to_string(),
                representative: PathBuf::from("a001.jpg"),
                count: 2,
            }
        ));

        assert!(!color_filter_item_supported(
            &crate::grid_item::GridItem::Folder(PathBuf::from("dir"))
        ));
        assert!(!color_filter_item_supported(
            &crate::grid_item::GridItem::Video(PathBuf::from("clip.mp4"))
        ));
        assert!(color_filter_item_supported(
            &crate::grid_item::GridItem::ZipFile(PathBuf::from("book.zip"))
        ));
        assert!(color_filter_item_supported(
            &crate::grid_item::GridItem::PdfFile(PathBuf::from("book.pdf"))
        ));
        for item in [
            GridItem::PdfFile(PathBuf::from("book.epub")),
            GridItem::ConvertibleArchive {
                path: PathBuf::from("book.rar"),
                format: crate::archive_converter::ArchiveFormat::Rar,
            },
            GridItem::Audio(PathBuf::from("song.mp3")),
        ] {
            assert!(!color_filter_item_supported(&item));
        }
    }

    #[test]
    fn pdf_color_scan_miss_does_not_write_thumbnail_cache() {
        let decision = always_cache_decision();
        let pdf_req = LoadRequest {
            pdf_page: Some(0),
            ..Default::default()
        };
        let image_req = LoadRequest::default();
        let representative = LoadRequest {
            cache_key_override: Some("pdfthumb:book.pdf#pin:page3".into()),
            ..pdf_req.clone()
        };
        assert!(
            color_scan_cache_decision(&representative, decision).should_cache(
                Path::new("book.pdf"),
                1,
                0.0,
                0.0
            )
        );

        assert!(decision.should_cache(Path::new("book.pdf"), 1, 0.0, 0.0));
        assert!(!color_scan_cache_decision(&pdf_req, decision).should_cache(
            Path::new("book.pdf"),
            1,
            0.0,
            0.0,
        ));
        assert!(
            color_scan_cache_decision(&image_req, decision).should_cache(
                Path::new("image.jpg"),
                1,
                0.0,
                0.0,
            )
        );
    }
}

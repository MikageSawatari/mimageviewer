use super::*;

struct ColorScanWorkItem {
    key: crate::color_search::ColorPaletteKey,
    req: LoadRequest,
}

const COLOR_SCAN_CONFIRM_MISSING_THRESHOLD: usize = 2_000;
const MAX_COLOR_SCAN_MESSAGES_PER_FRAME: usize = 256;

fn color_filter_item_supported(item: &crate::grid_item::GridItem) -> bool {
    item.has_page_data() || matches!(item, crate::grid_item::GridItem::Stack { .. })
}

fn color_scan_cache_decision(req: &LoadRequest, cache_decision: CacheDecision) -> CacheDecision {
    if req.pdf_page.is_none() {
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

impl App {
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
        // 集約ビュー (タグ / 全文検索 / お気に入り検索 / 読書履歴 / レーティング一覧 /
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
            let key = item.perf_key();
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
            work.push(ColorScanWorkItem { key, req });
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
        Some((item.perf_key(), mtime, file_size))
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
        let req = make_load_request(
            item,
            idx,
            mtime,
            file_size,
            false,
            self.pdf_current_password.as_deref(),
            Some(self.settings.folder_thumb_sort),
            self.settings.folder_thumb_depth,
            &self.folder_pin_map,
            &self.converted_archive_cache_paths,
            self.archive_source_override.as_deref(),
            self.current_folder.as_deref(),
            self.folder_thumb_pin_db.as_deref(),
            self.video_pin_db.as_ref(),
            self.use_full_path_cache_keys(),
        )?;
        Some((item.perf_key(), req))
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
                   item: ColorScanWorkItem| {
        if cancel.load(Ordering::Relaxed) {
            return;
        }
        let mtime = item.req.mtime;
        let file_size = item.req.file_size;
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
    let (tx, rx) = mpsc::channel();
    req.priority = false;
    let cache_decision = color_scan_cache_decision(&req, cache_decision);
    if req.pdf_page.is_some() {
        req.force_cache = false;
    }
    crate::thumb_loader::process_load_request(
        &req,
        cache_map,
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
        pin_db,
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

    #[test]
    fn color_filter_targets_page_images_and_stacks_only() {
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
        assert!(!color_filter_item_supported(
            &crate::grid_item::GridItem::ZipFile(PathBuf::from("book.zip"))
        ));
        assert!(!color_filter_item_supported(
            &crate::grid_item::GridItem::PdfFile(PathBuf::from("book.pdf"))
        ));
    }

    #[test]
    fn pdf_color_scan_miss_does_not_write_thumbnail_cache() {
        let decision = always_cache_decision();
        let pdf_req = LoadRequest {
            pdf_page: Some(0),
            ..Default::default()
        };
        let image_req = LoadRequest::default();

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

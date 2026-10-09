//! Read-only witnesses and normal producers for the isolated release-check recipe.
use super::{App, GridItem, ThumbnailState, container_cache_base_key};
use crate::test_script::release_checks::{self as protocol, Observation, Operation};
use serde_json::{Value, json};

impl App {
    pub(super) fn release_check_record_early_tail(&self, ctx: &egui::Context) {
        if !crate::test_script::release_checks::is_active() {
            return;
        }
        let delay = ctx.miv_requested_repaint_delay();
        let action = match delay {
            Some(delay) if delay.is_zero() => "request_repaint",
            Some(_) => "request_repaint_after_observed",
            None => "none",
        };
        crate::perf::event(
            "ui",
            "tail_repaint",
            None,
            0,
            &[
                ("n", json!(self.frame_counter)),
                ("action", json!(action)),
                ("delay_ms", json!(delay.map(|d| d.as_secs_f64() * 1000.0))),
                ("early_return", json!(true)),
            ],
        );
    }

    fn release_check_observation(&self, ctx: &egui::Context) -> Observation {
        let snapshot = self.test_script_snapshot(ctx);
        let mut keep: Vec<_> = self.keep_set.iter().copied().collect();
        keep.sort_unstable();
        let loaded = keep
            .iter()
            .filter(|&&i| {
                self.thumbnails
                    .get(i)
                    .is_some_and(ThumbnailState::is_terminal)
            })
            .count();
        let targets: Vec<_> = keep.iter().filter_map(|&i| {
            let item=self.items.get(i)?;
            let base=match item {
                GridItem::Folder(path) if self.folder_pin_map.contains_key(&crate::path_key::normalize_keep_drive(path)) =>
                    crate::thumb_loader::folder_thumb_cache_key_for_path(path,self.use_full_path_cache_keys(),self.settings.folder_thumb_sort,self.settings.folder_thumb_depth,crate::catalog::FolderThumbProvenance::Seeded),
                _=>container_cache_base_key(item,self.use_full_path_cache_keys(),Some(self.settings.folder_thumb_sort),self.settings.folder_thumb_depth),
            }
                .or_else(|| match item { GridItem::Image(p)|GridItem::Video(p) => p.file_name().map(|p|p.to_string_lossy().into_owned()), _=>None })?;
            Some(json!({"key":item.perf_key(),"idx":i,"items_gen":self.items_generation,"cache_base_key":base}))
        }).collect();
        let complete = !keep.is_empty()
            && loaded == keep.len()
            && self.requested.is_empty()
            && self.texture_backlog.is_empty()
            && !snapshot.startup_open_pending;
        Observation {
            value: json!({"pid":std::process::id(),"root_hwnd":self.main_hwnd.map(|h|h as u64),"perf_t":crate::perf::program_start().map(|s|s.elapsed().as_secs_f64()),
                "window_visible":self.window_visible,"focused":snapshot.focused,
                "current_folder_path":snapshot.current_folder_path,"items_generation":self.items_generation,
                "items_len":self.items.len(),"requested_count":self.requested.len(),
                "texture_backlog_count":self.texture_backlog.len(),"keep_count":keep.len(),"loaded_keep_count":loaded,
                "requested_px":self.display_px_shared.load(std::sync::atomic::Ordering::Relaxed),
                "loading_complete":complete,"index_settled":!self.initial_scan_settled_pending,
                "search_active":self.global_search.active,"search_focused":self.global_search.has_focus,
                "search_query":self.global_search.query,"search_done":self.global_search.done,
                "search_searching":self.global_search.is_searching(),
                "search_hits":self.global_search.all_hits.iter().map(|h|h.path.clone()).collect::<Vec<_>>(),
                "is_fullscreen":snapshot.is_fullscreen,"cache_policy_off":matches!(self.settings.cache_policy,crate::settings::CachePolicy::Off),
                "ai_pending":self.ai_upscale_pending.len()+self.final_ai_pending.len(),"ai_ready":self.ai_upscale_cache.len()+self.final_ai_cache.len(),
                "ai_backend":self.ai_runtime_init.ready_runtime().map(|r|r.active_backend().effective.as_str()),
                "tray_available":self.tray_controller.is_some(),"targets":targets}),
            catalog: self.current_color_catalog.clone(),
            cache: self.current_color_cache_map.clone(),
            pins: self.folder_thumb_pin_db.clone(),
        }
    }
    pub(super) fn poll_release_check_commands(&mut self, ctx: &egui::Context) {
        while let Some(request) = protocol::take_request() {
            let result = (|| {
                if std::time::Instant::now() >= request.deadline {
                    return Err("environment: expired release UI command".into());
                }
                crate::test_script::validate_release_owner(&request.owner)?;
                if self.main_hwnd.map(|h| h as u64) != Some(request.owner.hwnd()) {
                    return Err("environment: release root HWND changed".into());
                }
                match request.operation {
                    Operation::Observe => (),
                    Operation::Open(path) => {
                        self.load_folder(path);
                    }
                    Operation::Text(text) => {
                        if !self.release_check_observation(ctx).value["focused"]
                            .as_bool()
                            .unwrap_or(false)
                            || !self.global_search.active
                            || !self.global_search.has_focus
                            || self.ime_input_active(ctx)
                        {
                            return Err(
                                "environment: exact root search editor must own text input".into(),
                            );
                        }
                        // The normal TextEdit consumes a real text event; never assign query or
                        // call a search handler. This diagnostic doesn't claim IME coverage.
                        ctx.input_mut(|input| input.events.push(egui::Event::Text(text.clone())));
                        protocol::trace(
                            "text",
                            json!({"text":text,"root_hwnd":request.owner.hwnd()}),
                        );
                    }
                    Operation::Pin(path) => {
                        if self.current_folder.as_deref() != path.parent() {
                            return Err(
                                "environment: video pin requires its generated physical folder"
                                    .into(),
                            );
                        }
                        let idx = self
                            .items
                            .iter()
                            .position(|item| matches!(item,GridItem::Video(p) if *p==path))
                            .ok_or("environment: generated video item missing")?;
                        self.toggle_folder_pin_for_idx(idx);
                    }
                }
                Ok(self.release_check_observation(ctx))
            })();
            let _ = request.reply.send(result);
        }
    }
}

pub(crate) fn record_close_accept(app: &App) {
    let mut session = protocol::session().lock().unwrap();
    let Some(s) = session.as_mut() else { return };
    if s.close_mode.is_none() {
        return;
    }
    s.close_sequence += 1;
    let keep: Vec<_> = app.keep_set.iter().copied().collect();
    let complete = !keep.is_empty()
        && keep.iter().all(|&i| {
            app.thumbnails
                .get(i)
                .is_some_and(ThumbnailState::is_terminal)
        })
        && app.requested.is_empty()
        && app.texture_backlog.is_empty();
    let mut targets = s.cache_proof["targets"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let same = s.cache_proof["items_gen"].as_u64() == Some(app.items_generation)
        && s.cache_proof["folder_path"].as_str()
            == app
                .current_folder
                .as_ref()
                .map(|p| p.to_string_lossy().into_owned())
                .as_deref();
    for target in &mut targets {
        let base = target["cache_base_key"].as_str().unwrap_or("");
        let cache_absent = app
            .current_color_cache_map
            .as_ref()
            .and_then(|m| m.try_read().ok())
            .is_some_and(|m| !m.keys().any(|k| k.starts_with(base)));
        target["cache_map_absent"] = json!(same && cache_absent);
        target["catalog_absent"] = json!(same && target["catalog_absent"] == true);
    }
    s.close_proof = json!({"close_sequence":s.close_sequence,"folder_path":app.current_folder,
        "items_gen":app.items_generation,"loading_complete":complete,"targets":targets,
        "cache_check_unix_ms":s.cache_proof["cache_check_unix_ms"],"native_message":"WM_CLOSE"});
    let proof = s.close_proof.clone();
    drop(session);
    protocol::trace("close_accept", proof);
}
pub(crate) fn record_tray_hide(_app: &App) {
    let proof = protocol::session().lock().ok().and_then(|s| {
        s.as_ref()
            .filter(|s| s.close_sequence > 0)
            .map(|s| s.close_proof.clone())
    });
    if let Some(proof) = proof {
        protocol::trace("tray_hide", proof);
    }
}
pub(crate) fn record_finalized(
    app: &App,
    request_id: u64,
    idx: usize,
    items_gen: u64,
    input_seq: u64,
) {
    let backlog = app.texture_backlog.iter().any(|msg| {
        msg.diagnostic_id == request_id
            && msg.idx == idx
            && msg.items_gen == items_gen
            && matches!(msg.payload, crate::thumb_loader::ThumbMsgPayload::Pixels(_))
    });
    let state = match app.thumbnails.get(idx) {
        Some(ThumbnailState::Pending) => "Pending",
        Some(ThumbnailState::Evicted) => "Evicted",
        Some(ThumbnailState::Loaded { .. }) => "Loaded",
        Some(ThumbnailState::NoArt) => "NoArt",
        Some(ThumbnailState::Failed) => "Failed",
        None => "missing",
    };
    let mut session = protocol::session().lock().unwrap();
    let Some(s) = session.as_mut().filter(|s| s.close_sequence > 0) else {
        return;
    };
    let value = json!({"request_id":request_id,"close_sequence":s.close_sequence,"key":app.items.get(idx).map(GridItem::perf_key),
        "idx":idx,"items_gen":items_gen,"input_seq":input_seq,"state":state,
        "image_backlog_contains_request":backlog,"window_visible":app.window_visible});
    s.finalized.push(value.clone());
    drop(session);
    protocol::trace("finalized", value);
}

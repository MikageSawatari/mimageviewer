//! Next-unit edit preparation. All unpublished CPU/GPU resources live in the
//! existing final-effect owner; display and editing never wait for this worker.
use super::final_prepare::{
    FinalEffectOrigin, FinalEffectReady, FinalEffectReceiver, FinalEffectState, final_upload_pixels,
};
use super::final_prepare_budget::{
    FinalPrepareAdoption, FinalPrepareGpuAdmission, FinalPrepareGpuDeferred,
};
use super::final_prepare_cpu::{
    FinalPrepareReservation, FinalPrepareStartDenied, FinalPrepareUnit,
};
use super::*;

pub(crate) mod rejection;
pub(crate) use rejection::{
    ComicExclusion, FinalPipelineInvalidation, PreparedEditCancel, PreparedEditDisposition,
    PreparedEditReject,
};

#[cfg(test)]
mod font_prefetch_tests;
#[cfg(test)]
mod review_lifecycle_tests;
#[cfg(test)]
mod review_pixel_tests;

#[derive(Clone, Copy)]
enum InputStage {
    Raw,
    Erase,
    Local,
    Conceal,
    Edit,
    AfterAi,
    Final,
}

#[derive(Clone, Copy)]
enum PreparedOutput {
    Final,
    AnnotationOnly,
}

/// Stage materialization and the authoritative final input are independent.
/// Rehydrating an evicted local stage must not apply edits/tone to AI twice.
enum PreparedFinalInput {
    Edit,
    AfterAi(Arc<egui::ColorImage>),
    Completed {
        pixels: Arc<egui::ColorImage>,
        basis: crate::material_display_basis::MaterialDisplayBasis,
    },
}

fn identity_plan(plan: &FinalCompositePlan) -> bool {
    plan.adjust_before_effect.is_none()
        && plan.smart_sharpen == 0
        && !plan.colorize.is_enabled()
        && plan.creative_lut.is_none()
        && plan.post_filter == crate::adjustment::PostFilter::None
}

pub(super) struct PreparedEditInput {
    pub(super) key: FinalCompositeKey,
    local_key: LocalAdjustResultKey,
    page_key: String,
    source: Arc<egui::ColorImage>,
    stage: InputStage,
    source_size: [usize; 2],
    params: crate::adjustment::AdjustParams,
    plan: FinalCompositePlan,
    output: PreparedOutput,
    final_input: PreparedFinalInput,
    local: Option<LocalMaterializeLayers>,
    conceal: Option<(PathBuf, crate::conceal::ConcealPreset)>,
    comic: Option<ComicInput>,
    canonical: Option<crate::page_dims::CanonicalPageDimensions>,
    pdf: Option<PdfPageMetadataSnapshot>,
    basis: crate::material_display_basis::MaterialDisplayBasis,
}

struct ComicInput {
    doc: Option<Vec<comic_core::AnnotationObject>>,
    path: PathBuf,
    generation: u64,
    fonts: Option<Arc<comic_core::FontSet>>,
    loaded_keys: std::collections::HashSet<String>,
    canonical_size: [usize; 2],
    stamps: std::collections::HashMap<String, Option<Arc<comic_core::RgbaOverlay>>>,
}

pub(super) struct PreparedEditPixels {
    pub(super) input: Arc<PreparedEditInput>,
    output: Result<EditOutput, String>,
    _reservation: Arc<FinalPrepareReservation>,
    elapsed_ms: f64,
}

struct EditOutput {
    local: Option<crate::edit_source::LocalAdjustMaterializeOutput>,
    conceal: Option<Arc<egui::ColorImage>>,
    edit: Option<Arc<egui::ColorImage>>,
    final_pixels: Option<Arc<egui::ColorImage>>,
    final_upload: Option<Arc<egui::ColorImage>>,
    timing: crate::final_composite::FinalCompositeTiming,
    clamp_ms: f64,
    doc: Option<Vec<comic_core::AnnotationObject>>,
    comic: Option<ComicPixels>,
}

struct ComicPixels {
    pixels: Arc<egui::ColorImage>,
    upload: Arc<egui::ColorImage>,
    stamps: Vec<(String, Option<Arc<comic_core::RgbaOverlay>>)>,
}

pub(super) struct PreparedEditRegistration {
    pub(super) ready: PreparedEditPixels,
    final_texture: Option<egui::TextureHandle>,
    comic_texture: Option<egui::TextureHandle>,
    load_texture_ms: f64,
}

struct WorkerTrace {
    request_id: u64,
    context: u64,
    idx: usize,
    started: Option<std::time::Instant>,
    cancel: Arc<AtomicBool>,
}

impl Drop for WorkerTrace {
    fn drop(&mut self) {
        if crate::perf::is_enabled() {
            crate::perf::event(
                "fs",
                "final_prepare_worker_finished",
                None,
                0,
                &[
                    ("request_id", self.request_id.into()),
                    ("context", self.context.into()),
                    ("idx", self.idx.into()),
                    ("pipeline", "edit".into()),
                    ("requested_position", "next".into()),
                    ("demand_position", "unknown".into()),
                    ("cancelled", self.cancel.load(Ordering::Relaxed).into()),
                    (
                        "worker_ms",
                        self.started
                            .map_or(0.0, |started| started.elapsed().as_secs_f64() * 1000.0)
                            .into(),
                    ),
                ],
            );
        }
    }
}

fn prepared_upload(pixels: &Arc<egui::ColorImage>, clamp_ms: &mut f64) -> Arc<egui::ColorImage> {
    let started = crate::perf::is_enabled().then(std::time::Instant::now);
    let upload = final_upload_pixels(pixels);
    if let Some(started) = started {
        *clamp_ms += started.elapsed().as_secs_f64() * 1000.0;
    }
    upload
}

impl PreparedEditRegistration {
    pub(super) fn textures(
        &self,
    ) -> impl Iterator<Item = (&egui::TextureHandle, egui::TextureOptions)> {
        let options = if self.ready.input.plan.needs_nearest_sampler() {
            egui::TextureOptions::NEAREST
        } else {
            DISPLAY_IMAGE_TEXTURE_OPTIONS
        };
        self.final_texture
            .iter()
            .chain(self.comic_texture.iter())
            .map(move |texture| (texture, options))
    }
}

fn fonts_satisfy(
    doc: &[comic_core::AnnotationObject],
    fonts: &Option<Arc<comic_core::FontSet>>,
    keys: &std::collections::HashSet<String>,
) -> bool {
    fonts.is_some()
        && crate::comic_overlay::referenced_font_keys(doc)
            .iter()
            .all(|key| keys.contains(key))
}

fn image_bytes([w, h]: [usize; 2]) -> Option<u64> {
    u64::try_from(w)
        .ok()?
        .checked_mul(u64::try_from(h).ok()?)?
        .checked_mul(4)
}

/// Reserve allocations, not aliases: edit shares the last stage, identity final
/// shares edit, and unclamped uploads share their canonical result.
fn output_bytes(input: &PreparedEditInput) -> Option<u64> {
    let mut bytes = 0u64;
    let size = input.source.size;
    if input.local.is_some() {
        bytes = bytes.checked_add(image_bytes(size)?)?;
    }
    if input.conceal.is_some() {
        bytes = bytes.checked_add(image_bytes(size)?)?;
    }
    let final_size = match &input.final_input {
        PreparedFinalInput::Completed { pixels, .. } => pixels.size,
        PreparedFinalInput::AfterAi(pixels) => {
            crate::post_filter::output_dimensions(pixels.size, input.plan.post_filter)
        }
        PreparedFinalInput::Edit => {
            crate::post_filter::output_dimensions(size, input.plan.post_filter)
        }
    };
    if !matches!(input.final_input, PreparedFinalInput::Completed { .. }) {
        if input.plan.adjust_before_effect.is_some()
            || input.plan.smart_sharpen != 0
            || input.plan.colorize.is_enabled()
            || input.plan.creative_lut.is_some()
            || input.plan.post_filter != crate::adjustment::PostFilter::None
        {
            bytes = bytes.checked_add(image_bytes(final_size)?)?;
        }
        let upload = gpu_clamped_dimensions(final_size);
        if upload != final_size {
            bytes = bytes.checked_add(image_bytes(upload)?)?;
        }
    }
    if input.comic.is_some() {
        bytes = bytes.checked_add(image_bytes(final_size)?)?;
        let upload = gpu_clamped_dimensions(final_size);
        if upload != final_size {
            bytes = bytes.checked_add(image_bytes(upload)?)?;
        }
    }
    Some(bytes)
}

fn read_local(
    input: &LocalMaterializeLayers,
) -> Result<local_adjust_core::LocalAdjustmentLayers, String> {
    match input {
        LocalMaterializeLayers::Memory(layers) => Ok(layers.clone()),
        LocalMaterializeLayers::Database { path, page_key } => {
            let db = crate::local_adjust_db::LocalAdjustDb::open_readonly(path)
                .map_err(|e| e.to_string())?;
            let Some(json) = db.get_layers_json(page_key) else {
                return Ok(Default::default());
            };
            crate::local_adjust_db::parse_layers_json(&json)
                .map(Arc::new)
                .map_err(|e| e.to_string())
        }
    }
}

fn execute(input: &PreparedEditInput, cancel: &Arc<AtomicBool>) -> Result<EditOutput, String> {
    let mut out = EditOutput {
        local: None,
        conceal: None,
        edit: None,
        final_pixels: None,
        final_upload: None,
        timing: Default::default(),
        clamp_ms: 0.0,
        doc: None,
        comic: None,
    };
    let mut pixels = input.source.clone();
    if let Some(local) = &input.local {
        let result =
            crate::edit_source::materialize_local_adjust(&pixels, read_local(local)?, cancel)
                .map_err(|e| e.to_string())?;
        pixels = result.pixels.clone();
        out.local = Some(result);
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(out);
    }
    if let Some((path, preset)) = &input.conceal {
        let db = crate::conceal_db::ConcealDb::open_readonly(path).map_err(|e| e.to_string())?;
        let [w, h] = pixels.size;
        let mask = db
            .get_raw(&input.page_key)
            .and_then(|raw| crate::conceal_db::ConcealDb::materialize_raw(raw, w, h, Some(cancel)));
        let (bitmap, shapes) = mask.unwrap_or_else(|| (vec![false; w * h], Vec::new()));
        pixels = crate::edit_source::materialize_conceal(
            &pixels,
            crate::edit_source::ConcealMaterialize {
                mask: crate::edit_source::MaskSnapshot {
                    bitmap,
                    shapes,
                    size: [w, h],
                },
                preset: preset.clone(),
            },
            cancel,
        )
        .map_err(|e| e.to_string())?;
        out.conceal = Some(pixels.clone());
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(out);
    }
    if !matches!(input.stage, InputStage::AfterAi | InputStage::Final) {
        out.edit = Some(pixels.clone());
    }
    match &input.final_input {
        PreparedFinalInput::Edit => {}
        PreparedFinalInput::AfterAi(source)
        | PreparedFinalInput::Completed { pixels: source, .. } => pixels = source.clone(),
    }
    if matches!(input.output, PreparedOutput::Final) {
        if !matches!(input.final_input, PreparedFinalInput::Completed { .. }) {
            let FinalCompositeResult::Ready {
                pixels: final_pixels,
                timing,
                ..
            } = execute_final_composite(pixels, input.plan.clone(), cancel)
            else {
                return Ok(out);
            };
            pixels = final_pixels;
            out.timing = timing;
            out.final_upload = Some(prepared_upload(&pixels, &mut out.clamp_ms));
        }
        out.final_pixels = Some(pixels.clone());
    }
    if cancel.load(Ordering::Relaxed) {
        return Ok(out);
    }
    if let Some(comic) = &input.comic {
        let doc = match &comic.doc {
            Some(doc) => doc.clone(),
            None => crate::comic_db::ComicDb::open_readonly_at(&comic.path)
                .map_err(|e| e.to_string())?
                .get(&input.page_key)
                .unwrap_or_default(),
        };
        if !doc.is_empty() && fonts_satisfy(&doc, &comic.fonts, &comic.loaded_keys) {
            let scale = pixels.size[0].max(pixels.size[1]) as f32
                / comic.canonical_size[0].max(comic.canonical_size[1]).max(1) as f32;
            let scaled = if (scale - 1.0).abs() > 1e-4 {
                comic_core::scale_scene(&doc, scale)
            } else {
                doc.clone()
            };
            let mut stamps = comic.stamps.clone();
            let (images, updates, _) = crate::comic_stamp::build_stamp_images_from_cache_snapshot(
                &scaled,
                &mut stamps,
                cancel,
            );
            if cancel.load(Ordering::Relaxed) {
                return Ok(out);
            }
            let layers = comic_core::bake_annotation_layers(
                &scaled,
                pixels.size[0],
                pixels.size[1],
                comic.fonts.as_ref().unwrap(),
                &images,
            );
            let pixels = Arc::new(crate::comic_overlay::composite_annotation_layers(
                &pixels, &layers,
            ));
            let upload = prepared_upload(&pixels, &mut out.clamp_ms);
            out.comic = Some(ComicPixels {
                pixels,
                upload,
                stamps: updates,
            });
        }
        // Resolving a cold doc is useful even when no font can bake it yet.
        out.doc = Some(doc);
    }
    if matches!(input.output, PreparedOutput::AnnotationOnly)
        && !matches!(input.final_input, PreparedFinalInput::Completed { .. })
        && out.comic.is_some()
    {
        out.final_upload = Some(prepared_upload(&pixels, &mut out.clamp_ms));
        out.final_pixels = Some(pixels);
    }
    Ok(out)
}

#[cfg(test)]
mod tests;

impl App {
    fn record_edit_texture_registration(
        &self,
        key: FinalCompositeKey,
        texture: Option<&egui::TextureHandle>,
        stage: &'static str,
        shared: bool,
        display: bool,
        load_texture_ms: f64,
        worker_ms: f64,
        clamp_ms: f64,
    ) {
        if !crate::perf::is_enabled() {
            return;
        }
        let pending = &self.final_effect_pending[&key];
        let options = if pending.nearest_sampler {
            egui::TextureOptions::NEAREST
        } else {
            DISPLAY_IMAGE_TEXTURE_OPTIONS
        };
        let bytes = texture.map_or(0, |texture| {
            crate::vram_budget::rgba8_texels_to_bytes(crate::vram_budget::texture_size_texels(
                texture.size(),
                options.mipmap_mode.is_some(),
            ))
        });
        crate::perf::event(
            "fs",
            "final_prepare_registered",
            None,
            self.input_seq,
            &[
                ("request_id", pending.gpu_diagnostics.request_id().into()),
                (
                    "context",
                    self.projected_viewer_context_id().serial().into(),
                ),
                ("idx", key.edit_key.idx.into()),
                ("pipeline", "edit".into()),
                ("stage", stage.into()),
                (
                    "requested_position",
                    pending.gpu_diagnostics.requested_position().into(),
                ),
                (
                    "demand_position",
                    self.final_prepare_key_position(key)
                        .map_or("unknown", crate::ui_fullscreen::FinalUnitPosition::as_str)
                        .into(),
                ),
                (
                    "renderer",
                    self.wgpu_render_state
                        .as_ref()
                        .map(|state| state.renderer.read().diag_id().into())
                        .unwrap_or(serde_json::Value::Null),
                ),
                (
                    "texture",
                    texture
                        .map(|texture| format!("{:?}", texture.id()).into())
                        .unwrap_or(serde_json::Value::Null),
                ),
                ("params_hash", key.params_hash.into()),
                ("items_generation", pending.items_generation.into()),
                ("source_generation", key.edit_key.source_gen.into()),
                ("origin", "speculative".into()),
                ("speculative", (!display).into()),
                ("display_takeover", display.into()),
                ("shared", shared.into()),
                ("bytes", bytes.into()),
                ("new_bytes", (if shared { 0 } else { bytes }).into()),
                ("options", format!("{options:?}").into()),
                ("outer_frame", self.speculative_gpu_frame.frame.into()),
                ("load_texture_ms", load_texture_ms.into()),
                ("worker_ms", worker_ms.into()),
                ("clamp_ms", clamp_ms.into()),
                ("published", false.into()),
            ],
        );
    }
    /// An editing request always revokes B's publication before handing control
    /// to the existing Enter/Switch/Resume owner. Never merge a continuation.
    #[track_caller]
    pub(crate) fn cancel_prepared_edit_for_entry(&mut self, idx: usize) {
        self.cancel_prepared_edit_for_idx(idx, PreparedEditCancel::EditingEntry);
    }

    #[track_caller]
    fn cancel_prepared_edit_for_idx(&mut self, idx: usize, reason: PreparedEditCancel) {
        let keys: Vec<_> = self
            .final_effect_pending
            .iter()
            .filter_map(|(key, pending)| {
                (key.edit_key.idx == idx && pending.state.edit_input().is_some()).then_some(*key)
            })
            .collect();
        for key in keys {
            self.cancel_final_effect_key(key, reason);
        }
    }

    /// Receive before classifying: a result already in the channel is ready,
    /// even when no ordinary polling pass has run since worker completion.
    #[track_caller]
    pub(crate) fn resolve_prepared_edit_for_display(&mut self, ctx: &egui::Context, idx: usize) {
        let keys: Vec<_> = self
            .final_effect_pending
            .iter()
            .filter_map(|(key, pending)| {
                (key.edit_key.idx == idx && pending.state.edit_input().is_some()).then_some(*key)
            })
            .collect();
        for key in keys {
            let received = self
                .final_effect_pending
                .get_mut(&key)
                .unwrap()
                .state
                .receive();
            if received.is_err() {
                self.cancel_final_effect_key(
                    key,
                    match received {
                        Err(mpsc::TryRecvError::Empty) => PreparedEditCancel::DisplayComputing,
                        _ => PreparedEditCancel::WorkerDisconnected,
                    },
                );
                continue;
            }
            self.adopt_prepared_edit(ctx, key, FinalPrepareAdoption::BeforeLookup, true);
        }
    }

    fn validate_prepared_comic(
        &self,
        input: &PreparedEditInput,
        out: &EditOutput,
    ) -> Result<(), ComicExclusion> {
        let Some(comic) = &input.comic else {
            return Ok(());
        };
        if comic.generation != self.comic_generation {
            return Err(ComicExclusion::Generation);
        }
        if !self
            .comic_docs
            .get(&input.page_key)
            .is_none_or(|doc| Some(doc) == out.doc.as_ref())
        {
            return Err(ComicExclusion::DocumentChanged);
        }
        if !match (&comic.fonts, self.comic_fonts.fonts()) {
            (Some(old), Some(current)) => Arc::ptr_eq(old, current),
            (None, None) => true,
            _ => false,
        } {
            return Err(ComicExclusion::FontSetChanged);
        }
        if &comic.loaded_keys != self.comic_fonts.loaded_keys() {
            return Err(ComicExclusion::FontKeysChanged);
        }
        Ok(())
    }

    /// Returns true for B-owned states (including deferred registration). The
    /// legacy adoption path must never destructure or partially publish them.
    #[track_caller]
    pub(super) fn adopt_prepared_edit(
        &mut self,
        ctx: &egui::Context,
        key: FinalCompositeKey,
        phase: FinalPrepareAdoption,
        display: bool,
    ) -> bool {
        let Some(pending) = self.final_effect_pending.get(&key) else {
            return false;
        };
        if pending.state.edit_input().is_none() {
            return false;
        }
        if let Err(reason) = self.validate_final_prepare_pending(key, pending) {
            self.cancel_final_effect_key(key, PreparedEditCancel::Rejected(reason));
            return true;
        }
        if !display
            && pending
                .origin
                .unit()
                .is_some_and(|unit| !self.speculative_final_unit_is_needed(unit))
        {
            self.cancel_final_effect_key(key, PreparedEditCancel::DemandObsolete);
            return true;
        }
        let (input, out, elapsed_ms) = match &pending.state {
            FinalEffectState::CpuReady(FinalEffectReady::Edited(ready)) => {
                (&ready.input, &ready.output, ready.elapsed_ms)
            }
            FinalEffectState::Registering(registration) => (
                &registration.ready.input,
                &registration.ready.output,
                registration.ready.elapsed_ms,
            ),
            _ => return true,
        };
        let out = match out {
            Ok(out) => out,
            Err(error) => {
                crate::logger::log(format!("[edit-prepare] idx={}: {error}", key.edit_key.idx));
                self.cancel_final_effect_key(key, PreparedEditCancel::WorkerFailed);
                self.show_feedback_toast("編集画像の先読みを完了できませんでした".to_owned());
                return true;
            }
        };
        // A cold document hydrated by another owner cannot be overwritten by
        // speculative normalization, even when the numerical key is unchanged.
        if matches!(input.local, Some(LocalMaterializeLayers::Database { .. }))
            && let Some(local) = &out.local
            && self
                .local_adjust_page_layers
                .get(&key.edit_key.idx)
                .is_some_and(|layers| *layers != local.layers)
        {
            self.cancel_final_effect_key(
                key,
                PreparedEditCancel::Rejected(PreparedEditReject::LayersChanged),
            );
            return true;
        }
        let comic_verdict = self.validate_prepared_comic(input, out);
        if let Err(reason) = comic_verdict {
            self.record_prepared_edit_disposition(
                key,
                PreparedEditDisposition::ComicExcluded(reason),
            );
        }
        let comic_current = input.comic.is_some() && comic_verdict.is_ok();
        let has_images = out.final_pixels.is_some() || (comic_current && out.comic.is_some());
        if !has_images {
            self.record_edit_texture_registration(
                key,
                None,
                "doc",
                true,
                display,
                0.0,
                elapsed_ms,
                out.clamp_ms,
            );
            let pending = self.final_effect_pending.remove(&key).unwrap();
            let ready = match pending.state {
                FinalEffectState::CpuReady(FinalEffectReady::Edited(ready)) => ready,
                FinalEffectState::Registering(registration) => registration.ready,
                _ => unreachable!(),
            };
            if comic_current
                && let Ok(out) = ready.output
                && let Some(doc) = out.doc
            {
                self.comic_docs
                    .entry(ready.input.page_key.clone())
                    .or_insert(doc);
            }
            return true;
        }
        if !display {
            if self.final_prepare_key_position(key)
                == Some(crate::ui_fullscreen::FinalUnitPosition::Previous)
                && !matches!(
                    phase,
                    FinalPrepareAdoption::RootFinalized(
                        _,
                        crate::ui_fullscreen::FinalUnitPosition::Previous
                    )
                )
            {
                return true;
            }
            if self.fullscreen_idx.is_some_and(|idx| {
                !self
                    .final_prepare_work_admission(idx)
                    .admits_backlog_upload(
                        self.fs_navigation_rendition_target_pages(idx)
                            .is_some_and(|pages| pages.contains(&key.edit_key.idx)),
                    )
            }) {
                return true;
            }
        }
        // Convert the same owner, keeping its reservation and private handles.
        if matches!(pending.state, FinalEffectState::CpuReady(_)) {
            let mut pending = self.final_effect_pending.remove(&key).unwrap();
            let FinalEffectState::CpuReady(FinalEffectReady::Edited(ready)) = pending.state else {
                unreachable!()
            };
            pending.state = FinalEffectState::Registering(PreparedEditRegistration {
                ready,
                final_texture: None,
                comic_texture: None,
                load_texture_ms: 0.0,
            });
            self.final_effect_pending.insert(key, pending);
        }
        // Re-evaluate sharing before spending a GPU slot. The final source of a
        // comic-only job already belongs to the complete final cache.
        let registration = match &self.final_effect_pending[&key].state {
            FinalEffectState::Registering(registration) => registration,
            _ => unreachable!(),
        };
        let input = &registration.ready.input;
        let out = registration.ready.output.as_ref().unwrap();
        let options = if input.plan.needs_nearest_sampler() {
            egui::TextureOptions::NEAREST
        } else {
            DISPLAY_IMAGE_TEXTURE_OPTIONS
        };
        let shared = if registration.final_texture.is_none() {
            if matches!(input.final_input, PreparedFinalInput::Completed { .. }) {
                self.exact_complete_final_composite_entry(key.edit_key.idx)
                    .map(|(_, e)| e.texture.clone())
            } else if let (Some(pixels), Some(upload)) = (&out.final_pixels, &out.final_upload)
                && Arc::ptr_eq(pixels, upload)
            {
                self.shared_prepared_edit_texture(ctx, key, pixels, options)
            } else {
                None
            }
        } else {
            None
        };
        if let Some(texture) = shared {
            self.record_edit_texture_registration(
                key,
                Some(&texture),
                "shared_handle",
                true,
                display,
                0.0,
                registration.ready.elapsed_ms,
                out.clamp_ms,
            );
            let FinalEffectState::Registering(registration) =
                &mut self.final_effect_pending.get_mut(&key).unwrap().state
            else {
                unreachable!()
            };
            registration.final_texture = Some(texture);
        }
        loop {
            // Keep the owner in its map while the all-context VRAM inventory is
            // read, so the first private texture is counted before the second.
            let FinalEffectState::Registering(registration) =
                &self.final_effect_pending[&key].state
            else {
                unreachable!()
            };
            let out = registration.ready.output.as_ref().unwrap();
            let worker_ms = registration.ready.elapsed_ms;
            let clamp_ms = out.clamp_ms;
            let next = if registration.final_texture.is_none() {
                out.final_upload
                    .as_ref()
                    .map(|upload| (false, upload.clone()))
            } else if comic_current && registration.comic_texture.is_none() {
                out.comic.as_ref().map(|comic| (true, comic.upload.clone()))
            } else {
                None
            };
            let Some((comic, upload)) = next else {
                break;
            };
            if !display {
                let deferred = match phase {
                    FinalPrepareAdoption::BeforeLookup => {
                        Some(FinalPrepareGpuDeferred::BeforeFinalOutput)
                    }
                    FinalPrepareAdoption::RootFinalized(reason, _) => reason,
                };
                if let Some(reason) = deferred {
                    self.record_final_prepare_gpu_deferred(key, reason);
                    if matches!(
                        reason,
                        FinalPrepareGpuDeferred::OrdinaryUpload
                            | FinalPrepareGpuDeferred::FrameBusy
                    ) {
                        ctx.request_repaint_of(egui::ViewportId::ROOT);
                    }
                    return true;
                }
                match self.admit_final_prepare_registration(upload.size, options) {
                    FinalPrepareGpuAdmission::Admitted => {}
                    FinalPrepareGpuAdmission::FrameBusy => {
                        self.record_final_prepare_gpu_deferred(
                            key,
                            FinalPrepareGpuDeferred::FrameBusy,
                        );
                        ctx.request_repaint_of(egui::ViewportId::ROOT);
                        return true;
                    }
                    FinalPrepareGpuAdmission::OverBudget => {
                        self.record_final_prepare_gpu_deferred(
                            key,
                            FinalPrepareGpuDeferred::OverBudget,
                        );
                        return true;
                    }
                }
            } else {
                self.speculative_gpu_frame.note_ordinary_upload();
            }
            let registration_started = crate::perf::is_enabled().then(std::time::Instant::now);
            diagnostic_stage(
                &self.final_effect_pending[&key].diagnostic,
                JobStage::GpuRegistration,
            );
            let texture = ctx.load_texture(
                format!(
                    "edit_prepare_{}_{}_{comic}",
                    key.edit_key.idx, key.params_hash
                ),
                upload,
                options,
            );
            self.bind_display_texture_identity(&texture, key.edit_key.item_id);
            let load_texture_ms = registration_started
                .map_or(0.0, |started| started.elapsed().as_secs_f64() * 1000.0);
            self.record_edit_texture_registration(
                key,
                Some(&texture),
                if comic { "comic" } else { "final" },
                false,
                display,
                load_texture_ms,
                worker_ms,
                clamp_ms,
            );
            let FinalEffectState::Registering(registration) =
                &mut self.final_effect_pending.get_mut(&key).unwrap().state
            else {
                unreachable!()
            };
            registration.load_texture_ms += load_texture_ms;
            if comic {
                registration.comic_texture = Some(texture);
            } else {
                registration.final_texture = Some(texture);
            }
            // The common ROOT claim makes the next allocation defer, requesting
            // another ROOT frame. Display takeover can finish ordinary uploads.
        }
        // No mutation to any image cache has occurred before this point. The
        // preflight remains valid: registration itself runs no producer or I/O.
        let pending = self.final_effect_pending.remove(&key).unwrap();
        let FinalEffectState::Registering(registration) = pending.state else {
            unreachable!()
        };
        diagnostic_stage(&pending.diagnostic, JobStage::Completed);
        let input = &registration.ready.input;
        let worker_ms = registration.ready.elapsed_ms;
        let load_texture_ms = registration.load_texture_ms;
        let out = registration.ready.output.unwrap();
        let idx = key.edit_key.idx;
        if let Some(local) = out.local {
            // Match the ordinary inactive-document adoption. Memory-only edits
            // must not become persisted-presence entries through the helper.
            if local.layers.is_empty() {
                Self::set_page_key_presence(
                    &mut self.local_adjust_page_keys,
                    &input.page_key,
                    false,
                );
            }
            self.adopt_materialized_local_layers(idx, local.layers);
            if local.active {
                self.local_adjust_cache.insert(
                    input.local_key,
                    LocalAdjustCacheEntry {
                        pixels: local.pixels,
                        texture: None,
                        texture_options: DISPLAY_IMAGE_TEXTURE_OPTIONS,
                        display_basis: input.basis,
                    },
                );
            }
        }
        if let Some(pixels) = out.conceal {
            self.conceal_cache.insert(
                idx,
                ConcealCacheEntry {
                    pixels,
                    texture: None,
                    texture_options: DISPLAY_IMAGE_TEXTURE_OPTIONS,
                    display_basis: input.basis,
                    generation: key.edit_key.conceal_gen,
                },
            );
        }
        if let Some(pixels) = out.edit {
            self.edit_result_cache.insert(
                key.edit_key,
                EditResultEntry {
                    pixels,
                    texture: None,
                    display_basis: input.basis,
                },
            );
        }
        let texture = registration.final_texture.expect("registered final base");
        let base = out
            .final_pixels
            .unwrap_or_else(|| match &input.final_input {
                PreparedFinalInput::Edit => input.source.clone(),
                PreparedFinalInput::AfterAi(pixels)
                | PreparedFinalInput::Completed { pixels, .. } => pixels.clone(),
            });
        let basis = if let PreparedFinalInput::Completed { basis, .. } = input.final_input {
            basis
        } else {
            self.accept_raster_display_basis(
                idx,
                texture.size(),
                input.canonical,
                input.source_size,
                input.pdf,
                true,
            )
        };
        if !matches!(input.final_input, PreparedFinalInput::Completed { .. }) {
            self.final_composite_cache.insert(
                key,
                FinalCompositeEntry {
                    pixels: base.clone(),
                    texture,
                    texture_options: options,
                    display_basis: basis,
                    canonical_dims: input.canonical,
                    source_size: input.source_size,
                    complete: true,
                },
            );
            self.final_composite_cache
                .record_colorize_applied(key, out.timing.colorize_applied);
            self.continuous_page_transitions.remove(&idx);
            self.comic_cache.remove(&idx);
        }
        if comic_current {
            if let Some(doc) = out.doc {
                self.comic_docs.entry(input.page_key.clone()).or_insert(doc);
            }
            if let Some(comic) = out.comic {
                let texture = registration.comic_texture.expect("registered annotation");
                for (key, value) in comic.stamps {
                    self.comic_stamp_cache.entry(key).or_insert(value);
                }
                self.comic_cache.insert(
                    idx,
                    ComicCacheEntry {
                        display_basis: Self::accept_comic_material_display_basis(
                            basis,
                            texture.size(),
                        ),
                        pixels: comic.pixels,
                        texture,
                        texture_options: options,
                        base: base.clone(),
                        comic_gen: self.comic_generation,
                        dims: base.size,
                        preview_scale: 1,
                    },
                );
            }
        }
        self.invalidate_final_prepare_decision(ctx);
        if crate::perf::is_enabled() {
            crate::perf::event(
                "fs",
                "final_effect_worker",
                None,
                self.input_seq,
                &[
                    ("request_id", pending.gpu_diagnostics.request_id().into()),
                    (
                        "context",
                        self.projected_viewer_context_id().serial().into(),
                    ),
                    ("idx", idx.into()),
                    ("pipeline", "edit".into()),
                    ("worker", true.into()),
                    ("display_takeover", display.into()),
                    ("prefetch", (!display).into()),
                    ("complete", true.into()),
                    ("ms", (worker_ms + load_texture_ms).into()),
                    ("worker_ms", worker_ms.into()),
                    ("clamp_ms", out.clamp_ms.into()),
                    ("load_texture_ms", load_texture_ms.into()),
                    ("upload_ms", load_texture_ms.into()),
                    ("w", base.size[0].into()),
                    ("h", base.size[1].into()),
                ],
            );
        }
        true
    }

    fn shared_prepared_edit_texture(
        &self,
        ctx: &egui::Context,
        key: FinalCompositeKey,
        pixels: &Arc<egui::ColorImage>,
        options: egui::TextureOptions,
    ) -> Option<egui::TextureHandle> {
        let texture = if options == DISPLAY_IMAGE_TEXTURE_OPTIONS {
            self.shared_resident_texture_for_edit(ctx, &key.edit_key, pixels)
                .map(|r| r.source_texture().clone())
        } else {
            None
        }
        .or_else(|| {
            self.edit_result_cache
                .get(&key.edit_key)
                .filter(|e| Arc::ptr_eq(&e.pixels, pixels))
                .and_then(|e| e.texture.clone())
        })?;
        let matches = ctx
            .tex_manager()
            .read()
            .meta(texture.id())
            .is_some_and(|meta| meta.options == options && meta.size == pixels.size);
        (matches && self.display_texture_matches_page(&texture, key.edit_key.idx))
            .then_some(texture)
    }

    pub(super) fn prepared_edit_page(&self, idx: usize) -> bool {
        self.local_adjust_pages.contains(&idx)
            || self.conceal_pages.contains(&idx)
            || self.comic_pages.contains(&idx)
    }

    fn prepared_edit_conflict(&self, idx: usize, page_key: &str) -> bool {
        let context = self.projected_viewer_context_id();
        self.commit_pending_matches_current_erase_key(idx)
            || self.current_edit_stage_producer_pending(idx)
            || self.comic_bake_pending.contains_key(&idx)
            || self
                .local_adjust_lut_pending
                .as_ref()
                .is_some_and(|p| p.owner.context == context && p.owner.target.page_key == page_key)
            || self
                .local_adjust_segmentation_pending
                .as_ref()
                .is_some_and(|p| p.owner.context == context && p.owner.target.page_key == page_key)
    }

    /// Resolve missing stages before choosing the final input. A cached final
    /// or AI result does not substitute for normalization/CPU stage completion
    /// while the ordinary local producer can still see its lower source.
    fn prepared_edit_input(&self, idx: usize) -> Option<PreparedEditInput> {
        if self.fs_entry_is_animated(idx) || self.comic_demo_enabled {
            return None;
        }
        let page_key = self.page_path_key(idx)?;
        if self.prepared_edit_conflict(idx, &page_key) {
            return None;
        }
        let edit_key = self.current_edit_result_key(idx);
        if self
            .final_effect_pending
            .keys()
            .any(|key| key.edit_key == edit_key)
        {
            return None;
        }
        let params = self.effective_params(idx);
        let lut = self.resolved_creative_lut(params);
        let lower = self.current_local_adjust_source_pixels(idx);
        let needs_local = !self.local_adjust_is_materialized(idx);
        let needs_conceal = self.conceal_pages.contains(&idx)
            && !self
                .conceal_cache
                .get(&idx)
                .is_some_and(|e| e.generation == self.conceal_generation);
        let build_stages = lower.is_some() && (needs_local || needs_conceal);
        let complete = self
            .exact_complete_final_composite_entry(idx)
            .filter(|(_, entry)| {
                !build_stages
                    || lower
                        .as_ref()
                        .is_some_and(|source| source.size == entry.source_size)
            });
        let ai = self.final_ai_cache.iter().find(|(key, entry)| {
            key.edit_key == edit_key
                && self.final_ai_key_for_pixels(edit_key, entry.source_size, params) == Some(**key)
                && (!build_stages
                    || lower
                        .as_ref()
                        .is_some_and(|source| source.size == entry.source_size))
        });
        let final_input = if let Some((_, entry)) = complete {
            PreparedFinalInput::Completed {
                pixels: entry.pixels.clone(),
                basis: entry.display_basis,
            }
        } else if let Some((_, entry)) = ai {
            PreparedFinalInput::AfterAi(entry.pixels.clone())
        } else {
            PreparedFinalInput::Edit
        };
        let mut local = None;
        let mut conceal = None;
        let (stage, source) = if !build_stages && let Some((_, entry)) = complete {
            (InputStage::Final, entry.pixels.clone())
        } else if !build_stages && let Some((_, entry)) = ai {
            (InputStage::AfterAi, entry.pixels.clone())
        } else if !build_stages && let Some(entry) = self.edit_result_cache.get(&edit_key) {
            (InputStage::Edit, entry.pixels.clone())
        } else if !build_stages
            && let Some(entry) = self
                .conceal_cache
                .get(&idx)
                .filter(|e| e.generation == self.conceal_generation)
        {
            (InputStage::Conceal, entry.pixels.clone())
        } else {
            let (stage, source) =
                if !needs_local && let Some(pixels) = self.current_local_adjust_pixels(idx) {
                    (InputStage::Local, pixels)
                } else {
                    if needs_local {
                        local = Some(match self.local_adjust_page_layers.get(&idx) {
                            Some(layers) => LocalMaterializeLayers::Memory(layers.clone()),
                            None => LocalMaterializeLayers::Database {
                                path: crate::local_adjust_db::LocalAdjustDb::db_path(),
                                page_key: page_key.clone(),
                            },
                        });
                    }
                    if self.mask_pages.contains(&idx) {
                        (InputStage::Erase, self.current_erase_result_pixels(idx)?)
                    } else {
                        (InputStage::Raw, self.current_raw_source_pixels(idx)?)
                    }
                };
            if self.conceal_pages.contains(&idx) {
                conceal = Some((
                    crate::conceal_db::ConcealDb::db_path(),
                    self.current_conceal_preset_from_settings(),
                ));
            }
            (stage, source)
        };
        let source_size = complete
            .map(|(_, entry)| entry.source_size)
            .or_else(|| ai.map(|(_, entry)| entry.source_size))
            .unwrap_or(source.size);
        if matches!(final_input, PreparedFinalInput::Edit)
            && self
                .final_ai_key_for_pixels(edit_key, source_size, params)
                .is_some_and(|key| !self.final_ai_failed.contains(&key))
        {
            return None;
        }
        let plan = if let Some((_, entry)) = ai.filter(|_| complete.is_none()) {
            build_final_composite_plan_after_ai(params, lut, entry.used_upscale)
        } else {
            build_final_composite_plan_without_ai(params, lut)
        };
        let comic = if self.comic_pages.contains(&idx) {
            let doc = self.comic_docs.get(&page_key).cloned();
            let completed = complete.is_some_and(|(_, base)| {
                self.comic_cache.get(&idx).is_some_and(|entry| {
                    Arc::ptr_eq(&entry.base, &base.pixels)
                        && entry.comic_gen == self.comic_generation
                        && entry.preview_scale == 1
                })
            });
            if completed
                || doc.as_ref().is_some_and(|doc| {
                    doc.is_empty()
                        || !fonts_satisfy(
                            doc,
                            self.comic_fonts.fonts(),
                            self.comic_fonts.loaded_keys(),
                        )
                })
                || self.comic_bake_inflight.load(Ordering::Relaxed) >= 2
            {
                None
            } else if let Some((w, h)) = self.comic_source_dims_for_idx(idx) {
                Some(ComicInput {
                    doc,
                    path: crate::comic_db::ComicDb::db_path(),
                    generation: self.comic_generation,
                    fonts: self.comic_fonts.fonts().clone(),
                    loaded_keys: self.comic_fonts.loaded_keys().clone(),
                    canonical_size: [w as usize, h as usize],
                    stamps: self.comic_stamp_cache.clone(),
                })
            } else {
                None
            }
        } else {
            None
        };
        let resident_texture = match stage {
            InputStage::Raw | InputStage::Erase | InputStage::Final => true,
            InputStage::AfterAi => false,
            InputStage::Edit => self
                .edit_result_cache
                .get(&edit_key)
                .is_some_and(|entry| entry.texture.is_some()),
            InputStage::Local => self.current_local_adjust_material(idx).is_some(),
            InputStage::Conceal => self.current_conceal_material(idx).is_some(),
        };
        let output = if local.is_some()
            || conceal.is_some()
            || (complete.is_none() && (!identity_plan(&plan) || !resident_texture))
        {
            PreparedOutput::Final
        } else {
            PreparedOutput::AnnotationOnly
        };
        if matches!(output, PreparedOutput::AnnotationOnly) && comic.is_none() {
            return None;
        }
        let canonical = complete
            .map(|(_, e)| e.canonical_dims)
            .or_else(|| ai.map(|(_, e)| e.canonical_dims))
            .unwrap_or_else(|| self.canonical_source_dimensions(idx));
        let pdf = self.current_pdf_page_metadata_snapshot(idx);
        let accepted_basis = match stage {
            InputStage::Raw => self
                .source_material_for_idx(idx)
                .map(|resource| resource.display_basis()),
            InputStage::Erase => self
                .erase_result_cache
                .get(&self.current_erase_result_key(idx))
                .map(|entry| entry.display_basis),
            InputStage::Local => self
                .local_adjust_cache
                .get(&self.current_local_adjust_key(idx))
                .map(|entry| entry.display_basis),
            InputStage::Conceal => self
                .conceal_cache
                .get(&idx)
                .map(|entry| entry.display_basis),
            InputStage::Edit => self
                .edit_result_cache
                .get(&edit_key)
                .map(|entry| entry.display_basis),
            InputStage::Final => complete.map(|(_, entry)| entry.display_basis),
            InputStage::AfterAi => ai.map(|(_, entry)| {
                self.accept_raster_display_basis(
                    idx,
                    material_sampler_size(entry.pixels.size),
                    entry.canonical_dims,
                    entry.source_size,
                    pdf,
                    true,
                )
            }),
        };
        let basis = accepted_basis.unwrap_or_else(|| {
            self.accept_raster_display_basis(
                idx,
                material_sampler_size(source.size),
                canonical,
                source.size,
                pdf,
                false,
            )
        });
        Some(PreparedEditInput {
            key: self.final_composite_key_for_pixels(edit_key, source_size, params),
            local_key: self.current_local_adjust_key(idx),
            page_key,
            source,
            stage,
            source_size,
            params: params.clone(),
            plan,
            output,
            final_input,
            local,
            conceal,
            comic,
            canonical,
            pdf,
            basis,
        })
    }

    pub(super) fn prepared_edit_input_is_current(&self, input: &PreparedEditInput) -> bool {
        self.validate_prepared_edit_input(input).is_ok()
    }

    pub(super) fn validate_prepared_edit_input(
        &self,
        input: &PreparedEditInput,
    ) -> Result<(), PreparedEditReject> {
        let idx = input.key.edit_key.idx;
        self.validate_final_prepare_identity(input.key.edit_key)?;
        if input.key.edit_key != self.current_edit_result_key(idx) {
            return Err(PreparedEditReject::EditGeneration);
        }
        if input.params != *self.effective_params(idx) {
            return Err(PreparedEditReject::Params);
        }
        if self.page_path_key(idx).as_ref() != Some(&input.page_key) {
            return Err(PreparedEditReject::PageIdentity);
        }
        if self.prepared_edit_conflict(idx, &input.page_key) {
            return Err(PreparedEditReject::ProducerConflict);
        }
        if let Some(LocalMaterializeLayers::Memory(layers)) = &input.local
            && self.local_adjust_page_layers.get(&idx) != Some(layers)
        {
            return Err(PreparedEditReject::LayersChanged);
        }
        if input.local.is_none()
            && !self.local_adjust_is_materialized(idx)
            && self.current_local_adjust_source_pixels(idx).is_some()
        {
            return Err(PreparedEditReject::NormalizationRequired);
        }
        let final_current = match &input.final_input {
            PreparedFinalInput::Edit => true,
            PreparedFinalInput::Completed { pixels, .. } => self
                .exact_complete_final_composite_entry(idx)
                .is_some_and(|(key, entry)| key == input.key && Arc::ptr_eq(&entry.pixels, pixels)),
            PreparedFinalInput::AfterAi(pixels) => {
                self.final_ai_cache.iter().any(|(key, entry)| {
                    key.edit_key == input.key.edit_key
                        && entry.source_size == input.source_size
                        && self.final_ai_key_for_pixels(
                            key.edit_key,
                            entry.source_size,
                            self.effective_params(idx),
                        ) == Some(*key)
                        && Arc::ptr_eq(&entry.pixels, pixels)
                })
            }
        };
        if !final_current {
            return Err(PreparedEditReject::FinalBaseChanged);
        }
        let current = self
            .current_prepared_edit_source(input)
            .ok_or(PreparedEditReject::SourceMissing)?;
        if !Arc::ptr_eq(&current, &input.source) {
            return Err(PreparedEditReject::SourceArcChanged);
        }
        Ok(())
    }

    fn current_prepared_edit_source(
        &self,
        input: &PreparedEditInput,
    ) -> Option<Arc<egui::ColorImage>> {
        let idx = input.key.edit_key.idx;
        match input.stage {
            InputStage::Raw => self.current_raw_source_pixels(idx),
            InputStage::Erase => self.current_erase_result_pixels(idx),
            InputStage::Local => self.current_local_adjust_pixels(idx),
            InputStage::Conceal => self.conceal_cache.get(&idx).map(|e| e.pixels.clone()),
            InputStage::Edit => self
                .edit_result_cache
                .get(&input.key.edit_key)
                .map(|e| e.pixels.clone()),
            InputStage::AfterAi => self.final_ai_cache.iter().find_map(|(key, e)| {
                (key.edit_key == input.key.edit_key
                    && self.final_ai_key_for_pixels(
                        key.edit_key,
                        e.source_size,
                        self.effective_params(idx),
                    ) == Some(*key))
                .then(|| e.pixels.clone())
            }),
            InputStage::Final => self
                .exact_complete_final_composite_entry(idx)
                .map(|(_, e)| e.pixels.clone()),
        }
    }

    pub(super) fn spawn_prepared_edit_job(
        &mut self,
        ctx: &egui::Context,
        idx: usize,
        unit: FinalPrepareUnit,
        reclaim_attempted: &mut bool,
    ) -> bool {
        let Some(input) = self.prepared_edit_input(idx) else {
            return false;
        };
        let Some(bytes) = output_bytes(&input) else {
            return false;
        };
        let wake = self.final_prepare_wake(ctx);
        let mut admission = self
            .final_prepare_cpu
            .try_start(unit.clone(), bytes, wake.clone());
        if !*reclaim_attempted
            && let Err(FinalPrepareStartDenied::Reservation { units, .. }) = &admission
        {
            *reclaim_attempted = true;
            self.reclaim_previous_final_results(units, &unit);
            if !self.prepared_edit_input_is_current(&input) {
                return false;
            }
            admission = self.final_prepare_cpu.try_start(unit.clone(), bytes, wake);
        }
        let (permit, reservation) = match admission {
            Ok(leases) => leases,
            Err(denied) => {
                self.record_final_cpu_admission_denied(&unit, bytes, &denied);
                return false;
            }
        };
        // No intervening UI work: the same running-count owner used by ordinary
        // comic workers includes this lease, cancelled and orphan workers alike.
        let comic_guard = input.comic.as_ref().map(|_| {
            self.comic_bake_inflight.fetch_add(1, Ordering::Relaxed);
            ComicBakeInflightGuard(self.comic_bake_inflight.clone())
        });
        let input = Arc::new(input);
        let worker_input = input.clone();
        let worker_reservation = reservation.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = cancel.clone();
        let request_id = final_prepare::next_final_prepare_request_id();
        let context = unit.context.serial();
        let (tx, rx) = mpsc::channel();
        #[cfg(test)]
        let test_registration = final_prepare::tests::attach_worker(&worker_cancel);
        let diagnostic = self.diagnostic_post_ai(input.key, input.source_size, Arc::clone(&cancel));
        let worker_diagnostic = diagnostic.clone();
        let spawn = std::thread::Builder::new()
            .name(format!("miv-edit-prepare-{idx}"))
            .spawn(move || {
                let _diagnostic = DiagnosticEffectWorker::new(worker_diagnostic);
                let started = crate::perf::is_enabled().then(std::time::Instant::now);
                // The trace drops after all execution leases, including unwind.
                let _finished = WorkerTrace {
                    request_id,
                    context,
                    idx,
                    started,
                    cancel: worker_cancel.clone(),
                };
                let _permit = permit;
                let _comic_guard = comic_guard;
                // Move captures into later locals so cancelled/undeliverable
                // output releases its reservation before the permit's wake.
                let worker_reservation = worker_reservation;
                let worker_input = worker_input;
                #[cfg(test)]
                let _test_registration = test_registration;
                let output = execute(&worker_input, &worker_cancel);
                let elapsed_ms =
                    started.map_or(0.0, |started| started.elapsed().as_secs_f64() * 1000.0);
                if !worker_cancel.load(Ordering::Relaxed) {
                    let clamp_ms = output.as_ref().map_or(0.0, |out| out.clamp_ms);
                    let size = output
                        .as_ref()
                        .ok()
                        .and_then(|out| out.final_pixels.as_ref())
                        .map_or([0, 0], |pixels| pixels.size);
                    let key = worker_input.key;
                    diagnostic_stage(&_diagnostic.0, JobStage::FinalEffectPending);
                    let sent = tx.send(PreparedEditPixels {
                        input: worker_input,
                        output,
                        _reservation: worker_reservation,
                        elapsed_ms,
                    });
                    if sent.is_ok() && crate::perf::is_enabled() {
                        crate::perf::event(
                            "fs",
                            "final_prepare_cpu_ready",
                            None,
                            0,
                            &[
                                ("request_id", request_id.into()),
                                ("context", context.into()),
                                ("idx", idx.into()),
                                ("pipeline", "edit".into()),
                                ("requested_position", "next".into()),
                                ("demand_position", "unknown".into()),
                                ("items_generation", key.edit_key.items_generation.into()),
                                ("source_generation", key.edit_key.source_gen.into()),
                                ("params_hash", key.params_hash.into()),
                                ("worker_ms", elapsed_ms.into()),
                                ("clamp_ms", clamp_ms.into()),
                                ("w", size[0].into()),
                                ("h", size[1].into()),
                                ("reservation_bytes", bytes.into()),
                            ],
                        );
                    }
                }
            });
        if let Err(error) = spawn {
            crate::logger::log(format!("[edit-prepare] spawn failed idx={idx}: {error}"));
            self.show_feedback_toast("編集画像の先読みを開始できませんでした".to_owned());
            return false;
        }
        // A new page request supersedes an older generation of B at this
        // existing ownership boundary; its worker keeps any execution leases.
        self.cancel_prepared_edit_for_idx(idx, PreparedEditCancel::RequestSuperseded);
        if crate::perf::is_enabled() {
            crate::perf::event(
                "fs",
                "final_prepare_cpu_started",
                None,
                self.input_seq,
                &[
                    ("request_id", request_id.into()),
                    ("context", context.into()),
                    ("idx", idx.into()),
                    ("pipeline", "edit".into()),
                    ("requested_position", "next".into()),
                    ("demand_position", "next".into()),
                    ("items_generation", self.items_generation.into()),
                    ("params_hash", input.key.params_hash.into()),
                    ("source_generation", input.key.edit_key.source_gen.into()),
                    ("reservation_bytes", bytes.into()),
                    ("unit_pages", format!("{:?}", unit.pages).into()),
                ],
            );
        }
        self.final_effect_pending.insert(
            input.key,
            FinalEffectPending {
                diagnostic,
                pdf_metadata: input.pdf,
                canonical_dims: input.canonical,
                source_size: input.source_size,
                cancel,
                state: FinalEffectState::Computing(FinalEffectReceiver::Edited {
                    input: input.clone(),
                    rx,
                    _reservation: reservation,
                }),
                items_generation: self.items_generation,
                output_complete: true,
                nearest_sampler: input.plan.needs_nearest_sampler(),
                origin: FinalEffectOrigin::Speculative(unit),
                gpu_diagnostics: final_prepare_budget::FinalPrepareGpuDiagnostics::requested(
                    "next", request_id,
                ),
            },
        );
        true
    }
}

//! Viewer-owned temporary preview for the Similar metadata panel.
//!
//! Presentation rights, cached GPU resources, and the bounded worker have separate lifetimes.
//! Releasing the pointer ends presentation without discarding a valid preparation. Page, mode,
//! source, park, and close invalidations advance the owner generation and reject late results.

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, mpsc};

use eframe::egui;

/// One indexed page that the viewer may show while the primary pointer remains held.
///
/// Query rows and book-strip matches use the same presentation owner. Keeping this small DTO
/// separate from either result shape avoids giving the strip a second gesture/cancellation state.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SimilarPreviewCandidate {
    pub(crate) item_key: String,
    pub(crate) target: crate::similar_index::SimilarItemTarget,
    pub(crate) indexed_mtime: i64,
    pub(crate) indexed_file_size: i64,
}

impl SimilarPreviewCandidate {
    pub(crate) fn for_hit(hit: &crate::similar_index::QueryHit) -> Option<Self> {
        Some(Self {
            item_key: hit.item_key.clone(),
            target: crate::similar_index::target_for_hit(hit).cloned()?,
            indexed_mtime: hit.mtime,
            indexed_file_size: hit.file_size,
        })
    }

    pub(crate) fn for_book_match(page: &crate::similar_index::BookPageMatch) -> Option<Self> {
        Some(Self {
            item_key: page.other_item_key.clone(),
            target: page.other_target.clone()?,
            indexed_mtime: page.other_mtime,
            indexed_file_size: page.other_file_size,
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SimilarPreviewStamp {
    pub(crate) item_key: String,
    target: crate::similar_index::SimilarItemTarget,
    source_key: String,
    indexed_mtime: i64,
    indexed_file_size: i64,
    pdf_credential_revision: u64,
    pdf_viewport: crate::pdf_loader::PdfDisplayTarget,
}

impl SimilarPreviewStamp {
    pub(crate) fn for_hit(
        hit: &crate::similar_index::QueryHit,
        passwords: &crate::pdf_passwords::PdfPasswordStore,
        pdf_viewport: crate::pdf_loader::PdfDisplayTarget,
    ) -> Option<Self> {
        let candidate = SimilarPreviewCandidate::for_hit(hit)?;
        Some(Self::for_candidate(&candidate, passwords, pdf_viewport))
    }

    fn for_candidate(
        candidate: &SimilarPreviewCandidate,
        passwords: &crate::pdf_passwords::PdfPasswordStore,
        pdf_viewport: crate::pdf_loader::PdfDisplayTarget,
    ) -> Self {
        let target = candidate.target.clone();
        let source_path = match &target {
            crate::similar_index::SimilarItemTarget::File(path) => path,
            crate::similar_index::SimilarItemTarget::ZipPage { zip_path, .. } => zip_path,
            crate::similar_index::SimilarItemTarget::PdfPage { pdf_path, .. } => pdf_path,
        };
        let source_key = crate::path_key::normalize_keep_drive(source_path);
        let pdf_credential_revision = match &target {
            crate::similar_index::SimilarItemTarget::PdfPage { pdf_path, .. } => {
                passwords.credential_revision(pdf_path)
            }
            _ => 0,
        };
        Self {
            item_key: candidate.item_key.clone(),
            target,
            source_key,
            indexed_mtime: candidate.indexed_mtime,
            indexed_file_size: candidate.indexed_file_size,
            pdf_credential_revision,
            pdf_viewport,
        }
    }

    fn same_candidate(&self, other: &Self) -> bool {
        self.item_key == other.item_key && self.target == other.target
    }

    fn is_stale_against(&self, current: &Self) -> bool {
        if self.source_key != current.source_key {
            return false;
        }
        let source_version_changed = self.indexed_mtime != current.indexed_mtime
            || self.indexed_file_size != current.indexed_file_size;
        let credential_changed = self.pdf_credential_revision != current.pdf_credential_revision;
        source_version_changed
            || credential_changed
            || (self.same_candidate(current) && self != current)
    }

    fn with_current_credentials(&self, passwords: &crate::pdf_passwords::PdfPasswordStore) -> Self {
        let mut current = self.clone();
        if let crate::similar_index::SimilarItemTarget::PdfPage { pdf_path, .. } = &self.target {
            current.pdf_credential_revision = passwords.credential_revision(pdf_path);
        }
        current
    }

    fn credentials_are_current(&self, passwords: &crate::pdf_passwords::PdfPasswordStore) -> bool {
        *self == self.with_current_credentials(passwords)
    }

    fn resource_path(&self) -> &Path {
        match &self.target {
            crate::similar_index::SimilarItemTarget::File(path) => path,
            crate::similar_index::SimilarItemTarget::ZipPage { zip_path, .. } => zip_path,
            crate::similar_index::SimilarItemTarget::PdfPage { pdf_path, .. } => pdf_path,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SimilarPreviewLayoutMode {
    Single,
    Spread,
    Continuous,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SimilarPreviewSession {
    pub(crate) viewport: egui::ViewportId,
    pub(crate) items_generation: u64,
    pub(crate) page_idx: usize,
    pub(crate) page_slice: crate::page_split::PageSlice,
    pub(crate) layout_mode: SimilarPreviewLayoutMode,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SimilarPreviewInput {
    pub(crate) primary_down: bool,
    pub(crate) primary_released: bool,
    pub(crate) viewport_focused: bool,
}

impl SimilarPreviewInput {
    pub(crate) fn for_viewport(ctx: &egui::Context, viewport: egui::ViewportId) -> Self {
        ctx.input_for(viewport, |input| Self {
            primary_down: input.pointer.primary_down(),
            primary_released: input.events.iter().any(|event| {
                matches!(
                    event,
                    egui::Event::PointerButton {
                        button: egui::PointerButton::Primary,
                        pressed: false,
                        ..
                    }
                )
            }),
            viewport_focused: input.viewport().focused.unwrap_or(true),
        })
    }
}

#[derive(Clone)]
pub(crate) struct SimilarPreviewAsset {
    pub(crate) stamp: SimilarPreviewStamp,
    pub(crate) texture: egui::TextureHandle,
    /// Coordinate space for Original/100% scaling and pointer mapping.
    pub(crate) source_size: egui::Vec2,
    /// Optional layout aspect independent of the rendered texture resolution (PDF page box).
    pub(crate) layout_source_size: Option<egui::Vec2>,
    /// Viewer-local stable identity for derived paint resources.
    pub(crate) paint_resource_id: u64,
    source_version: SimilarPreviewSourceVersion,
    session: SimilarPreviewSession,
    owner_generation: u64,
}

impl SimilarPreviewAsset {
    pub(crate) fn paint_generation(&self) -> crate::gpu_lanczos::FullscreenPaintSourceGeneration {
        crate::gpu_lanczos::FullscreenPaintSourceGeneration {
            items: self.owner_generation,
            input: self.paint_resource_id,
        }
    }
}

pub(crate) struct SimilarPreviewPixels {
    pub(crate) display_name: String,
    pub(crate) pixels: Arc<egui::ColorImage>,
    /// Coordinate space for Original/100% scaling before the GPU texture clamp.
    pub(crate) source_dims: [usize; 2],
    /// PDF page-box aspect, kept separate from raster/native pixel coordinates.
    pub(crate) layout_dims: Option<[u32; 2]>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SimilarPreviewSourceVersion {
    modified: std::time::SystemTime,
    file_size: u64,
}

#[derive(Clone)]
struct SimilarPreviewRequest {
    stamp: SimilarPreviewStamp,
    passwords: crate::pdf_passwords::PdfPasswordStore,
    session: SimilarPreviewSession,
    owner_generation: u64,
    press_id: u64,
    cached_source_version: Option<SimilarPreviewSourceVersion>,
}

struct SimilarPreviewPending {
    request_id: u64,
    request: SimilarPreviewRequest,
    cancel: Arc<AtomicBool>,
    rx: mpsc::Receiver<Result<SimilarPreviewWorkerResult, String>>,
}

enum SimilarPreviewWorkerResult {
    Unchanged {
        source_version: SimilarPreviewSourceVersion,
    },
    Prepared {
        source_version: SimilarPreviewSourceVersion,
        pixels: SimilarPreviewPixels,
    },
}

#[cfg(test)]
pub(crate) struct SimilarPreviewTestCompletion {
    tx: mpsc::Sender<Result<SimilarPreviewWorkerResult, String>>,
}

#[cfg(test)]
impl SimilarPreviewTestCompletion {
    pub(crate) fn send_prepared(
        self,
        pixels: Arc<egui::ColorImage>,
        source_dims: [usize; 2],
        layout_dims: Option<[u32; 2]>,
        source_version: u64,
    ) -> Result<(), String> {
        self.tx
            .send(Ok(SimilarPreviewWorkerResult::Prepared {
                source_version: SimilarPreviewSourceVersion {
                    modified: std::time::UNIX_EPOCH
                        + std::time::Duration::from_secs(source_version),
                    file_size: source_version,
                },
                pixels: SimilarPreviewPixels {
                    display_name: "candidate".to_owned(),
                    pixels,
                    source_dims,
                    layout_dims,
                },
            }))
            .map_err(|_| "test preview receiver was dropped".to_owned())
    }
}

enum SimilarPreviewPreparation {
    Idle,
    Running(SimilarPreviewPending),
    Draining {
        pending: SimilarPreviewPending,
        next: Option<SimilarPreviewRequest>,
    },
    Failed {
        request: SimilarPreviewRequest,
        error: String,
    },
}

impl Default for SimilarPreviewPreparation {
    fn default() -> Self {
        Self::Idle
    }
}

#[derive(Default)]
enum SimilarPreviewGesture {
    #[default]
    Idle,
    Holding {
        stamp: SimilarPreviewStamp,
        session: SimilarPreviewSession,
        owner_generation: u64,
        press_id: u64,
        authorized_request_id: Option<u64>,
    },
}

#[derive(Default)]
pub(crate) struct SimilarPreviewState {
    session: Option<SimilarPreviewSession>,
    owner_generation: u64,
    gesture: SimilarPreviewGesture,
    preparation: SimilarPreviewPreparation,
    cached: Option<SimilarPreviewAsset>,
    next_request_id: u64,
    next_press_id: u64,
    #[cfg(test)]
    test_receivers:
        std::collections::VecDeque<mpsc::Receiver<Result<SimilarPreviewWorkerResult, String>>>,
    #[cfg(test)]
    test_senders:
        std::collections::VecDeque<mpsc::Sender<Result<SimilarPreviewWorkerResult, String>>>,
}

impl SimilarPreviewState {
    pub(crate) fn begin_press(
        &mut self,
        ctx: &egui::Context,
        hit: &crate::similar_index::QueryHit,
        passwords: &crate::pdf_passwords::PdfPasswordStore,
        pdf_viewport: crate::pdf_loader::PdfDisplayTarget,
        session: SimilarPreviewSession,
    ) -> bool {
        let Some(candidate) = SimilarPreviewCandidate::for_hit(hit) else {
            return false;
        };
        self.begin_candidate_press(ctx, &candidate, passwords, pdf_viewport, session)
    }

    pub(crate) fn begin_candidate_press(
        &mut self,
        ctx: &egui::Context,
        candidate: &SimilarPreviewCandidate,
        passwords: &crate::pdf_passwords::PdfPasswordStore,
        pdf_viewport: crate::pdf_loader::PdfDisplayTarget,
        session: SimilarPreviewSession,
    ) -> bool {
        let stamp = SimilarPreviewStamp::for_candidate(candidate, passwords, pdf_viewport);
        self.observe_session(session);
        self.observe_indexed_stamp(&stamp);
        let press_id = self.next_press_id.wrapping_add(1).max(1);
        self.next_press_id = press_id;
        self.gesture = SimilarPreviewGesture::Holding {
            stamp: stamp.clone(),
            session,
            owner_generation: self.owner_generation,
            press_id,
            authorized_request_id: None,
        };
        let cached_source_version = self
            .cached
            .as_ref()
            .filter(|asset| {
                asset.stamp == stamp
                    && asset.session == session
                    && asset.owner_generation == self.owner_generation
            })
            .map(|asset| asset.source_version.clone());
        self.request(
            ctx,
            SimilarPreviewRequest {
                stamp,
                passwords: passwords.clone(),
                session,
                owner_generation: self.owner_generation,
                press_id,
                cached_source_version,
            },
        );
        true
    }

    /// Feed the current Item query result into the preview freshness owner.
    ///
    /// Only a fresh `Ready` result carries indexed source-version evidence. `Preparing` is shown
    /// with a separate `last_ready` fallback in the panel, while empty/error states do not prove
    /// that a previously returned candidate resource was changed or removed.
    pub(crate) fn observe_query_result(
        &mut self,
        query: &crate::similar_index::ItemQuery,
        passwords: &crate::pdf_passwords::PdfPasswordStore,
        pdf_viewport: crate::pdf_loader::PdfDisplayTarget,
    ) {
        let crate::similar_index::ItemQuery::Ready(matches) = query else {
            return;
        };
        for hit in &matches.hits {
            self.observe_ready_hit(hit, passwords, pdf_viewport);
        }
    }

    /// Feed one hit from a fresh Ready query into the owner without doing filesystem work.
    fn observe_ready_hit(
        &mut self,
        hit: &crate::similar_index::QueryHit,
        passwords: &crate::pdf_passwords::PdfPasswordStore,
        pdf_viewport: crate::pdf_loader::PdfDisplayTarget,
    ) {
        let Some(stamp) = SimilarPreviewStamp::for_hit(hit, passwords, pdf_viewport) else {
            return;
        };
        self.observe_indexed_stamp(&stamp);
    }

    pub(crate) fn presentation_for_frame(
        &mut self,
        ctx: &egui::Context,
        session: SimilarPreviewSession,
        input: SimilarPreviewInput,
        passwords: &crate::pdf_passwords::PdfPasswordStore,
    ) -> Option<SimilarPreviewAsset> {
        self.observe_session(session);
        let stale_credential = match &self.gesture {
            SimilarPreviewGesture::Holding { stamp, .. }
                if !stamp.credentials_are_current(passwords) =>
            {
                Some(stamp.with_current_credentials(passwords))
            }
            SimilarPreviewGesture::Holding { .. } | SimilarPreviewGesture::Idle => None,
        };
        if let Some(stamp) = stale_credential {
            self.invalidate_updated_source(&stamp);
        }
        if input.primary_released || !input.primary_down || !input.viewport_focused {
            self.end_gesture();
        }
        self.poll_worker(ctx, passwords);
        let SimilarPreviewGesture::Holding {
            stamp,
            session: owner_session,
            owner_generation,
            authorized_request_id: Some(_),
            ..
        } = &self.gesture
        else {
            return None;
        };
        self.cached
            .as_ref()
            .filter(|asset| {
                asset.stamp == *stamp
                    && asset.session == *owner_session
                    && asset.owner_generation == *owner_generation
            })
            .cloned()
    }

    /// End only the current presentation right. Preparation and a valid cache keep running.
    pub(crate) fn end_gesture(&mut self) {
        self.gesture = SimilarPreviewGesture::Idle;
    }

    /// Page, mode, source, park, and close transitions invalidate the owning viewer generation.
    pub(crate) fn invalidate(&mut self) {
        let has_live_scope = self.session.is_some()
            || !matches!(self.gesture, SimilarPreviewGesture::Idle)
            || self.cached.is_some()
            || matches!(
                &self.preparation,
                SimilarPreviewPreparation::Running(_)
                    | SimilarPreviewPreparation::Failed { .. }
                    | SimilarPreviewPreparation::Draining { next: Some(_), .. }
            );
        if !has_live_scope {
            return;
        }
        self.owner_generation = self.owner_generation.wrapping_add(1).max(1);
        self.session = None;
        self.gesture = SimilarPreviewGesture::Idle;
        self.cached = None;
        self.cancel_preparation(None);
    }

    /// Drain a bounded worker independently of whether this viewer is currently rendered.
    pub(crate) fn poll_background(
        &mut self,
        ctx: &egui::Context,
        passwords: &crate::pdf_passwords::PdfPasswordStore,
    ) {
        if matches!(
            self.preparation,
            SimilarPreviewPreparation::Running(_) | SimilarPreviewPreparation::Draining { .. }
        ) {
            self.poll_worker(ctx, passwords);
        }
    }

    pub(crate) fn has_active_gesture(&self) -> bool {
        matches!(self.gesture, SimilarPreviewGesture::Holding { .. })
    }

    pub(crate) fn cached_paint_resource_id(&self) -> Option<u64> {
        self.cached.as_ref().map(|asset| asset.paint_resource_id)
    }

    pub(crate) fn cached_texture(&self) -> Option<&egui::TextureHandle> {
        self.cached.as_ref().map(|asset| &asset.texture)
    }

    #[cfg(test)]
    pub(crate) fn has_pending_worker_for_test(&self) -> bool {
        matches!(
            self.preparation,
            SimilarPreviewPreparation::Running(_) | SimilarPreviewPreparation::Draining { .. }
        )
    }

    /// Queue a deterministic completion receiver for the next production press path.
    ///
    /// Unlike [`Self::begin_test_press`], this does not create a gesture. Integration tests must
    /// first drive the real widget/dispatcher, verify the resulting active candidate, and only
    /// then send the returned completion. That keeps a missing press from being hidden by the
    /// test seam while leaving filesystem decoding to the dedicated worker tests below.
    #[cfg(test)]
    pub(crate) fn queue_test_completion_for_next_press(&mut self) -> SimilarPreviewTestCompletion {
        let (tx, rx) = mpsc::channel();
        self.test_receivers.push_back(rx);
        SimilarPreviewTestCompletion { tx }
    }

    #[cfg(test)]
    pub(crate) fn active_candidate_for_test(
        &self,
    ) -> Option<(SimilarPreviewCandidate, SimilarPreviewSession)> {
        let SimilarPreviewGesture::Holding { stamp, session, .. } = &self.gesture else {
            return None;
        };
        Some((
            SimilarPreviewCandidate {
                item_key: stamp.item_key.clone(),
                target: stamp.target.clone(),
                indexed_mtime: stamp.indexed_mtime,
                indexed_file_size: stamp.indexed_file_size,
            },
            *session,
        ))
    }

    fn observe_session(&mut self, session: SimilarPreviewSession) {
        if self.session == Some(session) {
            return;
        }
        self.owner_generation = self.owner_generation.wrapping_add(1).max(1);
        self.session = Some(session);
        self.gesture = SimilarPreviewGesture::Idle;
        self.cached = None;
        self.cancel_preparation(None);
    }

    fn observe_indexed_stamp(&mut self, stamp: &SimilarPreviewStamp) {
        let changed = self
            .cached
            .as_ref()
            .is_some_and(|asset| asset.stamp.is_stale_against(stamp))
            || matches!(
                &self.gesture,
                SimilarPreviewGesture::Holding { stamp: current, .. }
                    if current.is_stale_against(stamp)
            )
            || matches!(
                &self.preparation,
                SimilarPreviewPreparation::Running(pending)
                    if pending.request.stamp.is_stale_against(stamp)
            )
            || matches!(
                &self.preparation,
                SimilarPreviewPreparation::Draining { next: Some(next), .. }
                    if next.stamp.is_stale_against(stamp)
            )
            || matches!(
                &self.preparation,
                SimilarPreviewPreparation::Failed { request, .. }
                    if request.stamp.is_stale_against(stamp)
            );
        if changed {
            self.invalidate_updated_source(stamp);
        }
    }

    fn invalidate_updated_source(&mut self, current: &SimilarPreviewStamp) {
        if self
            .cached
            .as_ref()
            .is_some_and(|asset| asset.stamp.is_stale_against(current))
        {
            self.cached = None;
        }
        if matches!(
            &self.gesture,
            SimilarPreviewGesture::Holding { stamp, .. } if stamp.is_stale_against(current)
        ) {
            self.gesture = SimilarPreviewGesture::Idle;
        }
        match std::mem::take(&mut self.preparation) {
            SimilarPreviewPreparation::Running(pending)
                if pending.request.stamp.is_stale_against(current) =>
            {
                pending.cancel.store(true, Ordering::Relaxed);
                self.preparation = SimilarPreviewPreparation::Draining {
                    pending,
                    next: None,
                };
            }
            SimilarPreviewPreparation::Draining { pending, mut next } => {
                if next
                    .as_ref()
                    .is_some_and(|request| request.stamp.is_stale_against(current))
                {
                    next = None;
                }
                self.preparation = SimilarPreviewPreparation::Draining { pending, next };
            }
            SimilarPreviewPreparation::Failed { request, .. }
                if request.stamp.is_stale_against(current) => {}
            other => self.preparation = other,
        }
    }

    fn request(&mut self, ctx: &egui::Context, request: SimilarPreviewRequest) {
        match std::mem::take(&mut self.preparation) {
            SimilarPreviewPreparation::Idle | SimilarPreviewPreparation::Failed { .. } => {
                self.start_worker(ctx, request);
            }
            SimilarPreviewPreparation::Running(pending) => {
                pending.cancel.store(true, Ordering::Relaxed);
                self.preparation = SimilarPreviewPreparation::Draining {
                    pending,
                    next: Some(request),
                };
            }
            SimilarPreviewPreparation::Draining { pending, .. } => {
                self.preparation = SimilarPreviewPreparation::Draining {
                    pending,
                    next: Some(request),
                };
            }
        }
        ctx.request_repaint();
    }

    fn cancel_preparation(&mut self, next: Option<SimilarPreviewRequest>) {
        match std::mem::take(&mut self.preparation) {
            SimilarPreviewPreparation::Idle | SimilarPreviewPreparation::Failed { .. } => {}
            SimilarPreviewPreparation::Running(pending)
            | SimilarPreviewPreparation::Draining { pending, .. } => {
                pending.cancel.store(true, Ordering::Relaxed);
                self.preparation = SimilarPreviewPreparation::Draining { pending, next };
            }
        }
    }

    fn start_worker(&mut self, ctx: &egui::Context, request: SimilarPreviewRequest) {
        let request_id = self.next_request_id.wrapping_add(1).max(1);
        self.next_request_id = request_id;
        let cancel = Arc::new(AtomicBool::new(false));

        #[cfg(test)]
        if let Some(rx) = self.test_receivers.pop_front() {
            self.preparation = SimilarPreviewPreparation::Running(SimilarPreviewPending {
                request_id,
                request,
                cancel,
                rx,
            });
            return;
        }

        let worker_cancel = cancel.clone();
        let worker_request = request.clone();
        let (tx, rx) = mpsc::channel();
        let repaint = ctx.clone();
        let spawned = std::thread::Builder::new()
            .name("similar-preview".to_owned())
            .spawn(move || {
                let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                    validate_or_prepare_similar_preview(&worker_request, worker_cancel.clone())
                }))
                .unwrap_or_else(|_| Err("画像の準備に失敗しました".to_owned()));
                let _ = tx.send(result);
                repaint.request_repaint_of(egui::ViewportId::ROOT);
            });
        if spawned.is_ok() {
            self.preparation = SimilarPreviewPreparation::Running(SimilarPreviewPending {
                request_id,
                request,
                cancel,
                rx,
            });
        } else {
            self.preparation = SimilarPreviewPreparation::Failed {
                request,
                error: "画像準備ワーカーを開始できませんでした".to_owned(),
            };
        }
    }

    fn poll_worker(
        &mut self,
        ctx: &egui::Context,
        passwords: &crate::pdf_passwords::PdfPasswordStore,
    ) {
        match std::mem::take(&mut self.preparation) {
            SimilarPreviewPreparation::Idle => {}
            failed @ SimilarPreviewPreparation::Failed { .. } => self.preparation = failed,
            SimilarPreviewPreparation::Running(pending) => match pending.rx.try_recv() {
                Err(mpsc::TryRecvError::Empty) => {
                    self.preparation = SimilarPreviewPreparation::Running(pending);
                }
                Err(mpsc::TryRecvError::Disconnected) => self.accept_completion(
                    ctx,
                    passwords,
                    pending,
                    Err("画像の準備が終了しました".to_owned()),
                ),
                Ok(result) => self.accept_completion(ctx, passwords, pending, result),
            },
            SimilarPreviewPreparation::Draining { pending, mut next } => {
                match pending.rx.try_recv() {
                    Err(mpsc::TryRecvError::Empty) => {
                        self.preparation = SimilarPreviewPreparation::Draining { pending, next };
                    }
                    Ok(_) | Err(mpsc::TryRecvError::Disconnected) => {
                        if let Some(next_request) = next.take()
                            && self.request_is_current(&next_request, passwords)
                        {
                            self.start_worker(ctx, next_request);
                        }
                    }
                }
            }
        }
    }

    fn request_is_current(
        &self,
        request: &SimilarPreviewRequest,
        passwords: &crate::pdf_passwords::PdfPasswordStore,
    ) -> bool {
        self.session == Some(request.session)
            && self.owner_generation == request.owner_generation
            && request.stamp.credentials_are_current(passwords)
    }

    fn accept_completion(
        &mut self,
        ctx: &egui::Context,
        passwords: &crate::pdf_passwords::PdfPasswordStore,
        pending: SimilarPreviewPending,
        result: Result<SimilarPreviewWorkerResult, String>,
    ) {
        let request = pending.request;
        if !self.request_is_current(&request, passwords) {
            return;
        }
        let accepted = match result {
            Ok(SimilarPreviewWorkerResult::Unchanged { source_version }) => self
                .cached
                .as_ref()
                .filter(|asset| {
                    asset.stamp == request.stamp
                        && asset.session == request.session
                        && asset.owner_generation == request.owner_generation
                        && asset.source_version == source_version
                })
                .cloned()
                .ok_or_else(|| "検証対象の画像キャッシュがありません".to_owned()),
            Ok(SimilarPreviewWorkerResult::Prepared {
                source_version,
                pixels,
            }) => {
                let source_size =
                    egui::vec2(pixels.source_dims[0] as f32, pixels.source_dims[1] as f32);
                let layout_source_size = pixels
                    .layout_dims
                    .map(|[width, height]| egui::vec2(width as f32, height as f32));
                let texture = ctx.load_texture(
                    format!(
                        "similar-preview:{}:{}",
                        request.stamp.item_key, pending.request_id
                    ),
                    pixels.pixels,
                    egui::TextureOptions::LINEAR,
                );
                Ok(SimilarPreviewAsset {
                    stamp: request.stamp.clone(),
                    texture,
                    source_size,
                    layout_source_size,
                    paint_resource_id: pending.request_id,
                    source_version,
                    session: request.session,
                    owner_generation: request.owner_generation,
                })
            }
            Err(error) => Err(error),
        };
        match accepted {
            Ok(asset) => {
                self.cached = Some(asset);
                if let SimilarPreviewGesture::Holding {
                    stamp,
                    session,
                    owner_generation,
                    press_id,
                    authorized_request_id,
                } = &mut self.gesture
                    && *stamp == request.stamp
                    && *session == request.session
                    && *owner_generation == request.owner_generation
                    && *press_id == request.press_id
                {
                    *authorized_request_id = Some(pending.request_id);
                }
                self.preparation = SimilarPreviewPreparation::Idle;
                ctx.request_repaint();
            }
            Err(error) => {
                if self
                    .cached
                    .as_ref()
                    .is_some_and(|asset| asset.stamp == request.stamp)
                {
                    self.cached = None;
                }
                self.preparation = SimilarPreviewPreparation::Failed { request, error };
            }
        }
    }

    #[cfg(test)]
    fn queue_test_worker(&mut self) -> mpsc::Sender<Result<SimilarPreviewWorkerResult, String>> {
        let (tx, rx) = mpsc::channel();
        self.test_receivers.push_back(rx);
        tx
    }

    #[cfg(test)]
    pub(crate) fn begin_test_press(
        &mut self,
        ctx: &egui::Context,
        hit: &crate::similar_index::QueryHit,
        passwords: &crate::pdf_passwords::PdfPasswordStore,
        pdf_viewport: crate::pdf_loader::PdfDisplayTarget,
        session: SimilarPreviewSession,
    ) -> bool {
        let (tx, rx) = mpsc::channel();
        self.test_senders.push_back(tx);
        self.test_receivers.push_back(rx);
        self.begin_press(ctx, hit, passwords, pdf_viewport, session)
    }

    #[cfg(test)]
    pub(crate) fn take_test_completion(&mut self) -> SimilarPreviewTestCompletion {
        SimilarPreviewTestCompletion {
            tx: self
                .test_senders
                .pop_front()
                .expect("begin_test_press must own a deterministic completion sender"),
        }
    }
}

impl Drop for SimilarPreviewState {
    fn drop(&mut self) {
        match &self.preparation {
            SimilarPreviewPreparation::Running(pending)
            | SimilarPreviewPreparation::Draining { pending, .. } => {
                pending.cancel.store(true, Ordering::Relaxed);
            }
            SimilarPreviewPreparation::Idle | SimilarPreviewPreparation::Failed { .. } => {}
        }
    }
}

fn source_version(stamp: &SimilarPreviewStamp) -> Result<SimilarPreviewSourceVersion, String> {
    let metadata = std::fs::metadata(stamp.resource_path())
        .map_err(|error| format!("画像の更新情報を取得できません: {error}"))?;
    let modified = metadata
        .modified()
        .map_err(|error| format!("画像の更新日時を取得できません: {error}"))?;
    Ok(SimilarPreviewSourceVersion {
        modified,
        file_size: metadata.len(),
    })
}

fn validate_or_prepare_similar_preview(
    request: &SimilarPreviewRequest,
    cancel: Arc<AtomicBool>,
) -> Result<SimilarPreviewWorkerResult, String> {
    validate_or_prepare_similar_preview_with(request, cancel, |request, cancel| {
        load_similar_preview_pixels(&request.stamp, &request.passwords, cancel, 0)
    })
}

fn validate_or_prepare_similar_preview_with(
    request: &SimilarPreviewRequest,
    cancel: Arc<AtomicBool>,
    prepare: impl FnOnce(
        &SimilarPreviewRequest,
        Arc<AtomicBool>,
    ) -> Result<SimilarPreviewPixels, String>,
) -> Result<SimilarPreviewWorkerResult, String> {
    let before = source_version(&request.stamp)?;
    if request.cached_source_version.as_ref() == Some(&before) {
        return Ok(SimilarPreviewWorkerResult::Unchanged {
            source_version: before,
        });
    }
    let pixels = prepare(request, cancel)?;
    let after = source_version(&request.stamp)?;
    if before != after {
        return Err("画像が準備中に更新されました".to_owned());
    }
    Ok(SimilarPreviewWorkerResult::Prepared {
        source_version: after,
        pixels,
    })
}

pub(crate) fn load_similar_preview_pixels(
    stamp: &SimilarPreviewStamp,
    passwords: &crate::pdf_passwords::PdfPasswordStore,
    cancel: Arc<AtomicBool>,
    pdf_context_epoch: u64,
) -> Result<SimilarPreviewPixels, String> {
    if cancel.load(Ordering::Relaxed) {
        return Err("画像の準備を取り消しました".to_owned());
    }
    let (display_name, pixels, source_dims, layout_dims) = match &stamp.target {
        crate::similar_index::SimilarItemTarget::File(path) => {
            let display_name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("?")
                .to_owned();
            let decoded = crate::canonical_image_loader::decode_canonical_image(
                crate::canonical_image_loader::CanonicalImageSource::File {
                    path,
                    verified_bytes: None,
                },
                crate::canonical_image_loader::CanonicalDecodeOptions::fullscreen(
                    crate::canonical_image_loader::AnimationPolicy::FirstFrameOnly,
                ),
            )
            .map_err(|error| error.to_string())?;
            let crate::canonical_image_loader::CanonicalImageDecode::Static(image) = decoded else {
                return Err("動画は画像として準備できません".to_owned());
            };
            let raster = image.into_gpu_raster();
            (
                display_name,
                Arc::new(raster.pixels),
                raster.source_dims,
                None,
            )
        }
        crate::similar_index::SimilarItemTarget::ZipPage {
            zip_path,
            entry_name: _,
        } => {
            let entry_name = crate::zip_loader::enumerate_image_entries_detailed(zip_path)
                .map_err(|error| error.to_string())?
                .entries
                .into_iter()
                .find(|entry| {
                    crate::similar_index::item_key_for_zip_page(zip_path, &entry.entry_name)
                        == stamp.item_key
                })
                .map(|entry| entry.entry_name)
                .ok_or_else(|| "ZIP内の画像が見つかりません".to_owned())?;
            if cancel.load(Ordering::Relaxed) {
                return Err("画像の準備を取り消しました".to_owned());
            }
            let display_name = crate::zip_loader::entry_basename(&entry_name).to_owned();
            let decoded = crate::canonical_image_loader::decode_canonical_image(
                crate::canonical_image_loader::CanonicalImageSource::ArchiveEntry {
                    archive_path: zip_path,
                    entry_name: &entry_name,
                },
                crate::canonical_image_loader::CanonicalDecodeOptions::fullscreen(
                    crate::canonical_image_loader::AnimationPolicy::FirstFrameOnly,
                ),
            )
            .map_err(|error| error.to_string())?;
            let crate::canonical_image_loader::CanonicalImageDecode::Static(image) = decoded else {
                return Err("動画は画像として準備できません".to_owned());
            };
            let raster = image.into_gpu_raster();
            (
                display_name,
                Arc::new(raster.pixels),
                raster.source_dims,
                None,
            )
        }
        crate::similar_index::SimilarItemTarget::PdfPage { pdf_path, page_num } => {
            let password = passwords.get(pdf_path);
            let display_name = format!(
                "{} - Page {}",
                pdf_path
                    .file_name()
                    .and_then(|name| name.to_str())
                    .unwrap_or("PDF"),
                page_num + 1
            );
            let rendered = crate::pdf_loader::render_page_for_display(
                pdf_path,
                *page_num,
                stamp.pdf_viewport,
                false,
                password.as_deref(),
                Some(cancel.clone()),
                crate::pdf_loader::JobPriority::Critical,
                pdf_context_epoch,
                crate::pdf_loader::CancelWaitPolicy::AbortOnCancel,
            )
            .map_err(|error| error.to_string())?;
            let rendered_dims = [
                rendered.image.width() as usize,
                rendered.image.height() as usize,
            ];
            let source_dims = crate::pdf_loader::canonical_pdf_raster_dims(rendered.content_type)
                .map(|[width, height]| [width as usize, height as usize])
                .unwrap_or(rendered_dims);
            let layout_dims = rendered
                .page_size_points
                .catalog_layout_dims()
                .map(|(width, height)| [width, height]);
            let image = crate::canonical_image_loader::clamp_dynamic_for_gpu(rendered.image);
            (
                display_name,
                Arc::new(crate::canonical_image_loader::dynamic_image_to_color_image(
                    &image,
                )),
                source_dims,
                layout_dims,
            )
        }
    };
    if cancel.load(Ordering::Relaxed) {
        return Err("画像の準備を取り消しました".to_owned());
    }
    Ok(SimilarPreviewPixels {
        display_name,
        pixels,
        source_dims,
        layout_dims,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::similar_db::ItemKind;
    use crate::similar_image::SimilarImageFormat;

    fn viewport() -> crate::pdf_loader::PdfDisplayTarget {
        crate::pdf_loader::PdfDisplayTarget {
            width_px: 800,
            height_px: 600,
            fit_mode: crate::pdf_loader::PdfDisplayFitMode::Page,
        }
    }

    fn session(page_idx: usize) -> SimilarPreviewSession {
        SimilarPreviewSession {
            viewport: egui::ViewportId::ROOT,
            items_generation: 10,
            page_idx,
            page_slice: crate::page_split::PageSlice::Full,
            layout_mode: SimilarPreviewLayoutMode::Single,
        }
    }

    fn hit(key: &str, mtime: i64) -> crate::similar_index::QueryHit {
        crate::similar_index::QueryHit {
            item_id: mtime as u64,
            item_key: key.to_owned(),
            kind: ItemKind::Image,
            container_key: None,
            page_index: None,
            distance: 1,
            band: crate::similar_index::MatchBand::NearlyIdentical,
            mtime,
            file_size: 20,
            width: 32,
            height: 24,
            format: SimilarImageFormat::Png,
            target: Some(crate::similar_index::SimilarItemTarget::File(
                std::path::PathBuf::from(key),
            )),
        }
    }

    fn fake_source_version(value: u64) -> SimilarPreviewSourceVersion {
        SimilarPreviewSourceVersion {
            modified: std::time::UNIX_EPOCH + std::time::Duration::from_secs(value),
            file_size: value,
        }
    }

    fn prepared(value: u64) -> SimilarPreviewWorkerResult {
        SimilarPreviewWorkerResult::Prepared {
            source_version: fake_source_version(value),
            pixels: SimilarPreviewPixels {
                display_name: "candidate".to_owned(),
                pixels: Arc::new(egui::ColorImage::new([2, 1], vec![egui::Color32::WHITE; 2])),
                source_dims: [20, 10],
                layout_dims: None,
            },
        }
    }

    fn input(down: bool) -> SimilarPreviewInput {
        SimilarPreviewInput {
            primary_down: down,
            primary_released: !down,
            viewport_focused: true,
        }
    }

    #[test]
    fn release_then_new_press_aggregate_ends_old_gesture_without_level_reactivation() {
        let ctx = egui::Context::default();
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let hit = hit(r"C:\a.png", 1);
        let mut state = SimilarPreviewState::default();
        let worker = state.queue_test_worker();
        state.begin_press(&ctx, &hit, &passwords, viewport(), session(0));
        worker.send(Ok(prepared(1))).unwrap();
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_some()
        );

        assert!(
            state
                .presentation_for_frame(
                    &ctx,
                    session(0),
                    SimilarPreviewInput {
                        primary_down: true,
                        primary_released: true,
                        viewport_focused: true,
                    },
                    &passwords,
                )
                .is_none(),
            "the release owns the old preview even when a later press leaves the level down"
        );
        assert!(!state.has_active_gesture());
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_none(),
            "a held level cannot recreate the ended gesture"
        );
    }

    #[test]
    fn repeated_invalidation_keeps_one_draining_owner_until_background_poll_reaps_it() {
        let ctx = egui::Context::default();
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let hit = hit(r"C:\a.png", 1);
        let mut state = SimilarPreviewState::default();
        let worker = state.queue_test_worker();
        state.begin_press(&ctx, &hit, &passwords, viewport(), session(0));

        state.invalidate();
        let invalidated_generation = state.owner_generation;
        assert!(matches!(
            state.preparation,
            SimilarPreviewPreparation::Draining { next: None, .. }
        ));
        state.invalidate();
        assert_eq!(state.owner_generation, invalidated_generation);
        assert!(matches!(
            state.preparation,
            SimilarPreviewPreparation::Draining { next: None, .. }
        ));

        worker.send(Ok(prepared(1))).unwrap();
        state.poll_background(&ctx, &passwords);
        assert!(matches!(state.preparation, SimilarPreviewPreparation::Idle));
        assert!(state.cached.is_none());
        assert!(!state.has_active_gesture());
    }

    #[test]
    fn stamp_tracks_pdf_credential_revision_and_viewport_without_password_material() {
        let path = std::path::PathBuf::from(r"C:\books\sample.pdf");
        let mut hit = hit("pdf-key", 10);
        hit.kind = ItemKind::PdfPage;
        hit.target = Some(crate::similar_index::SimilarItemTarget::PdfPage {
            pdf_path: path.clone(),
            page_num: 3,
        });
        let mut passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let before = SimilarPreviewStamp::for_hit(&hit, &passwords, viewport()).unwrap();
        passwords.bump_credential_revision_for_test(&path);
        let after = SimilarPreviewStamp::for_hit(&hit, &passwords, viewport()).unwrap();
        assert_ne!(before, after);
    }

    #[test]
    fn release_keeps_completion_hidden_until_a_new_press_is_validated() {
        let ctx = egui::Context::default();
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let hit = hit(r"C:\a.png", 1);
        let mut state = SimilarPreviewState::default();
        let first = state.queue_test_worker();
        assert!(state.begin_press(&ctx, &hit, &passwords, viewport(), session(0)));
        state.end_gesture();
        first.send(Ok(prepared(1))).unwrap();
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(false), &passwords)
                .is_none()
        );
        assert!(
            state.cached.is_some(),
            "released completion remains a resource cache"
        );

        let validate = state.queue_test_worker();
        assert!(state.begin_press(&ctx, &hit, &passwords, viewport(), session(0)));
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_none(),
            "cache alone must not authorize a new press"
        );
        validate
            .send(Ok(SimilarPreviewWorkerResult::Unchanged {
                source_version: fake_source_version(1),
            }))
            .unwrap();
        let asset = state
            .presentation_for_frame(&ctx, session(0), input(true), &passwords)
            .expect("validation authorizes this press");
        assert_eq!(asset.source_size, egui::vec2(20.0, 10.0));
    }

    #[test]
    fn focus_loss_ends_only_the_presentation_and_does_not_restart_on_level_input() {
        let ctx = egui::Context::default();
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let hit = hit(r"C:\a.png", 1);
        let mut state = SimilarPreviewState::default();
        let worker = state.queue_test_worker();
        state.begin_press(&ctx, &hit, &passwords, viewport(), session(0));
        worker.send(Ok(prepared(1))).unwrap();
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_some()
        );
        assert!(
            state
                .presentation_for_frame(
                    &ctx,
                    session(0),
                    SimilarPreviewInput {
                        primary_down: true,
                        primary_released: false,
                        viewport_focused: false,
                    },
                    &passwords,
                )
                .is_none()
        );
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_none(),
            "a held level after focus returns is not a new press edge"
        );
    }

    #[test]
    fn cached_a_press_drains_old_b_and_only_the_new_request_can_authorize() {
        let ctx = egui::Context::default();
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let hit_a = hit(r"C:\a.png", 1);
        let hit_b = hit(r"C:\b.png", 2);
        let mut state = SimilarPreviewState::default();

        let first_a = state.queue_test_worker();
        state.begin_press(&ctx, &hit_a, &passwords, viewport(), session(0));
        first_a.send(Ok(prepared(1))).unwrap();
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_some()
        );
        state.end_gesture();

        let old_b = state.queue_test_worker();
        state.begin_press(&ctx, &hit_b, &passwords, viewport(), session(0));
        let validate_a = state.queue_test_worker();
        state.begin_press(&ctx, &hit_a, &passwords, viewport(), session(0));
        assert!(matches!(
            state.preparation,
            SimilarPreviewPreparation::Draining { .. }
        ));
        old_b.send(Ok(prepared(2))).unwrap();
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_none()
        );
        validate_a
            .send(Ok(SimilarPreviewWorkerResult::Unchanged {
                source_version: fake_source_version(1),
            }))
            .unwrap();
        let shown = state
            .presentation_for_frame(&ctx, session(0), input(true), &passwords)
            .unwrap();
        assert_eq!(shown.stamp.item_key, hit_a.item_key);
    }

    #[test]
    fn source_or_session_invalidation_rejects_late_completion_even_after_aba() {
        let ctx = egui::Context::default();
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let hit = hit(r"C:\a.png", 1);
        let mut state = SimilarPreviewState::default();
        let stale = state.queue_test_worker();
        state.begin_press(&ctx, &hit, &passwords, viewport(), session(0));
        let first_generation = state.owner_generation;
        state.invalidate();
        state.observe_session(session(0));
        assert_ne!(state.owner_generation, first_generation);
        stale.send(Ok(prepared(1))).unwrap();
        state.poll_worker(&ctx, &passwords);
        assert!(state.cached.is_none());
        assert!(!state.has_active_gesture());
    }

    #[test]
    fn failure_is_typed_and_a_new_press_explicitly_retries() {
        let ctx = egui::Context::default();
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let hit = hit(r"C:\a.png", 1);
        let mut state = SimilarPreviewState::default();
        let failed = state.queue_test_worker();
        state.begin_press(&ctx, &hit, &passwords, viewport(), session(0));
        failed.send(Err("decode failed".to_owned())).unwrap();
        state.poll_worker(&ctx, &passwords);
        assert!(matches!(
            &state.preparation,
            SimilarPreviewPreparation::Failed { error, .. } if error == "decode failed"
        ));

        let retry = state.queue_test_worker();
        state.begin_press(&ctx, &hit, &passwords, viewport(), session(0));
        retry.send(Ok(prepared(2))).unwrap();
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_some()
        );
    }

    #[test]
    fn a_ready_query_stamp_change_invalidates_without_treating_absence_as_deletion() {
        let ctx = egui::Context::default();
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let mut state = SimilarPreviewState::default();
        let worker = state.queue_test_worker();
        let before = hit(r"C:\a.png", 1);
        state.begin_press(&ctx, &before, &passwords, viewport(), session(0));
        worker.send(Ok(prepared(1))).unwrap();
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_some()
        );
        let after = hit(r"C:\a.png", 2);
        state.observe_ready_hit(&after, &passwords, viewport());
        assert!(state.cached.is_none());
        assert!(!state.has_active_gesture());
    }

    #[test]
    fn current_pdf_credential_change_revokes_an_already_authorized_preview() {
        let ctx = egui::Context::default();
        let path = std::path::PathBuf::from(r"C:\books\sample.pdf");
        let mut pdf_hit = hit("pdf-key", 10);
        pdf_hit.kind = ItemKind::PdfPage;
        pdf_hit.target = Some(crate::similar_index::SimilarItemTarget::PdfPage {
            pdf_path: path.clone(),
            page_num: 3,
        });
        let mut passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let mut state = SimilarPreviewState::default();
        let worker = state.queue_test_worker();
        state.begin_press(&ctx, &pdf_hit, &passwords, viewport(), session(0));
        worker.send(Ok(prepared(1))).unwrap();
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_some()
        );

        passwords.bump_credential_revision_for_test(&path);
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_none()
        );
        assert!(state.cached.is_none());
        assert!(!state.has_active_gesture());
    }

    #[test]
    fn repeated_new_stamp_notification_does_not_erase_the_next_request_while_old_drains() {
        let ctx = egui::Context::default();
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let old_hit = hit(r"C:\a.png", 1);
        let new_hit = hit(r"C:\a.png", 2);
        let mut state = SimilarPreviewState::default();
        let old_worker = state.queue_test_worker();
        state.begin_press(&ctx, &old_hit, &passwords, viewport(), session(0));
        state.observe_ready_hit(&new_hit, &passwords, viewport());

        let new_worker = state.queue_test_worker();
        state.begin_press(&ctx, &new_hit, &passwords, viewport(), session(0));
        state.observe_ready_hit(&new_hit, &passwords, viewport());
        state.observe_ready_hit(&new_hit, &passwords, viewport());
        assert!(state.has_active_gesture());
        assert!(matches!(
            &state.preparation,
            SimilarPreviewPreparation::Draining { next: Some(next), .. }
                if next.stamp.indexed_mtime == 2
        ));

        old_worker.send(Ok(prepared(1))).unwrap();
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_none()
        );
        new_worker.send(Ok(prepared(2))).unwrap();
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_some()
        );
    }

    #[test]
    fn updating_cached_a_does_not_cancel_the_active_b_request() {
        let ctx = egui::Context::default();
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let hit_a = hit(r"C:\a.png", 1);
        let changed_a = hit(r"C:\a.png", 2);
        let hit_b = hit(r"C:\b.png", 3);
        let mut state = SimilarPreviewState::default();

        let worker_a = state.queue_test_worker();
        state.begin_press(&ctx, &hit_a, &passwords, viewport(), session(0));
        worker_a.send(Ok(prepared(1))).unwrap();
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_some()
        );
        state.end_gesture();

        let worker_b = state.queue_test_worker();
        state.begin_press(&ctx, &hit_b, &passwords, viewport(), session(0));
        state.observe_ready_hit(&changed_a, &passwords, viewport());
        assert!(state.cached.is_none(), "only stale A cache is removed");
        assert!(state.has_active_gesture(), "B gesture remains owned");
        assert!(matches!(
            &state.preparation,
            SimilarPreviewPreparation::Running(pending)
                if pending.request.stamp.item_key == hit_b.item_key
        ));

        worker_b.send(Ok(prepared(3))).unwrap();
        let shown = state
            .presentation_for_frame(&ctx, session(0), input(true), &passwords)
            .expect("B completes despite A's independent update");
        assert_eq!(shown.stamp.item_key, hit_b.item_key);
    }

    fn zip_hit(archive: &str, entry: &str, mtime: i64) -> crate::similar_index::QueryHit {
        crate::similar_index::QueryHit {
            item_id: mtime as u64,
            item_key: crate::similar_index::item_key_for_zip_page(Path::new(archive), entry),
            kind: ItemKind::ZipPage,
            container_key: Some(crate::path_key::normalize(Path::new(archive))),
            page_index: Some(0),
            distance: 1,
            band: crate::similar_index::MatchBand::NearlyIdentical,
            mtime,
            file_size: 100,
            width: 32,
            height: 24,
            format: SimilarImageFormat::Png,
            target: Some(crate::similar_index::SimilarItemTarget::ZipPage {
                zip_path: std::path::PathBuf::from(archive),
                entry_name: entry.to_owned(),
            }),
        }
    }

    #[test]
    fn outer_archive_update_invalidates_other_old_pages_but_keeps_current_version_pages() {
        let ctx = egui::Context::default();
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let page_a_new = zip_hit(r"C:\book.zip", "a.png", 2);
        let page_b_old = zip_hit(r"C:\book.zip", "b.png", 1);
        let page_b_new = zip_hit(r"C:\book.zip", "b.png", 2);

        let mut stale = SimilarPreviewState::default();
        let stale_worker = stale.queue_test_worker();
        stale.begin_press(&ctx, &page_b_old, &passwords, viewport(), session(0));
        stale.observe_ready_hit(&page_a_new, &passwords, viewport());
        assert!(!stale.has_active_gesture());
        assert!(matches!(
            stale.preparation,
            SimilarPreviewPreparation::Draining { .. }
        ));
        stale_worker.send(Ok(prepared(1))).unwrap();
        stale.poll_worker(&ctx, &passwords);
        assert!(stale.cached.is_none());

        let mut current = SimilarPreviewState::default();
        let current_worker = current.queue_test_worker();
        current.begin_press(&ctx, &page_b_new, &passwords, viewport(), session(0));
        current.observe_ready_hit(&page_a_new, &passwords, viewport());
        assert!(current.has_active_gesture());
        assert!(matches!(
            current.preparation,
            SimilarPreviewPreparation::Running(_)
        ));
        current_worker.send(Ok(prepared(2))).unwrap();
        assert!(
            current
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_some()
        );
    }

    #[test]
    fn same_archive_path_on_different_drives_is_not_the_same_source() {
        let ctx = egui::Context::default();
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let page_b_c = zip_hit(r"C:\books\book.zip", "b.png", 1);
        let page_a_d = zip_hit(r"D:\books\book.zip", "a.png", 2);
        let mut state = SimilarPreviewState::default();
        let worker = state.queue_test_worker();
        state.begin_press(&ctx, &page_b_c, &passwords, viewport(), session(0));
        state.observe_ready_hit(&page_a_d, &passwords, viewport());
        assert!(state.has_active_gesture());
        assert!(matches!(
            state.preparation,
            SimilarPreviewPreparation::Running(_)
        ));
        worker.send(Ok(prepared(1))).unwrap();
        assert!(
            state
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_some()
        );
    }

    fn item_query_with_hits(
        hits: Vec<crate::similar_index::QueryHit>,
    ) -> crate::similar_index::ItemQuery {
        crate::similar_index::ItemQuery::Ready(crate::similar_index::ItemMatches {
            origin: crate::similar_index::OriginItem {
                item_key: "origin".to_owned(),
                kind: ItemKind::Image,
                mtime: 0,
                file_size: 1,
                width: 1,
                height: 1,
                format: SimilarImageFormat::Png,
                target: None,
            },
            hits,
        })
    }

    #[test]
    fn only_fresh_ready_query_hits_are_preview_freshness_signals() {
        let ctx = egui::Context::default();
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let old_hit = hit(r"C:\query-candidate.png", 1);
        let new_hit = hit(r"C:\query-candidate.png", 2);

        let mut updated = SimilarPreviewState::default();
        let _worker = updated.queue_test_worker();
        updated.begin_press(&ctx, &old_hit, &passwords, viewport(), session(0));
        updated.observe_query_result(&item_query_with_hits(vec![new_hit]), &passwords, viewport());
        assert!(!updated.has_active_gesture());
        assert!(matches!(
            updated.preparation,
            SimilarPreviewPreparation::Draining { .. }
        ));

        for query in [
            crate::similar_index::ItemQuery::Preparing,
            item_query_with_hits(Vec::new()),
            crate::similar_index::ItemQuery::NotIndexed,
            crate::similar_index::ItemQuery::NoIndex,
            crate::similar_index::ItemQuery::Featureless,
            crate::similar_index::ItemQuery::Failed("query failed".to_owned()),
        ] {
            let mut unchanged = SimilarPreviewState::default();
            let _worker = unchanged.queue_test_worker();
            unchanged.begin_press(&ctx, &old_hit, &passwords, viewport(), session(0));
            unchanged.observe_query_result(&query, &passwords, viewport());
            assert!(
                unchanged.has_active_gesture(),
                "unexpected signal: {query:?}"
            );
            assert!(matches!(
                unchanged.preparation,
                SimilarPreviewPreparation::Running(_)
            ));
        }
    }

    fn request_for_file(
        path: &Path,
        cached_source_version: Option<SimilarPreviewSourceVersion>,
    ) -> SimilarPreviewRequest {
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let hit = hit(&path.to_string_lossy(), 0);
        SimilarPreviewRequest {
            stamp: SimilarPreviewStamp::for_hit(&hit, &passwords, viewport()).unwrap(),
            passwords,
            session: session(0),
            owner_generation: 1,
            press_id: 1,
            cached_source_version,
        }
    }

    #[test]
    fn real_metadata_validation_reuses_unchanged_and_decodes_changed_source() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("candidate.png");
        image::RgbaImage::from_pixel(2, 1, image::Rgba([1, 2, 3, 255]))
            .save(&path)
            .unwrap();
        let initial = request_for_file(&path, None);
        let initial_version = super::source_version(&initial.stamp).unwrap();
        let unchanged = request_for_file(&path, Some(initial_version));
        assert!(matches!(
            validate_or_prepare_similar_preview(&unchanged, Arc::new(AtomicBool::new(false))),
            Ok(SimilarPreviewWorkerResult::Unchanged { .. })
        ));

        image::RgbaImage::from_pixel(20, 10, image::Rgba([4, 5, 6, 255]))
            .save(&path)
            .unwrap();
        let changed =
            validate_or_prepare_similar_preview(&unchanged, Arc::new(AtomicBool::new(false)))
                .unwrap();
        let SimilarPreviewWorkerResult::Prepared { pixels, .. } = changed else {
            panic!("changed source must be decoded");
        };
        assert_eq!(pixels.source_dims, [20, 10]);
    }

    #[test]
    fn metadata_change_during_prepare_is_rejected() {
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("candidate.png");
        std::fs::write(&path, b"before").unwrap();
        let request = request_for_file(&path, None);
        let result = validate_or_prepare_similar_preview_with(
            &request,
            Arc::new(AtomicBool::new(false)),
            |_request, _cancel| {
                std::fs::write(&path, b"after-with-a-different-length").unwrap();
                Ok(SimilarPreviewPixels {
                    display_name: "candidate".to_owned(),
                    pixels: Arc::new(egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE])),
                    source_dims: [1, 1],
                    layout_dims: None,
                })
            },
        );
        assert!(matches!(result, Err(error) if error.contains("準備中に更新")));
    }

    #[test]
    fn moving_one_viewer_owner_and_dropping_another_does_not_steal_completion() {
        let ctx = egui::Context::default();
        let passwords = crate::pdf_passwords::PdfPasswordStore::empty_for_test();
        let hit_a = hit(r"C:\a.png", 1);
        let hit_b = hit(r"C:\b.png", 2);
        let mut viewer_a = SimilarPreviewState::default();
        let worker_a = viewer_a.queue_test_worker();
        viewer_a.begin_press(&ctx, &hit_a, &passwords, viewport(), session(0));
        let cancel_a = match &viewer_a.preparation {
            SimilarPreviewPreparation::Running(pending) => pending.cancel.clone(),
            _ => panic!("A worker must be running"),
        };

        let parked_a = Some(viewer_a);
        let mut viewer_b = SimilarPreviewState::default();
        let worker_b = viewer_b.queue_test_worker();
        viewer_b.begin_press(&ctx, &hit_b, &passwords, viewport(), session(1));
        let cancel_b = match &viewer_b.preparation {
            SimilarPreviewPreparation::Running(pending) => pending.cancel.clone(),
            _ => panic!("B worker must be running"),
        };
        drop(viewer_b);
        assert!(cancel_b.load(Ordering::Relaxed));
        assert!(!cancel_a.load(Ordering::Relaxed));
        assert!(worker_b.send(Ok(prepared(2))).is_err());

        let mut restored_a = parked_a.unwrap();
        worker_a.send(Ok(prepared(1))).unwrap();
        assert!(
            restored_a
                .presentation_for_frame(&ctx, session(0), input(true), &passwords)
                .is_some()
        );
    }
}

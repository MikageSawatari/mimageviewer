//! Private, disposable editor thumbnail worker. Its channels and cancellation
//! belong to one editor lifetime, never the mounted viewer's thumbnail queues.
use super::{
    book_composition_editor_prepare::EditorSourceInput,
    book_composition_runtime::BookCompositionSource, book_materialization::MaterializedBookRows,
};
use crate::{
    book_composition::{CompositionPageRef, PageDomain},
    book_scope::BookScopeKey,
    file_type_visibility::FileTypeVisibility,
    grid_item::GridItem,
};
use eframe::egui;
use std::{
    collections::{HashMap, HashSet},
    path::Path,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
        mpsc,
    },
};

const REQUEST_BUDGET: usize = 24;
const THUMB_PX: u32 = 192;
type Demand = Vec<(CompositionPageRef, usize)>;
type Reply = (CompositionPageRef, egui::TextureHandle);

enum WorkerReply {
    Pixels(CompositionPageRef, egui::TextureHandle),
    Failed(CompositionPageRef),
}
impl std::fmt::Debug for WorkerReply {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Pixels(page, _) => f.debug_tuple("Pixels").field(page).finish(),
            Self::Failed(page) => f.debug_tuple("Failed").field(page).finish(),
        }
    }
}

#[derive(Debug)]
enum RequestState {
    Pending,
    Failed,
}

#[derive(Debug)]
pub(crate) struct EditorThumbnails {
    scope: BookScopeKey,
    domain: PageDomain,
    policy: FileTypeVisibility,
    rows: Arc<MaterializedBookRows>,
    requested: HashMap<CompositionPageRef, RequestState>,
    tx: mpsc::SyncSender<Demand>,
    rx: mpsc::Receiver<WorkerReply>,
    cancel: Arc<AtomicBool>,
}

impl EditorThumbnails {
    pub(crate) fn spawn(
        input: EditorSourceInput,
        rows: MaterializedBookRows,
        ctx: egui::Context,
    ) -> Result<Self, String> {
        if rows.page_identities.policy != input.policy {
            return Err("本の構成のサムネイル設定が一致しません".into());
        }
        let rows = Arc::new(rows);
        let (tx, requests) = mpsc::sync_channel::<Demand>(2);
        let (results, rx) = mpsc::sync_channel::<WorkerReply>(REQUEST_BUDGET);
        let cancel = Arc::new(AtomicBool::new(false));
        let owner = Self {
            scope: input.scope.clone(),
            domain: input.domain.clone(),
            policy: input.policy.clone(),
            rows: rows.clone(),
            requested: HashMap::new(),
            tx,
            rx,
            cancel: cancel.clone(),
        };
        std::thread::Builder::new()
            .name("composition-editor-thumbnails".into())
            .spawn(move || {
                // Existing rotation storage is only read on this worker. No writes,
                // thumbnail catalog or editor-specific store are introduced.
                let rotation_path = input.data_dir.join("rotation.db");
                let rotations = if rotation_path.exists() {
                    match crate::rotation_db::RotationDb::open_readonly_at(&rotation_path) {
                        Ok(db) => Some(db),
                        Err(error) => {
                            crate::logger::log(format!(
                                "composition editor thumbnail rotations: {error}"
                            ));
                            None
                        }
                    }
                } else {
                    None
                };
                let live_rotations: HashMap<_, _> = input
                    .rotations
                    .iter()
                    .map(|(page, rotation)| (page.matching_key(), *rotation))
                    .collect();
                while let Ok(batch) = requests.recv() {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    for (page, index) in batch {
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        let Some(item) = rows.items.get(index) else {
                            continue;
                        };
                        let pixels = match decode_thumbnail(&input, item, &cancel) {
                            Ok(pixels) => pixels,
                            Err(error) => {
                                if !cancel.load(Ordering::Relaxed) {
                                    crate::logger::log(format!(
                                        "composition editor thumbnail {page:?}: {error}"
                                    ));
                                }
                                if cancel.load(Ordering::Relaxed)
                                    || results.send(WorkerReply::Failed(page)).is_err()
                                {
                                    return;
                                }
                                ctx.request_repaint_of(egui::ViewportId::ROOT);
                                ctx.request_repaint();
                                continue;
                            }
                        };
                        let rotation = live_rotations
                            .get(&page.matching_key())
                            .copied()
                            .or_else(|| {
                                rotations.as_ref().and_then(|db| {
                                    crate::edit_source::page_key_for_grid_item(item)
                                        .and_then(|key| db.get_key(&key))
                                })
                            })
                            .unwrap_or(crate::rotation_db::Rotation::None);
                        let pixels = crate::capture::rotate_color_image(&pixels, rotation);
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        // Egui's thread-safe texture manager accepts CPU pixels here;
                        // backend upload stays with the existing frame renderer.
                        let texture = ctx.load_texture(
                            format!("composition_editor_{page:?}"),
                            pixels,
                            egui::TextureOptions::LINEAR,
                        );
                        if cancel.load(Ordering::Relaxed) {
                            return;
                        }
                        if results.send(WorkerReply::Pixels(page, texture)).is_err() {
                            return;
                        }
                        ctx.request_repaint_of(egui::ViewportId::ROOT);
                        ctx.request_repaint();
                    }
                }
            })
            .map_err(|error| error.to_string())?;
        Ok(owner)
    }

    /// Demand is bounded and nonblocking. A full queue leaves the demand
    /// unregistered so the next visible-frame request can submit it again.
    pub(crate) fn request(&mut self, pages: Vec<CompositionPageRef>) {
        if self.cancel.load(Ordering::Relaxed) {
            return;
        }
        let mut seen = HashSet::new();
        let batch: Demand = pages
            .into_iter()
            .take(REQUEST_BUDGET)
            .filter_map(|page| {
                let key = page.matching_key();
                if self.requested.contains_key(&key) || !seen.insert(key) {
                    return None;
                }
                self.admitted_index(&page).map(|index| (page, index))
            })
            .collect();
        if batch.is_empty() {
            return;
        }
        let keys: Vec<_> = batch.iter().map(|(page, _)| page.matching_key()).collect();
        if self.tx.try_send(batch).is_ok() {
            self.requested
                .extend(keys.into_iter().map(|key| (key, RequestState::Pending)));
        }
    }

    fn admitted_index(&self, page: &CompositionPageRef) -> Option<usize> {
        self.domain.validate_page(page).ok()?;
        let allowed = match page {
            CompositionPageRef::ArchiveEntry { entry_name } => {
                self.policy.allows_physical_file(Path::new(entry_name))
            }
            CompositionPageRef::PdfPage { .. } => {
                self.policy.allows_physical_file(&self.scope.logical_root)
            }
            CompositionPageRef::RelativePath { .. } => false, // G4F is separate.
        };
        if !allowed {
            return None;
        }
        let index = self
            .rows
            .page_identities
            .resolve_shift_identity(&page.resume_identity(&self.scope.logical_root))?;
        let stored = match self.rows.items.get(index)? {
            GridItem::ZipImage { entry_name, .. } => CompositionPageRef::ArchiveEntry {
                entry_name: entry_name.clone(),
            },
            GridItem::PdfPage { page_num, .. } => CompositionPageRef::PdfPage {
                page_num: *page_num,
            },
            _ => return None,
        };
        (stored.matching_key() == page.matching_key()).then_some(index)
    }

    pub(crate) fn poll(&mut self) -> Vec<Reply> {
        if self.cancel.load(Ordering::Relaxed) {
            while self.rx.try_recv().is_ok() {}
            return vec![];
        }
        let mut pixels = Vec::new();
        for result in self.rx.try_iter().take(REQUEST_BUDGET) {
            match result {
                WorkerReply::Pixels(page, texture) => {
                    // The UI cache now owns this successful texture. Once it
                    // evicts it, ordinary missing-cache demand can decode again.
                    self.requested.remove(&page.matching_key());
                    pixels.push((page, texture));
                }
                WorkerReply::Failed(page) => {
                    // Cosmetic failures finish once; visible frames do not
                    // create an automatic decode retry loop.
                    self.requested
                        .insert(page.matching_key(), RequestState::Failed);
                }
            }
        }
        pixels
    }
}

impl Drop for EditorThumbnails {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

fn decode_thumbnail(
    input: &EditorSourceInput,
    item: &GridItem,
    cancel: &Arc<AtomicBool>,
) -> Result<egui::ColorImage, String> {
    use crate::canonical_image_loader::{
        AnimationPolicy, CanonicalDecodeOptions, CanonicalImageDecode, CanonicalImageSource,
        RawStage,
    };
    let image = match item {
        GridItem::ZipImage {
            zip_path,
            entry_name,
        } => {
            if crate::raw_format::is_raw_path(Path::new(entry_name)) {
                let bytes =
                    crate::zip_loader::read_entry_bytes_cancellable(zip_path, entry_name, cancel)
                        .map_err(|error| error.to_string())?;
                let raster = crate::thumb_loader::decode_raw_thumbnail_on_bounded_worker(
                    crate::raw::RawOwnedSource::Bytes(bytes.into()),
                    THUMB_PX,
                    &input.raw_executor,
                    cancel,
                    256 * 1024 * 1024,
                )
                .map_err(|error| error.to_string())?;
                return Ok(crate::thumb_loader::resize_to_display_color_image(
                    &raster.image,
                    THUMB_PX,
                    Some(raster.developed_dims),
                ));
            }
            let decoded = crate::canonical_image_loader::decode_canonical_image(
                CanonicalImageSource::ArchiveEntry {
                    archive_path: zip_path,
                    entry_name,
                },
                CanonicalDecodeOptions::fullscreen_cancellable(
                    AnimationPolicy::FirstFrameOnly,
                    cancel,
                    RawStage::Preview,
                ),
            )
            .map_err(|error| format!("{error:?}"))?;
            match decoded {
                CanonicalImageDecode::Static(image) => image.image,
                CanonicalImageDecode::RawPreview {
                    preview: Some(preview),
                    ..
                } => preview.image,
                _ => return Err("サムネイル画像を読み取れません".into()),
            }
        }
        GridItem::PdfPage { page_num, .. } => {
            let BookCompositionSource::Document {
                physical_source,
                read_target,
                ..
            } = &input.source
            else {
                return Err("文書の元ページが一致しません".into());
            };
            let read = match read_target {
                Some(read) => read.clone(),
                None => crate::pdf_loader::resolve_read_target(physical_source)
                    .map_err(|error| error.to_string())?,
            };
            crate::pdf_loader::render_page_with_read_target(
                &input.scope.logical_root,
                &read,
                *page_num,
                THUMB_PX,
                input.password.as_deref(),
                Some(cancel.clone()),
                crate::pdf_loader::JobPriority::Normal,
                0,
                crate::pdf_loader::CancelWaitPolicy::AbortOnCancel,
            )
            .map_err(|error| error.to_string())?
            .image
        }
        _ => return Err("実ページのサムネイルではありません".into()),
    };
    Ok(crate::thumb_loader::resize_to_display_color_image(
        &image, THUMB_PX, None,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn owner() -> (
        EditorThumbnails,
        mpsc::Receiver<Demand>,
        mpsc::SyncSender<WorkerReply>,
    ) {
        let logical = Path::new("C:/test/book.zip");
        let policy = FileTypeVisibility::default();
        let items = vec![GridItem::ZipImage {
            zip_path: logical.to_owned(),
            entry_name: "page.png".into(),
        }];
        let rows = MaterializedBookRows {
            page_identities: Arc::new(
                super::super::file_type_resume::PreparedPageIdentityIndex::prepare(
                    &items,
                    Some(logical),
                    policy.clone(),
                ),
            ),
            items,
            metas: vec![None],
            composition: crate::book_composition::CompositionProjectionOutcome::Automatic,
        };
        let (tx, requests) = mpsc::sync_channel(2);
        let (results, rx) = mpsc::sync_channel(24);
        (
            EditorThumbnails {
                scope: BookScopeKey::archive(logical, &[]).unwrap(),
                domain: PageDomain::ArchiveLevel {
                    prefix: String::new(),
                },
                policy,
                rows: Arc::new(rows),
                requested: HashMap::new(),
                tx,
                rx,
                cancel: Arc::new(AtomicBool::new(false)),
            },
            requests,
            results,
        )
    }

    fn page(name: &str) -> CompositionPageRef {
        CompositionPageRef::ArchiveEntry {
            entry_name: name.into(),
        }
    }

    #[test]
    fn policy_excluded_and_missing_pages_never_enter_decode_queue() {
        let (mut owner, requests, _) = owner();
        owner.policy.excluded_extensions.insert("png".into());
        owner.request(vec![page("page.png"), page("missing.jpg")]);
        assert!(requests.try_recv().is_err());
        assert!(owner.requested.is_empty());
    }

    #[test]
    fn independent_editor_policies_and_alias_demands_keep_private_queue_ownership() {
        let (mut admitted, admitted_requests, _) = owner();
        let (mut excluded, excluded_requests, _) = owner();
        excluded.policy.excluded_extensions.insert("png".into());
        let logical = &excluded.scope.logical_root;
        excluded.rows = Arc::new(MaterializedBookRows {
            items: vec![],
            metas: vec![],
            composition: crate::book_composition::CompositionProjectionOutcome::Automatic,
            page_identities: Arc::new(
                super::super::file_type_resume::PreparedPageIdentityIndex::prepare(
                    &[],
                    Some(logical),
                    excluded.policy.clone(),
                ),
            ),
        });
        admitted.request(vec![page("PAGE.PNG"), page("page.png")]);
        excluded.request(vec![page("PAGE.PNG")]);
        assert_eq!(
            admitted_requests.try_recv().unwrap(),
            vec![(page("PAGE.PNG"), 0)]
        );
        assert!(excluded_requests.try_recv().is_err());
        assert_eq!(admitted.requested.len(), 1);
        assert!(excluded.requested.is_empty());
        drop(excluded);
        assert!(!admitted.cancel.load(Ordering::Relaxed));
        admitted.request(vec![page("page.png")]);
        assert!(
            admitted_requests.try_recv().is_err(),
            "saved alias shares one pending key"
        );
    }

    #[test]
    fn duplicate_demand_is_submitted_once_and_full_queue_keeps_it_retryable() {
        let (mut owner, requests, _) = owner();
        owner.tx.try_send(vec![]).unwrap();
        owner.tx.try_send(vec![]).unwrap();
        owner.request(vec![page("page.png")]);
        assert!(owner.requested.is_empty());
        requests.try_recv().unwrap();
        requests.try_recv().unwrap();
        owner.request(vec![page("page.png"), page("PAGE.PNG")]);
        assert_eq!(requests.try_recv().unwrap().len(), 1);
        owner.request(vec![page("page.png")]);
        assert!(requests.try_recv().is_err());
    }

    #[test]
    fn retired_owner_cancels_and_private_late_reply_cannot_reach_next_editor() {
        let (first, _, results) = owner();
        let cancel = first.cancel.clone();
        drop(first);
        assert!(cancel.load(Ordering::Relaxed));
        let texture = egui::Context::default().load_texture(
            "late",
            egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        assert!(
            results
                .try_send(WorkerReply::Pixels(page("page.png"), texture))
                .is_err()
        );
        let (mut next, _, _) = owner();
        assert!(next.poll().is_empty());
        assert!(next.requested.is_empty());
    }

    #[test]
    fn successful_poll_allows_re_request_after_ui_cache_eviction() {
        let (mut owner, requests, results) = owner();
        owner.request(vec![page("page.png")]);
        requests.try_recv().unwrap();
        let texture = egui::Context::default().load_texture(
            "ready",
            egui::ColorImage::filled([1, 1], egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        );
        results
            .try_send(WorkerReply::Pixels(page("page.png"), texture))
            .unwrap();
        assert_eq!(owner.poll().len(), 1);
        assert!(owner.requested.is_empty());
        // Root requests only cache-missing pages, so this stands for eviction.
        owner.request(vec![page("page.png")]);
        assert_eq!(requests.try_recv().unwrap().len(), 1);
    }

    #[test]
    fn failed_poll_finishes_once_without_visible_frame_retry_loop() {
        let (mut owner, requests, results) = owner();
        owner.request(vec![page("page.png")]);
        requests.try_recv().unwrap();
        results
            .try_send(WorkerReply::Failed(page("page.png")))
            .unwrap();
        assert!(owner.poll().is_empty());
        assert!(matches!(
            owner.requested.get(&page("page.png").matching_key()),
            Some(RequestState::Failed)
        ));
        owner.request(vec![page("page.png")]);
        assert!(requests.try_recv().is_err());
    }
}

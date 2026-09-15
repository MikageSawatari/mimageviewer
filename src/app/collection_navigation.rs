//! Viewer-context-owned collection playback navigation.
//!
//! A request observes the collection actor before enqueueing its load, prepares one exact
//! immutable revision away from the UI thread, resolves against that prepared order, and
//! preflights the finite candidate set before replacing the current presentation.

use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use super::folder_scan::ScannedDir;
use super::top_level_grid_view::{
    CollectionGridIdentity, CollectionGridLoadState, CollectionGridPosition, CollectionGridSession,
    CollectionGridViewportAnchor, TopLevelGridRestore, TopLevelGridSurface,
};
use super::{App, FolderOpenOutcome, HistoryTrigger, ManualMediaNavigationLanding};
use crate::collection_store::{
    CollectionEntryId, CollectionId, CollectionNavigationAnchor, CollectionNavigationDirection,
    CollectionNavigationEntryIdentity, CollectionNavigationTail, CollectionNavigationTargetKind,
    CollectionPrepareError, CollectionPreparedNavigationTarget, CollectionPreparedSnapshot,
    CollectionResolvedKind, CollectionRevisionWatch, CollectionSnapshot, CollectionStoreError,
    prepare_collection_snapshot, resolve_prepared_collection_navigation,
};

#[derive(Clone, Debug, PartialEq, Eq)]
struct CollectionNavigationOrigin {
    context_id: super::ViewerContextId,
    collection_id: CollectionId,
    surface_generation: u64,
    items_generation: u64,
    intent_sequence: u64,
    anchor: Option<CollectionNavigationAnchor>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum CollectionNavigationAction {
    Manual {
        fs_idx: usize,
        delta: i32,
        queued_steps: i32,
        display_unit_step: bool,
        landing: ManualMediaNavigationLanding,
        still_only: bool,
    },
    Slideshow {
        fs_idx: usize,
        end_action: crate::settings::SlideshowEndAction,
        outer: bool,
    },
    OuterGrid {
        forward: bool,
        queued_steps: i32,
    },
    OuterFullscreen {
        fs_idx: usize,
        forward: bool,
        resume_slideshow: bool,
        native_toast: bool,
        queued_steps: i32,
    },
    VideoContinuousEof {
        fs_idx: usize,
        seek_serial: u64,
        wraps: bool,
    },
    #[cfg(windows)]
    VideoAudioModeContinuousEof {
        fs_idx: usize,
        seek_serial: u64,
        wraps: bool,
    },
    MusicContinuousEof {
        fs_idx: usize,
        seek_serial: u64,
        wraps: bool,
    },
}

impl CollectionNavigationAction {
    fn direction(&self) -> CollectionNavigationDirection {
        match self {
            Self::Manual { delta, .. } => direction(*delta > 0),
            Self::OuterGrid { forward, .. } | Self::OuterFullscreen { forward, .. } => {
                direction(*forward)
            }
            Self::Slideshow { .. }
            | Self::VideoContinuousEof { .. }
            | Self::MusicContinuousEof { .. } => CollectionNavigationDirection::Forward,
            #[cfg(windows)]
            Self::VideoAudioModeContinuousEof { .. } => CollectionNavigationDirection::Forward,
        }
    }

    fn target_kind(&self) -> CollectionNavigationTargetKind {
        match self {
            Self::Manual {
                still_only: true, ..
            } => CollectionNavigationTargetKind::StillImage,
            Self::Manual { .. } => CollectionNavigationTargetKind::NavigableMedia,
            Self::Slideshow { .. } => CollectionNavigationTargetKind::StillImage,
            Self::OuterGrid { .. } | Self::OuterFullscreen { .. } => {
                CollectionNavigationTargetKind::OuterContainer
            }
            Self::VideoContinuousEof { .. } => CollectionNavigationTargetKind::Video,
            #[cfg(windows)]
            Self::VideoAudioModeContinuousEof { .. } => CollectionNavigationTargetKind::Video,
            Self::MusicContinuousEof { .. } => CollectionNavigationTargetKind::Audio,
        }
    }

    fn tail(&self) -> CollectionNavigationTail {
        match self {
            Self::Slideshow { end_action, .. } => match end_action {
                crate::settings::SlideshowEndAction::LoopFolder => CollectionNavigationTail::Loop,
                crate::settings::SlideshowEndAction::NextFolder
                | crate::settings::SlideshowEndAction::Stop => CollectionNavigationTail::Stop,
            },
            Self::VideoContinuousEof { wraps, .. } | Self::MusicContinuousEof { wraps, .. } => {
                if *wraps {
                    CollectionNavigationTail::Loop
                } else {
                    CollectionNavigationTail::Stop
                }
            }
            #[cfg(windows)]
            Self::VideoAudioModeContinuousEof { wraps, .. } => {
                if *wraps {
                    CollectionNavigationTail::Loop
                } else {
                    CollectionNavigationTail::Stop
                }
            }
            Self::Manual { .. } | Self::OuterGrid { .. } | Self::OuterFullscreen { .. } => {
                CollectionNavigationTail::Stop
            }
        }
    }

    fn required_matches(&self) -> usize {
        match self {
            Self::Manual { delta, .. } => delta.unsigned_abs().max(1) as usize,
            _ => 1,
        }
    }

    fn is_media_eof(&self) -> bool {
        match self {
            Self::VideoContinuousEof { .. } | Self::MusicContinuousEof { .. } => true,
            #[cfg(windows)]
            Self::VideoAudioModeContinuousEof { .. } => true,
            _ => false,
        }
    }

    fn history_trigger(&self) -> HistoryTrigger {
        match self {
            Self::Manual { .. } | Self::OuterGrid { .. } | Self::OuterFullscreen { .. } => {
                HistoryTrigger::UserChosen
            }
            Self::Slideshow { .. }
            | Self::VideoContinuousEof { .. }
            | Self::MusicContinuousEof { .. } => HistoryTrigger::AutoAdvance,
            #[cfg(windows)]
            Self::VideoAudioModeContinuousEof { .. } => HistoryTrigger::AutoAdvance,
        }
    }
}

impl CollectionNavigationRequest {
    fn is_slideshow(&self) -> bool {
        matches!(self.action, CollectionNavigationAction::Slideshow { .. })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(in crate::app) struct CollectionNavigationRequest {
    origin: CollectionNavigationOrigin,
    action: CollectionNavigationAction,
}

enum CollectionNavigationPreflightPayload {
    Media,
    Folder(ScannedDir),
    Zip(crate::zip_loader::ZipEnumeration),
    PdfPages(Vec<crate::pdf_loader::PdfPageEntry>),
    PdfPasswordRequired,
    ConvertibleArchive(crate::archive_converter::ArchiveImageSummary),
    ConvertiblePasswordRequired,
}

pub(in crate::app) struct CollectionNavigationPreflightReady {
    target: CollectionPreparedNavigationTarget,
    payload: CollectionNavigationPreflightPayload,
    rejected: Vec<CollectionEntryId>,
}

struct CollectionNavigationPreflightCandidate {
    target: CollectionPreparedNavigationTarget,
    pdf_password: Option<String>,
}

struct CollectionNavigationRootLanding {
    target_idx: usize,
    remapped_origin_idx: Option<usize>,
    transient_origin: bool,
}

#[derive(Clone, Debug)]
pub(in crate::app) struct CollectionNavigationContinuationStamp {
    context_id: super::ViewerContextId,
    collection_id: CollectionId,
    surface_generation: u64,
    items_generation: u64,
    intent_sequence: u64,
    target_idx: usize,
    target: CollectionNavigationEntryIdentity,
}

pub(in crate::app) enum CollectionNavigationPending {
    Snapshot {
        request: CollectionNavigationRequest,
        watch: CollectionRevisionWatch,
        receiver: crossbeam_channel::Receiver<Result<CollectionSnapshot, CollectionStoreError>>,
    },
    Preparing {
        request: CollectionNavigationRequest,
        watch: CollectionRevisionWatch,
        exact_revision: u64,
        cancel: Arc<AtomicBool>,
        receiver:
            std::sync::mpsc::Receiver<Result<CollectionPreparedSnapshot, CollectionPrepareError>>,
    },
    Preflighting {
        request: CollectionNavigationRequest,
        watch: CollectionRevisionWatch,
        prepared: Arc<CollectionPreparedSnapshot>,
        target_kind: CollectionNavigationTargetKind,
        cancel: Arc<AtomicBool>,
        receiver: std::sync::mpsc::Receiver<
            Result<Option<CollectionNavigationPreflightReady>, CollectionPrepareError>,
        >,
    },
    AwaitingPdfPassword {
        request: CollectionNavigationRequest,
        watch: CollectionRevisionWatch,
        prepared: Arc<CollectionPreparedSnapshot>,
        target: CollectionPreparedNavigationTarget,
    },
    PdfPasswordPreflighting {
        request: CollectionNavigationRequest,
        watch: CollectionRevisionWatch,
        prepared: Arc<CollectionPreparedSnapshot>,
        target: CollectionPreparedNavigationTarget,
        password: String,
        save: bool,
        cancel: Arc<AtomicBool>,
        receiver: std::sync::mpsc::Receiver<std::io::Result<Vec<crate::pdf_loader::PdfPageEntry>>>,
    },
    AwaitingOuterContinuation {
        steps: i32,
        fullscreen: bool,
        resume_slideshow: bool,
        native_toast: bool,
    },
    AwaitingManualContinuation {
        steps: i32,
        landing: ManualMediaNavigationLanding,
        display_unit_step: bool,
        still_only: bool,
        stamp: CollectionNavigationContinuationStamp,
    },
}

impl CollectionNavigationPending {
    fn action(&self) -> Option<&CollectionNavigationAction> {
        match self {
            Self::Snapshot { request, .. }
            | Self::Preparing { request, .. }
            | Self::Preflighting { request, .. }
            | Self::AwaitingPdfPassword { request, .. }
            | Self::PdfPasswordPreflighting { request, .. } => Some(&request.action),
            Self::AwaitingOuterContinuation { .. } | Self::AwaitingManualContinuation { .. } => {
                None
            }
        }
    }

    pub(crate) fn is_slideshow(&self) -> bool {
        self.action()
            .is_some_and(|action| matches!(action, CollectionNavigationAction::Slideshow { .. }))
    }

    pub(crate) fn is_media_eof(&self) -> bool {
        self.action()
            .is_some_and(CollectionNavigationAction::is_media_eof)
    }

    pub(crate) fn is_pdf_password(&self) -> bool {
        matches!(
            self,
            Self::AwaitingPdfPassword { .. } | Self::PdfPasswordPreflighting { .. }
        )
    }

    pub(crate) fn cancel(&self) {
        match self {
            Self::Preparing { cancel, .. }
            | Self::Preflighting { cancel, .. }
            | Self::PdfPasswordPreflighting { cancel, .. } => {
                cancel.store(true, Ordering::Release);
            }
            Self::Snapshot { .. } => {}
            Self::AwaitingPdfPassword { .. } => {}
            Self::AwaitingOuterContinuation { .. } => {}
            Self::AwaitingManualContinuation { .. } => {}
        }
    }

    pub(crate) fn owns_fs_navigation_lock(&self) -> bool {
        let action = match self {
            Self::Snapshot { request, .. }
            | Self::Preparing { request, .. }
            | Self::Preflighting { request, .. }
            | Self::AwaitingPdfPassword { request, .. }
            | Self::PdfPasswordPreflighting { request, .. } => &request.action,
            Self::AwaitingOuterContinuation { fullscreen, .. } => return *fullscreen,
            Self::AwaitingManualContinuation { .. } => return false,
        };
        matches!(
            action,
            CollectionNavigationAction::OuterFullscreen { .. }
                | CollectionNavigationAction::Slideshow { outer: true, .. }
        )
    }

    pub(crate) fn accumulate_manual(
        &mut self,
        delta: i32,
        landing: ManualMediaNavigationLanding,
        still_only: bool,
        display_unit_step: bool,
    ) -> bool {
        if let Self::AwaitingManualContinuation {
            steps,
            landing: active_landing,
            display_unit_step: active_display_unit_step,
            still_only: active_still_only,
            ..
        } = self
        {
            if *active_landing != landing
                || *active_still_only != still_only
                || *active_display_unit_step != display_unit_step
                || (*steps != 0 && steps.signum() != delta.signum())
            {
                return false;
            }
            *steps = (*steps + delta).clamp(-32, 32);
            return true;
        }
        let action = match self {
            Self::Snapshot { request, .. }
            | Self::Preparing { request, .. }
            | Self::Preflighting { request, .. }
            | Self::AwaitingPdfPassword { request, .. }
            | Self::PdfPasswordPreflighting { request, .. } => &mut request.action,
            _ => return false,
        };
        let CollectionNavigationAction::Manual {
            delta: active,
            queued_steps,
            landing: active_landing,
            still_only: active_still_only,
            display_unit_step: active_display_unit_step,
            ..
        } = action
        else {
            return false;
        };
        if *active_landing != landing
            || *active_still_only != still_only
            || *active_display_unit_step != display_unit_step
            || active.signum() != delta.signum()
        {
            return false;
        }
        *queued_steps = (*queued_steps + delta).clamp(-32, 32);
        true
    }

    pub(in crate::app) fn set_intent_sequence(&mut self, sequence: u64) {
        match self {
            Self::Snapshot { request, .. }
            | Self::Preparing { request, .. }
            | Self::Preflighting { request, .. }
            | Self::AwaitingPdfPassword { request, .. }
            | Self::PdfPasswordPreflighting { request, .. } => {
                request.origin.intent_sequence = sequence;
            }
            Self::AwaitingManualContinuation { stamp, .. } => {
                stamp.intent_sequence = sequence;
            }
            _ => {}
        }
    }

    pub(crate) fn accumulate_outer(&mut self, fullscreen: bool, forward: bool) -> bool {
        let delta = if forward { 1 } else { -1 };
        let action = match self {
            Self::Snapshot { request, .. }
            | Self::Preparing { request, .. }
            | Self::Preflighting { request, .. }
            | Self::AwaitingPdfPassword { request, .. }
            | Self::PdfPasswordPreflighting { request, .. } => &mut request.action,
            Self::AwaitingOuterContinuation {
                steps,
                fullscreen: pending_fullscreen,
                ..
            } if *pending_fullscreen == fullscreen => {
                *steps = (*steps + delta).clamp(-32, 32);
                return true;
            }
            Self::AwaitingOuterContinuation { .. } => return false,
            Self::AwaitingManualContinuation { .. } => return false,
        };
        match action {
            CollectionNavigationAction::OuterGrid { queued_steps, .. } if !fullscreen => {
                *queued_steps = (*queued_steps + delta).clamp(-32, 32);
                true
            }
            CollectionNavigationAction::OuterFullscreen { queued_steps, .. } if fullscreen => {
                *queued_steps = (*queued_steps + delta).clamp(-32, 32);
                true
            }
            _ => false,
        }
    }

    pub(in crate::app) fn set_outer_queued_steps(&mut self, steps: i32) {
        let action = match self {
            Self::Snapshot { request, .. }
            | Self::Preparing { request, .. }
            | Self::Preflighting { request, .. }
            | Self::AwaitingPdfPassword { request, .. }
            | Self::PdfPasswordPreflighting { request, .. } => &mut request.action,
            Self::AwaitingOuterContinuation { steps: pending, .. } => {
                *pending = steps;
                return;
            }
            Self::AwaitingManualContinuation { .. } => return,
        };
        match action {
            CollectionNavigationAction::OuterGrid { queued_steps, .. }
            | CollectionNavigationAction::OuterFullscreen { queued_steps, .. } => {
                *queued_steps = steps;
            }
            _ => {}
        }
    }
}

enum RevisionObservation {
    Unchanged,
    Revision(u64),
    Deleted,
}

fn direction(forward: bool) -> CollectionNavigationDirection {
    if forward {
        CollectionNavigationDirection::Forward
    } else {
        CollectionNavigationDirection::Backward
    }
}

fn observe_revision(watch: &CollectionRevisionWatch, id: CollectionId) -> RevisionObservation {
    let Some(notice) = watch.take_latest() else {
        return RevisionObservation::Unchanged;
    };
    notice
        .collection_revisions
        .iter()
        .find_map(|(candidate, revision)| (*candidate == id).then_some(*revision))
        .map(RevisionObservation::Revision)
        .unwrap_or(RevisionObservation::Deleted)
}

fn preflight_candidates(
    candidates: Vec<CollectionNavigationPreflightCandidate>,
    required_matches: usize,
    still_only: bool,
    tree_options: crate::folder_tree::FolderTreeOptions,
    show_hidden_files: bool,
    cancel: &Arc<AtomicBool>,
) -> Result<Option<CollectionNavigationPreflightReady>, CollectionPrepareError> {
    let mut accepted = 0usize;
    let mut rejected = Vec::new();
    for candidate in candidates {
        if cancel.load(Ordering::Acquire) {
            return Err(CollectionPrepareError::Cancelled);
        }
        let path = &candidate.target.source_path;
        let payload = match candidate.target.resolved_kind {
            CollectionResolvedKind::Image
            | CollectionResolvedKind::Video
            | CollectionResolvedKind::Audio => std::fs::metadata(path)
                .ok()
                .filter(std::fs::Metadata::is_file)
                .map(|_| CollectionNavigationPreflightPayload::Media),
            CollectionResolvedKind::Folder => {
                let qualifies = if still_only {
                    crate::folder_tree::folder_has_still_image_with_options(
                        path,
                        Some(cancel),
                        tree_options,
                    )
                } else {
                    crate::folder_tree::folder_should_stop_with_options(
                        path,
                        Some(cancel),
                        tree_options,
                    )
                };
                if qualifies && !cancel.load(Ordering::Acquire) {
                    super::folder_scan::scan_directory_with_convertible_archives_cancel(
                        path,
                        tree_options.include_convertible_archives,
                        show_hidden_files,
                        Some(cancel),
                    )
                    .ok()
                    .filter(|scan| {
                        scan.all_media.iter().any(|entry| {
                            if still_only {
                                entry.kind == super::folder_scan::ScanMediaKind::Image
                            } else {
                                matches!(
                                    entry.kind,
                                    super::folder_scan::ScanMediaKind::Image
                                        | super::folder_scan::ScanMediaKind::Video
                                        | super::folder_scan::ScanMediaKind::Audio
                                )
                            }
                        })
                    })
                    .map(CollectionNavigationPreflightPayload::Folder)
                } else {
                    None
                }
            }
            CollectionResolvedKind::Zip => {
                crate::zip_loader::enumerate_image_entries_detailed_with_cancel(path, Some(cancel))
                    .ok()
                    .filter(|enumeration| !enumeration.entries.is_empty())
                    .inspect(|enumeration| {
                        crate::zip_key_migration::migrate_if_needed(
                            path,
                            &enumeration.legacy_renames,
                        );
                    })
                    .map(CollectionNavigationPreflightPayload::Zip)
            }
            CollectionResolvedKind::Pdf => match crate::pdf_loader::enumerate_pages_with_cancel(
                path,
                candidate.pdf_password.as_deref(),
                Some(Arc::clone(cancel)),
            ) {
                Ok(pages) if !pages.is_empty() => {
                    Some(CollectionNavigationPreflightPayload::PdfPages(pages))
                }
                Err(error) if crate::pdf_loader::is_password_required_error(&error) => {
                    Some(CollectionNavigationPreflightPayload::PdfPasswordRequired)
                }
                _ => None,
            },
            CollectionResolvedKind::ConvertibleArchive => {
                if !tree_options.include_convertible_archives {
                    rejected.push(candidate.target.entry_id);
                    continue;
                }
                let format = path
                    .extension()
                    .and_then(|extension| extension.to_str())
                    .and_then(crate::archive_converter::ArchiveFormat::from_extension);
                format.and_then(|format| {
                    match crate::archive_converter::scan_summary_with_password_cancelable(
                        path, format, None, cancel,
                    ) {
                        Ok(summary)
                            if summary.image_count != 0 || summary.nested_archive_count != 0 =>
                        {
                            Some(CollectionNavigationPreflightPayload::ConvertibleArchive(
                                summary,
                            ))
                        }
                        Err(crate::archive_converter::ConvertError::PasswordRequired) => {
                            Some(CollectionNavigationPreflightPayload::ConvertiblePasswordRequired)
                        }
                        _ => None,
                    }
                })
            }
            CollectionResolvedKind::Unresolved => None,
        };
        let Some(payload) = payload else {
            rejected.push(candidate.target.entry_id);
            continue;
        };
        accepted += 1;
        if accepted >= required_matches {
            return Ok(Some(CollectionNavigationPreflightReady {
                target: candidate.target,
                payload,
                rejected,
            }));
        }
    }
    Ok(None)
}

impl App {
    pub(crate) fn cancel_collection_navigation_intent(&mut self) {
        self.cancel_transferred_collection_archive_navigation("collection_navigation_cancelled");
        self.top_level_grid_view
            .advance_collection_navigation_sequence();
        let Some(pending) = self
            .top_level_grid_view
            .take_collection_navigation_pending()
        else {
            return;
        };
        if pending.is_pdf_password() {
            self.pdf_password_request = None;
            self.pdf_password_pending_save = None;
        }
        pending.cancel();
        if pending.owns_fs_navigation_lock() {
            self.finish_fs_navigation_sequence(super::FsNavigationSequenceFinish::RequestFailed);
        }
        if let Some(action) = pending.action() {
            let action = action.clone();
            self.finish_collection_navigation_without_target_no_context(&action);
        }
    }

    fn cancel_transferred_collection_archive_navigation(&mut self, reason: &'static str) -> bool {
        let transferred = self.archive_convert.as_ref().is_some_and(|state| {
            matches!(
                &state.completion,
                crate::ui_dialogs::archive_convert::ArchiveConvertCompletionPolicy::MainGridArchive(
                    intent
                ) if intent.collection_grid_owner.as_ref().is_some_and(|owner| {
                    owner.navigation_request.is_some()
                })
            )
        });
        transferred && self.cancel_archive_convert_for_navigation(reason)
    }

    pub(crate) fn cancel_collection_slideshow_navigation_intent(&mut self) {
        let transferred_slideshow = self.archive_convert.as_ref().is_some_and(|state| {
            matches!(
                &state.completion,
                crate::ui_dialogs::archive_convert::ArchiveConvertCompletionPolicy::MainGridArchive(
                    intent
                ) if intent.collection_grid_owner.as_ref().and_then(|owner| {
                    owner.navigation_request.as_ref()
                }).is_some_and(CollectionNavigationRequest::is_slideshow)
            )
        });
        let is_slideshow = self
            .top_level_grid_view
            .take_collection_navigation_pending()
            .map(|pending| {
                let is_slideshow = pending.is_slideshow();
                self.top_level_grid_view
                    .set_collection_navigation_pending(Some(pending));
                is_slideshow
            })
            .unwrap_or(false);
        if is_slideshow || transferred_slideshow {
            self.cancel_collection_navigation_intent();
        }
    }

    pub(crate) fn cancel_collection_media_eof_navigation_intent(&mut self) {
        let is_media_eof = self
            .top_level_grid_view
            .take_collection_navigation_pending()
            .map(|pending| {
                let is_media_eof = pending.is_media_eof();
                self.top_level_grid_view
                    .set_collection_navigation_pending(Some(pending));
                is_media_eof
            })
            .unwrap_or(false);
        if is_media_eof {
            self.cancel_collection_navigation_intent();
        }
    }

    fn collection_navigation_identity_at(
        prepared: &CollectionPreparedSnapshot,
        index: usize,
    ) -> Option<CollectionNavigationEntryIdentity> {
        prepared
            .entries
            .get(index)
            .map(|entry| CollectionNavigationEntryIdentity {
                entry_id: entry.entry_id,
                source_key: entry.source_key.clone(),
            })
    }

    fn collection_root_navigation_origin(
        &mut self,
        fs_idx: usize,
        capture_spread: bool,
    ) -> Option<CollectionNavigationOrigin> {
        let partner_idx = if capture_spread {
            match self.resolve_visible_spread_pair(fs_idx) {
                crate::ui_fullscreen::SpreadPair::Single => None,
                crate::ui_fullscreen::SpreadPair::Double { left, right } => (left != fs_idx)
                    .then_some(left)
                    .or_else(|| (right != fs_idx).then_some(right)),
            }
        } else {
            None
        };
        let session = self.top_level_grid_view.collection_session()?;
        if !matches!(session.position, CollectionGridPosition::Root)
            || session.installed_items_generation != Some(self.items_generation)
        {
            return None;
        }
        let prepared = session.prepared()?;
        let primary = Self::collection_navigation_identity_at(prepared, fs_idx)?;
        let entry = prepared.entries.get(fs_idx)?;
        if !matches!(
            entry.availability,
            crate::collection_store::CollectionSourcePreparation::Available {
                kind: CollectionResolvedKind::Image
                    | CollectionResolvedKind::Video
                    | CollectionResolvedKind::Audio,
                ..
            }
        ) {
            return None;
        }
        let partner =
            partner_idx.and_then(|index| Self::collection_navigation_identity_at(prepared, index));
        Some(CollectionNavigationOrigin {
            context_id: self.collection_grid_context_id(),
            collection_id: session.identity.collection_id,
            surface_generation: self.top_level_grid_view.generation(),
            items_generation: self.items_generation,
            intent_sequence: self.top_level_grid_view.collection_navigation_sequence(),
            anchor: Some(CollectionNavigationAnchor { primary, partner }),
        })
    }

    fn collection_outer_navigation_origin(
        &self,
        current_idx: Option<usize>,
    ) -> Option<CollectionNavigationOrigin> {
        if let Some(session) = self.top_level_grid_view.collection_session() {
            let anchor = match &session.position {
                CollectionGridPosition::PhysicalSource {
                    entry_id,
                    source_key,
                    ..
                } => Some(CollectionNavigationAnchor {
                    primary: CollectionNavigationEntryIdentity {
                        entry_id: *entry_id,
                        source_key: source_key.clone(),
                    },
                    partner: None,
                }),
                CollectionGridPosition::Root
                    if session.installed_items_generation == Some(self.items_generation) =>
                {
                    current_idx
                        .or(self.selected)
                        .and_then(|index| session.prepared()?.entries.get(index))
                        .map(|entry| CollectionNavigationAnchor {
                            primary: CollectionNavigationEntryIdentity {
                                entry_id: entry.entry_id,
                                source_key: entry.source_key.clone(),
                            },
                            partner: None,
                        })
                }
                CollectionGridPosition::Root => None,
            };
            return Some(CollectionNavigationOrigin {
                context_id: self.collection_grid_context_id(),
                collection_id: session.identity.collection_id,
                surface_generation: self.top_level_grid_view.generation(),
                items_generation: self.items_generation,
                intent_sequence: self.top_level_grid_view.collection_navigation_sequence(),
                anchor,
            });
        }
        let TopLevelGridRestore::Collection(restore) = self.top_level_grid_view.return_to()? else {
            return None;
        };
        let anchor = restore
            .viewport_anchor
            .as_ref()
            .map(|anchor| CollectionNavigationAnchor {
                primary: CollectionNavigationEntryIdentity {
                    entry_id: anchor.entry_id,
                    source_key: anchor.source_key.clone(),
                },
                partner: None,
            });
        Some(CollectionNavigationOrigin {
            context_id: self.collection_grid_context_id(),
            collection_id: restore.identity.collection_id,
            surface_generation: self.top_level_grid_view.generation(),
            items_generation: self.items_generation,
            intent_sequence: self.top_level_grid_view.collection_navigation_sequence(),
            anchor,
        })
    }

    fn enqueue_collection_navigation(
        &mut self,
        ctx: &egui::Context,
        origin: CollectionNavigationOrigin,
        action: CollectionNavigationAction,
    ) -> bool {
        let request = CollectionNavigationRequest { origin, action };
        self.top_level_grid_view
            .set_collection_navigation_pending(None);
        let Some(client) = self.collection_store_client() else {
            self.finish_collection_navigation_without_target(ctx, &request.action);
            return true;
        };
        // Establish the revision watch before enqueueing the actor load. The load reply is the
        // decision linearization point; a later notice causes an exact-revision restart.
        let Ok(watch) = client.subscribe() else {
            self.finish_collection_navigation_without_target(ctx, &request.action);
            return true;
        };
        let Ok(receiver) = client.load_collection(request.origin.collection_id) else {
            self.finish_collection_navigation_without_target(ctx, &request.action);
            return true;
        };
        self.top_level_grid_view
            .set_collection_navigation_pending(Some(CollectionNavigationPending::Snapshot {
                request,
                watch,
                receiver,
            }));
        ctx.request_repaint_after(std::time::Duration::from_millis(16));
        true
    }

    pub(crate) fn start_collection_manual_navigation(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        delta: i32,
        landing: ManualMediaNavigationLanding,
    ) -> bool {
        self.start_collection_manual_navigation_inner(ctx, fs_idx, delta, landing, false)
    }

    pub(crate) fn start_collection_manual_display_unit_navigation(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        delta: i32,
        landing: ManualMediaNavigationLanding,
    ) -> bool {
        self.start_collection_manual_navigation_inner(ctx, fs_idx, delta, landing, true)
    }

    fn start_collection_manual_navigation_inner(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        delta: i32,
        landing: ManualMediaNavigationLanding,
        display_unit_step: bool,
    ) -> bool {
        if delta == 0 {
            return false;
        }
        let still_only = self.continuous_reading_active_for_idx(fs_idx);
        // A native source swap can still report its old presenter index after the latest
        // collection root has been installed. The continuation stamp, rather than that transient
        // index, owns inputs until the landing completes, so admit queued steps before trying to
        // reconstruct a fresh origin from `fs_idx`.
        if let Some(mut pending) = self
            .top_level_grid_view
            .take_collection_navigation_pending()
        {
            let continuation_is_current = matches!(
                &pending,
                CollectionNavigationPending::AwaitingManualContinuation {
                    stamp,
                    landing: active_landing,
                    ..
                } if *active_landing == landing
                    && self.collection_navigation_manual_continuation_is_current(stamp, landing)
            );
            if continuation_is_current
                && pending.accumulate_manual(delta, landing, still_only, display_unit_step)
            {
                let sequence = self
                    .top_level_grid_view
                    .advance_collection_navigation_sequence();
                pending.set_intent_sequence(sequence);
                self.top_level_grid_view
                    .set_collection_navigation_pending(Some(pending));
                return true;
            }
            self.top_level_grid_view
                .set_collection_navigation_pending(Some(pending));
        }
        let Some(mut origin) = self.collection_root_navigation_origin(fs_idx, true) else {
            return false;
        };
        let delta = if display_unit_step
            && origin
                .anchor
                .as_ref()
                .is_some_and(|anchor| anchor.partner.is_some())
        {
            delta.signum()
        } else {
            delta
        };
        let sequence = self
            .top_level_grid_view
            .advance_collection_navigation_sequence();
        if self
            .top_level_grid_view
            .accumulate_collection_manual_navigation(delta, landing, still_only, display_unit_step)
        {
            if let Some(pending) = self
                .top_level_grid_view
                .take_collection_navigation_pending()
            {
                let mut pending = pending;
                pending.set_intent_sequence(sequence);
                self.top_level_grid_view
                    .set_collection_navigation_pending(Some(pending));
            }
            return true;
        }
        origin.intent_sequence = sequence;
        self.enqueue_collection_navigation(
            ctx,
            origin,
            CollectionNavigationAction::Manual {
                fs_idx,
                delta,
                queued_steps: 0,
                display_unit_step,
                landing,
                still_only,
            },
        )
    }

    pub(crate) fn start_collection_slideshow_navigation(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
    ) -> bool {
        let Some(mut origin) = self.collection_root_navigation_origin(fs_idx, true) else {
            return false;
        };
        origin.intent_sequence = self
            .top_level_grid_view
            .advance_collection_navigation_sequence();
        self.enqueue_collection_navigation(
            ctx,
            origin,
            CollectionNavigationAction::Slideshow {
                fs_idx,
                end_action: self.settings.slideshow_end_action,
                outer: false,
            },
        )
    }

    fn promote_collection_slideshow_to_outer(&mut self, request: &mut CollectionNavigationRequest) {
        let CollectionNavigationAction::Slideshow { fs_idx, outer, .. } = &mut request.action
        else {
            return;
        };
        if *outer {
            return;
        }
        self.capture_fs_nav_holdover(*fs_idx);
        self.slideshow_playing = false;
        self.slideshow_anchor_idx = None;
        self.continuous_reading_scroll_transition = None;
        self.slideshow_scroll_range_cache = None;
        *outer = true;
    }

    pub(crate) fn start_collection_outer_grid_navigation(
        &mut self,
        ctx: &egui::Context,
        forward: bool,
    ) -> bool {
        if self
            .top_level_grid_view
            .accumulate_collection_outer_navigation(false, forward)
        {
            let sequence = self
                .top_level_grid_view
                .advance_collection_navigation_sequence();
            if let Some(pending) = self
                .top_level_grid_view
                .take_collection_navigation_pending()
            {
                let mut pending = pending;
                pending.set_intent_sequence(sequence);
                self.top_level_grid_view
                    .set_collection_navigation_pending(Some(pending));
            }
            return true;
        }
        let Some(mut origin) = self.collection_outer_navigation_origin(None) else {
            return false;
        };
        origin.intent_sequence = self
            .top_level_grid_view
            .advance_collection_navigation_sequence();
        self.enqueue_collection_navigation(
            ctx,
            origin,
            CollectionNavigationAction::OuterGrid {
                forward,
                queued_steps: 0,
            },
        )
    }

    pub(crate) fn start_collection_outer_fullscreen_navigation(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        forward: bool,
        resume_slideshow: bool,
        native_toast: bool,
    ) -> bool {
        if self
            .top_level_grid_view
            .accumulate_collection_outer_navigation(true, forward)
        {
            let sequence = self
                .top_level_grid_view
                .advance_collection_navigation_sequence();
            if let Some(pending) = self
                .top_level_grid_view
                .take_collection_navigation_pending()
            {
                let mut pending = pending;
                pending.set_intent_sequence(sequence);
                self.top_level_grid_view
                    .set_collection_navigation_pending(Some(pending));
            }
            return true;
        }
        let Some(mut origin) = self.collection_outer_navigation_origin(Some(fs_idx)) else {
            return false;
        };
        origin.intent_sequence = self
            .top_level_grid_view
            .advance_collection_navigation_sequence();
        if resume_slideshow {
            self.capture_fs_nav_holdover(fs_idx);
        } else {
            self.begin_fs_folder_navigation_sequence(ctx, fs_idx);
        }
        self.enqueue_collection_navigation(
            ctx,
            origin,
            CollectionNavigationAction::OuterFullscreen {
                fs_idx,
                forward,
                resume_slideshow,
                native_toast,
                queued_steps: 0,
            },
        )
    }

    pub(crate) fn start_collection_video_eof_navigation(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        seek_serial: u64,
    ) -> bool {
        let Some(mut origin) = self.collection_root_navigation_origin(fs_idx, false) else {
            return false;
        };
        origin.intent_sequence = self
            .top_level_grid_view
            .advance_collection_navigation_sequence();
        self.enqueue_collection_navigation(
            ctx,
            origin,
            CollectionNavigationAction::VideoContinuousEof {
                fs_idx,
                seek_serial,
                wraps: self.video_continuous_mode.wraps(),
            },
        )
    }

    #[cfg(windows)]
    pub(crate) fn start_collection_video_audio_mode_eof_navigation(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        seek_serial: u64,
    ) -> bool {
        let Some(mut origin) = self.collection_root_navigation_origin(fs_idx, false) else {
            return false;
        };
        origin.intent_sequence = self
            .top_level_grid_view
            .advance_collection_navigation_sequence();
        self.enqueue_collection_navigation(
            ctx,
            origin,
            CollectionNavigationAction::VideoAudioModeContinuousEof {
                fs_idx,
                seek_serial,
                wraps: self.video_continuous_mode.wraps(),
            },
        )
    }

    pub(crate) fn start_collection_music_eof_navigation(
        &mut self,
        ctx: &egui::Context,
        fs_idx: usize,
        seek_serial: u64,
    ) -> bool {
        let Some(mut origin) = self.collection_root_navigation_origin(fs_idx, false) else {
            return false;
        };
        origin.intent_sequence = self
            .top_level_grid_view
            .advance_collection_navigation_sequence();
        self.enqueue_collection_navigation(
            ctx,
            origin,
            CollectionNavigationAction::MusicContinuousEof {
                fs_idx,
                seek_serial,
                wraps: self.video_continuous_mode.wraps(),
            },
        )
    }

    pub(in crate::app) fn resume_collection_pdf_password_request(
        &mut self,
        path: &std::path::Path,
        password: String,
        save: bool,
    ) -> bool {
        let Some(pending) = self
            .top_level_grid_view
            .take_collection_navigation_pending()
        else {
            return false;
        };
        let CollectionNavigationPending::AwaitingPdfPassword {
            request,
            watch,
            prepared,
            target,
        } = pending
        else {
            self.top_level_grid_view
                .set_collection_navigation_pending(Some(pending));
            return false;
        };
        if !crate::folder_tree::path_eq(path, &target.source_path)
            || !self.collection_navigation_exact_target_is_current(&request, &prepared, &target)
        {
            self.finish_collection_navigation_without_target_no_context(&request.action);
            return true;
        }
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let worker_path = target.source_path.clone();
        let worker_password = password.clone();
        let (sender, receiver) = std::sync::mpsc::channel();
        let spawn = std::thread::Builder::new()
            .name("collection-pdf-password-preflight".into())
            .spawn(move || {
                let result = crate::pdf_loader::enumerate_pages_with_cancel(
                    &worker_path,
                    Some(&worker_password),
                    Some(worker_cancel),
                );
                let _ = sender.send(result);
            });
        if spawn.is_err() {
            self.finish_collection_navigation_without_target_no_context(&request.action);
            return true;
        }
        self.top_level_grid_view
            .set_collection_navigation_pending(Some(
                CollectionNavigationPending::PdfPasswordPreflighting {
                    request,
                    watch,
                    prepared,
                    target,
                    password,
                    save,
                    cancel,
                    receiver,
                },
            ));
        true
    }

    pub(in crate::app) fn cancel_collection_pdf_password_request(&mut self) -> bool {
        let Some(pending) = self
            .top_level_grid_view
            .take_collection_navigation_pending()
        else {
            return false;
        };
        pending.cancel();
        match pending {
            CollectionNavigationPending::AwaitingPdfPassword { request, .. }
            | CollectionNavigationPending::PdfPasswordPreflighting { request, .. } => {
                self.finish_collection_navigation_without_target_no_context(&request.action);
                true
            }
            pending => {
                self.top_level_grid_view
                    .set_collection_navigation_pending(Some(pending));
                false
            }
        }
    }

    fn restart_collection_navigation(
        &mut self,
        ctx: &egui::Context,
        mut request: CollectionNavigationRequest,
    ) {
        request.origin.surface_generation = self.top_level_grid_view.generation();
        request.origin.items_generation = self.items_generation;
        let origin = request.origin.clone();
        let action = request.action.clone();
        let _ = self.enqueue_collection_navigation(ctx, origin, action);
    }

    fn spawn_collection_navigation_prepare(
        &mut self,
        ctx: &egui::Context,
        request: CollectionNavigationRequest,
        watch: CollectionRevisionWatch,
        snapshot: CollectionSnapshot,
    ) {
        let exact_revision = snapshot.revision();
        let display_order = self.settings.grid_display_order.clone();
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (sender, receiver) = std::sync::mpsc::channel();
        let spawn = std::thread::Builder::new()
            .name("collection-navigation-prepare".into())
            .spawn(move || {
                let result = prepare_collection_snapshot(
                    &snapshot,
                    &display_order,
                    &worker_cancel,
                    |_, _| {},
                );
                let _ = sender.send(result);
            });
        if spawn.is_err() {
            self.finish_collection_navigation_without_target(ctx, &request.action);
            return;
        }
        self.top_level_grid_view
            .set_collection_navigation_pending(Some(CollectionNavigationPending::Preparing {
                request,
                watch,
                exact_revision,
                cancel,
                receiver,
            }));
    }

    fn spawn_collection_navigation_preflight(
        &mut self,
        ctx: &egui::Context,
        mut request: CollectionNavigationRequest,
        watch: CollectionRevisionWatch,
        prepared: Arc<CollectionPreparedSnapshot>,
        target_kind: CollectionNavigationTargetKind,
    ) {
        let resolved = resolve_prepared_collection_navigation(
            &prepared,
            request.origin.anchor.as_ref(),
            request.action.direction(),
            target_kind,
            request.action.tail(),
            &std::collections::HashSet::new(),
        );
        if resolved.targets.is_empty() {
            if matches!(
                request.action,
                CollectionNavigationAction::Slideshow {
                    end_action: crate::settings::SlideshowEndAction::NextFolder,
                    ..
                }
            ) && target_kind != CollectionNavigationTargetKind::OuterContainer
            {
                self.promote_collection_slideshow_to_outer(&mut request);
                self.spawn_collection_navigation_preflight(
                    ctx,
                    request,
                    watch,
                    prepared,
                    CollectionNavigationTargetKind::OuterContainer,
                );
            } else {
                self.finish_collection_navigation_without_target(ctx, &request.action);
            }
            return;
        }
        let still_only = matches!(
            request.action,
            CollectionNavigationAction::Slideshow { .. }
                | CollectionNavigationAction::OuterFullscreen {
                    resume_slideshow: true,
                    ..
                }
        );
        let candidates = resolved
            .targets
            .into_iter()
            .map(|target| CollectionNavigationPreflightCandidate {
                pdf_password: (target.resolved_kind == CollectionResolvedKind::Pdf)
                    .then(|| self.pdf_open_password(&target.source_path))
                    .flatten(),
                target,
            })
            .collect::<Vec<_>>();
        let required_matches = if target_kind == CollectionNavigationTargetKind::OuterContainer {
            1
        } else {
            request.action.required_matches()
        };
        let tree_options = crate::folder_tree::FolderTreeOptions::from_settings(&self.settings);
        let show_hidden_files = self.settings.show_hidden_files;
        let cancel = Arc::new(AtomicBool::new(false));
        let worker_cancel = Arc::clone(&cancel);
        let (sender, receiver) = std::sync::mpsc::channel();
        let spawn = std::thread::Builder::new()
            .name("collection-navigation-preflight".into())
            .spawn(move || {
                let result = preflight_candidates(
                    candidates,
                    required_matches,
                    still_only,
                    tree_options,
                    show_hidden_files,
                    &worker_cancel,
                );
                let _ = sender.send(result);
            });
        if spawn.is_err() {
            self.finish_collection_navigation_without_target(ctx, &request.action);
            return;
        }
        self.top_level_grid_view
            .set_collection_navigation_pending(Some(CollectionNavigationPending::Preflighting {
                request,
                watch,
                prepared,
                target_kind,
                cancel,
                receiver,
            }));
    }

    pub(crate) fn poll_collection_navigation(&mut self, ctx: &egui::Context) {
        if self
            .top_level_grid_view
            .take_collection_navigation_retired_pdf_password()
            && !self
                .top_level_grid_view
                .collection_navigation_owns_pdf_password()
        {
            self.pdf_password_request = None;
            self.pdf_password_pending_save = None;
        }
        if self
            .top_level_grid_view
            .take_collection_navigation_retired_fs_lock()
            && !self
                .top_level_grid_view
                .collection_navigation_owns_fs_lock()
        {
            self.finish_fs_navigation_sequence(super::FsNavigationSequenceFinish::RequestFailed);
        }
        let Some(pending) = self
            .top_level_grid_view
            .take_collection_navigation_pending()
        else {
            return;
        };
        match pending {
            CollectionNavigationPending::Snapshot {
                request,
                watch,
                receiver,
            } => {
                let observation = observe_revision(&watch, request.origin.collection_id);
                match receiver.try_recv() {
                    Ok(Ok(snapshot)) => match observation {
                        RevisionObservation::Deleted => {
                            self.finish_collection_navigation_without_target(ctx, &request.action)
                        }
                        RevisionObservation::Revision(revision)
                            if revision > snapshot.revision() =>
                        {
                            self.restart_collection_navigation(ctx, request)
                        }
                        _ if snapshot.collection_id() == request.origin.collection_id => {
                            self.spawn_collection_navigation_prepare(ctx, request, watch, snapshot)
                        }
                        _ => self.finish_collection_navigation_without_target(ctx, &request.action),
                    },
                    Ok(Err(_)) | Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        self.finish_collection_navigation_without_target(ctx, &request.action)
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => {
                        if matches!(observation, RevisionObservation::Deleted) {
                            self.finish_collection_navigation_without_target(ctx, &request.action);
                        } else {
                            self.top_level_grid_view
                                .set_collection_navigation_pending(Some(
                                    CollectionNavigationPending::Snapshot {
                                        request,
                                        watch,
                                        receiver,
                                    },
                                ));
                        }
                    }
                }
            }
            CollectionNavigationPending::Preparing {
                request,
                watch,
                exact_revision,
                cancel,
                receiver,
            } => match observe_revision(&watch, request.origin.collection_id) {
                RevisionObservation::Deleted => {
                    cancel.store(true, Ordering::Release);
                    self.finish_collection_navigation_without_target(ctx, &request.action);
                }
                RevisionObservation::Revision(revision) if revision > exact_revision => {
                    cancel.store(true, Ordering::Release);
                    self.restart_collection_navigation(ctx, request);
                }
                _ => match receiver.try_recv() {
                    Ok(Ok(prepared))
                        if prepared.collection_id == request.origin.collection_id
                            && prepared.collection_revision == exact_revision =>
                    {
                        self.spawn_collection_navigation_preflight(
                            ctx,
                            request.clone(),
                            watch,
                            Arc::new(prepared),
                            request.action.target_kind(),
                        );
                    }
                    Ok(Ok(_)) | Ok(Err(CollectionPrepareError::Cancelled)) => {}
                    Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.finish_collection_navigation_without_target(ctx, &request.action)
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        self.top_level_grid_view
                            .set_collection_navigation_pending(Some(
                                CollectionNavigationPending::Preparing {
                                    request,
                                    watch,
                                    exact_revision,
                                    cancel,
                                    receiver,
                                },
                            ));
                    }
                },
            },
            CollectionNavigationPending::Preflighting {
                request,
                watch,
                prepared,
                target_kind,
                cancel,
                receiver,
            } => match observe_revision(&watch, request.origin.collection_id) {
                RevisionObservation::Deleted => {
                    cancel.store(true, Ordering::Release);
                    self.finish_collection_navigation_without_target(ctx, &request.action);
                }
                RevisionObservation::Revision(revision)
                    if revision > prepared.collection_revision =>
                {
                    cancel.store(true, Ordering::Release);
                    self.restart_collection_navigation(ctx, request);
                }
                _ => match receiver.try_recv() {
                    Ok(Ok(Some(ready))) => {
                        // Re-observe after the filesystem worker reply. The worker result is not
                        // a commit authority: an actor notice may have arrived between the first
                        // poll observation and this landing reply.
                        match observe_revision(&watch, request.origin.collection_id) {
                            RevisionObservation::Deleted => self
                                .finish_collection_navigation_without_target(ctx, &request.action),
                            RevisionObservation::Revision(revision)
                                if revision > prepared.collection_revision =>
                            {
                                self.restart_collection_navigation(ctx, request)
                            }
                            _ if self.collection_navigation_exact_target_is_current(
                                &request,
                                &prepared,
                                &ready.target,
                            ) =>
                            {
                                self.commit_collection_navigation(
                                    ctx, request, watch, prepared, ready,
                                )
                            }
                            _ => self
                                .finish_collection_navigation_without_target(ctx, &request.action),
                        }
                    }
                    Ok(Ok(None)) => {
                        let mut request = request;
                        if matches!(
                            request.action,
                            CollectionNavigationAction::Slideshow {
                                end_action: crate::settings::SlideshowEndAction::NextFolder,
                                ..
                            }
                        ) && target_kind != CollectionNavigationTargetKind::OuterContainer
                            && prepared.entries.iter().any(|entry| {
                                matches!(
                                    entry.availability,
                                    crate::collection_store::CollectionSourcePreparation::Available {
                                        kind: CollectionResolvedKind::Folder
                                            | CollectionResolvedKind::Zip
                                            | CollectionResolvedKind::Pdf
                                            | CollectionResolvedKind::ConvertibleArchive,
                                        ..
                                    }
                                )
                            })
                        {
                            self.promote_collection_slideshow_to_outer(&mut request);
                            self.spawn_collection_navigation_preflight(
                                ctx,
                                request,
                                watch,
                                prepared,
                                CollectionNavigationTargetKind::OuterContainer,
                            );
                        } else {
                            self.finish_collection_navigation_without_target(ctx, &request.action)
                        }
                    }
                    Ok(Err(CollectionPrepareError::Cancelled)) => {}
                    Ok(Err(_)) | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                        self.finish_collection_navigation_without_target(ctx, &request.action)
                    }
                    Err(std::sync::mpsc::TryRecvError::Empty) => {
                        self.top_level_grid_view
                            .set_collection_navigation_pending(Some(
                                CollectionNavigationPending::Preflighting {
                                    request,
                                    watch,
                                    prepared,
                                    target_kind,
                                    cancel,
                                    receiver,
                                },
                            ));
                    }
                },
            },
            CollectionNavigationPending::AwaitingPdfPassword {
                request,
                watch,
                prepared,
                target,
            } => {
                let observation = observe_revision(&watch, request.origin.collection_id);
                if matches!(
                    observation,
                    RevisionObservation::Revision(revision)
                        if revision > prepared.collection_revision
                ) {
                    self.pdf_password_request = None;
                    self.restart_collection_navigation(ctx, request);
                } else if self
                    .collection_navigation_exact_target_is_current(&request, &prepared, &target)
                    && !matches!(observation, RevisionObservation::Deleted)
                {
                    self.top_level_grid_view
                        .set_collection_navigation_pending(Some(
                            CollectionNavigationPending::AwaitingPdfPassword {
                                request,
                                watch,
                                prepared,
                                target,
                            },
                        ));
                } else {
                    self.pdf_password_request = None;
                    self.finish_collection_navigation_without_target(ctx, &request.action);
                }
            }
            CollectionNavigationPending::PdfPasswordPreflighting {
                request,
                watch,
                prepared,
                target,
                password,
                save,
                cancel,
                receiver,
            } => {
                let observation = observe_revision(&watch, request.origin.collection_id);
                if !self.collection_navigation_exact_target_is_current(&request, &prepared, &target)
                    || matches!(observation, RevisionObservation::Deleted)
                {
                    cancel.store(true, Ordering::Release);
                    self.finish_collection_navigation_without_target(ctx, &request.action);
                } else if matches!(
                    observation,
                    RevisionObservation::Revision(revision)
                        if revision > prepared.collection_revision
                ) {
                    cancel.store(true, Ordering::Release);
                    self.restart_collection_navigation(ctx, request);
                } else {
                    match receiver.try_recv() {
                        Ok(Ok(pages)) if !pages.is_empty() => {
                            self.pdf_current_password = Some(password.clone());
                            self.pdf_password_pending_save =
                                save.then(|| (target.source_path.clone(), password));
                            self.commit_collection_navigation(
                                ctx,
                                request,
                                watch,
                                prepared,
                                CollectionNavigationPreflightReady {
                                    target,
                                    payload: CollectionNavigationPreflightPayload::PdfPages(pages),
                                    rejected: Vec::new(),
                                },
                            );
                        }
                        Ok(Err(error)) if crate::pdf_loader::is_password_required_error(&error) => {
                            self.pdf_current_password = None;
                            self.pdf_password_pending_save = None;
                            self.pdf_password_request = Some(super::PdfPasswordRequest {
                                path: target.source_path.clone(),
                            });
                            self.top_level_grid_view
                                .set_collection_navigation_pending(Some(
                                    CollectionNavigationPending::AwaitingPdfPassword {
                                        request,
                                        watch,
                                        prepared,
                                        target,
                                    },
                                ));
                        }
                        Ok(Ok(_))
                        | Ok(Err(_))
                        | Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                            self.finish_collection_navigation_without_target(ctx, &request.action)
                        }
                        Err(std::sync::mpsc::TryRecvError::Empty) => self
                            .top_level_grid_view
                            .set_collection_navigation_pending(Some(
                                CollectionNavigationPending::PdfPasswordPreflighting {
                                    request,
                                    watch,
                                    prepared,
                                    target,
                                    password,
                                    save,
                                    cancel,
                                    receiver,
                                },
                            )),
                    }
                }
            }
            CollectionNavigationPending::AwaitingManualContinuation {
                steps,
                landing,
                display_unit_step,
                still_only,
                stamp,
            } => {
                if !self.collection_navigation_manual_continuation_is_current(&stamp, landing) {
                    return;
                }
                let fs_idx = stamp.target_idx;
                #[cfg(windows)]
                let landing_ready = self.native_video_source_swap_pending.is_none()
                    && self.native_video_fast_swap_pending.is_none()
                    && self.video_tile_swap_pending.is_none()
                    && self.native_video_open_pending.is_none();
                #[cfg(not(windows))]
                let landing_ready = true;
                if !landing_ready {
                    self.top_level_grid_view
                        .set_collection_navigation_pending(Some(
                            CollectionNavigationPending::AwaitingManualContinuation {
                                steps,
                                landing,
                                display_unit_step,
                                still_only,
                                stamp,
                            },
                        ));
                } else if steps != 0 {
                    let forward = steps > 0;
                    let remaining = steps - if forward { 1 } else { -1 };
                    let started = if display_unit_step {
                        self.start_collection_manual_display_unit_navigation(
                            ctx,
                            fs_idx,
                            if forward { 1 } else { -1 },
                            landing,
                        )
                    } else {
                        self.start_collection_manual_navigation(
                            ctx,
                            fs_idx,
                            if forward { 1 } else { -1 },
                            landing,
                        )
                    };
                    if started
                        && remaining != 0
                        && let Some(mut next) = self
                            .top_level_grid_view
                            .take_collection_navigation_pending()
                    {
                        if let CollectionNavigationPending::Snapshot { request, .. }
                        | CollectionNavigationPending::Preparing { request, .. }
                        | CollectionNavigationPending::Preflighting { request, .. } = &mut next
                            && let CollectionNavigationAction::Manual { queued_steps, .. } =
                                &mut request.action
                        {
                            *queued_steps = remaining;
                        }
                        self.top_level_grid_view
                            .set_collection_navigation_pending(Some(next));
                    }
                }
            }
            CollectionNavigationPending::AwaitingOuterContinuation {
                steps,
                fullscreen,
                resume_slideshow,
                native_toast,
            } => {
                if steps == 0 {
                    return;
                }
                let ready = if fullscreen {
                    self.fullscreen_idx.is_some() && !self.fs_nav_is_locked()
                } else {
                    self.fullscreen_idx.is_none()
                        && self.pdf_enumerate_pending.is_none()
                        && self.zip_enumerate_pending.is_none()
                        && !self.archive_convert_dialog_visible()
                };
                if !ready {
                    self.top_level_grid_view
                        .set_collection_navigation_pending(Some(
                            CollectionNavigationPending::AwaitingOuterContinuation {
                                steps,
                                fullscreen,
                                resume_slideshow,
                                native_toast,
                            },
                        ));
                } else {
                    let forward = steps > 0;
                    let remaining = steps - if forward { 1 } else { -1 };
                    let started = if fullscreen {
                        if let Some(fs_idx) = self.fullscreen_idx {
                            self.start_collection_outer_fullscreen_navigation(
                                ctx,
                                fs_idx,
                                forward,
                                resume_slideshow,
                                native_toast,
                            )
                        } else {
                            false
                        }
                    } else {
                        self.start_collection_outer_grid_navigation(ctx, forward)
                    };
                    if started {
                        self.top_level_grid_view
                            .set_collection_outer_queued_steps(remaining);
                    } else if fullscreen {
                        self.release_fs_nav_lock();
                    }
                }
            }
        }
        if self.top_level_grid_view.collection_navigation_pending() {
            ctx.request_repaint_after(std::time::Duration::from_millis(16));
        }
    }

    pub(in crate::app) fn collection_navigation_request_is_current(
        &self,
        request: &CollectionNavigationRequest,
    ) -> bool {
        if self.collection_grid_context_id() != request.origin.context_id
            || self.top_level_grid_view.generation() != request.origin.surface_generation
            || self.top_level_grid_view.collection_navigation_sequence()
                != request.origin.intent_sequence
        {
            return false;
        }
        if !matches!(request.action, CollectionNavigationAction::OuterGrid { .. })
            && self.items_generation != request.origin.items_generation
        {
            return false;
        }
        match &request.action {
            CollectionNavigationAction::Manual { fs_idx, .. }
            | CollectionNavigationAction::OuterFullscreen { fs_idx, .. } => {
                self.fullscreen_idx == Some(*fs_idx)
            }
            CollectionNavigationAction::Slideshow { fs_idx, outer, .. } => {
                self.fullscreen_idx == Some(*fs_idx) && (*outer || self.slideshow_playing)
            }
            CollectionNavigationAction::VideoContinuousEof {
                fs_idx,
                seek_serial,
                ..
            }
            | CollectionNavigationAction::MusicContinuousEof {
                fs_idx,
                seek_serial,
                ..
            } => {
                self.fullscreen_idx == Some(*fs_idx)
                    && self.video_continuous_mode.is_enabled()
                    && self.video_continuous_last_eof == Some((*fs_idx, *seek_serial))
            }
            #[cfg(windows)]
            CollectionNavigationAction::VideoAudioModeContinuousEof {
                fs_idx,
                seek_serial,
                ..
            } => {
                self.fullscreen_idx == Some(*fs_idx)
                    && self.video_continuous_mode.is_enabled()
                    && self.video_audio_mode == Some(*fs_idx)
                    && self.video_continuous_last_eof == Some((*fs_idx, *seek_serial))
            }
            CollectionNavigationAction::OuterGrid { .. } => {
                self.fullscreen_idx.is_none()
                    && self
                        .top_level_grid_view
                        .collection_session()
                        .filter(|session| {
                            matches!(session.position, CollectionGridPosition::Root)
                                && session.installed_items_generation == Some(self.items_generation)
                        })
                        .is_some_and(|session| {
                            let selected = self.selected.and_then(|index| {
                                session.prepared()?.entries.get(index).map(|entry| {
                                    CollectionNavigationEntryIdentity {
                                        entry_id: entry.entry_id,
                                        source_key: entry.source_key.clone(),
                                    }
                                })
                            });
                            let requested = request
                                .origin
                                .anchor
                                .as_ref()
                                .map(|anchor| anchor.primary.clone());
                            selected == requested
                        })
            }
        }
    }

    pub(in crate::app) fn collection_navigation_source_owner_is_current(
        &self,
        owner: &super::top_level_grid_view::CollectionGridSourceOpenOwner,
    ) -> bool {
        let (Some(request), Some(watch), Some(prepared)) = (
            owner.navigation_request.as_ref(),
            owner.navigation_watch.as_ref(),
            owner.navigation_prepared.as_ref(),
        ) else {
            return owner.navigation_request.is_none()
                && owner.navigation_watch.is_none()
                && owner.navigation_prepared.is_none();
        };
        if !self.collection_navigation_request_is_current(request) {
            return false;
        }
        let revision_is_current = match observe_revision(watch, request.origin.collection_id) {
            RevisionObservation::Unchanged => true,
            RevisionObservation::Revision(revision) => revision <= prepared.collection_revision,
            RevisionObservation::Deleted => false,
        };
        if !revision_is_current {
            return false;
        }
        prepared.collection_id == request.origin.collection_id
            && prepared.entries.iter().any(|entry| {
                entry.entry_id == owner.anchor.entry_id
                    && entry.source_key == owner.anchor.source_key
            })
    }

    pub(in crate::app) fn collection_navigation_source_owner_landed_is_current(
        &self,
        owner: &super::top_level_grid_view::CollectionGridSourceOpenOwner,
    ) -> bool {
        let (Some(request), Some(watch), Some(prepared)) = (
            owner.navigation_request.as_ref(),
            owner.navigation_watch.as_ref(),
            owner.navigation_prepared.as_ref(),
        ) else {
            return owner.navigation_request.is_none()
                && owner.navigation_watch.is_none()
                && owner.navigation_prepared.is_none();
        };
        if self.collection_grid_context_id() != request.origin.context_id
            || self.top_level_grid_view.generation() != request.origin.surface_generation
            || !matches!(
                self.top_level_grid_view.collection_navigation_sequence(),
                sequence if sequence == request.origin.intent_sequence
                    || sequence == request.origin.intent_sequence.wrapping_add(1)
            )
        {
            return false;
        }
        let revision_is_current = match observe_revision(watch, request.origin.collection_id) {
            RevisionObservation::Unchanged => true,
            RevisionObservation::Revision(revision) => revision <= prepared.collection_revision,
            RevisionObservation::Deleted => false,
        };
        if !revision_is_current {
            return false;
        }
        prepared.collection_id == request.origin.collection_id
            && prepared.entries.iter().any(|entry| {
                entry.entry_id == owner.anchor.entry_id
                    && entry.source_key == owner.anchor.source_key
            })
    }

    fn collection_navigation_exact_target_is_current(
        &self,
        request: &CollectionNavigationRequest,
        prepared: &CollectionPreparedSnapshot,
        target: &CollectionPreparedNavigationTarget,
    ) -> bool {
        if !self.collection_navigation_request_is_current(request)
            || prepared.collection_id != request.origin.collection_id
        {
            return false;
        }
        let wanted_is_newer = self
            .top_level_grid_view
            .collection_session()
            .filter(|session| session.identity.collection_id == request.origin.collection_id)
            .is_some_and(|session| session.wanted_revision > prepared.collection_revision);
        if wanted_is_newer {
            return false;
        }
        prepared.entries.iter().any(|entry| {
            entry.entry_id == target.entry_id
                && entry.source_key == target.source_key
                && crate::folder_tree::path_eq(&entry.source_path, &target.source_path)
                && matches!(
                    entry.availability,
                    crate::collection_store::CollectionSourcePreparation::Available { kind, .. }
                        if kind == target.resolved_kind
                )
        })
    }

    fn collection_navigation_manual_continuation_is_current(
        &self,
        stamp: &CollectionNavigationContinuationStamp,
        landing: ManualMediaNavigationLanding,
    ) -> bool {
        #[cfg(windows)]
        let presentation_target_is_current = self.fullscreen_idx == Some(stamp.target_idx)
            || matches!(landing, ManualMediaNavigationLanding::NativeVideo)
                && (self
                    .native_video_fast_swap_pending
                    .as_ref()
                    .is_some_and(|pending| pending.target_idx == stamp.target_idx)
                    || self
                        .video_tile_swap_pending
                        .as_ref()
                        .is_some_and(|pending| pending.target_idx == stamp.target_idx)
                    || self
                        .native_video_open_pending
                        .as_ref()
                        .is_some_and(|pending| pending.idx == stamp.target_idx));
        #[cfg(not(windows))]
        let presentation_target_is_current = self.fullscreen_idx == Some(stamp.target_idx);
        if self.collection_grid_context_id() != stamp.context_id
            || self.top_level_grid_view.generation() != stamp.surface_generation
            || self.items_generation != stamp.items_generation
            || self.top_level_grid_view.collection_navigation_sequence() != stamp.intent_sequence
            || !presentation_target_is_current
        {
            return false;
        }
        self.top_level_grid_view
            .collection_session()
            .filter(|session| {
                session.identity.collection_id == stamp.collection_id
                    && matches!(session.position, CollectionGridPosition::Root)
                    && session.installed_items_generation == Some(stamp.items_generation)
            })
            .and_then(CollectionGridSession::prepared)
            .and_then(|prepared| prepared.entries.get(stamp.target_idx))
            .is_some_and(|entry| {
                entry.entry_id == stamp.target.entry_id
                    && entry.source_key == stamp.target.source_key
            })
    }

    fn install_collection_navigation_root(
        &mut self,
        request: &CollectionNavigationRequest,
        prepared: Arc<CollectionPreparedSnapshot>,
        target: &CollectionPreparedNavigationTarget,
        preserve_active_media: bool,
    ) -> Option<CollectionNavigationRootLanding> {
        let same_collection = self
            .top_level_grid_view
            .collection_session()
            .is_some_and(|session| session.identity.collection_id == request.origin.collection_id);
        if !same_collection {
            self.top_level_grid_view.begin(
                TopLevelGridSurface::Collection(CollectionGridIdentity {
                    collection_id: request.origin.collection_id,
                }),
                None,
            );
        }
        let previous = self
            .top_level_grid_view
            .collection_session()
            .and_then(|session| session.prepared().cloned());
        if let Some(session) = self.top_level_grid_view.collection_session_mut() {
            session.wanted_revision = prepared.collection_revision;
            session.restore_anchor = Some(CollectionGridViewportAnchor {
                entry_id: target.entry_id,
                source_key: target.source_key.clone(),
            });
            session.load = CollectionGridLoadState::RequestNeeded {
                installed: previous.clone(),
            };
        }
        let remapped_origin_idx = request.origin.anchor.as_ref().and_then(|anchor| {
            prepared
                .entries
                .iter()
                .position(|entry| entry.entry_id == anchor.primary.entry_id)
                .or_else(|| {
                    prepared
                        .entries
                        .iter()
                        .position(|entry| entry.source_key == anchor.primary.source_key)
                })
        });
        let target_idx = prepared
            .entries
            .iter()
            .position(|entry| entry.entry_id == target.entry_id)
            .or_else(|| {
                prepared
                    .entries
                    .iter()
                    .position(|entry| entry.source_key == target.source_key)
            })?;
        let old_fs_idx = self.fullscreen_idx;
        let preserved_player = if preserve_active_media {
            old_fs_idx.and_then(|index| self.fs_cache.remove(&index))
        } else {
            None
        };
        // A latest-revision resolve may intentionally fall back to head after the currently
        // playing entry was deleted. EOF landing must still retain its existing player owner long
        // enough to use the established source-swap / ParkedLive path. Keep it under an index that
        // cannot alias a real item; the typed landing consumes that transient owner immediately.
        let landing_origin_idx = remapped_origin_idx.or_else(|| {
            (request.action.is_media_eof() && preserved_player.is_some())
                .then_some(prepared.entries.len())
        });
        self.apply_collection_grid_prepared(prepared.clone(), previous);
        if let (Some(new_idx), Some(player)) = (landing_origin_idx, preserved_player) {
            self.fs_cache.insert(new_idx, player);
            if self.fullscreen_idx == old_fs_idx {
                self.fullscreen_idx = Some(new_idx);
            }
            if self.video_audio_mode == old_fs_idx {
                self.video_audio_mode = Some(new_idx);
            }
            if let Some(video_audio_vst) = self.video_audio_vst.as_mut()
                && Some(video_audio_vst.fs_idx) == old_fs_idx
            {
                video_audio_vst.fs_idx = new_idx;
            }
            #[cfg(windows)]
            if let Some(runtime) = self.video_audio_mode_runtime.as_mut()
                && Some(runtime.fs_idx) == old_fs_idx
            {
                runtime.remap_fs_idx(new_idx);
            }
            if let Some((index, serial)) = self.video_continuous_last_eof
                && Some(index) == old_fs_idx
            {
                self.video_continuous_last_eof = Some((new_idx, serial));
            }
        }
        Some(CollectionNavigationRootLanding {
            target_idx,
            remapped_origin_idx: landing_origin_idx,
            transient_origin: landing_origin_idx.is_some() && remapped_origin_idx.is_none(),
        })
    }

    fn collection_navigation_source_open_owner(
        &self,
        request: &CollectionNavigationRequest,
        watch: &CollectionRevisionWatch,
        prepared: Arc<CollectionPreparedSnapshot>,
        target: &CollectionPreparedNavigationTarget,
    ) -> Option<super::top_level_grid_view::CollectionGridSourceOpenOwner> {
        let session = self.top_level_grid_view.collection_session()?;
        if session.identity.collection_id != request.origin.collection_id {
            return None;
        }
        Some(super::top_level_grid_view::CollectionGridSourceOpenOwner {
            stamp: super::top_level_grid_view::CollectionGridRequestStamp {
                context_id: request.origin.context_id,
                surface_generation: request.origin.surface_generation,
                collection_id: request.origin.collection_id,
            },
            accepted_revision: session.accepted_revision,
            wanted_revision: session.wanted_revision,
            anchor: CollectionGridViewportAnchor {
                entry_id: target.entry_id,
                source_key: target.source_key.clone(),
            },
            navigation_prepared: Some(prepared),
            navigation_origin: request.origin.anchor.as_ref().map(|anchor| {
                CollectionGridViewportAnchor {
                    entry_id: anchor.primary.entry_id,
                    source_key: anchor.primary.source_key.clone(),
                }
            }),
            navigation_request: Some(request.clone()),
            navigation_watch: Some(watch.clone()),
        })
    }

    pub(in crate::app) fn resume_collection_archive_navigation(
        &mut self,
        continuation: super::CollectionArchiveNavigationContinuation,
    ) {
        if continuation.steps == 0 {
            return;
        }
        self.top_level_grid_view
            .set_collection_navigation_pending(Some(
                CollectionNavigationPending::AwaitingOuterContinuation {
                    steps: continuation.steps,
                    fullscreen: continuation.fullscreen,
                    resume_slideshow: continuation.resume_slideshow,
                    native_toast: continuation.native_toast,
                },
            ));
    }

    fn commit_collection_navigation(
        &mut self,
        ctx: &egui::Context,
        request: CollectionNavigationRequest,
        watch: CollectionRevisionWatch,
        prepared: Arc<CollectionPreparedSnapshot>,
        ready: CollectionNavigationPreflightReady,
    ) {
        match observe_revision(&watch, request.origin.collection_id) {
            RevisionObservation::Deleted => {
                self.finish_collection_navigation_without_target(ctx, &request.action);
                return;
            }
            RevisionObservation::Revision(revision) if revision > prepared.collection_revision => {
                self.restart_collection_navigation(ctx, request);
                return;
            }
            _ => {}
        }
        if !self.collection_navigation_exact_target_is_current(&request, &prepared, &ready.target) {
            return;
        }
        let loops_current_media = request
            .origin
            .anchor
            .as_ref()
            .is_some_and(|anchor| anchor.primary.source_key == ready.target.source_key)
            && request.action.is_media_eof();
        if loops_current_media {
            if let Some(fs_idx) = self.fullscreen_idx
                && let Some(super::FsCacheEntry::Video { player, .. }) = self.fs_cache.get(&fs_idx)
            {
                player.seek(0.0);
                player.set_playing(true);
            }
            return;
        }
        if matches!(
            ready.payload,
            CollectionNavigationPreflightPayload::PdfPasswordRequired
        ) {
            self.pdf_password_request = Some(super::PdfPasswordRequest {
                path: ready.target.source_path.clone(),
            });
            self.top_level_grid_view
                .set_collection_navigation_pending(Some(
                    CollectionNavigationPending::AwaitingPdfPassword {
                        request,
                        watch,
                        prepared,
                        target: ready.target,
                    },
                ));
            ctx.request_repaint();
            return;
        }
        for entry_id in &ready.rejected {
            crate::logger::log(format!(
                "[collection-nav] preflight rejected entry_id={entry_id} revision={}",
                prepared.collection_revision
            ));
        }
        let payload_description = match &ready.payload {
            CollectionNavigationPreflightPayload::Media => "media".to_string(),
            CollectionNavigationPreflightPayload::Folder(scan) => format!(
                "folder:{}",
                scan.folders.len().saturating_add(scan.all_media.len())
            ),
            CollectionNavigationPreflightPayload::Zip(enumeration) => {
                format!("zip:{}", enumeration.entries.len())
            }
            CollectionNavigationPreflightPayload::PdfPages(pages) => {
                format!("pdf:{}", pages.len())
            }
            CollectionNavigationPreflightPayload::PdfPasswordRequired => "pdf-password".into(),
            CollectionNavigationPreflightPayload::ConvertibleArchive(summary) => format!(
                "convertible:{}+{}",
                summary.image_count, summary.nested_archive_count
            ),
            CollectionNavigationPreflightPayload::ConvertiblePasswordRequired => {
                "convertible-password".into()
            }
        };
        crate::logger::log(format!(
            "[collection-nav] commit collection={} revision={} target={} payload={payload_description}",
            request.origin.collection_id,
            prepared.collection_revision,
            ready.target.source_path.display()
        ));

        let action = request.action.clone();
        let history_trigger = action.history_trigger();
        let outer_continuation = match &action {
            CollectionNavigationAction::OuterGrid { queued_steps, .. } if *queued_steps != 0 => {
                Some((*queued_steps, false, false, false))
            }
            CollectionNavigationAction::OuterFullscreen {
                queued_steps,
                resume_slideshow,
                native_toast,
                ..
            } if *queued_steps != 0 => {
                Some((*queued_steps, true, *resume_slideshow, *native_toast))
            }
            _ => None,
        };
        let is_outer_target = ready.target.resolved_kind.is_container();
        #[cfg(windows)]
        let restore_video_tile = self.video_tile_mode_active;
        #[cfg(not(windows))]
        let restore_video_tile = false;

        if ready.target.resolved_kind == CollectionResolvedKind::ConvertibleArchive {
            let Some(collection_owner) = self.collection_navigation_source_open_owner(
                &request,
                &watch,
                Arc::clone(&prepared),
                &ready.target,
            ) else {
                self.finish_collection_navigation_without_target(ctx, &action);
                return;
            };
            let continuation =
                outer_continuation.map(|(steps, fullscreen, resume_slideshow, native_toast)| {
                    super::CollectionArchiveNavigationContinuation {
                        steps,
                        fullscreen,
                        resume_slideshow,
                        native_toast,
                    }
                });
            let owner = self.collection_archive_open_owner(
                &ready.target.source_path,
                collection_owner,
                continuation,
            );
            let outcome = self.load_folder_or_convert_archive_with_auto_fullscreen_owned(
                ready.target.source_path,
                false,
                owner,
            );
            match action {
                CollectionNavigationAction::OuterFullscreen {
                    resume_slideshow, ..
                } => match outcome {
                    FolderOpenOutcome::Loaded => {
                        let _ = self.reopen_fullscreen_after_folder_nav_load(
                            ctx,
                            restore_video_tile,
                            resume_slideshow,
                            history_trigger,
                        );
                    }
                    FolderOpenOutcome::ConversionDialogOpened => {
                        let _ = self.attach_archive_convert_deferred_fullscreen(
                            restore_video_tile,
                            resume_slideshow,
                            history_trigger,
                        );
                    }
                    FolderOpenOutcome::Ignored => self.release_fs_nav_lock(),
                },
                CollectionNavigationAction::Slideshow { .. } => match outcome {
                    FolderOpenOutcome::Loaded => {
                        let _ = self.reopen_fullscreen_after_folder_nav_load(
                            ctx,
                            restore_video_tile,
                            true,
                            history_trigger,
                        );
                    }
                    FolderOpenOutcome::ConversionDialogOpened => {
                        let _ = self.attach_archive_convert_deferred_fullscreen(
                            restore_video_tile,
                            true,
                            history_trigger,
                        );
                    }
                    FolderOpenOutcome::Ignored => self.release_fs_nav_lock(),
                },
                CollectionNavigationAction::OuterGrid { .. } => {}
                _ => {}
            }
            ctx.request_repaint();
            return;
        }

        if is_outer_target && !matches!(action, CollectionNavigationAction::OuterGrid { .. }) {
            self.close_fullscreen_for_folder_nav_reopen();
        }
        let Some(landing) = self.install_collection_navigation_root(
            &request,
            Arc::clone(&prepared),
            &ready.target,
            !is_outer_target,
        ) else {
            self.finish_collection_navigation_without_target(ctx, &action);
            return;
        };
        let target_idx = landing.target_idx;

        if is_outer_target {
            let anchor = CollectionGridViewportAnchor {
                entry_id: ready.target.entry_id,
                source_key: ready.target.source_key.clone(),
            };
            let outcome = match ready.payload {
                CollectionNavigationPreflightPayload::Folder(scan) => {
                    self.load_folder_nav_target(ready.target.source_path.clone(), Some(scan))
                }
                CollectionNavigationPreflightPayload::Zip(enumeration) => {
                    self.load_zip_as_folder_prepared(ready.target.source_path.clone(), enumeration);
                    FolderOpenOutcome::Loaded
                }
                CollectionNavigationPreflightPayload::PdfPages(pages) => {
                    self.load_pdf_as_folder_prepared(ready.target.source_path.clone(), pages);
                    // The preflight pages are delivered through a completed typed handle; poll
                    // owns the normal PDF landing tail on the next frame.
                    FolderOpenOutcome::Loaded
                }
                _ => FolderOpenOutcome::Ignored,
            };
            if matches!(outcome, FolderOpenOutcome::Loaded) {
                self.commit_collection_grid_source_open(anchor, ready.target.source_path);
            }
            let deferred_pdf = ready.target.resolved_kind == CollectionResolvedKind::Pdf
                && matches!(
                    action,
                    CollectionNavigationAction::OuterFullscreen { .. }
                        | CollectionNavigationAction::Slideshow { .. }
                );
            if deferred_pdf {
                let resume_slideshow = matches!(
                    action,
                    CollectionNavigationAction::Slideshow { .. }
                        | CollectionNavigationAction::OuterFullscreen {
                            resume_slideshow: true,
                            ..
                        }
                );
                self.fs_nav_after_pdf_enumerate = Some(super::DeferredFsReopen {
                    history_trigger,
                    resume_slideshow,
                    target: outer_continuation
                        .map(|(steps, fullscreen, resume_slideshow, native_toast)| {
                            super::DeferredFsTarget::CollectionNavigation(
                                super::CollectionArchiveNavigationContinuation {
                                    steps,
                                    fullscreen,
                                    resume_slideshow,
                                    native_toast,
                                },
                            )
                        })
                        .unwrap_or(super::DeferredFsTarget::None),
                    resume_to_last_page: self.settings.book_nav_resume.resumes(),
                    from_explicit_open: false,
                    preserve_after_password_prompt: false,
                });
            }
            match action {
                CollectionNavigationAction::OuterGrid { .. } => {}
                CollectionNavigationAction::OuterFullscreen {
                    resume_slideshow, ..
                } => {
                    if matches!(outcome, FolderOpenOutcome::Loaded) && !deferred_pdf {
                        let _ = self.reopen_fullscreen_after_folder_nav_load(
                            ctx,
                            restore_video_tile,
                            resume_slideshow,
                            history_trigger,
                        );
                    } else if matches!(outcome, FolderOpenOutcome::ConversionDialogOpened) {
                        let _ = self.attach_archive_convert_deferred_fullscreen(
                            restore_video_tile,
                            resume_slideshow,
                            history_trigger,
                        );
                    } else {
                        self.release_fs_nav_lock();
                    }
                }
                CollectionNavigationAction::Slideshow { .. } => {
                    if matches!(outcome, FolderOpenOutcome::Loaded) && !deferred_pdf {
                        let _ = self.reopen_fullscreen_after_folder_nav_load(
                            ctx,
                            restore_video_tile,
                            true,
                            history_trigger,
                        );
                    } else if matches!(outcome, FolderOpenOutcome::ConversionDialogOpened) {
                        let _ = self.attach_archive_convert_deferred_fullscreen(
                            restore_video_tile,
                            true,
                            history_trigger,
                        );
                    } else {
                        self.release_fs_nav_lock();
                    }
                }
                _ => {}
            }
            if !deferred_pdf
                && !matches!(outcome, FolderOpenOutcome::Ignored)
                && let Some((steps, fullscreen, resume_slideshow, native_toast)) =
                    outer_continuation
            {
                self.top_level_grid_view
                    .set_collection_navigation_pending(Some(
                        CollectionNavigationPending::AwaitingOuterContinuation {
                            steps,
                            fullscreen,
                            resume_slideshow,
                            native_toast,
                        },
                    ));
            }
            ctx.request_repaint();
            return;
        }

        match action {
            CollectionNavigationAction::Manual {
                landing,
                queued_steps,
                display_unit_step,
                still_only,
                ..
            } => {
                match landing {
                    ManualMediaNavigationLanding::Fullscreen => {
                        self.open_fullscreen_from_fs_navigation(ctx, target_idx, history_trigger)
                    }
                    #[cfg(windows)]
                    ManualMediaNavigationLanding::NativeVideo => self
                        .open_native_video_fullscreen_from_navigation(
                            ctx,
                            target_idx,
                            history_trigger,
                        ),
                }
                #[cfg(windows)]
                let native_landing = matches!(landing, ManualMediaNavigationLanding::NativeVideo);
                #[cfg(not(windows))]
                let native_landing = false;
                if queued_steps != 0 || native_landing {
                    let target = Self::collection_navigation_identity_at(&prepared, target_idx);
                    if let Some(target) = target {
                        let stamp = CollectionNavigationContinuationStamp {
                            context_id: request.origin.context_id,
                            collection_id: request.origin.collection_id,
                            surface_generation: self.top_level_grid_view.generation(),
                            items_generation: self.items_generation,
                            intent_sequence: request.origin.intent_sequence,
                            target_idx,
                            target,
                        };
                        self.top_level_grid_view
                            .set_collection_navigation_pending(Some(
                                CollectionNavigationPending::AwaitingManualContinuation {
                                    steps: queued_steps,
                                    landing,
                                    display_unit_step,
                                    still_only,
                                    stamp,
                                },
                            ));
                    }
                }
            }
            CollectionNavigationAction::Slideshow { .. } => {
                self.open_fullscreen_from_slideshow_navigation(ctx, target_idx);
            }
            CollectionNavigationAction::VideoContinuousEof { seek_serial, .. } => {
                if let Some(origin_idx) = landing.remapped_origin_idx {
                    self.apply_video_continuous_eof_target(
                        ctx,
                        origin_idx,
                        seek_serial,
                        Some(target_idx),
                        history_trigger,
                    );
                }
            }
            #[cfg(windows)]
            CollectionNavigationAction::VideoAudioModeContinuousEof { seek_serial, .. } => {
                if let Some(origin_idx) = landing.remapped_origin_idx {
                    self.apply_video_audio_mode_continuous_eof_target(
                        ctx,
                        origin_idx,
                        seek_serial,
                        Some(target_idx),
                        history_trigger,
                    );
                }
            }
            CollectionNavigationAction::MusicContinuousEof { seek_serial, .. } => {
                if let Some(origin_idx) = landing.remapped_origin_idx {
                    self.apply_music_continuous_eof_target(
                        ctx,
                        origin_idx,
                        seek_serial,
                        Some(target_idx),
                        history_trigger,
                    );
                }
            }
            CollectionNavigationAction::OuterGrid { .. }
            | CollectionNavigationAction::OuterFullscreen { .. } => {}
        }
        if landing.transient_origin
            && let Some(origin_idx) = landing.remapped_origin_idx
            && self.fullscreen_idx != Some(origin_idx)
        {
            self.fs_cache.remove(&origin_idx);
        }
        ctx.request_repaint();
    }

    fn finish_collection_navigation_without_target(
        &mut self,
        ctx: &egui::Context,
        action: &CollectionNavigationAction,
    ) {
        match action {
            CollectionNavigationAction::Manual { delta, landing, .. } => {
                #[cfg(windows)]
                if matches!(landing, ManualMediaNavigationLanding::NativeVideo) {
                    self.show_native_video_boundary_toast(ctx, *delta > 0);
                    return;
                }
                let _ = landing;
                self.fs_boundary_hint = Some(crate::ui_fullscreen::FsBoundaryHint::Edge {
                    at_end: *delta > 0,
                    at: std::time::Instant::now(),
                });
            }
            CollectionNavigationAction::Slideshow { .. } => {
                self.stop_slideshow_playback();
                self.release_fs_nav_lock();
            }
            CollectionNavigationAction::OuterGrid { forward, .. } => {
                self.show_feedback_toast(if *forward {
                    "コレクション内の次のコンテナはありません".into()
                } else {
                    "コレクション内の前のコンテナはありません".into()
                });
            }
            CollectionNavigationAction::OuterFullscreen {
                forward,
                native_toast,
                ..
            } => {
                let hint = crate::ui_fullscreen::FsBoundaryHint::NoImageFolder {
                    forward: *forward,
                    at: std::time::Instant::now(),
                };
                self.fs_boundary_hint = Some(hint);
                #[cfg(windows)]
                if *native_toast {
                    self.show_native_video_boundary_hint_overlay(hint);
                }
                let _ = native_toast;
                self.release_fs_nav_lock();
            }
            CollectionNavigationAction::VideoContinuousEof { .. }
            | CollectionNavigationAction::MusicContinuousEof { .. } => {
                self.show_feedback_toast("コレクション末尾です".into());
            }
            #[cfg(windows)]
            CollectionNavigationAction::VideoAudioModeContinuousEof { .. } => {
                self.show_feedback_toast("コレクション末尾です".into());
            }
        }
        ctx.request_repaint();
    }

    fn finish_collection_navigation_without_target_no_context(
        &mut self,
        action: &CollectionNavigationAction,
    ) {
        match action {
            CollectionNavigationAction::Slideshow { .. }
            | CollectionNavigationAction::OuterFullscreen { .. } => {
                self.release_fs_nav_lock();
            }
            CollectionNavigationAction::VideoContinuousEof {
                fs_idx,
                seek_serial,
                ..
            }
            | CollectionNavigationAction::MusicContinuousEof {
                fs_idx,
                seek_serial,
                ..
            } => {
                if self.video_continuous_last_eof == Some((*fs_idx, *seek_serial)) {
                    self.video_continuous_last_eof = None;
                }
            }
            #[cfg(windows)]
            CollectionNavigationAction::VideoAudioModeContinuousEof {
                fs_idx,
                seek_serial,
                ..
            } => {
                if self.video_continuous_last_eof == Some((*fs_idx, *seek_serial)) {
                    self.video_continuous_last_eof = None;
                }
            }
            CollectionNavigationAction::Manual { .. }
            | CollectionNavigationAction::OuterGrid { .. } => {}
        }
    }
}

trait CollectionResolvedKindExt {
    fn is_container(self) -> bool;
}

impl CollectionResolvedKindExt for CollectionResolvedKind {
    fn is_container(self) -> bool {
        matches!(
            self,
            CollectionResolvedKind::Folder
                | CollectionResolvedKind::Zip
                | CollectionResolvedKind::Pdf
                | CollectionResolvedKind::ConvertibleArchive
        )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::{Duration, Instant};

    fn recv<T>(receiver: crossbeam_channel::Receiver<Result<T, CollectionStoreError>>) -> T {
        receiver
            .recv_timeout(Duration::from_secs(3))
            .expect("collection actor reply")
            .expect("collection actor operation")
    }

    fn start_ready_app(
        db_path: &std::path::Path,
    ) -> (App, crate::collection_store::CollectionStoreClient) {
        let runtime =
            crate::collection_store::CollectionStoreRuntime::start_at(db_path.to_path_buf())
                .expect("collection runtime");
        let client = runtime.client();
        let mut app = App::new_from_settings(crate::settings::Settings::default());
        app.install_collection_runtime(runtime);
        let ctx = egui::Context::default();
        let deadline = Instant::now() + Duration::from_secs(3);
        while app.collection_store_client().is_none() {
            assert!(
                Instant::now() < deadline,
                "collection runtime did not become ready"
            );
            app.poll_collection_ui(&ctx);
            std::thread::sleep(Duration::from_millis(2));
        }
        (app, client)
    }

    fn prepare_snapshot(snapshot: &CollectionSnapshot) -> Arc<CollectionPreparedSnapshot> {
        Arc::new(
            prepare_collection_snapshot(
                snapshot,
                &crate::settings::Settings::default().grid_display_order,
                &AtomicBool::new(false),
                |_, _| {},
            )
            .unwrap(),
        )
    }

    fn candidate(
        path: PathBuf,
        kind: CollectionResolvedKind,
    ) -> CollectionNavigationPreflightCandidate {
        let source = crate::collection_store::CollectionSourcePath::from_trusted(&path).unwrap();
        CollectionNavigationPreflightCandidate {
            target: CollectionPreparedNavigationTarget {
                entry_id: CollectionEntryId::new(),
                source_key: source.key().clone(),
                source_path: path,
                resolved_kind: kind,
            },
            pdf_password: None,
        }
    }

    #[test]
    fn collection_preflight_is_finite_and_selects_the_required_existing_media() {
        let temp = tempfile::TempDir::new().unwrap();
        let missing = temp.path().join("missing.jpg");
        let first = temp.path().join("first.jpg");
        let second = temp.path().join("second.jpg");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let missing_id = candidate(missing, CollectionResolvedKind::Image)
            .target
            .entry_id;
        let candidates = vec![
            CollectionNavigationPreflightCandidate {
                target: CollectionPreparedNavigationTarget {
                    entry_id: missing_id,
                    source_key: crate::collection_store::CollectionSourcePath::from_trusted(
                        temp.path().join("missing.jpg"),
                    )
                    .unwrap()
                    .key()
                    .clone(),
                    source_path: temp.path().join("missing.jpg"),
                    resolved_kind: CollectionResolvedKind::Image,
                },
                pdf_password: None,
            },
            candidate(first, CollectionResolvedKind::Image),
            candidate(second.clone(), CollectionResolvedKind::Image),
        ];
        let ready = preflight_candidates(
            candidates,
            2,
            false,
            crate::folder_tree::FolderTreeOptions::default(),
            false,
            &Arc::new(AtomicBool::new(false)),
        )
        .unwrap()
        .expect("second existing candidate");
        assert!(crate::folder_tree::path_eq(
            &ready.target.source_path,
            &second
        ));
        assert_eq!(ready.rejected, vec![missing_id]);
    }

    #[test]
    fn collection_preflight_observes_cancellation_before_filesystem_work() {
        let cancel = Arc::new(AtomicBool::new(true));
        let result = preflight_candidates(
            vec![candidate(
                PathBuf::from(r"Z:\never-probed.jpg"),
                CollectionResolvedKind::Image,
            )],
            1,
            false,
            crate::folder_tree::FolderTreeOptions::default(),
            false,
            &cancel,
        );
        assert!(matches!(result, Err(CollectionPrepareError::Cancelled)));
    }

    #[test]
    fn collection_outer_repeats_use_a_bounded_signed_accumulator() {
        let mut pending = CollectionNavigationPending::AwaitingOuterContinuation {
            steps: 1,
            fullscreen: true,
            resume_slideshow: false,
            native_toast: false,
        };
        assert!(pending.accumulate_outer(true, true));
        assert!(pending.accumulate_outer(true, false));
        assert!(!pending.accumulate_outer(false, true));
        assert!(matches!(
            pending,
            CollectionNavigationPending::AwaitingOuterContinuation {
                steps: 1,
                fullscreen: true,
                ..
            }
        ));
        for _ in 0..64 {
            assert!(pending.accumulate_outer(true, true));
        }
        assert!(matches!(
            pending,
            CollectionNavigationPending::AwaitingOuterContinuation { steps: 32, .. }
        ));
    }

    #[test]
    fn manual_repeats_queue_while_the_previous_landing_is_pending() {
        let target_path = PathBuf::from(r"C:\collection\target.jpg");
        let source =
            crate::collection_store::CollectionSourcePath::from_trusted(&target_path).unwrap();
        let mut pending = CollectionNavigationPending::AwaitingManualContinuation {
            steps: 0,
            landing: ManualMediaNavigationLanding::Fullscreen,
            display_unit_step: true,
            still_only: true,
            stamp: CollectionNavigationContinuationStamp {
                context_id: super::super::viewer_context_registry::ViewerContextId::for_test(7),
                collection_id: CollectionId::new(),
                surface_generation: 11,
                items_generation: 13,
                intent_sequence: 17,
                target_idx: 2,
                target: CollectionNavigationEntryIdentity {
                    entry_id: CollectionEntryId::new(),
                    source_key: source.key().clone(),
                },
            },
        };

        assert!(
            pending.accumulate_manual(1, ManualMediaNavigationLanding::Fullscreen, true, true,)
        );
        assert!(
            pending.accumulate_manual(1, ManualMediaNavigationLanding::Fullscreen, true, true,)
        );
        pending.set_intent_sequence(18);
        assert!(!pending.accumulate_manual(
            -1,
            ManualMediaNavigationLanding::Fullscreen,
            true,
            true,
        ));
        assert!(matches!(
            pending,
            CollectionNavigationPending::AwaitingManualContinuation {
                steps: 2,
                stamp: CollectionNavigationContinuationStamp {
                    intent_sequence: 18,
                    ..
                },
                ..
            }
        ));
    }

    #[test]
    fn manual_landing_queue_uses_its_stamp_when_the_presenter_index_is_transient() {
        let temp = tempfile::tempdir().unwrap();
        let image = temp.path().join("target.jpg");
        std::fs::write(&image, b"image").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let created = recv(client.create_collection("landing".into()).unwrap());
        let added = recv(
            client
                .add_batch(
                    created.collection_id(),
                    created.revision(),
                    vec![
                        crate::collection_store::CollectionRegistration::from_trusted_path(
                            &image,
                            CollectionResolvedKind::Image,
                        )
                        .unwrap(),
                    ],
                )
                .unwrap(),
        );
        let prepared = prepare_snapshot(&added.snapshot);
        app.top_level_grid_view.begin(
            TopLevelGridSurface::Collection(CollectionGridIdentity {
                collection_id: prepared.collection_id,
            }),
            None,
        );
        app.apply_collection_grid_prepared(Arc::clone(&prepared), None);
        app.fullscreen_idx = Some(0);
        let sequence = app
            .top_level_grid_view
            .advance_collection_navigation_sequence();
        let target = CollectionNavigationEntryIdentity {
            entry_id: prepared.entries[0].entry_id,
            source_key: prepared.entries[0].source_key.clone(),
        };
        app.top_level_grid_view
            .set_collection_navigation_pending(Some(
                CollectionNavigationPending::AwaitingManualContinuation {
                    steps: 0,
                    landing: ManualMediaNavigationLanding::Fullscreen,
                    display_unit_step: false,
                    still_only: false,
                    stamp: CollectionNavigationContinuationStamp {
                        context_id: app.collection_grid_context_id(),
                        collection_id: prepared.collection_id,
                        surface_generation: app.top_level_grid_view.generation(),
                        items_generation: app.items_generation,
                        intent_sequence: sequence,
                        target_idx: 0,
                        target,
                    },
                },
            ));

        assert!(app.start_collection_manual_navigation(
            &egui::Context::default(),
            1,
            1,
            ManualMediaNavigationLanding::Fullscreen,
        ));
        assert!(matches!(
            app.top_level_grid_view.take_collection_navigation_pending(),
            Some(CollectionNavigationPending::AwaitingManualContinuation {
                steps: 1,
                stamp: CollectionNavigationContinuationStamp { target_idx: 0, .. },
                ..
            })
        ));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn replacing_a_surface_retires_its_fullscreen_navigation_lock_once() {
        let mut view = super::super::top_level_grid_view::TopLevelGridView::default();
        let initial_sequence = view.collection_navigation_sequence();
        view.set_collection_navigation_pending(Some(
            CollectionNavigationPending::AwaitingOuterContinuation {
                steps: 1,
                fullscreen: true,
                resume_slideshow: false,
                native_toast: false,
            },
        ));

        view.begin(TopLevelGridSurface::Folder, None);

        assert!(!view.collection_navigation_pending());
        assert_ne!(view.collection_navigation_sequence(), initial_sequence);
        assert!(view.take_collection_navigation_retired_fs_lock());
        assert!(!view.take_collection_navigation_retired_fs_lock());
    }

    #[test]
    fn commit_barrier_restarts_when_revision_changes_after_preflight() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("a.jpg");
        let second = temp.path().join("b.jpg");
        std::fs::write(&first, b"first").unwrap();
        std::fs::write(&second, b"second").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("collection.db"));
        let created = recv(client.create_collection("before".into()).unwrap());
        let added = recv(
            client
                .add_batch(
                    created.collection_id(),
                    created.revision(),
                    vec![
                        crate::collection_store::CollectionRegistration::from_trusted_path(
                            &first,
                            CollectionResolvedKind::Image,
                        )
                        .unwrap(),
                        crate::collection_store::CollectionRegistration::from_trusted_path(
                            &second,
                            CollectionResolvedKind::Image,
                        )
                        .unwrap(),
                    ],
                )
                .unwrap(),
        );
        let prepared = prepare_snapshot(&added.snapshot);
        app.top_level_grid_view.begin(
            TopLevelGridSurface::Collection(CollectionGridIdentity {
                collection_id: prepared.collection_id,
            }),
            None,
        );
        app.apply_collection_grid_prepared(Arc::clone(&prepared), None);
        app.fullscreen_idx = Some(0);
        let sequence = app
            .top_level_grid_view
            .advance_collection_navigation_sequence();
        let mut origin = app.collection_root_navigation_origin(0, false).unwrap();
        origin.intent_sequence = sequence;
        let request = CollectionNavigationRequest {
            origin,
            action: CollectionNavigationAction::Manual {
                fs_idx: 0,
                delta: 1,
                queued_steps: 0,
                display_unit_step: false,
                landing: ManualMediaNavigationLanding::Fullscreen,
                still_only: false,
            },
        };
        let target_entry = prepared.entries[1].clone();
        let target = CollectionPreparedNavigationTarget {
            entry_id: target_entry.entry_id,
            source_key: target_entry.source_key,
            source_path: target_entry.source_path,
            resolved_kind: CollectionResolvedKind::Image,
        };
        let watch = client.subscribe().unwrap();
        let renamed = recv(
            client
                .rename_collection(
                    prepared.collection_id,
                    prepared.collection_revision,
                    "after".into(),
                )
                .unwrap(),
        );
        assert!(renamed.revision() > prepared.collection_revision);
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
        while !watch.has_pending() {
            assert!(
                std::time::Instant::now() < deadline,
                "revision watch was not published after the rename reply"
            );
            std::thread::yield_now();
        }
        let installed_generation = app.items_generation;

        app.commit_collection_navigation(
            &egui::Context::default(),
            request,
            watch,
            Arc::clone(&prepared),
            CollectionNavigationPreflightReady {
                target,
                payload: CollectionNavigationPreflightPayload::Media,
                rejected: Vec::new(),
            },
        );

        assert_eq!(app.items_generation, installed_generation);
        assert!(matches!(
            app.top_level_grid_view.take_collection_navigation_pending(),
            Some(CollectionNavigationPending::Snapshot { .. })
        ));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn intent_sequence_rejects_a_fullscreen_index_aba() {
        let mut app = App::new_from_settings(crate::settings::Settings::default());
        let collection_id = CollectionId::new();
        app.top_level_grid_view.begin(
            TopLevelGridSurface::Collection(CollectionGridIdentity { collection_id }),
            None,
        );
        app.items.push(crate::app::GridItem::Image(PathBuf::from(
            r"C:\collection\a.jpg",
        )));
        app.fullscreen_idx = Some(0);
        let sequence = app
            .top_level_grid_view
            .advance_collection_navigation_sequence();
        let request = CollectionNavigationRequest {
            origin: CollectionNavigationOrigin {
                context_id: app.collection_grid_context_id(),
                collection_id,
                surface_generation: app.top_level_grid_view.generation(),
                items_generation: app.items_generation,
                intent_sequence: sequence,
                anchor: None,
            },
            action: CollectionNavigationAction::Manual {
                fs_idx: 0,
                delta: 1,
                queued_steps: 0,
                display_unit_step: false,
                landing: ManualMediaNavigationLanding::Fullscreen,
                still_only: false,
            },
        };
        assert!(app.collection_navigation_request_is_current(&request));

        app.cancel_collection_navigation_intent();
        app.fullscreen_idx = None;
        app.fullscreen_idx = Some(0);

        assert!(!app.collection_navigation_request_is_current(&request));
    }

    #[test]
    fn root_grid_selection_change_rejects_an_inflight_outer_request() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::create_dir_all(&second).unwrap();
        std::fs::write(first.join("page.jpg"), b"first").unwrap();
        std::fs::write(second.join("page.jpg"), b"second").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("selection.db"));
        let created = recv(client.create_collection("selection".into()).unwrap());
        let added = recv(
            client
                .add_batch(
                    created.collection_id(),
                    created.revision(),
                    vec![
                        crate::collection_store::CollectionRegistration::from_trusted_path(
                            &first,
                            CollectionResolvedKind::Folder,
                        )
                        .unwrap(),
                        crate::collection_store::CollectionRegistration::from_trusted_path(
                            &second,
                            CollectionResolvedKind::Folder,
                        )
                        .unwrap(),
                    ],
                )
                .unwrap(),
        );
        let prepared = prepare_snapshot(&added.snapshot);
        app.top_level_grid_view.begin(
            TopLevelGridSurface::Collection(CollectionGridIdentity {
                collection_id: prepared.collection_id,
            }),
            None,
        );
        app.apply_collection_grid_prepared(Arc::clone(&prepared), None);
        app.selected = Some(0);
        let mut origin = app.collection_outer_navigation_origin(None).unwrap();
        origin.intent_sequence = app
            .top_level_grid_view
            .advance_collection_navigation_sequence();
        let request = CollectionNavigationRequest {
            origin,
            action: CollectionNavigationAction::OuterGrid {
                forward: true,
                queued_steps: 0,
            },
        };
        assert!(app.collection_navigation_request_is_current(&request));

        let previous = app
            .top_level_grid_view
            .collection_session()
            .and_then(CollectionGridSession::prepared)
            .cloned()
            .unwrap();
        app.apply_collection_grid_prepared(Arc::clone(&prepared), Some(previous));
        assert_ne!(app.items_generation, request.origin.items_generation);
        assert!(app.collection_navigation_request_is_current(&request));

        app.selected = Some(1);
        assert!(!app.collection_navigation_request_is_current(&request));

        app.selected = Some(0);
        let first_entry = &prepared.entries[0];
        app.commit_collection_grid_source_open(
            CollectionGridViewportAnchor {
                entry_id: first_entry.entry_id,
                source_key: first_entry.source_key.clone(),
            },
            first.clone(),
        );
        assert!(!app.collection_navigation_request_is_current(&request));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn transferred_archive_owner_is_rejected_after_its_intent_is_cancelled() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second.rar");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::write(first.join("page.jpg"), b"first").unwrap();
        std::fs::write(&second, b"archive").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("owner.db"));
        let created = recv(client.create_collection("owner".into()).unwrap());
        let added = recv(
            client
                .add_batch(
                    created.collection_id(),
                    created.revision(),
                    vec![
                        crate::collection_store::CollectionRegistration::from_trusted_path(
                            &first,
                            CollectionResolvedKind::Folder,
                        )
                        .unwrap(),
                        crate::collection_store::CollectionRegistration::from_trusted_path(
                            &second,
                            CollectionResolvedKind::ConvertibleArchive,
                        )
                        .unwrap(),
                    ],
                )
                .unwrap(),
        );
        let prepared = prepare_snapshot(&added.snapshot);
        app.top_level_grid_view.begin(
            TopLevelGridSurface::Collection(CollectionGridIdentity {
                collection_id: prepared.collection_id,
            }),
            None,
        );
        app.apply_collection_grid_prepared(Arc::clone(&prepared), None);
        app.selected = Some(0);
        let mut origin = app.collection_outer_navigation_origin(None).unwrap();
        origin.intent_sequence = app
            .top_level_grid_view
            .advance_collection_navigation_sequence();
        let request = CollectionNavigationRequest {
            origin,
            action: CollectionNavigationAction::OuterGrid {
                forward: true,
                queued_steps: 0,
            },
        };
        let watch = client.subscribe().unwrap();
        let target_entry = &prepared.entries[1];
        let target = CollectionPreparedNavigationTarget {
            entry_id: target_entry.entry_id,
            source_key: target_entry.source_key.clone(),
            source_path: target_entry.source_path.clone(),
            resolved_kind: CollectionResolvedKind::ConvertibleArchive,
        };
        let owner = app
            .collection_navigation_source_open_owner(
                &request,
                &watch,
                Arc::clone(&prepared),
                &target,
            )
            .unwrap();
        assert!(app.collection_navigation_source_owner_is_current(&owner));

        app.cancel_collection_navigation_intent();
        assert!(!app.collection_navigation_source_owner_is_current(&owner));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn transferred_archive_owner_allows_the_grid_watch_to_catch_up_to_its_exact_revision() {
        let temp = tempfile::tempdir().unwrap();
        let first = temp.path().join("first");
        let second = temp.path().join("second.rar");
        std::fs::create_dir_all(&first).unwrap();
        std::fs::write(first.join("page.jpg"), b"first").unwrap();
        std::fs::write(&second, b"archive").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("owner-revision.db"));
        let created = recv(client.create_collection("owner revision".into()).unwrap());
        let added = recv(
            client
                .add_batch(
                    created.collection_id(),
                    created.revision(),
                    vec![
                        crate::collection_store::CollectionRegistration::from_trusted_path(
                            &first,
                            CollectionResolvedKind::Folder,
                        )
                        .unwrap(),
                        crate::collection_store::CollectionRegistration::from_trusted_path(
                            &second,
                            CollectionResolvedKind::ConvertibleArchive,
                        )
                        .unwrap(),
                    ],
                )
                .unwrap(),
        );
        let prepared = prepare_snapshot(&added.snapshot);
        app.top_level_grid_view.begin(
            TopLevelGridSurface::Collection(CollectionGridIdentity {
                collection_id: prepared.collection_id,
            }),
            None,
        );
        app.apply_collection_grid_prepared(Arc::clone(&prepared), None);
        app.selected = Some(0);
        let previous_revision = prepared.collection_revision.saturating_sub(1);
        {
            let session = app.top_level_grid_view.collection_session_mut().unwrap();
            session.accepted_revision = previous_revision;
            session.wanted_revision = previous_revision;
        }
        let mut origin = app.collection_outer_navigation_origin(None).unwrap();
        origin.intent_sequence = app
            .top_level_grid_view
            .advance_collection_navigation_sequence();
        let request = CollectionNavigationRequest {
            origin,
            action: CollectionNavigationAction::OuterGrid {
                forward: true,
                queued_steps: 0,
            },
        };
        let target_entry = &prepared.entries[1];
        let target = CollectionPreparedNavigationTarget {
            entry_id: target_entry.entry_id,
            source_key: target_entry.source_key.clone(),
            source_path: target_entry.source_path.clone(),
            resolved_kind: CollectionResolvedKind::ConvertibleArchive,
        };
        let owner = app
            .collection_navigation_source_open_owner(
                &request,
                &client.subscribe().unwrap(),
                Arc::clone(&prepared),
                &target,
            )
            .unwrap();

        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .wanted_revision = prepared.collection_revision;
        assert!(app.collection_grid_source_open_owner_is_current(&owner, &second));
        app.current_folder = Some(second.clone());
        assert!(app.collection_grid_source_open_owner_landed_is_current(&owner, &second));

        app.top_level_grid_view
            .collection_session_mut()
            .unwrap()
            .wanted_revision = prepared.collection_revision + 1;
        assert!(!app.collection_grid_source_open_owner_is_current(&owner, &second));
        assert!(!app.collection_grid_source_open_owner_landed_is_current(&owner, &second));
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn collection_physical_zip_uses_child_dfs_before_resuming_outer_order() {
        let temp = tempfile::tempdir().unwrap();
        let zip_path = temp.path().join("first.zip");
        let next = temp.path().join("second");
        std::fs::write(&zip_path, b"fixture identity").unwrap();
        std::fs::create_dir_all(&next).unwrap();
        std::fs::write(next.join("page.jpg"), b"next").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("zip-child.db"));
        let created = recv(client.create_collection("zip child".into()).unwrap());
        let added = recv(
            client
                .add_batch(
                    created.collection_id(),
                    created.revision(),
                    vec![
                        crate::collection_store::CollectionRegistration::from_trusted_path(
                            &zip_path,
                            CollectionResolvedKind::Zip,
                        )
                        .unwrap(),
                        crate::collection_store::CollectionRegistration::from_trusted_path(
                            &next,
                            CollectionResolvedKind::Folder,
                        )
                        .unwrap(),
                    ],
                )
                .unwrap(),
        );
        let prepared = prepare_snapshot(&added.snapshot);
        app.top_level_grid_view.begin(
            TopLevelGridSurface::Collection(CollectionGridIdentity {
                collection_id: prepared.collection_id,
            }),
            None,
        );
        app.apply_collection_grid_prepared(Arc::clone(&prepared), None);
        let first = &prepared.entries[0];
        app.commit_collection_grid_source_open(
            CollectionGridViewportAnchor {
                entry_id: first.entry_id,
                source_key: first.source_key.clone(),
            },
            zip_path.clone(),
        );
        let entries = ["root.jpg", "book-a/page.jpg", "book-b/page.jpg"]
            .into_iter()
            .map(|entry_name| crate::zip_loader::ZipImageEntry {
                entry_name: entry_name.into(),
                uncompressed_size: 0,
                mtime: 0,
            })
            .collect();
        app.current_folder = Some(zip_path.clone());
        app.zip_nav = Some(crate::zip_tree::ZipNavState::new(Arc::new(
            crate::zip_tree::ZipTree::build(zip_path.clone(), entries),
        )));
        app.zip_nav_show_current_level();
        app.fullscreen_idx = app
            .items
            .iter()
            .position(|item| matches!(item, crate::grid_item::GridItem::ZipImage { entry_name, .. } if entry_name == "root.jpg"));
        let fs_idx = app.fullscreen_idx.unwrap();
        let ctx = egui::Context::default();

        app.handle_fullscreen_ctrl_nav_context(&ctx, fs_idx, true, false);

        assert!(app.items.iter().any(|item| {
            matches!(item, crate::grid_item::GridItem::ZipImage { entry_name, .. } if entry_name == "book-a/page.jpg")
        }));
        assert!(!app.top_level_grid_view.collection_navigation_pending());

        app.finish_fs_navigation_sequence(super::super::FsNavigationSequenceFinish::RequestFailed);
        app.zip_nav.as_mut().unwrap().enter("book-b/");
        app.zip_nav_show_current_level();
        app.fullscreen_idx = Some(0);
        app.handle_fullscreen_ctrl_nav_context(&ctx, 0, true, false);
        assert!(app.top_level_grid_view.collection_navigation_pending());
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn detached_collection_zip_uses_child_dfs_before_resuming_outer_order() {
        let temp = tempfile::tempdir().unwrap();
        let zip_path = temp.path().join("first.zip");
        let next = temp.path().join("second");
        std::fs::write(&zip_path, b"fixture identity").unwrap();
        std::fs::create_dir_all(&next).unwrap();
        std::fs::write(next.join("page.jpg"), b"next").unwrap();
        let (mut app, client) = start_ready_app(&temp.path().join("detached-zip-child.db"));
        let created = recv(
            client
                .create_collection("detached zip child".into())
                .unwrap(),
        );
        let added = recv(
            client
                .add_batch(
                    created.collection_id(),
                    created.revision(),
                    vec![
                        crate::collection_store::CollectionRegistration::from_trusted_path(
                            &zip_path,
                            CollectionResolvedKind::Zip,
                        )
                        .unwrap(),
                        crate::collection_store::CollectionRegistration::from_trusted_path(
                            &next,
                            CollectionResolvedKind::Folder,
                        )
                        .unwrap(),
                    ],
                )
                .unwrap(),
        );
        let prepared = prepare_snapshot(&added.snapshot);
        let first = &prepared.entries[0];
        app.top_level_grid_view.begin(
            TopLevelGridSurface::Folder,
            Some(TopLevelGridRestore::Collection(
                super::super::top_level_grid_view::CollectionGridRestore {
                    identity: CollectionGridIdentity {
                        collection_id: prepared.collection_id,
                    },
                    revision_at_open: prepared.collection_revision,
                    viewport_anchor: Some(CollectionGridViewportAnchor {
                        entry_id: first.entry_id,
                        source_key: first.source_key.clone(),
                    }),
                },
            )),
        );
        let entries = ["root.jpg", "book-a/page.jpg", "book-b/page.jpg"]
            .into_iter()
            .map(|entry_name| crate::zip_loader::ZipImageEntry {
                entry_name: entry_name.into(),
                uncompressed_size: 0,
                mtime: 0,
            })
            .collect();
        app.current_folder = Some(zip_path.clone());
        app.zip_nav = Some(crate::zip_tree::ZipNavState::new(Arc::new(
            crate::zip_tree::ZipTree::build(zip_path, entries),
        )));
        app.zip_nav_show_current_level();
        app.fullscreen_idx = app
            .items
            .iter()
            .position(|item| matches!(item, crate::grid_item::GridItem::ZipImage { entry_name, .. } if entry_name == "root.jpg"));
        let fs_idx = app.fullscreen_idx.unwrap();
        let ctx = egui::Context::default();

        app.handle_fullscreen_ctrl_nav_context(&ctx, fs_idx, true, false);

        assert!(app.items.iter().any(|item| {
            matches!(item, crate::grid_item::GridItem::ZipImage { entry_name, .. } if entry_name == "book-a/page.jpg")
        }));
        assert!(!app.top_level_grid_view.collection_navigation_pending());

        app.finish_fs_navigation_sequence(super::super::FsNavigationSequenceFinish::RequestFailed);
        app.zip_nav.as_mut().unwrap().enter("book-b/");
        app.zip_nav_show_current_level();
        app.fullscreen_idx = Some(0);
        app.handle_fullscreen_ctrl_nav_context(&ctx, 0, true, false);
        assert!(app.top_level_grid_view.collection_navigation_pending());
        app.shutdown_collection_runtime_for_exit();
    }

    #[test]
    fn collection_intents_derive_media_filter_tail_and_history_class() {
        let manual = CollectionNavigationAction::Manual {
            fs_idx: 2,
            delta: -1,
            queued_steps: 0,
            display_unit_step: false,
            landing: ManualMediaNavigationLanding::Fullscreen,
            still_only: true,
        };
        assert_eq!(manual.direction(), CollectionNavigationDirection::Backward);
        assert_eq!(
            manual.target_kind(),
            CollectionNavigationTargetKind::StillImage
        );
        assert_eq!(manual.tail(), CollectionNavigationTail::Stop);
        assert!(!manual.is_media_eof());
        assert_eq!(manual.history_trigger(), HistoryTrigger::UserChosen);

        let eof = CollectionNavigationAction::MusicContinuousEof {
            fs_idx: 4,
            seek_serial: 9,
            wraps: true,
        };
        assert_eq!(eof.target_kind(), CollectionNavigationTargetKind::Audio);
        assert_eq!(eof.tail(), CollectionNavigationTail::Loop);
        assert!(eof.is_media_eof());
        assert_eq!(eof.history_trigger(), HistoryTrigger::AutoAdvance);
    }
}

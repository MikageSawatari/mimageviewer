//! Context-local reservation and decoded-image lifetime. Display state is not a terminal.
use crate::thumb_loader::{ThumbMsg, ThumbMsgPayload, ThumbnailRequestId};
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ThumbnailRequestKind {
    Normal,
    Upgrade,
    VideoBatch,
    DirectPixels,
}
impl ThumbnailRequestKind {
    fn legacy(self) -> bool {
        matches!(self, Self::Normal | Self::Upgrade)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ImageTerminal {
    SourceWaiting,
    SeedWaiting,
    Complete,
}
enum Phase {
    Awaiting,
    FinalizedAwaiting,
    Queued {
        image: Box<ThumbMsg>,
        terminal: ImageTerminal,
    },
    Uploading {
        terminal: ImageTerminal,
    },
    Settled {
        terminal: ImageTerminal,
    },
}
struct Request {
    id: ThumbnailRequestId,
    items_gen: u64,
    kind: ThumbnailRequestKind,
    edit_preview_epoch: Option<u64>,
    phase: Phase,
}
// A completed CacheOnly miss has no outstanding request, but demand still waits for
// archive judgement. Keep this bounded, context-local admission state in the same
// owner instead of preserving a fictitious worker or adding a separate pending set.
enum Slot {
    Request(Request),
    AwaitingArchiveJudgement,
}
impl Slot {
    fn request(&self) -> Option<&Request> {
        match self {
            Self::Request(r) => Some(r),
            Self::AwaitingArchiveJudgement => None,
        }
    }
    fn request_mut(&mut self) -> Option<&mut Request> {
        match self {
            Self::Request(r) => Some(r),
            Self::AwaitingArchiveJudgement => None,
        }
    }
}
#[derive(Default)]
pub(crate) struct ThumbnailRequests {
    entries: HashMap<usize, Slot>,
    upload_order: VecDeque<(usize, ThumbnailRequestId)>,
    legacy_count: usize,
}
pub(crate) enum ThumbnailReceipt {
    Ignored,
    Queued,
    Finalized { source_changed: bool },
    Terminal(ThumbMsgPayload),
}
/// Read-only diagnostics from the canonical request and its actual CPU image queue.
#[cfg(any(test, feature = "test-script"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct ThumbnailRequestProjection {
    pub(crate) request_id: ThumbnailRequestId,
    pub(crate) idx: usize,
    pub(crate) items_gen: u64,
    pub(crate) image_backlog_contains_request: bool,
    /// The owner has not received this request's worker terminal yet.
    pub(crate) worker_pending: bool,
}
impl ThumbnailRequests {
    pub(crate) fn begin(
        &mut self,
        idx: usize,
        items_gen: u64,
        kind: ThumbnailRequestKind,
        edit_preview_epoch: Option<u64>,
    ) -> Option<ThumbnailRequestId> {
        if self.entries.contains_key(&idx) {
            return None;
        }
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let id = ThumbnailRequestId::Registered(NEXT.fetch_add(1, Ordering::Relaxed));
        self.entries.insert(
            idx,
            Slot::Request(Request {
                id,
                items_gen,
                kind,
                edit_preview_epoch,
                phase: Phase::Awaiting,
            }),
        );
        self.legacy_count += usize::from(kind.legacy());
        Some(id)
    }
    pub(crate) fn id(&self, idx: usize) -> Option<ThumbnailRequestId> {
        self.entries.get(&idx).and_then(Slot::request).map(|r| r.id)
    }
    pub(crate) fn matches(&self, msg: &ThumbMsg) -> bool {
        self.entries
            .get(&msg.idx)
            .and_then(Slot::request)
            .is_some_and(|r| r.id == msg.request_id && r.items_gen == msg.items_gen)
    }
    #[cfg(any(test, feature = "test-script"))]
    pub(crate) fn diagnostic_projection(
        &self,
        idx: usize,
        request_id: ThumbnailRequestId,
        items_gen: u64,
    ) -> Option<ThumbnailRequestProjection> {
        let request = self
            .entries
            .get(&idx)
            .and_then(Slot::request)
            .filter(|request| request.id == request_id && request.items_gen == items_gen)?;
        let image_backlog_contains_request = match &request.phase {
            Phase::Queued { image, .. } => {
                image.idx == idx
                    && image.request_id == request_id
                    && image.items_gen == items_gen
                    && self.upload_order.contains(&(idx, request_id))
            }
            _ => false,
        };
        let worker_pending = matches!(
            request.phase,
            Phase::Awaiting
                | Phase::Queued {
                    terminal: ImageTerminal::SourceWaiting | ImageTerminal::SeedWaiting,
                    ..
                }
                | Phase::Uploading {
                    terminal: ImageTerminal::SourceWaiting | ImageTerminal::SeedWaiting,
                }
                | Phase::Settled {
                    terminal: ImageTerminal::SourceWaiting | ImageTerminal::SeedWaiting,
                }
        );
        Some(ThumbnailRequestProjection {
            request_id,
            idx,
            items_gen,
            image_backlog_contains_request,
            worker_pending,
        })
    }
    #[cfg(any(test, feature = "test-script"))]
    pub(crate) fn diagnostic_requests(&self) -> Vec<ThumbnailRequestProjection> {
        let mut requests: Vec<_> = self
            .entries
            .iter()
            .filter_map(|(idx, slot)| {
                let request = slot.request()?;
                self.diagnostic_projection(*idx, request.id, request.items_gen)
            })
            .collect();
        requests.sort_by_key(|request| request.idx);
        requests
    }
    /// Admission includes judgement-waiting demand; activity projections only see Requests.
    pub(crate) fn contains_key(&self, idx: &usize) -> bool {
        self.entries.contains_key(idx)
    }
    #[cfg(test)]
    pub(crate) fn get(&self, idx: &usize) -> Option<&bool> {
        self.entries.get(idx).and_then(Slot::request).map(|r| {
            if r.kind == ThumbnailRequestKind::Upgrade {
                &true
            } else {
                &false
            }
        })
    }
    pub(crate) fn owner_matches_edit_preview_epoch(&self, idx: usize, epoch: u64) -> bool {
        self.entries
            .get(&idx)
            .and_then(Slot::request)
            .is_some_and(|r| r.edit_preview_epoch == Some(epoch))
    }
    pub(crate) fn clear(&mut self) {
        self.entries.clear();
        self.upload_order.clear();
        self.legacy_count = 0;
    }
    /// Scope/source retirement, never ordinary GPU eviction or keep-range departure.
    pub(crate) fn remove(&mut self, idx: &usize) -> Option<bool> {
        let Slot::Request(r) = self.entries.remove(idx)? else {
            return Some(false);
        };
        self.legacy_count -= usize::from(r.kind.legacy());
        if matches!(r.phase, Phase::Queued { .. }) {
            self.upload_order
                .retain(|(i, id)| *i != *idx || *id != r.id);
        }
        Some(r.kind == ThumbnailRequestKind::Upgrade)
    }
    pub(crate) fn cancel_queued(&mut self, idx: usize, id: ThumbnailRequestId) {
        if self.id(idx) == Some(id) {
            self.remove(&idx);
        }
    }
    pub(crate) fn receive(&mut self, msg: ThumbMsg) -> ThumbnailReceipt {
        if !self.matches(&msg) {
            return ThumbnailReceipt::Ignored;
        }
        let idx = msg.idx;
        let r = self
            .entries
            .get_mut(&idx)
            .and_then(Slot::request_mut)
            .unwrap();
        match msg.payload {
            ThumbMsgPayload::CacheOnlyMiss => {
                // Explicitly settle both terminals, retire correlation, then retain only
                // the existing archive-judgement demand gate. It is never busy work.
                self.remove(&idx);
                self.entries.insert(idx, Slot::AwaitingArchiveJudgement);
                ThumbnailReceipt::Terminal(ThumbMsgPayload::CacheOnlyMiss)
            }
            ThumbMsgPayload::Finalized => {
                let source_changed = !matches!(
                    r.phase,
                    Phase::Queued {
                        terminal: ImageTerminal::SeedWaiting,
                        ..
                    } | Phase::Uploading {
                        terminal: ImageTerminal::SeedWaiting
                    } | Phase::Settled {
                        terminal: ImageTerminal::SeedWaiting
                    }
                );
                match &mut r.phase {
                    Phase::Awaiting => r.phase = Phase::FinalizedAwaiting,
                    Phase::Queued { terminal, .. } | Phase::Uploading { terminal } => {
                        if *terminal == ImageTerminal::Complete {
                            return ThumbnailReceipt::Ignored;
                        }
                        *terminal = ImageTerminal::Complete;
                    }
                    Phase::Settled { .. } => {
                        self.remove(&idx);
                    }
                    Phase::FinalizedAwaiting => return ThumbnailReceipt::Ignored,
                }
                ThumbnailReceipt::Finalized { source_changed }
            }
            ThumbMsgPayload::Pixels(_)
            | ThumbMsgPayload::SourcePixels(_)
            | ThumbMsgPayload::ProvisionalPixels(_) => {
                let replacing_seed = matches!(
                    r.phase,
                    Phase::Queued {
                        terminal: ImageTerminal::SeedWaiting,
                        ..
                    } | Phase::Uploading {
                        terminal: ImageTerminal::SeedWaiting
                    } | Phase::Settled {
                        terminal: ImageTerminal::SeedWaiting
                    }
                ) && matches!(msg.payload, ThumbMsgPayload::Pixels(_));
                if !matches!(r.phase, Phase::Awaiting | Phase::FinalizedAwaiting) && !replacing_seed
                {
                    return ThumbnailReceipt::Ignored;
                }
                let terminal = if matches!(r.phase, Phase::FinalizedAwaiting)
                    || matches!(msg.payload, ThumbMsgPayload::Pixels(_))
                {
                    ImageTerminal::Complete
                } else if matches!(msg.payload, ThumbMsgPayload::ProvisionalPixels(_)) {
                    ImageTerminal::SeedWaiting
                } else {
                    ImageTerminal::SourceWaiting
                };
                if replacing_seed {
                    self.upload_order
                        .retain(|(i, id)| *i != idx || *id != msg.request_id);
                }
                self.upload_order.push_back((idx, msg.request_id));
                r.phase = Phase::Queued {
                    image: Box::new(msg),
                    terminal,
                };
                ThumbnailReceipt::Queued
            }
            terminal => {
                self.remove(&idx);
                ThumbnailReceipt::Terminal(terminal)
            }
        }
    }
    pub(crate) fn next_image(&mut self) -> Option<ThumbMsg> {
        while let Some((idx, id)) = self.upload_order.pop_front() {
            let Some(r) = self
                .entries
                .get_mut(&idx)
                .and_then(Slot::request_mut)
                .filter(|r| r.id == id)
            else {
                continue;
            };
            let phase = std::mem::replace(&mut r.phase, Phase::Awaiting);
            match phase {
                Phase::Queued { image, terminal } => {
                    r.phase = Phase::Uploading { terminal };
                    return Some(*image);
                }
                other => r.phase = other,
            }
        }
        None
    }
    pub(crate) fn settle_image(&mut self, idx: usize, id: ThumbnailRequestId) {
        let Some(r) = self
            .entries
            .get_mut(&idx)
            .and_then(Slot::request_mut)
            .filter(|r| r.id == id)
        else {
            return;
        };
        match r.phase {
            Phase::Uploading {
                terminal: ImageTerminal::Complete,
            } => {
                self.remove(&idx);
            }
            Phase::Uploading { terminal } => r.phase = Phase::Settled { terminal },
            _ => panic!("settle must follow next_image"),
        }
    }
    pub(crate) fn discard_images(&mut self) {
        while let Some(msg) = self.next_image() {
            self.settle_image(msg.idx, msg.request_id);
        }
    }
    pub(crate) fn discard_images_outside(&mut self, keep: &HashSet<usize>) {
        let mut retained = VecDeque::new();
        while let Some((idx, id)) = self.upload_order.pop_front() {
            let Some(r) = self
                .entries
                .get_mut(&idx)
                .and_then(Slot::request_mut)
                .filter(|r| r.id == id)
            else {
                continue;
            };
            // Video batches and direct bookmark images have no normal re-enqueue route.
            // Their existing Grid/Rendition consumer decides adoption/discard in poll;
            // nonvideo direct images outside keep are discarded there as before.
            if keep.contains(&idx)
                || matches!(
                    r.kind,
                    ThumbnailRequestKind::VideoBatch | ThumbnailRequestKind::DirectPixels
                )
            {
                retained.push_back((idx, id));
                continue;
            }
            if let Phase::Queued { terminal, .. } = r.phase {
                // The order token is already removed. Settle through the same path as poll,
                // avoiding a scan of the remaining queue for each discarded image.
                r.phase = Phase::Uploading { terminal };
                self.settle_image(idx, id);
            }
        }
        self.upload_order = retained;
    }
    pub(crate) fn images_len(&self) -> usize {
        self.upload_order.len()
    }
    pub(crate) fn blocks_idle_upgrade(&self) -> bool {
        self.legacy_count != 0
    }
    pub(crate) fn legacy_len(&self) -> usize {
        self.legacy_count
    }
    pub(crate) fn video_image_pending(&self, keep: &HashSet<usize>) -> bool {
        keep.iter().any(|idx| {
            self.entries
                .get(idx)
                .and_then(Slot::request)
                .is_some_and(|r| {
                    r.kind == ThumbnailRequestKind::VideoBatch
                        && matches!(
                            r.phase,
                            Phase::Awaiting
                                | Phase::FinalizedAwaiting
                                | Phase::Queued { .. }
                                | Phase::Uploading { .. }
                        )
                })
        })
    }
    pub(crate) fn progress_counts(&self, keep: &HashSet<usize>) -> (usize, usize, usize) {
        let (mut normal, mut upgrade, mut queued_normal) = (0, 0, 0);
        for idx in keep {
            if let Some(r) = self.entries.get(idx).and_then(Slot::request) {
                match r.kind {
                    ThumbnailRequestKind::Normal => {
                        normal += 1;
                        queued_normal += usize::from(matches!(r.phase, Phase::Queued { .. }));
                    }
                    ThumbnailRequestKind::Upgrade => upgrade += 1,
                    _ => {}
                }
            }
        }
        (normal, upgrade, queued_normal)
    }
    pub(crate) fn keys(&self) -> impl Iterator<Item = &usize> {
        self.entries.keys()
    }
    pub(crate) fn iter(&self) -> impl Iterator<Item = (&usize, &bool)> {
        self.entries
            .iter()
            .filter_map(|(idx, slot)| slot.request().map(|r| (idx, r)))
            .filter(|(_, r)| r.kind.legacy())
            .map(|(idx, r)| {
                (
                    idx,
                    if r.kind == ThumbnailRequestKind::Upgrade {
                        &true
                    } else {
                        &false
                    },
                )
            })
    }
    #[cfg(test)]
    pub(crate) fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.entries.len()
    }
    #[cfg(test)]
    pub(crate) fn insert(&mut self, idx: usize, upgrade: bool) -> Option<bool> {
        self.insert_with_edit_preview_epoch(idx, upgrade, None)
    }
    #[cfg(test)]
    pub(crate) fn insert_with_edit_preview_epoch(
        &mut self,
        idx: usize,
        upgrade: bool,
        epoch: Option<u64>,
    ) -> Option<bool> {
        let old = self.remove(&idx);
        self.begin(
            idx,
            0,
            if upgrade {
                ThumbnailRequestKind::Upgrade
            } else {
                ThumbnailRequestKind::Normal
            },
            epoch,
        );
        old
    }
    #[cfg(test)]
    pub(crate) fn correlate_fixture(&mut self, msg: ThumbMsg) -> ThumbMsg {
        if msg.request_id != ThumbnailRequestId::Standalone {
            return msg;
        }
        if !self.entries.contains_key(&msg.idx) {
            if matches!(
                msg.payload,
                ThumbMsgPayload::Finalized | ThumbMsgPayload::Canceled
            ) {
                return msg;
            }
            self.begin(
                msg.idx,
                msg.items_gen,
                ThumbnailRequestKind::Normal,
                msg.pixels().and_then(|p| p.origin.edit_preview_epoch()),
            );
        }
        let Some(r) = self.entries.get_mut(&msg.idx).and_then(Slot::request_mut) else {
            return msg;
        };
        // Old handler fixtures do not model admission. New lifecycle regressions use begin + ID.
        if r.items_gen == 0 {
            r.items_gen = msg.items_gen;
        }
        msg.with_request_id(r.id)
    }
    #[cfg(test)]
    pub(crate) fn images(&self) -> impl Iterator<Item = &ThumbMsg> {
        self.upload_order
            .iter()
            .filter_map(|(idx, id)| {
                self.entries
                    .get(idx)
                    .and_then(Slot::request)
                    .filter(|r| r.id == *id)
            })
            .filter_map(|r| {
                if let Phase::Queued { image, .. } = &r.phase {
                    Some(image.as_ref())
                } else {
                    None
                }
            })
    }
    #[cfg(test)]
    pub(crate) fn snapshot(
        &self,
    ) -> Vec<(
        usize,
        ThumbnailRequestId,
        u64,
        ThumbnailRequestKind,
        &'static str,
    )> {
        let mut rows: Vec<_> = self
            .entries
            .iter()
            .filter_map(|(idx, slot)| slot.request().map(|r| (idx, r)))
            .map(|(idx, r)| {
                (
                    *idx,
                    r.id,
                    r.items_gen,
                    r.kind,
                    match r.phase {
                        Phase::Awaiting => "awaiting",
                        Phase::FinalizedAwaiting => "finalized-awaiting",
                        Phase::Queued {
                            terminal: ImageTerminal::SourceWaiting,
                            ..
                        } => "source-queued",
                        Phase::Queued {
                            terminal: ImageTerminal::SeedWaiting,
                            ..
                        } => "seed-queued",
                        Phase::Queued {
                            terminal: ImageTerminal::Complete,
                            ..
                        } => "terminal-queued",
                        Phase::Uploading {
                            terminal: ImageTerminal::SourceWaiting,
                        } => "source-uploading",
                        Phase::Uploading {
                            terminal: ImageTerminal::SeedWaiting,
                        } => "seed-uploading",
                        Phase::Uploading {
                            terminal: ImageTerminal::Complete,
                        } => "terminal-uploading",
                        Phase::Settled {
                            terminal: ImageTerminal::SourceWaiting,
                        } => "source-settled",
                        Phase::Settled {
                            terminal: ImageTerminal::SeedWaiting,
                        } => "seed-settled",
                        Phase::Settled {
                            terminal: ImageTerminal::Complete,
                        } => unreachable!(),
                    },
                )
            })
            .collect();
        rows.sort_by_key(|r| r.0);
        rows
    }
    #[cfg(test)]
    pub(crate) fn finalized_image_wait(&self, idx: usize) -> bool {
        self.entries
            .get(&idx)
            .and_then(Slot::request)
            .is_some_and(|r| {
                matches!(
                    r.phase,
                    Phase::FinalizedAwaiting
                        | Phase::Queued {
                            terminal: ImageTerminal::Complete,
                            ..
                        }
                )
            })
    }
    #[cfg(test)]
    pub(crate) fn any_finalized_image_wait(&self) -> bool {
        self.entries
            .keys()
            .any(|idx| self.finalized_image_wait(*idx))
    }
    #[cfg(test)]
    pub(crate) fn mark_finalized_for_test(&mut self, idx: usize) {
        if !self.contains_key(&idx) {
            self.insert(idx, false);
        }
        let r = self.entries.get(&idx).and_then(Slot::request).unwrap();
        let msg = ThumbMsg::from_legacy_parts(
            idx,
            None,
            crate::thumb_loader::ThumbLoadOrigin::SourceIntrinsic,
            false,
            None,
            None,
            None,
            false,
            true,
            0,
            r.items_gen,
        )
        .with_request_id(r.id);
        self.receive(msg);
    }
}
#[cfg(test)]
impl Extend<(usize, bool)> for ThumbnailRequests {
    fn extend<T: IntoIterator<Item = (usize, bool)>>(&mut self, iter: T) {
        for (idx, upgrade) in iter {
            self.insert(idx, upgrade);
        }
    }
}

#[cfg(test)]
mod diagnostic_projection_tests {
    use super::*;
    use crate::thumb_loader::{ThumbLoadOrigin, ThumbPixels};

    fn source(idx: usize, id: ThumbnailRequestId, items_gen: u64) -> ThumbMsg {
        ThumbMsg {
            idx,
            request_id: id,
            items_gen,
            input_seq: 0,
            payload: ThumbMsgPayload::SourcePixels(ThumbPixels {
                image: egui::ColorImage::new([1, 1], vec![egui::Color32::WHITE]),
                origin: ThumbLoadOrigin::SourceGenerated {
                    evaluated_display_px: 128,
                },
                from_edit_preview: false,
                edit_preview_adjustment: None,
                source_coverage: crate::source_coverage::SourceCoverage::FullSource,
                source_dims: Some((1, 1)),
                layout_dims: None,
            }),
        }
    }

    fn finalized(idx: usize, id: ThumbnailRequestId, items_gen: u64) -> ThumbMsg {
        ThumbMsg {
            idx,
            request_id: id,
            items_gen,
            input_seq: 0,
            payload: ThumbMsgPayload::Finalized,
        }
    }

    #[test]
    fn same_request_finalized_projects_real_queued_image_before_receive() {
        let mut owner = ThumbnailRequests::default();
        let id = owner
            .begin(4, 7, ThumbnailRequestKind::Normal, None)
            .unwrap();
        owner.receive(source(4, id, 7));
        let before = owner.diagnostic_projection(4, id, 7).unwrap();
        assert!(before.image_backlog_contains_request);
        assert!(before.worker_pending);
        owner.receive(finalized(4, id, 7));
        let after = owner.diagnostic_projection(4, id, 7).unwrap();
        assert!(after.image_backlog_contains_request);
        assert!(!after.worker_pending);
        assert_eq!(owner.diagnostic_requests(), vec![after]);
        let image = owner.next_image().unwrap();
        assert!(
            !owner
                .diagnostic_projection(4, id, 7)
                .unwrap()
                .image_backlog_contains_request
        );
        owner.settle_image(image.idx, image.request_id);
        assert!(owner.diagnostic_projection(4, id, 7).is_none());
    }

    #[test]
    fn other_request_or_generation_cannot_claim_queued_image() {
        let mut owner = ThumbnailRequests::default();
        let id = owner
            .begin(4, 7, ThumbnailRequestKind::Normal, None)
            .unwrap();
        let other = owner
            .begin(5, 7, ThumbnailRequestKind::Normal, None)
            .unwrap();
        owner.receive(source(4, id, 7));
        assert!(owner.diagnostic_projection(4, other, 7).is_none());
        assert!(owner.diagnostic_projection(4, id, 8).is_none());
        assert!(
            owner
                .diagnostic_projection(4, ThumbnailRequestId::Standalone, 7)
                .is_none()
        );
        assert!(
            !owner
                .diagnostic_projection(5, other, 7)
                .unwrap()
                .image_backlog_contains_request
        );
        assert!(matches!(
            owner.receive(finalized(4, other, 7)),
            ThumbnailReceipt::Ignored
        ));
        assert!(
            owner
                .diagnostic_projection(4, id, 7)
                .unwrap()
                .worker_pending
        );
    }

    #[test]
    fn finalized_without_queue_and_discarded_images_do_not_claim_membership() {
        let mut owner = ThumbnailRequests::default();
        let id = owner
            .begin(4, 7, ThumbnailRequestKind::Normal, None)
            .unwrap();
        owner.receive(finalized(4, id, 7));
        let projection = owner.diagnostic_projection(4, id, 7).unwrap();
        assert!(!projection.image_backlog_contains_request);
        assert!(!projection.worker_pending);
        owner.receive(source(4, id, 7));
        owner.discard_images();
        assert!(owner.diagnostic_requests().is_empty());

        let id = owner
            .begin(4, 8, ThumbnailRequestKind::Normal, None)
            .unwrap();
        owner.receive(source(4, id, 8));
        owner.discard_images();
        let projection = owner.diagnostic_projection(4, id, 8).unwrap();
        assert!(!projection.image_backlog_contains_request);
        assert!(projection.worker_pending);
        owner.receive(finalized(4, id, 8));
        assert!(owner.diagnostic_requests().is_empty());
    }
}

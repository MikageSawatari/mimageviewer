//! Worker-only reuse of a previously validated, immutable sidecar parse.
//!
//! Reuse never skips the writer fence owned by `SidecarRestoreState`, the full-byte
//! disk identity, the requested SQLite markers, or the final source check.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use super::{
    ImportFamilies, MarkerStore, SidecarImportProbe, SidecarProbeFamilyOutcome, probe,
    probe_result, read_marker,
};
use crate::sidecar::{
    SidecarDiskReuseRevalidationError, SidecarDiskToken, SidecarFile,
    revalidate_disk_import_source_for_reuse,
};

/// Budget is charged to the parsed proof. The visible writable owner receives a
/// separate outer map on the Checking worker; large inner raster Arcs stay shared.
const MAX_PROOF_SOURCE_BYTES: u64 = 8 * 1024 * 1024;
pub(crate) const MAX_PROOF_BYTES: u64 = 192 * 1024 * 1024;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidecarProbeReuseMissReason {
    NoPriorProof,
    KeyMismatch,
    Cancelled,
    PendingWriter,
    WriterFailed,
    WriterStateUnavailable,
    TokenMismatch,
    MarkerMismatch,
    MarkerReadFailed,
    BudgetRejected,
    Ineligible,
    AdmissionPaused,
}

impl SidecarProbeReuseMissReason {
    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::NoPriorProof => "no_prior_proof",
            Self::KeyMismatch => "key_mismatch",
            Self::Cancelled => "cancelled",
            Self::PendingWriter => "pending_writer",
            Self::WriterFailed => "writer_failed",
            Self::WriterStateUnavailable => "writer_state_unavailable",
            Self::TokenMismatch => "token_mismatch",
            Self::MarkerMismatch => "marker_mismatch",
            Self::MarkerReadFailed => "marker_read_failed",
            Self::BudgetRejected => "budget_rejected",
            Self::Ineligible => "ineligible",
            Self::AdmissionPaused => "admission_paused",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum SidecarProbeReuseOutcome {
    Hit,
    Miss(SidecarProbeReuseMissReason),
}

impl SidecarProbeReuseOutcome {
    pub(crate) const fn reused(self) -> bool {
        matches!(self, Self::Hit)
    }

    pub(crate) const fn label(self) -> &'static str {
        match self {
            Self::Hit => "hit",
            Self::Miss(reason) => reason.label(),
        }
    }
}

pub(crate) struct SidecarProbeProof {
    folder_key: String,
    data_dir: PathBuf,
    families: ImportFamilies,
    sidecar: SidecarFile,
    source: SidecarDiskToken,
    estimated_heap_bytes: u64,
}

impl SidecarProbeProof {
    pub(crate) fn folder_key(&self) -> &str {
        &self.folder_key
    }

    pub(crate) fn estimated_heap_bytes(&self) -> u64 {
        self.estimated_heap_bytes
    }

    #[cfg(test)]
    pub(crate) fn sidecar_for_test(&self) -> &SidecarFile {
        &self.sidecar
    }

    pub(crate) fn matches(&self, folder: &Path, data_dir: &Path, families: ImportFamilies) -> bool {
        self.sidecar.folder() == folder
            && self.folder_key == crate::adjustment_db::normalize_path(folder)
            && self.data_dir == data_dir
            && self.families == families
    }

    fn try_current(
        &self,
        folder: &Path,
        data_dir: &Path,
        families: ImportFamilies,
        cancel: &AtomicBool,
    ) -> Result<SidecarImportProbe, SidecarProbeReuseMissReason> {
        self.try_current_with_revalidate(
            folder,
            data_dir,
            families,
            cancel,
            revalidate_disk_import_source_for_reuse,
        )
    }

    fn try_current_with_revalidate(
        &self,
        folder: &Path,
        data_dir: &Path,
        families: ImportFamilies,
        cancel: &AtomicBool,
        revalidate: impl Fn(&Path, &SidecarDiskToken) -> Result<(), SidecarDiskReuseRevalidationError>,
    ) -> Result<SidecarImportProbe, SidecarProbeReuseMissReason> {
        if !self.matches(folder, data_dir, families) {
            return Err(SidecarProbeReuseMissReason::KeyMismatch);
        }
        if cancel.load(Ordering::Relaxed) {
            return Err(SidecarProbeReuseMissReason::Cancelled);
        }
        let started = Instant::now();
        // This checks pending/failed writer before metadata-before, full bytes SHA-256,
        // and metadata-after. A metadata-only match is never a proof.
        revalidate(folder, &self.source).map_err(revalidation_miss_reason)?;
        let load_elapsed = started.elapsed();
        let folder_key = &self.folder_key;
        let family = |requested: bool, store: MarkerStore| {
            if !requested {
                return Ok(SidecarProbeFamilyOutcome::NotRequested);
            }
            match read_marker(data_dir, folder_key, store)
                .map_err(|_| SidecarProbeReuseMissReason::MarkerReadFailed)?
            {
                Some(marker) if marker == self.source.sync_marker() => {
                    Ok(SidecarProbeFamilyOutcome::AlreadySynchronized)
                }
                _ => Err(SidecarProbeReuseMissReason::MarkerMismatch),
            }
        };
        let edits = family(families.edits, MarkerStore::Edits)?;
        let tags = family(families.tags, MarkerStore::Tags)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(SidecarProbeReuseMissReason::Cancelled);
        }
        // Keep the original probe's after-marker source linearization point.
        revalidate(folder, &self.source).map_err(revalidation_miss_reason)?;
        if cancel.load(Ordering::Relaxed) {
            return Err(SidecarProbeReuseMissReason::Cancelled);
        }
        Ok(SidecarImportProbe::Current {
            sidecar: self
                .sidecar
                .clone_clean_import_snapshot()
                .ok_or(SidecarProbeReuseMissReason::Ineligible)?,
            source: Some(self.source.clone()),
            result: probe_result(started, load_elapsed, Duration::ZERO, edits, tags),
        })
    }
}

fn revalidation_miss_reason(
    error: SidecarDiskReuseRevalidationError,
) -> SidecarProbeReuseMissReason {
    match error {
        SidecarDiskReuseRevalidationError::PendingWriter => {
            SidecarProbeReuseMissReason::PendingWriter
        }
        SidecarDiskReuseRevalidationError::WriterFailed => {
            SidecarProbeReuseMissReason::WriterFailed
        }
        SidecarDiskReuseRevalidationError::WriterStateUnavailable => {
            SidecarProbeReuseMissReason::WriterStateUnavailable
        }
        SidecarDiskReuseRevalidationError::TokenMismatch => {
            SidecarProbeReuseMissReason::TokenMismatch
        }
    }
}

pub(crate) struct ProbeForRestore {
    pub(crate) probe: SidecarImportProbe,
    pub(crate) proof_candidate: Option<Arc<SidecarProbeProof>>,
    pub(crate) reuse: SidecarProbeReuseOutcome,
}

/// Called only inside the existing Checking worker, after flush/strict idle.
pub(crate) fn probe_for_restore(
    folder: &Path,
    data_dir: &Path,
    families: ImportFamilies,
    cancel: &AtomicBool,
    prior: Option<&Arc<SidecarProbeProof>>,
    allow_new_proof: bool,
) -> ProbeForRestore {
    let mut reuse = SidecarProbeReuseOutcome::Miss(SidecarProbeReuseMissReason::NoPriorProof);
    if let Some(prior) = prior {
        match prior.try_current(folder, data_dir, families, cancel) {
            Ok(mut probe) => {
                detach_writable_owner(&mut probe);
                return ProbeForRestore {
                    probe,
                    proof_candidate: Some(Arc::clone(prior)),
                    reuse: SidecarProbeReuseOutcome::Hit,
                };
            }
            Err(reason) => reuse = SidecarProbeReuseOutcome::Miss(reason),
        }
    }
    let mut probe = probe(folder, data_dir, families, cancel);
    let proof_candidate = if allow_new_proof {
        match proof_candidate(&probe, folder, data_dir, families) {
            Ok(proof) => Some(proof),
            Err(reason) => {
                if matches!(
                    reuse,
                    SidecarProbeReuseOutcome::Miss(SidecarProbeReuseMissReason::NoPriorProof)
                ) {
                    reuse = SidecarProbeReuseOutcome::Miss(reason);
                }
                None
            }
        }
    } else {
        if matches!(
            reuse,
            SidecarProbeReuseOutcome::Miss(SidecarProbeReuseMissReason::NoPriorProof)
        ) {
            reuse = SidecarProbeReuseOutcome::Miss(SidecarProbeReuseMissReason::AdmissionPaused);
        }
        None
    };
    if proof_candidate.is_some() {
        detach_writable_owner(&mut probe);
    }
    ProbeForRestore {
        probe,
        proof_candidate,
        reuse,
    }
}

fn detach_writable_owner(probe: &mut SidecarImportProbe) {
    if let SidecarImportProbe::Current { sidecar, .. } = probe {
        sidecar.detach_outer_items_for_writable_owner();
    }
}

pub(super) fn proof_candidate(
    probe: &SidecarImportProbe,
    folder: &Path,
    data_dir: &Path,
    families: ImportFamilies,
) -> Result<Arc<SidecarProbeProof>, SidecarProbeReuseMissReason> {
    if !families.edits && !families.tags {
        return Err(SidecarProbeReuseMissReason::Ineligible);
    }
    let SidecarImportProbe::Current {
        sidecar,
        result,
        source: Some(source),
    } = probe
    else {
        return Err(SidecarProbeReuseMissReason::Ineligible);
    };
    if result.source_validation_error.is_some()
        || sidecar.is_dirty()
        || !family_synchronized(families.edits, &result.edits)
        || !family_synchronized(families.tags, &result.tags)
        || sidecar.folder() != folder
    {
        return Err(SidecarProbeReuseMissReason::Ineligible);
    }
    let estimated_heap_bytes =
        estimate_proof_heap(source, sidecar).ok_or(SidecarProbeReuseMissReason::BudgetRejected)?;
    Ok(Arc::new(SidecarProbeProof {
        folder_key: crate::adjustment_db::normalize_path(folder),
        data_dir: data_dir.to_path_buf(),
        families,
        sidecar: sidecar
            .clone_clean_import_snapshot()
            .ok_or(SidecarProbeReuseMissReason::Ineligible)?,
        source: source.clone(),
        estimated_heap_bytes,
    }))
}

fn family_synchronized(requested: bool, outcome: &SidecarProbeFamilyOutcome) -> bool {
    matches!(
        (requested, outcome),
        (true, SidecarProbeFamilyOutcome::AlreadySynchronized)
            | (false, SidecarProbeFamilyOutcome::NotRequested)
    )
}

/// Charge JSON-backed variable strings at 8× disk bytes, and separately count
/// map nodes, inline structs, Vec capacities, and decompressed raster buffers.
/// Saturation rejects a proof. Shared Arcs are deliberately overcounted.
fn estimate_proof_heap(source: &SidecarDiskToken, sidecar: &SidecarFile) -> Option<u64> {
    if !proof_source_bytes_allowed(source.byte_len()) {
        return None;
    }
    let mut bytes = source.byte_len().checked_mul(8)?;
    for entry in sidecar.items().values() {
        // BTreeMap node slots/allocator slack plus the inline String and entry.
        bytes = bytes.checked_add(
            (std::mem::size_of::<String>()
                + std::mem::size_of::<crate::sidecar::SidecarEntry>()
                + 256) as u64,
        )?;
        for mask in [entry.mask.as_ref(), entry.conceal.as_ref()]
            .into_iter()
            .flatten()
        {
            bytes = bytes.checked_add(
                (mask.vectors.capacity() * std::mem::size_of::<crate::mask_db::Shape>()) as u64,
            )?;
        }
        if let Some(tags) = &entry.tags {
            bytes = bytes.checked_add((tags.capacity() * std::mem::size_of::<String>()) as u64)?;
        }
        if let Some(comic) = &entry.comic {
            bytes = bytes.checked_add(
                (comic.capacity() * std::mem::size_of::<comic_core::AnnotationObject>()) as u64,
            )?;
        }
        if let Some(layers) = &entry.local_adjust_layers {
            use local_adjust_core::LocalMask;
            bytes = bytes.checked_add(
                (layers.capacity() * std::mem::size_of::<local_adjust_core::LocalAdjustmentLayer>())
                    as u64,
            )?;
            for layer in layers.iter() {
                let raster = |mask: &LocalMask| -> u64 {
                    match mask {
                        LocalMask::Raster(mask) => mask.alpha.capacity() as u64 * 4,
                        LocalMask::RasterVector(mask) => mask.alpha.capacity() as u64 * 4,
                        LocalMask::Subject(mask) => {
                            mask.alpha.capacity() as u64 * 4
                                + mask
                                    .source_alpha
                                    .as_ref()
                                    .map_or(0, |v| v.capacity() as u64 * 4)
                        }
                        LocalMask::Segmentation(mask) => {
                            mask.labels.capacity() as u64 * 4 + mask.selected.capacity() as u64
                        }
                        _ => 0,
                    }
                };
                bytes = bytes.checked_add(raster(&layer.mask))?;
                if let LocalMask::RasterVector(mask) = &layer.mask {
                    bytes = bytes.checked_add(
                        (mask.shapes.capacity()
                            * std::mem::size_of::<local_adjust_core::MaskShape>())
                            as u64,
                    )?;
                }
                for mask in [
                    layer.manual_override.add.as_ref(),
                    layer.manual_override.subtract.as_ref(),
                ]
                .into_iter()
                .flatten()
                {
                    bytes = bytes.checked_add(mask.alpha.capacity() as u64 * 4)?;
                    bytes = bytes.checked_add(
                        (mask.shapes.capacity()
                            * std::mem::size_of::<local_adjust_core::MaskShape>())
                            as u64,
                    )?;
                }
            }
        }
        if !proof_heap_bytes_allowed(bytes) {
            return None;
        }
    }
    proof_heap_bytes_allowed(bytes).then_some(bytes)
}

const fn proof_source_bytes_allowed(bytes: u64) -> bool {
    bytes <= MAX_PROOF_SOURCE_BYTES
}

const fn proof_heap_bytes_allowed(bytes: u64) -> bool {
    bytes <= MAX_PROOF_BYTES
}

#[cfg(test)]
mod tests {
    use super::*;

    fn synchronized_proof() -> (tempfile::TempDir, tempfile::TempDir, Arc<SidecarProbeProof>) {
        let media = tempfile::tempdir().unwrap();
        let data = tempfile::tempdir().unwrap();
        drop(
            crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
                .unwrap(),
        );
        drop(crate::tags_db::TagsDb::open_at(&data.path().join("tags.db")).unwrap());
        let mut sidecar = SidecarFile::new(media.path().to_path_buf());
        sidecar.set_tags("page.jpg", ["#saved"]);
        assert!(sidecar.flush_blocking());
        let crate::sidecar::SidecarImportLoad::Loaded(loaded) =
            SidecarFile::load_for_import(media.path())
        else {
            panic!("fixture sidecar must load");
        };
        let (_, crate::sidecar::SidecarImportSource::Disk(token)) = loaded.into_parts() else {
            panic!("fixture must have a disk token");
        };
        let folder_key = crate::adjustment_db::normalize_path(media.path());
        crate::adjustment_db::AdjustmentDb::open_at(&data.path().join("adjustment.db"))
            .unwrap()
            .sidecar_sync_upsert(&folder_key, token.sync_marker())
            .unwrap();
        crate::tags_db::TagsDb::open_at(&data.path().join("tags.db"))
            .unwrap()
            .sidecar_sync_upsert(&folder_key, token.sync_marker())
            .unwrap();
        let checked = probe_for_restore(
            media.path(),
            data.path(),
            ImportFamilies::ALL,
            &AtomicBool::new(false),
            None,
            true,
        );
        let proof = checked.proof_candidate.expect("synchronized proof");
        (media, data, proof)
    }

    #[test]
    fn writer_revalidation_failures_keep_distinct_reuse_reasons() {
        assert_eq!(
            revalidation_miss_reason(SidecarDiskReuseRevalidationError::PendingWriter),
            SidecarProbeReuseMissReason::PendingWriter
        );
        assert_eq!(
            revalidation_miss_reason(SidecarDiskReuseRevalidationError::WriterFailed),
            SidecarProbeReuseMissReason::WriterFailed
        );
        assert_eq!(
            revalidation_miss_reason(SidecarDiskReuseRevalidationError::WriterStateUnavailable),
            SidecarProbeReuseMissReason::WriterStateUnavailable
        );
        assert_eq!(
            revalidation_miss_reason(SidecarDiskReuseRevalidationError::TokenMismatch),
            SidecarProbeReuseMissReason::TokenMismatch
        );
    }

    #[test]
    fn retained_proof_rejects_pending_and_failed_writer_states() {
        let (media, data, proof) = synchronized_proof();
        for (error, expected) in [
            (
                SidecarDiskReuseRevalidationError::PendingWriter,
                SidecarProbeReuseMissReason::PendingWriter,
            ),
            (
                SidecarDiskReuseRevalidationError::WriterFailed,
                SidecarProbeReuseMissReason::WriterFailed,
            ),
        ] {
            let result = proof.try_current_with_revalidate(
                media.path(),
                data.path(),
                ImportFamilies::ALL,
                &AtomicBool::new(false),
                move |_, _| Err(error),
            );
            assert!(matches!(result, Err(reason) if reason == expected));
        }
    }

    #[test]
    fn proof_source_and_heap_budgets_are_inclusive_and_bounded() {
        assert!(proof_source_bytes_allowed(MAX_PROOF_SOURCE_BYTES));
        assert!(!proof_source_bytes_allowed(MAX_PROOF_SOURCE_BYTES + 1));
        assert!(proof_heap_bytes_allowed(MAX_PROOF_BYTES));
        assert!(!proof_heap_bytes_allowed(MAX_PROOF_BYTES + 1));
    }
}

//! C1–C12 value contracts. These functions are pure and never touch stored rows.
use std::{
    collections::HashMap,
    path::Path,
    sync::{Arc, mpsc},
};

use super::{ShiftReadReply, SpreadShiftService};
use crate::{
    app::ViewerContextId,
    book_resume_db::{BookResumeWriter, PageIdentity, PositionReadReply, ReadingPosition},
    book_scope::{
        BookScopeKey, normalize_relative_reference, relative_reference_from_root,
        validate_relative_reference,
    },
    settings::{
        PageAloneSettings, ReadingDirection, ReadingFlow, SingletonSpreadEndpointSettings,
        SpreadMode,
    },
    ui_fullscreen::SpreadDisplayComposition,
};

/// Wire v1. Document indices are the explicitly accepted G Q6 physical-page exception.
#[derive(Clone, Debug, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    content = "value",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum SpreadShiftPageRef {
    RelativePath(String),
    ArchiveEntry(String),
    DocumentPageIdentity(u32),
}

impl SpreadShiftPageRef {
    fn lookup_key(&self) -> Result<Self, String> {
        Ok(match self {
            Self::RelativePath(path) => Self::RelativePath(normalize_relative_reference(path)?),
            Self::ArchiveEntry(path) => Self::ArchiveEntry(normalize_relative_reference(path)?),
            Self::DocumentPageIdentity(page) => Self::DocumentPageIdentity(*page),
        })
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        match self {
            Self::RelativePath(path) | Self::ArchiveEntry(path) => {
                validate_relative_reference(path)
            }
            Self::DocumentPageIdentity(_) => Ok(()),
        }
    }

    /// Convert an existing absolute resume identity; never persist a physical cache path.
    pub(crate) fn from_identity(
        scope: &BookScopeKey,
        identity: &PageIdentity,
    ) -> Result<Self, String> {
        scope.validate()?;
        let root = crate::path_key::normalize_keep_drive(&scope.logical_root);
        let value = match identity {
            PageIdentity::Image { path } => {
                Self::RelativePath(relative_reference_from_root(&scope.logical_root, path)?)
            }
            PageIdentity::Archive {
                container,
                entry_name,
            } => {
                if crate::path_key::normalize_keep_drive(Path::new(container)) != root {
                    return Err("archive identity belongs to another logical root".into());
                }
                // Full outer entry chain; level_prefix must not be prepended a second time.
                Self::ArchiveEntry(entry_name.clone())
            }
            PageIdentity::Document {
                container,
                page_num,
            } => {
                if crate::path_key::normalize_keep_drive(Path::new(container)) != root
                    || !scope.level_prefix.is_empty()
                {
                    return Err("document identity belongs to another scope".into());
                }
                Self::DocumentPageIdentity(
                    u32::try_from(*page_num).map_err(|_| "document page exceeds wire range")?,
                )
            }
        };
        value.validate()?;
        Ok(value)
    }

    pub(crate) fn to_identity(&self, scope: &BookScopeKey) -> Result<PageIdentity, String> {
        self.validate()?;
        scope.validate()?;
        let root = crate::path_key::normalize_keep_drive(&scope.logical_root);
        Ok(match self {
            Self::RelativePath(path) => PageIdentity::Image {
                path: format!("{}/{}", root.trim_end_matches('/'), path),
            },
            Self::ArchiveEntry(path) => PageIdentity::Archive {
                container: root,
                entry_name: path.clone(),
            },
            Self::DocumentPageIdentity(page_num) => PageIdentity::Document {
                container: root,
                page_num: *page_num as usize,
            },
        })
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RestoreReceipt {
    pub(crate) open_id: u64,
    pub(crate) context_id: ViewerContextId,
    pub(crate) scope: BookScopeKey,
    pub(crate) items_generation: u64,
    pub(crate) settings_generation: u64,
    pub(crate) shift_cutoff: u64,
    pub(crate) resume_cutoff: u64,
}

impl RestoreReceipt {
    pub(crate) fn capture(
        open_id: u64,
        context_id: ViewerContextId,
        scope: BookScopeKey,
        items_generation: u64,
        settings_generation: u64,
        service: &SpreadShiftService,
        resume_writer: Option<&BookResumeWriter>,
    ) -> Self {
        Self {
            open_id,
            context_id,
            scope,
            items_generation,
            settings_generation,
            shift_cutoff: service.watermarks().admitted,
            resume_cutoff: resume_writer.map_or(0, |writer| writer.watermarks().admitted),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShiftRestoreCause {
    FreshOpen,
    RetainedReadoption,
    CompositionRemoved,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShiftMemoryPolicy {
    Remember,
    SessionOnly,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PairingEditAvailability {
    Automatic,
    Composed,
    Unreadable,
    ExtractedResult,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ShiftResolution {
    Resolved(usize),
    NotInView,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ShiftPersistence {
    SessionOnly,
    Saving(u64),
    Saved(u64),
    SaveFailed,
}

/// The admitted operation survives its viewer so completion notifications can
/// describe the write even after the original context has retired.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ShiftWriteKind {
    Set,
    Clear,
}
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) enum SpreadShiftState {
    #[default]
    None,
    Adjusted {
        anchor_ref: SpreadShiftPageRef,
        resolution: ShiftResolution,
        persistence: ShiftPersistence,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolvedShift {
    pub(crate) anchor_ref: SpreadShiftPageRef,
    pub(crate) anchor_idx: usize,
    pub(crate) manifest_generation: u64,
    pub(crate) nav_generation: u64,
}

/// Already resolved by the existing spread owner, including effective endpoint choices.
/// The shift resolver does not construct navigation units or reimplement these rules.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ResolvedAutomaticRules {
    pub(crate) mode: SpreadMode,
    pub(crate) flow: ReadingFlow,
    pub(crate) direction: ReadingDirection,
    pub(crate) final_cover: bool,
    pub(crate) page_alone: PageAloneSettings,
    pub(crate) singleton_endpoints: SingletonSpreadEndpointSettings,
    pub(crate) landscape_epoch: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShiftManifest {
    pub(crate) generation: u64,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShiftNavOrder {
    pub(crate) generation: u64,
    pub(crate) indices: Arc<[usize]>,
}

/// Prepared alongside the accepted rows. Identity resolution never substitutes a raw index.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct ShiftIdentityIndex(Arc<HashMap<SpreadShiftPageRef, usize>>);
impl ShiftIdentityIndex {
    pub(crate) fn prepare(
        scope: &BookScopeKey,
        identities: impl IntoIterator<Item = (PageIdentity, usize)>,
    ) -> Result<Self, String> {
        let mut index = HashMap::new();
        for (identity, idx) in identities {
            let reference = SpreadShiftPageRef::from_identity(scope, &identity)?;
            index.entry(reference.lookup_key()?).or_insert(idx);
        }
        Ok(Self(Arc::new(index)))
    }
    pub(crate) fn resolve(&self, reference: &SpreadShiftPageRef) -> Option<usize> {
        self.0.get(&reference.lookup_key().ok()?).copied()
    }
}

#[derive(Clone, Debug)]
pub(crate) struct ResolveShiftRequest {
    pub(crate) receipt: RestoreReceipt,
    pub(crate) cause: ShiftRestoreCause,
    pub(crate) scope: BookScopeKey,
    pub(crate) manifest: ShiftManifest,
    pub(crate) nav_order: ShiftNavOrder,
    pub(crate) identity_index: ShiftIdentityIndex,
    pub(crate) rules: ResolvedAutomaticRules,
    pub(crate) memory_policy: ShiftMemoryPolicy,
    pub(crate) layout_eligibility: PairingEditAvailability,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ResolveShiftReply {
    pub(crate) receipt: RestoreReceipt,
    pub(crate) manifest_generation: u64,
    pub(crate) nav_generation: u64,
    pub(crate) maintenance_epoch: u64,
    pub(crate) state: SpreadShiftState,
    pub(crate) optional_shift: Option<ResolvedShift>,
}

/// G supplies its composition value; this module owns only the Automatic shift projection.
pub(crate) enum PreparedLayoutRecipe<C> {
    Automatic {
        shift_read: Option<ShiftReadReply>,
        retained_state: SpreadShiftState,
        maintenance_epoch: u64,
    },
    Composed(C),
    Unreadable(String),
    ExtractedResult,
}
pub(crate) enum BookLayoutSource<C> {
    Automatic {
        rules: ResolvedAutomaticRules,
        shift: ResolveShiftReply,
    },
    Composed(C),
    Unreadable(String),
    ExtractedResult,
}

pub(crate) fn resolve_book_layout_source<C>(
    prepared_recipe: PreparedLayoutRecipe<C>,
    request: ResolveShiftRequest,
) -> Result<BookLayoutSource<C>, String> {
    // Composition classification happens before any read or resolution of dormant rows.
    let (shift_read, retained_state, maintenance_epoch) = match prepared_recipe {
        PreparedLayoutRecipe::Composed(value) => return Ok(BookLayoutSource::Composed(value)),
        PreparedLayoutRecipe::Unreadable(error) => return Ok(BookLayoutSource::Unreadable(error)),
        PreparedLayoutRecipe::ExtractedResult => return Ok(BookLayoutSource::ExtractedResult),
        PreparedLayoutRecipe::Automatic {
            shift_read,
            retained_state,
            maintenance_epoch,
        } => (shift_read, retained_state, maintenance_epoch),
    };
    if request.layout_eligibility != PairingEditAvailability::Automatic
        || request.scope != request.receipt.scope
        || request.manifest.generation != request.receipt.items_generation
    {
        return Err("shift request does not match its accepted book recipe".into());
    }
    request.scope.validate()?;
    let state = if request.memory_policy == ShiftMemoryPolicy::SessionOnly {
        if request.cause == ShiftRestoreCause::RetainedReadoption {
            retained_state
        } else {
            SpreadShiftState::None
        }
    } else {
        let read = shift_read.ok_or_else(|| "shift read dependency is incomplete".to_string())?;
        validate_shift_read(&request.receipt, maintenance_epoch, &read)?;
        match read.row {
            None => SpreadShiftState::None,
            Some(anchor_ref) => SpreadShiftState::Adjusted {
                anchor_ref,
                resolution: ShiftResolution::NotInView,
                persistence: ShiftPersistence::Saved(read.completed_through),
            },
        }
    };
    let (state, optional_shift) = resolve_state(state, &request);
    Ok(BookLayoutSource::Automatic {
        rules: request.rules,
        shift: ResolveShiftReply {
            receipt: request.receipt,
            manifest_generation: request.manifest.generation,
            nav_generation: request.nav_order.generation,
            maintenance_epoch,
            state,
            optional_shift,
        },
    })
}

fn resolve_state(
    state: SpreadShiftState,
    request: &ResolveShiftRequest,
) -> (SpreadShiftState, Option<ResolvedShift>) {
    match state {
        SpreadShiftState::None => (SpreadShiftState::None, None),
        SpreadShiftState::Adjusted {
            anchor_ref,
            persistence,
            ..
        } => {
            let idx = request
                .identity_index
                .resolve(&anchor_ref)
                .filter(|idx| request.nav_order.indices.contains(idx));
            let resolution = idx.map_or(ShiftResolution::NotInView, ShiftResolution::Resolved);
            let optional_shift = idx
                .filter(|_| request.rules.mode.is_spread())
                .map(|anchor_idx| ResolvedShift {
                    anchor_ref: anchor_ref.clone(),
                    anchor_idx,
                    manifest_generation: request.manifest.generation,
                    nav_generation: request.nav_order.generation,
                });
            (
                SpreadShiftState::Adjusted {
                    anchor_ref,
                    resolution,
                    persistence,
                },
                optional_shift,
            )
        }
    }
}

impl ResolveShiftReply {
    /// The consuming continuation takes the reply once; unrelated/superseded contexts fail.
    pub(crate) fn matches_adoption(
        &self,
        receipt: &RestoreReceipt,
        nav_generation: u64,
        maintenance_epoch: u64,
    ) -> bool {
        &self.receipt == receipt
            && self.manifest_generation == receipt.items_generation
            && self.nav_generation == nav_generation
            && self.maintenance_epoch == maintenance_epoch
    }
}

#[derive(Clone, Debug)]
pub(crate) struct EffectiveLayoutSnapshot<T> {
    pub(crate) scope: BookScopeKey,
    pub(crate) context_id: ViewerContextId,
    pub(crate) items_generation: u64,
    pub(crate) nav_generation: u64,
    pub(crate) settings_generation: u64,
    pub(crate) rules: ResolvedAutomaticRules,
    pub(crate) live_shift: SpreadShiftState,
    pub(crate) units: Arc<[SpreadDisplayComposition]>,
    pub(crate) layout_token: T,
}
/// Borrowed accepted owner state, captured in one short Preparing admission boundary.
pub(crate) struct AutomaticContextToken<'a, T> {
    pub(crate) effective: &'a EffectiveLayoutSnapshot<T>,
}
pub(crate) fn capture_effective_automatic_layout<T: Clone>(
    context_token: AutomaticContextToken<'_, T>,
) -> EffectiveLayoutSnapshot<T> {
    context_token.effective.clone()
}

pub(crate) struct PairingContextToken<'a> {
    pub(crate) rules: &'a ResolvedAutomaticRules,
    pub(crate) availability: PairingEditAvailability,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PairingAction {
    Nudge,
    ClearShift,
    SetMemoryPolicy,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PairingAdmission {
    Accepted(PairingOperation),
    Rejected(&'static str),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PairingOperation {
    EditShift,
    NavigationFallback,
}
pub(crate) fn admit_pairing_action(
    context_token: PairingContextToken<'_>,
    action: PairingAction,
) -> PairingAdmission {
    if action == PairingAction::Nudge && !context_token.rules.mode.is_spread() {
        return PairingAdmission::Accepted(PairingOperation::NavigationFallback);
    }
    if context_token.availability == PairingEditAvailability::Automatic {
        PairingAdmission::Accepted(PairingOperation::EditShift)
    } else {
        PairingAdmission::Rejected(pairing_rejection(context_token.availability))
    }
}
fn pairing_rejection(availability: PairingEditAvailability) -> &'static str {
    match availability {
        PairingEditAvailability::Composed => {
            "本の構成で設定されています。組を変えるには本の構成を編集してください"
        }
        PairingEditAvailability::Unreadable => {
            "本の構成を読み込めないため、見開きの調整を変更できません"
        }
        PairingEditAvailability::ExtractedResult => {
            "抽出したページでは本の見開きの調整を変更できません"
        }
        PairingEditAvailability::Automatic => "",
    }
}

fn validate_shift_completion(
    receipt: &RestoreReceipt,
    read: &ShiftReadReply,
) -> Result<(), String> {
    if read.cutoff != receipt.shift_cutoff || read.completed_through < receipt.shift_cutoff {
        return Err("shift completion proof does not match restore receipt".into());
    }
    read.status.clone()
}

fn validate_shift_read(
    receipt: &RestoreReceipt,
    epoch: u64,
    read: &ShiftReadReply,
) -> Result<(), String> {
    validate_shift_completion(receipt, read)?;
    if read.maintenance_epoch != epoch {
        return Err("shift completion proof does not match restore receipt".into());
    }
    Ok(())
}

/// The D2 typed open continuation embeds this dependency owner. Dropping it cancels
/// only adoption, never already admitted saves. Both values are fixed at their read points.
pub(crate) struct RestoreReadDependencies {
    receipt: RestoreReceipt,
    shift_rx: Option<mpsc::Receiver<ShiftReadReply>>,
    resume_rx: Option<mpsc::Receiver<PositionReadReply>>,
    shift: Option<ShiftReadReply>,
    resume: Option<PositionReadReply>,
    adoption: RestoreReadAdoption,
}
enum RestoreReadAdoption {
    Waiting,
    Consumed,
}
#[derive(Clone, Debug)]
pub(crate) struct CompletedRestoreReads {
    pub(crate) receipt: RestoreReceipt,
    pub(crate) shift: CompletedShiftRead,
    pub(crate) resume: Option<ReadingPosition>,
}

/// Only a captured, current recipe proves either a saved shift or its absence.
/// Expiry never changes an already accepted session; fresh opens start unadjusted.
#[derive(Clone, Debug)]
pub(crate) enum CompletedShiftRead {
    Unrequested,
    Captured(ShiftReadReply),
    Expired,
}
impl RestoreReadDependencies {
    /// A memory-policy change replaces only shift adoption. The resume read was
    /// fixed at open admission, so retain its receiver or already received result.
    pub(crate) fn refresh_shift_dependency(
        &mut self,
        receipt: RestoreReceipt,
        service: Option<&SpreadShiftService>,
    ) {
        debug_assert!(matches!(self.adoption, RestoreReadAdoption::Waiting));
        debug_assert_eq!(self.receipt.open_id, receipt.open_id);
        debug_assert_eq!(self.receipt.context_id, receipt.context_id);
        debug_assert_eq!(self.receipt.scope, receipt.scope);
        debug_assert_eq!(self.receipt.items_generation, receipt.items_generation);
        debug_assert_eq!(self.receipt.resume_cutoff, receipt.resume_cutoff);
        self.shift_rx = service.map(|s| s.read_after(receipt.scope.clone(), receipt.shift_cutoff));
        self.shift = None;
        self.receipt = receipt;
    }

    pub(crate) fn admit(
        receipt: RestoreReceipt,
        service: Option<&SpreadShiftService>,
        resume_writer: Option<&BookResumeWriter>,
        resume_key: Option<&str>,
    ) -> Self {
        Self::from_receivers(
            receipt.clone(),
            service.map(|s| s.read_after(receipt.scope.clone(), receipt.shift_cutoff)),
            resume_writer
                .zip(resume_key)
                .map(|(w, key)| w.read_position_after(key, receipt.resume_cutoff)),
        )
    }
    fn from_receivers(
        receipt: RestoreReceipt,
        shift_rx: Option<mpsc::Receiver<ShiftReadReply>>,
        resume_rx: Option<mpsc::Receiver<PositionReadReply>>,
    ) -> Self {
        Self {
            receipt,
            shift_rx,
            resume_rx,
            shift: None,
            resume: None,
            adoption: RestoreReadAdoption::Waiting,
        }
    }
    /// Returns the completed value once. A cancelled/superseded receipt is rejected
    /// before any state or target can be adopted. No blocking recv occurs on UI.
    pub(crate) fn try_complete(
        &mut self,
        current: &RestoreReceipt,
        maintenance_epoch: u64,
    ) -> Result<Option<CompletedRestoreReads>, String> {
        if matches!(self.adoption, RestoreReadAdoption::Consumed) {
            return Err("book restore already consumed".into());
        }
        if &self.receipt != current {
            return Err("superseded book restore receipt".into());
        }
        if let Some(rx) = &self.shift_rx {
            match rx.try_recv() {
                Ok(read) => {
                    validate_shift_completion(&self.receipt, &read)?;
                    // Preserve provenance through completion, even when the recipe
                    // expires. Neither expiry nor an unrequested read proves absence.
                    self.shift = Some(read);
                    self.shift_rx = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("shift restore worker disconnected".into());
                }
            }
        }
        if let Some(rx) = &self.resume_rx {
            match rx.try_recv() {
                Ok(read) => {
                    if read.cutoff != self.receipt.resume_cutoff
                        || read.completed_through < self.receipt.resume_cutoff
                    {
                        return Err("resume completion proof does not match restore receipt".into());
                    }
                    read.status.clone()?;
                    self.resume = Some(read);
                    self.resume_rx = None;
                }
                Err(mpsc::TryRecvError::Empty) => {}
                Err(mpsc::TryRecvError::Disconnected) => {
                    return Err("resume restore worker disconnected".into());
                }
            }
        }
        if self.shift_rx.is_some() || self.resume_rx.is_some() {
            return Ok(None);
        }
        let shift = if let Some(read) = self.shift.take() {
            validate_shift_completion(&self.receipt, &read)?;
            if read.maintenance_epoch != maintenance_epoch {
                CompletedShiftRead::Expired
            } else {
                CompletedShiftRead::Captured(read)
            }
        } else {
            CompletedShiftRead::Unrequested
        };
        self.adoption = RestoreReadAdoption::Consumed;
        Ok(Some(CompletedRestoreReads {
            receipt: self.receipt.clone(),
            shift,
            resume: self.resume.take().and_then(|r| r.position),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn receipt() -> RestoreReceipt {
        RestoreReceipt {
            open_id: 10,
            context_id: ViewerContextId::for_test(3),
            scope: BookScopeKey::folder("C:/books/a").unwrap(),
            items_generation: 4,
            settings_generation: 5,
            shift_cutoff: 2,
            resume_cutoff: 7,
        }
    }
    fn rules(mode: SpreadMode) -> ResolvedAutomaticRules {
        ResolvedAutomaticRules {
            mode,
            flow: ReadingFlow::Horizontal,
            direction: ReadingDirection::Rtl,
            final_cover: true,
            page_alone: PageAloneSettings {
                after_cover: true,
                last: false,
            },
            singleton_endpoints: SingletonSpreadEndpointSettings {
                first: true,
                last: false,
            },
            landscape_epoch: 9,
        }
    }
    fn request() -> ResolveShiftRequest {
        let receipt = receipt();
        ResolveShiftRequest {
            scope: receipt.scope.clone(),
            receipt,
            cause: ShiftRestoreCause::FreshOpen,
            manifest: ShiftManifest { generation: 4 },
            nav_order: ShiftNavOrder {
                generation: 12,
                indices: Arc::from([0, 2]),
            },
            identity_index: ShiftIdentityIndex::prepare(
                &BookScopeKey::folder("C:/books/a").unwrap(),
                [(
                    PageIdentity::Image {
                        path: "C:/books/a/p.jpg".into(),
                    },
                    2,
                )],
            )
            .unwrap(),
            rules: rules(SpreadMode::RtlCover),
            memory_policy: ShiftMemoryPolicy::Remember,
            layout_eligibility: PairingEditAvailability::Automatic,
        }
    }
    fn shift_read(reference: Option<SpreadShiftPageRef>) -> ShiftReadReply {
        ShiftReadReply {
            cutoff: 2,
            completed_through: 2,
            maintenance_epoch: 6,
            row: reference,
            status: Ok(()),
        }
    }
    fn resolve(request: ResolveShiftRequest, read: ShiftReadReply) -> ResolveShiftReply {
        match resolve_book_layout_source::<()>(
            PreparedLayoutRecipe::Automatic {
                shift_read: Some(read),
                retained_state: SpreadShiftState::None,
                maintenance_epoch: 6,
            },
            request,
        )
        .unwrap()
        {
            BookLayoutSource::Automatic { shift, .. } => shift,
            _ => panic!("expected automatic"),
        }
    }

    #[test]
    fn page_refs_bridge_resume_without_cache_or_raw_index_fallback() {
        let scope = BookScopeKey::folder("C:/books/a").unwrap();
        let image = PageIdentity::Image {
            path: "C:/BOOKS/A/P.JPG".into(),
        };
        let reference = SpreadShiftPageRef::from_identity(&scope, &image).unwrap();
        assert_eq!(reference, SpreadShiftPageRef::RelativePath("P.JPG".into()));
        assert_eq!(
            reference.to_identity(&scope).unwrap(),
            PageIdentity::Image {
                path: "c:/books/a/P.JPG".into()
            }
        );
        assert!(
            SpreadShiftPageRef::from_identity(
                &scope,
                &PageIdentity::Image {
                    path: "C:/books/ab/p.jpg".into()
                }
            )
            .is_err()
        );
        let archive = BookScopeKey::archive("C:/source.rar", &["wrapper/ch.zip".into()]).unwrap();
        let identity = PageIdentity::Archive {
            container: "C:/source.rar".into(),
            entry_name: "wrapper/ch.zip/P.jpg".into(),
        };
        let reference = SpreadShiftPageRef::from_identity(&archive, &identity).unwrap();
        assert_eq!(
            reference,
            SpreadShiftPageRef::ArchiveEntry("wrapper/ch.zip/P.jpg".into())
        );
        assert_eq!(
            reference.to_identity(&archive).unwrap(),
            PageIdentity::Archive {
                container: "c:/source.rar".into(),
                entry_name: "wrapper/ch.zip/P.jpg".into()
            }
        );
        assert!(
            SpreadShiftPageRef::from_identity(
                &archive,
                &PageIdentity::Archive {
                    container: "C:/cache.zip".into(),
                    entry_name: "p.jpg".into()
                }
            )
            .is_err()
        );
        let document = BookScopeKey::document("C:/source.epub").unwrap();
        assert_eq!(
            SpreadShiftPageRef::from_identity(
                &document,
                &PageIdentity::Document {
                    container: "C:/source.epub".into(),
                    page_num: 19
                }
            )
            .unwrap(),
            SpreadShiftPageRef::DocumentPageIdentity(19)
        );
    }

    #[test]
    fn wire_v1_rejects_unsafe_and_unknown_references() {
        for reference in [
            SpreadShiftPageRef::RelativePath("../p.jpg".into()),
            SpreadShiftPageRef::ArchiveEntry("C:/p.jpg".into()),
        ] {
            assert!(reference.validate().is_err());
        }
        let reference = SpreadShiftPageRef::DocumentPageIdentity(0);
        assert_eq!(
            serde_json::from_str::<SpreadShiftPageRef>(&serde_json::to_string(&reference).unwrap())
                .unwrap(),
            reference
        );
        assert!(
            serde_json::from_str::<SpreadShiftPageRef>(r#"{"kind":"future","value":1}"#).is_err()
        );
        assert!(
            serde_json::from_str::<SpreadShiftPageRef>(
                r#"{"kind":"document_page_identity","value":0,"unknown":1}"#
            )
            .is_err()
        );
    }

    #[test]
    fn identity_lookup_normalizes_case_and_slashes_without_rewriting_saved_refs() {
        let folder = BookScopeKey::folder("C:/books/a").unwrap();
        let archive = BookScopeKey::archive("C:/books/a.zip", &["chapter".into()]).unwrap();
        for (scope, identity, saved) in [
            (
                folder,
                PageIdentity::Image {
                    path: "C:\\BOOKS\\A\\Chapter\\PAGE.JPG".into(),
                },
                SpreadShiftPageRef::RelativePath("chapter/page.jpg".into()),
            ),
            (
                archive,
                PageIdentity::Archive {
                    container: "C:\\BOOKS\\A.ZIP".into(),
                    entry_name: "Chapter\\INNER.ZIP\\PAGE.JPG".into(),
                },
                SpreadShiftPageRef::ArchiveEntry("chapter/inner.zip/page.jpg".into()),
            ),
        ] {
            let reference = SpreadShiftPageRef::from_identity(&scope, &identity).unwrap();
            let index = ShiftIdentityIndex::prepare(&scope, [(identity, 7)]).unwrap();
            assert_eq!(index.resolve(&saved), Some(7));
            assert_eq!(index.resolve(&reference), Some(7));
            let wire = serde_json::to_string(&reference).unwrap();
            let decoded = serde_json::from_str::<SpreadShiftPageRef>(&wire).unwrap();
            assert_eq!(decoded, reference);
            assert_ne!(
                decoded, saved,
                "lookup normalization must not rewrite wire spelling"
            );
        }
        let index = ShiftIdentityIndex::prepare(
            &BookScopeKey::folder("C:/books/a").unwrap(),
            [(
                PageIdentity::Image {
                    path: "C:/books/a/page.jpg".into(),
                },
                3,
            )],
        )
        .unwrap();
        assert_eq!(
            index.resolve(&SpreadShiftPageRef::ArchiveEntry("page.jpg".into())),
            None
        );
        assert_eq!(
            index.resolve(&SpreadShiftPageRef::RelativePath("../page.jpg".into())),
            None
        );
    }

    fn resolve_saved_folder_reference(
        scope: BookScopeKey,
        identity: PageIdentity,
        saved: SpreadShiftPageRef,
    ) -> ResolveShiftReply {
        let mut request = request();
        request.scope = scope.clone();
        request.receipt.scope = scope.clone();
        request.identity_index = ShiftIdentityIndex::prepare(&scope, [(identity, 2)]).unwrap();
        resolve(request, shift_read(Some(saved)))
    }

    #[test]
    fn renamed_page_reference_resolves_against_new_manifest_spelling() {
        use super::super::store;
        let temp = tempfile::TempDir::new().unwrap();
        let db_path = temp.path().join("spread_shift.db");
        store::initialize_database_at(&db_path).unwrap();
        let scope = BookScopeKey::folder("C:/books/a").unwrap();
        let mut connection = store::open_existing(&db_path).unwrap().unwrap();
        store::set(
            &mut connection,
            &scope.storage_key(),
            &SpreadShiftPageRef::RelativePath("Chapter\\Old.JPG".into()),
        )
        .unwrap();
        drop(connection);
        assert_eq!(
            store::migrate_page_paths_at(
                temp.path(),
                &[(
                    "C:/books/a/Chapter/Old.JPG".into(),
                    "C:/books/a/Chapter/New.JPG".into()
                )]
            )
            .unwrap(),
            1
        );
        let connection = store::open_existing(&db_path).unwrap().unwrap();
        let saved = store::load_rows(&connection)
            .unwrap()
            .remove(&scope.storage_key())
            .unwrap();
        assert_eq!(
            saved,
            SpreadShiftPageRef::RelativePath("Chapter/New.JPG".into())
        );
        let reply = resolve_saved_folder_reference(
            scope,
            PageIdentity::Image {
                path: "C:\\BOOKS\\A\\chapter\\new.jpg".into(),
            },
            saved.clone(),
        );
        assert_eq!(reply.optional_shift.as_ref().unwrap().anchor_idx, 2);
        assert_eq!(reply.optional_shift.unwrap().anchor_ref, saved);
    }

    #[test]
    fn imported_page_reference_preserves_wire_spelling_and_reresolves() {
        use super::super::store;
        let temp = tempfile::TempDir::new().unwrap();
        let root = temp.path().join("book");
        let data = temp.path().join("data");
        std::fs::create_dir_all(root.join("Pages")).unwrap();
        std::fs::write(root.join("Pages/Old.JPG"), b"page").unwrap();
        let db_path = data.join("spread_shift.db");
        store::initialize_database_at(&db_path).unwrap();
        let scope = BookScopeKey::folder(&root).unwrap();
        let reference = SpreadShiftPageRef::RelativePath("Pages\\Old.JPG".into());
        let mut connection = store::open_existing(&db_path).unwrap().unwrap();
        store::set(&mut connection, &scope.storage_key(), &reference).unwrap();
        drop(connection);
        let cancel = std::sync::atomic::AtomicBool::new(false);
        crate::metadata_transfer::export_at(&data, &root, false, &cancel, |_| {}).unwrap();
        let mut connection = store::open_existing(&db_path).unwrap().unwrap();
        store::clear(&mut connection, &scope.storage_key()).unwrap();
        drop(connection);
        let imported = crate::metadata_transfer::import_at(&data, &root, &cancel, |_| {}).unwrap();
        assert_eq!(imported.failed_entries, 0);
        assert_eq!(imported.committed.spread_shift_additions, 1);
        let connection = store::open_existing(&db_path).unwrap().unwrap();
        let saved = store::load_rows(&connection)
            .unwrap()
            .remove(&scope.storage_key())
            .unwrap();
        assert_eq!(saved, reference);
        let reply = resolve_saved_folder_reference(
            scope,
            PageIdentity::Image {
                path: root.join("pages/old.jpg").to_string_lossy().into_owned(),
            },
            saved.clone(),
        );
        assert_eq!(reply.optional_shift.as_ref().unwrap().anchor_idx, 2);
        assert_eq!(reply.optional_shift.unwrap().anchor_ref, saved);
    }

    #[test]
    fn automatic_result_and_preparing_capture_preserve_both_page_alone_choices() {
        for after_cover in [false, true] {
            for last in [false, true] {
                let expected = PageAloneSettings { after_cover, last };
                let mut request = request();
                request.rules.page_alone = expected;
                let BookLayoutSource::Automatic { rules, shift } =
                    resolve_book_layout_source::<()>(
                        PreparedLayoutRecipe::Automatic {
                            shift_read: Some(shift_read(None)),
                            retained_state: SpreadShiftState::None,
                            maintenance_epoch: 6,
                        },
                        request,
                    )
                    .unwrap()
                else {
                    panic!("expected Automatic");
                };
                assert_eq!(rules.page_alone, expected);
                let live = EffectiveLayoutSnapshot {
                    scope: shift.receipt.scope,
                    context_id: ViewerContextId::for_test(3),
                    items_generation: 4,
                    nav_generation: 12,
                    settings_generation: 5,
                    rules,
                    live_shift: shift.state,
                    units: Arc::from([]),
                    layout_token: 77,
                };
                let captured =
                    capture_effective_automatic_layout(AutomaticContextToken { effective: &live });
                assert_eq!(captured.rules.page_alone, expected);
            }
        }
    }

    #[test]
    fn missing_filtered_and_returned_anchor_use_identity_only() {
        let reference = SpreadShiftPageRef::RelativePath("p.jpg".into());
        let resolved = resolve(request(), shift_read(Some(reference.clone())));
        assert_eq!(resolved.optional_shift.as_ref().unwrap().anchor_idx, 2);
        let mut filtered = request();
        filtered.nav_order.indices = Arc::from([0]);
        let filtered = resolve(filtered, shift_read(Some(reference.clone())));
        assert!(filtered.optional_shift.is_none());
        assert!(matches!(
            filtered.state,
            SpreadShiftState::Adjusted {
                resolution: ShiftResolution::NotInView,
                ..
            }
        ));
        let missing = resolve(
            request(),
            shift_read(Some(SpreadShiftPageRef::RelativePath("gone.jpg".into()))),
        );
        assert!(missing.optional_shift.is_none());
        let returned = resolve(request(), shift_read(Some(reference)));
        assert_eq!(returned.optional_shift.as_ref().unwrap().anchor_idx, 2);
    }

    #[test]
    fn off_fresh_open_keeps_dormant_row_and_retained_keeps_live_session() {
        let mut request = request();
        request.memory_policy = ShiftMemoryPolicy::SessionOnly;
        let live = SpreadShiftState::Adjusted {
            anchor_ref: SpreadShiftPageRef::RelativePath("p.jpg".into()),
            resolution: ShiftResolution::Resolved(99),
            persistence: ShiftPersistence::SessionOnly,
        };
        let recipe = || PreparedLayoutRecipe::<()>::Automatic {
            shift_read: None,
            retained_state: live.clone(),
            maintenance_epoch: 6,
        };
        let BookLayoutSource::Automatic { shift, .. } =
            resolve_book_layout_source(recipe(), request.clone()).unwrap()
        else {
            panic!()
        };
        assert_eq!(shift.state, SpreadShiftState::None);
        request.cause = ShiftRestoreCause::RetainedReadoption;
        let BookLayoutSource::Automatic { shift, .. } =
            resolve_book_layout_source(recipe(), request).unwrap()
        else {
            panic!()
        };
        assert_eq!(shift.optional_shift.unwrap().anchor_idx, 2);
    }

    #[test]
    fn composition_and_extracted_recipe_do_not_resolve_dormant_shift() {
        let mut request = request();
        request.layout_eligibility = PairingEditAvailability::Composed;
        assert!(matches!(
            resolve_book_layout_source(PreparedLayoutRecipe::Composed(42), request.clone())
                .unwrap(),
            BookLayoutSource::Composed(42)
        ));
        assert!(matches!(
            resolve_book_layout_source::<()>(
                PreparedLayoutRecipe::Unreadable("broken".into()),
                request.clone()
            )
            .unwrap(),
            BookLayoutSource::Unreadable(_)
        ));
        assert!(matches!(
            resolve_book_layout_source::<()>(PreparedLayoutRecipe::ExtractedResult, request)
                .unwrap(),
            BookLayoutSource::ExtractedResult
        ));
    }

    #[test]
    fn all_pairing_producers_reject_before_writes_but_single_split_navigate() {
        for availability in [
            PairingEditAvailability::Composed,
            PairingEditAvailability::Unreadable,
            PairingEditAvailability::ExtractedResult,
        ] {
            for action in [
                PairingAction::Nudge,
                PairingAction::ClearShift,
                PairingAction::SetMemoryPolicy,
            ] {
                assert!(matches!(
                    admit_pairing_action(
                        PairingContextToken {
                            rules: &rules(SpreadMode::Ltr),
                            availability
                        },
                        action
                    ),
                    PairingAdmission::Rejected(_)
                ));
            }
            for mode in [
                SpreadMode::Single,
                SpreadMode::SplitLtr,
                SpreadMode::SplitRtl,
            ] {
                assert_eq!(
                    admit_pairing_action(
                        PairingContextToken {
                            rules: &rules(mode),
                            availability
                        },
                        PairingAction::Nudge
                    ),
                    PairingAdmission::Accepted(PairingOperation::NavigationFallback)
                );
            }
        }
    }

    #[test]
    fn preparing_capture_preserves_live_saving_off_or_saved_units_without_store_read() {
        for persistence in [
            ShiftPersistence::SessionOnly,
            ShiftPersistence::Saving(10),
            ShiftPersistence::Saved(8),
            ShiftPersistence::SaveFailed,
        ] {
            let live = EffectiveLayoutSnapshot {
                scope: receipt().scope,
                context_id: ViewerContextId::for_test(3),
                items_generation: 4,
                nav_generation: 12,
                settings_generation: 5,
                rules: rules(SpreadMode::RtlCover),
                live_shift: SpreadShiftState::Adjusted {
                    anchor_ref: SpreadShiftPageRef::RelativePath("p.jpg".into()),
                    resolution: ShiftResolution::Resolved(2),
                    persistence,
                },
                units: Arc::from([]),
                layout_token: 77,
            };
            let captured =
                capture_effective_automatic_layout(AutomaticContextToken { effective: &live });
            assert_eq!(captured.live_shift, live.live_shift);
            assert_eq!(captured.layout_token, 77);
            assert!(Arc::ptr_eq(&captured.units, &live.units));
            assert_eq!(captured.rules, live.rules);
        }
    }

    #[test]
    fn memory_refresh_preserves_received_resume_while_replacing_shift_dependency() {
        let (shift_tx, shift_rx) = mpsc::channel();
        let (resume_tx, resume_rx) = mpsc::channel();
        let mut original = receipt();
        let mut pending = RestoreReadDependencies::from_receivers(
            original.clone(),
            Some(shift_rx),
            Some(resume_rx),
        );
        let position = ReadingPosition {
            page: 2,
            identity: None,
        };
        resume_tx
            .send(PositionReadReply {
                cutoff: original.resume_cutoff,
                completed_through: original.resume_cutoff,
                position: Some(position.clone()),
                status: Ok(()),
            })
            .unwrap();
        assert!(pending.try_complete(&original, 6).unwrap().is_none());
        assert!(pending.resume_rx.is_none());
        assert_eq!(
            pending.resume.as_ref().unwrap().position,
            Some(position.clone())
        );

        // OFF removes the shift wait, but the completed initial resume stays owned.
        original.settings_generation += 1;
        original.shift_cutoff += 1;
        pending.refresh_shift_dependency(original.clone(), None);
        assert!(shift_tx.send(shift_read(None)).is_err());
        let completed = pending.try_complete(&original, 6).unwrap().unwrap();
        assert_eq!(completed.resume, Some(position));
        assert_eq!(completed.receipt.resume_cutoff, 7);
        assert!(matches!(completed.shift, CompletedShiftRead::Unrequested));
        assert!(pending.try_complete(&original, 6).is_err());
    }

    #[test]
    fn restore_waits_for_both_writers_in_either_completion_order_and_consumes_once() {
        for shift_first in [false, true] {
            let (st, sr) = mpsc::channel();
            let (rt, rr) = mpsc::channel();
            let receipt = receipt();
            let mut pending =
                RestoreReadDependencies::from_receivers(receipt.clone(), Some(sr), Some(rr));
            let position = ReadingPosition {
                page: 2,
                identity: Some(PageIdentity::Image {
                    path: "c:/books/a/p.jpg".into(),
                }),
            };
            let resume = PositionReadReply {
                cutoff: 7,
                completed_through: 7,
                position: Some(position.clone()),
                status: Ok(()),
            };
            if shift_first {
                st.send(shift_read(None)).unwrap();
            } else {
                rt.send(resume.clone()).unwrap();
            }
            assert!(pending.try_complete(&receipt, 6).unwrap().is_none());
            if shift_first {
                rt.send(resume).unwrap();
            } else {
                st.send(shift_read(None)).unwrap();
            }
            let completed = pending.try_complete(&receipt, 6).unwrap().unwrap();
            assert_eq!(completed.resume, Some(position));
            assert_eq!(completed.receipt, receipt);
            assert!(pending.try_complete(&receipt, 6).is_err());
        }
    }

    #[test]
    fn accepted_set_and_record_reopen_waits_for_real_writer_fences_in_both_orders() {
        for shift_first in [false, true] {
            let data = tempfile::tempdir().unwrap();
            let (service, owner) =
                SpreadShiftService::spawn_at(data.path().to_path_buf(), None).unwrap();
            let resume = BookResumeWriter::spawn_at(data.path().join("book_resume.db")).unwrap();
            let release_shift = service.pause_for_test();
            let release_resume = resume.pause_for_test();
            let root = data.path().join("book");
            let scope = BookScopeKey::folder(&root).unwrap();
            let reference = SpreadShiftPageRef::RelativePath("002.jpg".into());
            let identity = reference.to_identity(&scope).unwrap();
            let operation = super::super::ShiftOperation {
                context_id: ViewerContextId::for_test(1),
                items_generation: 4,
                operation_sequence: 5,
            };
            service
                .set_shift(scope.clone(), reference.clone(), operation)
                .unwrap();
            assert!(resume.record_identity(&root, 2, None, Some(identity.clone())));
            let receipt = RestoreReceipt::capture(
                10,
                ViewerContextId::for_test(2),
                scope.clone(),
                6,
                7,
                &service,
                Some(&resume),
            );
            assert_eq!((receipt.shift_cutoff, receipt.resume_cutoff), (1, 1));
            let resume_key = crate::path_key::normalize(&root);
            let mut pending = RestoreReadDependencies::admit(
                receipt.clone(),
                Some(&service),
                Some(&resume),
                Some(&resume_key),
            );
            assert!(pending.try_complete(&receipt, 0).unwrap().is_none());
            if shift_first {
                release_shift.send(()).unwrap();
                service
                    .read_after(scope.clone(), 1)
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
            } else {
                release_resume.send(()).unwrap();
                resume
                    .read_position_after(&resume_key, 1)
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
            }
            assert!(pending.try_complete(&receipt, 0).unwrap().is_none());
            if shift_first {
                release_resume.send(()).unwrap();
                resume
                    .read_position_after(&resume_key, 1)
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
            } else {
                release_shift.send(()).unwrap();
                service
                    .read_after(scope, 1)
                    .recv_timeout(std::time::Duration::from_secs(10))
                    .unwrap();
            }
            let completed = pending.try_complete(&receipt, 0).unwrap().unwrap();
            let CompletedShiftRead::Captured(read) = completed.shift else {
                panic!("expected the current captured recipe");
            };
            assert_eq!(read.row, Some(reference));
            assert_eq!(
                completed.resume,
                Some(ReadingPosition {
                    page: 2,
                    identity: Some(identity)
                })
            );
            assert!(
                owner
                    .shutdown_and_join()
                    .iter()
                    .all(|result| result.status.is_ok())
            );
            drop(resume);
            // The original sender's retired context is irrelevant to commit/drain ownership.
            let db =
                crate::book_resume_db::BookResumeDb::open_at(&data.path().join("book_resume.db"))
                    .unwrap();
            assert_eq!(db.get_position(&root).unwrap().page, 2);
        }
    }

    #[test]
    fn stale_receipt_and_failed_reads_never_certify_absence() {
        let receipt = receipt();
        for mismatch in 0..5 {
            let (tx, rx) = mpsc::channel();
            tx.send(shift_read(None)).unwrap();
            let mut pending =
                RestoreReadDependencies::from_receivers(receipt.clone(), Some(rx), None);
            let mut current = receipt.clone();
            match mismatch {
                0 => current.open_id += 1,
                1 => current.context_id = ViewerContextId::for_test(8),
                2 => current.scope = BookScopeKey::folder("C:/other").unwrap(),
                3 => current.items_generation += 1,
                _ => current.settings_generation += 1,
            };
            assert!(pending.try_complete(&current, 6).is_err());
        }
        let reply = resolve(request(), shift_read(None));
        assert!(!reply.matches_adoption(&receipt, 13, 6));
        assert!(!reply.matches_adoption(&receipt, 12, 7));
        let (tx, rx) = mpsc::channel();
        let mut read = shift_read(None);
        read.status = Err("unreadable original".into());
        tx.send(read).unwrap();
        assert!(
            RestoreReadDependencies::from_receivers(receipt.clone(), Some(rx), None)
                .try_complete(&receipt, 6)
                .is_err()
        );
    }

    #[test]
    fn maintenance_rejects_only_shift_recipe_and_preserves_resume_in_both_read_orders() {
        // Cover a reply first polled after maintenance, a cached shift awaiting
        // resume, and a cached resume awaiting shift. None may trigger a reread.
        for first in [0, 1, 2] {
            let receipt = receipt();
            let (st, sr) = mpsc::channel();
            let (rt, rr) = mpsc::channel();
            let mut pending =
                RestoreReadDependencies::from_receivers(receipt.clone(), Some(sr), Some(rr));
            let position = ReadingPosition {
                page: 2,
                identity: None,
            };
            let resume = PositionReadReply {
                cutoff: receipt.resume_cutoff,
                completed_through: receipt.resume_cutoff,
                position: Some(position.clone()),
                status: Ok(()),
            };
            if first == 1 {
                st.send(shift_read(Some(SpreadShiftPageRef::RelativePath(
                    "p.jpg".into(),
                ))))
                .unwrap();
            } else if first == 2 {
                rt.send(resume.clone()).unwrap();
            }
            assert!(pending.try_complete(&receipt, 6).unwrap().is_none());
            // Invalidate an already captured recipe while resume is still pending.
            assert!(pending.try_complete(&receipt, 7).unwrap().is_none());
            if first != 1 {
                st.send(shift_read(Some(SpreadShiftPageRef::RelativePath(
                    "p.jpg".into(),
                ))))
                .unwrap();
            }
            if first != 2 {
                rt.send(resume).unwrap();
            }
            let completed = pending.try_complete(&receipt, 7).unwrap().unwrap();
            assert!(
                matches!(completed.shift, CompletedShiftRead::Expired),
                "the old recipe must not be adopted"
            );
            assert_eq!(completed.resume, Some(position));
            assert_eq!(completed.receipt, receipt);
            assert!(pending.try_complete(&receipt, 7).is_err());
        }
    }
}

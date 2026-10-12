//! Permission belongs to a visible presentation episode, never to a decoded image.
use std::{path::PathBuf, sync::Arc};

use super::{CompositionPageRef, CompositionSnapshot, CompositionToken, PageDomain};
use crate::{book_resume_db::PageIdentity, book_scope::BookScopeKey, zip_tree::ZipTree};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PresentationRole {
    Navigation,
    Supplement,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PresentationSlice {
    Full,
    Left,
    Right,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) enum PresentationSlot {
    Center,
    Left,
    Right,
}
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub(crate) struct PresentationOccurrence {
    pub(crate) item_index: usize,
    pub(crate) navigation_anchor: usize,
    pub(crate) role: PresentationRole,
    pub(crate) slice: PresentationSlice,
    pub(crate) slot: PresentationSlot,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum PresentationDecision {
    Checking,
    Allowed(Vec<bool>),
    Hidden,
}

#[derive(Clone, Debug)]
pub(crate) struct CandidatePage {
    pub(crate) identity: PageIdentity,
    pub(crate) physical_archive: Option<PathBuf>,
    /// Only an unfiltered tree can prove the effective owning level.
    pub(crate) raw_tree: Option<Arc<ZipTree>>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CandidateOccurrence {
    Presented(PresentationOccurrence),
    IdentityOnly,
}
impl CandidateOccurrence {
    pub(crate) fn presented(&self) -> Option<PresentationOccurrence> {
        match self {
            Self::Presented(occurrence) => Some(*occurrence),
            Self::IdentityOnly => None,
        }
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CandidateCorrelation {
    pub(crate) context_id: u64,
    pub(crate) request_id: u64,
    pub(crate) items_generation: u64,
    pub(crate) occurrence: CandidateOccurrence,
    pub(crate) identity: PageIdentity,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScopePermissionToken {
    pub(crate) scope: BookScopeKey,
    pub(crate) token: CompositionToken,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CandidatePermission {
    Checking,
    Allowed(Option<ScopePermissionToken>),
    Hidden(ScopePermissionToken),
}
#[derive(Debug)]
pub(crate) struct CandidatePermissionReply {
    pub(crate) correlation: CandidateCorrelation,
    pub(crate) status: Result<CandidatePermission, CandidatePermissionError>,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CandidatePermissionError {
    UnsupportedSource(String),
    Unavailable(String),
}
impl From<String> for CandidatePermissionError {
    fn from(error: String) -> Self {
        Self::UnsupportedSource(error)
    }
}
impl From<&str> for CandidatePermissionError {
    fn from(error: &str) -> Self {
        Self::UnsupportedSource(error.into())
    }
}
impl std::fmt::Display for CandidatePermissionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnsupportedSource(error) | Self::Unavailable(error) => f.write_str(error),
        }
    }
}

#[derive(Clone, Debug)]
pub(crate) struct CompositionTarget {
    pub(crate) scope: BookScopeKey,
    pub(crate) page: CompositionPageRef,
}
#[derive(Clone, Debug, Default)]
pub(crate) struct CompositionWriteAdmission {
    pub(crate) targets: Vec<CompositionTarget>,
    pub(crate) binding_scope: Option<BookScopeKey>,
    /// Worker receipts from the exact source that cannot support composition.
    pub(crate) unsupported_scopes: Vec<ScopePermissionToken>,
    pub(crate) unsupported_pages: Vec<PageIdentity>,
}
impl CompositionWriteAdmission {
    pub(crate) fn validate(&self, snapshot: &CompositionSnapshot) -> Result<(), String> {
        // An outside-scope write has no composition resource to protect.
        if self.targets.is_empty()
            && self.binding_scope.is_none()
            && self.unsupported_scopes.is_empty()
            && self.unsupported_pages.is_empty()
        {
            return Ok(());
        }
        snapshot.status.clone()?;
        for proof in &self.unsupported_scopes {
            if snapshot.token_for(&proof.scope) != proof.token {
                return Err("本の構成が更新されました。もう一度操作してください".into());
            }
        }
        if self.binding_scope.as_ref().is_some_and(|scope| {
            snapshot.rows.contains_key(&scope.storage_key())
                && !self
                    .unsupported_scopes
                    .iter()
                    .any(|proof| &proof.scope == scope)
        }) {
            return Err("本の構成で設定されています".into());
        }
        for target in &self.targets {
            if snapshot
                .rows
                .get(&target.scope.storage_key())
                .is_some_and(|row| {
                    row.composition
                        .hidden_pages
                        .iter()
                        .any(|page| page.matching_key() == target.page.matching_key())
                })
            {
                return Err("本の構成で非表示に設定されています".into());
            }
        }
        Ok(())
    }
}

/// Only the owning raw level is inspected; child books remain independent.
pub(crate) fn validate_archive_scope_in_tree(
    scope: &BookScopeKey,
    tree: &ZipTree,
) -> Result<(), CandidatePermissionError> {
    let raw = tree.raw_source();
    let mut node = &raw.root;
    for segment in scope
        .level_prefix
        .split('/')
        .filter(|value| !value.is_empty())
    {
        let matches = node
            .dirs
            .iter()
            .filter(|(name, _)| name.to_lowercase() == segment.to_lowercase())
            .collect::<Vec<_>>();
        if matches.is_empty() {
            return Err(CandidatePermissionError::Unavailable(
                "archive level is unavailable".into(),
            ));
        }
        if matches.len() != 1 {
            return Err("archive level is ambiguous".into());
        }
        node = matches[0].1;
    }
    let domain = PageDomain::ArchiveLevel {
        prefix: scope.level_prefix.clone(),
    };
    let mut identities = std::collections::HashSet::new();
    for entry in &node.images {
        let page = CompositionPageRef::ArchiveEntry {
            entry_name: entry.entry_name.clone(),
        };
        domain
            .validate_page(&page)
            .map_err(|error| CandidatePermissionError::UnsupportedSource(error.to_string()))?;
        if !identities.insert(page.matching_key()) {
            return Err("archive pages have ambiguous normalized identities".into());
        }
    }
    Ok(())
}

pub(crate) fn validate_archive_scope_source(
    scope: &BookScopeKey,
    physical: &std::path::Path,
) -> Result<(), CandidatePermissionError> {
    let entries = if physical.is_dir()
        && crate::path_key::normalize_keep_drive(physical)
            != crate::path_key::normalize_keep_drive(&scope.logical_root)
    {
        crate::zip_loader::enumerate_converted_image_entries_detailed_readonly_with_cancel(
            physical, None,
        )
    } else {
        crate::zip_loader::enumerate_image_entries_detailed_readonly_with_cancel(physical, None)
    }
    .map_err(|error| CandidatePermissionError::Unavailable(error.to_string()))?;
    validate_archive_scope_in_tree(scope, &ZipTree::build(physical.to_owned(), entries.entries))
}

impl CandidatePermission {
    pub(crate) fn is_current(&self, snapshot: &CompositionSnapshot) -> bool {
        match self {
            Self::Checking => false,
            Self::Allowed(None) => true,
            Self::Allowed(Some(proof)) | Self::Hidden(proof) => {
                snapshot.status.is_ok() && snapshot.token_for(&proof.scope) == proof.token
            }
        }
    }
}

/// Worker-only logical scope resolution. An archive entry's textual parent alone
/// cannot prove a collapsed/nested book boundary; use the safe raw tree.
pub(crate) fn resolve_candidate_scope(
    candidate: &CandidatePage,
) -> Result<(BookScopeKey, PageDomain, CompositionPageRef), CandidatePermissionError> {
    match &candidate.identity {
        PageIdentity::Document {
            container,
            page_num,
        } => {
            let page = CompositionPageRef::PdfPage {
                page_num: u32::try_from(*page_num).map_err(|_| "document page exceeds limit")?,
            };
            Ok((
                BookScopeKey::document(container)?,
                PageDomain::Document,
                page,
            ))
        }
        PageIdentity::Image { path } => {
            let path = std::path::Path::new(path);
            let parent = path.parent().ok_or("image has no parent folder")?;
            let name = path
                .file_name()
                .and_then(|value| value.to_str())
                .ok_or("invalid image filename")?;
            let page = CompositionPageRef::RelativePath { path: name.into() };
            PageDomain::ImageFolder
                .validate_page(&page)
                .map_err(|error| error.to_string())?;
            Ok((BookScopeKey::folder(parent)?, PageDomain::ImageFolder, page))
        }
        PageIdentity::Archive {
            container,
            entry_name,
        } => {
            crate::book_scope::validate_relative_reference(entry_name)?;
            let physical = candidate
                .physical_archive
                .as_deref()
                .unwrap_or_else(|| std::path::Path::new(container));
            let owned;
            let tree = if let Some(tree) = candidate.raw_tree.as_deref().filter(|tree| {
                crate::path_key::normalize_keep_drive(&tree.zip_path)
                    == crate::path_key::normalize_keep_drive(physical)
            }) {
                tree.raw_source()
            } else {
                let entries = if physical.is_dir() && crate::path_key::normalize_keep_drive(physical) != crate::path_key::normalize_keep_drive(std::path::Path::new(container)) {
                    crate::zip_loader::enumerate_converted_image_entries_detailed_readonly_with_cancel(physical, None)
                } else { crate::zip_loader::enumerate_image_entries_detailed_readonly_with_cancel(physical, None) }
                    .map_err(|error| CandidatePermissionError::Unavailable(error.to_string()))?;
                owned = ZipTree::build(physical.to_owned(), entries.entries);
                &owned
            };
            let key = entry_name.replace('\\', "/").to_lowercase();
            fn find(
                node: &crate::zip_tree::ZipTreeNode,
                prefix: &mut Vec<String>,
                key: &str,
                found: &mut Vec<Vec<String>>,
            ) {
                for _ in node
                    .images
                    .iter()
                    .filter(|entry| entry.entry_name.replace('\\', "/").to_lowercase() == key)
                {
                    found.push(prefix.clone());
                }
                for (name, child) in &node.dirs {
                    prefix.push(name.clone());
                    find(child, prefix, key, found);
                    prefix.pop();
                }
            }
            let mut found = Vec::new();
            find(&tree.root, &mut Vec::new(), &key, &mut found);
            if found.is_empty() {
                return Err(CandidatePermissionError::Unavailable(
                    "archive page owning level is unavailable".into(),
                ));
            }
            if found.len() != 1 {
                return Err("archive page has no unambiguous owning level".into());
            }
            let prefix = found.pop().unwrap();
            let scope = BookScopeKey::archive(container, &prefix)?;
            validate_archive_scope_in_tree(&scope, tree)?;
            let domain = PageDomain::ArchiveLevel {
                prefix: scope.level_prefix.clone(),
            };
            let page = CompositionPageRef::ArchiveEntry {
                entry_name: entry_name.clone(),
            };
            domain
                .validate_page(&page)
                .map_err(|error| error.to_string())?;
            Ok((scope, domain, page))
        }
    }
}

pub(crate) fn candidate_permission(
    snapshot: &CompositionSnapshot,
    candidate: &CandidatePage,
) -> Result<CandidatePermission, CandidatePermissionError> {
    snapshot
        .status
        .clone()
        .map_err(CandidatePermissionError::Unavailable)?;
    let (scope, domain, page) = resolve_candidate_scope(candidate)?;
    let proof = ScopePermissionToken {
        token: snapshot.token_for(&scope),
        scope,
    };
    if let Some(row) = snapshot.rows.get(&proof.scope.storage_key()) {
        if row.domain != domain {
            return Err(CandidatePermissionError::Unavailable(
                "saved composition kind does not match candidate".into(),
            ));
        }
        if row
            .composition
            .hidden_pages
            .iter()
            .any(|hidden| hidden.matching_key() == page.matching_key())
        {
            return Ok(CandidatePermission::Hidden(proof));
        }
    }
    Ok(CandidatePermission::Allowed(Some(proof)))
}

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
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum CompositionBindingWriteKind {
    #[default]
    Binding,
    AutomaticRule,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct CompositionWriteAdmission {
    pub(crate) targets: Vec<CompositionTarget>,
    pub(crate) binding_scope: Option<BookScopeKey>,
    pub(crate) binding_kind: CompositionBindingWriteKind,
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
        if self.binding_kind == CompositionBindingWriteKind::Binding
            && self.binding_scope.as_ref().is_some_and(|scope| {
                snapshot.rows.contains_key(&scope.storage_key())
                    && !self
                        .unsupported_scopes
                        .iter()
                        .any(|proof| &proof.scope == scope)
            })
        {
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
            let index = prepare_archive_scope_index(candidate, container, physical)?;
            let (scope, domain) = index.scope_for(entry_name)?;
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

/// Identity-to-owning-scope index for one captured archive source. Every raw
/// node/page is inspected once; direct-level validation is shared by its pages.
struct ArchiveScopeIndex {
    entries: std::collections::HashMap<String, Result<usize, CandidatePermissionError>>,
    scopes: Vec<Result<(BookScopeKey, PageDomain), CandidatePermissionError>>,
    #[cfg(test)]
    visited_pages: usize,
}

impl ArchiveScopeIndex {
    fn prepare(container: &str, tree: &ZipTree) -> Self {
        fn walk(
            index: &mut ArchiveScopeIndex,
            container: &str,
            node: &crate::zip_tree::ZipTreeNode,
            prefix: &mut Vec<String>,
            ambiguous: bool,
        ) {
            let result = (|| {
                if ambiguous {
                    return Err(CandidatePermissionError::UnsupportedSource(
                        "archive level is ambiguous".into(),
                    ));
                }
                let scope = BookScopeKey::archive(container, prefix)?;
                let domain = PageDomain::ArchiveLevel {
                    prefix: scope.level_prefix.clone(),
                };
                let mut identities = std::collections::HashSet::with_capacity(node.images.len());
                for entry in &node.images {
                    let page = CompositionPageRef::ArchiveEntry {
                        entry_name: entry.entry_name.clone(),
                    };
                    domain
                        .validate_page(&page)
                        .map_err(|error| error.to_string())?;
                    if !identities.insert(page.matching_key()) {
                        return Err(CandidatePermissionError::UnsupportedSource(
                            "archive pages have ambiguous normalized identities".into(),
                        ));
                    }
                }
                Ok((scope, domain))
            })();
            let scope_index = index.scopes.len();
            index.scopes.push(result);
            for entry in &node.images {
                #[cfg(test)]
                {
                    index.visited_pages += 1;
                }
                let key = crate::book_bookmarks::normalize_page_path(&entry.entry_name);
                index
                    .entries
                    .entry(key)
                    .and_modify(|entry| {
                        *entry = Err(CandidatePermissionError::UnsupportedSource(
                            "archive page has no unambiguous owning level".into(),
                        ))
                    })
                    .or_insert(Ok(scope_index));
            }
            let mut child_counts = std::collections::HashMap::<String, usize>::new();
            for name in node.dirs.keys() {
                *child_counts.entry(name.to_lowercase()).or_default() += 1;
            }
            for (name, child) in &node.dirs {
                prefix.push(name.clone());
                walk(
                    index,
                    container,
                    child,
                    prefix,
                    ambiguous || child_counts[&name.to_lowercase()] > 1,
                );
                prefix.pop();
            }
        }
        let mut index = Self {
            entries: Default::default(),
            scopes: Vec::new(),
            #[cfg(test)]
            visited_pages: 0,
        };
        walk(
            &mut index,
            container,
            &tree.raw_source().root,
            &mut Vec::new(),
            false,
        );
        index
    }
    fn scope_for(
        &self,
        entry: &str,
    ) -> Result<(BookScopeKey, PageDomain), CandidatePermissionError> {
        let key = crate::book_bookmarks::normalize_page_path(entry);
        let scope_index = self
            .entries
            .get(&key)
            .ok_or_else(|| {
                CandidatePermissionError::Unavailable(
                    "archive page owning level is unavailable".into(),
                )
            })?
            .clone()?;
        self.scopes[scope_index].clone()
    }
}

fn prepare_archive_scope_index(
    candidate: &CandidatePage,
    container: &str,
    physical: &std::path::Path,
) -> Result<ArchiveScopeIndex, CandidatePermissionError> {
    if let Some(tree) = candidate.raw_tree.as_deref().filter(|tree| {
        crate::path_key::normalize_keep_drive(&tree.zip_path)
            == crate::path_key::normalize_keep_drive(physical)
    }) {
        return Ok(ArchiveScopeIndex::prepare(container, tree));
    }
    let entries = if physical.is_dir()
        && crate::path_key::normalize_keep_drive(physical)
            != crate::path_key::normalize_keep_drive(std::path::Path::new(container))
    {
        crate::zip_loader::enumerate_converted_image_entries_detailed_readonly_with_cancel(
            physical, None,
        )
    } else {
        crate::zip_loader::enumerate_image_entries_detailed_readonly_with_cancel(physical, None)
    }
    .map_err(|error| CandidatePermissionError::Unavailable(error.to_string()))?;
    Ok(ArchiveScopeIndex::prepare(
        container,
        &ZipTree::build(physical.to_owned(), entries.entries),
    ))
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

/// Output admission is checked in its worker when a real source is accepted.
/// Capturing a source or displaying a cached thumbnail is not an output permit.
#[derive(Clone)]
pub struct CompositionOutputCheck {
    service: super::BookCompositionService,
    candidate: CandidatePage,
}

pub(crate) const COMPOSITION_OUTPUT_HIDDEN_NOTICE: &str =
    "本の構成で非表示に設定されているため、出力から除外しました";

impl CompositionOutputCheck {
    pub(crate) fn new(service: super::BookCompositionService, candidate: CandidatePage) -> Self {
        Self { service, candidate }
    }

    /// Single-candidate adapter; multi-page workers share a run resolver.
    pub(crate) fn permits(&self) -> Result<bool, String> {
        self.permits_with(&mut CompositionOutputResolver::default())
    }

    pub(crate) fn permits_with(
        &self,
        resolver: &mut CompositionOutputResolver,
    ) -> Result<bool, String> {
        let snapshot = self.service.snapshot();
        snapshot.status.clone()?;
        match resolver.permission(&snapshot, &self.candidate, || self.service.snapshot()) {
            Ok(value) => Ok(value),
            Err(CandidatePermissionError::UnsupportedSource(reason)) => {
                crate::logger::log(format!(
                    "composition output: unsupported source, preserving ordinary output: {reason}"
                ));
                Ok(true)
            }
            Err(CandidatePermissionError::Unavailable(reason)) => Err(reason),
        }
    }
}

/// Static source indexes and revision-scoped hidden sets belong to one output
/// run. No permission result or source index is shared across worker lifetimes.
#[derive(Default)]
pub(crate) struct CompositionOutputResolver {
    archives: std::collections::HashMap<(String, String, usize), PreparedOutputArchive>,
    hidden: std::collections::HashMap<
        String,
        (
            CompositionToken,
            PageDomain,
            std::collections::HashSet<CompositionPageRef>,
        ),
    >,
    #[cfg(test)]
    pub(crate) prepared_sources: usize,
    #[cfg(test)]
    pub(crate) visited_pages: usize,
}

struct PreparedOutputArchive {
    // Keep the captured Arc alive alongside its pointer key; pointer reuse must
    // never alias another context's raw source within a run.
    _raw_tree: Option<Arc<ZipTree>>,
    index: Result<ArchiveScopeIndex, CandidatePermissionError>,
}

impl CompositionOutputResolver {
    pub(crate) fn permission(
        &mut self,
        snapshot: &CompositionSnapshot,
        candidate: &CandidatePage,
        fresh_snapshot: impl FnOnce() -> Arc<CompositionSnapshot>,
    ) -> Result<bool, CandidatePermissionError> {
        if snapshot.rows.is_empty() {
            return Ok(true);
        }
        let (scope, domain, page) = if let PageIdentity::Archive {
            container,
            entry_name,
        } = &candidate.identity
        {
            let root = crate::path_key::normalize_keep_drive(std::path::Path::new(container));
            let prefix = format!("{root}/");
            // BTreeMap's range gives O(log scopes) root membership, without
            // scanning all stored books for every selected page.
            let has_rows = snapshot.rows.contains_key(&root)
                || snapshot
                    .rows
                    .range(prefix.clone()..)
                    .next()
                    .is_some_and(|(key, _)| key.starts_with(&prefix));
            if !has_rows {
                return Ok(true);
            }
            crate::book_scope::validate_relative_reference(entry_name)?;
            let physical = candidate
                .physical_archive
                .as_deref()
                .unwrap_or_else(|| std::path::Path::new(container));
            let physical_key = crate::path_key::normalize_keep_drive(physical);
            let tree_ptr = candidate
                .raw_tree
                .as_ref()
                .filter(|tree| {
                    crate::path_key::normalize_keep_drive(&tree.zip_path) == physical_key
                })
                .map_or(0, |tree| Arc::as_ptr(tree) as usize);
            let key = (root, physical_key, tree_ptr);
            if !self.archives.contains_key(&key) {
                let index = prepare_archive_scope_index(candidate, container, physical);
                #[cfg(test)]
                {
                    self.prepared_sources += 1;
                    if let Ok(index) = &index {
                        self.visited_pages += index.visited_pages;
                    }
                }
                self.archives.insert(
                    key.clone(),
                    PreparedOutputArchive {
                        _raw_tree: candidate.raw_tree.clone(),
                        index,
                    },
                );
            }
            let index = self.archives[&key].index.as_ref().map_err(Clone::clone)?;
            let (scope, domain) = index.scope_for(entry_name)?;
            let page = CompositionPageRef::ArchiveEntry {
                entry_name: entry_name.clone(),
            };
            domain
                .validate_page(&page)
                .map_err(|error| error.to_string())?;
            (scope, domain, page)
        } else {
            resolve_candidate_scope(candidate)?
        };
        // Source preparation above may enumerate physical ZIP bytes. Accept the
        // real output source only against the publication after that work, also
        // when this run already owns an index. No pre-I/O permit is reused.
        let snapshot = fresh_snapshot();
        snapshot
            .status
            .clone()
            .map_err(CandidatePermissionError::Unavailable)?;
        let storage_key = scope.storage_key();
        let Some(row) = snapshot.rows.get(&storage_key) else {
            return Ok(true);
        };
        let token = snapshot.token_for(&scope);
        if self
            .hidden
            .get(&storage_key)
            .is_none_or(|(cached, _, _)| *cached != token)
        {
            self.hidden.insert(
                storage_key.clone(),
                (
                    token,
                    row.domain.clone(),
                    row.composition
                        .hidden_pages
                        .iter()
                        .map(CompositionPageRef::matching_key)
                        .collect(),
                ),
            );
        }
        let (_, stored_domain, hidden) = &self.hidden[&storage_key];
        if stored_domain != &domain {
            return Err(CandidatePermissionError::Unavailable(
                "saved composition kind does not match candidate".into(),
            ));
        }
        Ok(!hidden.contains(&page.matching_key()))
    }
}

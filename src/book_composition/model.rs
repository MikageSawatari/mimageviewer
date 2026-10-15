use std::collections::HashSet;
use std::path::Path;

use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 2;
pub const MAX_PAGES: usize = 100_000;
pub const MAX_REFERENCE_BYTES: usize = 32_768;
pub const MAX_JSON_BYTES: usize = 16 * 1024 * 1024;

/// The composition owns binding; a matching legacy request only selects Spread viewing.
pub(crate) fn composition_spread_mode(
    direction: ReadingDirection,
    cover_enabled: bool,
) -> crate::settings::SpreadMode {
    use crate::settings::SpreadMode;
    match (direction, cover_enabled) {
        (ReadingDirection::Ltr, false) => SpreadMode::Ltr,
        (ReadingDirection::Ltr, true) => SpreadMode::LtrCover,
        (ReadingDirection::Rtl, false) => SpreadMode::Rtl,
        (ReadingDirection::Rtl, true) => SpreadMode::RtlCover,
    }
}

#[cfg(test)]
mod legacy_spread_mode_tests {
    use super::*;

    #[test]
    fn composition_spread_mode_uses_direction_and_cover_only() {
        use crate::settings::SpreadMode;
        for (direction, cover, expected) in [
            (ReadingDirection::Ltr, false, SpreadMode::Ltr),
            (ReadingDirection::Ltr, true, SpreadMode::LtrCover),
            (ReadingDirection::Rtl, false, SpreadMode::Rtl),
            (ReadingDirection::Rtl, true, SpreadMode::RtlCover),
        ] {
            assert_eq!(composition_spread_mode(direction, cover), expected);
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CompositionError {
    UnknownSchema(u32),
    Limit(&'static str),
    Invalid(&'static str),
    Json(String),
}

impl std::fmt::Display for CompositionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownSchema(version) => write!(f, "unknown composition schema {version}"),
            Self::Limit(name) => write!(f, "composition exceeds {name} limit"),
            Self::Invalid(reason) => f.write_str(reason),
            Self::Json(reason) => write!(f, "invalid composition JSON: {reason}"),
        }
    }
}

impl std::error::Error for CompositionError {}

pub type CompositionResult<T> = Result<T, CompositionError>;

/// Container-relative identity, never a cache path or an items ordinal.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CompositionPageRef {
    RelativePath { path: String },
    ArchiveEntry { entry_name: String },
    PdfPage { page_num: u32 },
}

impl CompositionPageRef {
    /// Same case/slash matching rule as bookmarks; retain original spelling for reads.
    pub(crate) fn matching_key(&self) -> Self {
        match self {
            Self::RelativePath { path } => Self::RelativePath {
                path: crate::book_bookmarks::normalize_page_path(path),
            },
            Self::ArchiveEntry { entry_name } => Self::ArchiveEntry {
                entry_name: crate::book_bookmarks::normalize_page_path(entry_name),
            },
            Self::PdfPage { .. } => self.clone(),
        }
    }

    pub fn bookmark_identity(&self) -> crate::book_bookmarks::PageIdentity {
        match self {
            Self::RelativePath { path } => {
                crate::book_bookmarks::PageIdentity::RelativePath(path.clone())
            }
            Self::ArchiveEntry { entry_name } => {
                crate::book_bookmarks::PageIdentity::ArchiveEntry(entry_name.clone())
            }
            Self::PdfPage { page_num } => crate::book_bookmarks::PageIdentity::PdfPage(*page_num),
        }
    }

    /// Logical root is supplied by the eventual scope adapter, not owned by this model.
    pub fn resume_identity(&self, logical_root: &Path) -> crate::book_resume_db::PageIdentity {
        use crate::book_resume_db::PageIdentity;
        match self {
            Self::RelativePath { path } => PageIdentity::Image {
                path: crate::path_key::normalize_keep_drive(
                    &logical_root.join(path.replace('\\', "/")),
                ),
            },
            Self::ArchiveEntry { entry_name } => PageIdentity::Archive {
                container: crate::path_key::normalize_keep_drive(logical_root),
                entry_name: entry_name.replace('\\', "/"),
            },
            Self::PdfPage { page_num } => PageIdentity::Document {
                container: crate::path_key::normalize_keep_drive(logical_root),
                page_num: *page_num as usize,
            },
        }
    }

    /// EPUB and PDF deliberately use the existing per-page edit identity.
    /// Physical read target / conversion generation cannot enter this calculation.
    pub fn document_edit_key(&self, logical_root: &Path) -> Option<String> {
        match self {
            Self::PdfPage { page_num } => Some(crate::edit_source::page_key_for_pdf(
                logical_root,
                *page_num,
            )),
            _ => None,
        }
    }
}

/// Validation context only, not a persistence key or logical/physical root alias.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PageDomain {
    ImageFolder,
    /// Full outer-root entry prefix, without a trailing slash; empty for root.
    ArchiveLevel {
        prefix: String,
    },
    /// Both PDF and EPUB use physical 0-based page numbers.
    Document,
}

impl PageDomain {
    pub(crate) fn validate(&self) -> CompositionResult<()> {
        if let Self::ArchiveLevel { prefix } = self
            && !prefix.is_empty()
        {
            safe_relative(prefix)?;
        }
        Ok(())
    }

    pub fn validate_page(&self, page: &CompositionPageRef) -> CompositionResult<()> {
        match (self, page) {
            (Self::ImageFolder, CompositionPageRef::RelativePath { path }) => {
                let path = safe_relative(path)?;
                if path.contains('/') {
                    return Err(CompositionError::Invalid(
                        "page is outside the direct folder level",
                    ));
                }
                if path
                    .chars()
                    .any(|c| matches!(c, ':' | '<' | '>' | '"' | '|' | '?' | '*'))
                    || path.ends_with(['.', ' '])
                {
                    return Err(CompositionError::Invalid(
                        "unsafe Windows file page reference",
                    ));
                }
                let stem = path
                    .split('.')
                    .next()
                    .unwrap_or_default()
                    .to_ascii_lowercase();
                if matches!(stem.as_str(), "con" | "prn" | "aux" | "nul")
                    || ["com", "lpt"].iter().any(|prefix| {
                        stem.strip_prefix(prefix).is_some_and(|number| {
                            matches!(
                                number,
                                "1" | "2"
                                    | "3"
                                    | "4"
                                    | "5"
                                    | "6"
                                    | "7"
                                    | "8"
                                    | "9"
                                    | "¹"
                                    | "²"
                                    | "³"
                            )
                        })
                    })
                {
                    return Err(CompositionError::Invalid(
                        "unsafe Windows device page reference",
                    ));
                }
            }
            (Self::ArchiveLevel { prefix }, CompositionPageRef::ArchiveEntry { entry_name }) => {
                let entry = safe_relative(entry_name)?;
                let prefix = crate::book_bookmarks::normalize_page_path(prefix);
                let entry = crate::book_bookmarks::normalize_page_path(&entry);
                let member =
                    if prefix.is_empty() {
                        entry.as_str()
                    } else {
                        entry.strip_prefix(&format!("{prefix}/")).ok_or(
                            CompositionError::Invalid("page is outside the archive level"),
                        )?
                    };
                if member.is_empty() || member.contains('/') {
                    return Err(CompositionError::Invalid(
                        "page is outside the direct archive level",
                    ));
                }
            }
            (Self::Document, CompositionPageRef::PdfPage { .. }) => {}
            _ => {
                return Err(CompositionError::Invalid(
                    "page reference kind does not match book",
                ));
            }
        }
        Ok(())
    }
}

/// Lexical validation only. A source worker remains responsible for filesystem
/// containment/reparse-point checks and for supplying a safe raw manifest.
fn safe_relative(value: &str) -> CompositionResult<String> {
    if value.len() > MAX_REFERENCE_BYTES {
        return Err(CompositionError::Limit("reference bytes"));
    }
    let value = value.replace('\\', "/");
    let drive_prefix = value.as_bytes().get(1) == Some(&b':')
        && value
            .as_bytes()
            .first()
            .is_some_and(u8::is_ascii_alphabetic);
    if value.is_empty()
        || value.starts_with('/')
        || drive_prefix
        || value.chars().any(char::is_control)
        || value
            .split('/')
            .any(|part| part.is_empty() || part == "." || part == "..")
    {
        return Err(CompositionError::Invalid("unsafe relative page reference"));
    }
    Ok(value)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReadingDirection {
    Ltr,
    Rtl,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SinglePlacement {
    Center,
    Leading,
    Trailing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ScreenSlot {
    Center,
    Left,
    Right,
}

impl SinglePlacement {
    pub fn screen_slot(self, direction: ReadingDirection) -> ScreenSlot {
        match (self, direction) {
            (Self::Center, _) => ScreenSlot::Center,
            (Self::Leading, ReadingDirection::Ltr) | (Self::Trailing, ReadingDirection::Rtl) => {
                ScreenSlot::Left
            }
            _ => ScreenSlot::Right,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PageSide {
    Leading,
    Trailing,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum PageOverride {
    Alone,
    BlankOpposite { page_side: PageSide },
}

// serde's internally tagged unit variants otherwise ignore unknown fields.
// The empty struct wire variant enforces the same strict shape as BlankOpposite.
impl<'de> Deserialize<'de> for PageOverride {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
        enum Wire {
            Alone {},
            BlankOpposite { page_side: PageSide },
        }
        Ok(match Wire::deserialize(deserializer)? {
            Wire::Alone {} => Self::Alone,
            Wire::BlankOpposite { page_side } => Self::BlankOpposite { page_side },
        })
    }
}

/// A sequence on the wire makes duplicate normalized keys observable during validation.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PageOverrideEntry {
    pub page: CompositionPageRef,
    pub directive: PageOverride,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BookComposition {
    pub schema_version: u32,
    pub revision: u64,
    pub direction: ReadingDirection,
    pub cover_enabled: bool,
    pub page_order: Vec<CompositionPageRef>,
    pub hidden_pages: Vec<CompositionPageRef>,
    pub page_overrides: Vec<PageOverrideEntry>,
}

impl BookComposition {
    pub fn new(direction: ReadingDirection, page_order: Vec<CompositionPageRef>) -> Self {
        Self {
            schema_version: SCHEMA_VERSION,
            revision: 0,
            direction,
            cover_enabled: false,
            page_order,
            hidden_pages: Vec::new(),
            page_overrides: Vec::new(),
        }
    }

    pub fn pages(&self) -> impl Iterator<Item = &CompositionPageRef> {
        self.page_order.iter()
    }

    pub fn page_override(&self, page: &CompositionPageRef) -> Option<PageOverride> {
        let key = page.matching_key();
        self.page_overrides
            .iter()
            .find(|entry| entry.page.matching_key() == key)
            .map(|entry| entry.directive)
    }

    pub fn validate(&self, domain: &PageDomain) -> CompositionResult<()> {
        if self.schema_version != SCHEMA_VERSION {
            return Err(CompositionError::UnknownSchema(self.schema_version));
        }
        domain.validate()?;
        if self.page_order.len() > MAX_PAGES
            || self.hidden_pages.len() > MAX_PAGES
            || self.page_overrides.len() > MAX_PAGES
        {
            return Err(CompositionError::Limit("page count"));
        }
        let mut registered = HashSet::with_capacity(self.page_order.len());
        for page in &self.page_order {
            domain.validate_page(page)?;
            if !registered.insert(page.matching_key()) {
                return Err(CompositionError::Invalid(
                    "duplicate ordinary page occurrence",
                ));
            }
        }
        let mut hidden = HashSet::new();
        for page in &self.hidden_pages {
            domain.validate_page(page)?;
            let key = page.matching_key();
            if !registered.contains(&key) || !hidden.insert(key) {
                return Err(CompositionError::Invalid(
                    "hidden pages must be a unique subset of registered pages",
                ));
            }
        }
        let mut overrides = HashSet::new();
        for entry in &self.page_overrides {
            domain.validate_page(&entry.page)?;
            let key = entry.page.matching_key();
            if !registered.contains(&key) || !overrides.insert(key) {
                return Err(CompositionError::Invalid(
                    "override keys must be a unique subset of registered pages",
                ));
            }
        }
        let mut count = JsonByteCount(0);
        serde_json::to_writer(&mut count, self)
            .map_err(|_| CompositionError::Limit("JSON bytes"))?;
        Ok(())
    }

    pub fn from_json(json: &[u8], domain: &PageDomain) -> CompositionResult<Self> {
        if json.len() > MAX_JSON_BYTES {
            return Err(CompositionError::Limit("JSON bytes"));
        }
        let composition: Self = serde_json::from_slice(json)
            .map_err(|error| CompositionError::Json(error.to_string()))?;
        composition.validate(domain)?;
        Ok(composition)
    }

    pub fn to_json(&self, domain: &PageDomain) -> CompositionResult<Vec<u8>> {
        self.validate(domain)?;
        serde_json::to_vec(self).map_err(|error| CompositionError::Json(error.to_string()))
    }
}

struct JsonByteCount(usize);
impl std::io::Write for JsonByteCount {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0 = self.0.saturating_add(bytes.len());
        if self.0 > MAX_JSON_BYTES {
            return Err(std::io::Error::other("composition JSON byte limit"));
        }
        Ok(bytes.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

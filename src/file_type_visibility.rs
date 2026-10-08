//! Persistent list policy. Logical sources determine visibility, never decode/cache paths.

use std::collections::BTreeSet;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileTypeCategory {
    Image,
    Raw,
    Video,
    Audio,
    Zip,
    Pdf,
    Epub,
    Rar,
    SevenZip,
    Lzh,
}

impl FileTypeCategory {
    pub const ALL: [Self; 10] = [
        Self::Image,
        Self::Raw,
        Self::Video,
        Self::Audio,
        Self::Zip,
        Self::Pdf,
        Self::Epub,
        Self::Rar,
        Self::SevenZip,
        Self::Lzh,
    ];

    pub fn label(self) -> &'static str {
        match self {
            Self::Image => "画像",
            Self::Raw => "RAW",
            Self::Video => "動画",
            Self::Audio => "音声",
            Self::Zip => "ZIP / CBZ",
            Self::Pdf => "PDF",
            Self::Epub => "EPUB",
            Self::Rar => "RAR / CBR",
            Self::SevenZip => "7z / CB7",
            Self::Lzh => "LZH / LHA",
        }
    }
}

/// Empty exclusions also allow formats added in a later version.
#[derive(Clone, Debug, Default, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct FileTypeVisibility {
    #[serde(default)]
    pub excluded_categories: BTreeSet<FileTypeCategory>,
    #[serde(default, deserialize_with = "deserialize_extensions")]
    pub excluded_extensions: BTreeSet<String>,
}

/// Recognition happens before projection. A directory is a passage, including archive directories;
/// actual nested archive boundaries must be passed separately using enumeration provenance.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ListingCandidateType {
    PhysicalFile {
        source_path: PathBuf,
        category: FileTypeCategory,
    },
    ArchiveMember {
        entry_name: String,
        category: FileTypeCategory,
    },
    DocumentPage {
        source_path: PathBuf,
        category: FileTypeCategory,
    },
    Directory,
    StructuralReference {
        source_path: Option<PathBuf>,
        category: Option<FileTypeCategory>,
    },
}

impl ListingCandidateType {
    pub fn physical_file(path: impl Into<PathBuf>, category: FileTypeCategory) -> Self {
        Self::PhysicalFile {
            source_path: path.into(),
            category,
        }
    }
    pub fn archive_member(name: impl Into<String>, category: FileTypeCategory) -> Self {
        Self::ArchiveMember {
            entry_name: name.into(),
            category,
        }
    }
    pub fn document_page(path: impl Into<PathBuf>, category: FileTypeCategory) -> Self {
        Self::DocumentPage {
            source_path: path.into(),
            category,
        }
    }
}

/// Normalize an extension supplied by settings/UI, rejecting paths and malformed tokens.
pub fn normalize_extension(value: &str) -> Result<String, &'static str> {
    let extension = value.trim().trim_start_matches('.').to_ascii_lowercase();
    if extension.is_empty() || !extension.bytes().all(|byte| byte.is_ascii_alphanumeric()) {
        return Err("拡張子はドットなしの半角英数字で指定してください");
    }
    Ok(extension)
}

fn deserialize_extensions<'de, D: serde::Deserializer<'de>>(
    deserializer: D,
) -> Result<BTreeSet<String>, D::Error> {
    let extensions = Vec::<String>::deserialize(deserializer)?;
    extensions
        .into_iter()
        .map(|extension| normalize_extension(&extension).map_err(serde::de::Error::custom))
        .collect()
}

pub fn category_for_extension(extension: &str) -> Option<FileTypeCategory> {
    use FileTypeCategory as C;
    let extension = extension.to_ascii_lowercase();
    if crate::raw_format::is_raw_ext(&extension) {
        return Some(C::Raw);
    }
    if crate::folder_tree::is_recognized_image_ext(&extension) {
        return Some(C::Image);
    }
    if crate::folder_tree::SUPPORTED_VIDEO_EXTENSIONS.contains(&extension.as_str()) {
        return Some(C::Video);
    }
    if crate::folder_tree::is_audio_ext(&extension) {
        return Some(C::Audio);
    }
    if crate::folder_tree::is_zip_extension(&extension) {
        return Some(C::Zip);
    }
    if crate::folder_tree::is_pdf_extension(&extension) {
        return Some(C::Pdf);
    }
    if extension == "epub" {
        return Some(C::Epub);
    }
    crate::archive_converter::ArchiveFormat::from_extension(&extension)
        .map(category_for_archive_format)
}

pub fn category_for_archive_format(
    format: crate::archive_converter::ArchiveFormat,
) -> FileTypeCategory {
    use crate::archive_converter::ArchiveFormat;
    match format {
        ArchiveFormat::Zip => FileTypeCategory::Zip,
        ArchiveFormat::Rar => FileTypeCategory::Rar,
        ArchiveFormat::SevenZ => FileTypeCategory::SevenZip,
        ArchiveFormat::Lzh => FileTypeCategory::Lzh,
    }
}

impl FileTypeVisibility {
    pub fn is_all_visible(&self) -> bool {
        self.excluded_categories.is_empty() && self.excluded_extensions.is_empty()
    }

    /// Old converted caches need origin manifests only when archive boundaries may be excluded.
    pub fn requires_archive_origins(&self) -> bool {
        self.excluded_categories.iter().any(|category| {
            matches!(
                category,
                FileTypeCategory::Zip
                    | FileTypeCategory::Rar
                    | FileTypeCategory::SevenZip
                    | FileTypeCategory::Lzh
            )
        }) || self.excluded_extensions.iter().any(|extension| {
            crate::archive_converter::ArchiveFormat::nested_from_extension(extension).is_some()
        })
    }

    pub fn allows_extension(&self, category: FileTypeCategory, extension: &str) -> bool {
        !self.excluded_categories.contains(&category)
            && !self
                .excluded_extensions
                .contains(&extension.to_ascii_lowercase())
    }

    pub fn allows_path(&self, category: FileTypeCategory, source_path: &Path) -> bool {
        self.allows_extension(
            category,
            source_path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or(""),
        )
    }

    /// Unknown references are retained; this policy does not add format support or perform stat.
    pub fn allows_physical_file(&self, source_path: &Path) -> bool {
        let extension = source_path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or("");
        match category_for_extension(extension) {
            Some(category) => self.allows_extension(category, extension),
            None => true,
        }
    }

    pub fn allows(&self, candidate: &ListingCandidateType) -> bool {
        match candidate {
            ListingCandidateType::PhysicalFile {
                source_path,
                category,
            }
            | ListingCandidateType::DocumentPage {
                source_path,
                category,
            } => self.allows_path(*category, source_path),
            ListingCandidateType::ArchiveMember {
                entry_name,
                category,
            } => self.allows_path(*category, Path::new(entry_name)),
            ListingCandidateType::Directory => true,
            ListingCandidateType::StructuralReference {
                source_path,
                category,
            } => match (source_path, category) {
                (Some(path), Some(category)) => self.allows_path(*category, path),
                (None, Some(category)) => !self.excluded_categories.contains(category),
                _ => true,
            },
        }
    }

    /// Adapter for recognized leaf rows. Aggregates must be built from policy-projected members.
    /// Cached document/archive callers must supply their logical source through `allows_path`.
    pub fn allows_grid_item(&self, item: &crate::grid_item::GridItem) -> bool {
        use crate::grid_item::GridItem as G;
        use FileTypeCategory as C;
        match item {
            G::Image(path) => self.allows_path(
                if crate::raw_format::is_raw_path(path) {
                    C::Raw
                } else {
                    C::Image
                },
                path,
            ),
            G::Video(path) => self.allows_path(C::Video, path),
            G::Audio(path) => self.allows_path(C::Audio, path),
            G::ZipFile(path) => self.allows_path(C::Zip, path),
            G::PdfFile(path) | G::PdfPage { pdf_path: path, .. } => self.allows_path(
                if path
                    .extension()
                    .and_then(|ext| ext.to_str())
                    .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
                {
                    C::Epub
                } else {
                    C::Pdf
                },
                path,
            ),
            G::ConvertibleArchive { path, format } => {
                self.allows_path(category_for_archive_format(*format), path)
            }
            G::ZipImage { entry_name, .. } => self.allows_path(
                if crate::raw_format::is_raw_path(Path::new(entry_name)) {
                    C::Raw
                } else {
                    C::Image
                },
                Path::new(entry_name),
            ),
            G::CollectionPlaceholder {
                path,
                last_known_kind,
                ..
            } => {
                use crate::collection_store::CollectionResolvedKind as K;
                match last_known_kind {
                    K::Image => self.allows_path(
                        if crate::raw_format::is_raw_path(path) {
                            C::Raw
                        } else {
                            C::Image
                        },
                        path,
                    ),
                    K::Video => self.allows_path(C::Video, path),
                    K::Audio => self.allows_path(C::Audio, path),
                    K::Zip => self.allows_path(C::Zip, path),
                    K::Pdf => self.allows_path(
                        if path
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
                        {
                            C::Epub
                        } else {
                            C::Pdf
                        },
                        path,
                    ),
                    K::ConvertibleArchive => self.allows_physical_file(path),
                    K::Folder | K::Unresolved => true,
                }
            }
            G::Folder(_) | G::ZipDir { .. } | G::Stack { .. } | G::SearchContainer { .. } => true,
        }
    }

    /// Stable FNV-1a over canonical ordered policy values, also suitable for persistent proofs.
    pub fn fingerprint(&self) -> u64 {
        let mut hash = 0xcbf29ce484222325_u64;
        let mut feed = |byte: u8| {
            hash ^= u64::from(byte);
            hash = hash.wrapping_mul(0x100000001b3);
        };
        feed(1); // format version
        for category in &self.excluded_categories {
            feed(*category as u8);
        }
        feed(255);
        for extension in &self.excluded_extensions {
            for byte in extension.bytes() {
                feed(byte);
            }
            feed(0);
        }
        hash
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_and_member_document_identity() {
        let mut policy = FileTypeVisibility::default();
        assert!(policy.is_all_visible());
        for category in FileTypeCategory::ALL {
            assert!(policy.allows_extension(category, "anything"));
        }
        policy.excluded_extensions.insert("png".into());
        assert!(!policy.allows(&ListingCandidateType::archive_member(
            "pages/2.PNG",
            FileTypeCategory::Image
        )));
        assert!(policy.allows(&ListingCandidateType::document_page(
            "book.pdf",
            FileTypeCategory::Pdf
        )));
        policy.excluded_categories.insert(FileTypeCategory::Epub);
        assert!(!policy.allows_grid_item(&crate::grid_item::GridItem::PdfFile("book.epub".into())));
        assert!(policy.allows(&ListingCandidateType::Directory));
        assert!(policy.allows(&ListingCandidateType::StructuralReference {
            source_path: None,
            category: None
        }));
    }

    #[test]
    fn category_aliases_and_individual_raw_extension() {
        for (extension, category) in [
            ("CR2", FileTypeCategory::Raw),
            ("cbz", FileTypeCategory::Zip),
            ("epub", FileTypeCategory::Epub),
            ("cbr", FileTypeCategory::Rar),
            ("cb7", FileTypeCategory::SevenZip),
            ("lha", FileTypeCategory::Lzh),
        ] {
            assert_eq!(category_for_extension(extension), Some(category));
        }
        let policy: FileTypeVisibility =
            serde_json::from_str(r#"{"excluded_extensions":[".CR2","cr2"]}"#).unwrap();
        assert_eq!(policy.excluded_extensions.len(), 1);
        assert!(!policy.allows_physical_file(Path::new("photo.CR2")));
        assert!(policy.allows_physical_file(Path::new("photo.nef")));
    }

    #[test]
    fn strict_category_and_extension_parse_and_stable_fingerprint() {
        assert!(
            serde_json::from_str::<FileTypeVisibility>(r#"{"excluded_categories":["future"]}"#)
                .is_err()
        );
        assert!(
            serde_json::from_str::<FileTypeVisibility>(r#"{"excluded_extensions":["../jpg"]}"#)
                .is_err()
        );
        let a: FileTypeVisibility =
            serde_json::from_str(r#"{"excluded_extensions":["PNG",".jpg"]}"#).unwrap();
        let b: FileTypeVisibility =
            serde_json::from_str(r#"{"excluded_extensions":["jpg","png","png"]}"#).unwrap();
        assert_eq!(a, b);
        assert_eq!(a.fingerprint(), b.fingerprint());
        assert_ne!(a.fingerprint(), FileTypeVisibility::default().fingerprint());
    }
}

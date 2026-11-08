//! Resume storage identity: released physical keys and logical archive-book scopes.

use std::path::{Path, PathBuf};

const ARCHIVE_NAMESPACE: &str = "miv:zipbook:v1:";

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub enum BookResumeKey {
    LegacyPath(String),
    ArchiveLevel {
        logical_source: PathBuf,
        raw_prefix: String,
    },
}

impl BookResumeKey {
    pub fn legacy(path: &Path) -> Self {
        Self::LegacyPath(crate::path_key::normalize(path))
    }

    pub fn archive(logical_source: &Path, raw: &[String]) -> Result<Self, String> {
        let scope = crate::book_scope::BookScopeKey::archive(logical_source, raw)?;
        if raw.is_empty() {
            return Err("archive resume requires a non-root book prefix".into());
        }
        Ok(Self::ArchiveLevel {
            logical_source: scope.logical_root,
            // Scope validation is shared, but ZIP directory spelling is case-sensitive.
            raw_prefix: raw.join("/").replace('\\', "/"),
        })
    }

    /// Callers supply their released root key (PC backing or Remote logical source).
    pub fn for_archive(
        logical_source: &Path,
        legacy_root: &Path,
        nav: &crate::zip_tree::ZipNavState,
    ) -> Result<Self, String> {
        if nav.at_root() {
            Ok(Self::legacy(legacy_root))
        } else {
            Self::archive(logical_source, nav.current_raw())
        }
    }

    pub fn storage_key(&self) -> String {
        match self {
            Self::LegacyPath(key) => key.clone(),
            Self::ArchiveLevel {
                logical_source,
                raw_prefix,
            } => format!(
                "{ARCHIVE_NAMESPACE}{}:{}",
                encode_hex(logical_source.to_string_lossy().as_bytes()),
                encode_hex(raw_prefix.as_bytes()),
            ),
        }
    }

    pub fn display_label(&self) -> String {
        match self {
            Self::LegacyPath(path) => path.clone(),
            Self::ArchiveLevel {
                logical_source,
                raw_prefix,
            } => {
                format!("{}::{raw_prefix}", logical_source.display())
            }
        }
    }

    pub fn from_storage_key(value: &str) -> Result<Self, String> {
        let Some(encoded) = value.strip_prefix(ARCHIVE_NAMESPACE) else {
            if value.starts_with("miv:zipbook:") {
                return Err("unsupported archive resume key".into());
            }
            return Ok(Self::LegacyPath(value.to_owned()));
        };
        let (source, prefix) = encoded
            .split_once(':')
            .ok_or("invalid archive resume key")?;
        let source = decode_hex(source)?;
        let prefix = decode_hex(prefix)?;
        let result = Self::archive(Path::new(&source), &[prefix])?;
        if result.storage_key() != value {
            return Err("noncanonical archive resume key".into());
        }
        Ok(result)
    }

    /// Existing physical-key semantics stay drive-stripped; archive sources retain drives.
    pub fn matches_source_family(&self, path: &Path, tree: bool) -> bool {
        let (source, root) = match self {
            Self::LegacyPath(key) => (key.clone(), crate::path_key::normalize(path)),
            Self::ArchiveLevel { logical_source, .. } => (
                crate::path_key::normalize_keep_drive(logical_source),
                crate::path_key::normalize_keep_drive(path),
            ),
        };
        let root = root.trim_end_matches('/');
        source.trim_end_matches('/') == root
            || (tree
                && source.strip_prefix(root).is_some_and(|suffix| {
                    suffix.starts_with('/')
                        || (matches!(self, Self::LegacyPath(_)) && suffix.starts_with("::"))
                }))
    }
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut result = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        result.push(HEX[(byte >> 4) as usize] as char);
        result.push(HEX[(byte & 15) as usize] as char);
    }
    result
}

fn decode_hex(value: &str) -> Result<String, String> {
    if value.len() % 2 != 0 {
        return Err("invalid archive resume hex".into());
    }
    let digit = |byte: u8| match byte {
        b'0'..=b'9' => Ok(byte - b'0'),
        b'a'..=b'f' => Ok(byte - b'a' + 10),
        _ => Err("invalid archive resume hex".to_owned()),
    };
    let bytes = value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| Ok((digit(pair[0])? << 4) | digit(pair[1])?))
        .collect::<Result<Vec<_>, String>>()?;
    String::from_utf8(bytes).map_err(|_| "invalid archive resume UTF-8".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;

    fn nav(names: &[&str]) -> crate::zip_tree::ZipNavState {
        crate::zip_tree::ZipNavState::new(Arc::new(crate::zip_tree::ZipTree::build(
            "C:/cache/backing.zip".into(),
            names
                .iter()
                .map(|name| crate::zip_loader::ZipImageEntry {
                    entry_name: (*name).into(),
                    uncompressed_size: 0,
                    mtime: 0,
                    container_ancestry: Vec::new(),
                })
                .collect(),
        )))
    }

    #[test]
    fn section1675_inner_resume_key_namespace() {
        let mut storage = std::collections::HashSet::new();
        for source in [
            "C:/Books/外側.zip",
            "D:/Books/外側.zip",
            "//server/share/外側.zip",
            "C:/Books/a_b%file.zip",
        ] {
            for prefix in ["A", "a", "日本語/inner.zip/章", "semi;_%-name"] {
                let key = BookResumeKey::archive(Path::new(source), &[prefix.into()]).unwrap();
                let encoded = key.storage_key();
                assert!(encoded.starts_with(ARCHIVE_NAMESPACE));
                assert!(storage.insert(encoded.clone()));
                assert_eq!(BookResumeKey::from_storage_key(&encoded).unwrap(), key);
                assert!(!key.display_label().contains(ARCHIVE_NAMESPACE));
                assert_ne!(
                    encoded,
                    BookResumeKey::legacy(Path::new(source)).storage_key()
                );
            }
        }
        assert_eq!(
            BookResumeKey::archive(Path::new(r"C:\Books\A.ZIP"), &[r"A\inner.zip".into()]).unwrap(),
            BookResumeKey::archive(Path::new("c:/books/a.zip"), &["A/inner.zip".into()]).unwrap()
        );
        for prefix in ["", "../A", "A/..", "/A", "A//B", "A:B", "A\0B"] {
            assert!(BookResumeKey::archive(Path::new("C:/A.zip"), &[prefix.into()]).is_err());
        }
        for invalid in [
            "miv:zipbook:v2:00:00",
            "miv:zipbook:v1:0:00",
            "miv:zipbook:v1:ff:41",
            "miv:zipbook:v1:43:2E2E",
            "miv:zipbook:v1:63:2e2e",
        ] {
            assert!(BookResumeKey::from_storage_key(invalid).is_err());
        }
    }

    #[test]
    fn section1675_inner_resume_root_keys_unchanged() {
        let logical = Path::new("C:/Books/original.rar");
        let backing = Path::new("D:/Cache/backing.zip");
        for root in [
            nav(&["A/page.jpg", "B/page.jpg"]),
            nav(&["wrapper/only/page.jpg"]),
        ] {
            let pc = BookResumeKey::for_archive(logical, backing, &root).unwrap();
            let remote = BookResumeKey::for_archive(logical, logical, &root).unwrap();
            assert_eq!(pc.storage_key(), crate::path_key::normalize(backing));
            assert_eq!(remote.storage_key(), crate::path_key::normalize(logical));
            assert_eq!(
                BookResumeKey::from_storage_key(&pc.storage_key()).unwrap(),
                pc
            );
            assert_ne!(pc, remote);
        }
        for physical in ["C:/Folder", "D:/book.pdf", "//server/share/folder"] {
            let key = BookResumeKey::legacy(Path::new(physical));
            assert_eq!(
                key.storage_key(),
                crate::path_key::normalize(Path::new(physical))
            );
        }
    }

    #[test]
    fn section1675_inner_resume_key_uses_raw_scope_before_policy_collapse() {
        let source = nav(&["root.jpg", "A/cover.png", "A/child/page.jpg"]);
        let mut policy = crate::file_type_visibility::FileTypeVisibility::default();
        policy.excluded_extensions.insert("png".into());
        let mut projected =
            crate::zip_tree::ZipNavState::new(Arc::new(source.tree.project(&policy)));
        projected.enter("A/");
        assert_eq!(projected.current(), &["A".to_owned(), "child".to_owned()]);
        assert_eq!(projected.current_raw(), &["A".to_owned()]);
        let key = BookResumeKey::for_archive(
            Path::new("C:/Source.rar"),
            Path::new("D:/cache.zip"),
            &projected,
        )
        .unwrap();
        assert_eq!(
            key,
            BookResumeKey::archive(Path::new("C:/Source.rar"), &["A".into()]).unwrap()
        );
    }

    #[test]
    fn section1675_inner_resume_family_preserves_drive_and_component_boundaries() {
        let key = BookResumeKey::archive(Path::new("C:/Books/A.zip"), &["A".into()]).unwrap();
        assert!(key.matches_source_family(Path::new("c:/books/a.ZIP"), false));
        assert!(key.matches_source_family(Path::new("C:/Books"), true));
        assert!(!key.matches_source_family(Path::new("C:/Books"), false));
        assert!(!key.matches_source_family(Path::new("C:/Book"), true));
        assert!(!key.matches_source_family(Path::new("D:/Books"), true));
        assert!(
            BookResumeKey::legacy(Path::new("C:/Books/A.zip"))
                .matches_source_family(Path::new("D:/Books/A.zip"), false)
        );
        let legacy_page = BookResumeKey::LegacyPath("/books/a.zip::page.jpg".into());
        assert!(legacy_page.matches_source_family(Path::new("C:/Books/A.zip"), true));
        assert!(!legacy_page.matches_source_family(Path::new("C:/Books/A.zip"), false));
    }
}

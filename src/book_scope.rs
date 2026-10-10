//! Canonical logical persistence scope shared by spread shifts and book composition.
//! G's semantic BookScope classification adapts here; it does not normalize keys itself.

use std::path::{Path, PathBuf};

pub(crate) const MAX_BOOK_REFERENCE_BYTES: usize = 16 * 1024;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub(crate) struct BookScopeKey {
    pub(crate) logical_root: PathBuf,
    pub(crate) level_prefix: String,
}

/// Validate a relative reference without opening it. Preserve spelling for byte readers.
pub(crate) fn validate_relative_reference(value: &str) -> Result<(), String> {
    if value.is_empty() || value.len() > MAX_BOOK_REFERENCE_BYTES {
        return Err("empty or oversized book reference".into());
    }
    if value.contains(['\0', ':']) || value.starts_with(['/', '\\']) {
        return Err("absolute or unsafe book reference".into());
    }
    if value
        .split(['/', '\\'])
        .any(|part| matches!(part, "" | "." | ".."))
    {
        return Err("unsafe book reference component".into());
    }
    Ok(())
}

pub(crate) fn normalize_level_prefix(prefix: &str) -> Result<String, String> {
    if prefix.is_empty() {
        return Ok(String::new());
    }
    validate_relative_reference(prefix)?;
    Ok(prefix.replace('\\', "/").to_lowercase())
}

impl BookScopeKey {
    pub(crate) fn new(logical_root: impl AsRef<Path>, level_prefix: &str) -> Result<Self, String> {
        let root = crate::path_key::normalize_keep_drive(logical_root.as_ref());
        if root.is_empty() || root.contains('\0') || root.len() > MAX_BOOK_REFERENCE_BYTES {
            return Err("empty or oversized book root".into());
        }
        // Keep the slash in a drive root. Other trailing separators have no scope meaning.
        let root = if root == "/" || (root.len() == 3 && root.as_bytes()[1] == b':') {
            root
        } else {
            root.trim_end_matches('/').to_owned()
        };
        Ok(Self {
            logical_root: root.into(),
            level_prefix: normalize_level_prefix(level_prefix)?,
        })
    }

    pub(crate) fn archive(root: impl AsRef<Path>, segments: &[String]) -> Result<Self, String> {
        // A segment may represent a collapsed wrapper path, but never an absolute component.
        for segment in segments {
            validate_relative_reference(segment)?;
        }
        Self::new(root, &segments.join("/"))
    }

    pub(crate) fn document(root: impl AsRef<Path>) -> Result<Self, String> {
        Self::new(root, "")
    }

    pub(crate) fn folder(root: impl AsRef<Path>) -> Result<Self, String> {
        Self::new(root, "")
    }

    pub(crate) fn storage_key(&self) -> String {
        let root = crate::path_key::normalize_keep_drive(&self.logical_root);
        if self.level_prefix.is_empty() {
            root
        } else {
            format!(
                "{}/{prefix}",
                root.trim_end_matches('/'),
                prefix = self.level_prefix
            )
        }
    }

    pub(crate) fn validate(&self) -> Result<(), String> {
        let canonical = Self::new(&self.logical_root, &self.level_prefix)?;
        if &canonical != self {
            return Err("noncanonical book scope".into());
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(
    tag = "kind",
    content = "prefixes",
    rename_all = "snake_case",
    deny_unknown_fields
)]
pub(crate) enum BookScopeCoverage {
    Root,
    Prefixes(Vec<String>),
    Family,
}

impl BookScopeCoverage {
    pub(crate) fn validate(&self) -> Result<(), String> {
        if let Self::Prefixes(prefixes) = self {
            let mut seen = std::collections::HashSet::new();
            for prefix in prefixes {
                let normalized = normalize_level_prefix(prefix)?;
                if &normalized != prefix || !seen.insert(normalized) {
                    return Err("duplicate or noncanonical scope coverage".into());
                }
            }
        }
        Ok(())
    }

    pub(crate) fn contains(&self, prefix: &str) -> bool {
        match self {
            Self::Root => prefix.is_empty(),
            Self::Prefixes(prefixes) => prefixes.iter().any(|value| value == prefix),
            Self::Family => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scope_preserves_drive_and_raw_effective_prefix_without_parent_fallback() {
        let scope = BookScopeKey::archive(
            "C:\\Books\\A.ZIP",
            &["Wrapper/Chapter".into(), "inner.zip".into()],
        )
        .unwrap();
        assert_eq!(
            scope.storage_key(),
            "c:/books/a.zip/wrapper/chapter/inner.zip"
        );
        assert_ne!(
            scope,
            BookScopeKey::archive(
                "D:/Books/A.ZIP",
                &["Wrapper/Chapter".into(), "inner.zip".into()]
            )
            .unwrap()
        );
        assert_ne!(scope, BookScopeKey::document("C:/Books/A.ZIP").unwrap());
        assert_eq!(
            BookScopeKey::folder("C:/Books/A/").unwrap().storage_key(),
            "c:/books/a"
        );
        assert_eq!(BookScopeKey::folder("C:/").unwrap().storage_key(), "c:/");
    }

    #[test]
    fn unsafe_prefixes_and_references_are_rejected() {
        for invalid in ["..", "a/../b", "a//b", "a/", "/a", "D:a", "a\0b", "a/./b"] {
            assert!(normalize_level_prefix(invalid).is_err(), "{invalid:?}");
            assert!(BookScopeKey::archive("C:/book.zip", &[invalid.into()]).is_err());
        }
        assert_eq!(normalize_level_prefix("A\\B").unwrap(), "a/b");
    }

    #[test]
    fn coverage_is_exact_and_never_inherits_parent_scope() {
        let coverage = BookScopeCoverage::Prefixes(vec!["".into(), "chapter".into()]);
        coverage.validate().unwrap();
        assert!(coverage.contains("chapter"));
        assert!(!coverage.contains("chapter/child"));
        assert!(
            BookScopeCoverage::Prefixes(vec!["chapter".into(), "chapter".into()])
                .validate()
                .is_err()
        );
        let wire = serde_json::to_string(&coverage).unwrap();
        assert_eq!(
            serde_json::from_str::<BookScopeCoverage>(&wire).unwrap(),
            coverage
        );
    }
}

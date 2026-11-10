//! Immutable, I/O-free source facts captured while assembling a book thumbnail request.
use crate::rotation_db::Rotation;
use std::collections::BTreeMap;

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CoverSourceScope {
    keys: Vec<String>,
    prefixes: Vec<String>,
}

impl CoverSourceScope {
    pub(crate) fn page(key: String) -> Self {
        Self {
            keys: vec![key],
            prefixes: Vec::new(),
        }
    }
    pub(crate) fn prefix(prefix: String) -> Self {
        Self {
            keys: Vec::new(),
            prefixes: vec![prefix],
        }
    }
    pub(crate) fn folder(path: &std::path::Path) -> Self {
        Self::prefix(format!(
            "{}/",
            crate::adjustment_db::normalize_path(path).trim_end_matches('/')
        ))
    }
    pub(crate) fn archive(path: &std::path::Path) -> Self {
        Self::prefix(format!("{}::", crate::adjustment_db::normalize_path(path)))
    }
    pub(crate) fn zip_dir(path: &std::path::Path, prefix: &str) -> Self {
        Self::prefix(crate::adjustment_db::zip_entry_key(path, prefix))
    }
    pub(crate) fn union(mut self, other: Self) -> Self {
        for key in other.keys {
            if !self.keys.contains(&key) {
                self.keys.push(key);
            }
        }
        for prefix in other.prefixes {
            if !self.prefixes.contains(&prefix) {
                self.prefixes.push(prefix);
            }
        }
        self
    }
    pub(crate) fn has_rotation(&self, owner: &BTreeMap<String, Rotation>) -> bool {
        self.keys.iter().any(|key| owner.contains_key(key))
            || self.prefixes.iter().any(|prefix| {
                owner
                    .range(prefix.clone()..)
                    .next()
                    .is_some_and(|(key, _)| key.starts_with(prefix))
            })
    }
}

/// Only accepted display material enriches Legacy with its request's source facts.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LoadedCoverIdentity {
    Legacy { scope: Option<CoverSourceScope> },
    Unknown,
    Unrotatable,
    Page(String),
}
impl LoadedCoverIdentity {
    pub(crate) fn from_material(
        cover: crate::cover_identity::CoverIdentity,
        scope: Option<CoverSourceScope>,
    ) -> Self {
        match cover {
            crate::cover_identity::CoverIdentity::Legacy => Self::Legacy { scope },
            crate::cover_identity::CoverIdentity::Unknown => Self::Unknown,
            crate::cover_identity::CoverIdentity::Unrotatable => Self::Unrotatable,
            crate::cover_identity::CoverIdentity::Page(key) => Self::Page(key),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn source_scope_matches_boundaries_and_recursive_union() {
        let scope = CoverSourceScope::folder(std::path::Path::new("c:/books/one"))
            .union(CoverSourceScope::zip_dir(
                std::path::Path::new("c:/cache/book.zip"),
                "part/",
            ))
            .union(CoverSourceScope::page("c:/outside/cover.png".into()));
        for key in [
            "c:/books/one/cover.jpg",
            "c:/cache/book.zip::part/cover.png",
            "c:/outside/cover.png",
        ] {
            assert!(
                scope.has_rotation(&BTreeMap::from([(key.into(), Rotation::Cw90)])),
                "{key}"
            );
        }
        for key in [
            "c:/books/one-more/cover.jpg",
            "c:/cache/book.zip::parts/cover.png",
            "c:/outside/cover.png-more",
        ] {
            assert!(
                !scope.has_rotation(&BTreeMap::from([(key.into(), Rotation::Cw90)])),
                "{key}"
            );
        }
    }
}

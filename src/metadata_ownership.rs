//! Metadata scopes share one deterministic ownership rule; no filesystem I/O.
use crate::settings::FavoriteEntry;
use std::collections::{BTreeSet, HashMap, HashSet};
use std::path::{Path, PathBuf};
use uuid::Uuid;

pub fn root_key(path: &Path) -> String {
    crate::path_key::normalize_keep_drive(path)
        .trim_end_matches('/')
        .to_owned()
}

pub fn contains(root: &str, path: &str) -> bool {
    path == root
        || path
            .strip_prefix(root)
            .is_some_and(|rest| rest.starts_with('/'))
}

pub fn overlaps(a: &str, b: &str) -> bool {
    contains(a, b) || contains(b, a)
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PathRange {
    pub start: String,
    pub end: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OwnedRange {
    pub root: String,
    pub exclusions: Vec<String>,
}

impl OwnedRange {
    pub fn contains(&self, path: &str) -> bool {
        contains(&self.root, path) && !self.exclusions.iter().any(|e| contains(e, path))
    }

    /// Half-open BINARY SQL intervals. A separate point interval includes root itself.
    pub fn sql_ranges(&self) -> Vec<PathRange> {
        if self.exclusions.iter().any(|e| contains(e, &self.root)) {
            return Vec::new();
        }
        let mut ranges = vec![
            PathRange {
                start: self.root.clone(),
                end: format!("{}\0", self.root),
            },
            PathRange {
                start: format!("{}/", self.root),
                end: format!("{}0", self.root),
            },
        ];
        for excluded in &self.exclusions {
            if !contains(&self.root, excluded) {
                continue;
            }
            for cut in [
                PathRange {
                    start: excluded.clone(),
                    end: format!("{excluded}\0"),
                },
                PathRange {
                    start: format!("{excluded}/"),
                    end: format!("{excluded}0"),
                },
            ] {
                ranges = ranges
                    .into_iter()
                    .flat_map(|r| {
                        if cut.end <= r.start || cut.start >= r.end {
                            return vec![r];
                        }
                        let mut out = Vec::new();
                        if r.start < cut.start {
                            out.push(PathRange {
                                start: r.start,
                                end: cut.start.clone(),
                            });
                        }
                        if cut.end < r.end {
                            out.push(PathRange {
                                start: cut.end.clone(),
                                end: r.end,
                            });
                        }
                        out
                    })
                    .collect();
            }
        }
        ranges
    }
}

#[derive(Clone, Debug)]
pub struct FavoriteOwnership {
    pub id: Uuid,
    pub root: String,
    pub effective_metadata: bool,
    pub excluded_roots: Vec<PathBuf>,
    pub owned_range: Option<OwnedRange>,
}

#[derive(Clone, Debug, Default)]
pub struct MetadataOwnership {
    pub favorites: HashMap<Uuid, FavoriteOwnership>,
}

pub fn metadata_ownership(
    favorites: &[FavoriteEntry],
    common_excluded: &[PathBuf],
) -> MetadataOwnership {
    let common: BTreeSet<String> = common_excluded.iter().map(|p| root_key(p)).collect();
    let mut winners: HashMap<String, Uuid> = HashMap::new();
    for f in favorites.iter().filter(|f| f.auto_index_metadata) {
        let root = root_key(&f.path);
        winners
            .entry(root)
            .and_modify(|id| {
                if f.id.to_string() < id.to_string() {
                    *id = f.id;
                }
            })
            .or_insert(f.id);
    }
    MetadataOwnership {
        favorites: favorites
            .iter()
            .map(|f| {
                let root = root_key(&f.path);
                let effective_metadata = winners.get(&root) == Some(&f.id);
                let exclusions: BTreeSet<String> = common
                    .iter()
                    .cloned()
                    .chain(
                        winners
                            .keys()
                            .filter(|r| **r != root && contains(&root, r))
                            .cloned(),
                    )
                    .collect();
                let owned_range = effective_metadata.then(|| OwnedRange {
                    root: root.clone(),
                    exclusions: exclusions.iter().cloned().collect(),
                });
                (
                    f.id,
                    FavoriteOwnership {
                        id: f.id,
                        root,
                        effective_metadata,
                        excluded_roots: exclusions.into_iter().map(PathBuf::from).collect(),
                        owned_range,
                    },
                )
            })
            .collect(),
    }
}

impl MetadataOwnership {
    pub fn owner(&self, path: &str) -> Option<&FavoriteOwnership> {
        self.favorites
            .values()
            .find(|f| f.owned_range.as_ref().is_some_and(|r| r.contains(path)))
    }
    pub fn filter_set(&self, selected: Uuid) -> Vec<Uuid> {
        let Some(selected) = self.favorites.get(&selected) else {
            return Vec::new();
        };
        let mut out: Vec<_> = self
            .favorites
            .values()
            .filter(|f| f.effective_metadata && contains(&selected.root, &f.root))
            .map(|f| f.id)
            .collect();
        out.sort_by_key(Uuid::to_string);
        out
    }
    pub fn effective_ids(&self) -> Vec<Uuid> {
        let mut out: Vec<_> = self
            .favorites
            .values()
            .filter(|f| f.effective_metadata)
            .map(|f| f.id)
            .collect();
        out.sort_by_key(Uuid::to_string);
        out
    }
    /// Both snapshots participate, including disabled favorites and both sides of root changes.
    pub fn overlap_group(&self, next: &Self, seeds: &HashSet<Uuid>, all: bool) -> HashSet<Uuid> {
        if all {
            return self
                .favorites
                .keys()
                .chain(next.favorites.keys())
                .copied()
                .collect();
        }
        let mut group = seeds.clone();
        loop {
            let roots: Vec<_> = self
                .favorites
                .values()
                .chain(next.favorites.values())
                .filter(|f| group.contains(&f.id))
                .map(|f| f.root.clone())
                .collect();
            let before = group.len();
            for f in self.favorites.values().chain(next.favorites.values()) {
                if roots.iter().any(|r| overlaps(r, &f.root)) {
                    group.insert(f.id);
                }
            }
            if before == group.len() {
                return group;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fav(id: u128, root: &str, on: bool) -> FavoriteEntry {
        FavoriteEntry {
            id: Uuid::from_u128(id),
            name: String::new(),
            path: root.into(),
            auto_index_metadata: on,
            auto_index_structure: false,
            auto_index_similar: false,
            auto_index_thumbs: false,
        }
    }
    #[test]
    fn deepest_tie_exclusions_and_filter() {
        let fs = vec![
            fav(4, "C:/Photos", true),
            fav(3, "c:/photos/inner", true),
            fav(2, "C:/PHOTOS/INNER/", true),
            fav(1, "c:/photos2", true),
        ];
        let o = metadata_ownership(&fs, &["c:/photos/private".into()]);
        assert_eq!(o.owner("c:/photos/x.jpg").unwrap().id, fs[0].id);
        assert_eq!(o.owner("c:/photos/inner/x.jpg").unwrap().id, fs[2].id);
        assert!(o.owner("c:/photos/private/x.jpg").is_none());
        assert!(!o.favorites[&fs[1].id].effective_metadata);
        assert_eq!(o.filter_set(fs[0].id), vec![fs[2].id, fs[0].id]);
        let reversed: Vec<_> = fs.into_iter().rev().collect();
        assert_eq!(
            metadata_ownership(&reversed, &[])
                .owner("c:/photos/inner/x.jpg")
                .unwrap()
                .id,
            Uuid::from_u128(2)
        );
    }
    #[test]
    fn sql_ranges_match_scope_boundaries() {
        let r = OwnedRange {
            root: "c:/a".into(),
            exclusions: vec!["c:/a/b".into(), "c:/a/b/deeper".into(), "c:/a/z".into()],
        };
        let ranges = r.sql_ranges();
        for p in [
            "c:/a",
            "c:/a/aa.jpg",
            "c:/a/b",
            "c:/a/b/x.jpg",
            "c:/a/bb/x.jpg",
            "c:/a/z/x.jpg",
            "c:/ab/x.jpg",
        ] {
            assert_eq!(
                r.contains(p),
                ranges
                    .iter()
                    .any(|r| p >= r.start.as_str() && p < r.end.as_str()),
                "{p}"
            );
        }
        let blocked = OwnedRange {
            root: "c:/a/b".into(),
            exclusions: vec!["c:/a".into()],
        };
        assert!(blocked.sql_ranges().is_empty());
    }
    #[test]
    fn overlap_is_transitive_through_old_and_new_roots() {
        let old = metadata_ownership(
            &[
                fav(1, "c:/a/one", true),
                fav(2, "c:/a", false),
                fav(3, "c:/a/two", true),
                fav(4, "d:/unrelated", true),
            ],
            &[],
        );
        let new = metadata_ownership(
            &[
                fav(1, "e:/new", true),
                fav(2, "c:/a", false),
                fav(3, "c:/a/two", true),
                fav(4, "d:/unrelated", true),
            ],
            &[],
        );
        assert_eq!(
            old.overlap_group(&new, &HashSet::from([Uuid::from_u128(1)]), false),
            HashSet::from([1, 2, 3].map(Uuid::from_u128))
        );
        assert_eq!(old.overlap_group(&new, &HashSet::new(), true).len(), 4);
    }
}

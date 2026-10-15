//! Main-grid facet lineage. History stores routes, never filter snapshots.
//!
//! The active filter remains in Settings. This owner only retains live parent
//! filters for entered edges; adoption is the sole automatic mutation boundary.

use crate::settings::FacetFilter;
use std::path::Path;

/// Lexical Windows identity; unlike DB keys, this retains drive and UNC roots.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct FacetPathIdentity {
    root: String,
    components: Vec<String>,
}

impl FacetPathIdentity {
    pub(crate) fn new(path: &Path) -> Self {
        let mut text = path.to_string_lossy().replace('\\', "/").to_lowercase();
        if let Some(rest) = text.strip_prefix("//?/unc/") {
            text = format!("//{rest}");
        } else if let Some(rest) = text.strip_prefix("//?/") {
            text = rest.to_owned();
        }
        let (root, tail) = if let Some(rest) = text.strip_prefix("//") {
            let mut parts = rest.split('/').filter(|part| !part.is_empty());
            let server = parts.next().unwrap_or_default();
            let share = parts.next().unwrap_or_default();
            (
                format!("//{server}/{share}/"),
                parts.collect::<Vec<_>>().join("/"),
            )
        } else if text.as_bytes().get(1) == Some(&b':') {
            if text.as_bytes().get(2) == Some(&b'/') {
                (format!("{}/", &text[..2]), text[3..].to_owned())
            } else {
                (text[..2].to_owned(), text[2..].to_owned())
            }
        } else if let Some(rest) = text.strip_prefix('/') {
            ("/".to_owned(), rest.to_owned())
        } else {
            (String::new(), text)
        };
        let mut parts = Vec::new();
        for part in tail
            .split('/')
            .filter(|part| !part.is_empty() && *part != ".")
        {
            if part == ".." && parts.last().is_some_and(|last| *last != "..") {
                parts.pop();
            } else if part != ".." || root.is_empty() || !root.ends_with('/') {
                parts.push(part);
            }
        }
        Self {
            root,
            components: parts.into_iter().map(str::to_owned).collect(),
        }
    }

    pub(crate) fn contains(&self, other: &Self) -> bool {
        self.root == other.root && other.components.starts_with(&self.components)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum FacetScope {
    Path(FacetPathIdentity),
    /// Always the logical source archive, never a converted cache alias.
    Book {
        source: FacetPathIdentity,
        prefix: Vec<String>,
    },
    Rating {
        stars: u8,
    },
    Collection {
        id: String,
        entry: Option<String>,
    },
    Smart {
        id: String,
        position: String,
    },
    Search {
        identity: String,
    },
}

impl FacetScope {
    pub(crate) fn path(path: &Path) -> Self {
        Self::Path(FacetPathIdentity::new(path))
    }

    pub(crate) fn book(source: &Path, prefix: &str) -> Self {
        Self::Book {
            source: FacetPathIdentity::new(source),
            // ZIP member identity is case-sensitive and has component boundaries.
            prefix: prefix
                .split(['/', '\\'])
                .filter(|part| !part.is_empty())
                .map(str::to_owned)
                .collect(),
        }
    }

    pub(crate) fn contains(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::Path(parent), Self::Path(child)) => parent.contains(child),
            (Self::Path(parent), Self::Book { source, .. }) => parent.contains(source),
            (
                Self::Book { source, prefix },
                Self::Book {
                    source: child_source,
                    prefix: child_prefix,
                },
            ) => source == child_source && child_prefix.starts_with(prefix),
            _ => self == other,
        }
    }
}

/// Only lineage is copied into history. No historical filter values are held.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FacetRoute(pub(crate) Vec<FacetScope>);

impl FacetRoute {
    pub(crate) fn root(scope: FacetScope) -> Self {
        Self(vec![scope])
    }

    pub(crate) fn child(&self, scope: FacetScope) -> Self {
        let mut route = self.clone();
        route.0.push(scope);
        route
    }

    pub(crate) fn current(&self) -> Option<&FacetScope> {
        self.0.last()
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
struct SavedFacetFrame {
    /// Child's index in route, identifying the entered parent -> child edge.
    entered: usize,
    filter: FacetFilter,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FacetAdoption {
    pub(crate) stashed: usize,
    pub(crate) restored: usize,
    pub(crate) location_changed: bool,
}

/// A synthetic physical child represents the ZIP's effective root without an
/// explicit empty-prefix Book scope. Capture both roots at refresh adoption.
#[derive(Clone, Debug)]
pub(crate) struct ZipRefreshFacetRootAlias {
    pub(crate) scope: FacetScope,
    pub(crate) old_book_root: FacetScope,
    pub(crate) new_book_root: FacetScope,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub(crate) struct FacetNavigationState {
    route: FacetRoute,
    frames: Vec<SavedFacetFrame>,
}

impl FacetNavigationState {
    pub(crate) fn route(&self) -> &FacetRoute {
        &self.route
    }

    pub(crate) fn suppressed(&self) -> bool {
        self.frames.iter().any(|frame| frame.filter.is_active())
    }

    pub(crate) fn saved_frame_count(&self) -> usize {
        self.frames
            .iter()
            .filter(|frame| frame.filter.is_active())
            .count()
    }

    /// Re-enumeration keeps the live filter but rebuilds ancestry from the new tree.
    /// Frames are parent-owned, not index-owned; consumed frames are never recreated.
    pub(crate) fn reproject(&mut self, destination: FacetRoute) {
        self.reproject_with_zip_root_alias(destination, None);
    }

    pub(crate) fn reproject_zip_refresh(
        &mut self,
        destination: FacetRoute,
        actual_book: FacetScope,
        root_alias: Option<ZipRefreshFacetRootAlias>,
    ) {
        if let Some(alias) = root_alias.as_ref() {
            self.reproject_with_zip_root_alias(destination, Some((alias, &actual_book)));
        } else {
            self.reproject(destination);
        }
    }

    fn reproject_with_zip_root_alias(
        &mut self,
        destination: FacetRoute,
        alias: Option<(&ZipRefreshFacetRootAlias, &FacetScope)>,
    ) {
        let old = &self.route.0;
        let mut frames = self
            .frames
            .drain(..)
            .filter_map(|mut frame| {
                let parent = old.get(frame.entered.checked_sub(1)?)?;
                let index = if let Some((alias, actual)) = alias
                    && parent == &alias.scope
                {
                    // Root aliases are semantic Book parents, not stable-key
                    // parents. Retain only levels still in the effective stack.
                    if &alias.old_book_root == actual {
                        return None;
                    }
                    destination
                        .0
                        .iter()
                        .position(|scope| scope == &alias.old_book_root)
                        .or_else(|| {
                            (alias.old_book_root == alias.new_book_root)
                                .then(|| {
                                    destination.0.iter().position(|scope| scope == &alias.scope)
                                })
                                .flatten()
                        })?
                } else {
                    destination
                        .0
                        .iter()
                        .position(|scope| scope == parent)
                        .or_else(|| {
                            // A surviving Book parent can become the new
                            // effective root represented by the physical child.
                            alias
                                .filter(|(alias, _)| parent == &alias.new_book_root)
                                .and_then(|(alias, _)| {
                                    destination.0.iter().position(|scope| scope == &alias.scope)
                                })
                        })?
                };
                if index + 1 >= destination.0.len() {
                    return None;
                }
                frame.entered = index + 1;
                Some(frame)
            })
            .collect::<Vec<_>>();
        // Newly exposed wrapper parents have no prior live filter. Record their default
        // return value, without inventing a suppressed-filter badge or a consumed frame.
        for entered in 1..destination.0.len() {
            let parent = &destination.0[entered - 1];
            let was_known = if let Some((alias, _)) = alias {
                if parent == &alias.scope {
                    // Nonempty effective roots also have an explicit Book scope;
                    // that duplicate projection is not another return edge.
                    if destination.0[entered] == alias.new_book_root {
                        continue;
                    }
                    alias.old_book_root == alias.new_book_root || old.contains(&alias.new_book_root)
                } else {
                    old.contains(parent) || parent == &alias.old_book_root
                }
            } else {
                old.contains(parent)
            };
            if !was_known {
                frames.push(SavedFacetFrame {
                    entered,
                    filter: FacetFilter::default(),
                });
            }
        }
        frames.sort_by_key(|frame| frame.entered);
        self.frames = frames;
        self.route = destination;
    }

    /// A missing ZIP node returns to the actual surviving ancestor. Wrapper collapse
    /// may remove earlier route levels, so restoring by common-prefix depth can walk
    /// past that ancestor and overwrite its filter with an obsolete root's filter.
    pub(crate) fn adopt_refresh_ancestor(
        &mut self,
        destination: FacetRoute,
        actual_book: FacetScope,
        root_alias: Option<ZipRefreshFacetRootAlias>,
        active: &mut FacetFilter,
    ) -> FacetAdoption {
        let parent_index = self.route.0.iter().rposition(|scope| {
            let book_scope = root_alias
                .as_ref()
                .filter(|alias| &alias.scope == scope)
                .map_or(scope, |alias| &alias.old_book_root);
            matches!(book_scope, FacetScope::Book { .. }) && book_scope.contains(&actual_book)
        });
        let mut result = FacetAdoption {
            // Ancestor adoption means the requested node disappeared. Synthetic
            // scope keys can stay equal while their semantic Book prefix changes.
            location_changed: true,
            ..FacetAdoption::default()
        };
        if result.location_changed {
            active.place_keys.clear();
            for frame in &mut self.frames {
                frame.filter.place_keys.clear();
            }
        }
        if let Some(parent_index) = parent_index {
            if let Some(index) = self
                .frames
                .iter()
                .position(|frame| frame.entered == parent_index + 1)
            {
                *active = self.frames.remove(index).filter;
                result.restored = 1;
            }
        } else {
            // The old displayed root may itself have been a collapsed branch.
            // A different new branch has no old ancestor condition to inherit.
            *active = FacetFilter::default();
        }
        // A manually consumed ancestor frame stays absent; surviving outer frames
        // retain their ownership, while obsolete wrappers are discarded silently.
        self.reproject_zip_refresh(destination, actual_book, root_alias);
        result
    }

    #[cfg(test)]
    pub(crate) fn saved_filter(&self, index: usize) -> &FacetFilter {
        &self.frames[index].filter
    }

    /// Apply only after destination load and its ownership validators succeed.
    /// Same-route reloads cannot recreate a manually consumed frame.
    pub(crate) fn adopt(
        &mut self,
        destination: FacetRoute,
        active: &mut FacetFilter,
    ) -> FacetAdoption {
        if self.route == destination {
            return FacetAdoption::default();
        }
        let mut result = FacetAdoption {
            location_changed: self.route.current() != destination.current(),
            ..FacetAdoption::default()
        };
        if result.location_changed {
            active.place_keys.clear();
            for frame in &mut self.frames {
                frame.filter.place_keys.clear();
            }
        }
        let common = self
            .route
            .0
            .iter()
            .zip(&destination.0)
            .take_while(|(source, target)| source == target)
            .count();
        while self
            .frames
            .last()
            .is_some_and(|frame| frame.entered >= common)
        {
            *active = self.frames.pop().unwrap().filter;
            result.restored += 1;
        }
        // Index zero is a root, not an entered edge. Independent navigation
        // inherits the filter restored on exit instead of inventing a stash.
        for entered in common.max(1)..destination.0.len() {
            if active.is_active() {
                self.frames.push(SavedFacetFrame {
                    entered,
                    filter: std::mem::take(active),
                });
                result.stashed += 1;
            }
        }
        self.route = destination;
        result
    }

    /// Badge action: consume only the latest live frame; lineage stays intact.
    /// The caller synchronizes name runtime and persists Settings as before.
    pub(crate) fn restore_latest(&mut self, active: &mut FacetFilter) -> bool {
        let Some(index) = self
            .frames
            .iter()
            .rposition(|frame| frame.filter.is_active())
        else {
            return false;
        };
        *active = self.frames.remove(index).filter;
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn filter(ext: &str) -> FacetFilter {
        let mut value = FacetFilter::default();
        value.exts.insert(ext.to_owned());
        value
    }

    fn rating() -> FacetRoute {
        FacetRoute::root(FacetScope::Rating { stars: 3 })
    }

    fn book() -> FacetScope {
        FacetScope::book(Path::new(r"C:\books\comic.zip"), "")
    }

    #[test]
    fn section1353_reproject_preserves_frames_by_parent_and_new_parent_defaults() {
        let root = FacetRoute::root(book());
        let leaf = FacetScope::book(Path::new(r"C:\books\comic.zip"), "A/sub");
        let parent = FacetScope::book(Path::new(r"C:\books\comic.zip"), "A");
        let mut state = FacetNavigationState::default();
        let mut active = filter("zip");
        state.adopt(root.clone(), &mut active);
        state.adopt(root.child(leaf.clone()), &mut active);
        active = filter("jpg");
        state.reproject(root.child(parent.clone()).child(leaf));
        assert_eq!(active, filter("jpg"));
        assert_eq!(state.saved_frame_count(), 1);
        let mut manually_restored = state.clone();
        let mut manual_filter = active.clone();
        assert!(manually_restored.restore_latest(&mut manual_filter));
        assert_eq!(manual_filter, filter("zip"));
        assert!(!manually_restored.restore_latest(&mut manual_filter));
        assert!(!manually_restored.suppressed());
        manually_restored.adopt(root.child(parent.clone()), &mut manual_filter);
        assert!(
            !manual_filter.is_active(),
            "manual restoration must keep the new parent's empty return frame"
        );
        manually_restored.adopt(root.clone(), &mut manual_filter);
        assert!(
            !manual_filter.is_active(),
            "the manually consumed root frame must stay absent on Backspace"
        );
        state.adopt(root.child(parent), &mut active);
        assert!(
            !active.is_active(),
            "unseen parent has no inherited child facet"
        );
        state.adopt(root, &mut active);
        assert_eq!(active, filter("zip"));
    }

    #[test]
    fn section1353_ancestor_refresh_restores_scope_when_root_collapses() {
        for manual in [false, true] {
            let outer = rating();
            let root = outer.child(book());
            let parent = FacetScope::book(Path::new(r"C:\books\comic.zip"), "A");
            let leaf = FacetScope::book(Path::new(r"C:\books\comic.zip"), "A/sub");
            let mut state = FacetNavigationState::default();
            let mut active = filter("cbz");
            state.adopt(outer.clone(), &mut active);
            state.adopt(root.clone(), &mut active);
            active = filter("zip");
            state.adopt(root.child(parent.clone()), &mut active);
            active = filter("png");
            state.adopt(root.child(parent.clone()).child(leaf), &mut active);
            if manual {
                assert!(state.restore_latest(&mut active));
                assert_eq!(active, filter("png"));
            }
            active = filter("jpg");
            active.place_keys.insert("old-child-row".to_owned());
            let destination = outer.child(parent);
            let result = state.adopt_refresh_ancestor(
                destination.clone(),
                destination.current().unwrap().clone(),
                None,
                &mut active,
            );
            assert_eq!(result.restored, usize::from(!manual));
            assert!(result.location_changed);
            assert_eq!(active, filter(if manual { "jpg" } else { "png" }));
            assert_eq!(state.route(), &destination);
            assert_eq!(state.saved_frame_count(), 1);
            assert_eq!(state.saved_filter(0), &filter("cbz"));
            state.adopt(outer, &mut active);
            assert_eq!(active, filter("cbz"));
            assert!(!state.suppressed());
        }
    }

    #[test]
    fn section1353_ancestor_refresh_uses_nearest_old_scope_for_new_collapsed_branch() {
        for (prefix, expected) in [("A/new", "png"), ("B/pages", "zip")] {
            let outer = rating();
            let root = outer.child(book());
            let parent = FacetScope::book(Path::new(r"C:\books\comic.zip"), "A");
            let leaf = FacetScope::book(Path::new(r"C:\books\comic.zip"), "A/sub");
            let actual = FacetScope::book(Path::new(r"C:\books\comic.zip"), prefix);
            let mut state = FacetNavigationState::default();
            let mut active = filter("cbz");
            state.adopt(outer.clone(), &mut active);
            state.adopt(root.clone(), &mut active);
            active = filter("zip");
            state.adopt(root.child(parent.clone()), &mut active);
            active = filter("png");
            state.adopt(root.child(parent).child(leaf), &mut active);
            active = filter("jpg");
            state.adopt_refresh_ancestor(outer.child(actual.clone()), actual, None, &mut active);
            assert_eq!(active, filter(expected));
            assert_eq!(state.saved_frame_count(), 1);
            state.adopt(outer, &mut active);
            assert_eq!(active, filter("cbz"));
        }

        // Both trees collapse at root, but into unrelated branches. The old A
        // filter cannot become B's condition, and the outer return is unchanged.
        let outer = rating();
        let old_root = FacetScope::book(Path::new(r"C:\books\comic.zip"), "A");
        let actual = FacetScope::book(Path::new(r"C:\books\comic.zip"), "B");
        let mut state = FacetNavigationState::default();
        let mut active = filter("cbz");
        state.adopt(outer.clone(), &mut active);
        state.adopt(outer.child(old_root), &mut active);
        active = filter("jpg");
        state.adopt_refresh_ancestor(outer.child(actual.clone()), actual, None, &mut active);
        assert!(!active.is_active());
        assert_eq!(state.saved_frame_count(), 1);
        state.adopt(outer, &mut active);
        assert_eq!(active, filter("cbz"));
    }

    #[test]
    fn section1353_ancestor_refresh_restores_synthetic_zip_root_frame_only() {
        for (outer, child) in [
            (
                FacetScope::Smart {
                    id: "smart".into(),
                    position: "root".into(),
                },
                FacetScope::Smart {
                    id: "smart".into(),
                    position: "book.zip".into(),
                },
            ),
            (
                FacetScope::Collection {
                    id: "collection".into(),
                    entry: None,
                },
                FacetScope::Collection {
                    id: "collection".into(),
                    entry: Some("book".into()),
                },
            ),
        ] {
            for manual in [false, true] {
                let outer = FacetRoute::root(outer.clone());
                let zip_root = outer.child(child.clone());
                let nested =
                    zip_root.child(FacetScope::book(Path::new(r"C:\books\comic.zip"), "A"));
                let mut state = FacetNavigationState::default();
                let mut active = filter("zip");
                state.adopt(outer.clone(), &mut active);
                state.adopt(zip_root.clone(), &mut active);
                active = filter("png");
                state.adopt(nested, &mut active);
                if manual {
                    assert!(state.restore_latest(&mut active));
                    assert_eq!(active, filter("png"));
                }
                active = filter("jpg");
                let actual = book();
                let alias = ZipRefreshFacetRootAlias {
                    scope: child.clone(),
                    old_book_root: actual.clone(),
                    new_book_root: actual.clone(),
                };
                let result = state.adopt_refresh_ancestor(
                    zip_root.clone(),
                    actual,
                    Some(alias),
                    &mut active,
                );
                assert_eq!(result.restored, usize::from(!manual));
                assert_eq!(active, filter(if manual { "jpg" } else { "png" }));
                assert_eq!(state.route(), &zip_root);
                assert_eq!(state.saved_frame_count(), 1);
                assert_eq!(state.saved_filter(0), &filter("zip"));
                state.adopt(outer, &mut active);
                assert_eq!(active, filter("zip"));
                assert!(!state.suppressed());
            }
        }
    }

    #[test]
    fn section1353_synthetic_root_alias_tracks_semantic_prefix_and_surviving_parents() {
        let source = Path::new(r"C:\books\comic.zip");
        let scope = |prefix: &str| FacetScope::book(source, prefix);
        for (old_root, new_root, actual, saved_parent_survives, expected) in [
            ("", "B", "B", false, "png"),
            ("A", "B", "B", false, ""),
            ("A", "", "", false, ""),
            ("", "A/new", "A/new", false, "gif"),
            ("", "", "A/new", true, "gif"),
        ] {
            for manual in [false, true] {
                let outer_scope = FacetScope::Smart {
                    id: "smart".into(),
                    position: "root".into(),
                };
                let child_scope = FacetScope::Smart {
                    id: "smart".into(),
                    position: "book.zip".into(),
                };
                let outer = FacetRoute::root(outer_scope);
                let root = outer.child(child_scope.clone());
                let mut state = FacetNavigationState::default();
                let mut active = filter("zip");
                state.adopt(outer.clone(), &mut active);
                state.adopt(root.clone(), &mut active);
                active = filter("png");
                let current = if expected == "gif" {
                    state.adopt(root.child(scope("A")), &mut active);
                    active = filter("gif");
                    root.child(scope("A")).child(scope("A/sub"))
                } else {
                    root.child(scope(if old_root.is_empty() {
                        "A/sub"
                    } else {
                        "A/sub/deep"
                    }))
                };
                state.adopt(current, &mut active);
                if manual {
                    assert!(state.restore_latest(&mut active));
                }
                active = filter("jpg");
                active.place_keys.insert("old-page".into());
                let mut destination = root.clone();
                if actual != new_root {
                    destination = destination.child(scope(actual));
                }
                let alias = ZipRefreshFacetRootAlias {
                    scope: child_scope,
                    old_book_root: scope(old_root),
                    new_book_root: scope(new_root),
                };
                let result = state.adopt_refresh_ancestor(
                    destination.clone(),
                    scope(actual),
                    Some(alias),
                    &mut active,
                );
                assert!(result.location_changed);
                let expected = if manual && !expected.is_empty() {
                    "jpg"
                } else {
                    expected
                };
                assert_eq!(
                    active,
                    if expected.is_empty() {
                        FacetFilter::default()
                    } else {
                        filter(expected)
                    }
                );
                assert_eq!(state.route(), &destination);
                assert_eq!(
                    state.saved_frame_count(),
                    1 + usize::from(saved_parent_survives)
                );
                assert_eq!(state.saved_filter(0), &filter("zip"));
                if saved_parent_survives {
                    state.adopt(root, &mut active);
                    assert_eq!(active, filter("png"));
                }
                state.adopt(outer, &mut active);
                assert_eq!(active, filter("zip"));
            }
        }

        // A collapsed old root can change semantic position while the route key
        // stays exactly the same; this still clears location-bound conditions.
        let outer = rating();
        let child = FacetScope::Collection {
            id: "collection".into(),
            entry: Some("book".into()),
        };
        let destination = outer.child(child.clone());
        let mut state = FacetNavigationState::default();
        let mut active = filter("zip");
        state.adopt(outer, &mut active);
        state.adopt(destination.clone(), &mut active);
        active = filter("png");
        active.place_keys.insert("old-page".into());
        let result = state.adopt_refresh_ancestor(
            destination.clone(),
            scope(""),
            Some(ZipRefreshFacetRootAlias {
                scope: child,
                old_book_root: scope("A"),
                new_book_root: scope(""),
            }),
            &mut active,
        );
        assert!(result.location_changed);
        assert!(!active.is_active());
        assert_eq!(state.saved_frame_count(), 1);

        // An old collapsed root can become an explicit effective parent after a
        // wrapper is exposed. Its frame moves to that Book parent, not to the
        // newly exposed empty root represented by the same synthetic key.
        let outer = rating();
        let child = FacetScope::Smart {
            id: "smart".into(),
            position: "book.zip".into(),
        };
        let root = outer.child(child.clone());
        let mut state = FacetNavigationState::default();
        let mut active = filter("zip");
        state.adopt(outer.clone(), &mut active);
        state.adopt(root.clone(), &mut active);
        active = filter("png");
        state.adopt(root.child(scope("A/sub")), &mut active);
        active = filter("gif");
        state.adopt(
            root.child(scope("A/sub")).child(scope("A/sub/deep")),
            &mut active,
        );
        active = filter("jpg");
        let destination = root.child(scope("A")).child(scope("A/sub/new"));
        state.adopt_refresh_ancestor(
            destination,
            scope("A/sub/new"),
            Some(ZipRefreshFacetRootAlias {
                scope: child,
                old_book_root: scope("A"),
                new_book_root: scope(""),
            }),
            &mut active,
        );
        assert_eq!(active, filter("gif"));
        assert_eq!(state.saved_frame_count(), 2);
        state.adopt(root.child(scope("A")), &mut active);
        assert_eq!(active, filter("png"));
        state.adopt(root, &mut active);
        assert!(
            !active.is_active(),
            "newly exposed empty root has a default return"
        );
        state.adopt(outer, &mut active);
        assert_eq!(active, filter("zip"));
    }

    #[test]
    fn section1353_preserved_node_drops_synthetic_wrapper_frames_without_touching_active() {
        let source = Path::new(r"C:\books\comic.zip");
        let actual = FacetScope::book(source, "A/sub");
        let parent = FacetScope::book(source, "A");
        for synthetic in [false, true] {
            let outer = rating();
            let root_scope = if synthetic {
                FacetScope::Smart {
                    id: "smart".into(),
                    position: "book.zip".into(),
                }
            } else {
                book()
            };
            let root = outer.child(root_scope.clone());
            let mut state = FacetNavigationState::default();
            let mut active = filter("zip");
            state.adopt(outer.clone(), &mut active);
            state.adopt(root.clone(), &mut active);
            active = filter("png");
            state.adopt(root.child(parent.clone()), &mut active);
            active = filter("gif");
            state.adopt(
                root.child(FacetScope::book(source, "A"))
                    .child(actual.clone()),
                &mut active,
            );
            active = filter("jpg");
            active.place_keys.insert("same-page".into());
            let before = active.clone();
            let destination = if synthetic {
                root.clone()
            } else {
                outer.child(actual.clone())
            };
            let alias = synthetic.then(|| ZipRefreshFacetRootAlias {
                scope: root_scope,
                old_book_root: book(),
                new_book_root: actual.clone(),
            });
            state.reproject_zip_refresh(destination.clone(), actual.clone(), alias);
            assert_eq!(active, before);
            assert_eq!(state.route(), &destination);
            assert_eq!(state.saved_frame_count(), 1);
            assert_eq!(state.saved_filter(0), &filter("zip"));
            state.adopt(outer, &mut active);
            assert_eq!(active, filter("zip"));
        }
    }

    #[test]
    fn section1353_preserved_book_parent_becomes_synthetic_root_without_losing_its_frame() {
        let source = Path::new(r"C:\books\comic.zip");
        let parent = FacetScope::book(source, "A");
        let actual = FacetScope::book(source, "A/sub");
        for manual in [false, true] {
            let outer = rating();
            let child = FacetScope::Smart {
                id: "smart".into(),
                position: "book.zip".into(),
            };
            let root = outer.child(child.clone());
            let mut state = FacetNavigationState::default();
            let mut active = filter("zip");
            state.adopt(outer.clone(), &mut active);
            state.adopt(root.clone(), &mut active);
            active = filter("png");
            state.adopt(root.child(parent.clone()), &mut active);
            active = filter("gif");
            state.adopt(
                root.child(parent.clone()).child(actual.clone()),
                &mut active,
            );
            if manual {
                assert!(state.restore_latest(&mut active));
            }
            active = filter("jpg");
            active.place_keys.insert("same-page".into());
            let before = active.clone();
            let destination = root.child(actual.clone());
            state.reproject_zip_refresh(
                destination.clone(),
                actual.clone(),
                Some(ZipRefreshFacetRootAlias {
                    scope: child,
                    old_book_root: book(),
                    new_book_root: parent.clone(),
                }),
            );
            assert_eq!(active, before);
            assert_eq!(state.route(), &destination);
            assert_eq!(state.saved_frame_count(), 1 + usize::from(!manual));
            state.adopt(root, &mut active);
            assert_eq!(active, filter(if manual { "jpg" } else { "gif" }));
            state.adopt(outer, &mut active);
            assert_eq!(active, filter("zip"));
            assert!(!state.suppressed());
        }
    }

    #[test]
    fn section1353_reproject_collapse_does_not_recreate_consumed_parent_frame() {
        let root = FacetRoute::root(book());
        let parent = FacetScope::book(Path::new(r"C:\books\comic.zip"), "A");
        let leaf = FacetScope::book(Path::new(r"C:\books\comic.zip"), "A/sub");
        let mut state = FacetNavigationState::default();
        let mut active = filter("zip");
        state.adopt(root.clone(), &mut active);
        state.adopt(root.child(parent), &mut active);
        assert!(state.restore_latest(&mut active));
        let manual = active.clone();
        state.reproject(root.child(leaf));
        assert_eq!(active, manual);
        assert_eq!(state.saved_frame_count(), 0);
        assert!(!state.suppressed());
    }

    #[test]
    fn rating_zip_book_back_forward_restashes_live_parent() {
        let parent = rating();
        let child = parent.child(book());
        let mut state = FacetNavigationState::default();
        let mut active = filter("zip");
        state.adopt(parent.clone(), &mut active);
        assert_eq!(state.adopt(child.clone(), &mut active).stashed, 1);
        assert!(!active.is_active());
        // BS, then toolbar back to the child, forward to parent, back to child.
        assert_eq!(state.adopt(parent.clone(), &mut active).restored, 1);
        assert_eq!(active, filter("zip"));
        state.adopt(child.clone(), &mut active);
        state.adopt(parent, &mut active);
        state.adopt(child, &mut active);
        assert!(!active.is_active());
        assert_eq!(state.saved_frame_count(), 1);
    }

    #[test]
    fn parent_edit_is_used_on_history_reentry() {
        let parent = rating();
        let child = parent.child(book());
        let mut state = FacetNavigationState::default();
        let mut active = filter("zip");
        state.adopt(parent.clone(), &mut active);
        state.adopt(child.clone(), &mut active);
        state.adopt(parent.clone(), &mut active);
        active = filter("pdf");
        state.adopt(child, &mut active);
        state.adopt(parent, &mut active);
        assert_eq!(active, filter("pdf"));
    }

    #[test]
    fn manual_restore_consumes_once_without_route_change_or_reload_restash() {
        let parent = rating();
        let child = parent.child(book());
        let mut state = FacetNavigationState::default();
        let mut active = filter("zip");
        state.adopt(parent.clone(), &mut active);
        state.adopt(child.clone(), &mut active);
        assert!(state.restore_latest(&mut active));
        assert!(!state.restore_latest(&mut active));
        assert_eq!(state.route(), &child);
        assert_eq!(state.adopt(child, &mut active), FacetAdoption::default());
        active = filter("jpg");
        state.adopt(parent, &mut active);
        assert_eq!(active, filter("jpg"));
        assert!(!state.suppressed());
    }

    #[test]
    fn independent_location_inherits_restored_parent_not_child_edit() {
        let parent = rating();
        let mut state = FacetNavigationState::default();
        let mut active = filter("zip");
        state.adopt(parent.clone(), &mut active);
        state.adopt(parent.child(book()), &mut active);
        active = filter("jpg");
        state.adopt(
            FacetRoute::root(FacetScope::path(Path::new(r"D:\elsewhere"))),
            &mut active,
        );
        assert_eq!(active, filter("zip"));
        assert!(!state.suppressed());
    }

    #[test]
    fn empty_parent_has_no_frame_and_keeps_child_edits_on_exit() {
        let parent = rating();
        let mut state = FacetNavigationState::default();
        let mut active = FacetFilter::default();
        state.adopt(parent.clone(), &mut active);
        state.adopt(parent.child(book()), &mut active);
        assert!(!state.suppressed());
        active = filter("png");
        state.adopt(parent, &mut active);
        assert_eq!(active, filter("png"));
    }

    #[test]
    fn nested_edges_restore_inner_then_outer() {
        let parent = rating();
        let child = parent.child(book());
        let nested = child.child(FacetScope::book(
            Path::new(r"C:\books\comic.zip"),
            "chapter/pages",
        ));
        let mut state = FacetNavigationState::default();
        let mut active = filter("zip");
        state.adopt(parent.clone(), &mut active);
        state.adopt(child.clone(), &mut active);
        active = filter("jpg");
        state.adopt(nested, &mut active);
        assert_eq!(state.saved_frame_count(), 2);
        assert_eq!(state.adopt(parent, &mut active).restored, 2);
        assert_eq!(active, filter("zip"));
    }

    #[test]
    fn sibling_edge_restores_then_stashes_same_live_parent() {
        let parent = rating();
        let mut state = FacetNavigationState::default();
        let mut active = filter("zip");
        state.adopt(parent.clone(), &mut active);
        state.adopt(parent.child(book()), &mut active);
        active = filter("jpg");
        let adopted = state.adopt(
            parent.child(FacetScope::book(Path::new(r"C:\other.zip"), "")),
            &mut active,
        );
        assert_eq!((adopted.restored, adopted.stashed), (1, 1));
        state.adopt(parent, &mut active);
        assert_eq!(active, filter("zip"));
    }

    #[test]
    fn location_change_clears_active_and_saved_place_conditions() {
        let parent = rating();
        let mut state = FacetNavigationState::default();
        let mut active = filter("zip");
        state.adopt(parent.clone(), &mut active);
        active.place_keys.insert("old-row".to_owned());
        state.adopt(parent.child(book()), &mut active);
        active.place_keys.insert("child-row".to_owned());
        state.adopt(parent, &mut active);
        assert_eq!(active, filter("zip"));
    }

    #[test]
    fn place_only_filter_is_not_stashed_for_another_location() {
        let parent = rating();
        let mut state = FacetNavigationState::default();
        let mut active = FacetFilter::default();
        state.adopt(parent.clone(), &mut active);
        active.place_keys.insert("row".to_owned());
        state.adopt(parent.child(book()), &mut active);
        assert!(!active.is_active());
        assert!(!state.suppressed());
    }

    #[test]
    fn path_identity_preserves_roots_and_component_boundaries() {
        let path = |value| FacetPathIdentity::new(Path::new(value));
        assert_eq!(path(r"C:\Books\"), path("c:/books"));
        assert!(path(r"C:\").contains(&path(r"C:\books")));
        assert!(path(r"C:\book").contains(&path(r"C:\book\chapter")));
        assert!(!path(r"C:\book").contains(&path(r"C:\book2")));
        assert!(!path(r"C:\book").contains(&path(r"D:\book")));
        assert_ne!(path(r"C:book"), path(r"C:\book"));
        assert!(!path("C:").contains(&path(r"C:\book")));
        assert_eq!(path(r"C:\book\.\chapter\.."), path(r"C:\book"));
        assert_eq!(path(r"\\Server\Share\"), path("//server/share"));
        assert!(path(r"\\Server\Share\").contains(&path(r"\\server\share\book")));
        assert!(!path(r"\\server\share\").contains(&path(r"\\server\share2\book")));
        assert_eq!(path(r"\\?\C:\Books"), path(r"C:\books"));
        assert_eq!(
            path(r"\\?\UNC\Server\Share\book"),
            path(r"\\server\share\book")
        );
    }

    #[test]
    fn book_prefix_uses_components_and_logical_archive_identity() {
        let parent = FacetScope::book(Path::new(r"C:\Comic.ZIP"), "chapter/");
        let child = FacetScope::book(Path::new("c:/comic.zip"), "chapter/pages");
        assert!(parent.contains(&child));
        assert!(!parent.contains(&FacetScope::book(Path::new(r"C:\comic.zip"), "chapter2")));
        assert!(!parent.contains(&FacetScope::book(Path::new(r"C:\cache.zip"), "chapter")));
    }

    #[test]
    fn logical_alias_and_case_reload_are_noops() {
        let mut state = FacetNavigationState::default();
        let mut active = filter("zip");
        let source = rating();
        state.adopt(source.clone(), &mut active);
        state.adopt(source.child(book()), &mut active);
        // Both Direct and CachedZip adapters supply the original source path.
        let alias = source.child(FacetScope::book(Path::new("c:/BOOKS/COMIC.ZIP"), "/"));
        assert_eq!(state.adopt(alias, &mut active), FacetAdoption::default());
        assert_eq!(state.saved_frame_count(), 1);
    }

    #[test]
    fn same_archive_different_origins_do_not_share_filter_snapshots() {
        let parent = rating();
        let other = FacetRoute::root(FacetScope::Collection {
            id: "collection-1".to_owned(),
            entry: Some("entry-7".to_owned()),
        });
        let mut state = FacetNavigationState::default();
        let mut active = filter("zip");
        state.adopt(parent.clone(), &mut active);
        state.adopt(parent.child(book()), &mut active);
        state.adopt(other.clone(), &mut active);
        active = filter("pdf");
        state.adopt(other.child(book()), &mut active);
        state.adopt(other, &mut active);
        assert_eq!(active, filter("pdf"));
    }
}

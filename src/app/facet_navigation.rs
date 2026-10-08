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
        !self.frames.is_empty()
    }

    pub(crate) fn saved_frame_count(&self) -> usize {
        self.frames.len()
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
            self.frames.retain(|frame| frame.filter.is_active());
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
        let Some(frame) = self.frames.pop() else {
            return false;
        };
        *active = frame.filter;
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

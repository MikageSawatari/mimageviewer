//! Accepted-listing sort policy shared by controls and row producers.

use super::book_materialization::ProjectionRecipe;
use super::top_level_grid_view::TopLevelGridSurface;
use super::{App, GridSortLockReason};
use crate::settings::SortOrder;

pub(crate) const DURATION_SORT_UNSUPPORTED: &str =
    "この一覧では長さ順を使えません。詳細表示の長さ列で並べ替えられます";

pub(crate) struct ListingSortContext<'a> {
    pub(crate) surface: &'a TopLevelGridSurface,
    pub(crate) recipe: &'a ProjectionRecipe,
    pub(crate) page_order_locked: bool,
    pub(crate) details_sort_active: bool,
    pub(crate) collection_root: bool,
    pub(crate) existing_lock: Option<GridSortLockReason>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct ListingSortCapabilities {
    pub(crate) duration_allowed: bool,
    pub(crate) sort_locked_reason: Option<GridSortLockReason>,
    rating_disabled_reason: Option<&'static str>,
}

impl ListingSortCapabilities {
    pub(crate) fn disabled_reason(self, order: SortOrder) -> Option<&'static str> {
        if let Some(reason) = self.sort_locked_reason {
            Some(reason.tooltip())
        } else if order.is_duration() && !self.duration_allowed {
            Some(DURATION_SORT_UNSUPPORTED)
        } else if order.is_rating() {
            self.rating_disabled_reason
        } else {
            None
        }
    }

    /// Header sorting can own presentation without changing the underlying rows.
    /// Only unsupported duration requests are normalized at producer boundaries.
    pub(crate) fn effective_order(self, order: SortOrder) -> SortOrder {
        if !self.duration_allowed {
            without_duration_order(order)
        } else {
            order
        }
    }
}

/// Captured requests for aggregate listings and Remote cannot acquire duration facts.
/// Keep their existing order policy, normalizing only the unsupported new variants.
pub(crate) fn without_duration_order(order: SortOrder) -> SortOrder {
    if order.is_duration() {
        SortOrder::FileName
    } else {
        order
    }
}

pub(crate) fn without_duration_request(
    request: crate::rating_sort::ListingOrderRequest,
) -> crate::rating_sort::ListingOrderRequest {
    match request {
        crate::rating_sort::ListingOrderRequest::Standard(order) => {
            crate::rating_sort::ListingOrderRequest::Standard(without_duration_order(order))
        }
        request => request,
    }
}

pub(crate) fn listing_sort_capabilities(
    context: ListingSortContext<'_>,
) -> ListingSortCapabilities {
    let duration_allowed = matches!(context.surface, TopLevelGridSurface::Folder)
        && matches!(context.recipe, ProjectionRecipe::OrdinaryList)
        && !context.page_order_locked
        && !context.collection_root;
    let sort_locked_reason = context.existing_lock.or_else(|| {
        if context.page_order_locked {
            Some(GridSortLockReason::PageOrderFixed)
        } else if context.details_sort_active {
            Some(GridSortLockReason::DetailsHeaderSort)
        } else {
            None
        }
    });
    let rating_disabled_reason = if context.collection_root {
        Some("コレクションでは評価順を使えません。手動の並べ替えで順番を変えられます")
    } else if matches!(context.surface, TopLevelGridSurface::Bookmarks) {
        Some("ブックマーク一覧では評価順を使えません")
    } else {
        None
    };
    ListingSortCapabilities {
        duration_allowed,
        sort_locked_reason,
        rating_disabled_reason,
    }
}

impl App {
    pub(crate) fn collection_sort_capabilities(&self) -> ListingSortCapabilities {
        listing_sort_capabilities(ListingSortContext {
            surface: self.top_level_grid_view.surface(),
            recipe: &ProjectionRecipe::OrdinaryList,
            page_order_locked: false,
            details_sort_active: false,
            collection_root: true,
            existing_lock: None,
        })
    }

    pub(crate) fn listing_sort_capabilities(&self) -> ListingSortCapabilities {
        listing_sort_capabilities(ListingSortContext {
            surface: self.top_level_grid_view.surface(),
            recipe: &self.projection_recipe,
            page_order_locked: self.page_order_locked_for_current_view(),
            details_sort_active: self.details_header_sort_active(),
            collection_root: self.collection_grid_root_order().is_some(),
            existing_lock: self.grid_sort_lock_reason(),
        })
    }

    pub(crate) fn effective_listing_sort_order(&self, order: SortOrder) -> SortOrder {
        self.listing_sort_capabilities().effective_order(order)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn capabilities(surface: &TopLevelGridSurface) -> ListingSortCapabilities {
        listing_sort_capabilities(ListingSortContext {
            surface,
            recipe: &ProjectionRecipe::OrdinaryList,
            page_order_locked: false,
            details_sort_active: false,
            collection_root: false,
            existing_lock: None,
        })
    }

    #[test]
    fn duration_sort_is_available_only_for_ordinary_physical_folders() {
        use super::super::top_level_grid_view::{
            CollectionGridIdentity, SmartFolderPosition, SmartFolderViewState, TopLevelSearchView,
        };
        let mut smart_child = SmartFolderViewState::root(uuid::Uuid::nil(), Vec::new());
        smart_child.position = SmartFolderPosition::Scoped {
            entry_index: 0,
            entry_root: "C:/media".into(),
            current: "C:/media/child".into(),
            current_kind: super::super::smart_folder::SmartChildKind::Folder,
            back_stack: Vec::new(),
        };
        for surface in [
            TopLevelGridSurface::Folder,
            TopLevelGridSurface::SubfolderExpansion,
            TopLevelGridSurface::Bookmarks,
            TopLevelGridSurface::Rating { stars: 3 },
            TopLevelGridSurface::Snapshot,
            TopLevelGridSurface::DriveList,
            TopLevelGridSurface::Search(TopLevelSearchView::Global),
            TopLevelGridSurface::SmartFolder(SmartFolderViewState::root(
                uuid::Uuid::nil(),
                Vec::new(),
            )),
            TopLevelGridSurface::SmartFolder(smart_child),
            TopLevelGridSurface::Collection(CollectionGridIdentity {
                collection_id: crate::collection_store::CollectionId::from_uuid(uuid::Uuid::nil()),
            }),
        ] {
            let result = capabilities(&surface);
            let supported = matches!(surface, TopLevelGridSurface::Folder);
            assert_eq!(result.duration_allowed, supported);
            assert_eq!(
                result.effective_order(SortOrder::DurationDesc),
                if supported {
                    SortOrder::DurationDesc
                } else {
                    SortOrder::FileName
                }
            );
            assert_eq!(
                result.effective_order(SortOrder::SizeAsc),
                SortOrder::SizeAsc
            );
        }
    }

    #[test]
    fn book_and_extracted_rows_do_not_gain_duration_sort_from_a_folder_surface() {
        use super::super::book_materialization::{BookScope, ExtractedSurface};
        for recipe in [
            ProjectionRecipe::Book(BookScope::ImageFolder("C:/book".into())),
            ProjectionRecipe::ExtractedResult(ExtractedSurface::SmartFolder),
        ] {
            let result = listing_sort_capabilities(ListingSortContext {
                surface: &TopLevelGridSurface::Folder,
                recipe: &recipe,
                page_order_locked: false,
                details_sort_active: false,
                collection_root: false,
                existing_lock: None,
            });
            assert!(!result.duration_allowed);
            assert_eq!(
                result.disabled_reason(SortOrder::DurationAsc),
                Some(DURATION_SORT_UNSUPPORTED)
            );
        }
    }

    #[test]
    fn existing_sort_locks_and_collection_rating_reason_are_preserved() {
        let result = listing_sort_capabilities(ListingSortContext {
            surface: &TopLevelGridSurface::Folder,
            recipe: &ProjectionRecipe::OrdinaryList,
            page_order_locked: true,
            details_sort_active: true,
            collection_root: false,
            existing_lock: Some(GridSortLockReason::FixedOrder),
        });
        assert_eq!(
            result.sort_locked_reason,
            Some(GridSortLockReason::FixedOrder)
        );
        assert!(!result.duration_allowed);
        assert_eq!(
            result.disabled_reason(SortOrder::SizeAsc),
            Some(GridSortLockReason::FixedOrder.tooltip())
        );
        let mut result = capabilities(&TopLevelGridSurface::Folder);
        result.sort_locked_reason = Some(GridSortLockReason::DetailsHeaderSort);
        assert!(
            result.duration_allowed,
            "a header sort must not alter row producer capabilities"
        );
        assert_eq!(
            result.effective_order(SortOrder::DurationAsc),
            SortOrder::DurationAsc
        );
        let result = listing_sort_capabilities(ListingSortContext {
            surface: &TopLevelGridSurface::Folder,
            recipe: &ProjectionRecipe::OrdinaryList,
            page_order_locked: false,
            details_sort_active: false,
            collection_root: true,
            existing_lock: None,
        });
        assert_eq!(
            result.disabled_reason(SortOrder::RatingDesc),
            Some("コレクションでは評価順を使えません。手動の並べ替えで順番を変えられます")
        );
    }

    #[test]
    fn unsupported_duration_requests_preserve_existing_rating_policy() {
        use crate::rating_sort::ListingOrderRequest;
        for order in SortOrder::all() {
            let request = ListingOrderRequest::from_sort(*order, Default::default());
            let effective = without_duration_request(request);
            if order.is_duration() {
                assert_eq!(
                    effective,
                    ListingOrderRequest::Standard(SortOrder::FileName)
                );
            } else {
                assert_eq!(effective, request);
            }
        }
    }

    #[test]
    fn bookmark_rating_reason_is_preserved() {
        assert_eq!(
            capabilities(&TopLevelGridSurface::Bookmarks).disabled_reason(SortOrder::RatingAsc),
            Some("ブックマーク一覧では評価順を使えません")
        );
        assert_eq!(
            capabilities(&TopLevelGridSurface::Folder).disabled_reason(SortOrder::RatingAsc),
            None
        );
    }
}

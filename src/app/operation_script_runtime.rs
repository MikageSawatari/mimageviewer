//! 操作スクリプトの同期実行境界。所有passの入力面を使い、実行状態を持ち越さない。

use std::{path::PathBuf, sync::Arc};

use eframe::egui;

use super::top_level_grid_view::{TopLevelGridSurface, TopLevelSearchView};
use super::{App, ViewerPresentation};
use crate::grid_item::GridItem;
use crate::keymap::KeyAction;
use crate::operation_script::{
    OperationScriptState, ScriptContext, ScriptInvocation, StateSnapshot, ValidationContext,
    validate_output,
};
use crate::ui_fullscreen::{FsKeyAction, FsKeyPageTransition};
use crate::ui_main::AddressBarNav;

mod events;
pub(crate) use events::OperationScriptEvents;

pub(crate) const MAIN_WINDOW_ONLY_NOTICE: &str = "この操作はメインウィンドウでのみ使用できます";
pub(crate) const SCRIPT_SLOT_ACTIONS: [KeyAction; 20] = [
    KeyAction::ScriptSlot1,
    KeyAction::ScriptSlot2,
    KeyAction::ScriptSlot3,
    KeyAction::ScriptSlot4,
    KeyAction::ScriptSlot5,
    KeyAction::ScriptSlot6,
    KeyAction::ScriptSlot7,
    KeyAction::ScriptSlot8,
    KeyAction::ScriptSlot9,
    KeyAction::ScriptSlot10,
    KeyAction::ScriptSlot11,
    KeyAction::ScriptSlot12,
    KeyAction::ScriptSlot13,
    KeyAction::ScriptSlot14,
    KeyAction::ScriptSlot15,
    KeyAction::ScriptSlot16,
    KeyAction::ScriptSlot17,
    KeyAction::ScriptSlot18,
    KeyAction::ScriptSlot19,
    KeyAction::ScriptSlot20,
];

pub(crate) fn script_slot_number(action: KeyAction) -> Option<u8> {
    SCRIPT_SLOT_ACTIONS
        .iter()
        .position(|candidate| *candidate == action)
        .map(|index| index as u8 + 1)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ScriptInputSurface {
    Grid,
    Viewer,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ScriptEligibility {
    Available(ScriptContext),
    Silent,
    Notify(String),
}

pub(crate) enum ScriptRunResult {
    Grid {
        nav: Option<AddressBarNav>,
        rejected: bool,
    },
    Fullscreen {
        action: FsKeyAction,
        rejected: bool,
        delegated: bool,
    },
    /// Native input completes the ordinary operation before returning.
    NativeVideo { rejected: bool },
}

/// Notification emitted by this invocation, independent of its navigation result.
#[derive(Clone, Debug)]
pub(crate) enum ScriptFeedback {
    Notice(String),
    Warning(String),
}

impl ScriptRunResult {
    fn empty(surface: ScriptInputSurface, native_video: bool, rejected: bool) -> Self {
        if native_video {
            return Self::NativeVideo { rejected };
        }
        match surface {
            ScriptInputSurface::Grid => Self::Grid {
                nav: None,
                rejected,
            },
            ScriptInputSurface::Viewer => Self::Fullscreen {
                action: FsKeyAction::default(),
                rejected,
                delegated: false,
            },
        }
    }

    fn rejected(&self) -> bool {
        match self {
            Self::Grid { rejected, .. }
            | Self::Fullscreen { rejected, .. }
            | Self::NativeVideo { rejected } => *rejected,
        }
    }

    fn needs_postprocessing(&self) -> bool {
        match self {
            Self::NativeVideo { .. } => false,
            Self::Grid { nav, .. } => nav.is_some(),
            Self::Fullscreen {
                action, delegated, ..
            } => {
                *delegated
                    || !matches!(action.page_transition, FsKeyPageTransition::Unchanged)
                    || !action.page_nav.is_none()
                    || action.ctrl_nav.is_some()
                    || action.list_order_nav.is_some()
                    || action.sibling_nav.is_some()
                    || action.mouse_nav.is_some()
                    || action.jump_to.is_some()
                    || action.close
                    || action.close_to_page_list
            }
        }
    }
}

/// 純粋な利用資格の決定。イベント検出も同じ優先順位を使う。
struct EligibilityFacts {
    main_context: bool,
    viewer_detached_or_switching: bool,
    blocked_input: bool,
    remote: bool,
    enabled: bool,
}

fn eligibility_from_facts(
    facts: EligibilityFacts,
    surface: ScriptInputSurface,
    context: ScriptContext,
    state: &OperationScriptState,
) -> ScriptEligibility {
    if !facts.main_context
        || (surface == ScriptInputSurface::Viewer && facts.viewer_detached_or_switching)
    {
        return ScriptEligibility::Notify(MAIN_WINDOW_ONLY_NOTICE.into());
    }
    if facts.blocked_input || facts.remote || !facts.enabled {
        return ScriptEligibility::Silent;
    }
    match state {
        OperationScriptState::Disabled => ScriptEligibility::Silent,
        OperationScriptState::Loading(_) => {
            ScriptEligibility::Notify("スクリプトを読み込み中です".into())
        }
        OperationScriptState::Failed(error) => ScriptEligibility::Notify(error.clone()),
        OperationScriptState::Ready(_) => ScriptEligibility::Available(context),
    }
}

/// パスの表示文字列を識別に使わない。PDFのページ・ZIP内の階層も保持する。
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ScriptItemIdentity {
    File(&'static str, PathBuf),
    ZipImage(PathBuf, String),
    ZipDirectory(PathBuf, String),
    PdfPage(PathBuf, u32),
    Stack(String, PathBuf),
}

fn item_identity(item: &GridItem) -> ScriptItemIdentity {
    match item {
        GridItem::ZipImage {
            zip_path,
            entry_name,
        } => ScriptItemIdentity::ZipImage(zip_path.clone(), entry_name.clone()),
        GridItem::ZipDir {
            zip_path,
            dir_prefix,
            ..
        } => ScriptItemIdentity::ZipDirectory(zip_path.clone(), dir_prefix.clone()),
        GridItem::PdfPage {
            pdf_path, page_num, ..
        } => ScriptItemIdentity::PdfPage(pdf_path.clone(), *page_num),
        GridItem::Stack {
            key,
            representative,
            ..
        } => ScriptItemIdentity::Stack(key.clone(), representative.clone()),
        GridItem::Folder(p) => ScriptItemIdentity::File("folder", p.clone()),
        GridItem::Image(p) => ScriptItemIdentity::File("image", p.clone()),
        GridItem::Video(p) => ScriptItemIdentity::File("video", p.clone()),
        GridItem::Audio(p) => ScriptItemIdentity::File("audio", p.clone()),
        GridItem::ZipFile(p) => ScriptItemIdentity::File("archive", p.clone()),
        GridItem::PdfFile(p) => ScriptItemIdentity::File("pdf", p.clone()),
        GridItem::ConvertibleArchive { path, .. } => {
            ScriptItemIdentity::File("archive", path.clone())
        }
        GridItem::SearchContainer { path, .. } => {
            ScriptItemIdentity::File("search_container", path.clone())
        }
        GridItem::CollectionPlaceholder { path, .. } => {
            ScriptItemIdentity::File("unavailable", path.clone())
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ScriptLocation {
    Physical {
        path: Option<PathBuf>,
        prefix: Vec<String>,
    },
    DriveList,
    Search {
        kind: TopLevelSearchView,
        query: String,
        child: Option<PathBuf>,
        prefix: Vec<String>,
    },
    Snapshot(Option<PathBuf>),
    Expansion {
        root: Option<PathBuf>,
        roots: Vec<PathBuf>,
    },
    Smart {
        id: uuid::Uuid,
        path: Option<PathBuf>,
        prefix: Vec<String>,
    },
    ReadingHistory,
    Bookmarks,
    Rating(u8),
    Collection {
        id: super::top_level_grid_view::CollectionGridIdentity,
        child: Option<PathBuf>,
        prefix: Vec<String>,
    },
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ScriptTargetIdentity {
    #[cfg(windows)]
    context_id: super::viewer_context_registry::ViewerContextId,
    context: ScriptContext,
    location: ScriptLocation,
    /// Accepted anchor items, independent of cache availability or screen order.
    pages: Vec<ScriptItemIdentity>,
    /// Accepted pairing origins can change the unit without moving fullscreen_idx.
    pairing_origins: (Option<ScriptItemIdentity>, Option<ScriptItemIdentity>),
    slice: crate::page_split::PageSlice,
    checked: Vec<(usize, ScriptItemIdentity)>,
}

/// Stage 2b request identity retains its full semantic reader-order comparison.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScriptPageIdentity {
    location: ScriptLocation,
    scalars: ScriptPageScalars,
    unit_source: ScriptUnitSourceInputs,
    reader_order: Vec<ScriptItemIdentity>,
}

/// Shared O(1)-in-item-count inputs for stage 2b and r11 event comparison.
/// Paths are bounded in number, not in byte length. No reader-order/location clone.
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct ScriptPageScalars {
    item: Option<ScriptItemIdentity>,
    spread_mode: crate::settings::SpreadMode,
    reading_flow: crate::settings::ReadingFlow,
    reading_direction: crate::settings::ReadingDirection,
    pairing_origins: (Option<ScriptItemIdentity>, Option<ScriptItemIdentity>),
    slice: crate::page_split::PageSlice,
    page_alone: crate::settings::PageAloneSettings,
    singleton_endpoints: crate::settings::SingletonSpreadEndpointSettings,
    page_turn_animation: bool,
    /// Keep the raw inputs: the effective getter can consult the composition service.
    final_cover: (crate::settings::FinalCoverSpreadPreference, bool),
    presentation_source: super::SpreadPresentationSource,
    browser_output_root: Option<PathBuf>,
    extracted_surface: Option<super::book_materialization::ExtractedSurface>,
    grid_view: crate::settings::GridViewMode,
    /// These App modes select continuous still-only versus ordinary navigation.
    continuous_view_suppression: (bool, bool, bool),
    endpoint_context_eligible: bool,
    split_direction: Option<crate::page_split::SplitDirection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ScriptUnitSourceInputs {
    Automatic {
        rule_total: usize,
        item_count: usize,
    },
    Composed(ScriptCompositionInputs),
}

/// Share adopted composition inputs. Compare semantic pages/directives only when
/// adoption changed the allocation; publication tokens and prepared geometry are not inputs.
#[derive(Clone, Debug)]
struct ScriptCompositionInputs(Arc<crate::book_composition::AcceptedCompositionProjection>);

impl PartialEq for ScriptCompositionInputs {
    fn eq(&self, other: &Self) -> bool {
        if Arc::ptr_eq(&self.0, &other.0) {
            return true;
        }
        let a = &self.0.resolved;
        let b = &other.0.resolved;
        self.0.scope == other.0.scope
            && a.direction == b.direction
            && a.cover_enabled == b.cover_enabled
            && a.complete == b.complete
            && a.visible_pages.len() == b.visible_pages.len()
            && a.visible_pages
                .iter()
                .zip(&b.visible_pages)
                .all(|(a_page, b_page)| {
                    a_page.page == b_page.page
                        && a.page_overrides.get(&a_page.item_index)
                            == b.page_overrides.get(&b_page.item_index)
                })
    }
}

impl Eq for ScriptCompositionInputs {}

impl App {
    pub(crate) fn operation_script_uses_native_video(&self) -> bool {
        #[cfg(windows)]
        {
            self.is_native_video_script_surface()
        }
        #[cfg(not(windows))]
        {
            false
        }
    }

    /// An already satisfied rating command is accepted, like the ordinary key.
    /// This is an execution check, never a snapshot read: existing rating actions
    /// use the same cache/DB read boundary before changing a value.
    pub(crate) fn operation_script_rating_noop(
        &mut self,
        family: crate::keymap::RatingTargetFamily,
        idx: Option<usize>,
        edit: crate::keymap::RatingEdit,
    ) -> bool {
        use super::book_preferred_rating::BookPreferredRatingTarget;
        use crate::keymap::{RatingEdit, RatingTargetFamily};
        let container = match family {
            RatingTargetFamily::Container => self.current_container_rating_target(),
            RatingTargetFamily::BookPreferred => {
                match self.resolve_book_preferred_viewer_target() {
                    BookPreferredRatingTarget::Book(target) => Some(target),
                    BookPreferredRatingTarget::ViewerItem => None,
                    BookPreferredRatingTarget::Unavailable(_) => return false,
                }
            }
            RatingTargetFamily::Item => None,
        };
        let before = if let Some(target) = container {
            self.rating_session_value_for_key(&target.key)
                .unwrap_or_else(|| self.container_rating_before(&target.key))
        } else if family == RatingTargetFamily::Container {
            return false;
        } else if let Some(idx) =
            idx.filter(|idx| self.items.get(*idx).is_some_and(GridItem::accepts_rating))
        {
            self.get_rating(idx)
        } else {
            return false;
        };
        match edit {
            RatingEdit::Assign(stars) => before == stars.min(5),
            RatingEdit::Step(direction) => crate::keymap::step_rating(before, direction).is_none(),
        }
    }
    pub(crate) fn operation_script_fullscreen_transition_pending(&self) -> bool {
        self.book_composition_permission.open_pending()
    }

    pub(crate) fn operation_script_keyboard_blocked(&self, ctx: &egui::Context) -> bool {
        let snapshot = self.keyboard_ownership_snapshot(ctx);
        snapshot.text_input.is_some() || snapshot.focused_ui.is_some()
    }
    fn script_context(&self, surface: ScriptInputSurface) -> ScriptContext {
        if surface == ScriptInputSurface::Grid {
            return ScriptContext::Grid;
        }
        if self
            .fullscreen_idx
            .and_then(|idx| self.items.get(idx))
            .is_some_and(|item| matches!(item, GridItem::Video(_) | GridItem::Audio(_)))
        {
            ScriptContext::FsVideo
        } else {
            ScriptContext::FsImage
        }
    }

    /// 所有passのctxを渡す。キャッシュ済みkeyboard ownerはメニュー閉鎖前の値かもしれないので使わない。
    pub(crate) fn operation_script_eligibility(
        &self,
        ctx: &egui::Context,
        surface: ScriptInputSurface,
    ) -> ScriptEligibility {
        #[cfg(windows)]
        let main_context = self.projected_viewer_context_id() == self.viewer_context_main();
        #[cfg(not(windows))]
        let main_context = true;
        eligibility_from_facts(
            EligibilityFacts {
                main_context,
                viewer_detached_or_switching: self
                    .current_viewer_session_is_detached_or_switching(),
                blocked_input: self.is_overlay_edit_mode_active()
                    || self.page_overview.session().is_some()
                    || self.common_modal_dialog_open()
                    || self.operation_script_media_modal_blocked(surface)
                    || self.operation_script_text_input_blocked(ctx, surface),
                remote: self.remote_session_blocks_local_control(),
                enabled: self.settings.operation_script_enabled,
            },
            surface,
            self.script_context(surface),
            &self.operation_script.state,
        )
    }

    fn operation_script_media_modal_blocked(&self, surface: ScriptInputSurface) -> bool {
        #[cfg(windows)]
        if surface == ScriptInputSurface::Viewer
            && let Some(idx) = self.fullscreen_idx
        {
            return self.normalize_scan_is_modal_for_current_player(idx)
                || (self.fs_music_view_active(idx) && self.music_bookmark_modal_open())
                || self
                    .music_vst_shell
                    .as_ref()
                    .is_some_and(|shell| shell.fs_idx == idx);
        }
        let _ = surface;
        false
    }

    fn operation_script_text_input_blocked(
        &self,
        ctx: &egui::Context,
        surface: ScriptInputSurface,
    ) -> bool {
        #[cfg(windows)]
        if surface == ScriptInputSurface::Viewer && self.is_native_video_script_surface() {
            // The overlay owns a separate egui Context. ROOT keyboard focus does
            // not describe text/IME input in that native window.
            return self.native_video_script_input_blocked();
        }
        let _ = surface;
        self.keyboard_ownership_snapshot(ctx).text_input.is_some()
    }

    pub(crate) fn operation_script_location(&self) -> ScriptLocation {
        let (path, prefix) = self.operation_script_physical_place();
        let child = self
            .current_grid_reload_is_physical()
            .then(|| path.clone())
            .flatten();
        match self.top_level_grid_view.surface() {
            TopLevelGridSurface::Folder => ScriptLocation::Physical { path, prefix },
            TopLevelGridSurface::DriveList => ScriptLocation::DriveList,
            TopLevelGridSurface::Search(kind) => ScriptLocation::Search {
                kind: *kind,
                query: match kind {
                    TopLevelSearchView::Favorite => format!(
                        "{}\n{:?}\n{}\n{:?}",
                        self.favsearch.last_executed,
                        self.favsearch.favorite_filter,
                        self.favsearch.or_mode,
                        self.favsearch.kind_filter
                    ),
                    TopLevelSearchView::Global => match &self.global_search.accepted {
                        crate::global_search_ui::SearchAccepted::Metadata { applied } => {
                            applied.as_ref().map_or_else(
                                || "metadata".into(),
                                |spec| format!("metadata\n{}\n{:?}", spec.query, spec.filters),
                            )
                        }
                        crate::global_search_ui::SearchAccepted::Everything { .. } => {
                            format!(
                                "everything\n{}",
                                self.global_search.accepted.applied_query()
                            )
                        }
                    },
                    TopLevelSearchView::Tag => format!(
                        "{}\n{:?}",
                        self.tag_view.last_executed, self.tag_view.last_executed_kind_filter
                    ),
                },
                child,
                prefix,
            },
            TopLevelGridSurface::Snapshot => {
                ScriptLocation::Snapshot(self.snapshot.as_ref().map(|s| s.origin.clone()))
            }
            TopLevelGridSurface::SubfolderExpansion => ScriptLocation::Expansion {
                root: self
                    .subfolder_expansion_snapshot
                    .as_ref()
                    .map(|s| s.root.clone()),
                roots: self
                    .subfolder_expansion_snapshot
                    .as_ref()
                    .map(|s| s.roots.clone())
                    .unwrap_or_default(),
            },
            TopLevelGridSurface::SmartFolder(state) => ScriptLocation::Smart {
                id: state.definition_id,
                path: child,
                prefix,
            },
            TopLevelGridSurface::ReadingHistory(_) => ScriptLocation::ReadingHistory,
            TopLevelGridSurface::Bookmarks => ScriptLocation::Bookmarks,
            TopLevelGridSurface::Rating { stars } => {
                if let Some(path) = child {
                    ScriptLocation::Physical {
                        path: Some(path),
                        prefix,
                    }
                } else {
                    ScriptLocation::Rating(*stars)
                }
            }
            TopLevelGridSurface::Collection(id) => ScriptLocation::Collection {
                id: *id,
                child,
                prefix,
            },
        }
    }

    fn operation_script_physical_place(&self) -> (Option<PathBuf>, Vec<String>) {
        use super::book_materialization::{BookScope, ProjectionRecipe};
        // The recipe is adopted with the rows. current_folder may already name
        // an unfinished enumeration, and must not replace a retained book's identity.
        match &self.projection_recipe {
            ProjectionRecipe::Book(BookScope::Archive {
                logical_root,
                prefix,
            }) => (Some(logical_root.clone()), prefix.clone()),
            ProjectionRecipe::Book(
                BookScope::Document(path)
                | BookScope::ImageFolder(path)
                | BookScope::CompiledFolder(path),
            ) => (Some(path.clone()), Vec::new()),
            ProjectionRecipe::OrdinaryList | ProjectionRecipe::ExtractedResult(_) => (
                self.archive_source_override
                    .clone()
                    .or_else(|| self.current_folder.clone()),
                self.zip_nav
                    .as_ref()
                    .map(|nav| nav.current().to_vec())
                    .unwrap_or_default(),
            ),
        }
    }

    pub(crate) fn operation_script_target(
        &mut self,
        surface: ScriptInputSurface,
    ) -> ScriptTargetIdentity {
        let context = self.script_context(surface);
        let indices: Vec<usize> = (if surface == ScriptInputSurface::Grid {
            self.selected
        } else {
            self.fullscreen_idx
        })
        .into_iter()
        .collect();
        let pairing_origins = if surface == ScriptInputSurface::Viewer {
            (
                self.spread_shift
                    .anchor_idx()
                    .and_then(|idx| self.items.get(idx))
                    .map(item_identity),
                self.spread_step
                    .and_then(|step| self.items.get(step.start_idx))
                    .map(item_identity),
            )
        } else {
            (None, None)
        };
        let mut checked = if surface == ScriptInputSurface::Grid {
            self.checked
                .iter()
                .copied()
                .filter_map(|idx| self.items.get(idx).map(|item| (idx, item_identity(item))))
                .collect::<Vec<_>>()
        } else {
            Vec::new()
        };
        checked.sort_by_key(|(idx, _)| *idx);
        ScriptTargetIdentity {
            #[cfg(windows)]
            context_id: self.projected_viewer_context_id(),
            context,
            location: self.operation_script_location(),
            pages: indices
                .into_iter()
                .filter_map(|idx| self.items.get(idx).map(item_identity))
                .collect(),
            pairing_origins,
            slice: if surface == ScriptInputSurface::Viewer {
                self.fullscreen_page_slice
            } else {
                crate::page_split::PageSlice::Full
            },
            checked,
        }
    }

    /// The page-event/request identity is the input tuple, not a resolved unit head.
    /// Read only App-owned values. Do not consult navigation/material caches or the
    /// composition service, build topology, or include publication/generation tokens.
    /// Adopted composition inputs share their immutable allocation; late geometry is omitted.
    pub(crate) fn operation_script_page_identity(&self) -> ScriptPageIdentity {
        ScriptPageIdentity {
            location: self.operation_script_location(),
            scalars: self.operation_script_page_scalars(),
            unit_source: self.operation_script_unit_source(),
            reader_order: self
                .current_reader_order()
                .iter()
                .filter_map(|&idx| self.items.get(idx))
                .map(item_identity)
                .collect(),
        }
    }

    fn operation_script_unit_source(&self) -> ScriptUnitSourceInputs {
        self.retained_composed_projection_input().map_or_else(
            || ScriptUnitSourceInputs::Automatic {
                rule_total: self.smart_folder_rule_total(),
                item_count: self.items.len(),
            },
            |projection| {
                ScriptUnitSourceInputs::Composed(ScriptCompositionInputs(Arc::clone(projection)))
            },
        )
    }

    fn operation_script_page_scalars(&self) -> ScriptPageScalars {
        ScriptPageScalars {
            item: self
                .fullscreen_idx
                .and_then(|idx| self.items.get(idx))
                .map(item_identity),
            spread_mode: self.spread_mode,
            reading_flow: self.reading_flow,
            reading_direction: self
                .spread_mode
                .canonical_reading_direction()
                .unwrap_or(self.reading_direction),
            pairing_origins: (
                self.spread_shift
                    .anchor_idx()
                    .and_then(|idx| self.items.get(idx))
                    .map(item_identity),
                self.spread_step
                    .and_then(|step| self.items.get(step.start_idx))
                    .map(item_identity),
            ),
            slice: self.fullscreen_page_slice,
            page_alone: self.page_alone_enabled_for_current_book(),
            singleton_endpoints: self.singleton_spread_placement_enabled_for_current_book(),
            page_turn_animation: self.settings.page_turn_animation_enabled,
            final_cover: (
                self.final_cover_spread_preference,
                self.settings.final_cover_spread_enabled,
            ),
            presentation_source: self.spread_presentation_source.clone(),
            browser_output_root: self.settings.browser_view_output_dir.clone(),
            extracted_surface: self.extracted_result_surface(),
            grid_view: self.settings.grid_view_mode,
            continuous_view_suppression: (
                self.analysis_mode,
                !matches!(self.compare_view_mode, super::CompareViewMode::Off),
                matches!(
                    self.fs_exclusive_view,
                    crate::fs_exclusive_view::FsExclusiveView::Panorama(_)
                ),
            ),
            endpoint_context_eligible: self.spread_complete_book_context_eligible(),
            split_direction: self.split_direction_now(),
        }
    }

    /// レーティングの読み取り専用投影。cache missでDBへ問い合わせない。
    pub(crate) fn operation_script_cached_rating(&self, idx: usize) -> Option<u8> {
        self.rating_path_key(idx)
            .and_then(|key| {
                self.rating_session_writes
                    .get(&key)
                    .map(|write| write.stars)
            })
            .or_else(|| self.rating_cache.get(&idx).copied())
    }

    pub(crate) fn collect_operation_script_snapshot(
        &mut self,
        surface: ScriptInputSurface,
    ) -> StateSnapshot {
        let context = self.script_context(surface);
        let mut snapshot = StateSnapshot::new(context);
        let viewer = surface == ScriptInputSurface::Viewer;
        snapshot.window.fullscreen =
            Some(viewer && self.viewer_presentation == ViewerPresentation::Fullscreen);
        snapshot.window.mode = Some(
            if viewer && self.viewer_presentation == ViewerPresentation::Fullscreen {
                "fullscreen"
            } else {
                "window"
            }
            .into(),
        );
        let idx = if viewer {
            self.fullscreen_idx
        } else {
            self.selected
        };
        let nav = if viewer {
            self.operation_script_navigation_indices()
        } else {
            self.current_grid_order().to_vec()
        };
        snapshot.page.count = Some(nav.len() as i64);
        snapshot.page.index = idx
            .and_then(|idx| nav.iter().position(|candidate| *candidate == idx))
            .map(|index| index as i64);
        if let Some(idx) = idx {
            let (first, last) = if context == ScriptContext::FsImage {
                let (_, first, last) = self.operation_script_image_page_state(idx);
                (first, last)
            } else {
                (
                    Some(nav.first() == Some(&idx)),
                    Some(nav.last() == Some(&idx)),
                )
            };
            snapshot.page.is_first = first;
            snapshot.page.is_last = last;
            if let Some(item) = self.items.get(idx) {
                snapshot.item.name = Some(item.name().into_owned());
                let item_path = match item {
                    GridItem::ZipImage { entry_name, .. } => entry_name.clone(),
                    GridItem::ZipDir { dir_prefix, .. } => dir_prefix.clone(),
                    GridItem::PdfPage { .. } => item.name().into_owned(),
                    _ => item.display_path(),
                };
                snapshot.item.ext = Some(
                    std::path::Path::new(&item_path)
                        .extension()
                        .and_then(|e| e.to_str())
                        .unwrap_or("")
                        .to_lowercase(),
                );
                snapshot.item.path = Some(item_path);
                snapshot.item.kind = Some(
                    match item {
                        GridItem::PdfPage { .. } => "pdf_page",
                        GridItem::ZipImage { .. } | GridItem::Image(_) => "image",
                        GridItem::Folder(_) | GridItem::ZipDir { .. } => "folder",
                        GridItem::Video(_) => "video",
                        GridItem::Audio(_) => "audio",
                        GridItem::ZipFile(_) | GridItem::ConvertibleArchive { .. } => "archive",
                        GridItem::PdfFile(_) => "pdf",
                        GridItem::Stack { .. } => "stack",
                        GridItem::SearchContainer { .. } => "search_container",
                        GridItem::CollectionPlaceholder { .. } => "unavailable",
                    }
                    .into(),
                );
            }
            snapshot.item.rating = self.operation_script_cached_rating(idx).map(i64::from);
            if let Some((w, h)) = self
                .canonical_source_dimensions(idx)
                .map(|dims| dims.as_tuple())
                .or_else(|| {
                    self.details_lazy_meta_for_idx(idx)
                        .and_then(|meta| meta.image_dims)
                })
            {
                snapshot.item.width = Some(i64::from(w));
                snapshot.item.height = Some(i64::from(h));
            }
            if let Some(target) = self.tag_target_for_index(idx, viewer) {
                let key = crate::tags_db::item_key_for_path(&target.path);
                if let Some(tags) = self.tags_cache.get(&key) {
                    let mut pinned = [false; 20];
                    for (slot, tag) in self
                        .settings
                        .tags
                        .iter()
                        .filter(|tag| tag.show_shortcut)
                        .take(20)
                        .enumerate()
                    {
                        pinned[slot] = tags
                            .iter()
                            .any(|value| crate::tags_db::normalize_tag_key(value) == tag.tag_key);
                    }
                    snapshot.item.pinned_tags = Some(pinned);
                }
            }
        }
        if viewer {
            if let Some(container) = self.operation_script_viewer_container() {
                snapshot.book.path = Some(container.path.to_string_lossy().into_owned());
                snapshot.book.kind = Some(container.kind.into());
            }
        } else {
            let (book_path, _) = self.operation_script_physical_place();
            snapshot.book.path = book_path
                .as_ref()
                .map(|path| path.to_string_lossy().into_owned());
            use super::book_materialization::{BookScope, ProjectionRecipe};
            snapshot.book.kind = Some(
                match &self.projection_recipe {
                    ProjectionRecipe::Book(BookScope::Document(path)) => {
                        if path
                            .extension()
                            .and_then(|ext| ext.to_str())
                            .is_some_and(|ext| ext.eq_ignore_ascii_case("epub"))
                        {
                            "epub"
                        } else {
                            "pdf"
                        }
                    }
                    ProjectionRecipe::Book(BookScope::Archive { .. }) => "zip",
                    _ if self.zip_nav.is_some() => "zip",
                    _ => "folder",
                }
                .into(),
            );
        }
        snapshot.view.spread = Some(
            match self.spread_mode {
                crate::settings::SpreadMode::Single => "single",
                crate::settings::SpreadMode::Ltr => "ltr",
                crate::settings::SpreadMode::LtrCover => "ltr_cover",
                crate::settings::SpreadMode::Rtl => "rtl",
                crate::settings::SpreadMode::RtlCover => "rtl_cover",
                crate::settings::SpreadMode::Vertical => "vertical",
                crate::settings::SpreadMode::SplitLtr => "split_ltr",
                crate::settings::SpreadMode::SplitRtl => "split_rtl",
            }
            .into(),
        );
        snapshot.view.direction = Some(
            match self.reading_direction {
                crate::settings::ReadingDirection::Ltr => "ltr",
                crate::settings::ReadingDirection::Rtl => "rtl",
            }
            .into(),
        );
        snapshot.view.background = Some(
            match self.fs_transparent_bg_mode {
                0 => "black",
                1 => "white",
                _ => "checker",
            }
            .into(),
        );
        if !viewer {
            snapshot.grid_selected_count = Some(
                self.selection_target_indices(super::ActionSurface::MainWindow)
                    .len() as i64,
            );
        }
        if context == ScriptContext::FsVideo
            && let Some(super::FsCacheEntry::Video { player, .. }) =
                idx.and_then(|idx| self.fs_cache.get(&idx))
        {
            snapshot.video.playing = Some(player.is_playing());
            snapshot.video.position_sec = Some(player.position());
            // A loading player has no duration yet; zero is a legitimate cached
            // duration only once metadata has arrived.
            snapshot.video.duration_sec = player.info().map(|info| info.duration_secs);
        }
        snapshot
    }

    fn script_failure(
        &mut self,
        number: u8,
        reason: impl std::fmt::Display,
        feedback: &mut Option<ScriptFeedback>,
    ) {
        let message = format!("スクリプト操作 {number}: {reason}");
        crate::logger::log(&message);
        self.publish_script_feedback(ScriptFeedback::Warning(message), feedback);
    }

    fn publish_script_feedback(
        &mut self,
        message: ScriptFeedback,
        feedback: &mut Option<ScriptFeedback>,
    ) {
        match &message {
            ScriptFeedback::Notice(text) => self.show_feedback_toast(text.clone()),
            ScriptFeedback::Warning(text) => self.show_feedback_warning(text.clone()),
        }
        *feedback = Some(message);
    }

    pub(crate) fn run_script_slot(
        &mut self,
        ctx: &egui::Context,
        number: u8,
        surface: ScriptInputSurface,
        navigation_already_accepted: bool,
    ) -> ScriptRunResult {
        self.run_script_slot_with_feedback(ctx, number, surface, navigation_already_accepted)
            .0
    }

    pub(crate) fn run_script_slot_with_feedback(
        &mut self,
        ctx: &egui::Context,
        number: u8,
        surface: ScriptInputSurface,
        navigation_already_accepted: bool,
    ) -> (ScriptRunResult, Option<ScriptFeedback>) {
        let started = std::time::Instant::now();
        let mut operations = None;
        let mut feedback = None;
        let result = self.run_script_slot_with_metrics(
            ctx,
            number,
            surface,
            navigation_already_accepted,
            &mut operations,
            &mut feedback,
        );
        if crate::perf::is_enabled() {
            crate::perf::event(
                "script",
                "run",
                None,
                0,
                &[
                    ("slot", serde_json::json!(number)),
                    (
                        "ms",
                        serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
                    ),
                    ("operations", serde_json::json!(operations)),
                    ("rejected", serde_json::json!(result.rejected())),
                    (
                        "postprocessing",
                        serde_json::json!(result.needs_postprocessing()),
                    ),
                ],
            );
        }
        (result, feedback)
    }

    fn execute_operation_script_action(
        &mut self,
        ctx: &egui::Context,
        surface: ScriptInputSurface,
        key_action: KeyAction,
        native_video: bool,
    ) -> ScriptRunResult {
        match surface {
            ScriptInputSurface::Grid => {
                self.execute_grid_key_action(ctx, key_action, "script_grid")
            }
            ScriptInputSurface::Viewer => {
                #[cfg(windows)]
                if native_video {
                    let rejected = self.fullscreen_idx.is_none_or(|idx| {
                        !self.is_native_video_script_surface()
                            || !self.native_video_script_action_admitted(idx, key_action)
                            || matches!(
                                self.execute_video_key_action(
                                    ctx,
                                    idx,
                                    key_action,
                                    super::VideoKeyActionSurface::Native,
                                ),
                                super::VideoKeyActionResult::Rejected
                            )
                    });
                    ScriptRunResult::NativeVideo { rejected }
                } else {
                    let (action, rejected, delegated) =
                        self.execute_fullscreen_script_action(ctx, key_action);
                    ScriptRunResult::Fullscreen {
                        action,
                        rejected,
                        delegated,
                    }
                }
                #[cfg(not(windows))]
                {
                    let (action, rejected, delegated) =
                        self.execute_fullscreen_script_action(ctx, key_action);
                    ScriptRunResult::Fullscreen {
                        action,
                        rejected,
                        delegated,
                    }
                }
            }
        }
    }

    fn run_script_slot_with_metrics(
        &mut self,
        ctx: &egui::Context,
        number: u8,
        surface: ScriptInputSurface,
        navigation_already_accepted: bool,
        operations: &mut Option<u64>,
        feedback: &mut Option<ScriptFeedback>,
    ) -> ScriptRunResult {
        #[cfg(windows)]
        let native_video =
            surface == ScriptInputSurface::Viewer && self.is_native_video_script_surface();
        #[cfg(not(windows))]
        let native_video = false;
        let context = match self.operation_script_eligibility(ctx, surface) {
            ScriptEligibility::Available(context) => context,
            ScriptEligibility::Silent => {
                return ScriptRunResult::empty(surface, native_video, true);
            }
            ScriptEligibility::Notify(reason) => {
                self.script_failure(number, reason, feedback);
                return ScriptRunResult::empty(surface, native_video, true);
            }
        };
        let snapshot = self.collect_operation_script_snapshot(surface);
        let target = self.operation_script_target(surface);
        let output = match &self.operation_script.state {
            OperationScriptState::Ready(script) => script.evaluate_slot(number, &snapshot),
            _ => unreachable!("eligibility admitted only a loaded script"),
        };
        if let OperationScriptState::Ready(script) = &self.operation_script.state {
            *operations = Some(script.last_operation_count());
        }
        let output = match output.and_then(|output| {
            validate_output(
                output,
                ValidationContext {
                    context,
                    invocation: ScriptInvocation::Input,
                    navigation_already_accepted,
                },
            )
            .map_err(|error| format!("操作の指定を確認してください ({error:?})"))
        }) {
            Ok(output) => output,
            Err(error) => {
                self.script_failure(number, error, feedback);
                return ScriptRunResult::empty(surface, native_video, true);
            }
        };
        let mut result = ScriptRunResult::empty(surface, native_video, false);
        for (index, operation) in output.operations().iter().enumerate() {
            if !matches!(
                self.operation_script_eligibility(ctx, surface),
                ScriptEligibility::Available(current) if current == context
            ) || self.operation_script_target(surface) != target
            {
                self.script_failure(
                    number,
                    format!("『{}』を実行できませんでした", operation.action.ini_name()),
                    feedback,
                );
                return ScriptRunResult::empty(surface, native_video, true);
            }
            result =
                self.execute_operation_script_action(ctx, surface, operation.action, native_video);
            if result.rejected() {
                self.script_failure(
                    number,
                    format!("『{}』を実行できませんでした", operation.action.ini_name()),
                    feedback,
                );
                return result;
            }
            let remaining = index + 1 < output.operations().len();
            if result.needs_postprocessing() {
                if remaining {
                    self.script_failure(
                        number,
                        "画面の遷移があるため残りの操作を中止しました",
                        feedback,
                    );
                } else if let Some(notice) = output.notice() {
                    self.publish_script_feedback(
                        ScriptFeedback::Notice(notice.to_owned()),
                        feedback,
                    );
                }
                return result;
            }
            if remaining && self.operation_script_target(surface) != target {
                self.script_failure(
                    number,
                    "操作の対象が変わったため残りの操作を中止しました",
                    feedback,
                );
                return result;
            }
        }
        if let Some(notice) = output.notice() {
            self.publish_script_feedback(ScriptFeedback::Notice(notice.to_owned()), feedback);
        }
        result
    }
}

#[cfg(test)]
mod tests;

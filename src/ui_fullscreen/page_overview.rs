//! Overview content and input share the existing fullscreen surface and viewer owner.
use super::*;
use crate::app::page_overview::PageOverviewCommand;
use crate::page_overview::{PageOverviewCursorMove, PageOverviewDirection, PageOverviewSelect};
use crate::ui_page_overview::{
    PageOverviewRenderInput, PageOverviewThumbnail, PageOverviewUiAction,
};

fn page_overview_request_ordinals(
    layout: &crate::page_overview::PageOverviewLayout,
    anchor: crate::page_overview::PageOverviewScrollAnchor,
) -> impl Iterator<Item = usize> {
    let visible = layout.visible_ordinals(anchor);
    visible.clone().chain(
        layout
            .overscan_ordinals(anchor)
            .filter(move |ordinal| !visible.contains(ordinal)),
    )
}

fn page_overview_layout_cache_id(view: &crate::page_overview::PageOverviewView) -> egui::Id {
    egui::Id::new((
        "overview_layout",
        view.stamp.context_serial,
        view.stamp.items_generation,
        view.stamp.reader_order_revision,
        view.stamp.incarnation,
    ))
}

fn page_overview_layout_changed(
    previous: &crate::page_overview::PageOverviewLayout,
    next: &crate::page_overview::PageOverviewLayout,
) -> bool {
    previous.viewport() != next.viewport()
        || previous.columns() != next.columns()
        || previous.cell_size() != next.cell_size()
}

fn passive_page_overview_effective_view(
    view: &crate::page_overview::PageOverviewView,
    original: &crate::page_overview::PageOverviewLayout,
    surface: egui::Rect,
    direction: PageOverviewDirection,
) -> crate::page_overview::PageOverviewView {
    let next = crate::ui_page_overview::page_overview_geometry(surface, view, direction).layout;
    if page_overview_layout_changed(original, &next) {
        view.reflowed(original, &next)
    } else {
        view.clone()
    }
}

fn passive_page_overview_original_layout(
    ctx: &egui::Context,
    view: &crate::page_overview::PageOverviewView,
    captured: crate::page_overview::PageOverviewLayout,
) -> crate::page_overview::PageOverviewLayout {
    let cache_id = page_overview_layout_cache_id(view);
    App::page_overview_cached_layout(ctx, view).unwrap_or_else(|| {
        // A cold passive callback can precede the first active paint. Seed only the
        // immutable captured fact, leaving resume's original geometry intact.
        ctx.data_mut(|data| data.insert_temp(cache_id, captured));
        captured
    })
}

/// Loaded.tex already includes the worker's edit preview. Match the grid's evaluated
/// color/annotation layer preference without generating a texture or reading rotation DBs.
/// Unknown rotation leaves the cell's selectable placeholder until the existing worker resolves it.
fn page_overview_loaded_texture<'a>(
    raw: &'a egui::TextureHandle,
    evaluated: Option<&'a egui::TextureHandle>,
    rotation: Option<crate::rotation_db::Rotation>,
    adjustment_dragging: bool,
) -> Option<(&'a egui::TextureHandle, crate::rotation_db::Rotation)> {
    let rotation = rotation?;
    let texture = if adjustment_dragging {
        raw
    } else {
        evaluated.unwrap_or(raw)
    };
    Some((texture, rotation))
}

fn page_overview_loaded_thumbnail(
    texture: &egui::TextureHandle,
    rotation: crate::rotation_db::Rotation,
) -> PageOverviewThumbnail {
    let corners = [
        egui::pos2(0., 0.),
        egui::pos2(1., 0.),
        egui::pos2(1., 1.),
        egui::pos2(0., 1.),
    ];
    let offset = (rotation.degrees() / 90) as usize;
    PageOverviewThumbnail::Loaded {
        texture_id: texture.id(),
        display_size: rotated_display_size(texture.size_vec2(), rotation),
        uv_corners: std::array::from_fn(|i| corners[(i + 4 - offset) % 4]),
    }
}

fn overview_folder_navigation_action(action: KeyAction) -> bool {
    matches!(
        action,
        KeyAction::FsCtrlNavPrev
            | KeyAction::FsCtrlNavNext
            | KeyAction::FsSiblingPrev
            | KeyAction::FsSiblingNext
    )
}

/// Active and passive callbacks consume the same runtime-filtered priority list. Ordinary
/// overview operations stay no-repeat; the four existing cumulative navigation actions retain
/// their physical-press cardinality through the existing Keymap owner/edge helpers.
fn consume_page_overview_action(
    keymap: &crate::keymap::Keymap,
    ctx: &egui::Context,
    executable_actions: &[KeyAction],
) -> Option<(KeyAction, usize)> {
    for &action in crate::keymap::FS_PAGE_OVERVIEW_ACTION_PRIORITY {
        if !executable_actions.contains(&action) {
            continue;
        }
        if overview_folder_navigation_action(action) {
            let count = keymap.consume_action_press_count(ctx, action);
            if count > 0 {
                return Some((action, count));
            }
        } else if keymap
            .consume_first_action_no_repeat(
                ctx,
                crate::keymap::FS_PAGE_OVERVIEW_ACTIVE_SCOPES,
                &[action],
            )
            .is_some()
        {
            return Some((action, 1));
        }
    }
    None
}

impl App {
    pub(super) fn fs_page_transition_after_overview_input(
        &self,
        previous_idx: usize,
    ) -> FsKeyPageTransition {
        match self.fullscreen_idx {
            None => FsKeyPageTransition::Closed,
            Some(idx) if idx != previous_idx || self.page_overview.session().is_none() => {
                FsKeyPageTransition::Rebased { idx }
            }
            Some(_) => FsKeyPageTransition::Unchanged,
        }
    }
    /// This cache describes the canonical session's last active geometry. Passive rendering
    /// only reads it, so resume still reflows the owner from its parked original layout.
    pub(crate) fn page_overview_cached_layout(
        ctx: &egui::Context,
        view: &crate::page_overview::PageOverviewView,
    ) -> Option<crate::page_overview::PageOverviewLayout> {
        ctx.data(|data| data.get_temp(page_overview_layout_cache_id(view)))
    }

    pub(super) fn draw_page_overview_window_close(ui: &mut egui::Ui, surface: egui::Rect) -> bool {
        let rect = egui::Rect::from_min_size(
            egui::pos2(surface.right() - 28.0, surface.top() + 4.0),
            egui::vec2(24.0, 24.0),
        )
        .intersect(surface);
        let _response = ui.interact(
            rect,
            ui.id().with("overview-window-close"),
            egui::Sense::CLICK,
        );
        ui.painter().text(
            rect.center(),
            egui::Align2::CENTER_CENTER,
            "×",
            egui::FontId::proportional(17.0),
            ui.visuals().text_color(),
        );
        ui.rect_contains_pointer(rect)
            && ui.input(|input| input.pointer.button_clicked(egui::PointerButton::Primary))
    }

    #[cfg(windows)]
    pub(super) fn draw_passive_page_overview(
        ui: &mut egui::Ui,
        surface: egui::Rect,
        window: &crate::app::DeferredDetachedImageWindowView,
        overview: &crate::app::DetachedPageOverviewProjection,
    ) -> Vec<(
        PageOverviewCommand,
        crate::app::DetachedPageOverviewInputProvenance,
    )> {
        use crate::app::DetachedPageOverviewInputProvenance::{Key, Pointer};
        let ctx = ui.ctx().clone();
        let ime = crate::ime_focus::ime_input_active(&ctx);
        let focused_text = crate::ime_focus::focused_text_input(&ctx);
        let owner = crate::keyboard_input::decide_keyboard_owner(
            crate::keyboard_input::KeyboardOwnershipSnapshot {
                viewport: ctx.viewport_id(),
                viewport_focused: ctx.input(|input| input.viewport().focused.unwrap_or(false)),
                modal: overview.input_blocked,
                text_input: focused_text
                    .map(|id| {
                        crate::keyboard_input::TextInputClaim::new(
                            id,
                            crate::keyboard_input::TextInputPhase::Focused,
                        )
                    })
                    .or_else(|| {
                        ime.then(|| {
                            crate::keyboard_input::TextInputClaim::new(
                                egui::Id::new("overview-ime"),
                                crate::keyboard_input::TextInputPhase::ImeGrace,
                            )
                        })
                    }),
                focused_ui: ctx
                    .memory(|memory| memory.focused())
                    .filter(|id| Some(*id) != focused_text),
                shortcut_scope: Some(crate::keyboard_input::ShortcutScope::new(
                    ctx.viewport_id(),
                    crate::keyboard_input::ShortcutSurface::Fullscreen,
                )),
            },
        );
        crate::keyboard_input::cache_keyboard_owner(&ctx, owner);
        let original =
            passive_page_overview_original_layout(&ctx, &overview.view, overview.captured_layout);
        let effective_view = passive_page_overview_effective_view(
            &overview.view,
            &original,
            surface,
            overview.direction,
        );
        let view = &effective_view;
        let mut commands = Vec::new();
        let key_input = !overview.input_blocked
            && !ime
            && !owner.blocks_legacy_keymap_shortcuts()
            && crate::keyboard_input::keyboard_input_permits(&ctx)
                .discrete
                .is_some();
        let mut handled_key = false;
        if key_input {
            #[cfg(all(windows, feature = "test-script"))]
            crate::test_script::mark_action_pass_eligible(&ctx);
            if let Some(permit) = owner.fullscreen_raw_key_permit() {
                if crate::keyboard_input::consume_fullscreen_raw_key(
                    &ctx,
                    permit,
                    egui::Modifiers::NONE,
                    egui::Key::Escape,
                ) {
                    commands.push((PageOverviewCommand::Cancel { stamp: view.stamp }, Key));
                    handled_key = true;
                }
            }
            if !handled_key {
                if let Some((action, press_count)) = consume_page_overview_action(
                    &overview.keymap,
                    &ctx,
                    &overview.executable_actions,
                ) {
                    let movement = match action {
                        KeyAction::FsPageOverviewPagePrev => Some(PageOverviewCursorMove::PageUp),
                        KeyAction::FsPageOverviewPageNext => Some(PageOverviewCursorMove::PageDown),
                        KeyAction::FsPageOverviewFirst => Some(PageOverviewCursorMove::First),
                        KeyAction::FsPageOverviewLast => Some(PageOverviewCursorMove::Last),
                        _ => None,
                    };
                    let command = if let Some(movement) = movement {
                        Self::passive_overview_cursor_command(
                            view,
                            surface,
                            overview.direction,
                            movement,
                        )
                    } else {
                        match action {
                            KeyAction::FsPageOverviewToggle => {
                                Some(PageOverviewCommand::Cancel { stamp: view.stamp })
                            }
                            KeyAction::FsPageOverviewConfirm => view
                                .capture_select(view.cursor)
                                .map(PageOverviewCommand::Select),
                            KeyAction::FsPageOverviewColumnsLess => {
                                Some(PageOverviewCommand::ColumnsDelta {
                                    stamp: view.stamp,
                                    delta: -1,
                                })
                            }
                            KeyAction::FsPageOverviewColumnsMore => {
                                Some(PageOverviewCommand::ColumnsDelta {
                                    stamp: view.stamp,
                                    delta: 1,
                                })
                            }
                            _ => Some(PageOverviewCommand::Action {
                                stamp: view.stamp,
                                action,
                                press_count,
                            }),
                        }
                    };
                    if let Some(command) = command {
                        commands.push((command, Key));
                    }
                    handled_key = true;
                }
            }
            if !handled_key {
                if let Some(permit) = owner.fullscreen_raw_key_permit() {
                    for (key, movement) in [
                        (egui::Key::ArrowLeft, PageOverviewCursorMove::Left),
                        (egui::Key::ArrowRight, PageOverviewCursorMove::Right),
                        (egui::Key::ArrowUp, PageOverviewCursorMove::Up),
                        (egui::Key::ArrowDown, PageOverviewCursorMove::Down),
                    ] {
                        if crate::keyboard_input::consume_fullscreen_raw_key(
                            &ctx,
                            permit,
                            egui::Modifiers::NONE,
                            key,
                        ) {
                            if let Some(command) = Self::passive_overview_cursor_command(
                                view,
                                surface,
                                overview.direction,
                                movement,
                            ) {
                                commands.push((command, Key));
                            }
                            break;
                        }
                    }
                }
            }
        }
        let output = crate::ui_page_overview::draw_page_overview(
            ui,
            PageOverviewRenderInput {
                view,
                title: &window.location_display,
                current_items_indices: &overview.current_items_indices,
                thumbnails: Some(&overview.thumbnails),
                direction: overview.direction,
                input_enabled: !overview.input_blocked,
            },
        );
        commands.extend(
            output
                .commands
                .into_iter()
                .map(|command| (Self::page_overview_ui_command(command), Pointer)),
        );
        commands
    }

    fn passive_overview_cursor_command(
        view: &crate::page_overview::PageOverviewView,
        surface: egui::Rect,
        direction: PageOverviewDirection,
        movement: PageOverviewCursorMove,
    ) -> Option<PageOverviewCommand> {
        let layout =
            crate::ui_page_overview::page_overview_geometry(surface, view, direction).layout;
        let ordinal = layout.cursor_after_move(view.cursor, movement)?;
        Some(PageOverviewCommand::SetCursor {
            stamp: view.stamp,
            target: view.capture_select(ordinal)?.target,
        })
    }

    fn page_overview_ui_command(
        command: crate::ui_page_overview::PageOverviewUiCommand,
    ) -> PageOverviewCommand {
        match command.action {
            PageOverviewUiAction::Select(target) => {
                PageOverviewCommand::Select(PageOverviewSelect {
                    stamp: command.stamp,
                    target,
                })
            }
            PageOverviewUiAction::Cancel => PageOverviewCommand::Cancel {
                stamp: command.stamp,
            },
            PageOverviewUiAction::ScrollRows(delta) => PageOverviewCommand::ScrollRows {
                stamp: command.stamp,
                delta,
            },
            PageOverviewUiAction::ColumnsDelta(delta) => PageOverviewCommand::ColumnsDelta {
                stamp: command.stamp,
                delta,
            },
        }
    }

    pub(super) fn draw_page_overview_readonly(
        &self,
        ui: &mut egui::Ui,
        view: &crate::page_overview::PageOverviewView,
    ) {
        let direction = self.page_overview_direction();
        let original = Self::page_overview_cached_layout(ui.ctx(), view);
        let effective_view = original.map(|original| {
            passive_page_overview_effective_view(view, &original, ui.max_rect(), direction)
        });
        let view = effective_view.as_ref().unwrap_or(view);
        let layout =
            crate::ui_page_overview::page_overview_geometry(ui.max_rect(), view, direction).layout;
        let pages = page_overview_request_ordinals(&layout, view.scroll_anchor)
            .filter_map(|ordinal| view.projection.entry(ordinal).map(|entry| entry.items_idx))
            .collect::<Vec<_>>();
        let thumbnails = self.page_overview_thumbnail_projection(&pages);
        let current = self.fullscreen_idx.into_iter().collect::<Vec<_>>();
        crate::ui_page_overview::draw_page_overview(
            ui,
            PageOverviewRenderInput {
                view,
                title: "ページ一覧",
                current_items_indices: &current,
                thumbnails: Some(&thumbnails),
                direction,
                input_enabled: false,
            },
        );
    }
    pub(crate) fn page_overview_executable_actions(&self) -> Vec<KeyAction> {
        if self.remote_session_blocks_local_control() {
            return Vec::new();
        }
        crate::keymap::FS_PAGE_OVERVIEW_ACTION_PRIORITY
            .iter()
            .copied()
            .filter(|action| {
                if overview_folder_navigation_action(*action)
                    && !self.page_overview_folder_navigation_available(*action)
                {
                    return false;
                }
                #[cfg(windows)]
                if *action == KeyAction::ToggleDetachedViewerMode
                    && self.detached_toggle_disabled_by_always_new_images()
                {
                    return false;
                }
                true
            })
            .collect()
    }

    /// Mirror ordinary handler admission using its existing owner predicates. A navigation
    /// lock is not a blanket refusal: compatible DFS/Collection/Smart requests accept repeats.
    fn page_overview_folder_navigation_available(&self, action: KeyAction) -> bool {
        let Some(idx) = self.fullscreen_idx else {
            return false;
        };
        let sibling = matches!(action, KeyAction::FsSiblingPrev | KeyAction::FsSiblingNext);
        if !sibling && self.staged_smart_fullscreen_ctrl_step_available() {
            return true;
        }
        if self.fs_navigation_sequence_blocks_new_target() {
            return false;
        }
        if self.fs_nav_is_locked() {
            return (!sibling
                && self
                    .top_level_grid_view
                    .collection_outer_navigation_available(true))
                || self.locked_fullscreen_folder_nav_request(sibling).is_some();
        }
        let collection_available =
            !sibling && self.collection_outer_fullscreen_navigation_available(idx);
        let detached_collection_origin = self.top_level_grid_view.collection_session().is_none()
            && matches!(
                self.top_level_grid_view.return_to(),
                Some(crate::app::top_level_grid_view::TopLevelGridRestore::Collection(_))
            );
        if collection_available && detached_collection_origin {
            return true;
        }
        #[cfg(windows)]
        {
            if self.detached_physical_folder_nav_available() {
                return self.effective_folder().is_some();
            }
            if collection_available
                && matches!(
                    self.navigation_scope,
                    crate::app::ViewerNavigationScope::CollectionRoot
                )
            {
                return true;
            }
            if self.detached_independent_session_blocks_folder_nav() {
                return false;
            }
        }
        if self.is_snapshot_active() {
            return true;
        }
        if self.global_search.active {
            return !sibling && self.global_search.drill.is_some();
        }
        if self.favsearch.active {
            return !sibling
                && self.favsearch.nav_stack.first().is_some()
                && self.favsearch.nav_stack.last().is_some();
        }
        if self.show_search_bar || self.items_are_subfolder_expansion_view {
            return false;
        }
        if sibling && self.items_are_smart_folder_view {
            return false;
        }
        if !sibling
            && (self.zip_nav.is_some()
                || collection_available
                || self.top_level_grid_view.smart_folder().is_some())
        {
            return true;
        }
        self.effective_folder().is_some()
    }

    pub(crate) fn page_overview_direction(&self) -> PageOverviewDirection {
        if self.reading_direction == ReadingDirection::Rtl {
            PageOverviewDirection::RightToLeft
        } else {
            PageOverviewDirection::LeftToRight
        }
    }

    pub(crate) fn reflow_page_overview_to_surface(
        &mut self,
        ctx: &egui::Context,
        viewport: egui::ViewportId,
        surface: egui::Rect,
    ) -> Option<crate::page_overview::PageOverviewLayout> {
        let view = self.page_overview_view()?;
        if self.page_overview_active_context_serial() != Some(view.stamp.context_serial)
            || viewport != self.page_overview_viewer_viewport()
        {
            return None;
        }
        let layout = crate::ui_page_overview::page_overview_geometry(
            surface,
            &view,
            self.page_overview_direction(),
        )
        .layout;
        if let Some(previous) = Self::page_overview_cached_layout(ctx, &view)
            .filter(|previous| page_overview_layout_changed(previous, &layout))
        {
            self.page_overview.session_mut()?.reflow(&previous, &layout);
        }
        ctx.data_mut(|data| data.insert_temp(page_overview_layout_cache_id(&view), layout));
        Some(layout)
    }

    fn page_overview_viewer_viewport(&self) -> egui::ViewportId {
        let embedded = matches!(
            self.viewer_chrome_surface(),
            crate::ui_helpers::ViewerChromeSurface::MainEmbedded
        );
        if embedded {
            egui::ViewportId::ROOT
        } else {
            self.fullscreen_viewport_id()
        }
    }

    pub(crate) fn page_overview_surface_for_viewport(
        ctx: &egui::Context,
        viewport: egui::ViewportId,
    ) -> Option<egui::Rect> {
        if viewport == ctx.viewport_id() {
            return Some(ctx.content_rect())
                .filter(|surface| surface.is_finite() && surface.is_positive());
        }
        // Root input has the current child resize facts even before that child's next
        // pass creates or updates its separate InputState. Prefer those fresh facts.
        let current_size = ctx
            .input(|input| {
                input
                    .raw
                    .viewports
                    .get(&viewport)
                    .and_then(|info| info.inner_rect)
                    .map(|rect| rect.size())
            })
            .filter(|size| size.is_finite() && size.x > 0.0 && size.y > 0.0);
        current_size
            .or_else(|| {
                ctx.input_for(viewport, |input| {
                    input.viewport().inner_rect.map(|rect| rect.size())
                })
                .filter(|size| size.is_finite() && size.x > 0.0 && size.y > 0.0)
            })
            .map(|size| egui::Rect::from_min_size(egui::Pos2::ZERO, size))
    }

    fn page_overview_viewer_surface(&self, ctx: &egui::Context) -> Option<egui::Rect> {
        let surface =
            Self::page_overview_surface_for_viewport(ctx, self.page_overview_viewer_viewport());
        #[cfg(windows)]
        if surface.is_none() {
            if let crate::ui_helpers::ViewerChromeSurface::Detached { window_id, .. } =
                self.viewer_chrome_surface()
            {
                return self
                    .detached_window_runtime_placement(window_id)
                    .map(|placement| {
                        egui::Rect::from_min_size(
                            egui::Pos2::ZERO,
                            egui::vec2(
                                crate::settings::window_geometry_to_viewport_points(
                                    placement.w,
                                    self.settings.ui_scale_factor,
                                ),
                                crate::settings::window_geometry_to_viewport_points(
                                    placement.h,
                                    self.settings.ui_scale_factor,
                                ),
                            ),
                        )
                    });
            }
        }
        surface.filter(|surface| surface.is_finite() && surface.is_positive())
    }

    pub(crate) fn open_page_overview_for_viewer(&mut self, ctx: &egui::Context) -> bool {
        let Some(surface) = self.page_overview_viewer_surface(ctx) else {
            return false;
        };
        let direction = self.page_overview_direction();
        if !self.open_page_overview(surface, direction) {
            return false;
        }
        let view = self
            .page_overview_view()
            .expect("successful open retains view");
        let viewport = self.page_overview_viewer_viewport();
        // Open's initial anchor was computed against this real surface. Preserve that
        // geometric origin, then reflow once into the header/scrollbar-adjusted body.
        ctx.data_mut(|data| {
            data.insert_temp(
                page_overview_layout_cache_id(&view),
                view.layout(surface, direction),
            )
        });
        self.reflow_page_overview_to_surface(ctx, viewport, surface);
        true
    }

    fn page_overview_body(&mut self, ctx: &egui::Context) -> egui::Rect {
        let viewport = self.page_overview_viewer_viewport();
        let Some(view) = self.page_overview_view() else {
            return egui::Rect::ZERO;
        };
        let cached = Self::page_overview_cached_layout(ctx, &view);
        let surface = self.page_overview_viewer_surface(ctx);
        if let Some(surface) = surface {
            if let Some(layout) = self.reflow_page_overview_to_surface(ctx, viewport, surface) {
                return layout.viewport();
            }
        }
        cached.map_or(egui::Rect::ZERO, |layout| layout.viewport())
    }

    pub(crate) fn page_overview_cursor_input(
        &mut self,
        ctx: &egui::Context,
        movement: PageOverviewCursorMove,
    ) -> bool {
        if self.page_overview_active_context_serial().is_none()
            || self.any_dialog_open()
            || self.remote_session_blocks_local_control()
        {
            return false;
        }
        let body = self.page_overview_body(ctx);
        if !body.is_positive() {
            return false;
        }
        let changed =
            self.move_page_overview_cursor(movement, body, self.page_overview_direction());
        if changed {
            ctx.request_repaint();
        }
        changed
    }

    pub(crate) fn execute_page_overview_key_action(
        &mut self,
        ctx: &egui::Context,
        action: &KeyAction,
    ) -> bool {
        self.execute_page_overview_key_action_count(ctx, action, 1)
    }

    pub(crate) fn execute_page_overview_key_action_count(
        &mut self,
        ctx: &egui::Context,
        action: &KeyAction,
        press_count: usize,
    ) -> bool {
        if !PageOverviewCommand::action_count_is_valid(*action, press_count) {
            return false;
        }
        let opened = self.page_overview.session().is_some();
        if !opened {
            if *action != KeyAction::FsPageOverviewToggle || !self.page_overview_available() {
                return false;
            }
            let result = self.open_page_overview_for_viewer(ctx);
            if result {
                ctx.request_repaint();
            }
            return result;
        }
        if self.any_dialog_open() || !self.page_overview_executable_actions().contains(action) {
            return false;
        }
        let Some(view) = self.page_overview_view() else {
            return false;
        };
        if self.page_overview_active_context_serial() != Some(view.stamp.context_serial)
            || !self.page_overview_action_stamp_is_current(view.stamp)
        {
            return false;
        }
        let stamp = view.stamp;
        let movement = match action {
            KeyAction::FsPageOverviewPagePrev => Some(PageOverviewCursorMove::PageUp),
            KeyAction::FsPageOverviewPageNext => Some(PageOverviewCursorMove::PageDown),
            KeyAction::FsPageOverviewFirst => Some(PageOverviewCursorMove::First),
            KeyAction::FsPageOverviewLast => Some(PageOverviewCursorMove::Last),
            _ => None,
        };
        if let Some(movement) = movement {
            return self.page_overview_cursor_input(ctx, movement);
        }
        let command = match action {
            KeyAction::FsPageOverviewToggle => Some(PageOverviewCommand::Cancel { stamp }),
            KeyAction::FsPageOverviewConfirm => view
                .capture_select(view.cursor)
                .map(PageOverviewCommand::Select),
            KeyAction::FsPageOverviewColumnsLess => {
                Some(PageOverviewCommand::ColumnsDelta { stamp, delta: -1 })
            }
            KeyAction::FsPageOverviewColumnsMore => {
                Some(PageOverviewCommand::ColumnsDelta { stamp, delta: 1 })
            }
            KeyAction::HelpShowContextShortcuts => {
                self.show_context_shortcuts_help = true;
                ctx.request_repaint();
                None
            }
            KeyAction::FsToggleWindowMode => {
                #[cfg(windows)]
                self.toggle_egui_viewer_window_mode_for_input(ctx);
                None
            }
            KeyAction::ToggleDetachedViewerMode => {
                self.toggle_detached_viewer_mode();
                None
            }
            KeyAction::ToggleAlwaysOnTop => {
                self.toggle_always_on_top(ctx, crate::app::ActionSurface::Viewer);
                None
            }
            KeyAction::FsCtrlNavPrev
            | KeyAction::FsCtrlNavNext
            | KeyAction::FsSiblingPrev
            | KeyAction::FsSiblingNext => {
                let Some(idx) = self.fullscreen_idx else {
                    return false;
                };
                let forward = matches!(action, KeyAction::FsCtrlNavNext | KeyAction::FsSiblingNext);
                self.close_page_overview_with_ctx(ctx);
                for _ in 0..press_count {
                    if matches!(action, KeyAction::FsCtrlNavPrev | KeyAction::FsCtrlNavNext) {
                        self.handle_fullscreen_ctrl_nav_context(ctx, idx, forward, false);
                    } else {
                        self.handle_fullscreen_sibling_nav_context(ctx, idx, forward, false);
                    }
                }
                None
            }
            _ => return false,
        };
        if let Some(command) = command {
            let body = self.page_overview_body(ctx);
            if matches!(command, PageOverviewCommand::ColumnsDelta { .. }) && !body.is_positive() {
                return false;
            }
            self.handle_page_overview_command(
                ctx,
                &command,
                body,
                self.page_overview_direction(),
                self.page_overview_active_context_serial(),
            )
        } else {
            true
        }
    }

    pub(super) fn handle_page_overview_keys(&mut self, ctx: &egui::Context) {
        let owner = self.keyboard_owner_for_pass(ctx);
        if self.ime_input_active(ctx) || owner.blocks_legacy_keymap_shortcuts() {
            return;
        }
        #[cfg(all(windows, feature = "test-script"))]
        crate::test_script::mark_action_pass_eligible(ctx);
        if let Some(permit) = owner.fullscreen_raw_key_permit() {
            if crate::keyboard_input::consume_fullscreen_raw_key(
                ctx,
                permit,
                egui::Modifiers::NONE,
                egui::Key::Escape,
            ) {
                self.close_page_overview_with_ctx(ctx);
                ctx.request_repaint();
                return;
            }
        }
        let actions = self.page_overview_executable_actions();
        if let Some((action, press_count)) =
            consume_page_overview_action(&self.keymap, ctx, &actions)
        {
            self.execute_page_overview_key_action_count(ctx, &action, press_count);
            return;
        }
        if let Some(permit) = owner.fullscreen_raw_key_permit() {
            for (key, movement) in [
                (egui::Key::ArrowLeft, PageOverviewCursorMove::Left),
                (egui::Key::ArrowRight, PageOverviewCursorMove::Right),
                (egui::Key::ArrowUp, PageOverviewCursorMove::Up),
                (egui::Key::ArrowDown, PageOverviewCursorMove::Down),
            ] {
                if crate::keyboard_input::consume_fullscreen_raw_key(
                    ctx,
                    permit,
                    egui::Modifiers::NONE,
                    key,
                ) {
                    self.page_overview_cursor_input(ctx, movement);
                    return;
                }
            }
        }
    }

    pub(crate) fn render_page_overview_surface(
        &mut self,
        ui: &mut egui::Ui,
        surface: egui::Rect,
    ) -> bool {
        let ctx = ui.ctx().clone();
        let Some(view) = self.page_overview_view() else {
            return false;
        };
        let direction = self.page_overview_direction();
        let geometry = crate::ui_page_overview::page_overview_geometry(surface, &view, direction);
        self.reflow_page_overview_to_surface(&ctx, ctx.viewport_id(), surface);
        let view = self
            .page_overview_view()
            .expect("reflow retains the session");
        let pages = page_overview_request_ordinals(&geometry.layout, view.scroll_anchor)
            .filter_map(|ordinal| view.projection.entry(ordinal).map(|entry| entry.items_idx))
            .collect::<Vec<_>>();
        self.ensure_non_pdf_page_overview_requests(&ctx, &pages);
        self.start_still_seek_rotations(&ctx, &pages);
        let thumbnails = self.page_overview_thumbnail_projection(&pages);
        let idx = self.fullscreen_idx;
        let current = idx.map_or_else(Vec::new, |idx| {
            match self.resolve_visible_spread_pair(idx) {
                SpreadPair::Double { left, right } => vec![left, right],
                SpreadPair::Single => vec![idx],
            }
        });
        let title = self
            .effective_folder()
            .map(|path| {
                path.file_name()
                    .unwrap_or_else(|| path.as_os_str())
                    .to_string_lossy()
                    .into_owned()
            })
            .unwrap_or_else(|| "ページ一覧".into());
        let enabled = !self.any_dialog_open()
            && !self.remote_session_blocks_local_control()
            && self.page_overview_active_context_serial().is_some()
            && ctx.viewport_id() == self.page_overview_viewer_viewport();
        let output = crate::ui_page_overview::draw_page_overview(
            ui,
            PageOverviewRenderInput {
                view: &view,
                title: &title,
                current_items_indices: &current,
                thumbnails: Some(&thumbnails),
                direction,
                input_enabled: enabled,
            },
        );
        for command in output.commands {
            let command = match command.action {
                PageOverviewUiAction::Select(target) => {
                    PageOverviewCommand::Select(PageOverviewSelect {
                        stamp: command.stamp,
                        target,
                    })
                }
                PageOverviewUiAction::Cancel => PageOverviewCommand::Cancel {
                    stamp: command.stamp,
                },
                PageOverviewUiAction::ScrollRows(delta) => PageOverviewCommand::ScrollRows {
                    stamp: command.stamp,
                    delta,
                },
                PageOverviewUiAction::ColumnsDelta(delta) => PageOverviewCommand::ColumnsDelta {
                    stamp: command.stamp,
                    delta,
                },
            };
            self.handle_page_overview_command(
                &ctx,
                &command,
                output.layout.viewport(),
                direction,
                self.page_overview_active_context_serial(),
            );
        }
        Self::draw_page_overview_window_close(ui, surface) && enabled
    }

    pub(crate) fn page_overview_thumbnail_projection(
        &self,
        pages: &[usize],
    ) -> HashMap<usize, PageOverviewThumbnail> {
        pages
            .iter()
            .filter_map(|&idx| {
                let thumbnail = match self.thumbnails.get(idx)? {
                    ThumbnailState::Loaded { .. } => {
                        let (texture, rotation) = self.page_overview_loaded_texture(idx)?;
                        page_overview_loaded_thumbnail(texture, rotation)
                    }
                    ThumbnailState::Failed => PageOverviewThumbnail::Failed,
                    _ => return None,
                };
                Some((idx, thumbnail))
            })
            .collect()
    }

    fn page_overview_loaded_texture(
        &self,
        idx: usize,
    ) -> Option<(&egui::TextureHandle, crate::rotation_db::Rotation)> {
        let ThumbnailState::Loaded { tex, .. } = self.thumbnails.get(idx)? else {
            return None;
        };
        page_overview_loaded_texture(
            tex,
            self.thumb_adjust_tex.get(&idx),
            self.rotation_cache.get(&idx).copied(),
            self.adjustment_dragging,
        )
    }

    /// Passive projection must retain the same evaluated handles used by the texture IDs,
    /// so eviction or context exchange cannot drop a drawn layer before its callback runs.
    pub(crate) fn page_overview_retained_thumbnail_textures(
        &self,
        pages: &[usize],
    ) -> Vec<egui::TextureHandle> {
        pages
            .iter()
            .filter_map(|&idx| {
                self.page_overview_loaded_texture(idx)
                    .map(|(texture, _)| texture.clone())
            })
            .collect()
    }
}

#[cfg(test)]
mod thumbnail_tests {
    use super::*;
    use crate::rotation_db::Rotation;

    fn texture(ctx: &egui::Context, name: &str, size: [usize; 2]) -> egui::TextureHandle {
        ctx.load_texture(
            name,
            egui::ColorImage::filled(size, egui::Color32::WHITE),
            egui::TextureOptions::LINEAR,
        )
    }

    #[test]
    fn overview_root_forwarded_geometry_and_viewport_migration_reflow_before_scroll_once() {
        use crate::page_overview::{
            PageOverviewEntry, PageOverviewMetadata, PageOverviewPage, PageOverviewProjection,
            PageOverviewStamp, PageOverviewState,
        };
        let mut app = crate::app::setup_app_for_test();
        app.fullscreen_idx = Some(120);
        app.viewer_presentation = ViewerPresentation::Fullscreen;
        let direction = app.page_overview_direction();
        let metadata = PageOverviewMetadata::new((0..200).map(|idx| PageOverviewEntry {
            items_idx: idx,
            page: Arc::new(PageOverviewPage {
                identity: crate::book_resume_db::PageIdentity::Image {
                    path: format!("g:/overview/{idx}.png"),
                },
                label: Arc::from(format!("page {idx}")),
                white_companion: None,
            }),
        }))
        .unwrap();
        let projection = Arc::new(
            PageOverviewProjection::new(Arc::new(metadata), Arc::new((0..200).collect())).unwrap(),
        );
        app.page_overview = PageOverviewState::open(
            projection,
            PageOverviewStamp {
                context_serial: app.page_overview_active_context_serial().unwrap(),
                items_generation: app.items_generation,
                reader_order_revision: 23,
                incarnation: 24,
            },
            120,
            4,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(720., 500.)),
            direction,
        )
        .unwrap();
        let ctx = egui::Context::default();
        let viewport = app.fullscreen_viewport_id();
        let child_input = |size| {
            let mut input = egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(320., 240.),
                )),
                ..Default::default()
            };
            input.viewports.insert(
                viewport,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    inner_rect: Some(egui::Rect::from_min_size(egui::pos2(100., 100.), size)),
                    ..Default::default()
                },
            );
            input
        };
        ctx.begin_pass(child_input(egui::vec2(1280., 720.)));
        let body = app.page_overview_body(&ctx);
        let view = app.page_overview_view().unwrap();
        let actual = crate::ui_page_overview::page_overview_geometry(
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280., 720.)),
            &view,
            direction,
        )
        .layout;
        let wrong_root =
            crate::ui_page_overview::page_overview_geometry(ctx.content_rect(), &view, direction)
                .layout;
        assert_eq!(body, actual.viewport());
        assert_eq!(
            actual.cursor_after_move(120, PageOverviewCursorMove::PageDown),
            Some(124)
        );
        assert_ne!(
            actual.cursor_after_move(120, PageOverviewCursorMove::PageDown),
            wrong_root.cursor_after_move(120, PageOverviewCursorMove::PageDown)
        );
        let _ = ctx.end_pass();

        // F11/linked F12 can keep this exact overview owner while replacing its viewport.
        app.viewer_presentation = ViewerPresentation::MainWindow;
        ctx.begin_pass(egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(240., 500.),
            )),
            ..Default::default()
        });
        let body = app.page_overview_body(&ctx);
        let view = app.page_overview_view().unwrap();
        let next = view.layout(body, direction);
        assert_eq!(next.columns(), 2);
        assert_eq!(view.scroll_anchor.row, 60);
        app.page_overview
            .session_mut()
            .unwrap()
            .scroll_rows(1, &next);
        assert_eq!(app.page_overview_view().unwrap().scroll_anchor.row, 61);
        let mut other_owner = view.clone();
        other_owner.stamp.context_serial += 1;
        assert!(App::page_overview_cached_layout(&ctx, &other_owner).is_none());
        let mut reopened = view.clone();
        reopened.stamp.incarnation += 1;
        assert!(App::page_overview_cached_layout(&ctx, &reopened).is_none());
        // The active paint must not apply the parked 4-column reflow a second time,
        // which would erase the just-applied scroll command.
        app.page_overview_body(&ctx);
        assert_eq!(app.page_overview_view().unwrap().scroll_anchor.row, 61);
        app.fullscreen_idx = None;
        assert!(
            app.reflow_page_overview_to_surface(
                &ctx,
                viewport,
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(720., 500.)),
            )
            .is_none()
        );
        assert_eq!(app.page_overview_view().unwrap().scroll_anchor.row, 61);
        let _ = ctx.end_pass();
    }

    #[test]
    fn passive_overview_resize_preserves_cursor_and_first_click_uses_reflowed_identity() {
        use crate::page_overview::{
            PageOverviewEntry, PageOverviewMetadata, PageOverviewPage, PageOverviewProjection,
            PageOverviewStamp, PageOverviewState,
        };
        let metadata = PageOverviewMetadata::new((0..200).map(|idx| PageOverviewEntry {
            items_idx: idx,
            page: Arc::new(PageOverviewPage {
                identity: crate::book_resume_db::PageIdentity::Image {
                    path: format!("g:/overview/{idx}.png"),
                },
                label: Arc::from(format!("page {idx}")),
                white_companion: None,
            }),
        }))
        .unwrap();
        let projection = Arc::new(
            PageOverviewProjection::new(Arc::new(metadata), Arc::new((0..200).collect())).unwrap(),
        );
        for direction in [
            PageOverviewDirection::LeftToRight,
            PageOverviewDirection::RightToLeft,
        ] {
            let ctx = egui::Context::default();
            let wide_surface = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(720., 500.));
            let narrow_surface =
                egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(240., 500.));
            let mut canonical = PageOverviewState::open(
                Arc::clone(&projection),
                PageOverviewStamp {
                    context_serial: 21,
                    items_generation: 22,
                    reader_order_revision: 23,
                    incarnation: 24,
                },
                120,
                4,
                wide_surface,
                direction,
            )
            .unwrap();
            let captured = canonical.session().unwrap().view().clone();
            let original =
                crate::ui_page_overview::page_overview_geometry(wide_surface, &captured, direction)
                    .layout;
            assert_eq!(original.columns(), 4);
            assert_eq!(captured.scroll_anchor.row, 30);
            assert_eq!(
                passive_page_overview_original_layout(&ctx, &captured, original).columns(),
                4
            );
            let effective = passive_page_overview_effective_view(
                &captured,
                &original,
                narrow_surface,
                direction,
            );
            let next = crate::ui_page_overview::page_overview_geometry(
                narrow_surface,
                &effective,
                direction,
            )
            .layout;
            assert_eq!(next.columns(), 2);
            assert!(
                next.visible_ordinals(effective.scroll_anchor)
                    .contains(&120)
            );
            assert!(!next.visible_ordinals(captured.scroll_anchor).contains(&120));
            assert_eq!(effective.stamp, captured.stamp);
            assert!(Arc::ptr_eq(&effective.projection, &captured.projection));
            assert_eq!(canonical.session().unwrap().view().scroll_anchor.row, 30);
            let movement = App::passive_overview_cursor_command(
                &effective,
                narrow_surface,
                direction,
                PageOverviewCursorMove::Down,
            )
            .unwrap();
            assert!(
                matches!(movement, PageOverviewCommand::SetCursor { stamp, target }
                if stamp == captured.stamp && target.ordinal == 122)
            );

            // Cell 121 is newly exposed by reflow, still has no generated thumbnail,
            // and must select its drawn metadata identity on the very first click.
            let point = next
                .cell_rect(121, effective.scroll_anchor)
                .unwrap()
                .center();
            let mut commands = Vec::new();
            for events in [
                vec![],
                vec![
                    egui::Event::PointerMoved(point),
                    egui::Event::PointerButton {
                        pos: point,
                        button: egui::PointerButton::Primary,
                        pressed: true,
                        modifiers: Default::default(),
                    },
                ],
                vec![egui::Event::PointerButton {
                    pos: point,
                    button: egui::PointerButton::Primary,
                    pressed: false,
                    modifiers: Default::default(),
                }],
            ] {
                let _ = ctx.run(
                    egui::RawInput {
                        screen_rect: Some(narrow_surface),
                        events,
                        ..Default::default()
                    },
                    |ctx| {
                        egui::CentralPanel::default()
                            .frame(egui::Frame::NONE)
                            .show(ctx, |ui| {
                                let original =
                                    App::page_overview_cached_layout(ctx, &captured).unwrap();
                                let view = passive_page_overview_effective_view(
                                    &captured,
                                    &original,
                                    ui.max_rect(),
                                    direction,
                                );
                                let output = crate::ui_page_overview::draw_page_overview(
                                    ui,
                                    PageOverviewRenderInput {
                                        view: &view,
                                        title: "passive resized book",
                                        current_items_indices: &[120],
                                        thumbnails: None,
                                        direction,
                                        input_enabled: true,
                                    },
                                );
                                commands.extend(
                                    output
                                        .commands
                                        .into_iter()
                                        .map(App::page_overview_ui_command),
                                );
                            });
                    },
                );
            }
            assert_eq!(
                commands,
                vec![PageOverviewCommand::Select(
                    captured.capture_select(121).unwrap(),
                )]
            );
            let cached = App::page_overview_cached_layout(&ctx, &captured).unwrap();
            assert_eq!(cached.columns(), 4);
            // A refreshed passive snapshot must not replace the canonical origin with
            // its already resized placement while the parked session still has row 30.
            assert_eq!(
                passive_page_overview_original_layout(&ctx, &captured, next).columns(),
                4
            );
            canonical.session_mut().unwrap().reflow(&cached, &next);
            assert_eq!(
                canonical.session().unwrap().view().scroll_anchor,
                effective.scroll_anchor
            );
            assert_eq!(canonical.session().unwrap().view().cursor, 120);
        }
    }

    #[test]
    fn overview_unknown_rotation_waits_before_projecting_evaluated_thumbnail() {
        let ctx = egui::Context::default();
        let raw = texture(&ctx, "overview-raw", [20, 40]);
        let evaluated = texture(&ctx, "overview-evaluated", [12, 30]);
        assert!(page_overview_loaded_texture(&raw, Some(&evaluated), None, false).is_none());
        let (selected, rotation) =
            page_overview_loaded_texture(&raw, Some(&evaluated), Some(Rotation::Cw90), false)
                .unwrap();
        let PageOverviewThumbnail::Loaded {
            texture_id,
            display_size,
            uv_corners,
        } = page_overview_loaded_thumbnail(selected, rotation)
        else {
            panic!("resolved texture must project as Loaded");
        };
        assert_eq!(texture_id, evaluated.id());
        assert_eq!(display_size, egui::vec2(30.0, 12.0));
        assert_eq!(
            uv_corners,
            [
                egui::pos2(0., 1.),
                egui::pos2(0., 0.),
                egui::pos2(1., 0.),
                egui::pos2(1., 1.),
            ]
        );
    }

    #[test]
    fn overview_evaluated_layer_and_drag_policy_match_grid_and_retained_handle() {
        let ctx = egui::Context::default();
        // The raw handle can itself be a worker-composited edit preview. An evaluated
        // handle supersedes it, and its own cropped dimensions must drive painting.
        let raw = texture(&ctx, "overview-edit-preview", [20, 40]);
        let evaluated = texture(&ctx, "overview-evaluated-preview", [12, 30]);
        for (layer, dragging, expected) in [
            (Some(&evaluated), false, &evaluated),
            (Some(&evaluated), true, &raw),
            (None, false, &raw),
        ] {
            let (selected, rotation) =
                page_overview_loaded_texture(&raw, layer, Some(Rotation::None), dragging).unwrap();
            let retained = selected.clone();
            let PageOverviewThumbnail::Loaded {
                texture_id,
                display_size,
                ..
            } = page_overview_loaded_thumbnail(selected, rotation)
            else {
                panic!("resolved texture must project as Loaded");
            };
            assert_eq!(texture_id, expected.id());
            assert_eq!(retained.id(), texture_id);
            assert_eq!(display_size, expected.size_vec2());
        }
    }

    #[test]
    fn overview_request_order_prioritizes_visible_cells_before_bounded_overscan() {
        let layout = crate::page_overview::PageOverviewLayout::new(
            50_000,
            4,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(416.0, 250.0)),
            PageOverviewDirection::LeftToRight,
        );
        let anchor = crate::page_overview::PageOverviewScrollAnchor { row: 100 };
        let visible = layout.visible_ordinals(anchor);
        let overscan = layout.overscan_ordinals(anchor);
        let requests = page_overview_request_ordinals(&layout, anchor).collect::<Vec<_>>();
        assert_eq!(
            &requests[..visible.len()],
            visible.clone().collect::<Vec<_>>()
        );
        assert_eq!(
            &requests[visible.len()..],
            overscan
                .clone()
                .filter(|ordinal| !visible.contains(ordinal))
                .collect::<Vec<_>>()
        );
        assert_eq!(requests.len(), overscan.len());
        assert_eq!(
            requests.iter().copied().collect::<HashSet<_>>().len(),
            requests.len()
        );
        assert!(requests.len() <= visible.len() + layout.columns() * 2);
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    struct ClearOverviewKeyFrame;
    impl Drop for ClearOverviewKeyFrame {
        fn drop(&mut self) {
            crate::key_input::clear_test_frame();
        }
    }

    fn control_down_edge(repeat: bool) -> crate::key_input::KeyEdge {
        crate::key_input::KeyEdge {
            source_hwnd: 1,
            source_viewport: egui::ViewportId::ROOT,
            virtual_key: 0x28,
            scan_code: 0x50,
            extended: true,
            pressed: true,
            repeat,
            ctrl: true,
            shift: false,
            alt: false,
        }
    }

    fn key_pass(ctx: &egui::Context, edges: Vec<crate::key_input::KeyEdge>) {
        ctx.begin_pass(Default::default());
        let owner = crate::keyboard_input::decide_keyboard_owner(
            crate::keyboard_input::KeyboardOwnershipSnapshot {
                viewport: ctx.viewport_id(),
                viewport_focused: true,
                modal: false,
                text_input: None,
                focused_ui: None,
                shortcut_scope: Some(crate::keyboard_input::ShortcutScope::new(
                    ctx.viewport_id(),
                    crate::keyboard_input::ShortcutSurface::Fullscreen,
                )),
            },
        );
        crate::keyboard_input::cache_keyboard_owner(ctx, owner);
        crate::key_input::set_test_frame_for_viewport(ctx.viewport_id(), edges);
    }

    #[test]
    fn overview_shared_collector_preserves_same_frame_nav_press_count_and_repeat_policy() {
        let _guard = crate::key_input::lock_test_input();
        let _clear = ClearOverviewKeyFrame;
        let ctx = egui::Context::default();
        let keymap = crate::keymap::Keymap::empty();
        key_pass(
            &ctx,
            vec![
                control_down_edge(false),
                control_down_edge(false),
                control_down_edge(false),
            ],
        );
        assert_eq!(
            consume_page_overview_action(
                &keymap,
                &ctx,
                crate::keymap::FS_PAGE_OVERVIEW_ACTION_PRIORITY
            ),
            Some((KeyAction::FsCtrlNavNext, 3))
        );
        assert_eq!(
            consume_page_overview_action(
                &keymap,
                &ctx,
                crate::keymap::FS_PAGE_OVERVIEW_ACTION_PRIORITY
            ),
            None
        );
        let _ = ctx.end_pass();
        key_pass(
            &ctx,
            vec![
                control_down_edge(false),
                control_down_edge(true),
                control_down_edge(true),
            ],
        );
        assert_eq!(
            consume_page_overview_action(
                &keymap,
                &ctx,
                crate::keymap::FS_PAGE_OVERVIEW_ACTION_PRIORITY
            ),
            Some((KeyAction::FsCtrlNavNext, 1))
        );
        let _ = ctx.end_pass();
    }

    #[test]
    fn overview_shared_collector_override_priority_consumes_toggle_before_counted_nav() {
        let _guard = crate::key_input::lock_test_input();
        let _clear = ClearOverviewKeyFrame;
        let ctx = egui::Context::default();
        let keymap =
            crate::keymap::Keymap::from_ini_str("[FsCommon]\nFsPageOverviewToggle = Ctrl+Down\n");
        key_pass(
            &ctx,
            vec![control_down_edge(false), control_down_edge(false)],
        );
        assert_eq!(
            consume_page_overview_action(
                &keymap,
                &ctx,
                crate::keymap::FS_PAGE_OVERVIEW_ACTION_PRIORITY
            ),
            Some((KeyAction::FsPageOverviewToggle, 1))
        );
        assert_eq!(
            consume_page_overview_action(
                &keymap,
                &ctx,
                crate::keymap::FS_PAGE_OVERVIEW_ACTION_PRIORITY
            ),
            None
        );
        let _ = ctx.end_pass();
    }

    #[test]
    fn overview_shared_collector_runtime_gate_does_not_enable_an_excluded_nav_action() {
        let _guard = crate::key_input::lock_test_input();
        let _clear = ClearOverviewKeyFrame;
        let ctx = egui::Context::default();
        key_pass(
            &ctx,
            vec![control_down_edge(false), control_down_edge(false)],
        );
        assert_eq!(
            consume_page_overview_action(
                &crate::keymap::Keymap::empty(),
                &ctx,
                &[KeyAction::FsPageOverviewConfirm]
            ),
            None
        );
        assert!(crate::key_input::pressed_key_down(
            ctx.viewport_id(),
            |edge| edge.virtual_key == 0x28
        ));
        let _ = ctx.end_pass();
    }
}

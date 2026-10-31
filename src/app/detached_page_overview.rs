//! Stamped overview commands share the existing detached activation owner.
#![cfg(windows)]

use super::page_overview::PageOverviewCommand;
use super::*;
#[cfg(test)]
use crate::page_overview::PageOverviewDirection;
use crate::page_overview::{PageOverviewStamp, PageOverviewView};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DetachedPageOverviewInputProvenance {
    Pointer,
    Key,
}

/// At most one scroll delta, one column delta, one cursor target and one final action.
/// All entries belong to one immutable overview incarnation.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct IdentifiedPageOverviewCommand {
    commands: Vec<PageOverviewCommand>,
}

impl IdentifiedPageOverviewCommand {
    pub(super) fn new(command: PageOverviewCommand) -> Self {
        Self {
            commands: vec![command],
        }
    }

    pub(super) fn commands(&self) -> &[PageOverviewCommand] {
        &self.commands
    }

    pub(super) fn stamp(&self) -> PageOverviewStamp {
        self.commands[0].stamp()
    }

    pub(super) fn push(&mut self, command: PageOverviewCommand) -> bool {
        if command.stamp() != self.stamp()
            || self.commands.iter().any(|command| {
                matches!(
                    command,
                    PageOverviewCommand::Select(_)
                        | PageOverviewCommand::Cancel { .. }
                        | PageOverviewCommand::Action { .. }
                )
            })
        {
            return false;
        }
        match &command {
            PageOverviewCommand::ScrollRows { delta, .. } => {
                if let Some(PageOverviewCommand::ScrollRows { delta: total, .. }) = self
                    .commands
                    .iter_mut()
                    .find(|command| matches!(command, PageOverviewCommand::ScrollRows { .. }))
                {
                    *total = total.saturating_add(*delta);
                    return true;
                }
            }
            PageOverviewCommand::ColumnsDelta { delta, .. } => {
                if let Some(PageOverviewCommand::ColumnsDelta { delta: total, .. }) = self
                    .commands
                    .iter_mut()
                    .find(|command| matches!(command, PageOverviewCommand::ColumnsDelta { .. }))
                {
                    *total = total.saturating_add(*delta);
                    return true;
                }
            }
            PageOverviewCommand::SetCursor { .. } => {
                if let Some(previous) = self
                    .commands
                    .iter_mut()
                    .find(|command| matches!(command, PageOverviewCommand::SetCursor { .. }))
                {
                    *previous = command;
                    return true;
                }
            }
            PageOverviewCommand::Select(_)
            | PageOverviewCommand::Cancel { .. }
            | PageOverviewCommand::Action { .. } => {}
        }
        self.commands.push(command);
        debug_assert!(self.commands.len() <= 4);
        true
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum OverviewInputDisposition {
    Reject,
    Execute,
    Activate,
}

/// Foreground is evidence for a new explicit pointer activation only, never proof
/// of Select execution eligibility or a substitute for a delivered key edge.
fn overview_input_disposition(
    provenance: DetachedPageOverviewInputProvenance,
    actual_active_owner: bool,
    owned_host_foreground: bool,
    native_activation_pending: bool,
) -> OverviewInputDisposition {
    if actual_active_owner {
        OverviewInputDisposition::Execute
    } else if provenance == DetachedPageOverviewInputProvenance::Pointer
        && (owned_host_foreground || native_activation_pending)
    {
        OverviewInputDisposition::Activate
    } else {
        OverviewInputDisposition::Reject
    }
}

fn overview_command_matches_view(view: &PageOverviewView, command: &PageOverviewCommand) -> bool {
    view.stamp == command.stamp()
        && match command {
            PageOverviewCommand::Select(selection) => {
                view.capture_select(selection.target.ordinal).as_ref() == Some(selection)
            }
            PageOverviewCommand::SetCursor { stamp, target } => view
                .capture_select(target.ordinal)
                .is_some_and(|selection| selection.stamp == *stamp && selection.target == *target),
            PageOverviewCommand::Action {
                action,
                press_count,
                ..
            } => {
                crate::keymap::page_overview_action_allowed(*action)
                    && PageOverviewCommand::action_count_is_valid(*action, *press_count)
            }
            _ => true,
        }
}

impl App {
    fn detached_page_overview_input_blocked(&self, window_id: u64) -> bool {
        self.remote_session_blocks_local_control()
            || self.document_open_modal_admission_blocked()
            || self.any_modal_dialog_open_for_fullscreen_keys()
            || self.sidecar_restore_blocks_window(window_id)
            || self.detached_window_state(window_id) == Some(DetachedWindowState::Closing)
    }

    fn detached_page_overview_has_foreground(&self, window_id: u64) -> bool {
        self.detached_window_hwnd_alive_for_window_id(window_id)
            .is_some_and(|hwnd| crate::video::native_window::foreground_hwnd() == hwnd)
    }

    fn detached_page_overview_active_binding_matches(
        &self,
        window_id: u64,
        stamp: PageOverviewStamp,
    ) -> bool {
        self.active_detached_window_id() == Some(window_id)
            && self
                .locate_window_context(window_id)
                .is_some_and(|(owner, residence)| {
                    owner.serial() == stamp.context_serial
                        && matches!(
                            residence,
                            ContextResidence::Mounted | ContextResidence::AtRest
                        )
                })
    }

    fn detached_page_overview_command_is_current(
        &self,
        window_id: u64,
        command: &PageOverviewCommand,
    ) -> bool {
        let stamp = command.stamp();
        let Some((owner, residence)) = self.locate_window_context(window_id) else {
            return false;
        };
        if owner.serial() != stamp.context_serial
            || !matches!(
                residence,
                ContextResidence::Mounted | ContextResidence::AtRest
            )
        {
            return false;
        }
        self.with_viewer_context_ref(owner, |context| {
            context.items_generation() == stamp.items_generation
                && context
                    .page_overview_view()
                    .is_some_and(|view| overview_command_matches_view(&view, command))
        })
        .unwrap_or(false)
    }

    /// The callback has already captured a stable identity from its drawn layout.
    /// Keys never manufacture a new activation; the native watcher may have activated
    /// their owner first, otherwise the delivered command is discarded immediately.
    pub(crate) fn queue_detached_page_overview_command(
        &mut self,
        window_id: u64,
        command: PageOverviewCommand,
        provenance: DetachedPageOverviewInputProvenance,
    ) -> bool {
        if self.detached_page_overview_input_blocked(window_id)
            || !self.detached_page_overview_command_is_current(window_id, &command)
        {
            return false;
        }
        let disposition = overview_input_disposition(
            provenance,
            self.detached_page_overview_active_binding_matches(window_id, command.stamp()),
            self.detached_page_overview_has_foreground(window_id),
            self.detached_window_manager
                .pending_activation_is_activate_only(window_id),
        );
        if disposition == OverviewInputDisposition::Reject {
            return false;
        }
        let default_linked = self.detached_window_runtime_default_linked();
        self.detached_window_manager.queue_page_overview_command(
            window_id,
            command,
            disposition == OverviewInputDisposition::Activate,
            default_linked,
        )
    }

    /// Recheck at commit: an older pointer must not activate A after B owns foreground.
    /// A temporary context mount supplies no permission to activate or select.
    pub(crate) fn can_activate_detached_page_overview_command(
        &self,
        window_id: u64,
        command: &IdentifiedPageOverviewCommand,
    ) -> bool {
        !self.detached_page_overview_input_blocked(window_id)
            && self
                .detached_image_windows
                .iter()
                .any(|window| window.id == window_id)
            && (self.detached_page_overview_has_foreground(window_id)
                || self.detached_page_overview_active_binding_matches(window_id, command.stamp()))
            && command
                .commands()
                .iter()
                .all(|command| self.detached_page_overview_command_is_current(window_id, command))
    }

    /// Call within the existing active-context mount, before its ordinary input/render.
    /// Pending execution is consumed once even when a modal or newer owner rejects it.
    pub(crate) fn execute_pending_page_overview_command_in_mounted_context(
        &mut self,
        ctx: &egui::Context,
    ) -> bool {
        let active_id = self.active_detached_window_id();
        for window_id in self
            .detached_window_manager
            .pending_page_overview_execution_window_ids()
        {
            if Some(window_id) != active_id {
                self.detached_window_manager
                    .discard_page_overview_command(window_id);
            }
        }
        let Some(window_id) = active_id else {
            return false;
        };
        let Some(batch) = self
            .detached_window_manager
            .take_pending_page_overview_execution(window_id)
        else {
            return false;
        };
        let active_context = self.page_overview_active_context_serial();
        if self.detached_viewer_window_id() != Some(window_id)
            || active_context != Some(batch.stamp().context_serial)
            || self.detached_page_overview_input_blocked(window_id)
            || !self.page_overview_action_stamp_is_current(batch.stamp())
            || self.page_overview_view().is_none_or(|view| {
                !batch
                    .commands()
                    .iter()
                    .all(|command| overview_command_matches_view(&view, command))
            })
        {
            return false;
        }
        let viewport_id = Self::detached_image_window_viewport_id(window_id);
        let size = Self::page_overview_surface_for_viewport(ctx, viewport_id)
            .map(|surface| surface.size())
            .or_else(|| {
                self.detached_window_runtime_placement(window_id)
                    .map(|placement| {
                        Self::detached_snapshot_client_size(placement)
                            / crate::settings::normalize_ui_scale_factor(
                                self.settings.ui_scale_factor,
                            )
                    })
            });
        let Some(size) = size else { return false };
        let surface = egui::Rect::from_min_size(egui::Pos2::ZERO, size);
        if self
            .reflow_page_overview_to_surface(ctx, viewport_id, surface)
            .is_none()
        {
            return false;
        }
        let direction = self.page_overview_direction();
        let mut applied = false;
        for command in batch.commands() {
            let Some(view) = self.page_overview_view() else {
                break;
            };
            let viewport =
                crate::ui_page_overview::page_overview_geometry(surface, &view, direction)
                    .layout
                    .viewport();
            applied |= self.handle_page_overview_command(
                ctx,
                command,
                viewport,
                direction,
                active_context,
            );
        }
        if applied {
            ctx.request_repaint_of(viewport_id);
        }
        applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stamp() -> PageOverviewStamp {
        PageOverviewStamp {
            context_serial: 7,
            items_generation: 8,
            reader_order_revision: 9,
            incarnation: 10,
        }
    }

    #[test]
    fn overview_delivery_checks_incarnation_revision_and_stable_identity_without_textures() {
        use crate::page_overview::{
            PageOverviewEntry, PageOverviewMetadata, PageOverviewPage, PageOverviewProjection,
            PageOverviewState,
        };
        let metadata = Arc::new(
            PageOverviewMetadata::new((0..3).map(|idx| PageOverviewEntry {
                items_idx: idx,
                page: Arc::new(PageOverviewPage {
                    identity: crate::book_resume_db::PageIdentity::Image {
                        path: format!("g:/disposable-overview/{idx}.png"),
                    },
                    label: Arc::from(format!("Page {idx}")),
                    white_companion: None,
                }),
            }))
            .unwrap(),
        );
        let projection =
            Arc::new(PageOverviewProjection::new(metadata, Arc::new(vec![2, 0, 1])).unwrap());
        let state = PageOverviewState::open(
            projection,
            stamp(),
            0,
            3,
            egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(400.0, 300.0)),
            PageOverviewDirection::LeftToRight,
        )
        .unwrap();
        let view = state.session().unwrap().view();
        let selection = view.capture_select(0).unwrap();
        assert_eq!(selection.target.items_idx, 2);
        assert!(overview_command_matches_view(
            view,
            &PageOverviewCommand::Select(selection.clone())
        ));
        let mut wrong_idx = selection.clone();
        wrong_idx.target.items_idx = 0;
        assert!(!overview_command_matches_view(
            view,
            &PageOverviewCommand::Select(wrong_idx)
        ));
        let mut wrong_identity = selection;
        wrong_identity.target.identity = crate::book_resume_db::PageIdentity::Image {
            path: "g:/disposable-overview/other.png".into(),
        };
        assert!(!overview_command_matches_view(
            view,
            &PageOverviewCommand::Select(wrong_identity)
        ));
        let mut stale = stamp();
        stale.incarnation += 1;
        assert!(!overview_command_matches_view(
            view,
            &PageOverviewCommand::Cancel { stamp: stale }
        ));
        stale = stamp();
        stale.reader_order_revision += 1;
        assert!(!overview_command_matches_view(
            view,
            &PageOverviewCommand::ScrollRows {
                stamp: stale,
                delta: 1,
            }
        ));
        stale = stamp();
        stale.items_generation += 1;
        assert!(!overview_command_matches_view(
            view,
            &PageOverviewCommand::ColumnsDelta {
                stamp: stale,
                delta: 1,
            }
        ));
    }

    #[test]
    fn old_key_after_b_resume_cannot_create_a_activation() {
        assert_eq!(
            overview_input_disposition(DetachedPageOverviewInputProvenance::Key, false, true, true),
            OverviewInputDisposition::Reject
        );
        assert_eq!(
            overview_input_disposition(
                DetachedPageOverviewInputProvenance::Key,
                true,
                false,
                false
            ),
            OverviewInputDisposition::Execute
        );
    }

    #[test]
    fn passive_overview_action_retains_count_and_never_reactivates_an_inactive_owner() {
        let command = PageOverviewCommand::Action {
            stamp: stamp(),
            action: crate::keymap::KeyAction::FsCtrlNavNext,
            press_count: 3,
        };
        let batch = IdentifiedPageOverviewCommand::new(command.clone());
        assert_eq!(batch.commands(), &[command]);
        assert!(matches!(
            batch.commands()[0],
            PageOverviewCommand::Action { press_count: 3, .. }
        ));
        assert_eq!(
            overview_input_disposition(DetachedPageOverviewInputProvenance::Key, false, true, true),
            OverviewInputDisposition::Reject
        );
        assert!(!PageOverviewCommand::action_count_is_valid(
            crate::keymap::KeyAction::FsToggleWindowMode,
            3
        ));
        assert!(!PageOverviewCommand::action_count_is_valid(
            crate::keymap::KeyAction::FsCtrlNavNext,
            0
        ));
    }

    #[test]
    fn passive_pointer_requires_owned_foreground_or_existing_native_activation() {
        for (foreground, pending) in [(true, false), (false, true)] {
            assert_eq!(
                overview_input_disposition(
                    DetachedPageOverviewInputProvenance::Pointer,
                    false,
                    foreground,
                    pending
                ),
                OverviewInputDisposition::Activate
            );
        }
        assert_eq!(
            overview_input_disposition(
                DetachedPageOverviewInputProvenance::Pointer,
                false,
                false,
                false
            ),
            OverviewInputDisposition::Reject
        );
        assert_eq!(
            overview_input_disposition(
                DetachedPageOverviewInputProvenance::Pointer,
                true,
                false,
                false
            ),
            OverviewInputDisposition::Execute
        );
    }

    #[test]
    fn overview_batch_replaces_cursor_target_and_does_not_replay_terminal_action() {
        let target = |idx| crate::page_overview::PageOverviewTarget {
            ordinal: idx,
            items_idx: idx,
            identity: crate::book_resume_db::PageIdentity::Image {
                path: format!("page/{idx}.png"),
            },
        };
        let mut batch = IdentifiedPageOverviewCommand::new(PageOverviewCommand::ScrollRows {
            stamp: stamp(),
            delta: 2,
        });
        assert!(batch.push(PageOverviewCommand::SetCursor {
            stamp: stamp(),
            target: target(1)
        }));
        assert!(batch.push(PageOverviewCommand::SetCursor {
            stamp: stamp(),
            target: target(3)
        }));
        assert!(batch.push(PageOverviewCommand::ColumnsDelta {
            stamp: stamp(),
            delta: -1
        }));
        assert!(batch.push(PageOverviewCommand::Action {
            stamp: stamp(),
            action: crate::keymap::KeyAction::FsToggleWindowMode,
            press_count: 1
        }));
        assert_eq!(batch.commands().len(), 4);
        assert!(
            matches!(&batch.commands()[1], PageOverviewCommand::SetCursor { target, .. } if target.ordinal == 3)
        );
        assert!(!batch.push(PageOverviewCommand::Action {
            stamp: stamp(),
            action: crate::keymap::KeyAction::ToggleAlwaysOnTop,
            press_count: 1
        }));
        assert!(!batch.push(PageOverviewCommand::SetCursor {
            stamp: stamp(),
            target: target(4)
        }));
    }

    #[test]
    fn overview_batch_is_bounded_coalesces_deltas_and_keeps_one_terminal_command() {
        let mut batch = IdentifiedPageOverviewCommand::new(PageOverviewCommand::ScrollRows {
            stamp: stamp(),
            delta: isize::MAX,
        });
        for _ in 0..1000 {
            assert!(batch.push(PageOverviewCommand::ScrollRows {
                stamp: stamp(),
                delta: 1
            }));
            assert!(batch.push(PageOverviewCommand::ColumnsDelta {
                stamp: stamp(),
                delta: 1
            }));
        }
        assert_eq!(batch.commands().len(), 2);
        assert!(matches!(
            batch.commands()[0],
            PageOverviewCommand::ScrollRows {
                delta: isize::MAX,
                ..
            }
        ));
        assert!(matches!(
            batch.commands()[1],
            PageOverviewCommand::ColumnsDelta { delta: i8::MAX, .. }
        ));
        let mut stale = stamp();
        stale.incarnation += 1;
        assert!(!batch.push(PageOverviewCommand::Cancel { stamp: stale }));
        assert!(batch.push(PageOverviewCommand::Cancel { stamp: stamp() }));
        assert!(!batch.push(PageOverviewCommand::Cancel { stamp: stamp() }));
        assert!(!batch.push(PageOverviewCommand::ScrollRows {
            stamp: stamp(),
            delta: -1
        }));
        assert_eq!(batch.commands().len(), 3);
    }
}

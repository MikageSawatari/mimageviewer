//! Pass-scoped keyboard ownership.
//!
//! The decision itself is deliberately pure. `App` is responsible for taking one
//! [`KeyboardOwnershipSnapshot`] from egui and application state per viewport pass,
//! then caching the selected owner for the existing keymap paths.

/// The phase in which a text input owns the keyboard.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TextInputPhase {
    /// `request_focus` was issued, but egui has not focused the widget yet.
    PendingFocus,
    /// The text widget has keyboard focus.
    Focused,
    /// The existing 300 ms IME event grace is active.
    ImeGrace,
}

/// The application surface whose shortcuts are eligible in this viewport pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutSurface {
    Main,
    Fullscreen,
}

/// Scope carried by an application-shortcut owner and its future permit.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShortcutScope {
    pub viewport: egui::ViewportId,
    pub surface: ShortcutSurface,
}

impl ShortcutScope {
    pub const fn new(viewport: egui::ViewportId, surface: ShortcutSurface) -> Self {
        Self { viewport, surface }
    }
}

/// The single keyboard owner selected for one viewport pass.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyboardOwner {
    Modal,
    TextInput {
        viewport: egui::ViewportId,
        widget_id: egui::Id,
        phase: TextInputPhase,
    },
    FocusedUi {
        viewport: egui::ViewportId,
        widget_id: egui::Id,
    },
    ApplicationShortcut {
        scope: ShortcutScope,
    },
    Unclaimed,
}

/// Proof that the pass owner permits application shortcuts.
///
/// The field is private and there is no public constructor. The only issuance
/// path is [`KeyboardOwner::shortcut_permit`], and it succeeds only for
/// [`KeyboardOwner::ApplicationShortcut`]. S4 will make consume APIs require it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ShortcutPermit {
    scope: ShortcutScope,
}

impl ShortcutPermit {
    pub const fn scope(self) -> ShortcutScope {
        self.scope
    }
}

impl KeyboardOwner {
    pub const fn shortcut_permit(self) -> Option<ShortcutPermit> {
        match self {
            Self::ApplicationShortcut { scope } => Some(ShortcutPermit { scope }),
            Self::Modal | Self::TextInput { .. } | Self::FocusedUi { .. } | Self::Unclaimed => None,
        }
    }

    /// S3 compatibility projection for `App::shortcuts_blocked_by_text_input`.
    ///
    /// Pending focus is intentionally not activated as a new behavior in S3.
    /// S4 will switch callers from this legacy projection to `ShortcutPermit`.
    pub(crate) const fn blocks_legacy_main_shortcuts(self) -> bool {
        match self {
            Self::Modal => true,
            Self::TextInput {
                phase: TextInputPhase::Focused | TextInputPhase::ImeGrace,
                ..
            } => true,
            Self::ApplicationShortcut { scope } => {
                matches!(scope.surface, ShortcutSurface::Fullscreen)
            }
            Self::TextInput {
                phase: TextInputPhase::PendingFocus,
                ..
            }
            | Self::FocusedUi { .. }
            | Self::Unclaimed => false,
        }
    }

    /// S3 compatibility projection for the S1 `wants_keyboard_input` checks.
    ///
    /// Focused and IME text owners also block here because they can outrank the
    /// generic focused-UI claim that came from `Context::wants_keyboard_input`.
    /// Pending focus is not activated until S4. Modal and App-tracked states
    /// retain their existing outer gates in S3. Missing pass ownership is
    /// handled separately as fail-closed by `keymap_owner_blocks_shortcuts`.
    pub(crate) const fn blocks_legacy_keymap_shortcuts(self) -> bool {
        matches!(
            self,
            Self::TextInput {
                phase: TextInputPhase::Focused | TextInputPhase::ImeGrace,
                ..
            } | Self::FocusedUi { .. }
        )
    }
}

/// Pure inputs used to decide a viewport pass's keyboard owner.
///
/// This contains only values copied from application/egui state. In
/// particular, it deliberately has no draft/session field such as
/// `BookBookmarkTitleEdit`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct KeyboardOwnershipSnapshot {
    pub viewport: egui::ViewportId,
    pub viewport_focused: bool,
    pub modal: bool,
    pub focused_text_input: Option<egui::Id>,
    pub pending_text_input: Option<egui::Id>,
    pub ime_grace: bool,
    pub focused_ui: Option<egui::Id>,
    pub shortcut_scope: Option<ShortcutScope>,
}

/// Decide the one keyboard owner for a viewport pass without reading egui or
/// application state.
pub const fn decide_keyboard_owner(snapshot: KeyboardOwnershipSnapshot) -> KeyboardOwner {
    if snapshot.modal {
        return KeyboardOwner::Modal;
    }
    if let Some(widget_id) = snapshot.focused_text_input {
        return KeyboardOwner::TextInput {
            viewport: snapshot.viewport,
            widget_id,
            phase: TextInputPhase::Focused,
        };
    }
    if snapshot.ime_grace {
        return KeyboardOwner::TextInput {
            viewport: snapshot.viewport,
            widget_id: match snapshot.focused_ui {
                Some(widget_id) => widget_id,
                None => egui::Id::NULL,
            },
            phase: TextInputPhase::ImeGrace,
        };
    }
    if let Some(widget_id) = snapshot.pending_text_input {
        return KeyboardOwner::TextInput {
            viewport: snapshot.viewport,
            widget_id,
            phase: TextInputPhase::PendingFocus,
        };
    }
    if let Some(widget_id) = snapshot.focused_ui {
        return KeyboardOwner::FocusedUi {
            viewport: snapshot.viewport,
            widget_id,
        };
    }
    if let Some(scope) = snapshot.shortcut_scope {
        if snapshot.viewport_focused || matches!(scope.surface, ShortcutSurface::Fullscreen) {
            return KeyboardOwner::ApplicationShortcut { scope };
        }
    }
    KeyboardOwner::Unclaimed
}

/// One explicit focus request awaiting egui focus ownership.
///
/// This is the only pending-focus state stored by `App`; no per-widget bools or
/// draft-state sentinels participate in keyboard ownership.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingTextInputFocusClaim {
    pub(crate) viewport: egui::ViewportId,
    pub(crate) widget_id: egui::Id,
    pub(crate) issued_pass: u64,
}

impl PendingTextInputFocusClaim {
    pub(crate) const fn new(
        viewport: egui::ViewportId,
        widget_id: egui::Id,
        issued_pass: u64,
    ) -> Self {
        Self {
            viewport,
            widget_id,
            issued_pass,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PendingFocusEvent {
    ObservePass {
        viewport: egui::ViewportId,
        pass: u64,
        focused_widget: Option<egui::Id>,
    },
    EditingEnded,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct PendingFocusTransition {
    pub(crate) claim: Option<PendingTextInputFocusClaim>,
    pub(crate) ownership: Option<(egui::ViewportId, egui::Id, TextInputPhase)>,
}

/// Advance a pending focus claim without reading egui.
///
/// A claim is released when the target receives focus, another widget receives
/// focus, editing ends explicitly, or the one protected pass finishes without
/// focus. Passes from another viewport neither claim ownership nor age it.
pub(crate) fn transition_pending_focus_claim(
    claim: Option<PendingTextInputFocusClaim>,
    event: PendingFocusEvent,
) -> PendingFocusTransition {
    let Some(claim) = claim else {
        return PendingFocusTransition {
            claim: None,
            ownership: None,
        };
    };
    let PendingFocusEvent::ObservePass {
        viewport,
        pass,
        focused_widget,
    } = event
    else {
        return PendingFocusTransition {
            claim: None,
            ownership: None,
        };
    };
    if viewport != claim.viewport {
        return PendingFocusTransition {
            claim: Some(claim),
            ownership: None,
        };
    }
    if let Some(focused_widget) = focused_widget {
        if focused_widget == claim.widget_id {
            return PendingFocusTransition {
                claim: None,
                ownership: Some((claim.viewport, claim.widget_id, TextInputPhase::Focused)),
            };
        }
        return PendingFocusTransition {
            claim: None,
            ownership: None,
        };
    }
    if pass < claim.issued_pass || pass > claim.issued_pass.saturating_add(1) {
        return PendingFocusTransition {
            claim: None,
            ownership: None,
        };
    }
    PendingFocusTransition {
        claim: Some(claim),
        ownership: Some((
            claim.viewport,
            claim.widget_id,
            TextInputPhase::PendingFocus,
        )),
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct CachedKeyboardOwner {
    pass: u64,
    owner: KeyboardOwner,
}

fn owner_cache_id(viewport: egui::ViewportId) -> egui::Id {
    egui::Id::new(("keyboard_owner_for_pass", viewport))
}

pub(crate) fn cached_keyboard_owner(ctx: &egui::Context) -> Option<KeyboardOwner> {
    let viewport = ctx.viewport_id();
    let pass = ctx.cumulative_pass_nr();
    ctx.data(|data| {
        data.get_temp::<CachedKeyboardOwner>(owner_cache_id(viewport))
            .filter(|cached| cached.pass == pass)
            .map(|cached| cached.owner)
    })
}

pub(crate) fn cache_keyboard_owner(ctx: &egui::Context, owner: KeyboardOwner) {
    let viewport = ctx.viewport_id();
    let pass = ctx.cumulative_pass_nr();
    ctx.data_mut(|data| {
        data.insert_temp(
            owner_cache_id(viewport),
            CachedKeyboardOwner { pass, owner },
        );
    });
}

pub(crate) fn keymap_owner_blocks_shortcuts(ctx: &egui::Context) -> bool {
    cached_keyboard_owner(ctx)
        .map(KeyboardOwner::blocks_legacy_keymap_shortcuts)
        // A keymap path without the App pass gate is an invalid route. Keep it
        // fail-closed instead of falling back to another egui ownership read.
        .unwrap_or(true)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scope(surface: ShortcutSurface) -> ShortcutScope {
        ShortcutScope::new(egui::ViewportId::ROOT, surface)
    }

    fn snapshot() -> KeyboardOwnershipSnapshot {
        KeyboardOwnershipSnapshot {
            viewport: egui::ViewportId::ROOT,
            viewport_focused: true,
            modal: false,
            focused_text_input: None,
            pending_text_input: None,
            ime_grace: false,
            focused_ui: None,
            shortcut_scope: Some(scope(ShortcutSurface::Main)),
        }
    }

    #[test]
    fn keyboard_owner_decision_covers_every_owner_and_text_phase() {
        let text_id = egui::Id::new("text");
        let ui_id = egui::Id::new("focused-ui");

        let mut input = snapshot();
        input.focused_text_input = Some(text_id);
        assert_eq!(
            decide_keyboard_owner(input),
            KeyboardOwner::TextInput {
                viewport: egui::ViewportId::ROOT,
                widget_id: text_id,
                phase: TextInputPhase::Focused,
            }
        );

        let mut input = snapshot();
        input.pending_text_input = Some(text_id);
        assert_eq!(
            decide_keyboard_owner(input),
            KeyboardOwner::TextInput {
                viewport: egui::ViewportId::ROOT,
                widget_id: text_id,
                phase: TextInputPhase::PendingFocus,
            }
        );

        let mut input = snapshot();
        input.ime_grace = true;
        assert_eq!(
            decide_keyboard_owner(input),
            KeyboardOwner::TextInput {
                viewport: egui::ViewportId::ROOT,
                widget_id: egui::Id::NULL,
                phase: TextInputPhase::ImeGrace,
            }
        );

        let mut input = snapshot();
        input.focused_ui = Some(ui_id);
        assert_eq!(
            decide_keyboard_owner(input),
            KeyboardOwner::FocusedUi {
                viewport: egui::ViewportId::ROOT,
                widget_id: ui_id,
            }
        );

        assert_eq!(
            decide_keyboard_owner(snapshot()),
            KeyboardOwner::ApplicationShortcut {
                scope: scope(ShortcutSurface::Main)
            }
        );

        let mut input = snapshot();
        input.shortcut_scope = None;
        assert_eq!(decide_keyboard_owner(input), KeyboardOwner::Unclaimed);
    }

    #[test]
    fn modal_outranks_every_other_keyboard_claim() {
        let mut input = snapshot();
        input.modal = true;
        input.viewport_focused = false;
        input.focused_text_input = Some(egui::Id::new("text"));
        input.pending_text_input = Some(egui::Id::new("pending"));
        input.ime_grace = true;
        input.focused_ui = Some(egui::Id::new("ui"));
        input.shortcut_scope = Some(scope(ShortcutSurface::Fullscreen));
        assert_eq!(decide_keyboard_owner(input), KeyboardOwner::Modal);
    }

    #[test]
    fn ime_grace_is_text_input_even_with_generic_focused_ui() {
        let mut input = snapshot();
        input.ime_grace = true;
        input.focused_ui = Some(egui::Id::new("ime-widget"));
        assert!(matches!(
            decide_keyboard_owner(input),
            KeyboardOwner::TextInput {
                phase: TextInputPhase::ImeGrace,
                ..
            }
        ));
    }

    #[test]
    fn bookmark_draft_without_live_claim_does_not_own_keyboard() {
        let bookmark_title_draft = Some(String::from("draft"));
        assert!(bookmark_title_draft.is_some());
        // Draft/session state is intentionally absent from the snapshot.
        assert_eq!(
            decide_keyboard_owner(snapshot()),
            KeyboardOwner::ApplicationShortcut {
                scope: scope(ShortcutSurface::Main)
            }
        );
    }

    #[test]
    fn pending_focus_claim_clears_when_target_focuses() {
        let widget_id = egui::Id::new("target");
        let claim = Some(PendingTextInputFocusClaim::new(
            egui::ViewportId::ROOT,
            widget_id,
            10,
        ));
        let transition = transition_pending_focus_claim(
            claim,
            PendingFocusEvent::ObservePass {
                viewport: egui::ViewportId::ROOT,
                pass: 11,
                focused_widget: Some(widget_id),
            },
        );
        assert_eq!(transition.claim, None);
        assert_eq!(
            transition.ownership,
            Some((egui::ViewportId::ROOT, widget_id, TextInputPhase::Focused))
        );
    }

    #[test]
    fn pending_focus_claim_clears_when_another_widget_focuses() {
        let claim = Some(PendingTextInputFocusClaim::new(
            egui::ViewportId::ROOT,
            egui::Id::new("target"),
            10,
        ));
        let transition = transition_pending_focus_claim(
            claim,
            PendingFocusEvent::ObservePass {
                viewport: egui::ViewportId::ROOT,
                pass: 11,
                focused_widget: Some(egui::Id::new("other")),
            },
        );
        assert_eq!(transition.claim, None);
        assert_eq!(transition.ownership, None);
    }

    #[test]
    fn pending_focus_claim_clears_when_editing_ends() {
        let claim = Some(PendingTextInputFocusClaim::new(
            egui::ViewportId::ROOT,
            egui::Id::new("target"),
            10,
        ));
        let transition = transition_pending_focus_claim(claim, PendingFocusEvent::EditingEnded);
        assert_eq!(transition.claim, None);
        assert_eq!(transition.ownership, None);
    }

    #[test]
    fn pending_focus_claim_expires_after_one_unfocused_pass() {
        let widget_id = egui::Id::new("target");
        let claim = Some(PendingTextInputFocusClaim::new(
            egui::ViewportId::ROOT,
            widget_id,
            10,
        ));
        let protected_pass = transition_pending_focus_claim(
            claim,
            PendingFocusEvent::ObservePass {
                viewport: egui::ViewportId::ROOT,
                pass: 11,
                focused_widget: None,
            },
        );
        assert_eq!(protected_pass.claim, claim);
        assert_eq!(
            protected_pass.ownership,
            Some((
                egui::ViewportId::ROOT,
                widget_id,
                TextInputPhase::PendingFocus
            ))
        );

        let expired = transition_pending_focus_claim(
            protected_pass.claim,
            PendingFocusEvent::ObservePass {
                viewport: egui::ViewportId::ROOT,
                pass: 12,
                focused_widget: None,
            },
        );
        assert_eq!(expired.claim, None);
        assert_eq!(expired.ownership, None);
    }

    #[test]
    fn another_viewport_does_not_age_or_use_pending_focus_claim() {
        let claim = Some(PendingTextInputFocusClaim::new(
            egui::ViewportId::from_hash_of("child"),
            egui::Id::new("target"),
            10,
        ));
        let transition = transition_pending_focus_claim(
            claim,
            PendingFocusEvent::ObservePass {
                viewport: egui::ViewportId::ROOT,
                pass: 100,
                focused_widget: None,
            },
        );
        assert_eq!(transition.claim, claim);
        assert_eq!(transition.ownership, None);
    }

    #[test]
    fn shortcut_permit_is_issued_only_by_application_shortcut_owner() {
        let shortcut_scope = scope(ShortcutSurface::Fullscreen);
        let owner = KeyboardOwner::ApplicationShortcut {
            scope: shortcut_scope,
        };
        assert_eq!(
            owner.shortcut_permit().map(ShortcutPermit::scope),
            Some(shortcut_scope)
        );
        for owner in [
            KeyboardOwner::Modal,
            KeyboardOwner::TextInput {
                viewport: egui::ViewportId::ROOT,
                widget_id: egui::Id::new("text"),
                phase: TextInputPhase::Focused,
            },
            KeyboardOwner::FocusedUi {
                viewport: egui::ViewportId::ROOT,
                widget_id: egui::Id::new("ui"),
            },
            KeyboardOwner::Unclaimed,
        ] {
            assert_eq!(owner.shortcut_permit(), None, "{owner:?}");
        }
    }

    #[test]
    fn legacy_main_shortcut_projection_matches_previous_inputs() {
        for viewport_focused in [false, true] {
            for modal in [false, true] {
                for tracked_text_focus in [false, true] {
                    for ime_grace in [false, true] {
                        for wants_keyboard_input in [false, true] {
                            let old_answer = modal || tracked_text_focus || ime_grace;
                            let mut input = snapshot();
                            input.viewport_focused = viewport_focused;
                            input.modal = modal;
                            input.focused_text_input =
                                tracked_text_focus.then(|| egui::Id::new("tracked-text"));
                            input.ime_grace = ime_grace;
                            input.focused_ui =
                                wants_keyboard_input.then(|| egui::Id::new("egui-owner"));
                            input.shortcut_scope =
                                viewport_focused.then(|| scope(ShortcutSurface::Main));
                            assert_eq!(
                                decide_keyboard_owner(input).blocks_legacy_main_shortcuts(),
                                old_answer,
                                "focused={viewport_focused} modal={modal} tracked={tracked_text_focus} ime={ime_grace} wants={wants_keyboard_input}"
                            );
                        }
                    }
                }
            }
        }

        let mut fullscreen_scope = snapshot();
        fullscreen_scope.viewport_focused = false;
        fullscreen_scope.shortcut_scope = Some(scope(ShortcutSurface::Fullscreen));
        assert!(
            decide_keyboard_owner(fullscreen_scope).blocks_legacy_main_shortcuts(),
            "the former viewer_session/deferred-reopen gate still blocks grid shortcuts"
        );
    }

    #[test]
    fn legacy_keymap_projection_matches_previous_wants_keyboard_input_gate() {
        for viewport_focused in [false, true] {
            for wants_keyboard_input in [false, true] {
                let mut input = snapshot();
                input.viewport_focused = viewport_focused;
                input.focused_ui = wants_keyboard_input.then(|| egui::Id::new("egui-owner"));
                input.shortcut_scope = viewport_focused.then(|| scope(ShortcutSurface::Main));
                assert_eq!(
                    decide_keyboard_owner(input).blocks_legacy_keymap_shortcuts(),
                    wants_keyboard_input
                );
            }
        }

        for phase in [TextInputPhase::Focused, TextInputPhase::ImeGrace] {
            let mut input = snapshot();
            input.focused_ui = Some(egui::Id::new("egui-text-owner"));
            match phase {
                TextInputPhase::Focused => {
                    input.focused_text_input = Some(egui::Id::new("tracked-text"));
                }
                TextInputPhase::ImeGrace => input.ime_grace = true,
                TextInputPhase::PendingFocus => unreachable!(),
            }
            assert!(
                decide_keyboard_owner(input).blocks_legacy_keymap_shortcuts(),
                "{phase:?} must preserve the prior wants_keyboard_input=true answer"
            );
        }

        let mut pending = snapshot();
        pending.pending_text_input = Some(egui::Id::new("pending"));
        assert!(
            !decide_keyboard_owner(pending).blocks_legacy_keymap_shortcuts(),
            "S3 must not activate PendingFocus as a new keymap block"
        );
    }
}

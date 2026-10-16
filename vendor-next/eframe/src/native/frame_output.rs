//! Shared root/deferred UI eligibility and output actions, independent of painting.

use super::winit_integration::PassMode;
use egui::{ViewportCommand, ViewportInfo};
use egui_winit::ActionRequested;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct PassDecision {
    pub run_ui: bool,
    pub paint: bool,
}

impl PassDecision {
    /// The Windows minimized-Full policy does not change viewport metadata or
    /// turn an explicit LogicOnly tick into a UI pass.
    pub fn for_viewport(
        mode: PassMode,
        info: &ViewportInfo,
        has_visible_descendant: bool,
        actions_want_paint: bool,
        windows: bool,
    ) -> Self {
        let full = mode == PassMode::Full;
        let visible = info.visible().unwrap_or(true);
        Self {
            run_ui: full
                && (visible
                    || has_visible_descendant
                    || actions_want_paint
                    || (windows && info.minimized == Some(true))),
            paint: full && (visible || actions_want_paint),
        }
    }
}

/// Extract actions from this pass before deciding whether its surface needs
/// painting. Remaining native commands keep their order and normal owner.
pub(super) fn take_main_viewport_actions(
    commands: &mut Vec<ViewportCommand>,
    actions: &mut Vec<ActionRequested>,
) {
    for command in core::mem::take(commands) {
        let action = match command {
            ViewportCommand::RequestCut => ActionRequested::Cut,
            ViewportCommand::RequestCopy => ActionRequested::Copy,
            ViewportCommand::RequestPaste => ActionRequested::Paste,
            ViewportCommand::Screenshot(callback) => ActionRequested::Screenshot(callback),
            ViewportCommand::RequestPaintWhileHidden => ActionRequested::PaintWhileHidden,
            other => {
                commands.push(other);
                continue;
            }
        };
        actions.push(action);
    }
}

/// The clipboard and next-input queue belong to the action's viewport State.
pub(super) trait ClipboardInput {
    fn clipboard_text(&mut self) -> Option<String>;
    fn clipboard_image(&mut self) -> Option<egui::ColorImage>;
    fn input(&mut self) -> &mut egui::RawInput;
}

impl ClipboardInput for egui_winit::State {
    fn clipboard_text(&mut self) -> Option<String> {
        Self::clipboard_text(self)
    }

    fn clipboard_image(&mut self) -> Option<egui::ColorImage> {
        Self::clipboard_image(self)
    }

    fn input(&mut self) -> &mut egui::RawInput {
        self.egui_input_mut()
    }
}

pub(super) trait MainOutputTarget: ClipboardInput {
    fn handle_platform_output(&mut self, platform: egui::PlatformOutput);
}

/// Native adapter used only after user UI has returned, without holding a
/// viewport lock across the callback.
pub(super) struct WinitOutputTarget<'a> {
    pub state: &'a mut egui_winit::State,
    pub window: &'a winit::window::Window,
    pub event_loop: &'a winit::event_loop::ActiveEventLoop,
}

impl ClipboardInput for WinitOutputTarget<'_> {
    fn clipboard_text(&mut self) -> Option<String> {
        self.state.clipboard_text()
    }

    fn clipboard_image(&mut self) -> Option<egui::ColorImage> {
        self.state.clipboard_image()
    }

    fn input(&mut self) -> &mut egui::RawInput {
        self.state.egui_input_mut()
    }
}

impl MainOutputTarget for WinitOutputTarget<'_> {
    fn handle_platform_output(&mut self, platform: egui::PlatformOutput) {
        self.state
            .handle_platform_output_with_event_loop(self.window, self.event_loop, platform);
    }
}

/// Finish the current root/deferred pass's non-rendering output before choosing
/// its paint path. Native commands remain with the normal viewport handler.
pub(super) fn finish_main_viewport_output(
    platform: egui::PlatformOutput,
    commands: &mut Vec<ViewportCommand>,
    actions: &mut Vec<ActionRequested>,
    target: &mut impl MainOutputTarget,
) {
    take_main_viewport_actions(commands, actions);
    target.handle_platform_output(platform);
    process_non_paint_actions(actions, target);
}

/// Complete non-paint actions once, including on a Full pass that will not
/// paint. Keep capture/explicit-paint actions for the corresponding paint path.
pub(super) fn process_non_paint_actions(
    actions: &mut Vec<ActionRequested>,
    target: &mut impl ClipboardInput,
) {
    for action in core::mem::take(actions) {
        let event = match action {
            ActionRequested::Cut => Some(egui::Event::Cut),
            ActionRequested::Copy => Some(egui::Event::Copy),
            ActionRequested::Paste => {
                if let Some(contents) = target.clipboard_text() {
                    let contents = contents.replace("\r\n", "\n");
                    (!contents.is_empty()).then_some(egui::Event::Paste(contents))
                } else {
                    target
                        .clipboard_image()
                        .map(|image| egui::Event::PasteImage(std::sync::Arc::new(image)))
                }
            }
            paint_action @ (ActionRequested::Screenshot(_) | ActionRequested::PaintWhileHidden) => {
                actions.push(paint_action);
                continue;
            }
        };
        if let Some(event) = event {
            target.input().events.push(event);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::{Event, ViewportId};

    #[derive(Default)]
    struct FakeState {
        input: egui::RawInput,
        text: Option<String>,
        image: Option<egui::ColorImage>,
        text_reads: usize,
        image_reads: usize,
        platform_outputs: Vec<egui::PlatformOutput>,
    }

    impl ClipboardInput for FakeState {
        fn clipboard_text(&mut self) -> Option<String> {
            self.text_reads += 1;
            self.text.clone()
        }

        fn clipboard_image(&mut self) -> Option<egui::ColorImage> {
            self.image_reads += 1;
            self.image.clone()
        }

        fn input(&mut self) -> &mut egui::RawInput {
            &mut self.input
        }
    }

    impl MainOutputTarget for FakeState {
        fn handle_platform_output(&mut self, platform: egui::PlatformOutput) {
            // As with the native adapter, platform clipboard writes precede
            // actions that read the clipboard into the next input batch.
            for command in &platform.commands {
                if let egui::OutputCommand::CopyText(text) = command {
                    self.text = Some(text.clone());
                }
            }
            self.platform_outputs.push(platform);
        }
    }

    #[test]
    fn minimized_full_ui_and_surface_decisions_preserve_platform_and_logic_only_contracts() {
        for windows in [false, true] {
            for (minimized, occluded) in
                [(false, false), (true, false), (false, true), (true, true)]
            {
                let info = ViewportInfo {
                    minimized: Some(minimized),
                    occluded: Some(occluded),
                    ..Default::default()
                };
                for has_visible_descendant in [false, true] {
                    for explicit_paint in [false, true] {
                        let decision = PassDecision::for_viewport(
                            PassMode::Full,
                            &info,
                            has_visible_descendant,
                            explicit_paint,
                            windows,
                        );
                        let visible = !minimized && !occluded;
                        assert_eq!(decision.paint, visible || explicit_paint);
                        assert_eq!(
                            decision.run_ui,
                            visible
                                || has_visible_descendant
                                || explicit_paint
                                || (windows && minimized)
                        );
                        assert_eq!(
                            PassDecision::for_viewport(
                                PassMode::LogicOnly,
                                &info,
                                has_visible_descendant,
                                explicit_paint,
                                windows,
                            ),
                            PassDecision {
                                run_ui: false,
                                paint: false
                            }
                        );
                        assert_eq!(info.minimized, Some(minimized));
                        assert_eq!(info.occluded, Some(occluded));
                    }
                }
            }
        }
    }

    #[test]
    fn minimized_full_actions_keep_capture_identity_native_commands_and_owner_input_order() {
        let capture = egui::ScreenshotCallback::new(|_| panic!("capture needs a paint"));
        let mut actions = vec![ActionRequested::Copy];
        let mut commands = vec![
            ViewportCommand::Title("title".to_owned()),
            ViewportCommand::RequestCut,
            ViewportCommand::Screenshot(capture.clone()),
            ViewportCommand::RequestPaste,
            ViewportCommand::Minimized(false),
            ViewportCommand::RequestPaintWhileHidden,
        ];
        take_main_viewport_actions(&mut commands, &mut actions);
        assert!(
            matches!(commands.as_slice(), [ViewportCommand::Title(title), ViewportCommand::Minimized(false)] if title == "title")
        );
        let count = actions.len();
        take_main_viewport_actions(&mut commands, &mut actions);
        assert_eq!(
            actions.len(),
            count,
            "extraction cannot schedule actions twice"
        );

        let child = ViewportId::from_hash_of("clipboard-owner");
        let mut target = FakeState {
            input: egui::RawInput {
                viewport_id: child,
                events: vec![Event::Text("old".to_owned())],
                ..Default::default()
            },
            text: Some("one\r\ntwo".to_owned()),
            ..Default::default()
        };
        let sibling = FakeState::default();
        process_non_paint_actions(&mut actions, &mut target);
        assert_eq!(target.input.viewport_id, child);
        assert_eq!(
            target.input.events,
            vec![
                Event::Text("old".to_owned()),
                Event::Copy,
                Event::Cut,
                Event::Paste("one\ntwo".to_owned())
            ]
        );
        assert!(sibling.input.events.is_empty());
        assert!(
            matches!(actions.as_slice(), [ActionRequested::Screenshot(callback), ActionRequested::PaintWhileHidden] if callback == &capture)
        );
        process_non_paint_actions(&mut actions, &mut target);
        assert_eq!(target.input.events.len(), 4);
        assert_eq!(target.text_reads, 1);
        assert_eq!(target.image_reads, 0);
        assert_eq!(
            actions.len(),
            2,
            "non-paint completion must retain paint actions"
        );
    }

    #[test]
    fn minimized_full_paste_distinguishes_empty_text_from_missing_text_and_image() {
        for text in [Some("".to_owned()), Some("text".to_owned()), None] {
            let image = egui::ColorImage::filled([2, 1], egui::Color32::WHITE);
            let mut target = FakeState {
                text: text.clone(),
                image: Some(image.clone()),
                ..Default::default()
            };
            let mut actions = vec![ActionRequested::Paste];
            process_non_paint_actions(&mut actions, &mut target);
            match text {
                Some(text) if text.is_empty() => assert!(target.input.events.is_empty()),
                Some(text) => assert_eq!(target.input.events, vec![Event::Paste(text)]),
                None => assert_eq!(
                    target.input.events,
                    vec![Event::PasteImage(std::sync::Arc::new(image))]
                ),
            }
            assert!(actions.is_empty());
            assert_eq!(target.text_reads, 1);
            assert_eq!(target.image_reads, usize::from(target.text.is_none()));
        }
        let mut target = FakeState::default();
        let mut actions = vec![ActionRequested::Paste];
        process_non_paint_actions(&mut actions, &mut target);
        assert!(actions.is_empty());
        assert!(target.input.events.is_empty());
    }

    #[test]
    fn minimized_full_output_boundary_delivers_platform_ime_actions_and_retains_paint_requests() {
        let child = ViewportId::from_hash_of("output-owner");
        let mut owner = FakeState {
            input: egui::RawInput {
                viewport_id: child,
                events: vec![Event::Text("old".to_owned())],
                ..Default::default()
            },
            text: Some("before-platform".to_owned()),
            ..Default::default()
        };
        let sibling = FakeState::default();
        let rect = egui::Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(100.0, 30.0));
        let platform = egui::PlatformOutput {
            commands: vec![egui::OutputCommand::CopyText("platform\r\ntext".to_owned())],
            ime: Some(egui::output::IMEOutput {
                purpose: egui::IMEPurpose::Normal,
                rect,
                cursor_rect: rect,
                should_interrupt_composition: true,
            }),
            ..Default::default()
        };
        let capture = egui::ScreenshotCallback::new(|_| panic!("no surface painted yet"));
        let mut commands = vec![
            ViewportCommand::Title("child".to_owned()),
            ViewportCommand::RequestCopy,
            ViewportCommand::Screenshot(capture.clone()),
            ViewportCommand::RequestPaste,
            ViewportCommand::RequestPaintWhileHidden,
            ViewportCommand::Minimized(false),
        ];
        let mut actions = vec![ActionRequested::Cut];
        finish_main_viewport_output(platform.clone(), &mut commands, &mut actions, &mut owner);
        assert!(owner.platform_outputs == vec![platform]);
        assert_eq!(owner.input.viewport_id, child);
        assert_eq!(
            owner.input.events,
            vec![
                Event::Text("old".to_owned()),
                Event::Cut,
                Event::Copy,
                Event::Paste("platform\ntext".to_owned()),
            ]
        );
        assert!(sibling.input.events.is_empty() && sibling.platform_outputs.is_empty());
        assert!(
            matches!(commands.as_slice(), [ViewportCommand::Title(title), ViewportCommand::Minimized(false)] if title == "child")
        );
        assert!(
            matches!(actions.as_slice(), [ActionRequested::Screenshot(callback), ActionRequested::PaintWhileHidden] if callback == &capture)
        );
        assert_eq!(owner.text_reads, 1);

        finish_main_viewport_output(Default::default(), &mut commands, &mut actions, &mut owner);
        assert_eq!(
            owner.input.events.len(),
            4,
            "a later pass cannot replay clipboard actions"
        );
        assert_eq!(owner.text_reads, 1);
        assert_eq!(owner.platform_outputs.len(), 2);
        assert_eq!(
            actions.len(),
            2,
            "capture and explicit paint remain pending for paint"
        );
        assert_eq!(
            commands.len(),
            2,
            "native commands remain for their ordinary handler"
        );
    }
}

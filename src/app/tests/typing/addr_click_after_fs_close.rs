//! Investigation of 1.748. All root frames use the real EpiIntegration/App path.
//! The immediate viewport renderer below is windowless, like close_return_tests.
//! It does not reproduce HWND activation, SendInput, or USER32 event batching.

use super::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;

const ROOT_SIZE: egui::Vec2 = egui::vec2(1600.0, 900.0);
const CHILD_SIZE: egui::Vec2 = egui::vec2(1280.0, 720.0);
const IDLE_GAP: Duration = Duration::from_millis(1100);

fn address_id() -> egui::Id {
    egui::Id::unique("folder_address_input")
}

fn raw_input(
    viewport: egui::ViewportId,
    child: egui::ViewportId,
    root_focused: bool,
    previous_modifiers: egui::Modifiers,
    mut events: Vec<egui::Event>,
) -> egui::RawInput {
    let rect = |size| egui::Rect::from_min_size(egui::Pos2::ZERO, size);
    let modifiers = events
        .iter()
        .find_map(|event| match event {
            egui::Event::Key { modifiers, .. } => Some(*modifiers),
            _ => None,
        })
        .unwrap_or(egui::Modifiers::NONE);
    // Native input reports changes, not a new modifier event on every idle pass.
    // Even an unchanged ModifiersChanged(NONE) makes wants_repaint_after() zero.
    if modifiers != previous_modifiers {
        events.insert(0, egui::Event::ModifiersChanged(modifiers));
    }
    egui::RawInput {
        viewport_id: viewport,
        screen_rect: Some(rect(if viewport == egui::ViewportId::ROOT {
            ROOT_SIZE
        } else {
            CHILD_SIZE
        })),
        focused: if viewport == egui::ViewportId::ROOT {
            root_focused
        } else {
            !root_focused
        },
        events,
        viewports: [
            (
                egui::ViewportId::ROOT,
                egui::ViewportInfo {
                    focused: Some(root_focused),
                    inner_rect: Some(rect(ROOT_SIZE)),
                    outer_rect: Some(rect(ROOT_SIZE)),
                    native_pixels_per_point: Some(1.0),
                    ..Default::default()
                },
            ),
            (
                child,
                egui::ViewportInfo {
                    parent: Some(egui::ViewportId::ROOT),
                    focused: Some(!root_focused),
                    inner_rect: Some(rect(CHILD_SIZE)),
                    outer_rect: Some(rect(CHILD_SIZE)),
                    native_pixels_per_point: Some(1.0),
                    ..Default::default()
                },
            ),
        ]
        .into_iter()
        .collect(),
        ..Default::default()
    }
}

#[derive(Debug)]
struct RootSample {
    pass: u64,
    repaint: Duration,
    input_events: Vec<egui::Event>,
    prev_frame_causes: Vec<egui::RepaintCause>,
    tail_pending_state: Vec<(&'static str, bool)>,
    input_focused: bool,
    focused: Option<egui::Id>,
    fields: Vec<probe::Field>,
    widget: Option<egui::WidgetRect>,
    gates: Vec<probe::ChromeGate>,
}

struct Frames {
    env: AppTestEnv,
    backend: eframe::HeadlessEpiIntegration,
    ctx: egui::Context,
    child: egui::ViewportId,
    child_events: Rc<RefCell<Vec<egui::Event>>>,
    root_focused: Rc<Cell<bool>>,
    child_passes: Rc<Cell<usize>>,
    child_outputs: Rc<RefCell<Vec<egui::FullOutput>>>,
    samples: Vec<RootSample>,
    _probe: probe::Session,
}

impl Frames {
    fn new() -> Self {
        let mut env = fixture();
        env.settings.video_in_window_mode = false;
        env.settings.detached_viewer_enabled = false;
        env.settings.detached_viewer_open_images_in_window = false;
        // An address submission must remain distinguishable from GridOpenSelected.
        env.settings.auto_fullscreen_image_folders = false;
        let paths = super::search::seed_images(&mut env);
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        crate::ime_focus::install_ime_input_policy(&ctx);
        crate::ui_fullscreen::install_fs_navigator_input_tracking(&ctx);
        // Do not let an automatic retry hide the very first clicked pass.
        ctx.options_mut(|options| options.max_passes = std::num::NonZeroUsize::new(1).unwrap());
        let backend = eframe::HeadlessEpiIntegration::new(ctx.clone());
        let child = env.fullscreen_viewport_id();
        let child_events = Rc::new(RefCell::new(Vec::new()));
        let root_focused = Rc::new(Cell::new(true));
        let child_passes = Rc::new(Cell::new(0));
        let child_outputs = Rc::new(RefCell::new(Vec::new()));
        let events = Rc::clone(&child_events);
        let focus = Rc::clone(&root_focused);
        let calls = Rc::clone(&child_passes);
        let outputs = Rc::clone(&child_outputs);
        egui::Context::set_immediate_viewport_renderer(move |ctx, mut viewport| {
            calls.set(calls.get() + 1);
            let input = raw_input(
                viewport.ids.this,
                viewport.ids.this,
                focus.get(),
                ctx.input_for(viewport.ids.this, |input| input.modifiers),
                std::mem::take(&mut *events.borrow_mut()),
            );
            let output = ctx.run_ui(input, |ui| (viewport.viewport_ui_cb)(ui));
            outputs.borrow_mut().push(output);
        });
        let mut frames = Self {
            env,
            backend,
            ctx,
            child,
            child_events,
            root_focused,
            child_passes,
            child_outputs,
            samples: Vec::new(),
            _probe: probe::Session::new(),
        };
        let deadline = Instant::now() + DEADLINE;
        loop {
            frames.root(Vec::new());
            if frames.env.items.len() == paths.len()
                && frames.env.items.iter().all(|item| {
                    item.drag_source_path()
                        .is_some_and(|p| paths.iter().any(|path| path.as_path() == p))
                })
                && !frames.env.main_chrome_pointer_blocked()
                && !frames.env.sidecar_restore_active()
                && frames.env.requested.is_empty()
                && frames.last().widget.is_some_and(|widget| {
                    widget.enabled && widget.visible && widget.interact_rect.is_positive()
                })
            {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "origin adoption: {:?}",
                frames.samples
            );
            std::thread::sleep(Duration::from_millis(1));
        }
        frames
            .env
            .move_grid_cursor(Some(0), GridCursorIntent::ExplicitKeep);
        frames.root(Vec::new());
        frames
    }

    fn deliver(&self, output: egui::FullOutput) {
        for viewport in output.viewport_output.values() {
            for command in &viewport.commands {
                if let egui::ViewportCommand::RetireApproved(approval) = command {
                    self.ctx.retire_approved_viewports(approval);
                }
            }
        }
        output.drop_without_applying_deltas();
        self.ctx.take_viewport_retirement_texture_delta().clear();
    }

    fn root(&mut self, events: Vec<egui::Event>) {
        probe::begin_frame();
        let before = self.ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT);
        let input = raw_input(
            egui::ViewportId::ROOT,
            self.child,
            self.root_focused.get(),
            self.ctx.input(|input| input.modifiers),
            events,
        );
        let input_events = input.events.clone();
        let output = self.backend.update(&mut self.env.app, input);
        let repaint = output
            .viewport_output
            .get(&egui::ViewportId::ROOT)
            .unwrap()
            .repaint_delay;
        let widget = self.ctx.viewport(|viewport| {
            viewport
                .prev_pass
                .widgets
                .layers()
                .flat_map(|(_, widgets)| widgets.iter())
                .find(|widget| widget.id == address_id())
                .copied()
        });
        self.samples.push(RootSample {
            pass: before,
            repaint,
            input_events,
            // This is the same witness printed by App's tail_repaint event.
            prev_frame_causes: self.ctx.repaint_causes(),
            tail_pending_state: self.tail_pending_state(),
            input_focused: self.ctx.input(|input| input.focused),
            focused: self.ctx.memory(|memory| memory.focused()),
            fields: probe::fields()
                .into_iter()
                .filter(|field| field.id == address_id())
                .collect(),
            widget,
            gates: probe::gates(),
        });
        assert_eq!(
            self.ctx.cumulative_pass_nr_for(egui::ViewportId::ROOT),
            before + 1
        );
        self.deliver(output);
        let outputs: Vec<_> = self.child_outputs.borrow_mut().drain(..).collect();
        for output in outputs {
            self.deliver(output);
        }
    }

    fn last(&self) -> &RootSample {
        self.samples.last().unwrap()
    }

    fn tail_pending_state(&self) -> Vec<(&'static str, bool)> {
        // Read-only owners used by App's tail_repaint reasons. Do not enable the
        // process-global perf writer/clock or complete/reset any work in the test.
        let app = &self.env;
        vec![
            ("folder_nav_pending", app.folder_nav_pending.is_some()),
            ("requested_nonempty", app.requested.blocks_idle_upgrade()),
            ("texture_backlog_nonempty", app.requested.images_len() > 0),
            (
                "visible_video_thumb_pending",
                app.requested.video_image_pending(&app.keep_set),
            ),
            ("pdf_enumerate_pending", app.pdf_enumerate_pending.is_some()),
            ("zip_enumerate_pending", app.zip_enumerate_pending.is_some()),
            ("video_upscale_running", app.video_upscale_running.is_some()),
            (
                "book_bookmark_pending",
                !app.book_bookmark_pending_requests.is_empty(),
            ),
            (
                "bookmark_browser_pending",
                app.bookmark_browser_pending.is_some(),
            ),
            (
                "bookmark_presence_pending",
                app.bookmark_presence_pending.is_some(),
            ),
            (
                "bookmark_delete_pending",
                app.bookmark_delete_pending.is_some(),
            ),
            ("bookmark_open_pending", app.bookmark_open_pending.is_some()),
            (
                "fs_viewport_cleanup",
                app.fs_viewport_shown && app.fullscreen_idx.is_none(),
            ),
        ]
    }

    fn close_with_escape_before_first_click(&mut self, root_focused: bool) -> egui::Pos2 {
        let before = self.last().widget.expect("address before fullscreen").rect;
        self.env.open_fullscreen(0, HistoryTrigger::UserChosen);
        self.child = self.env.fullscreen_viewport_id();
        assert_eq!(self.env.viewer_presentation, ViewerPresentation::Fullscreen);
        self.root_focused.set(false);
        self.root(Vec::new());
        assert!(
            self.child_passes.get() > 0,
            "a real separate viewport must draw"
        );
        assert!(self.env.fullscreen_idx.is_some());
        *self.child_events.borrow_mut() = key_pair(egui::Key::Escape, egui::Modifiers::NONE);
        self.root(Vec::new());
        assert!(
            self.env.fullscreen_idx.is_none(),
            "child Escape must close fullscreen"
        );
        self.root_focused.set(root_focused);
        // Escape runs in the immediate child after this root's keep-alive phase.
        // App's fs_viewport_cleanup reason requests the next root, which delivers
        // Visible(false). This is lifecycle work, not a toolbar warm-up pass.
        if self.env.fs_viewport_shown && self.last().repaint == Duration::ZERO {
            self.root(Vec::new());
        }
        // egui request_repaint_after(ZERO) retains one outstanding repaint at
        // begin_pass. Run that requested follow-up, not an unbounded idle loop.
        if self.last().repaint == Duration::ZERO {
            self.root(Vec::new());
        }
        let last = self.last();
        eprintln!("addr_click_after_fs_close root history: {:?}", self.samples);
        if last.repaint == Duration::ZERO {
            eprintln!(
                "addr_click_after_fs_close: post-close boundary, NOT global idle; input_events={:?}, prev_frame_causes={:?}, tail_pending_state={:?}",
                last.input_events, last.prev_frame_causes, last.tail_pending_state
            );
        }
        // Disabled, clipped or omitted toolbar geometry is candidate B, not a
        // fixture-readiness failure. Click the visible rect, not the potentially
        // inverted interact_rect, and let the focus assertion expose the result.
        let click_rect = match last.widget {
            Some(widget) => widget.rect,
            None => {
                eprintln!(
                    "addr_click_after_fs_close: last root omitted the field; clicking its pre-fullscreen visible rect {before:?}"
                );
                before
            }
        };
        eprintln!(
            "address before open={before:?}, last visible rect={:?}",
            last.widget.map(|widget| widget.rect)
        );
        assert_ne!(
            last.focused,
            Some(address_id()),
            "this must be the first focus-acquiring click"
        );
        if last.repaint > Duration::ZERO && last.repaint < IDLE_GAP {
            eprintln!(
                "addr_click_after_fs_close: native timer {:?} falls inside the injected no-frame gap; native event-loop scheduling is not reproduced",
                last.repaint
            );
        }
        assert!(self.env.checked.is_empty());
        // EpiIntegration supplies its real elapsed clock; RawInput.time cannot
        // fast-forward it. This deliberately leaves a real gap with no App frame.
        // A native backend might service a positive repaint timer in this gap;
        // that OS scheduling/activation part is outside this windowless driver.
        std::thread::sleep(IDLE_GAP);
        click_rect.center()
    }

    fn assert_focused(&self) {
        assert!(
            self.env.fullscreen_idx.is_none(),
            "grid reopened fullscreen: {:?}",
            self.last()
        );
        assert_eq!(self.last().focused, Some(address_id()), "{:?}", self.last());
        assert!(
            self.env.address_has_focus,
            "App field flag: {:?}",
            self.last()
        );
        assert!(self.last().input_focused, "root focus: {:?}", self.last());
        assert!(
            !self.last().fields.is_empty(),
            "field was not drawn: {:?}",
            self.last()
        );
        assert!(
            self.last().fields.iter().all(|field| field.enabled),
            "{:?}",
            self.last()
        );
        assert!(
            self.env.checked.is_empty(),
            "Ctrl+A leaked into grid: {:?}",
            self.last()
        );
    }

    fn assert_ctrl_a_goes_to_address(&mut self) {
        self.root(key_pair(egui::Key::A, windows_ctrl()));
        self.assert_focused();
        let cursor = egui::TextEdit::load_state(&self.ctx, address_id())
            .unwrap()
            .cursor
            .char_range()
            .expect("address selection")
            .as_sorted_char_range();
        assert_eq!(cursor.start.0, 0);
        assert_eq!(cursor.end.0, self.env.address.chars().count());
    }

    fn assert_enter_does_not_open_grid_image(&mut self) {
        self.root(key_pair(egui::Key::Enter, egui::Modifiers::NONE));
        // Submitting a single-line field legitimately ends its focus.
        assert!(
            self.env.fullscreen_idx.is_none(),
            "Enter reached grid: {:?}",
            self.last()
        );
    }
}

fn key_pair(key_name: egui::Key, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    [true, false]
        .into_iter()
        .map(|pressed| key(key_name, pressed, modifiers))
        .collect()
}

fn windows_ctrl() -> egui::Modifiers {
    // egui-winit maps Ctrl to both ctrl and the platform-neutral command flag.
    egui::Modifiers {
        ctrl: true,
        command: true,
        ..egui::Modifiers::NONE
    }
}

fn click(pos: egui::Pos2) -> Vec<egui::Event> {
    vec![
        egui::Event::PointerMoved(pos),
        pointer(pos, true),
        pointer(pos, false),
    ]
}

fn case(run: impl FnOnce(&mut Frames) + Send + 'static) {
    // The immediate renderer is TLS. It must not remain installed on a reusable
    // test worker, and the App fixture/input lease must be created on its thread.
    if let Err(payload) = std::thread::spawn(move || run(&mut Frames::new())).join() {
        // Keep the original assertion/panic location instead of a second join unwrap.
        std::panic::resume_unwind(payload);
    }
}

#[test]
#[ignore = "§1.748 candidate A: click and key in one root pass; known ordering, not fixed (docs/next-release-backlog.md §1.748)"]
fn addr_click_after_fs_close_a_coalesced_enter_does_not_open_grid() {
    case(|h| {
        let pos = h.close_with_escape_before_first_click(true);
        let mut events = click(pos);
        events.extend(key_pair(egui::Key::Enter, egui::Modifiers::NONE));
        h.root(events);
        // Enter can submit/end focus; it must never reopen the selected image.
        assert!(
            h.env.fullscreen_idx.is_none(),
            "coalesced Enter went to grid: {:?}",
            h.last()
        );
        assert!(
            h.last().widget.is_some_and(|widget| widget.enabled),
            "{:?}",
            h.last()
        );
    });
}

#[test]
#[ignore = "§1.748 candidate A: click and key in one root pass; known ordering, not fixed (docs/next-release-backlog.md §1.748)"]
fn addr_click_after_fs_close_a_coalesced_ctrl_a_selects_address() {
    case(|h| {
        let pos = h.close_with_escape_before_first_click(true);
        let mut events = click(pos);
        events.extend(key_pair(egui::Key::A, windows_ctrl()));
        h.root(events);
        h.assert_focused();
        let cursor = egui::TextEdit::load_state(&h.ctx, address_id())
            .unwrap()
            .cursor
            .char_range()
            .unwrap()
            .as_sorted_char_range();
        assert_eq!(cursor.start.0, 0);
        assert_eq!(cursor.end.0, h.env.address.chars().count());
        h.assert_enter_does_not_open_grid_image();
    });
}

#[test]
fn addr_click_after_fs_close_b_first_click_uses_last_idle_toolbar_rect() {
    case(|h| {
        let pos = h.close_with_escape_before_first_click(true);
        let previous_widget = h.last().widget;
        h.root(click(pos));
        h.assert_focused();
        if let Some(previous) = previous_widget {
            assert_eq!(h.last().widget.unwrap().rect, previous.rect);
        }
        h.assert_ctrl_a_goes_to_address();
        h.assert_enter_does_not_open_grid_image();
    });
}

#[test]
fn addr_click_after_fs_close_b_root_focus_gain_and_click_share_a_pass() {
    case(|h| {
        let pos = h.close_with_escape_before_first_click(false);
        assert!(!h.last().input_focused);
        h.root_focused.set(true);
        let mut events = vec![egui::Event::WindowFocused(true)];
        events.extend(click(pos));
        h.root(events);
        h.assert_focused();
        h.assert_ctrl_a_goes_to_address();
    });
}

#[test]
fn addr_click_after_fs_close_b_focus_gain_between_press_and_release() {
    case(|h| {
        let pos = h.close_with_escape_before_first_click(false);
        h.root(vec![egui::Event::PointerMoved(pos), pointer(pos, true)]);
        h.root_focused.set(true);
        h.root(vec![egui::Event::WindowFocused(true), pointer(pos, false)]);
        h.assert_focused();
        h.assert_ctrl_a_goes_to_address();
        h.assert_enter_does_not_open_grid_image();
    });
}

#[test]
fn addr_click_after_fs_close_c_next_root_pass_preserves_clicked_focus() {
    case(|h| {
        let pos = h.close_with_escape_before_first_click(true);
        h.root(click(pos));
        h.assert_focused();
        h.root(Vec::new());
        h.assert_focused();
        h.assert_ctrl_a_goes_to_address();
        h.assert_enter_does_not_open_grid_image();
    });
}

#[test]
fn addr_click_after_fs_close_d_press_then_release_preserve_address_focus() {
    case(|h| {
        let pos = h.close_with_escape_before_first_click(true);
        h.root(vec![egui::Event::PointerMoved(pos), pointer(pos, true)]);
        h.assert_focused();
        h.root(vec![pointer(pos, false)]);
        h.assert_focused();
        h.assert_ctrl_a_goes_to_address();
        h.assert_enter_does_not_open_grid_image();
    });
}

//! §1.691: partial selection and cut in the Everything search field (temporary and
//! pinned bars), driven through full headless App frames with real pointer events.
use super::phase_c_support::{AppTestEnv, setup_app};
use super::*;
use crate::ui_pinned_search::PinnedSearchKind as Kind;

const QUERY: &str = "旅行 abc ext:jpg;png 日本語 test";
const LONG: &str =
    r"旅行 abc ext:jpg;png 日本語 test path:D:\写真\2026\夏休み !folder: size:>1mb dm:thisyear";

struct Frames {
    backend: eframe::HeadlessEpiIntegration,
    ctx: egui::Context,
    ppp: f32,
    modifiers: egui::Modifiers,
    query: &'static str,
    copied: Vec<String>,
    /// The field's own galley and where it was painted in the latest pass.
    painted: Option<(egui::Pos2, std::sync::Arc<egui::Galley>)>,
    labels: Vec<(String, egui::Rect)>,
    last_press: Option<egui::Pos2>,
}

impl Frames {
    fn new(query: &'static str, ppp: f32) -> Self {
        let ctx = egui::Context::default();
        crate::ui_fonts::configure_fonts(&ctx);
        Self {
            backend: eframe::HeadlessEpiIntegration::new(ctx.clone()),
            ctx,
            ppp,
            modifiers: egui::Modifiers::NONE,
            query,
            copied: Vec::new(),
            painted: None,
            labels: Vec::new(),
            last_press: None,
        }
    }

    fn tick(&mut self, app: &mut App, events: Vec<egui::Event>) {
        let mut raw = egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 800.0),
            )),
            events: std::iter::once(egui::Event::ModifiersChanged(self.modifiers))
                .chain(events)
                .collect(),
            focused: true,
            ..Default::default()
        };
        let viewport = raw.viewports.get_mut(&egui::ViewportId::ROOT).unwrap();
        viewport.focused = Some(true);
        viewport.native_pixels_per_point = Some(self.ppp);
        let output = self.backend.update(app, raw);
        for command in &output.platform_output.commands {
            if let egui::OutputCommand::CopyText(text) = command {
                self.copied.push(text.clone());
            }
        }
        self.labels.clear();
        for clipped in &output.shapes {
            visit_text(&clipped.shape, &mut |shape| {
                let text = shape.galley.text();
                self.labels.push((
                    text.to_owned(),
                    shape.galley.rect.translate(shape.pos.to_vec2()),
                ));
                if text == self.query {
                    self.painted = Some((shape.pos, shape.galley.clone()));
                }
            });
        }
        output.drop_without_applying_deltas();
    }

    fn idle(&mut self, app: &mut App, passes: usize) {
        for _ in 0..passes {
            self.tick(app, Vec::new());
        }
    }

    /// Press and release `button` at `pos`. The headless driver uses real time for
    /// click counting, so a nearby earlier press first waits out the double-click window.
    fn click(&mut self, app: &mut App, pos: egui::Pos2, button: egui::PointerButton) {
        if self
            .last_press
            .is_some_and(|last| (last.x - pos.x).abs() < 8.0)
        {
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
        self.last_press = Some(pos);
        self.tick(app, vec![egui::Event::PointerMoved(pos)]);
        self.tick(app, vec![pointer(pos, button, true)]);
        self.tick(app, vec![pointer(pos, button, false)]);
        self.idle(app, 2);
    }

    fn drag(&mut self, app: &mut App, from: egui::Pos2, to: egui::Pos2) {
        if self.last_press.is_some() {
            std::thread::sleep(std::time::Duration::from_millis(400));
        }
        self.last_press = Some(from);
        self.tick(app, vec![egui::Event::PointerMoved(from)]);
        self.tick(app, vec![pointer(from, egui::PointerButton::Primary, true)]);
        for step in 1..=4 {
            let pos = from + (to - from) * (step as f32 / 4.0);
            self.tick(app, vec![egui::Event::PointerMoved(pos)]);
        }
        self.tick(app, vec![pointer(to, egui::PointerButton::Primary, false)]);
        self.idle(app, 2);
    }

    fn key(&mut self, app: &mut App, key: egui::Key, modifiers: egui::Modifiers) {
        self.modifiers = modifiers;
        self.tick(
            app,
            vec![egui::Event::Key {
                key,
                physical_key: Some(key),
                pressed: true,
                repeat: false,
                modifiers,
            }],
        );
        self.modifiers = egui::Modifiers::NONE;
        self.idle(app, 2);
    }

    /// Screen x of the boundary before char `index`, where the field painted it.
    fn painted_x(&self, index: usize) -> f32 {
        let (pos, galley) = self.painted.as_ref().expect("query galley was not painted");
        pos.x
            + galley
                .pos_from_cursor(egui::text::CCursor::new(index))
                .min
                .x
    }

    fn between(&self, from: usize, to: usize, y: f32) -> egui::Pos2 {
        egui::pos2((self.painted_x(from) + self.painted_x(to)) / 2.0, y)
    }

    fn label(&self, text: &str) -> Option<egui::Pos2> {
        self.labels
            .iter()
            .find(|(label, _)| label == text)
            .map(|(_, rect)| rect.center())
    }

    fn range(&self, id: egui::Id) -> Option<(usize, usize)> {
        egui::TextEdit::load_state(&self.ctx, id)
            .and_then(|state| state.cursor.char_range())
            .map(|range| (range.secondary.index.0, range.primary.index.0))
    }
}

fn visit_text(shape: &egui::Shape, visit: &mut impl FnMut(&egui::epaint::TextShape)) {
    match shape {
        egui::Shape::Text(text) => visit(text),
        egui::Shape::Vec(shapes) => shapes.iter().for_each(|shape| visit_text(shape, visit)),
        _ => {}
    }
}

fn pointer(pos: egui::Pos2, button: egui::PointerButton, pressed: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button,
        pressed,
        modifiers: egui::Modifiers::NONE,
    }
}

#[derive(Clone, Copy, Debug)]
struct Bar {
    pinned: bool,
    drag: bool,
}

const BARS: [Bar; 3] = [
    Bar {
        pinned: false,
        drag: false,
    },
    Bar {
        pinned: true,
        drag: false,
    },
    Bar {
        pinned: true,
        drag: true,
    },
];

impl Bar {
    fn id(self) -> egui::Id {
        if self.pinned {
            Kind::Everything.input_id()
        } else {
            Kind::Everything.temporary_id()
        }
    }
}

/// Open the Everything field through the product entry with `query` as its draft.
fn open(bar: Bar, query: &'static str, ppp: f32) -> (AppTestEnv, Frames, egui::Rect) {
    let mut env = setup_app();
    env.initialized = true;
    env.settings.update_check_enabled = false;
    env.settings.minimize_to_tray_on_close = false;
    env.indexer_init = IndexerInit::Unavailable;
    env.settings.show_toolbar_search_everything = bar.pinned;
    env.settings.toolbar_section_drag_enabled = bar.drag;
    let mut frames = Frames::new(query, ppp);
    frames.idle(&mut env, 3);
    env.open_everything_search();
    assert!(
        env.global_search.active,
        "{bar:?}: Everything search did not open"
    );
    env.global_search.everything_draft = query.into();
    frames.idle(&mut env, 4);
    let rect = frames
        .ctx
        .read_response(bar.id())
        .unwrap_or_else(|| panic!("{bar:?}: field was not drawn"))
        .rect;
    assert_eq!(
        frames.ctx.memory(|memory| memory.focused()),
        Some(bar.id()),
        "{bar:?}: opening focuses the field"
    );
    assert_eq!(
        frames.range(bar.id()),
        Some((0, query.chars().count())),
        "{bar:?}: opening selects the previous query once"
    );
    (env, frames, rect)
}

/// The reported operation: drag over part of the query, then cut it with the
/// field's right-click menu. A secondary press used to start a new selection
/// at the pointer (egui's `any_pressed`), so the menu's cut was disabled.
#[test]
fn section1691_right_click_inside_partial_selection_cuts_it() {
    for bar in BARS {
        let (mut env, mut frames, rect) = open(bar, QUERY, 1.0);
        let y = rect.center().y;
        let id = bar.id();
        // "abc" is chars 3..6.
        let (from, to) = (
            egui::pos2(frames.painted_x(3) + 0.5, y),
            egui::pos2(frames.painted_x(6) + 0.5, y),
        );
        frames.drag(&mut env, from, to);
        assert_eq!(frames.range(id), Some((3, 6)), "{bar:?}: drag selection");

        let inside = frames.between(4, 5, y);
        frames.click(&mut env, inside, egui::PointerButton::Secondary);
        assert_eq!(
            frames.range(id),
            Some((3, 6)),
            "{bar:?}: right-click inside the selection keeps it"
        );
        let cut = frames
            .label("切り取り")
            .unwrap_or_else(|| panic!("{bar:?}: context menu is open"));
        frames.click(&mut env, cut, egui::PointerButton::Primary);
        assert_eq!(
            env.global_search.everything_draft, "旅行  ext:jpg;png 日本語 test",
            "{bar:?}: cut removes only the selection"
        );
        assert_eq!(frames.copied, ["abc"], "{bar:?}: cut copies the selection");
        assert_eq!(frames.range(id), Some((3, 3)), "{bar:?}: caret at the cut");
        assert_eq!(
            frames.ctx.memory(|memory| memory.focused()),
            Some(id),
            "{bar:?}: editing continues in the field"
        );
    }
}

/// Existing right-click behavior outside a selection: the caret moves to the
/// pointer, so the menu's paste inserts where the user right-clicked.
#[test]
fn section1691_right_click_outside_selection_still_places_caret() {
    for bar in BARS {
        let (mut env, mut frames, rect) = open(bar, QUERY, 1.0);
        let y = rect.center().y;
        let id = bar.id();
        frames.drag(
            &mut env,
            egui::pos2(frames.painted_x(3) + 0.5, y),
            egui::pos2(frames.painted_x(6) + 0.5, y),
        );
        assert_eq!(frames.range(id), Some((3, 6)), "{bar:?}: drag selection");
        // Inside "ext" (chars 7..10), outside the "abc" selection.
        let outside = frames.between(8, 8, y);
        frames.click(&mut env, outside, egui::PointerButton::Secondary);
        assert_eq!(frames.range(id), Some((8, 8)), "{bar:?}: caret at pointer");
        assert!(frames.label("切り取り").is_some(), "{bar:?}: menu is open");
    }
}

/// Selection by drag, Shift+Arrow and Ctrl+X (egui-winit's Cut event) keep the
/// partial range in both bars, with section reordering on and off.
#[test]
fn section1691_keyboard_and_drag_partial_selection_survive_app_frames() {
    for bar in BARS {
        let (mut env, mut frames, rect) = open(bar, QUERY, 1.0);
        let y = rect.center().y;
        let id = bar.id();
        frames.click(
            &mut env,
            frames.between(4, 4, y),
            egui::PointerButton::Primary,
        );
        assert_eq!(
            frames.range(id),
            Some((4, 4)),
            "{bar:?}: click places caret"
        );
        frames.drag(
            &mut env,
            egui::pos2(frames.painted_x(3) + 0.5, y),
            egui::pos2(frames.painted_x(6) + 0.5, y),
        );
        frames.idle(&mut env, 5);
        assert_eq!(
            frames.range(id),
            Some((3, 6)),
            "{bar:?}: idle keeps drag range"
        );
        frames.key(&mut env, egui::Key::ArrowRight, egui::Modifiers::SHIFT);
        assert_eq!(
            frames.range(id),
            Some((3, 7)),
            "{bar:?}: Shift+Right extends"
        );
        frames.tick(&mut env, vec![egui::Event::Cut]);
        frames.idle(&mut env, 2);
        assert_eq!(
            env.global_search.everything_draft, "旅行 ext:jpg;png 日本語 test",
            "{bar:?}: Ctrl+X cuts the range"
        );
        assert_eq!(frames.copied, ["abc "], "{bar:?}: Ctrl+X copies the range");
        assert_eq!(frames.range(id), Some((3, 3)), "{bar:?}: caret at the cut");
    }
}

/// Clicking inside every visible char lands on the nearer painted boundary, for a
/// horizontally scrolled long query, a fractional DPI, and a first click into an
/// unfocused field. This found no coordinate offset; it guards that finding.
#[test]
fn section1691_click_caret_matches_painted_text() {
    for (bar, unfocused) in [(BARS[0], true), (BARS[1], false)] {
        let (mut env, mut frames, rect) = open(bar, LONG, 1.25);
        let id = bar.id();
        let count = LONG.chars().count();
        let mut mismatches = Vec::new();
        for index in (0..count).step_by(3) {
            for fraction in [0.25_f32, 0.75] {
                if unfocused {
                    frames.ctx.memory_mut(|memory| memory.surrender_focus(id));
                    frames.idle(&mut env, 2);
                }
                let left = frames.painted_x(index);
                let x = left + (frames.painted_x(index + 1) - left) * fraction;
                if x <= rect.left() + 1.0 || x >= rect.right() - 1.0 {
                    continue; // scrolled out of the visible text
                }
                frames.click(
                    &mut env,
                    egui::pos2(x, rect.center().y),
                    egui::PointerButton::Primary,
                );
                let expected = index + usize::from(fraction > 0.5);
                if frames.range(id) != Some((expected, expected)) {
                    mismatches.push((index, fraction, frames.range(id)));
                }
            }
        }
        assert!(mismatches.is_empty(), "{bar:?}: {mismatches:?}");
    }
}

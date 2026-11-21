//! Passive, opt-in observations shared by the product's cfg(test) hooks.
use std::cell::RefCell;

#[derive(Clone, Debug)]
pub(crate) struct Field {
    pub id: egui::Id,
    pub file: &'static str,
    pub line: u32,
    pub pass: u64,
    pub frame: u64,
    pub enabled: bool,
    pub rect: egui::Rect,
    pub interact_rect: egui::Rect,
    pub focused_when_drawn: bool,
}

#[derive(Clone, Debug)]
pub(crate) struct ChromeGate {
    pub pass: u64,
    pub reason: Option<&'static str>,
}

#[derive(Clone, Debug)]
pub(crate) struct FocusEvent {
    pub frame: u64,
    pub pass: u64,
    pub stage: &'static str,
    pub id: egui::Id,
    pub requested: bool,
    pub active: bool,
    pub focused: Option<egui::Id>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Outcome {
    Started,
    Delivered(usize),
    Rejected,
}

#[derive(Clone, Debug)]
pub(crate) struct Job {
    pub frame: u64,
    pub field: &'static str,
    pub query: String,
    pub outcome: Outcome,
}

#[derive(Default)]
struct Observations {
    frame: u64,
    fields: Vec<Field>,
    gates: Vec<ChromeGate>,
    focus_events: Vec<FocusEvent>,
    jobs: Vec<Job>,
    actions: Vec<(crate::keymap::KeyAction, &'static str)>,
}

thread_local! {
    static OBSERVATIONS: RefCell<Option<Observations>> = const { RefCell::new(None) };
}

pub(crate) struct Session;

impl Session {
    pub fn new() -> Self {
        OBSERVATIONS.with_borrow_mut(|slot| {
            assert!(slot.is_none(), "nested typing observation session");
            *slot = Some(Observations::default());
        });
        Self
    }
}

impl Drop for Session {
    fn drop(&mut self) {
        OBSERVATIONS.with_borrow_mut(|slot| *slot = None);
    }
}

pub(crate) fn begin_frame() {
    OBSERVATIONS.with_borrow_mut(|slot| {
        if let Some(observations) = slot {
            observations.frame += 1;
            observations.fields.clear();
            observations.gates.clear();
            observations.focus_events.clear();
        }
    });
}

pub(crate) fn field(response: &egui::Response, site: &'static std::panic::Location<'static>) {
    OBSERVATIONS.with_borrow_mut(|slot| {
        if let Some(observations) = slot {
            observations.fields.push(Field {
                id: response.id,
                file: site.file(),
                line: site.line(),
                pass: response.ctx.cumulative_pass_nr(),
                frame: observations.frame,
                enabled: response.enabled(),
                rect: response.rect,
                interact_rect: response.interact_rect,
                focused_when_drawn: response.has_focus(),
            });
        }
    });
}

pub(crate) fn focus_event(
    ctx: &egui::Context,
    stage: &'static str,
    id: egui::Id,
    requested: bool,
    active: bool,
) {
    OBSERVATIONS.with_borrow_mut(|slot| {
        if let Some(observations) = slot {
            observations.focus_events.push(FocusEvent {
                frame: observations.frame,
                pass: ctx.cumulative_pass_nr(),
                stage,
                id,
                requested,
                active,
                focused: ctx.memory(|memory| memory.focused()),
            });
        }
    });
}

pub(crate) fn focus_events() -> Vec<FocusEvent> {
    OBSERVATIONS.with_borrow(|slot| {
        slot.as_ref()
            .map(|o| o.focus_events.clone())
            .unwrap_or_default()
    })
}

pub(crate) fn chrome_gate(ctx: &egui::Context, reason: Option<&'static str>) {
    OBSERVATIONS.with_borrow_mut(|slot| {
        if let Some(observations) = slot {
            observations.gates.push(ChromeGate {
                pass: ctx.cumulative_pass_nr(),
                reason,
            });
        }
    });
}

pub(crate) fn gates() -> Vec<ChromeGate> {
    OBSERVATIONS.with_borrow(|slot| slot.as_ref().map(|o| o.gates.clone()).unwrap_or_default())
}

pub(crate) fn job(field: &'static str, query: &str, outcome: Outcome) {
    OBSERVATIONS.with_borrow_mut(|slot| {
        if let Some(observations) = slot {
            observations.jobs.push(Job {
                frame: observations.frame,
                field,
                query: query.into(),
                outcome,
            });
        }
    });
}

pub(crate) fn action(action: crate::keymap::KeyAction, source: &'static str) {
    OBSERVATIONS.with_borrow_mut(|slot| {
        if let Some(observations) = slot {
            observations.actions.push((action, source));
        }
    });
}

pub(crate) fn fields() -> Vec<Field> {
    OBSERVATIONS.with_borrow(|slot| slot.as_ref().map(|o| o.fields.clone()).unwrap_or_default())
}

pub(crate) fn jobs() -> Vec<Job> {
    OBSERVATIONS.with_borrow(|slot| slot.as_ref().map(|o| o.jobs.clone()).unwrap_or_default())
}

pub(crate) fn take_actions() -> Vec<(crate::keymap::KeyAction, &'static str)> {
    OBSERVATIONS.with_borrow_mut(|slot| {
        slot.as_mut()
            .map(|o| std::mem::take(&mut o.actions))
            .unwrap_or_default()
    })
}

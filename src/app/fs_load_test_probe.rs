//! Fake only source-worker dispatch; request admission, tickets and result adoption stay real.
use super::*;

#[derive(Clone, Debug)]
pub(crate) struct Request {
    pub(crate) idx: usize,
    pub(crate) pdf_target: Option<crate::pdf_loader::PdfDisplayTarget>,
    pub(crate) target_px: Option<u32>,
    pub(crate) priority: FsPageLoadPriority,
}

#[derive(Default)]
struct State {
    requests: Vec<Request>,
    workers: HashMap<usize, mpsc::Sender<FsLoadResult>>,
}

thread_local! {
    static STATE: std::cell::RefCell<Option<State>> = const { std::cell::RefCell::new(None) };
}

pub(crate) struct Probe;

impl Probe {
    pub(crate) fn start() -> Self {
        STATE.with(|state| {
            assert!(state.borrow().is_none());
            *state.borrow_mut() = Some(State::default());
        });
        Self
    }

    pub(crate) fn take_requests(&self) -> Vec<Request> {
        STATE.with(|state| std::mem::take(&mut state.borrow_mut().as_mut().unwrap().requests))
    }

    pub(crate) fn complete(&self, idx: usize, result: FsLoadResult) {
        STATE.with(|state| {
            state
                .borrow_mut()
                .as_mut()
                .unwrap()
                .workers
                .remove(&idx)
                .unwrap()
                .send(result)
                .unwrap();
        });
    }
}

impl Drop for Probe {
    fn drop(&mut self) {
        STATE.with(|state| *state.borrow_mut() = None);
    }
}

pub(super) fn intercept(
    app: &mut App,
    idx: usize,
    sender: &mpsc::Sender<FsLoadResult>,
    pdf_target: Option<crate::pdf_loader::PdfDisplayTarget>,
    target_px: Option<u32>,
    priority: FsPageLoadPriority,
) -> bool {
    STATE.with(|state| {
        let mut state = state.borrow_mut();
        let Some(state) = state.as_mut() else {
            return false;
        };
        let pending = app.fs_pending.get_mut(&idx).unwrap();
        pending.probe_completion = Some(pending.ticket.completion_without_worker_for_test());
        state.requests.push(Request {
            idx,
            pdf_target,
            target_px,
            priority,
        });
        state.workers.insert(idx, sender.clone());
        true
    })
}

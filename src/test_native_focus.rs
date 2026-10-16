//! Headless application focus observations and requests. Never calls Win32.
use std::cell::RefCell;
use std::marker::PhantomData;
use std::rc::Rc;

#[derive(Default)]
struct State {
    foreground: u64,
    cursor_target: u64,
    foreground_owned: bool,
    requests: Vec<u64>,
}

thread_local! {
    static STATE: RefCell<State> = RefCell::new(State::default());
}

pub(crate) fn foreground_hwnd() -> u64 {
    STATE.with_borrow(|state| state.foreground)
}

pub(crate) fn foreground_belongs_to_current_process_strict() -> bool {
    STATE.with_borrow(|state| state.foreground != 0 && state.foreground_owned)
}

pub(crate) fn cursor_focus_target() -> (u64, u64) {
    STATE.with_borrow(|state| (state.foreground, state.cursor_target))
}

pub(crate) fn claim_foreground(target: u64) -> (u64, u64) {
    STATE.with_borrow_mut(|state| {
        let before = state.foreground;
        state.requests.push(target);
        if target != 0 {
            state.foreground = target;
            state.foreground_owned = true;
        }
        (before, state.foreground)
    })
}

pub(crate) fn requests() -> Vec<u64> {
    STATE.with_borrow(|state| state.requests.clone())
}

pub(crate) struct Scope {
    previous: State,
    _thread_bound: PhantomData<Rc<()>>,
}

pub(crate) fn scoped(foreground: u64, cursor_target: u64, foreground_owned: bool) -> Scope {
    let previous = STATE.replace(State {
        foreground,
        cursor_target,
        foreground_owned,
        requests: Vec::new(),
    });
    Scope {
        previous,
        _thread_bound: PhantomData,
    }
}

impl Drop for Scope {
    fn drop(&mut self) {
        STATE.replace(std::mem::take(&mut self.previous));
    }
}

#[test]
fn headless_focus_scope_restores_observations_and_isolates_threads() {
    assert_eq!(cursor_focus_target(), (0, 0));
    assert!(!foreground_belongs_to_current_process_strict());
    {
        let _scope = scoped(11, 22, false);
        std::thread::spawn(|| {
            assert_eq!(cursor_focus_target(), (0, 0));
            claim_foreground(99);
            assert_eq!(requests(), [99]);
        })
        .join()
        .unwrap();
        assert!(requests().is_empty());
        assert_eq!(claim_foreground(22), (11, 22));
        assert!(foreground_belongs_to_current_process_strict());
        {
            let _nested = scoped(33, 44, true);
            assert_eq!(cursor_focus_target(), (33, 44));
        }
        assert_eq!(cursor_focus_target(), (22, 22));
        assert_eq!(requests(), [22]);
    }
    assert_eq!(cursor_focus_target(), (0, 0));
    assert!(requests().is_empty());
}

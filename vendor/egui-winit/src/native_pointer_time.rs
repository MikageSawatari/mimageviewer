//! A queue timestamp belongs to one exact wndproc invocation, never merely to
//! matching button attributes. No RefCell borrow crosses DefSubclassProc.
#![expect(unsafe_code)]

use std::cell::RefCell;
use windows_sys::Win32::{
    Foundation::{HWND, LPARAM, LRESULT, WPARAM},
    System::SystemInformation::GetTickCount,
    UI::{
        Shell::{DefSubclassProc, GetWindowSubclass, RemoveWindowSubclass, SetWindowSubclass},
        WindowsAndMessaging::{MSG, WM_NCDESTROY},
    },
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct Message {
    hwnd: isize,
    message: u32,
    wparam: usize,
    lparam: isize,
}

impl Message {
    fn button(self) -> Option<(egui::PointerButton, bool)> {
        use egui::PointerButton::*;
        let (button, pressed) = match self.message {
            0x0201 | 0x0203 => (Primary, true), // DOWN or DBLCLK
            0x0202 => (Primary, false),
            0x0204 | 0x0206 => (Secondary, true),
            0x0205 => (Secondary, false),
            0x0207 | 0x0209 => (Middle, true),
            0x0208 => (Middle, false),
            0x020B | 0x020C | 0x020D => {
                let button = match (self.wparam >> 16) & 0xffff {
                    1 => Extra1,
                    2 => Extra2,
                    _ => return None,
                };
                (button, self.message != 0x020C)
            }
            _ => return None,
        };
        Some((button, pressed))
    }
}

#[derive(Clone, Copy)]
enum CallTime {
    Unavailable,
    Available(f64),
    Used,
    Ambiguous,
}

struct Call {
    message: Message,
    time: CallTime,
}

#[derive(Default)]
struct Dispatch {
    received: Option<(Message, Option<f64>)>,
    calls: Vec<Call>,
}

impl Dispatch {
    fn receive(&mut self, message: Message, time: Option<f64>) {
        self.received = message.button().map(|_| (message, time));
    }

    fn enter(&mut self, message: Message) {
        let nested_button = self
            .calls
            .iter()
            .any(|call| call.message.button().is_some());
        if message.button().is_some() {
            // A buffered nested MouseInput can later be delivered while its
            // outer invocation is still active. Neither may borrow that time.
            for call in &mut self.calls {
                if !matches!(call.time, CallTime::Used) {
                    call.time = CallTime::Ambiguous;
                }
            }
        }
        let time = if message.button().is_some()
            && self
                .received
                .as_ref()
                .is_some_and(|(queued, _)| *queued == message)
        {
            self.received.take().and_then(|(_, time)| time)
        } else {
            None
        };
        self.calls.push(Call {
            message,
            time: time
                .filter(|_| !nested_button)
                .map_or(CallTime::Unavailable, CallTime::Available),
        });
    }

    fn leave(&mut self) {
        self.calls
            .pop()
            .expect("balanced native pointer invocation");
    }

    fn take(&mut self, hwnd: isize, button: egui::PointerButton, pressed: bool) -> Option<f64> {
        let call = self.calls.last_mut()?;
        if call.message.hwnd != hwnd || call.message.button() != Some((button, pressed)) {
            return None;
        }
        if let CallTime::Available(time) = call.time {
            call.time = CallTime::Used;
            Some(time)
        } else {
            None
        }
    }

    fn retire(&mut self, hwnd: isize) {
        if self
            .received
            .as_ref()
            .is_some_and(|(message, _)| message.hwnd == hwnd)
        {
            self.received = None;
        }
        for call in &mut self.calls {
            if call.message.hwnd == hwnd {
                call.time = CallTime::Ambiguous;
            }
        }
    }
}

thread_local! {
    static DISPATCH: RefCell<Dispatch> = RefCell::new(Dispatch::default());
}

fn from_ticks(now: f64, ticks: u32, message_ticks: u32) -> Option<f64> {
    let age = ticks.wrapping_sub(message_ticks);
    (age <= 5_000).then_some(now - f64::from(age) / 1000.0)
}

/// Record the MSG just removed by winit. A user hook may replace this hook;
/// then no message can be claimed and input retains pass-time behavior.
///
/// # Safety
/// `message` must point to the live MSG supplied by winit's msg hook.
pub unsafe fn message_hook(message: *const core::ffi::c_void) -> bool {
    // SAFETY: the caller forwards winit's hook argument unchanged.
    let message = unsafe { &*message.cast::<MSG>() };
    let time = from_ticks(
        crate::input_clock::now(),
        unsafe { GetTickCount() },
        message.time,
    );
    let identity = Message {
        hwnd: message.hwnd as isize,
        message: message.message,
        wparam: message.wParam,
        lparam: message.lParam,
    };
    DISPATCH.with(|dispatch| dispatch.borrow_mut().receive(identity, time));
    false
}

pub(crate) fn take(
    window: &winit::window::Window,
    button: egui::PointerButton,
    pressed: bool,
) -> Option<f64> {
    use raw_window_handle::HasWindowHandle as _;
    let raw_window_handle::RawWindowHandle::Win32(handle) = window.window_handle().ok()?.as_raw()
    else {
        return None;
    };
    DISPATCH.with(|dispatch| {
        dispatch
            .borrow_mut()
            .take(handle.hwnd.get(), button, pressed)
    })
}

// Distinct from application input subclasses; function address is process-local
// and stable for the lifetime of every installed subclass.
fn subclass_id() -> usize {
    subclass as *const () as usize
}

fn restore_outermost_with(
    lookup: impl FnOnce() -> Option<usize>,
    remove: impl FnOnce() -> bool,
    install: impl FnOnce(usize) -> bool,
) -> bool {
    let Some(data) = lookup() else {
        return true;
    };
    // SetWindowSubclass with an existing proc/id only updates refData; it does
    // not move that invocation boundary ahead of a later listener subclass.
    remove() && install(data)
}

/// A replayed application listener may install a subclass above ours on ROOT.
/// Restore the timestamp boundary outside that listener before it can receive
/// input. Newly created children do not have our subclass yet at this point.
pub fn after_listener_install(hwnd: isize) {
    let hwnd = hwnd as HWND;
    let restored = restore_outermost_with(
        || {
            let mut data = 0;
            // SAFETY: a query for our own proc/id; an invalid or headless HWND
            // simply has no installed subclass and needs no native mutation.
            (unsafe { GetWindowSubclass(hwnd, Some(subclass), subclass_id(), &mut data) } != 0)
                .then_some(data)
        },
        || unsafe { RemoveWindowSubclass(hwnd, Some(subclass), subclass_id()) != 0 },
        |data| unsafe { SetWindowSubclass(hwnd, Some(subclass), subclass_id(), data) != 0 },
    );
    if !restored {
        log::warn!("Could not restore native pointer timestamp subclass order");
    }
}

/// Native resource retired by eframe's existing allocation lease.
pub struct WindowTimestamp {
    hwnd: isize,
}

impl WindowTimestamp {
    pub fn install(hwnd: isize) -> Option<Self> {
        // SAFETY: called by eframe on the owning window thread at allocation.
        let installed =
            unsafe { SetWindowSubclass(hwnd as HWND, Some(subclass), subclass_id(), 0) };
        if installed == 0 {
            log::warn!("Could not install native pointer timestamp subclass");
            None
        } else {
            Some(Self { hwnd })
        }
    }
}

impl Drop for WindowTimestamp {
    fn drop(&mut self) {
        DISPATCH.with(|dispatch| dispatch.borrow_mut().retire(self.hwnd));
        // SAFETY: the allocation owner retires on its backend/window thread.
        unsafe {
            RemoveWindowSubclass(self.hwnd as HWND, Some(subclass), subclass_id());
        }
    }
}

unsafe extern "system" fn subclass(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
    _id: usize,
    _data: usize,
) -> LRESULT {
    let identity = Message {
        hwnd: hwnd as isize,
        message,
        wparam,
        lparam,
    };
    DISPATCH.with(|dispatch| dispatch.borrow_mut().enter(identity));
    if message == WM_NCDESTROY {
        DISPATCH.with(|dispatch| dispatch.borrow_mut().retire(hwnd as isize));
        // SAFETY: remove only this subclass while the native window still exists.
        unsafe {
            RemoveWindowSubclass(hwnd, Some(subclass), subclass_id());
        }
    }
    // SAFETY: preserve the native chain, with this invocation already claimed.
    let result = unsafe { DefSubclassProc(hwnd, message, wparam, lparam) };
    DISPATCH.with(|dispatch| dispatch.borrow_mut().leave());
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::PointerButton::Primary;

    fn button(hwnd: isize) -> Message {
        Message {
            hwnd,
            message: 0x0201,
            wparam: 1,
            lparam: 20,
        }
    }

    #[derive(Clone, Copy, Debug, PartialEq, Eq)]
    enum Layer {
        Winit,
        Timestamp,
        Listener,
    }

    fn restore_chain(chain: &RefCell<Vec<Layer>>) {
        assert!(restore_outermost_with(
            || chain.borrow().contains(&Layer::Timestamp).then_some(37),
            || {
                let mut chain = chain.borrow_mut();
                let index = chain
                    .iter()
                    .position(|layer| *layer == Layer::Timestamp)
                    .unwrap();
                chain.remove(index);
                true
            },
            |data| {
                assert_eq!(data, 37);
                chain.borrow_mut().push(Layer::Timestamp);
                true
            },
        ));
    }

    fn deliver_chain(
        chain: &[Layer],
        index: usize,
        state: &mut Dispatch,
        message: Message,
        reenter: bool,
        times: &mut Vec<Option<f64>>,
    ) {
        match chain[index] {
            Layer::Winit => times.push(state.take(message.hwnd, Primary, true)),
            Layer::Timestamp => {
                state.enter(message);
                deliver_chain(chain, index - 1, state, message, reenter, times);
                state.leave();
            }
            Layer::Listener => {
                if reenter {
                    deliver_chain(chain, chain.len() - 1, state, message, false, times);
                }
                deliver_chain(chain, index - 1, state, message, false, times);
            }
        }
    }

    #[test]
    fn timestamp_919_listener_replay_and_new_child_keep_timestamp_outermost() {
        let root = RefCell::new(vec![Layer::Winit, Layer::Timestamp, Layer::Listener]);
        restore_chain(&root); // ROOT: timestamp already installed before listener replay.
        assert_eq!(
            *root.borrow(),
            [Layer::Winit, Layer::Listener, Layer::Timestamp]
        );
        let child = RefCell::new(vec![Layer::Winit, Layer::Listener]);
        restore_chain(&child); // Child: listener installs before initial timestamp subclass.
        assert_eq!(*child.borrow(), [Layer::Winit, Layer::Listener]);
        child.borrow_mut().push(Layer::Timestamp);
        assert_eq!(*child.borrow(), *root.borrow());
    }

    #[test]
    fn timestamp_919_listener_outermost_restoration_prevents_preclaim_reentry() {
        let chain = RefCell::new(vec![Layer::Winit, Layer::Timestamp, Layer::Listener]);
        let message = button(1);
        let deliver = || {
            let mut state = Dispatch::default();
            state.receive(message, Some(1.0));
            let mut times = Vec::new();
            let chain = chain.borrow();
            deliver_chain(
                &chain,
                chain.len() - 1,
                &mut state,
                message,
                true,
                &mut times,
            );
            times
        };
        assert_eq!(
            deliver(),
            [Some(1.0), None],
            "unrestored listener steals time before the outer timestamp frame starts"
        );
        restore_chain(&chain);
        assert_eq!(
            deliver(),
            [None, None],
            "nested and outer frames become unprovable together"
        );
    }

    #[test]
    fn timestamp_919_tick_age_range_and_wrap() {
        assert_eq!(from_ticks(10.0, 100, 100), Some(10.0));
        assert_eq!(from_ticks(10.0, 5_100, 100), Some(5.0));
        assert_eq!(from_ticks(10.0, 5_101, 100), None);
        assert_eq!(from_ticks(10.0, 100, 101), None);
        assert_eq!(from_ticks(10.0, 9, u32::MAX), Some(9.99));
    }

    #[test]
    fn timestamp_919_claims_exact_invocation_once_and_rejects_deferred_delivery() {
        let mut state = Dispatch::default();
        let message = button(1);
        state.receive(message, Some(1.0));
        state.enter(message);
        assert_eq!(state.take(2, Primary, true), None);
        assert_eq!(state.take(1, Primary, false), None);
        assert_eq!(state.take(1, Primary, true), Some(1.0));
        assert_eq!(state.take(1, Primary, true), None);
        state.leave();
        assert_eq!(state.take(1, Primary, true), None);
        // Same attributes do not create a new received queue message.
        state.enter(message);
        assert_eq!(state.take(1, Primary, true), None);
        state.leave();
    }

    #[test]
    fn timestamp_919_all_message_attributes_must_match() {
        let original = button(1);
        for other in [
            Message {
                hwnd: 2,
                ..original
            },
            Message {
                message: 0x0202,
                ..original
            },
            Message {
                wparam: 2,
                ..original
            },
            Message {
                lparam: 21,
                ..original
            },
        ] {
            let mut state = Dispatch::default();
            state.receive(original, Some(1.0));
            state.enter(other);
            assert_eq!(
                state.take(other.hwnd, Primary, other.message == 0x0201),
                None
            );
            state.leave();
            state.enter(original);
            assert_eq!(state.take(1, Primary, true), Some(1.0));
            state.leave();
        }
    }

    #[test]
    fn timestamp_919_nested_button_invalidates_outer_even_when_buffer_delivered_first() {
        let mut state = Dispatch::default();
        let message = button(1);
        state.receive(message, Some(1.0));
        state.enter(message);
        state.enter(message); // SendMessage in update_modifiers; runner buffers input.
        assert_eq!(state.take(1, Primary, true), None);
        state.leave();
        assert_eq!(state.take(1, Primary, true), None); // buffered inner
        assert_eq!(state.take(1, Primary, true), None); // outer
        state.leave();
    }

    #[test]
    fn timestamp_919_nonbutton_reentry_preserves_outer_but_never_lends_it() {
        let mut state = Dispatch::default();
        let message = button(1);
        state.receive(message, Some(1.0));
        state.enter(message);
        state.enter(Message {
            message: 0x0215,
            ..message
        }); // WM_CAPTURECHANGED
        assert_eq!(state.take(1, Primary, true), None);
        state.leave();
        assert_eq!(state.take(1, Primary, true), Some(1.0));
        state.leave();
    }

    #[test]
    fn timestamp_919_nested_loop_and_sent_messages_have_no_queue_time() {
        let mut state = Dispatch::default();
        let message = button(1);
        state.receive(
            Message {
                message: 0x000f,
                ..message
            },
            None,
        );
        state.enter(Message {
            message: 0x000f,
            ..message
        });
        state.enter(message);
        assert_eq!(state.take(1, Primary, true), None);
        state.leave();
        state.leave();
        state.enter(message);
        assert_eq!(state.take(1, Primary, true), None);
        state.leave();
    }

    #[test]
    fn timestamp_919_retire_recreate_does_not_change_other_window_queue_or_call() {
        let mut state = Dispatch::default();
        let b = button(2);
        state.receive(b, Some(2.0));
        state.enter(b);
        state.retire(1);
        assert_eq!(state.take(2, Primary, true), Some(2.0));
        state.leave();
        state.receive(b, Some(3.0));
        state.retire(1);
        state.enter(b);
        assert_eq!(state.take(2, Primary, true), Some(3.0));
        state.leave();
        let a = button(1);
        state.receive(a, Some(4.0));
        state.retire(1);
        state.enter(a); // HWND reuse cannot claim the retired allocation's record.
        assert_eq!(state.take(1, Primary, true), None);
        state.leave();
    }
}

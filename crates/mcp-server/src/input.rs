//! One bounded FIFO owns input until it is handed directly to Active.
use super::{ActiveOwner, cancel_matching, error};
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, Condvar, Mutex},
};

pub(super) const CAPACITY: usize = 32;
pub(super) enum InputLine {
    Request {
        bytes: Vec<u8>,
        id: Value,
        method: String,
    },
    Other(Vec<u8>),
    Rejected(Value),
}
impl InputLine {
    fn id(&self) -> Option<&Value> {
        match self {
            Self::Request { id, .. } => Some(id),
            _ => None,
        }
    }
}
enum State {
    Open(VecDeque<InputLine>),
    Closed,
}
pub(super) struct InputQueue {
    state: Mutex<State>,
    space: Condvar,
}
impl InputQueue {
    pub(super) fn new() -> Self {
        Self {
            state: Mutex::new(State::Open(VecDeque::new())),
            space: Condvar::new(),
        }
    }
    pub(super) fn push(&self, bytes: Vec<u8>, owner: &ActiveOwner) -> bool {
        let request = serde_json::from_slice::<Value>(&bytes)
            .ok()
            .and_then(|value| {
                if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
                    return None;
                }
                Some((
                    value.get("id").filter(|id| valid_id(id))?.clone(),
                    value.get("method")?.as_str()?.to_owned(),
                ))
            });
        let mut state = self.state.lock().unwrap();
        loop {
            match &mut *state {
                State::Closed => return false,
                State::Open(lines) if lines.len() < CAPACITY => {
                    let line = if let Some((id, method)) = request {
                        if lines.iter().any(|line| line.id() == Some(&id))
                            || owner.lock().unwrap().as_ref().is_some_and(|a| a.id == id)
                        {
                            InputLine::Rejected(error(id, -32600, "Request id is already in use"))
                        } else {
                            InputLine::Request { bytes, id, method }
                        }
                    } else {
                        InputLine::Other(bytes)
                    };
                    lines.push_back(line);
                    return true;
                }
                State::Open(_) => state = self.space.wait(state).unwrap(),
            }
        }
    }
    /// Hold the queue lock through Active registration, never through stdout/IPC.
    pub(super) fn dispatch_next<T>(
        &self,
        owner: &ActiveOwner,
        dispatch: impl FnOnce(InputLine) -> T,
    ) -> Option<T> {
        let mut state = self.state.lock().unwrap();
        if owner.lock().unwrap().is_some() {
            return None;
        }
        let State::Open(lines) = &mut *state else {
            return None;
        };
        let line = lines.pop_front()?;
        let result = dispatch(line);
        self.space.notify_one();
        Some(result)
    }
    /// The same lock order (queue -> Active) covers queued and executing calls.
    pub(super) fn cancel_notification(&self, owner: &ActiveOwner, bytes: &[u8]) -> bool {
        let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
            return false;
        };
        if value.get("jsonrpc").and_then(Value::as_str) != Some("2.0")
            || value.get("id").is_some()
            || value.get("method").and_then(Value::as_str) != Some("notifications/cancelled")
        {
            return false;
        }
        if let Some(id) = value.pointer("/params/requestId").filter(|id| valid_id(id)) {
            let mut state = self.state.lock().unwrap();
            if let State::Open(lines) = &mut *state {
                lines.retain(|line| !matches!(line, InputLine::Request { id: queued, method, .. } if queued == id && method != "initialize"));
            }
            cancel_matching(owner, Some(id));
            self.space.notify_one();
        }
        true
    }
    pub(super) fn close(&self) {
        *self.state.lock().unwrap() = State::Closed;
        self.space.notify_all();
    }
}
fn valid_id(id: &Value) -> bool {
    id.is_number() || id.as_str().is_some_and(|s| s.len() <= 256)
}
pub(super) struct InputLifetime(pub(super) Arc<InputQueue>);
impl Drop for InputLifetime {
    fn drop(&mut self) {
        self.0.close();
    }
}

//! In-memory host boundary for delivery regressions. No child or native window is created.
#![cfg(test)]

use super::*;

pub(crate) struct FakeHostEvents {
    event_tx: crossbeam_channel::Sender<std::io::Result<Event>>,
    visibility_tx: crossbeam_channel::Sender<GuiVisibilitySignal>,
    bypass_tx: crossbeam_channel::Sender<u64>,
    pending: PendingGuiRequests,
}

impl FakeHostEvents {
    pub(crate) fn emit(&self, event: Event) {
        // Keep the event schema and ordered GUI/ACK router used by the stdout pump.
        let event: Event = serde_json::from_slice(&serde_json::to_vec(&event).unwrap()).unwrap();
        match event {
            Event::GuiVisibilityResult { .. }
            | Event::GuiUserHidden { .. }
            | Event::GuiBypassToggle { .. } => route_gui_event(
                &event,
                &self.visibility_tx,
                &self.bypass_tx,
                None,
                &self.pending,
            ),
            other => self.event_tx.send(Ok(other)).unwrap(),
        }
    }
}

impl Bridge {
    pub(crate) fn fake_host_for_test() -> (
        Arc<Self>,
        std::sync::mpsc::Receiver<serde_json::Value>,
        FakeHostEvents,
    ) {
        let (commands, command_rx) = std::sync::mpsc::channel();
        let (event_tx, event_rx) = crossbeam_channel::bounded(64);
        let (_, reset_ack_rx) = crossbeam_channel::bounded(8);
        let (visibility_tx, gui_visibility_rx) = crossbeam_channel::unbounded();
        let (bypass_tx, gui_bypass_toggle_rx) = crossbeam_channel::bounded(64);
        let pending_gui_requests = Arc::new(Mutex::new(HashMap::new()));
        let events = FakeHostEvents {
            event_tx,
            visibility_tx,
            bypass_tx,
            pending: Arc::clone(&pending_gui_requests),
        };
        (
            Arc::new(Self {
                child: None,
                stdin: Mutex::new(None),
                fake_commands: Some(commands),
                event_rx,
                sync_call_mutex: Mutex::new(()),
                cached_latency_samples: Arc::new(AtomicU32::new(0)),
                cached_latency_by_slot: Arc::new(Mutex::new(HashMap::new())),
                reset_ack_rx,
                gui_visibility_rx,
                gui_bypass_toggle_rx,
                next_reset_id: AtomicU64::new(0),
                next_request_id: AtomicU64::new(0),
                pending_state_queries: Arc::new(Mutex::new(HashMap::new())),
                pending_gui_requests,
                #[cfg(windows)]
                shm: None,
                #[cfg(windows)]
                sig_in: None,
                #[cfg(windows)]
                sig_out: None,
            }),
            command_rx,
            events,
        )
    }
}

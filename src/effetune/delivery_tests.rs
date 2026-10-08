//! Actual controller worker -> DspBridge -> checked host command/ACK delivery.
//! Only the process/editor boundary is fake; no product binary or HWND is launched.
use super::*;
use crate::video::dsp::bridge::{Bridge, Event, GuiVisibilityOutcome};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum DeliveryStage {
    Loading,
    HiddenAttach,
    HostDelivery,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct HostSnapshot {
    pub attached: usize,
    pub delivered: usize,
    pub requested: bool,
    pub visible: bool,
    pub shows: usize,
    pub activate: usize,
    pub auto_activate: usize,
    pub auto_revision: Option<u64>,
    pub outcome: Option<GuiVisibilityOutcome>,
}

pub(crate) struct FakeHost {
    pub bridge: Arc<DspBridge>,
    reached: mpsc::Receiver<DeliveryStage>,
    release: mpsc::Sender<()>,
    snapshots: mpsc::Receiver<HostSnapshot>,
    commands: Arc<Bridge>,
    worker: Option<std::thread::JoinHandle<()>>,
}

impl FakeHost {
    pub(crate) fn new(controller: &EffetuneController, stop: DeliveryStage) -> Self {
        let gate = controller.gui_gate().unwrap();
        let (commands, command_rx, events) = Bridge::fake_host_for_test();
        let bridge = new_bridge();
        bridge.set_gui_gate(Arc::clone(&gate));
        bridge.install_fake_host_for_test(Arc::clone(&commands));
        // Same dispatch ownership as load_worker, including recovery reconciliation.
        let tx = controller.host_tx.clone();
        bridge.set_gui_command_dispatch(Arc::new(move |bridge, value| {
            tx.send(HostCommand::Gui { bridge, value }).unwrap();
        }));
        let (reached_tx, reached) = mpsc::channel();
        let (release, release_rx) = mpsc::channel();
        let (snapshot_tx, snapshots) = mpsc::channel();
        let worker = std::thread::spawn(move || {
            let mut state = HostSnapshot::default();
            let mut paused = false;
            while let Ok(value) = command_rx.recv() {
                let cmd = value["cmd"].as_str().unwrap();
                match cmd {
                    "query_gui_size" => events.emit(Event::GuiSize {
                        width: 640,
                        height: 480,
                        resizable: true,
                    }),
                    "show_gui" => {
                        assert_eq!(value["visible"], 0, "attach must remain hidden");
                        assert_eq!(value["unowned"], 1);
                        assert_eq!(value["gui_gate_name"], gate.name());
                        state.attached += 1;
                        if stop == DeliveryStage::HiddenAttach && !paused {
                            reached_tx.send(stop).unwrap();
                            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                            paused = true;
                        }
                        events.emit(Event::GuiAttached {
                            width: 640,
                            height: 480,
                            slot_id: 0,
                            container_hwnd: 1,
                        });
                    }
                    "set_gui_visibility_checked" => {
                        state.delivered += 1;
                        let auto = value["auto_video"] == 1;
                        let revision = value["auto_revision"].as_u64().unwrap();
                        if auto {
                            state.auto_revision = Some(revision);
                        }
                        if stop == DeliveryStage::HostDelivery && !paused {
                            reached_tx.send(stop).unwrap();
                            release_rx.recv_timeout(Duration::from_secs(10)).unwrap();
                            paused = true;
                        }
                        // Model GuiVisibility::accept_show's final native host boundary.
                        // This is deliberately after delivery, never a worker-side allows().
                        let current = gate.auto_snapshot();
                        let allowed = value["minimized_sequence"] == gate.minimized_sequence()
                            && value["remote_token"] == gate.remote()
                            && gate.remote() & 1 == 0
                            && (!auto || (current.allowed && revision == current.revision));
                        let outcome = if value["visible"] == 0 {
                            state.requested = false;
                            state.visible = false;
                            GuiVisibilityOutcome::Hidden
                        } else if allowed {
                            state.requested = true;
                            state.visible = true;
                            state.shows += 1;
                            // The host's Auto presentation uses show-without-activation.
                            if !auto {
                                state.activate += 1;
                            }
                            GuiVisibilityOutcome::Shown
                        } else {
                            GuiVisibilityOutcome::Cancelled
                        };
                        state.outcome = Some(outcome);
                        events.emit(Event::GuiVisibilityResult {
                            request_id: value["request_id"].as_u64().unwrap(),
                            slot_id: 0,
                            outcome,
                        });
                    }
                    "set_gui_remote_session" => {}
                    "sync_gui_main_visibility" => {
                        // Suppression recovery can restore only an accepted visible intent.
                        state.visible = state.requested && gate.remote() & 1 == 0;
                    }
                    "activate_gui" => {
                        state.activate += 1;
                        if value["auto_video"] == 1 {
                            state.auto_activate += 1;
                        }
                    }
                    "fixture_fence" => snapshot_tx.send(state.clone()).unwrap(),
                    "shutdown" => break,
                    other => panic!("unexpected fake host command: {other}"),
                }
            }
        });
        Self {
            bridge,
            reached,
            release,
            snapshots,
            commands,
            worker: Some(worker),
        }
    }

    pub(crate) fn wait_stage(&self, stage: DeliveryStage) {
        assert_eq!(
            self.reached.recv_timeout(Duration::from_secs(10)).unwrap(),
            stage
        );
    }
    pub(crate) fn release_stage(&self) {
        self.release.send(()).unwrap();
    }
    pub(crate) fn fence(&self, controller: &EffetuneController) -> HostSnapshot {
        controller
            .host_tx
            .send(HostCommand::Gui {
                bridge: Arc::clone(&self.commands),
                value: serde_json::json!({"cmd": "fixture_fence"}),
            })
            .unwrap();
        self.snapshots
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
    }
    pub(crate) fn reconcile(&self, controller: &EffetuneController) -> HostSnapshot {
        controller
            .host_tx
            .send(HostCommand::ReconcileVisibility)
            .unwrap();
        // Reconcile queues a Gui command at the tail of the same worker queue.
        self.fence(controller);
        self.fence(controller)
    }
}
impl Drop for FakeHost {
    fn drop(&mut self) {
        let _ = self.release.send(());
        let _ = self
            .commands
            .send_value(&serde_json::json!({"cmd": "shutdown"}));
        if let Some(worker) = self.worker.take() {
            if let Err(error) = worker.join() {
                if !std::thread::panicking() {
                    std::panic::resume_unwind(error);
                }
            }
        }
    }
}

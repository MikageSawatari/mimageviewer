//! The browser pipe feeds the SAME activation channel; no activation event is signalled.
use super::*;
use mimageviewer_browser_view_ipc as ipc;

pub(super) fn activation(
    request: ipc::Request,
    remote: Option<&crate::remote_ipc::session::SessionHandle>,
) -> ActivationOpenPath {
    // Capture receive ordering and Remote ownership immediately, before cleanup/repaint.
    let mut received = ActivationOpenPath::received(request.path, remote);
    received.request =
        crate::app::ViewerOpenRequest::new(crate::app::ViewerOpenIntent::BrowserOverlay {
            placement: placement(request.placement),
        });
    received
}
pub(crate) fn placement(
    value: ipc::Placement,
) -> crate::detached_window_geometry::PhysicalWindowPlacement {
    crate::detached_window_geometry::PhysicalWindowPlacement {
        outer: crate::detached_window_geometry::PhysicalRect {
            left: value.left,
            top: value.top,
            right: value.right,
            bottom: value.bottom,
        },
        maximized: value.maximized,
    }
}
pub(crate) fn startup_request(request: &ipc::Request) -> crate::app::ExplicitOpenPath {
    crate::app::ExplicitOpenPath::new(
        request.path.clone(),
        crate::app::ViewerOpenRequest::new(crate::app::ViewerOpenIntent::BrowserOverlay {
            placement: placement(request.placement),
        }),
    )
}

#[cfg(windows)]
pub(crate) fn forward(request: &ipc::Request) -> std::io::Result<()> {
    let mut pipe = ipc::transport::Stream::open(&process_names().browser_view_pipe)?;
    ipc::deliver(&mut pipe, request, |line| crate::logger::log(line))
}
#[cfg(not(windows))]
pub(crate) fn forward(_: &ipc::Request) -> std::io::Result<()> {
    Err(std::io::Error::new(
        std::io::ErrorKind::Unsupported,
        "browser pipe requires Windows",
    ))
}

#[cfg(windows)]
pub(super) struct Listener {
    stop: std::sync::Arc<ipc::transport::Stop>,
    worker: Option<std::thread::JoinHandle<()>>,
}
#[cfg(windows)]
impl Listener {
    pub(super) fn start(
        ctx: eframe::egui::Context,
        tx: std::sync::mpsc::Sender<ActivationOpenPath>,
        remote: Option<crate::remote_ipc::session::SessionHandle>,
    ) -> std::io::Result<Self> {
        let stop = ipc::transport::Stop::new()?;
        let worker_stop = stop.clone();
        let worker = std::thread::Builder::new()
            .name("miv-browser-view-pipe".into())
            .spawn(move || {
                let run = || -> std::io::Result<()> {
                    let mut security = crate::remote_ipc::current_user_pipe_security()?;
                    let attributes = security.attributes();
                    let mut pipe = ipc::transport::Stream::server(
                        &process_names().browser_view_pipe,
                        worker_stop.clone(),
                        Some(&attributes),
                    )?;
                    while !worker_stop.is_set() {
                        if pipe.connect().is_err() {
                            break;
                        }
                        let result = ipc::serve(&mut pipe, |request| {
                            let received = activation(request, remote.as_ref());
                            if worker_stop.is_set() || tx.send(received).is_err() {
                                return false;
                            }
                            ctx.request_repaint();
                            true
                        });
                        if let Err(error) = result {
                            if !worker_stop.is_set() {
                                crate::logger::log(format!(
                                    "browser_view: connection failed: {error}"
                                ));
                            }
                        }
                        pipe.disconnect();
                    }
                    Ok(())
                };
                if let Err(error) = run() {
                    crate::logger::log(format!("browser_view: listener failed: {error}"));
                }
            })?;
        Ok(Self {
            stop,
            worker: Some(worker),
        })
    }
}
#[cfg(windows)]
impl Drop for Listener {
    fn drop(&mut self) {
        self.stop.signal();
        if let Some(worker) = self.worker.take() {
            let _ = worker.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn browser818_ipc_receive_and_initial_startup_keep_same_intent_and_distinct_request_ids() {
        let path = std::path::absolute("日本語.png").unwrap();
        let request = ipc::Request::new(
            ipc::Placement {
                left: -800,
                top: 0,
                right: 0,
                bottom: 600,
                maximized: false,
            },
            path.clone(),
        );
        let before = std::time::Instant::now();
        let received = activation(request.clone(), None);
        let initial = startup_request(&request);
        assert_eq!(received.path, path);
        assert_eq!(received.request.intent, initial.request.intent);
        assert!(received.request.intent.is_browser_overlay());
        assert_ne!(received.request.id(), initial.request.id());
        assert!(received.received_at >= before);
        assert!(!received.remote_owned_at_receive);
        assert_eq!(received.acquisition_sequence_at_receive, None);
    }
    #[test]
    fn browser818_ipc_core_mutex_race_uses_terminal_browser_route() {
        let mut sends = 0;
        assert_eq!(
            ipc::route(false, || {
                sends += 1;
                Ok(())
            })
            .unwrap(),
            ipc::StartupRoute::StartCore
        );
        assert_eq!(
            ipc::route(true, || {
                sends += 1;
                Ok(())
            })
            .unwrap(),
            ipc::StartupRoute::Forwarded
        );
        assert!(
            ipc::route(true, || {
                sends += 1;
                Err(std::io::Error::other("old core/pipe missing"))
            })
            .is_err()
        );
        assert_eq!(sends, 2);
    }
    #[test]
    fn browser818_ipc_activation_captures_remote_ownership_before_channel_delivery() {
        let remote = crate::remote_ipc::session::SessionHandle::new();
        remote.acquire(mimageviewer_ipc::SessionAcquireRequest {
            client_id: "browser818-test".into(),
            peer: mimageviewer_ipc::SessionPeerInfo {
                connection_kind: mimageviewer_ipc::SessionConnectionKind::Direct,
                device_name: None,
            },
        });
        let snapshot = remote.snapshot();
        let request = ipc::Request::new(
            ipc::Placement {
                left: 0,
                top: 0,
                right: 600,
                bottom: 400,
                maximized: false,
            },
            std::path::absolute("remote.png").unwrap(),
        );
        let received = activation(request, Some(&remote));
        assert!(received.remote_owned_at_receive);
        assert_eq!(
            received.acquisition_sequence_at_receive,
            Some(snapshot.acquisition_sequence)
        );
        assert!(received.rejected_for_remote(Some(snapshot.acquisition_sequence)));
    }
}

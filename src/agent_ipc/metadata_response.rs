//! Admission keeps one shared CAS from validation through UI acceptance. Only
//! Pending work can expire; a claimed operation owns its eventual UI reply.
use super::*;
use crate::ui_request_dispatch::UiRequestDispatch;

fn failed() -> AgentError {
    error(ErrorCode::Internal, "変更要求の処理に失敗しました")
}

pub(super) fn respond(
    connection: Arc<Connection>,
    command: Command,
    permit: Permit,
    ctx: ReadContext,
    replies: crossbeam_channel::Sender<Reply>,
    started: Instant,
    id: u64,
    client: Option<ClientRequest>,
    tool: &'static str,
) {
    let dispatch = Arc::new(UiRequestDispatch::pending());
    let worker_dispatch = dispatch.clone();
    let controller = connection.handlers.open.clone();
    let worker_ctx = ctx.clone();
    let client_name = client
        .as_ref()
        .map(|client| client.identity.name.clone())
        .unwrap_or_else(|| "(名前なし)".into());
    let (send, receive) = crossbeam_channel::bounded(1);
    let worker = std::thread::Builder::new()
        .name("agent-metadata-worker".into())
        .spawn(move || {
            let result =
                controller.execute_write(command, &worker_ctx, client_name, worker_dispatch);
            let _ = send.send(result);
        });
    let Ok(worker) = worker else {
        drop(permit);
        let result = Err(failed());
        connection.record(client.as_ref(), tool, started, &result);
        let _ = replies.send(Reply {
            response: Response { id, result },
            policy: ReplyPolicy::Rejected,
        });
        return;
    };
    let mut completed = false;
    let value = loop {
        if ctx.cancel.load(Ordering::Acquire) || ctx.expired() {
            if dispatch.cancel_pending() {
                let error = if ctx.cancel.swap(true, Ordering::AcqRel) {
                    cancelled()
                } else {
                    timeout()
                };
                break Err(error);
            }
            // Claimed belongs to the UI; an inner cancellation also terminates
            // with its own reply. Neither is replaced by this outer deadline.
            completed = true;
            break receive.recv().unwrap_or_else(|_| Err(failed()));
        }
        match receive.recv_timeout(ctx.remaining().min(Duration::from_millis(25))) {
            Ok(value) => {
                completed = true;
                if ctx.expired() && dispatch.cancel_pending() {
                    ctx.cancel.store(true, Ordering::Release);
                    break Err(timeout());
                }
                break value;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(_) => {
                completed = true;
                break Err(failed());
            }
        }
    };
    let (value, policy) = match value {
        Ok(open::UiOpenOutcome::Written(result)) => (result, ReplyPolicy::AcceptedOperation),
        Ok(open::UiOpenOutcome::Refused(error)) | Err(error) => (Err(error), ReplyPolicy::Rejected),
        Ok(open::UiOpenOutcome::Admitted) => (Err(failed()), ReplyPolicy::Rejected),
    };
    connection.record(client.as_ref(), tool, started, &value);
    let reply = Reply {
        response: Response { id, result: value },
        policy,
    };
    if completed {
        let _ = worker.join();
        drop(permit);
        let _ = replies.send(reply);
    } else {
        let _ = replies.send(reply);
        let _ = worker.join();
        drop(permit);
    }
}

//! Collection mutations keep the actor's successful enqueue as their acceptance
//! point. Only preparation can expire; accepted work owns its result and permit.
use super::*;
use crate::collection_store::{
    CollectionId, CollectionSnapshot, CollectionSourcePath, CollectionStoreClient,
    CollectionStoreError, prepare_collection_registrations,
};
use changes::{ChangeCounts, ChangeRecord, ChangeRow};
use std::collections::HashSet;
use std::time::SystemTime;

pub(super) fn is_write(command: &Command) -> bool {
    matches!(
        command,
        Command::CreateCollection { .. }
            | Command::RenameCollection { .. }
            | Command::AddToCollection { .. }
            | Command::RemoveFromCollection { .. }
    )
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Admission {
    Preparing,
    Stopped,
    Accepted,
}
type Gate = Arc<Mutex<Admission>>;

#[cfg(test)]
pub(super) struct WriteHooks {
    pub(super) before_enqueue: Option<Arc<dyn Fn() + Send + Sync>>,
    pub(super) after_enqueue: Option<Arc<dyn Fn() + Send + Sync>>,
}

fn failed() -> AgentError {
    error(ErrorCode::Internal, "コレクションの変更に失敗しました")
}
fn store_error(cause: CollectionStoreError) -> AgentError {
    match cause {
        CollectionStoreError::Starting | CollectionStoreError::Unavailable => {
            error(ErrorCode::NotReady, "コレクションを準備しています…")
        }
        CollectionStoreError::Busy => error(
            ErrorCode::Busy,
            "コレクション処理が混み合っています。もう一度お試しください",
        ),
        CollectionStoreError::Conflict { .. } => error(
            ErrorCode::Conflict,
            "コレクションが同時に変更されました。もう一度お試しください",
        ),
        CollectionStoreError::NotFound => {
            error(ErrorCode::BadRequest, "コレクションが見つかりません")
        }
        CollectionStoreError::InvalidName => {
            error(ErrorCode::BadRequest, "コレクション名を入力してください")
        }
        _ => failed(),
    }
}
fn checkpoint(ctx: &ReadContext) -> Result<(), AgentError> {
    ctx.checkpoint()?;
    if ctx.expired() {
        return Err(timeout());
    }
    Ok(())
}

fn admission(handlers: &Handlers, ctx: &ReadContext) -> Result<(), AgentError> {
    checkpoint(ctx)?;
    if permission()?.0.level < AgentLevel::Organize {
        return Err(error(
            ErrorCode::Disabled,
            "コレクションを変更する許可がありません",
        ));
    }
    if !handlers.changes.is_ready() {
        return Err(error(ErrorCode::NotReady, "mIV の起動処理中です"));
    }
    if handlers.session.as_ref().is_some_and(|session| {
        session.snapshot().phase != crate::remote_ipc::session::RemoteControlPhase::Local
    }) {
        return Err(error(
            ErrorCode::Busy,
            "リモート操作中はコレクションを変更できません",
        ));
    }
    Ok(())
}

fn enqueue<T>(
    handlers: &Handlers,
    ctx: &ReadContext,
    gate: &Gate,
    send: impl FnOnce() -> Result<
        crossbeam_channel::Receiver<Result<T, CollectionStoreError>>,
        CollectionStoreError,
    >,
) -> Result<T, AgentError> {
    #[cfg(test)]
    if let Some(hook) = handlers
        .write_hooks
        .as_ref()
        .and_then(|hooks| hooks.before_enqueue.as_ref())
    {
        hook();
    }
    admission(handlers, ctx)?;
    let receiver = {
        // The deadline owner and successful actor enqueue share this boundary.
        // This lock never encloses actor/DB waits or test hooks.
        let mut state = gate.lock().unwrap();
        if *state != Admission::Preparing {
            return Err(cancelled());
        }
        checkpoint(ctx)?;
        let receiver = send().map_err(store_error)?;
        *state = Admission::Accepted;
        receiver
    };
    #[cfg(test)]
    if let Some(hook) = handlers
        .write_hooks
        .as_ref()
        .and_then(|hooks| hooks.after_enqueue.as_ref())
    {
        hook();
    }
    // Once accepted the actor executes even if its receiver is dropped. Always
    // harvest, record and return its result, including after cancellation/EOF.
    receiver.recv().map_err(|_| failed())?.map_err(store_error)
}

fn snapshot(
    client: &CollectionStoreClient,
    id: CollectionId,
    ctx: &ReadContext,
) -> Result<CollectionSnapshot, AgentError> {
    checkpoint(ctx)?;
    let value = client
        .load_collection(id)
        .map_err(store_error)?
        .recv()
        .map_err(|_| failed())?
        .map_err(store_error)?;
    checkpoint(ctx)?;
    Ok(value)
}

pub(super) fn respond(
    connection: Arc<Connection>,
    command: Command,
    permit: Permit,
    ctx: ReadContext,
    replies: crossbeam_channel::Sender<Reply>,
    started: Instant,
    id: u64,
    tool: &'static str,
    client: Option<ClientRequest>,
) {
    let gate = Arc::new(Mutex::new(Admission::Preparing));
    let worker_gate = gate.clone();
    let worker_ctx = ctx.clone();
    let handlers = connection.handlers.clone();
    let identity = client
        .as_ref()
        .map(|client| client.identity.name.clone())
        .unwrap_or_else(|| "(名前なし)".into());
    let (send, receive) = crossbeam_channel::bounded(1);
    let worker = std::thread::Builder::new()
        .name("agent-collection-worker".into())
        .spawn(move || {
            let mut record = ChangeRecord {
                id: Default::default(),
                at: SystemTime::now(),
                client_name: identity,
                tool: tool.into(),
                counts: ChangeCounts::default(),
                summary: String::new(),
                rows: vec![],
                notice: String::new(),
            };
            let result = execute(&handlers, command, &worker_ctx, &worker_gate, &mut record)
                .map(Payload::CollectionWrite);
            if *worker_gate.lock().unwrap() == Admission::Accepted {
                record.at = SystemTime::now();
                if let Err(cause) = &result {
                    record.counts = ChangeCounts {
                        failed: record.rows.len().max(1),
                        ..Default::default()
                    };
                    for row in &mut record.rows {
                        row.after = row.before.clone();
                        row.result = cause.message.clone();
                    }
                    record.summary = cause.message.clone();
                    record.notice = "AI によるコレクションの変更に失敗しました".into();
                }
                handlers.changes.publish(record);
            }
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
    let result = loop {
        if ctx.cancel.load(Ordering::Acquire) || ctx.expired() {
            let mut state = gate.lock().unwrap();
            if *state == Admission::Preparing {
                *state = Admission::Stopped;
                let already_cancelled = ctx.cancel.swap(true, Ordering::AcqRel);
                break Err(if already_cancelled {
                    cancelled()
                } else {
                    timeout()
                });
            }
            drop(state);
            completed = true;
            break receive.recv().unwrap_or_else(|_| Err(failed()));
        }
        match receive.recv_timeout(ctx.remaining().min(Duration::from_millis(25))) {
            Ok(value) => {
                completed = true;
                break value;
            }
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
            Err(_) => {
                completed = true;
                break Err(failed());
            }
        }
    };
    let accepted = *gate.lock().unwrap() == Admission::Accepted;
    connection.record(client.as_ref(), tool, started, &result);
    let reply = Reply {
        response: Response { id, result },
        policy: if accepted {
            ReplyPolicy::AcceptedOperation
        } else {
            ReplyPolicy::Rejected
        },
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

fn execute(
    handlers: &Handlers,
    command: Command,
    ctx: &ReadContext,
    gate: &Gate,
    record: &mut ChangeRecord,
) -> Result<CollectionWriteResult, AgentError> {
    admission(handlers, ctx)?;
    let _lease = handlers
        .producer
        .as_ref()
        .and_then(|producer| producer.begin_request())
        .ok_or_else(|| error(ErrorCode::NotReady, "コレクションを準備しています…"))?;
    let client = handlers
        .readers
        .collection_store()
        .ok_or_else(|| error(ErrorCode::NotReady, "コレクションを準備しています…"))?;
    match command {
        Command::CreateCollection { name } => {
            let name = name_value(&name)?;
            record.rows.push(ChangeRow {
                item: "コレクション".into(),
                before: "なし".into(),
                after: name.clone(),
                result: "作成".into(),
            });
            let created = enqueue(handlers, ctx, gate, || client.create_collection(name))?;
            record.rows[0].item = created.definition.id.to_string();
            record.counts.changed = 1;
            record.summary = format!("コレクション「{}」を作成", created.definition.name);
            record.notice = "AI がコレクションを作成しました (1 件)".into();
            Ok(CollectionWriteResult::Created {
                id: created.definition.id.to_string(),
            })
        }
        Command::RenameCollection { id, name } => {
            let id = collection_id(&id)?;
            let name = name_value(&name)?;
            let before = snapshot(&client, id, ctx)?;
            record.rows.push(ChangeRow {
                item: id.to_string(),
                before: before.definition.name.clone(),
                after: name.clone(),
                result: "名前変更".into(),
            });
            let after = enqueue(handlers, ctx, gate, || {
                client.rename_collection(id, before.revision(), name)
            })?;
            let changed = before.definition.name != after.definition.name;
            record.counts.changed = usize::from(changed);
            record.counts.unchanged = usize::from(!changed);
            record.rows[0].result = if changed {
                "名前変更"
            } else {
                "変更なし"
            }
            .into();
            record.summary = "コレクション名の変更".into();
            record.notice = format!(
                "AI がコレクションを変更しました ({} 件)",
                usize::from(changed)
            );
            Ok(CollectionWriteResult::Renamed {
                id: id.to_string(),
                changed,
            })
        }
        Command::AddToCollection { id, items } => {
            let id = collection_id(&id)?;
            validate_items(&items)?;
            let before = snapshot(&client, id, ctx)?;
            let mut statuses =
                vec![ItemResult::Skipped(CollectionSkipReason::UnsupportedType); items.len()];
            let mut paths = vec![];
            let mut positions = vec![];
            for (index, item) in items.iter().enumerate() {
                checkpoint(ctx)?;
                if let Some(reason) = invalid_item(item) {
                    statuses[index] = ItemResult::Skipped(reason);
                    continue;
                }
                paths.push(std::path::PathBuf::from(&item.path));
                positions.push(index);
            }
            let prepared = prepare_collection_registrations(&paths, &ctx.cancel, |_, _| {});
            checkpoint(ctx)?;
            let mut registrations = vec![];
            let mut keys = vec![];
            for (position, prepared) in positions.into_iter().zip(prepared) {
                match prepared.result {
                    Ok(registration)
                        if registration.resolved_kind()
                            == crate::collection_store::CollectionResolvedKind::Unresolved =>
                    {
                        statuses[position] = ItemResult::Skipped(CollectionSkipReason::NotFound);
                    }
                    Ok(registration) => {
                        statuses[position] = if before
                            .entries
                            .iter()
                            .any(|entry| entry.source_key == *registration.source_key())
                        {
                            ItemResult::Duplicate
                        } else {
                            ItemResult::NotPresent
                        };
                        keys.push((position, registration.source_key().clone()));
                        registrations.push(registration);
                    }
                    Err(crate::collection_store::CollectionPrepareError::Unsupported(_)) => {
                        statuses[position] =
                            ItemResult::Skipped(CollectionSkipReason::UnsupportedType)
                    }
                    Err(_) => return Err(failed()),
                }
            }
            record.rows = item_rows(&items, &statuses);
            let outcome = enqueue(handlers, ctx, gate, || {
                client.add_batch(id, before.revision(), registrations)
            })?;
            let added: HashSet<_> = outcome
                .snapshot
                .entries
                .iter()
                .filter(|entry| outcome.added.contains(&entry.id))
                .map(|entry| entry.source_key.clone())
                .collect();
            let mut seen = HashSet::new();
            for (position, key) in keys {
                statuses[position] = if !seen.insert(key.clone()) {
                    ItemResult::Duplicate
                } else if added.contains(&key) {
                    ItemResult::Added
                } else if outcome.capacity_rejected.contains(&key) {
                    ItemResult::CapacityRejected
                } else if outcome.duplicates.contains(&key) {
                    ItemResult::Duplicate
                } else {
                    return Err(failed());
                };
            }
            finish_items(record, &items, &statuses, &before.definition.name);
            Ok(CollectionWriteResult::Items {
                id: id.to_string(),
                outcomes: grouped(&statuses),
            })
        }
        Command::RemoveFromCollection { id, items } => {
            let id = collection_id(&id)?;
            validate_items(&items)?;
            let before = snapshot(&client, id, ctx)?;
            let mut statuses = vec![];
            let mut ids = vec![];
            let mut seen = HashSet::new();
            for item in &items {
                checkpoint(ctx)?;
                let status = if let Some(reason) = invalid_item(item) {
                    ItemResult::Skipped(reason)
                } else {
                    let source =
                        CollectionSourcePath::from_trusted(std::path::Path::new(&item.path))
                            .map_err(store_error)?;
                    match before
                        .entries
                        .iter()
                        .find(|entry| entry.source_key == *source.key())
                    {
                        Some(entry) if seen.insert(entry.id) => {
                            ids.push(entry.id);
                            ItemResult::Removed
                        }
                        _ => ItemResult::NotPresent,
                    }
                };
                statuses.push(status);
            }
            record.rows = item_rows(&items, &statuses);
            enqueue(handlers, ctx, gate, || {
                client.remove_entries(id, before.revision(), ids)
            })?;
            finish_items(record, &items, &statuses, &before.definition.name);
            Ok(CollectionWriteResult::Items {
                id: id.to_string(),
                outcomes: grouped(&statuses),
            })
        }
        _ => Err(error(
            ErrorCode::BadRequest,
            "コレクション操作ではありません",
        )),
    }
}

fn name_value(name: &str) -> Result<String, AgentError> {
    let name = name.trim();
    if name.is_empty() || name.contains('\0') {
        Err(error(
            ErrorCode::BadRequest,
            "コレクション名を入力してください",
        ))
    } else {
        Ok(name.into())
    }
}
fn collection_id(id: &str) -> Result<CollectionId, AgentError> {
    uuid::Uuid::parse_str(id)
        .map(CollectionId::from_uuid)
        .map_err(|_| {
            error(
                ErrorCode::BadRequest,
                "コレクションの id を確認してください",
            )
        })
}
fn validate_items(items: &[ItemRef]) -> Result<(), AgentError> {
    if items.is_empty() || items.len() > MAX_WRITE_ITEMS {
        Err(error(
            ErrorCode::BadRequest,
            "項目は1～500件で指定してください",
        ))
    } else {
        Ok(())
    }
}
fn invalid_item(item: &ItemRef) -> Option<CollectionSkipReason> {
    if input_path::validate(&item.path).is_err() {
        Some(CollectionSkipReason::InvalidPath)
    } else if item.subresource.is_some() {
        Some(CollectionSkipReason::UnsupportedSubresource)
    } else {
        None
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ItemResult {
    Added,
    Duplicate,
    Removed,
    NotPresent,
    CapacityRejected,
    Skipped(CollectionSkipReason),
}
fn item_rows(items: &[ItemRef], results: &[ItemResult]) -> Vec<ChangeRow> {
    items
        .iter()
        .zip(results)
        .map(|(item, result)| {
            let (before, after, outcome) = match result {
                ItemResult::Added => ("未登録", "登録済み", "追加"),
                ItemResult::Duplicate => ("登録済み", "登録済み", "変更なし"),
                ItemResult::Removed => ("登録済み", "未登録", "削除"),
                ItemResult::NotPresent => ("未登録", "未登録", "変更なし"),
                ItemResult::CapacityRejected => ("未登録", "未登録", "上限で追加できません"),
                ItemResult::Skipped(CollectionSkipReason::InvalidPath) => {
                    ("—", "—", "パスの表記が不正です")
                }
                ItemResult::Skipped(CollectionSkipReason::NotFound) => {
                    ("—", "—", "項目が見つかりません")
                }
                ItemResult::Skipped(CollectionSkipReason::UnsupportedSubresource) => {
                    ("—", "—", "本の中の項目は対象外です")
                }
                ItemResult::Skipped(CollectionSkipReason::UnsupportedType) => {
                    ("—", "—", "対象外の形式です")
                }
            };
            ChangeRow {
                item: item.subresource.as_ref().map_or_else(
                    || item.path.clone(),
                    |entry| format!("{}::{entry}", item.path),
                ),
                before: before.into(),
                after: after.into(),
                result: outcome.into(),
            }
        })
        .collect()
}
fn finish_items(record: &mut ChangeRecord, items: &[ItemRef], results: &[ItemResult], name: &str) {
    record.rows = item_rows(items, results);
    record.counts.changed = results
        .iter()
        .filter(|result| matches!(result, ItemResult::Added | ItemResult::Removed))
        .count();
    record.counts.unchanged = results
        .iter()
        .filter(|result| matches!(result, ItemResult::Duplicate | ItemResult::NotPresent))
        .count();
    record.counts.skipped = results.len() - record.counts.changed - record.counts.unchanged;
    record.summary = format!("コレクション「{name}」の項目を変更");
    record.notice = format!(
        "AI がコレクションを変更しました ({} 件)",
        record.counts.changed
    );
}
fn grouped(results: &[ItemResult]) -> Vec<CollectionItemOutcome> {
    let mut groups = vec![];
    for status in [
        ItemResult::Added,
        ItemResult::Duplicate,
        ItemResult::Removed,
        ItemResult::NotPresent,
        ItemResult::CapacityRejected,
        ItemResult::Skipped(CollectionSkipReason::InvalidPath),
        ItemResult::Skipped(CollectionSkipReason::NotFound),
        ItemResult::Skipped(CollectionSkipReason::UnsupportedSubresource),
        ItemResult::Skipped(CollectionSkipReason::UnsupportedType),
    ] {
        let indices: Vec<u16> = results
            .iter()
            .enumerate()
            .filter(|(_, result)| **result == status)
            .map(|(index, _)| index as u16)
            .collect();
        if indices.is_empty() {
            continue;
        }
        groups.push(match status {
            ItemResult::Added => CollectionItemOutcome::Added { indices },
            ItemResult::Duplicate => CollectionItemOutcome::Duplicate { indices },
            ItemResult::Removed => CollectionItemOutcome::Removed { indices },
            ItemResult::NotPresent => CollectionItemOutcome::NotPresent { indices },
            ItemResult::CapacityRejected => CollectionItemOutcome::CapacityRejected { indices },
            ItemResult::Skipped(reason) => CollectionItemOutcome::Skipped { indices, reason },
        });
    }
    groups
}

#[cfg(test)]
mod logging_tests {
    use super::*;
    #[test]
    fn agent_write_log_never_serializes_names_paths_or_error_details() {
        let secret = "C:/private/secret collection";
        for result in [
            Ok(Payload::CollectionWrite(CollectionWriteResult::Created {
                id: secret.into(),
            })),
            Err(error(ErrorCode::Internal, secret)),
        ] {
            let command = Command::CreateCollection {
                name: secret.into(),
            };
            let line = request_log_line(tool_name(&command), Instant::now(), &result);
            assert!(line.contains("tool=create_collection"));
            assert!(!line.contains(secret));
            assert!(!line.contains("private"));
        }
    }
}

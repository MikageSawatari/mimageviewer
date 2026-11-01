//! UI admission and accepted metadata operations. Target I/O belongs to the
//! agent worker; this boundary never enters the user's edit history.
use super::*;
use crate::agent_ipc::changes::{ChangeCounts, ChangeRecord, ChangeRow};
use crate::agent_ipc::metadata_write::PreparedItem;
use crate::agent_ipc::open::{UiOpenOutcome, UiRequestKind};
use mimageviewer_agent_ipc::{
    AgentError, AgentLevel, ErrorCode, MetadataItemOutcome, MetadataSkipReason,
    MetadataWriteResult, Payload,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum WriteRefusal {
    Startup,
    TagMaintenance,
    BookOperation,
    AppendUndo,
    MetadataTransfer,
    RenameMigration,
    SidecarRestore,
    Composition,
    DocumentModal,
}

impl WriteRefusal {
    fn message(self) -> &'static str {
        match self {
            Self::Startup => "mIV の起動処理中です",
            Self::TagMaintenance => "タグの名前を変更・統合しています",
            Self::BookOperation => "本を変更しています",
            Self::AppendUndo => "本への追加を取り消しています",
            Self::MetadataTransfer => "メタ情報を処理しています",
            Self::RenameMigration => "名前変更後の情報を移行しています",
            Self::SidecarRestore => "編集情報を復元しています",
            Self::Composition => "本の構成を編集中または保存中です",
            Self::DocumentModal => "文書を開く操作を処理しています",
        }
    }
}

impl App {
    fn agent_metadata_precheck(&self, tags: bool) -> Option<WriteRefusal> {
        if !self.initialized || self.indexer_init.overlay_active() {
            Some(WriteRefusal::Startup)
        } else if tags && self.tag_maintenance_in_progress() {
            Some(WriteRefusal::TagMaintenance)
        } else if self.book_structural_op.is_some() {
            Some(WriteRefusal::BookOperation)
        } else if self.book_append_undo_in_progress() {
            Some(WriteRefusal::AppendUndo)
        } else if self.metadata_transfer.is_some() || self.metadata_cleanup_pending.is_some() {
            Some(WriteRefusal::MetadataTransfer)
        } else if self.rename_migration_in_flight.is_some()
            || !self.rename_migration_queue.is_empty()
            || !self.rename_migration_boot_retry.is_empty()
        {
            Some(WriteRefusal::RenameMigration)
        } else if self.sidecar_restore_active() {
            Some(WriteRefusal::SidecarRestore)
        } else if self.composition_editor_modal_active() || self.composition_editor_writes_pending()
        {
            Some(WriteRefusal::Composition)
        } else if self.document_open_modal_admission_blocked() {
            Some(WriteRefusal::DocumentModal)
        } else {
            None
        }
    }

    pub(super) fn admit_agent_metadata_write(&mut self, prepared: &UiRequestKind) -> UiOpenOutcome {
        let refuse = |code, message| UiOpenOutcome::Refused(AgentError::new(code, message));
        if self.settings.agent_level < AgentLevel::Organize {
            return refuse(ErrorCode::Disabled, "タグ・★を変更する許可がありません");
        }
        if let Some(reason) =
            self.agent_metadata_precheck(matches!(prepared, UiRequestKind::TagWrite(_)))
        {
            return refuse(
                if reason == WriteRefusal::Startup {
                    ErrorCode::NotReady
                } else {
                    ErrorCode::Busy
                },
                reason.message(),
            );
        }
        let Some(changes) = self.agent_changes.as_ref().map(|ui| ui.controller.clone()) else {
            return refuse(ErrorCode::NotReady, "mIV の起動処理中です");
        };
        // Linearize acceptance with Remote acquisition. The session lock must
        // not enclose database writes, publication or callbacks into App.
        if self
            .remote_session_handle()
            .is_some_and(|handle| handle.with_local_composition_admission(|| ()).is_err())
        {
            return refuse(ErrorCode::Busy, "リモート操作中はタグ・★を変更できません");
        }
        let result = match prepared {
            UiRequestKind::TagWrite(write) => {
                let mut states = initial_states(&write.items);
                let targets = write
                    .items
                    .iter()
                    .enumerate()
                    .filter_map(|(index, item)| {
                        item.target.as_ref().ok().map(|target| {
                            states[index] = ItemState::Queued;
                            (index, target.clone())
                        })
                    })
                    .collect();
                let rows = write
                    .items
                    .iter()
                    .zip(&states)
                    .map(|(item, state)| ChangeRow {
                        item: item_label(&item.input),
                        before: if *state == ItemState::Queued {
                            "処理中"
                        } else {
                            "—"
                        }
                        .into(),
                        after: if *state == ItemState::Queued {
                            "処理中"
                        } else {
                            "—"
                        }
                        .into(),
                        result: state.message().into(),
                    })
                    .collect();
                self.request_agent_tag_set_for_targets(
                    targets,
                    &write.tags,
                    write.add,
                    write.client_name.clone(),
                    rows,
                    changes,
                );
                Payload::TagWrite(MetadataWriteResult {
                    outcomes: grouped(&states),
                })
            }
            UiRequestKind::RatingWrite(write) => {
                use crate::app::agent_rating_write::RatingTargetStatus;
                let mut states = initial_states(&write.items);
                let targets: Vec<_> = write
                    .items
                    .iter()
                    .filter_map(|item| item.target.as_ref().ok().cloned())
                    .collect();
                let results = self.apply_agent_rating_targets(&targets, write.stars);
                let mut results = results.into_iter();
                let mut counts = ChangeCounts::default();
                let rows = write
                    .items
                    .iter()
                    .enumerate()
                    .map(|(index, item)| {
                        let (before, after, message) = if item.target.is_ok() {
                            let result = results
                                .next()
                                .expect("one rating result per prepared target");
                            let state = match result.status {
                                RatingTargetStatus::Changed => {
                                    counts.changed += 1;
                                    ItemState::Changed
                                }
                                RatingTargetStatus::Unchanged => {
                                    counts.unchanged += 1;
                                    ItemState::Unchanged
                                }
                                RatingTargetStatus::Skipped => {
                                    counts.failed += 1;
                                    ItemState::Skipped(MetadataSkipReason::Unavailable)
                                }
                            };
                            states[index] = state;
                            (
                                result
                                    .before
                                    .map(|stars| stars.to_string())
                                    .unwrap_or_else(|| "—".into()),
                                result
                                    .after
                                    .map(|stars| stars.to_string())
                                    .unwrap_or_else(|| "—".into()),
                                if result.error.is_some() {
                                    "保存に失敗しました"
                                } else {
                                    state.message()
                                },
                            )
                        } else {
                            counts.skipped += 1;
                            ("—".into(), "—".into(), states[index].message())
                        };
                        ChangeRow {
                            item: item_label(&item.input),
                            before,
                            after,
                            result: message.into(),
                        }
                    })
                    .collect();
                let changed = counts.changed;
                changes.publish(ChangeRecord {
                    id: Default::default(),
                    at: std::time::SystemTime::now(),
                    client_name: write.client_name.clone(),
                    tool: "set_rating".into(),
                    collection: None,
                    counts,
                    summary: format!("★を変更 ({changed} 件)"),
                    rows,
                    notice: format!("AI が★を変更しました ({changed} 件)"),
                });
                Payload::RatingWrite(MetadataWriteResult {
                    outcomes: grouped(&states),
                })
            }
            UiRequestKind::Open(_) => unreachable!("open uses the existing activation resolver"),
        };
        UiOpenOutcome::Written(Ok(result))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ItemState {
    Queued,
    Changed,
    Unchanged,
    Skipped(MetadataSkipReason),
}
impl ItemState {
    fn message(self) -> &'static str {
        match self {
            Self::Queued => "処理中",
            Self::Changed => "変更",
            Self::Unchanged => "変更なし",
            Self::Skipped(MetadataSkipReason::InvalidPath) => "パスの表記が不正です",
            Self::Skipped(MetadataSkipReason::NotFound) => "項目が見つかりません",
            Self::Skipped(MetadataSkipReason::UnsupportedType) => "対象外の形式です",
            Self::Skipped(MetadataSkipReason::UnsupportedSubresource) => "本の中の項目は対象外です",
            Self::Skipped(MetadataSkipReason::Unavailable) => "項目を読み書きできません",
        }
    }
}
fn initial_states<T>(items: &[PreparedItem<T>]) -> Vec<ItemState> {
    items
        .iter()
        .map(|item| match &item.target {
            Ok(_) => ItemState::Unchanged,
            Err(reason) => ItemState::Skipped(*reason),
        })
        .collect()
}
fn item_label(item: &mimageviewer_agent_ipc::ItemRef) -> String {
    item.subresource.as_ref().map_or_else(
        || item.path.clone(),
        |entry| format!("{}::{entry}", item.path),
    )
}
fn grouped(states: &[ItemState]) -> Vec<MetadataItemOutcome> {
    let mut groups: Vec<(ItemState, Vec<u16>)> = vec![];
    for (index, state) in states.iter().enumerate() {
        if let Some((_, indices)) = groups.iter_mut().find(|(value, _)| value == state) {
            indices.push(index as u16);
        } else {
            groups.push((*state, vec![index as u16]));
        }
    }
    groups
        .into_iter()
        .map(|(state, indices)| match state {
            ItemState::Queued => MetadataItemOutcome::Queued { indices },
            ItemState::Changed => MetadataItemOutcome::Changed { indices },
            ItemState::Unchanged => MetadataItemOutcome::Unchanged { indices },
            ItemState::Skipped(reason) => MetadataItemOutcome::Skipped { indices, reason },
        })
        .collect()
}

#[cfg(test)]
#[path = "agent_metadata_write_tests.rs"]
mod tests;

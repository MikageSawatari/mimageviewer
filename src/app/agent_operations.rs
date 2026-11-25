//! Operations use the same admission and commit owner as the customization dialog.
use super::*;
use crate::agent_ipc::changes::{
    ChangeCounts, ChangeRecord, ChangeRecordId, ChangeRow, ChangeRowOutcome, ChangeRowResult,
};
use crate::operation_customize_agent as operations;
use crate::operation_customize_share::OperationCustomizeBundle;
use mimageviewer_agent_ipc::{
    AgentError, ErrorCode, MAX_ITEM_CHARS, MAX_PAGE_CHARS, OperationEdits, OperationsPage,
    OperationsQuery, OperationsWriteResult, mcp_content_chars,
};

fn check_budget(result: &OperationsWriteResult) -> Result<(), AgentError> {
    if mcp_content_chars(result).map_or(true, |n| n > MAX_PAGE_CHARS)
        || result
            .changed
            .iter()
            .chain(&result.unchanged)
            .any(|row| mcp_content_chars(row).map_or(true, |n| n > MAX_ITEM_CHARS))
        || result
            .conflicts
            .iter()
            .any(|row| mcp_content_chars(row).map_or(true, |n| n > MAX_ITEM_CHARS))
    {
        return Err(AgentError::new(
            ErrorCode::BadRequest,
            "値が大きすぎるので、項目を分けて変えてください",
        ));
    }
    Ok(())
}

impl App {
    pub(super) fn agent_get_operations(
        &self,
        args: &OperationsQuery,
    ) -> Result<OperationsPage, AgentError> {
        self.agent_settings_read_permission()?;
        operations::get_operations(
            &OperationCustomizeBundle::from_settings(&self.settings),
            args,
        )
    }

    pub(crate) fn agent_set_operations(
        &mut self,
        args: &OperationEdits,
        client_name: &str,
    ) -> Result<OperationsWriteResult, AgentError> {
        self.admit_agent_settings_write()?;
        let before = OperationCustomizeBundle::from_settings(&self.settings);
        let (candidate, preview) = operations::prepare_edits(&before, args)?;
        check_budget(&preview)?;
        if preview.changed.is_empty() {
            return Ok(preview);
        }
        let controller = self
            .agent_changes
            .as_ref()
            .map(|ui| ui.controller.clone())
            .ok_or_else(|| AgentError::new(ErrorCode::NotReady, "mIV の起動処理中です"))?;
        let started = std::time::Instant::now();
        let generation = crate::settings::save_generation();
        self.apply_operation_customize_bundle(candidate);
        if generation == crate::settings::save_generation() {
            crate::logger::log("agent_ipc: set_operations: operation settings could not be saved");
            self.show_feedback_warning("AI による設定変更を保存できませんでした".into());
        }
        crate::perf::event(
            "agent",
            "set_operations_commit",
            None,
            0,
            &[(
                "duration_ms",
                serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
            )],
        );
        let after = OperationCustomizeBundle::from_settings(&self.settings);
        let result = operations::readback(&before, &after, &preview);
        debug_assert!(
            check_budget(&result).is_ok(),
            "validated bundle must match the committed response"
        );
        let rows: Vec<_> = result
            .changed
            .iter()
            .map(|change| ChangeRow {
                item: change.label.clone(),
                before: operations::change_text(&change.name, &change.before),
                after: operations::change_text(&change.name, &change.after),
                result: ChangeRowResult::Done(ChangeRowOutcome::Changed),
            })
            .collect();
        for change in &result.changed {
            crate::logger::log(format!(
                "agent_ipc: tool=set_operations name={} before={} after={}",
                change.name, change.before, change.after
            ));
        }
        let notice = format!(
            "AI アプリが操作の割り当てを変更しました: {}",
            rows.iter()
                .map(|row| format!("{}: {} → {}", row.item, row.before, row.after))
                .collect::<Vec<_>>()
                .join("\n")
        );
        controller.publish(ChangeRecord {
            id: ChangeRecordId::default(),
            at: std::time::SystemTime::now(),
            client_name: client_name.into(),
            tool: "set_operations".into(),
            collection: None,
            counts: ChangeCounts {
                changed: result.changed.len(),
                unchanged: result.unchanged.len(),
                ..Default::default()
            },
            summary: "操作の割り当てを変更".into(),
            rows,
            notice,
        });
        Ok(result)
    }
}

#[cfg(test)]
#[path = "agent_operations_tests.rs"]
mod tests;

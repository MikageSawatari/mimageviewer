//! Main-context, in-memory settings reads and the existing Preferences OK path.
use super::*;
use crate::agent_ipc::changes::{
    ChangeCounts, ChangeRecord, ChangeRecordId, ChangeRow, ChangeRowOutcome, ChangeRowResult,
};
use crate::settings::Settings;
use mimageviewer_agent_ipc::{
    AgentError, AgentLevel, ErrorCode, MAX_ITEM_CHARS, MAX_PAGE_CHARS, MAX_SETTINGS_KEYS, PageRow,
    SETTINGS_PAGE_SIZE, SettingChange, SettingChangeRow, SettingChoice, SettingRow, SettingsPage,
    SettingsWriteResult, mcp_content_chars,
};
use serde_json::{Map, Value};

const DIALOG_BUSY: &str =
    "mIV でダイアログを開いている間は設定を変更できません。閉じてからやり直してください";
const RESPONSE_TOO_LARGE: &str = "値が大きすぎるので、項目を分けて変えてください";

fn bad(message: impl Into<String>) -> AgentError {
    AgentError::new(ErrorCode::BadRequest, message)
}

impl App {
    fn agent_settings_permission(&self) -> Result<(), AgentError> {
        if self.settings.agent_level < AgentLevel::ReadOnly || !self.settings.agent_allow_settings {
            return Err(AgentError::new(
                ErrorCode::Disabled,
                crate::agent_ipc::SETTINGS_PERMISSION_MESSAGE,
            ));
        }
        if !self.initialized || self.indexer_init.overlay_active() {
            return Err(AgentError::new(ErrorCode::NotReady, "mIV の起動処理中です"));
        }
        Ok(())
    }

    fn agent_settings_overlay(&self) -> (bool, bool) {
        let active = self.settings.favorite_view_overlay.is_some();
        let scale = active
            && self
                .favorite_view_state_for_capture()
                .fullscreen_specified_scale
                .is_some();
        (active, scale)
    }

    pub(super) fn agent_get_settings(
        &self,
        query: Option<&str>,
        keys: Option<&[String]>,
        page: Option<u32>,
    ) -> Result<SettingsPage, AgentError> {
        self.agent_settings_permission()?;
        if self
            .remote_session_handle()
            .is_some_and(|session| session.with_local_composition_admission(|| ()).is_err())
        {
            return Err(AgentError::new(
                ErrorCode::Busy,
                "リモート操作中は設定を返しません",
            ));
        }
        if query.is_some_and(|query| query.chars().take(201).count() > 200) {
            return Err(bad("query は 200 文字以内で指定してください"));
        }
        if keys.is_some_and(|keys| keys.is_empty() || keys.len() > MAX_SETTINGS_KEYS) {
            return Err(bad("設定は 1～32 項目で指定してください"));
        }
        let page = page.unwrap_or(1);
        if page == 0 {
            return Err(bad("page は 1 以上で指定してください"));
        }
        let (active, scale) = self.agent_settings_overlay();
        let snapshot = self.settings.preferences_snapshot();
        let all = crate::settings_transfer::capture_stage1(&snapshot, active, scale);
        // Explicit keys must all be admitted. Do not silently return a partial read.
        if let Some(keys) = keys {
            for key in keys {
                if !all.iter().any(|row| &row.key == key) {
                    let mut probe = Map::new();
                    probe.insert(key.clone(), Value::Null);
                    return Err(bad(crate::settings_transfer::stage1_validate(
                        &snapshot, &probe,
                    )
                    .err()
                    .unwrap_or_else(|| format!("{key}: この設定はまだ変えられません"))));
                }
            }
        }
        let terms: Vec<_> = query
            .unwrap_or("")
            .split_whitespace()
            .map(str::to_ascii_lowercase)
            .collect();
        let details = keys.is_some() || query.is_some();
        let mut selected: Vec<_> = all
            .into_iter()
            .filter(|row| {
                keys.is_none_or(|keys| keys.contains(&row.key))
                    && terms.iter().all(|term| {
                        row.key.to_ascii_lowercase().contains(term)
                            || row.label.to_ascii_lowercase().contains(term)
                    })
            })
            .collect();
        let (pages, more) = if details {
            if page != 1 {
                return Err(bad("keys/query では page は 1 です"));
            }
            let more = selected.len() > MAX_SETTINGS_KEYS;
            selected.truncate(MAX_SETTINGS_KEYS);
            (1, more)
        } else {
            let pages = selected.len().div_ceil(SETTINGS_PAGE_SIZE).max(1) as u32;
            if page > pages {
                return Err(bad("指定したページはありません"));
            }
            selected = selected
                .into_iter()
                .skip((page as usize - 1) * SETTINGS_PAGE_SIZE)
                .take(SETTINGS_PAGE_SIZE)
                .collect();
            (pages, false)
        };
        let mut response = SettingsPage {
            items: Vec::new(),
            page,
            pages,
            more,
        };
        for row in selected {
            let row = SettingRow {
                key: row.key,
                label: row.label,
                value: row.value,
                overridden_by_favorite: row.overridden_by_favorite,
                choices: details.then(|| {
                    row.choices.map(|choices| {
                        choices
                            .into_iter()
                            .map(|choice| SettingChoice {
                                value: choice.value,
                                label: choice.label,
                            })
                            .collect()
                    })
                }),
                template: if details { row.template } else { None },
            };
            let omitted =
                || PageRow::too_long(&row.key).map_err(|_| bad("応答を作成できませんでした"));
            let item = if mcp_content_chars(&row).map_err(|_| bad("応答を作成できませんでした"))?
                > MAX_ITEM_CHARS
            {
                omitted()?
            } else {
                PageRow::Value(row.clone())
            };
            response.items.push(item);
        }
        // Keep page membership stable, including late rows. Replace the largest
        // remaining value until the final structured JSON and text copy fit.
        while mcp_content_chars(&response).map_err(|_| bad("応答を作成できませんでした"))?
            > MAX_PAGE_CHARS
        {
            let index = response
                .items
                .iter()
                .enumerate()
                .filter_map(|(index, row)| match row {
                    PageRow::Value(row) => {
                        Some((index, mcp_content_chars(row).unwrap_or(usize::MAX)))
                    }
                    PageRow::TooLong(_) => None,
                })
                .max_by_key(|(_, size)| *size)
                .map(|(index, _)| index)
                .ok_or_else(|| bad(RESPONSE_TOO_LARGE))?;
            if let PageRow::Value(row) = &response.items[index] {
                response.items[index] =
                    PageRow::too_long(&row.key).map_err(|_| bad("応答を作成できませんでした"))?;
            }
        }
        Ok(response)
    }

    pub(super) fn admit_agent_settings_write(&self) -> Result<(), AgentError> {
        self.agent_settings_permission()?;
        if self.modal_dialog_block_reason().is_some() || self.pref_state.is_some() {
            return Err(AgentError::new(ErrorCode::Busy, DIALOG_BUSY));
        }
        // Linearize admission only; commits and notifications must not hold
        // Remote's session lock. The root UI owns the rest of this frame.
        if self
            .remote_session_handle()
            .is_some_and(|session| session.with_local_composition_admission(|| ()).is_err())
        {
            return Err(AgentError::new(
                ErrorCode::Busy,
                "リモート操作中は設定を変更できません",
            ));
        }
        Ok(())
    }

    pub(crate) fn agent_set_settings(
        &mut self,
        changes: &Map<String, Value>,
        client_name: &str,
        ctx: &egui::Context,
    ) -> Result<SettingsWriteResult, AgentError> {
        self.admit_agent_settings_write()?;
        if changes.is_empty() || changes.len() > MAX_SETTINGS_KEYS {
            return Err(bad("設定は 1～32 項目で指定してください"));
        }
        let before = self.settings.preferences_snapshot();
        let candidate = crate::settings_transfer::stage1_validate(&before, changes).map_err(bad)?;
        let keys: Vec<_> = changes.keys().cloned().collect();
        let (active, scale) = self.agent_settings_overlay();
        let requested = settings_result(&before, &candidate, &keys, active, scale);
        if requested.changed.is_empty() {
            check_settings_budget(&requested)?;
            return Ok(requested);
        }
        let changes_controller = self
            .agent_changes
            .as_ref()
            .map(|ui| ui.controller.clone())
            .ok_or_else(|| AgentError::new(ErrorCode::NotReady, "mIV の起動処理中です"))?;
        // Project the same live-value merge without touching live settings or
        // creating a draft. Validate the full response before Preferences OK.
        let mut projected = candidate.clone();
        let mut live = self.settings.clone();
        crate::ui_dialogs::preferences::prepare_preferences_edited_settings_for_commit(
            &mut projected,
            self.settings.ring_shortcuts.video_normal_wheel_action,
            &mut live,
        );
        let projected = projected.preferences_snapshot();
        let preview = settings_result(&before, &projected, &keys, active, scale);
        check_settings_budget(&preview)?;
        check_settings_budget(&settings_budget_bound(
            &preview,
            &settings_result(&before, &before, &keys, active, scale),
        ))?;

        self.install_headless_preferences_state(candidate);
        let started = std::time::Instant::now();
        let saved_generation = crate::settings::save_generation();
        self.apply_preferences_dialog(ctx);
        if crate::settings::save_generation() == saved_generation {
            crate::logger::log("agent_ipc: set_settings: Preferences settings could not be saved");
            self.show_feedback_warning("AI による設定変更を保存できませんでした".into());
        }
        crate::perf::event(
            "agent",
            "set_settings_commit",
            None,
            0,
            &[(
                "duration_ms",
                serde_json::json!(started.elapsed().as_secs_f64() * 1000.0),
            )],
        );
        debug_assert!(
            self.pref_state.is_none(),
            "admitted Preferences commit must consume its draft"
        );
        let after = self.settings.preferences_snapshot();
        let result = settings_result(&before, &after, &keys, active, scale);
        debug_assert!(
            check_settings_budget(&result).is_ok(),
            "pre-commit bound must cover the actual response"
        );
        if !result.changed.is_empty() {
            let rows: Vec<_> = result
                .changed
                .iter()
                .flat_map(|change| {
                    change.rows.iter().map(|row| ChangeRow {
                        item: row.item.clone(),
                        before: row.before.clone(),
                        after: row.after.clone(),
                        result: ChangeRowResult::Done(ChangeRowOutcome::Changed),
                    })
                })
                .collect();
            for change in &result.changed {
                crate::logger::log(format!(
                    "agent_ipc: tool=set_settings key={} before={} after={}",
                    change.key, change.before, change.after
                ));
            }
            let notice = format!(
                "AI アプリが設定を変更しました: {}",
                rows.iter()
                    .map(|row| format!("{}: {} → {}", row.item, row.before, row.after))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
            changes_controller.publish(ChangeRecord {
                id: ChangeRecordId::default(),
                at: std::time::SystemTime::now(),
                client_name: client_name.into(),
                tool: "set_settings".into(),
                collection: None,
                counts: ChangeCounts {
                    changed: result.changed.len(),
                    unchanged: result.unchanged.len(),
                    ..Default::default()
                },
                summary: "設定を変更".into(),
                rows,
                notice,
            });
        }
        Ok(result)
    }
}

fn settings_result(
    before: &Settings,
    after: &Settings,
    keys: &[String],
    active: bool,
    scale: bool,
) -> SettingsWriteResult {
    let mut result = SettingsWriteResult {
        changed: Vec::new(),
        unchanged: Vec::new(),
    };
    for change in crate::settings_transfer::stage1_changes(before, after, keys) {
        let overridden = match change.key.as_str() {
            "grid_display_order" | "default_spread_mode" | "default_reading_flow" => active,
            "fullscreen_specified_scale" => scale,
            _ => false,
        };
        let changed = change.before != change.after;
        let label = change.label;
        let row = SettingChange {
            key: change.key,
            label: label.clone(),
            before: change.before,
            after: change.after,
            overridden_by_favorite: overridden,
            rows: change
                .lines
                .into_iter()
                .map(|(item, before, after)| SettingChangeRow {
                    item: if item.is_empty() { label.clone() } else { item },
                    before,
                    after,
                })
                .collect(),
        };
        if changed {
            result.changed.push(row);
        } else {
            result.unchanged.push(row);
        }
    }
    result
}

fn check_settings_budget(result: &impl serde::Serialize) -> Result<(), AgentError> {
    if mcp_content_chars(result).map_err(|_| bad("応答を作成できませんでした"))? > MAX_PAGE_CHARS
    {
        return Err(bad(RESPONSE_TOO_LARGE));
    }
    Ok(())
}

/// A normal commit can retain the previous value (e.g. RAW executor refusal).
/// Cover each mixture, including escaped text, before any side effects occur.
fn settings_budget_bound(
    preview: &SettingsWriteResult,
    unchanged: &SettingsWriteResult,
) -> SettingsWriteResult {
    let mut rows: Vec<_> = preview
        .changed
        .iter()
        .chain(&preview.unchanged)
        .cloned()
        .collect();
    for row in &mut rows {
        if let Some(old) = unchanged.unchanged.iter().find(|old| old.key == row.key) {
            if mcp_content_chars(&old.after).unwrap_or(usize::MAX)
                > mcp_content_chars(&row.after).unwrap_or(usize::MAX)
            {
                row.after = old.after.clone();
            }
            for (index, previous) in old.rows.iter().enumerate() {
                if let Some(next) = row.rows.get_mut(index) {
                    for (next, previous) in [
                        (&mut next.item, &previous.item),
                        (&mut next.before, &previous.before),
                        (&mut next.after, &previous.after),
                    ] {
                        if mcp_content_chars(previous).unwrap_or(usize::MAX)
                            > mcp_content_chars(next).unwrap_or(usize::MAX)
                        {
                            *next = previous.clone();
                        }
                    }
                } else {
                    row.rows.push(previous.clone());
                }
            }
        }
    }
    SettingsWriteResult {
        changed: rows,
        unchanged: Vec::new(),
    }
}

#[cfg(test)]
#[path = "agent_settings_tests.rs"]
mod tests;

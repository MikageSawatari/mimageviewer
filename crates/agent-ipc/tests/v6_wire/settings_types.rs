// Frozen from a2d268b915e6de9e9e7df2e086c3fb1387820279:crates/agent-ipc/src/settings_types.rs; tests removed.
#![allow(dead_code)]
//! Standard preference values and their user-visible change descriptions.
use super::PageRow;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;

pub const MAX_SETTINGS_KEYS: usize = 32;
pub const SETTINGS_PAGE_SIZE: usize = 60;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SettingChoice {
    pub value: Value,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SettingRow {
    pub key: String,
    pub label: String,
    pub value: Value,
    pub overridden_by_favorite: bool,
    /// Omitted in listings; explicit null for details without enumerated choices.
    #[serde(
        default,
        skip_serializing_if = "Option::is_none",
        deserialize_with = "deserialize_choices"
    )]
    pub choices: Option<Option<Vec<SettingChoice>>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub template: Option<Value>,
}

fn deserialize_choices<'de, D>(
    deserializer: D,
) -> Result<Option<Option<Vec<SettingChoice>>>, D::Error>
where
    D: Deserializer<'de>,
{
    Option::<Vec<SettingChoice>>::deserialize(deserializer).map(Some)
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SettingsPage {
    pub items: Vec<PageRow<SettingRow>>,
    pub page: u32,
    pub pages: u32,
    pub more: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SettingChange {
    pub key: String,
    pub label: String,
    pub before: Value,
    pub after: Value,
    pub overridden_by_favorite: bool,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub rows: Vec<SettingChangeRow>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct SettingChangeRow {
    pub item: String,
    pub before: String,
    pub after: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct SettingsWriteResult {
    pub changed: Vec<SettingChange>,
    pub unchanged: Vec<SettingChange>,
}

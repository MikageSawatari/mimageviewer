// Frozen from 23d73f026349cadd258f295cd8ecf366194d1f3f:crates/agent-ipc/src/operations_types.rs; tests removed.
//! Operation customization uses existing persisted values; this is only the wire view.
use super::PageRow;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OperationsQuery {
    pub view: Option<String>,
    pub query: Option<String>,
    pub key: Option<String>,
    pub actions: Option<Vec<String>>,
    pub context: Option<String>,
    pub screen: Option<String>,
    pub slots: Option<String>,
    pub slot: Option<String>,
    pub page: Option<u32>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct OperationEdits {
    pub keys: Option<Map<String, Value>>,
    pub inputs: Option<Map<String, Value>>,
}

impl OperationsQuery {
    /// Shape validation shared by the adapter and core; catalogs belong to core.
    pub fn validate(&self) -> Result<(), String> {
        let view = self.view.as_deref().unwrap_or("keys");
        let allowed: &[&str] = match view {
            "keys" => &["query", "key", "actions", "context", "screen"],
            "inputs" => &["slots"],
            "conflicts" => &["actions", "key"],
            "choices" => &["slot"],
            _ => return Err("view は keys / inputs / conflicts / choices です".into()),
        };
        for (name, present) in [
            ("query", self.query.is_some()),
            ("key", self.key.is_some()),
            ("actions", self.actions.is_some()),
            ("context", self.context.is_some()),
            ("screen", self.screen.is_some()),
            ("slots", self.slots.is_some()),
            ("slot", self.slot.is_some()),
        ] {
            if present && !allowed.contains(&name) {
                return Err(format!("view={view} では {name} は使えません"));
            }
        }
        if self.page == Some(0) {
            return Err("page は 1 以上です".into());
        }
        if self
            .query
            .as_ref()
            .is_some_and(|text| text.chars().take(201).count() > 200)
        {
            return Err("query は 200 文字以内です".into());
        }
        if self
            .actions
            .as_ref()
            .is_some_and(|actions| actions.is_empty() || actions.len() > super::MAX_SETTINGS_KEYS)
        {
            return Err("actions は 1～32 個です".into());
        }
        if view == "keys" {
            if [
                self.query.is_some(),
                self.key.is_some(),
                self.actions.is_some(),
            ]
            .into_iter()
            .filter(|v| *v)
            .count()
                > 1
            {
                return Err("query / key / actions はどれか 1 つです".into());
            }
            if self.context.is_some() && self.screen.is_some() {
                return Err(
                    "context と screen は併用できません。空きの確認は screen と key を使います"
                        .into(),
                );
            }
        }
        if view == "conflicts"
            && (self.actions.is_some() != self.key.is_some()
                || self
                    .actions
                    .as_ref()
                    .is_some_and(|actions| actions.len() != 1))
        {
            return Err("試算は actions 1 個と key を併せて指定してください".into());
        }
        if view == "choices" && self.slot.is_none() {
            return Err("choices には slot を 1 つ指定してください".into());
        }
        Ok(())
    }
}

impl OperationEdits {
    pub fn validate(&self) -> Result<(), String> {
        let count = self.keys.as_ref().map_or(0, |values| values.len())
            + self.inputs.as_ref().map_or(0, |values| values.len());
        if !(1..=super::MAX_SETTINGS_KEYS).contains(&count) {
            return Err("keys と inputs を合わせて 1～32 項目で指定してください".into());
        }
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationKeyRow {
    pub action: String,
    pub label: String,
    pub context: String,
    pub context_label: String,
    pub trigger: String,
    pub keys: Vec<String>,
    pub default: Vec<String>,
    pub customized: bool,
    pub inputs: Vec<String>,
    pub screens: Vec<String>,
    pub screen_labels: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationInputRow {
    pub slot: String,
    pub label: String,
    pub value: Value,
    pub value_label: Value,
    pub default: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationConflict {
    pub key: String,
    pub kind: String,
    pub action: String,
    pub other: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationChoice {
    pub value: Value,
    pub label: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(untagged)]
pub enum OperationRow {
    Key(OperationKeyRow),
    Input(OperationInputRow),
    Conflict(OperationConflict),
    Choice(OperationChoice),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationsPage {
    pub items: Vec<PageRow<OperationRow>>,
    pub page: u32,
    pub pages: u32,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub reserved: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub note: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct OperationChange {
    pub name: String,
    pub label: String,
    pub before: Value,
    pub after: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct OperationsWriteResult {
    pub changed: Vec<OperationChange>,
    pub unchanged: Vec<OperationChange>,
    pub conflicts: Vec<OperationConflict>,
}

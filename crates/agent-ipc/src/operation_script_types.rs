//! Additive v8 operation-script responses. State is captured at UI admission,
//! independently of the source revision read or written by the FIFO worker.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum OperationScriptLoadState {
    Disabled,
    Loading,
    Ready,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum OperationScriptResult {
    Read(OperationScriptReadResult),
    Write(OperationScriptWriteResult),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperationScriptReadResult {
    pub source: String,
    /// SHA-256 of the complete UTF-8 source, including builtin defaults.
    pub sha256: String,
    pub builtin: bool,
    pub enabled_at_admission: bool,
    pub load_state_at_admission: OperationScriptLoadState,
    pub failure_reason_at_admission: Option<String>,
    pub part: u32,
    pub parts: u32,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct OperationScriptWriteResult {
    pub sha256: String,
    pub enabled_at_admission: bool,
    pub load_state_at_admission: OperationScriptLoadState,
    pub failure_reason_at_admission: Option<String>,
    pub before_lines: usize,
    pub after_lines: usize,
    pub message: String,
}

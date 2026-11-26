// Frozen from 23d73f026349cadd258f295cd8ecf366194d1f3f:crates/agent-ipc/src/stack_script_types.rs; tests removed.
//! Additive v6 stack-script responses. Writes never echo the prior source.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(untagged)]
pub enum StackScriptResult {
    Read(StackScriptReadResult),
    Write(StackScriptWriteResult),
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackScriptReadResult {
    pub source: String,
    /// SHA-256 of the complete UTF-8 source, including for builtin defaults.
    pub sha256: String,
    pub builtin: bool,
    pub enabled: bool,
    pub part: u32,
    pub parts: u32,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct StackScriptWriteResult {
    pub sha256: String,
    pub enabled: bool,
    pub before_lines: usize,
    pub after_lines: usize,
    pub message: String,
}

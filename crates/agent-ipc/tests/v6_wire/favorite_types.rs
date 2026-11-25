// Frozen from a2d268b915e6de9e9e7df2e086c3fb1387820279:crates/agent-ipc/src/favorite_types.rs; tests removed.
#![allow(dead_code)]
//! Favorite registration and reversible index flags.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq)]
pub struct FavoriteIndexState {
    pub item_index: bool,
    pub container_index: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct FavoriteWriteResult {
    pub path: String,
    pub name: String,
    pub added: bool,
    pub before: Option<FavoriteIndexState>,
    pub after: FavoriteIndexState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<String>,
}

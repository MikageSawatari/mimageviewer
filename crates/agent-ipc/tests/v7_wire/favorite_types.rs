// Frozen from 23d73f026349cadd258f295cd8ecf366194d1f3f:crates/agent-ipc/src/favorite_types.rs; tests removed.
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

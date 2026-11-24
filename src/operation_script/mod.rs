//! 操作スクリプトの土台。入力・操作の実行・イベントの検出はまだ接続しない。
//! 正本: docs/operation-script-plan.md (§1.363)。

mod contract;
mod engine;
mod loader;
mod snapshot;
mod syntax;

pub use contract::*;
pub use engine::build_engine;
pub use loader::*;
pub use snapshot::*;

pub const MAX_OPERATIONS: u64 = 2_000;
pub const MAX_STRING_BYTES: usize = 8 * 1024;
pub const MAX_COLLECTION_SIZE: usize = 200;
pub const MAX_ACTIONS: usize = 16;
pub const SLOT_COUNT: u8 = 20;
pub const DEFAULT_SCRIPT: &str = include_str!("../../assets/operation_scripts.default.rhai");

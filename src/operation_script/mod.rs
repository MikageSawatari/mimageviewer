//! 操作スクリプトのサンドボックス・読み込み・契約・状態の写し。
//! 入力と同期実行の所有境界は app::operation_script_runtime に置く。
//! 正本: docs/operation-script-plan.md (§1.363)。

mod contract;
mod engine;
mod file_worker;
mod loader;
#[cfg(test)]
mod manual_examples_tests;
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

//! 利用者が保存する名前付きコレクションの永続基盤。
//!
//! UI / Remote は SQLite を直接開かず、[`CollectionStoreRuntime`] が所有する単一 actor を
//! 経由する。App への表示統合は別段階で行い、この module 自体は path identity、DB transaction、
//! immutable snapshot、text import/export の正本を提供する。

mod db;
mod model;
mod path;
mod runtime;
mod text;

pub use model::*;
pub use path::{CollectionImportPathPolicy, CollectionSourcePath};
pub use runtime::{
    CollectionRevisionWatch, CollectionRuntimeEvent, CollectionStoreClient, CollectionStoreRuntime,
};
pub use text::{
    CollectionImportLine, CollectionImportLineStatus, CollectionImportPreview,
    parse_collection_text, serialize_collection_paths,
};

#[cfg(test)]
mod tests;

//! 利用者が保存する名前付きコレクションの永続基盤。
//!
//! UI / Remote は SQLite を直接開かず、[`CollectionStoreRuntime`] が所有する単一 actor を
//! 経由する。App への表示統合は別段階で行い、この module 自体は path identity、DB transaction、
//! immutable snapshot、text import/export の正本を提供する。

mod db;
mod export_all;
mod model;
mod path;
mod prepare;
mod read_lease;
mod runtime;
mod text;

pub(crate) use export_all::{CollectionAllExportFailure, write_all_collections_export};
pub use model::*;
pub use path::{CollectionImportPathPolicy, CollectionSourcePath};
pub(crate) use prepare::{
    CollectionNavigationAnchor, CollectionNavigationAnchorResolution,
    CollectionNavigationDirection, CollectionNavigationEntryIdentity, CollectionNavigationTail,
    CollectionNavigationTargetKind, CollectionPrepareError, CollectionPrepareReuseKey,
    CollectionPreparedNavigationCandidates, CollectionPreparedNavigationTarget,
    CollectionPreparedRegistration, CollectionPreparedSnapshot, CollectionSourcePreparation,
    PreparedCollectionEntry, inspect_collection_source, prepare_collection_export,
    prepare_collection_registrations, prepare_collection_snapshot,
    prepare_collection_snapshot_while, resolve_prepared_collection_navigation,
    write_collection_export_atomic,
};
pub(crate) use read_lease::{CollectionReadLease, CollectionReadScope};
pub(crate) use runtime::{
    CollectionRemoteProducerControl, CollectionRemoteRequestLease, CollectionRuntimeEventStream,
};
pub use runtime::{
    CollectionRevisionWatch, CollectionRuntimeEvent, CollectionStoreClient, CollectionStoreRuntime,
};
pub use text::{
    CollectionImportLimitError, CollectionImportLine, CollectionImportLineStatus,
    CollectionImportPreview, MAX_COLLECTION_IMPORT_BYTES, MAX_COLLECTION_IMPORT_NONEMPTY_LINES,
    parse_collection_text, serialize_collection_paths,
};

#[cfg(test)]
mod tests;

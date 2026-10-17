//! Book composition pure model and process-owned worker-only persistence.
//!
//! G2A deliberately does not define `BookScopeKey`: the later store will use line E's
//! key. `PageDomain` only validates the kind and direct membership of page references.
mod edit;
mod editor;
mod model;
mod permission;
mod projection;
mod resolve;
mod service;
pub(crate) mod store;

pub use edit::CompositionEdit;
pub(crate) use editor::*;
pub use model::*;
pub use permission::CompositionOutputCheck;
pub(crate) use permission::*;
pub(crate) use projection::*;
pub use resolve::*;
pub(crate) use service::*;
#[cfg(test)]
pub(crate) use store::initialize_database_at;
pub(crate) use store::{
    StoredComposition, migrate_page_paths_at, validate_database_rows, validate_database_schema,
};

#[cfg(test)]
mod editor_tests;
#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod permission_tests;
#[cfg(test)]
mod projection_tests;
#[cfg(test)]
mod store_tests;
#[cfg(test)]
mod tests;

//! Pure book composition model. No store, scope key, App, UI, I/O or automatic pairing.
//!
//! G2A deliberately does not define `BookScopeKey`: the later store will use line E's
//! key. `PageDomain` only validates the kind and direct membership of page references.
mod edit;
mod model;
mod resolve;

pub use edit::UnitEdit;
pub use model::*;
pub use resolve::*;

#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod tests;

//! Per-logical-book shift persistence. Display/input integration belongs to D2.
//! No existing spread state is written or hydrated by this service.

mod model;
mod service;
mod store;

#[cfg(test)]
pub(crate) mod contract_fixtures;

pub(crate) use model::*;
pub(crate) use service::{
    SpreadShiftMaintenanceLease, SpreadShiftProcessOwner, SpreadShiftService, maintenance_at,
};
pub(crate) use store::{initialize_database_at, migrate_page_paths_at, validate_database_schema};

pub(crate) fn validate_database_rows(connection: &rusqlite::Connection) -> Result<(), String> {
    store::load_rows(connection).map(|_| ())
}

use crate::{app::ViewerContextId, book_scope::BookScopeKey};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ShiftWatermarks {
    pub(crate) admitted: u64,
    pub(crate) completed_through: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShiftOperation {
    pub(crate) context_id: ViewerContextId,
    pub(crate) items_generation: u64,
    pub(crate) operation_sequence: u64,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShiftReadReply {
    pub(crate) cutoff: u64,
    pub(crate) completed_through: u64,
    /// Stamp for the requested book scope, independent of unrelated maintenance.
    pub(crate) maintenance_epoch: u64,
    pub(crate) row: Option<SpreadShiftPageRef>,
    pub(crate) status: Result<(), String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShiftWriteReply {
    pub(crate) sequence: u64,
    pub(crate) scope: BookScopeKey,
    pub(crate) operation: ShiftOperation,
    pub(crate) kind: ShiftWriteKind,
    pub(crate) changed: bool,
    pub(crate) status: Result<(), String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShiftCountReply {
    pub(crate) cutoff: u64,
    pub(crate) completed_through: u64,
    pub(crate) maintenance_epoch: u64,
    pub(crate) count: usize,
    pub(crate) status: Result<(), String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct ShiftClearAllReply {
    pub(crate) sequence: u64,
    pub(crate) removed: usize,
    pub(crate) status: Result<(), String>,
}

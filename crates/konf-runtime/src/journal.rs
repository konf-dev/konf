//! Event journal — append-only audit log for workflow lifecycle events.
//!
//! The trait, entry schema, and error types are defined in
//! `konflux_substrate::journal`. This module re-exports them and houses
//! the concrete backends (RedbJournal, FanoutJournalStore).

pub mod fanout;
pub mod redb;
pub mod subscribe;
pub mod sweep;

// Re-export the substrate-defined journal contract so runtime consumers
// can keep importing from `crate::journal::*`.
pub use konflux_substrate::journal::{
    AggregateQuery, AggregateResult, JournalEntry, JournalError, JournalFilter, JournalRow,
    JournalStore, RunId,
};

pub use fanout::{FanoutJournalStore, FanoutMetrics};
pub use redb::RedbJournal;

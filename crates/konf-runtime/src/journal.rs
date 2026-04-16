//! Event journal — append-only audit log for workflow lifecycle events.
//!
//! The trait, entry schema, and error types are defined in
//! `konflux_substrate::journal`. This module re-exports them and houses
//! the concrete backends (RedbJournal, FanoutJournalStore).

pub mod fanout;
pub mod redb;

// Re-export the substrate-defined journal contract so runtime consumers
// can keep importing from `crate::journal::*`.
pub use konflux_substrate::journal::{JournalEntry, JournalError, JournalRow, JournalStore, RunId};

pub use fanout::{FanoutJournalStore, FanoutMetrics};
pub use redb::RedbJournal;

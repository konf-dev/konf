//! Event journal — append-only audit log for workflow lifecycle events.
//!
//! The journal is an **audit** trail, not a replay log. It records every
//! workflow lifecycle event (`workflow_started`, `node_started`,
//! `workflow_completed`, etc.) for debugging, monitoring, and admin query.
//! It is not used to reconstruct mid-workflow execution state — konf
//! explicitly rejects checkpoint-and-replay durability because AI workflows
//! are non-deterministic (see `docs/architecture/durability.md`).
//!
//! # Shape
//!
//! - [`JournalStore`] is the trait every backend implements. It is
//!   intentionally small: append, query-by-run, query-by-session, recent,
//!   and reconcile-zombies.
//! - [`JournalEntry`] is the write-side view — what callers append.
//! - [`JournalRow`] is the read-side view — what queries return. It carries
//!   the backend-assigned sequence id and the timestamp at which the entry
//!   was recorded.
//!
//! # Backends
//!
//! - [`redb::RedbJournal`] — the default, embedded, pure-Rust backend.
//!   Backed by a single redb file shared with the scheduler and runner
//!   intent store via [`crate::storage::KonfStorage`].

use std::error::Error as StdError;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::RunId;

pub mod redb;

pub use redb::RedbJournal;

/// Errors raised by [`JournalStore`] operations.
#[derive(Debug, thiserror::Error)]
pub enum JournalError {
    /// The backing store raised an error (IO, corruption, transaction
    /// failure). The underlying error is boxed so the trait surface stays
    /// backend-agnostic.
    #[error("storage: {0}")]
    Storage(#[source] Box<dyn StdError + Send + Sync>),

    /// A value could not be serialized or deserialized to/from the backend.
    #[error("serialization: {0}")]
    Serialization(Box<dyn StdError + Send + Sync>),

    /// The requested entry was not found.
    #[error("not found")]
    NotFound,
}

impl JournalError {
    /// Helper to wrap any `Error + Send + Sync + 'static` as a storage error.
    pub fn storage<E: StdError + Send + Sync + 'static>(e: E) -> Self {
        Self::Storage(Box::new(e))
    }

    /// Helper to wrap any `Error + Send + Sync + 'static` as a serialization error.
    pub fn serialization<E: StdError + Send + Sync + 'static>(e: E) -> Self {
        Self::Serialization(Box::new(e))
    }
}

/// One event to append to the journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub run_id: RunId,
    pub session_id: String,
    pub namespace: String,
    pub event_type: String,
    pub payload: Value,
}

/// One row returned from the journal.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalRow {
    /// Monotonic sequence assigned by the backend at append time.
    pub id: u64,
    pub run_id: RunId,
    pub session_id: String,
    pub namespace: String,
    pub event_type: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

/// Append-only event journal.
///
/// All methods are async even when the underlying backend is synchronous,
/// because implementations wrap blocking work in `tokio::task::spawn_blocking`
/// to keep the tokio scheduler responsive.
#[async_trait::async_trait]
pub trait JournalStore: Send + Sync + 'static {
    /// Append one event. Returns the backend-assigned sequence id.
    async fn append(&self, entry: JournalEntry) -> Result<u64, JournalError>;

    /// Return every entry for a specific run, ordered by sequence ascending.
    async fn query_by_run(&self, run_id: RunId) -> Result<Vec<JournalRow>, JournalError>;

    /// Return up to `limit` entries for a specific session, most recent first.
    async fn query_by_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<JournalRow>, JournalError>;

    /// Return the `limit` most recent entries across all runs.
    async fn recent(&self, limit: usize) -> Result<Vec<JournalRow>, JournalError>;

    /// Inspect the journal for workflows that started but never reached a
    /// terminal event, and append a synthetic `workflow_failed` event for
    /// each. Returns the number of entries reconciled.
    ///
    /// Intended to be called once at startup. Matches the semantics of the
    /// prior Postgres implementation's `reconcile_zombies` — a crashed
    /// `konf-backend` left live workflows in an ambiguous state; after
    /// restart the journal should surface them as failed.
    async fn reconcile_zombies(&self) -> Result<u64, JournalError>;
}

//! Journal trait — append-only audit log contract.
//!
//! The substrate defines the trait and schema; concrete backends
//! (RedbJournal, FanoutJournalStore, SurrealJournalStore) live in
//! konf-runtime.

use std::error::Error as StdError;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use uuid::Uuid;

/// Unique identifier for a workflow run.
pub type RunId = Uuid;

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
///
/// `run_id` is `None` for direct tool invocations that occur outside a
/// workflow run (closes concession #4: `Uuid::nil()` sentinel removed).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JournalEntry {
    pub run_id: Option<RunId>,
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
    pub run_id: Option<RunId>,
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
    /// Intended to be called once at startup.
    async fn reconcile_zombies(&self) -> Result<u64, JournalError>;
}

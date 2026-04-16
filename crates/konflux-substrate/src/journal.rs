//! Journal trait — append-only audit log contract.
//!
//! The substrate defines the trait and schema; concrete backends
//! (RedbJournal, FanoutJournalStore, SurrealJournalStore) live in
//! konf-runtime.

use std::error::Error as StdError;
use std::time::Duration;

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
    /// When this entry expires and becomes invisible to queries.
    /// `None` means the entry never expires.
    #[serde(default)]
    pub valid_to: Option<DateTime<Utc>>,
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
    /// When this entry expires. `None` means never.
    #[serde(default)]
    pub valid_to: Option<DateTime<Utc>>,
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

    /// Query entries matching a filter, respecting the expired-invisible
    /// invariant unless `filter.include_expired` is true. Returns up to
    /// `limit` entries, most recent first.
    async fn query(
        &self,
        _filter: &JournalFilter,
        _limit: usize,
    ) -> Result<Vec<JournalRow>, JournalError> {
        Ok(vec![])
    }

    /// Compute an aggregate over entries matching a filter. Respects
    /// the expired-invisible invariant by default.
    async fn aggregate(
        &self,
        _filter: &JournalFilter,
        _query: &AggregateQuery,
    ) -> Result<AggregateResult, JournalError> {
        Ok(AggregateResult::Count(0))
    }

    /// Physically delete all entries where `valid_to < now`. Returns the
    /// number of entries removed. Implementations that don't support TTL
    /// return `Ok(0)`.
    async fn delete_expired(&self) -> Result<u64, JournalError> {
        Ok(0)
    }
}

/// Filter predicate for journal queries and subscriptions.
#[derive(Debug, Clone, Default)]
pub struct JournalFilter {
    pub namespace: Option<String>,
    pub event_type: Option<String>,
    pub run_id: Option<RunId>,
    pub session_id: Option<String>,
    pub trace_id: Option<Uuid>,
    /// If true, include entries past their `valid_to`. Default false —
    /// expired entries are invisible.
    pub include_expired: bool,
}

impl JournalFilter {
    /// Check if a row matches this filter (client-side predicate).
    pub fn matches(&self, row: &JournalRow) -> bool {
        if let Some(ref ns) = self.namespace {
            if row.namespace != *ns {
                return false;
            }
        }
        if let Some(ref et) = self.event_type {
            if row.event_type != *et {
                return false;
            }
        }
        if let Some(ref rid) = self.run_id {
            if row.run_id.as_ref() != Some(rid) {
                return false;
            }
        }
        if let Some(ref sid) = self.session_id {
            if row.session_id != *sid {
                return false;
            }
        }
        if let Some(ref tid) = self.trace_id {
            // trace_id is inside the payload (Interaction.trace_id).
            // For now, skip this check if the payload doesn't contain it.
            if let Some(payload_tid) = row.payload.get("trace_id").and_then(|v| v.as_str()) {
                if payload_tid != tid.to_string() {
                    return false;
                }
            }
        }
        if !self.include_expired {
            if let Some(valid_to) = row.valid_to {
                if valid_to < Utc::now() {
                    return false;
                }
            }
        }
        true
    }
}

/// What to compute over matching journal entries.
#[derive(Debug, Clone)]
pub enum AggregateQuery {
    /// Count matching entries.
    Count,
    /// Find the most recent `created_at` among matching entries.
    MostRecent,
    /// Sum a numeric field from payload within a time window.
    /// Stub — returns error until a concrete use case exists.
    WindowSum { field: String, window: Duration },
}

/// Result of an aggregate computation.
#[derive(Debug, Clone, PartialEq)]
pub enum AggregateResult {
    Count(u64),
    MostRecent(Option<DateTime<Utc>>),
    WindowSum(f64),
}

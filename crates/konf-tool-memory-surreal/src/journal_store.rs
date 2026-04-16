//! SurrealDB-backed implementation of [`JournalStore`].
//!
//! Writes each [`JournalEntry`] as a row in the existing `event` table
//! (defined in [`crate::schema`]). This is the secondary backend wired into
//! [`konf_runtime::FanoutJournalStore`] alongside the primary
//! [`konf_runtime::RedbJournal`] to give konf a long-term queryable
//! interaction graph on top of short-retention redb audit storage.
//!
//! # Recursion avoidance (schema-level guarantee)
//!
//! This store writes via the raw `Surreal<Any>` handle directly — it does
//! **not** go through [`konf_tool_memory::MemoryBackend`]. As a consequence
//! no journal append ever dispatches a tool, so the recorder cannot create
//! an infinite write loop even if the memory backend were to itself emit
//! journal entries on every write. The absence of the
//! `MemoryBackend` call path is the structural proof; no thread-local flag
//! is required.
//!
//! # Sequence id semantics
//!
//! [`JournalRow::id`] from this store is a per-process counter initialized
//! fresh at [`SurrealJournalStore::new`]. It is monotonic within a single
//! process but not durable across restarts. This is acceptable because:
//!
//! - Primary audit integrity lives in the redb journal (durable, monotonic)
//! - Secondary rows are used for queryable graph traversal, not replay
//! - Callers treat `JournalRow::id` as informational, not load-bearing

use std::sync::atomic::{AtomicU64, Ordering};

use async_trait::async_trait;
use konf_runtime::{JournalEntry, JournalError, JournalRow, JournalStore, RunId};
use serde::Deserialize;
use serde_json::{json, Value};
use surrealdb::engine::any::Any;
use surrealdb::Surreal;
use uuid::Uuid;

/// Writes [`JournalEntry`] rows to the SurrealDB `event` table.
pub struct SurrealJournalStore {
    db: Surreal<Any>,
    counter: AtomicU64,
}

impl SurrealJournalStore {
    /// Build a journal store over an already-connected SurrealDB handle.
    ///
    /// The schema is expected to have been applied at connection time (via
    /// [`crate::connect`] or manually). This constructor does not re-apply
    /// the schema; safe to call multiple times against the same handle.
    pub fn new(db: Surreal<Any>) -> Self {
        Self {
            db,
            counter: AtomicU64::new(0),
        }
    }
}

/// Raw event row as read from the SurrealDB `event` table. Internal helper
/// for deserializing query results.
#[derive(Deserialize)]
struct RawEventRow {
    namespace: String,
    event_type: String,
    payload: Value,
    created_at: chrono::DateTime<chrono::Utc>,
    #[serde(default)]
    valid_to: Option<chrono::DateTime<chrono::Utc>>,
    #[serde(default)]
    idempotency_key: Option<String>,
}

impl RawEventRow {
    fn into_journal_row(self) -> JournalRow {
        let obj = self.payload.as_object();

        let run_id = obj
            .and_then(|o| o.get("run_id"))
            .and_then(Value::as_str)
            .and_then(|s| Uuid::parse_str(s).ok());

        let session_id = obj
            .and_then(|o| o.get("session_id"))
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string();

        let data = obj
            .and_then(|o| o.get("data"))
            .cloned()
            .unwrap_or(Value::Null);

        let seq = obj
            .and_then(|o| o.get("seq"))
            .and_then(Value::as_u64)
            .unwrap_or(0);

        JournalRow {
            id: seq,
            run_id,
            session_id,
            namespace: self.namespace,
            event_type: self.event_type,
            payload: data,
            created_at: self.created_at,
            valid_to: self.valid_to,
            idempotency_key: self.idempotency_key.clone(),
        }
    }
}

fn wrap_db_err<E: std::fmt::Display>(e: E) -> JournalError {
    JournalError::Storage(Box::new(std::io::Error::other(e.to_string())))
}

/// Convert a batch of raw SurrealDB row objects (as `serde_json::Value`) into
/// [`JournalRow`] entries. Rows that fail to deserialize are skipped with a
/// `tracing::warn!` — the journal prefers partial data over a whole-query
/// failure, matching the "secondary is best-effort" contract.
fn rows_into_journal(rows: Vec<Value>) -> Result<Vec<JournalRow>, JournalError> {
    let mut out = Vec::with_capacity(rows.len());
    for row in rows {
        match serde_json::from_value::<RawEventRow>(row) {
            Ok(raw) => out.push(raw.into_journal_row()),
            Err(e) => tracing::warn!(error = %e, "skipping malformed event row"),
        }
    }
    Ok(out)
}

#[async_trait]
impl JournalStore for SurrealJournalStore {
    async fn append(&self, entry: JournalEntry) -> Result<u64, JournalError> {
        let seq = self.counter.fetch_add(1, Ordering::Relaxed);
        let payload = json!({
            "seq": seq,
            "run_id": entry.run_id.map(|id| id.to_string()),
            "session_id": entry.session_id,
            "data": entry.payload,
        });

        self.db
            .query(
                "CREATE event SET namespace = $ns, event_type = $et, payload = $p, valid_to = $vt",
            )
            .bind(("ns", entry.namespace))
            .bind(("et", entry.event_type))
            .bind(("p", payload))
            .bind(("vt", entry.valid_to))
            .await
            .map_err(wrap_db_err)?
            .check()
            .map_err(wrap_db_err)?;

        Ok(seq)
    }

    async fn query_by_run(&self, run_id: RunId) -> Result<Vec<JournalRow>, JournalError> {
        let run_id_str = run_id.to_string();
        let mut response = self
            .db
            .query(
                "SELECT namespace, event_type, payload, created_at, valid_to \
                 FROM event \
                 WHERE payload.run_id = $rid \
                 AND (valid_to IS NONE OR valid_to > time::now()) \
                 ORDER BY created_at ASC",
            )
            .bind(("rid", run_id_str))
            .await
            .map_err(wrap_db_err)?;
        let rows: Vec<Value> = response.take(0).map_err(wrap_db_err)?;
        rows_into_journal(rows)
    }

    async fn query_by_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<JournalRow>, JournalError> {
        let mut response = self
            .db
            .query(
                "SELECT namespace, event_type, payload, created_at, valid_to \
                 FROM event \
                 WHERE payload.session_id = $sid \
                 AND (valid_to IS NONE OR valid_to > time::now()) \
                 ORDER BY created_at DESC \
                 LIMIT $lim",
            )
            .bind(("sid", session_id.to_string()))
            .bind(("lim", limit))
            .await
            .map_err(wrap_db_err)?;
        let rows: Vec<Value> = response.take(0).map_err(wrap_db_err)?;
        rows_into_journal(rows)
    }

    async fn recent(&self, limit: usize) -> Result<Vec<JournalRow>, JournalError> {
        let mut response = self
            .db
            .query(
                "SELECT namespace, event_type, payload, created_at, valid_to \
                 FROM event \
                 WHERE valid_to IS NONE OR valid_to > time::now() \
                 ORDER BY created_at DESC \
                 LIMIT $lim",
            )
            .bind(("lim", limit))
            .await
            .map_err(wrap_db_err)?;
        let rows: Vec<Value> = response.take(0).map_err(wrap_db_err)?;
        rows_into_journal(rows)
    }

    async fn delete_expired(&self) -> Result<u64, JournalError> {
        let mut response = self
            .db
            .query(
                "DELETE event WHERE valid_to IS NOT NONE AND valid_to < time::now() RETURN BEFORE",
            )
            .await
            .map_err(wrap_db_err)?;
        let deleted: Vec<Value> = response.take(0).map_err(wrap_db_err)?;
        Ok(deleted.len() as u64)
    }

    async fn reconcile_zombies(&self) -> Result<u64, JournalError> {
        // Secondary stores do not own the primary's responsibility to
        // reconcile crashed workflows. The primary ([`RedbJournal`]) is
        // consulted at boot; this mirror contains a best-effort read-side
        // replica.
        Ok(0)
    }
}

//! Durable runner intent store — redb-backed persistence of
//! `runner:spawn` intents for restart replay.
//!
//! See `docs/architecture/durability.md` for the doctrine. Short version:
//!
//! - An intent is persisted **before** the tokio task is spawned.
//! - On successful completion the intent is marked
//!   [`TerminalStatus::Succeeded`] (or Failed/Cancelled).
//! - On crash, unterminated intents are replayed from the top on next boot
//!   with the **same `run_id`** so external references (TUI bookmarks,
//!   journal entries) still resolve. The workflow is NOT resumed mid-step —
//!   konf rejects checkpoint-and-replay because LLM calls are
//!   non-deterministic.
//! - Authors are responsible for idempotency (use `memory:*` for cursors
//!   and dedup keys).
//!
//! # Storage layout
//!
//! Two tables in the shared [`crate::KonfStorage`] redb database:
//!
//! - `runner_intents`: `[u8;16] run_id -> postcard(StoredIntent)`
//! - `runner_intents_by_namespace`: `(namespace_bytes, [u8;16] run_id) -> ()`
//!
//! The secondary index lets us list intents by namespace without scanning
//! every entry.
//!
//! # Garbage collection
//!
//! [`RunnerIntentStore::gc`] deletes terminal intents older than a retention
//! window. Called periodically from a background task owned by
//! [`crate::KonfStorage`].

use std::sync::Arc;

use chrono::{DateTime, TimeZone, Utc};
use redb::{
    Database, MultimapTableDefinition, ReadableDatabase, ReadableTable, TableDefinition,
};
use serde::{Deserialize, Serialize};

use crate::scope::Actor;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Errors raised by [`RunnerIntentStore`].
#[derive(Debug, thiserror::Error)]
pub enum IntentError {
    #[error("storage: {0}")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("serialization: {0}")]
    Serialization(#[source] Box<dyn std::error::Error + Send + Sync>),

    #[error("task join: {0}")]
    Join(String),
}

impl IntentError {
    fn storage<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::Storage(Box::new(e))
    }
    fn serialization<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::Serialization(Box::new(e))
    }
}

/// Stable identifier for an intent. The string form of a UUID v4, matching
/// `konf_tool_runner::registry::RunId` so both layers agree on ids.
pub type IntentId = String;

/// Terminal outcome of an intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalStatus {
    Succeeded,
    Failed { error: String },
    Cancelled { reason: String },
}

/// A persisted spawn intent.
#[derive(Debug, Clone)]
pub struct RunnerIntent {
    pub run_id: IntentId,
    pub parent_id: Option<IntentId>,
    pub workflow: String,
    pub input: serde_json::Value,
    pub namespace: String,
    pub capabilities: Vec<String>,
    pub actor: Actor,
    pub session_id: String,
    pub spawned_at: DateTime<Utc>,
    pub terminal: Option<TerminalStatus>,
    pub replay_count: u32,
}

impl RunnerIntent {
    /// Build a new non-terminal intent.
    pub fn new(
        run_id: impl Into<IntentId>,
        workflow: impl Into<String>,
        input: serde_json::Value,
        namespace: impl Into<String>,
        capabilities: Vec<String>,
        actor: Actor,
        session_id: impl Into<String>,
    ) -> Self {
        Self {
            run_id: run_id.into(),
            parent_id: None,
            workflow: workflow.into(),
            input,
            namespace: namespace.into(),
            capabilities,
            actor,
            session_id: session_id.into(),
            spawned_at: Utc::now(),
            terminal: None,
            replay_count: 0,
        }
    }
}

// ---------------------------------------------------------------------------
// On-disk representation
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredIntent {
    run_id: String,
    parent_id: Option<String>,
    workflow: String,
    input_json: String,
    namespace: String,
    capabilities: Vec<String>,
    actor: Actor,
    session_id: String,
    spawned_at_micros: i64,
    terminal: Option<TerminalStatus>,
    replay_count: u32,
}

impl StoredIntent {
    fn from_intent(intent: &RunnerIntent) -> Result<Self, IntentError> {
        Ok(Self {
            run_id: intent.run_id.clone(),
            parent_id: intent.parent_id.clone(),
            workflow: intent.workflow.clone(),
            input_json: serde_json::to_string(&intent.input).map_err(IntentError::serialization)?,
            namespace: intent.namespace.clone(),
            capabilities: intent.capabilities.clone(),
            actor: intent.actor.clone(),
            session_id: intent.session_id.clone(),
            spawned_at_micros: intent.spawned_at.timestamp_micros(),
            terminal: intent.terminal.clone(),
            replay_count: intent.replay_count,
        })
    }

    fn into_intent(self) -> Result<RunnerIntent, IntentError> {
        let input: serde_json::Value =
            serde_json::from_str(&self.input_json).map_err(IntentError::serialization)?;
        let spawned_at = Utc
            .timestamp_micros(self.spawned_at_micros)
            .single()
            .unwrap_or_else(Utc::now);
        Ok(RunnerIntent {
            run_id: self.run_id,
            parent_id: self.parent_id,
            workflow: self.workflow,
            input,
            namespace: self.namespace,
            capabilities: self.capabilities,
            actor: self.actor,
            session_id: self.session_id,
            spawned_at,
            terminal: self.terminal,
            replay_count: self.replay_count,
        })
    }
}

// ---------------------------------------------------------------------------
// Table definitions
// ---------------------------------------------------------------------------

/// Primary: `run_id (string bytes) -> postcard(StoredIntent)`.
const INTENTS: TableDefinition<&str, &[u8]> = TableDefinition::new("runner_intents");

/// Secondary index: `namespace_bytes -> run_id bytes`.
const INTENTS_BY_NAMESPACE: MultimapTableDefinition<&str, &str> =
    MultimapTableDefinition::new("runner_intents_by_namespace");

// ---------------------------------------------------------------------------
// RunnerIntentStore
// ---------------------------------------------------------------------------

/// redb-backed persistence for runner intents. Cheap to clone.
#[derive(Clone)]
pub struct RunnerIntentStore {
    db: Arc<Database>,
}

impl RunnerIntentStore {
    /// Open the intent store over a shared redb database. Materializes
    /// the required tables on first call.
    pub fn open(db: Arc<Database>) -> Result<Self, IntentError> {
        let write = db.begin_write().map_err(IntentError::storage)?;
        {
            let _ = write.open_table(INTENTS).map_err(IntentError::storage)?;
            let _ = write
                .open_multimap_table(INTENTS_BY_NAMESPACE)
                .map_err(IntentError::storage)?;
        }
        write.commit().map_err(IntentError::storage)?;
        Ok(Self { db })
    }

    /// Insert or replace an intent.
    pub async fn insert(&self, intent: RunnerIntent) -> Result<(), IntentError> {
        let db = self.db.clone();
        let stored = StoredIntent::from_intent(&intent)?;
        let bytes = postcard::to_allocvec(&stored).map_err(IntentError::serialization)?;
        let run_id = intent.run_id.clone();
        let namespace = intent.namespace.clone();

        tokio::task::spawn_blocking(move || -> Result<(), IntentError> {
            let write = db.begin_write().map_err(IntentError::storage)?;
            {
                let mut table = write.open_table(INTENTS).map_err(IntentError::storage)?;
                table
                    .insert(run_id.as_str(), bytes.as_slice())
                    .map_err(IntentError::storage)?;
            }
            {
                let mut ns = write
                    .open_multimap_table(INTENTS_BY_NAMESPACE)
                    .map_err(IntentError::storage)?;
                ns.insert(namespace.as_str(), run_id.as_str())
                    .map_err(IntentError::storage)?;
            }
            write.commit().map_err(IntentError::storage)?;
            Ok(())
        })
        .await
        .map_err(|e| IntentError::Join(e.to_string()))?
    }

    /// Mark an intent terminal. No-op if the intent is missing.
    pub async fn mark_terminal(
        &self,
        run_id: &str,
        status: TerminalStatus,
    ) -> Result<(), IntentError> {
        let Some(mut intent) = self.get(run_id).await? else {
            return Ok(());
        };
        intent.terminal = Some(status);
        self.insert(intent).await
    }

    /// Fetch a single intent by run id.
    pub async fn get(&self, run_id: &str) -> Result<Option<RunnerIntent>, IntentError> {
        let db = self.db.clone();
        let run_id = run_id.to_string();
        tokio::task::spawn_blocking(move || -> Result<Option<RunnerIntent>, IntentError> {
            let read = db.begin_read().map_err(IntentError::storage)?;
            let table = read.open_table(INTENTS).map_err(IntentError::storage)?;
            let Some(bytes) = table.get(run_id.as_str()).map_err(IntentError::storage)? else {
                return Ok(None);
            };
            let stored: StoredIntent =
                postcard::from_bytes(bytes.value()).map_err(IntentError::serialization)?;
            Ok(Some(stored.into_intent()?))
        })
        .await
        .map_err(|e| IntentError::Join(e.to_string()))?
    }

    /// List every intent whose `terminal` is `None` — the replay set.
    /// Ordered by `spawned_at` ascending so replays happen in roughly the
    /// order they were originally spawned.
    pub async fn list_unterminated(&self) -> Result<Vec<RunnerIntent>, IntentError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<RunnerIntent>, IntentError> {
            let read = db.begin_read().map_err(IntentError::storage)?;
            let table = read.open_table(INTENTS).map_err(IntentError::storage)?;
            let mut out = Vec::new();
            for pair in table.iter().map_err(IntentError::storage)? {
                let (_k, v) = pair.map_err(IntentError::storage)?;
                let stored: StoredIntent =
                    postcard::from_bytes(v.value()).map_err(IntentError::serialization)?;
                if stored.terminal.is_none() {
                    out.push(stored.into_intent()?);
                }
            }
            out.sort_by_key(|i| i.spawned_at);
            Ok(out)
        })
        .await
        .map_err(|e| IntentError::Join(e.to_string()))?
    }

    /// List intents by namespace prefix, optionally including terminal ones.
    pub async fn list_by_namespace(
        &self,
        prefix: &str,
        include_terminal: bool,
    ) -> Result<Vec<RunnerIntent>, IntentError> {
        let db = self.db.clone();
        let prefix = prefix.to_string();
        tokio::task::spawn_blocking(move || -> Result<Vec<RunnerIntent>, IntentError> {
            let read = db.begin_read().map_err(IntentError::storage)?;
            let table = read.open_table(INTENTS).map_err(IntentError::storage)?;
            let mut out = Vec::new();
            for pair in table.iter().map_err(IntentError::storage)? {
                let (_k, v) = pair.map_err(IntentError::storage)?;
                let stored: StoredIntent =
                    postcard::from_bytes(v.value()).map_err(IntentError::serialization)?;
                if !stored.namespace.starts_with(prefix.as_str()) {
                    continue;
                }
                if !include_terminal && stored.terminal.is_some() {
                    continue;
                }
                out.push(stored.into_intent()?);
            }
            out.sort_by_key(|i| i.spawned_at);
            Ok(out)
        })
        .await
        .map_err(|e| IntentError::Join(e.to_string()))?
    }

    /// Delete terminal intents older than `older_than`. Returns the number
    /// of entries removed.
    pub async fn gc(&self, older_than: DateTime<Utc>) -> Result<u64, IntentError> {
        let db = self.db.clone();
        let cutoff_micros = older_than.timestamp_micros();
        tokio::task::spawn_blocking(move || -> Result<u64, IntentError> {
            // First pass (read): collect victims.
            let read = db.begin_read().map_err(IntentError::storage)?;
            let table = read.open_table(INTENTS).map_err(IntentError::storage)?;
            let mut victims: Vec<(String, String)> = Vec::new();
            for pair in table.iter().map_err(IntentError::storage)? {
                let (k, v) = pair.map_err(IntentError::storage)?;
                let stored: StoredIntent =
                    postcard::from_bytes(v.value()).map_err(IntentError::serialization)?;
                if stored.terminal.is_some() && stored.spawned_at_micros < cutoff_micros {
                    victims.push((k.value().to_string(), stored.namespace));
                }
            }
            drop(table);
            drop(read);

            if victims.is_empty() {
                return Ok(0);
            }

            // Second pass (write): delete.
            let write = db.begin_write().map_err(IntentError::storage)?;
            {
                let mut table = write.open_table(INTENTS).map_err(IntentError::storage)?;
                for (run_id, _) in &victims {
                    table
                        .remove(run_id.as_str())
                        .map_err(IntentError::storage)?;
                }
            }
            {
                let mut ns = write
                    .open_multimap_table(INTENTS_BY_NAMESPACE)
                    .map_err(IntentError::storage)?;
                for (run_id, namespace) in &victims {
                    ns.remove(namespace.as_str(), run_id.as_str())
                        .map_err(IntentError::storage)?;
                }
            }
            write.commit().map_err(IntentError::storage)?;
            Ok(victims.len() as u64)
        })
        .await
        .map_err(|e| IntentError::Join(e.to_string()))?
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::{Actor, ActorRole};
    use redb::Database;
    use tempfile::tempdir;
    use uuid::Uuid;

    async fn open_store() -> (RunnerIntentStore, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("intents.redb");
        let db = Arc::new(
            tokio::task::spawn_blocking(move || Database::create(&path))
                .await
                .unwrap()
                .unwrap(),
        );
        let store = RunnerIntentStore::open(db).unwrap();
        (store, dir)
    }

    fn intent(run_id: &str, workflow: &str, namespace: &str) -> RunnerIntent {
        RunnerIntent::new(
            run_id,
            workflow,
            serde_json::json!({"x": 1}),
            namespace,
            vec!["*".to_string()],
            Actor {
                id: "test".into(),
                role: ActorRole::User,
            },
            "sess_test",
        )
    }

    #[tokio::test]
    async fn insert_and_get_roundtrip() {
        let (store, _dir) = open_store().await;
        let id = Uuid::new_v4().to_string();
        store
            .insert(intent(&id, "w", "konf:test:a"))
            .await
            .unwrap();
        let got = store.get(&id).await.unwrap().unwrap();
        assert_eq!(got.workflow, "w");
        assert_eq!(got.namespace, "konf:test:a");
        assert!(got.terminal.is_none());
    }

    #[tokio::test]
    async fn list_unterminated_filters_terminal() {
        let (store, _dir) = open_store().await;
        let live_id = Uuid::new_v4().to_string();
        let done_id = Uuid::new_v4().to_string();
        store
            .insert(intent(&live_id, "live", "konf:test:a"))
            .await
            .unwrap();
        store
            .insert(intent(&done_id, "done", "konf:test:a"))
            .await
            .unwrap();
        store
            .mark_terminal(&done_id, TerminalStatus::Succeeded)
            .await
            .unwrap();

        let unterminated = store.list_unterminated().await.unwrap();
        assert_eq!(unterminated.len(), 1);
        assert_eq!(unterminated[0].run_id, live_id);
    }

    #[tokio::test]
    async fn list_by_namespace_filters_prefix() {
        let (store, _dir) = open_store().await;
        store
            .insert(intent(&Uuid::new_v4().to_string(), "w1", "konf:a:1"))
            .await
            .unwrap();
        store
            .insert(intent(&Uuid::new_v4().to_string(), "w2", "konf:b:1"))
            .await
            .unwrap();
        let a = store.list_by_namespace("konf:a", false).await.unwrap();
        let b = store.list_by_namespace("konf:b", false).await.unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].workflow, "w1");
        assert_eq!(b[0].workflow, "w2");
    }

    #[tokio::test]
    async fn gc_deletes_terminal_entries_past_cutoff() {
        let (store, _dir) = open_store().await;
        let old_id = Uuid::new_v4().to_string();
        let fresh_id = Uuid::new_v4().to_string();
        let live_id = Uuid::new_v4().to_string();

        // Insert two terminal entries and one live entry.
        let mut old = intent(&old_id, "old", "konf:test:a");
        old.spawned_at = Utc::now() - chrono::Duration::days(30);
        store.insert(old).await.unwrap();
        store
            .mark_terminal(&old_id, TerminalStatus::Succeeded)
            .await
            .unwrap();

        store
            .insert(intent(&fresh_id, "fresh", "konf:test:a"))
            .await
            .unwrap();
        store
            .mark_terminal(&fresh_id, TerminalStatus::Succeeded)
            .await
            .unwrap();

        store
            .insert(intent(&live_id, "live", "konf:test:a"))
            .await
            .unwrap();

        // GC entries older than 7 days ago.
        let cutoff = Utc::now() - chrono::Duration::days(7);
        let deleted = store.gc(cutoff).await.unwrap();
        assert_eq!(deleted, 1);

        assert!(store.get(&old_id).await.unwrap().is_none());
        assert!(store.get(&fresh_id).await.unwrap().is_some());
        assert!(store.get(&live_id).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn mark_terminal_is_noop_for_missing_id() {
        let (store, _dir) = open_store().await;
        // Should not error.
        store
            .mark_terminal("nonexistent", TerminalStatus::Succeeded)
            .await
            .unwrap();
    }
}

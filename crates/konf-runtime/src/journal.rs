//! Event journal — append-only Postgres log for audit and monitoring.
//!
//! NOT for replay — for audit trails, debugging, and billing.
//! The process table (in-memory) tracks live state.

use chrono::{DateTime, Utc};
use serde_json::Value;
use sqlx::PgPool;
use tracing::debug;

use crate::error::{RunId, RuntimeError};

/// A journal entry recording a runtime event.
#[derive(Debug, Clone)]
pub struct JournalEntry {
    pub run_id: RunId,
    pub session_id: String,
    pub namespace: String,
    pub event_type: String,
    pub payload: Value,
}

/// Append-only event journal backed by Postgres.
pub struct EventJournal {
    pool: PgPool,
}

impl EventJournal {
    /// Create a new journal, running migrations if needed.
    pub async fn new(pool: PgPool) -> Result<Self, RuntimeError> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS runtime_events (
                id BIGSERIAL PRIMARY KEY,
                run_id UUID NOT NULL,
                session_id TEXT NOT NULL,
                namespace TEXT NOT NULL,
                event_type TEXT NOT NULL,
                payload JSONB NOT NULL DEFAULT '{}',
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW()
            )
            "#,
        )
        .execute(&pool)
        .await?;

        sqlx::query("CREATE INDEX IF NOT EXISTS idx_runtime_events_run ON runtime_events (run_id)")
            .execute(&pool)
            .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_runtime_events_session ON runtime_events (session_id, created_at DESC)",
        )
        .execute(&pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_runtime_events_namespace ON runtime_events (namespace, created_at DESC)",
        )
        .execute(&pool)
        .await?;

        Ok(Self { pool })
    }

    /// Append a journal entry.
    pub async fn append(&self, entry: JournalEntry) -> Result<i64, RuntimeError> {
        debug!(
            run_id = %entry.run_id,
            event_type = %entry.event_type,
            "journal.append"
        );
        let row = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO runtime_events (run_id, session_id, namespace, event_type, payload)
            VALUES ($1, $2, $3, $4, $5)
            RETURNING id
            "#,
        )
        .bind(entry.run_id)
        .bind(&entry.session_id)
        .bind(&entry.namespace)
        .bind(&entry.event_type)
        .bind(&entry.payload)
        .fetch_one(&self.pool)
        .await?;

        Ok(row)
    }

    /// Query journal entries for a specific run.
    pub async fn query_by_run(&self, run_id: RunId) -> Result<Vec<JournalRow>, RuntimeError> {
        let rows = sqlx::query_as::<_, JournalRow>(
            "SELECT id, run_id, session_id, namespace, event_type, payload, created_at FROM runtime_events WHERE run_id = $1 ORDER BY id",
        )
        .bind(run_id)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Query journal entries for a session.
    pub async fn query_by_session(
        &self,
        session_id: &str,
        limit: i64,
    ) -> Result<Vec<JournalRow>, RuntimeError> {
        let rows = sqlx::query_as::<_, JournalRow>(
            "SELECT id, run_id, session_id, namespace, event_type, payload, created_at FROM runtime_events WHERE session_id = $1 ORDER BY created_at DESC LIMIT $2",
        )
        .bind(session_id)
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Query the most recent journal events (for admin dashboard).
    pub async fn recent(&self, limit: i64) -> Result<Vec<JournalRow>, RuntimeError> {
        let rows = sqlx::query_as::<_, JournalRow>(
            r#"
            SELECT id, run_id, session_id, namespace, event_type, payload, created_at
            FROM runtime_events
            ORDER BY created_at DESC
            LIMIT $1
            "#,
        )
        .bind(limit)
        .fetch_all(&self.pool)
        .await?;

        Ok(rows)
    }

    /// Reconcile zombie workflows on startup.
    /// Finds runs with a "started" event but no terminal event and marks them as failed.
    pub async fn reconcile_zombies(&self) -> Result<u64, RuntimeError> {
        let result = sqlx::query(
            r#"
            INSERT INTO runtime_events (run_id, session_id, namespace, event_type, payload)
            SELECT
                e.run_id,
                e.session_id,
                e.namespace,
                'workflow_failed',
                jsonb_build_object('error', 'System restart — workflow was interrupted', 'reconciled', true)
            FROM runtime_events e
            WHERE e.event_type = 'workflow_started'
              AND NOT EXISTS (
                SELECT 1 FROM runtime_events t
                WHERE t.run_id = e.run_id
                  AND t.event_type IN ('workflow_completed', 'workflow_failed', 'workflow_cancelled')
              )
            "#,
        )
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected())
    }
}

/// A row from the runtime_events table.
#[derive(Debug, Clone, serde::Serialize, sqlx::FromRow)]
pub struct JournalRow {
    pub id: i64,
    pub run_id: RunId,
    pub session_id: String,
    pub namespace: String,
    pub event_type: String,
    pub payload: Value,
    pub created_at: DateTime<Utc>,
}

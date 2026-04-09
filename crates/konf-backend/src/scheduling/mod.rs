//! Job scheduling — Postgres-backed job queue via apalis.
//!
//! Supports:
//! - Cron jobs (from schedules.yaml)
//! - One-off delayed jobs (reminders, debounced extraction)
//! - FOR UPDATE SKIP LOCKED for multi-worker safety

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::PgPool;
use tracing::{info, warn};

use konf_runtime::scope::{Actor, ActorRole, CapabilityGrant, ExecutionScope, ResourceLimits};

/// A scheduled workflow job.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowJob {
    pub workflow_yaml: String,
    pub input: Value,
    pub namespace: String,
    pub session_id: String,
    pub capabilities: Vec<CapabilityGrant>,
}

/// Configuration for a cron job from schedules.yaml.
#[derive(Debug, Clone, Deserialize)]
#[allow(dead_code)] // Used when project config loading is wired
pub struct CronJobConfig {
    pub name: String,
    pub schedule: String,
    pub workflow: String,
}

/// Scheduler manages recurring and one-off jobs.
pub struct Scheduler {
    pool: PgPool,
    runtime: Arc<konf_runtime::Runtime>,
}

impl Scheduler {
    pub fn new(pool: PgPool, runtime: Arc<konf_runtime::Runtime>) -> Self {
        Self { pool, runtime }
    }

    /// Schedule a one-off job to run at a specific time.
    #[allow(dead_code)] // Used by API endpoints for reminders, debounced extraction
    pub async fn schedule_at(
        &self,
        job: WorkflowJob,
        run_at: chrono::DateTime<chrono::Utc>,
    ) -> Result<i64, sqlx::Error> {
        let payload = serde_json::to_value(&job)
            .map_err(|e| sqlx::Error::Protocol(format!("Job serialization failed: {e}")))?;

        let id = sqlx::query_scalar::<_, i64>(
            r#"
            INSERT INTO scheduled_jobs (job_type, payload, run_at, status)
            VALUES ('workflow', $1, $2, 'pending')
            RETURNING id
            "#,
        )
        .bind(payload)
        .bind(run_at)
        .fetch_one(&self.pool)
        .await?;

        info!(job_id = id, run_at = %run_at, "Job scheduled");
        Ok(id)
    }

    /// Poll for ready jobs and execute them.
    /// Uses FOR UPDATE SKIP LOCKED for multi-worker safety.
    pub async fn poll_and_execute(&self) -> Result<usize, anyhow::Error> {
        let jobs = sqlx::query_as::<_, JobRow>(
            r#"
            UPDATE scheduled_jobs
            SET status = 'running', claimed_at = NOW()
            WHERE id IN (
                SELECT id FROM scheduled_jobs
                WHERE status = 'pending' AND run_at <= NOW()
                ORDER BY run_at
                FOR UPDATE SKIP LOCKED
                LIMIT 10
            )
            RETURNING id, job_type, payload, run_at
            "#,
        )
        .fetch_all(&self.pool)
        .await?;

        let count = jobs.len();
        for job in jobs {
            if let Err(e) = self.execute_job(&job).await {
                warn!(job_id = job.id, error = %e, "Job execution failed");
                if let Err(db_err) = sqlx::query(
                    "UPDATE scheduled_jobs SET status = 'failed', error = $1 WHERE id = $2",
                )
                .bind(e.to_string())
                .bind(job.id)
                .execute(&self.pool)
                .await {
                    tracing::error!(job_id = job.id, error = %db_err, "Failed to update job status to 'failed'");
                }
            } else if let Err(db_err) = sqlx::query(
                    "UPDATE scheduled_jobs SET status = 'completed', completed_at = NOW() WHERE id = $1",
                )
                .bind(job.id)
                .execute(&self.pool)
                .await {
                    tracing::error!(job_id = job.id, error = %db_err, "Failed to update job status to 'completed'");
                }
        }

        Ok(count)
    }

    async fn execute_job(&self, job: &JobRow) -> Result<(), anyhow::Error> {
        let workflow_job: WorkflowJob = serde_json::from_value(job.payload.clone())?;

        let workflow = self.runtime.parse_yaml(&workflow_job.workflow_yaml)?;

        let scope = ExecutionScope {
            namespace: workflow_job.namespace,
            capabilities: workflow_job.capabilities,
            limits: ResourceLimits::default(),
            actor: Actor {
                id: "scheduler".into(),
                role: ActorRole::System,
            },
            depth: 0,
        };

        self.runtime
            .run(
                &workflow,
                workflow_job.input,
                scope,
                workflow_job.session_id,
            )
            .await?;

        Ok(())
    }

    /// Start the background polling loop.
    pub fn start_polling(self: Arc<Self>, interval_secs: u64) {
        tokio::spawn(async move {
            loop {
                match self.poll_and_execute().await {
                    Ok(count) if count > 0 => {
                        info!(jobs = count, "Executed scheduled jobs");
                    }
                    Err(e) => {
                        warn!(error = %e, "Job polling error");
                    }
                    _ => {}
                }
                tokio::time::sleep(std::time::Duration::from_secs(interval_secs)).await;
            }
        });
    }

    /// Ensure the scheduled_jobs table exists.
    pub async fn migrate(&self) -> Result<(), sqlx::Error> {
        sqlx::query(
            r#"
            CREATE TABLE IF NOT EXISTS scheduled_jobs (
                id BIGSERIAL PRIMARY KEY,
                job_type TEXT NOT NULL DEFAULT 'workflow',
                payload JSONB NOT NULL DEFAULT '{}',
                run_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                cron TEXT,
                status TEXT NOT NULL DEFAULT 'pending',
                created_at TIMESTAMPTZ NOT NULL DEFAULT NOW(),
                claimed_at TIMESTAMPTZ,
                completed_at TIMESTAMPTZ,
                error TEXT
            )
            "#,
        )
        .execute(&self.pool)
        .await?;

        sqlx::query(
            "CREATE INDEX IF NOT EXISTS idx_jobs_pending ON scheduled_jobs (run_at) WHERE status = 'pending'",
        )
        .execute(&self.pool)
        .await?;

        Ok(())
    }
}

#[derive(sqlx::FromRow)]
#[allow(dead_code)] // Fields read by sqlx FromRow derive
struct JobRow {
    id: i64,
    job_type: String,
    payload: Value,
    run_at: chrono::DateTime<chrono::Utc>,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_workflow_job_serialization() {
        let job = WorkflowJob {
            workflow_yaml: "workflow: test\nnodes:\n  s1:\n    do: echo\n    return: true".into(),
            input: serde_json::json!({"msg": "hello"}),
            namespace: "konf:test:user_1".into(),
            session_id: "sess_1".into(),
            capabilities: vec![CapabilityGrant::new("*")],
        };

        let json = serde_json::to_value(&job).unwrap();
        assert!(json.get("workflow_yaml").is_some());
        assert!(json.get("namespace").is_some());

        let deserialized: WorkflowJob = serde_json::from_value(json).unwrap();
        assert_eq!(deserialized.namespace, "konf:test:user_1");
    }

    #[test]
    fn test_cron_config_deserialization() {
        let config: CronJobConfig = serde_json::from_value(serde_json::json!({
            "name": "nightly-synthesis",
            "schedule": "0 3 * * *",
            "workflow": "workflows/synthesis.yaml"
        }))
        .unwrap();

        assert_eq!(config.name, "nightly-synthesis");
        assert_eq!(config.schedule, "0 3 * * *");
    }
}

//! Durable scheduler — persistent timers backed by redb.
//!
//! Replaces two older mechanisms:
//!
//! - The dead `konf-backend/src/scheduling/` Postgres module (never wired).
//! - The ephemeral `schedule:create` tokio-timer in
//!   `konf-init/src/schedule.rs` (non-durable; lost on restart).
//!
//! # Data model
//!
//! One primary redb table and one secondary index:
//!
//! ```text
//! timers       : (fire_at_ms: u64, job_id: [u8;16]) -> postcard(TimerRecord)
//! timers_by_id : [u8;16] job_id                     -> u64 fire_at_ms
//! ```
//!
//! The primary key encodes the fire time so polling is `timers.range(..now)`.
//! The secondary index lets [`RedbScheduler::cancel`] locate a timer by id
//! without scanning the whole table.
//!
//! # Firing order
//!
//! When a timer is due the poll loop:
//!
//! 1. Resolves `workflow:<id>` in the live engine registry.
//! 2. Builds an [`ExecutionScope`] from the stored record and calls
//!    `runtime.start(...)` (fire-and-forget).
//! 3. Deletes the `(old_fire_at, id)` key.
//! 4. For [`TimerMode::Fixed`] / [`TimerMode::Cron`], inserts a new key with
//!    the next fire time.
//!
//! If konf-backend crashes between steps 2 and 4 the same timer fires again
//! on the next poll — we accept at-least-once semantics, workflow authors
//! are responsible for idempotency. See `docs/architecture/durability.md`.
//!
//! # Missing workflow tolerance
//!
//! If step 1 fails because the workflow is no longer in the registry (user
//! deleted the file), the scheduler logs a warning and leaves the timer in
//! place. When the workflow is re-registered the next fire succeeds.

use std::sync::{Arc, Weak};
use std::time::Duration;

use chrono::{DateTime, TimeZone, Utc};
use cron::Schedule as CronSchedule;
use redb::{
    Database, ReadableDatabase, ReadableTable, ReadableTableMetadata, TableDefinition,
};
use serde::{Deserialize, Serialize};
use tokio_util::sync::CancellationToken;
use tracing::{debug, info, warn};
use uuid::Uuid;

use crate::runtime::Runtime;
use crate::scope::Actor;

// ---------------------------------------------------------------------------
// Public types
// ---------------------------------------------------------------------------

/// Opaque handle returned from [`RedbScheduler::schedule_once`] and friends.
pub type JobId = Uuid;

/// Minimum delay for [`TimerMode::Fixed`]. Prevents hot-spin loops.
pub const MIN_FIXED_DELAY_MS: u64 = 1_000;

/// Maximum delay for [`TimerMode::Fixed`]. Prevents unbounded accumulation.
/// Cron timers have no upper bound; a yearly cron is fine.
pub const MAX_FIXED_DELAY_MS: u64 = 7 * 24 * 3600 * 1_000;

/// Errors raised by [`RedbScheduler`] operations.
#[derive(Debug, thiserror::Error)]
pub enum SchedulerError {
    /// Backend storage error (IO, transaction, corruption).
    #[error("storage: {0}")]
    Storage(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Input could not be serialized or deserialized.
    #[error("serialization: {0}")]
    Serialization(#[source] Box<dyn std::error::Error + Send + Sync>),

    /// Invalid input (out-of-range delay, malformed cron, empty workflow).
    #[error("invalid input: {0}")]
    Invalid(String),

    /// Job id not found.
    #[error("job not found: {0}")]
    NotFound(JobId),

    /// Internal blocking-task join failure.
    #[error("task join: {0}")]
    Join(String),
}

impl SchedulerError {
    fn storage<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::Storage(Box::new(e))
    }
    fn serialization<E: std::error::Error + Send + Sync + 'static>(e: E) -> Self {
        Self::Serialization(Box::new(e))
    }
}

/// How a timer re-fires (or doesn't).
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub enum TimerMode {
    /// Fire once at the scheduled time, then delete.
    Once,
    /// Fire every `delay_ms`. The next fire is scheduled relative to the
    /// previous fire's completion.
    Fixed { delay_ms: u64 },
    /// Fire according to a cron expression (re-parsed at each fire to
    /// compute the next time).
    Cron { expr: String },
}

/// Everything the scheduler needs to re-launch a workflow at fire time.
///
/// Public API uses `serde_json::Value` for `input`. On-disk, we serialize
/// with postcard which doesn't support `Value` directly, so we round-trip
/// the input through a JSON string via [`StoredTimer`].
#[derive(Debug, Clone)]
pub struct TimerRecord {
    /// Stable job id (set by [`RedbScheduler::schedule_once`] and friends).
    pub job_id: JobId,
    /// Workflow id to fire. Resolved from `workflow:<id>` in the live
    /// engine registry at each fire — **not** snapshotted.
    pub workflow: String,
    /// Input payload. Snapshotted at schedule time and reused on every fire.
    pub input: serde_json::Value,
    /// Execution namespace (tenant scope).
    pub namespace: String,
    /// Capability patterns granted to each fire.
    pub capabilities: Vec<String>,
    /// Actor attribution.
    pub actor: Actor,
    /// How the timer re-fires (or doesn't).
    pub mode: TimerMode,
    /// When the timer was first scheduled.
    pub created_at: DateTime<Utc>,
    /// Who scheduled it (free-text, for audit).
    pub created_by: String,
}

/// On-disk (postcard-friendly) shape of a [`TimerRecord`].
///
/// `input` is stored as a JSON-encoded string because `serde_json::Value`
/// can't be serialized through postcard's restricted format.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredTimer {
    job_id_bytes: [u8; 16],
    workflow: String,
    input_json: String,
    namespace: String,
    capabilities: Vec<String>,
    actor: Actor,
    mode: TimerMode,
    created_at_micros: i64,
    created_by: String,
}

impl StoredTimer {
    fn from_record(record: &TimerRecord) -> Result<Self, SchedulerError> {
        Ok(Self {
            job_id_bytes: *record.job_id.as_bytes(),
            workflow: record.workflow.clone(),
            input_json: serde_json::to_string(&record.input).map_err(SchedulerError::serialization)?,
            namespace: record.namespace.clone(),
            capabilities: record.capabilities.clone(),
            actor: record.actor.clone(),
            mode: record.mode.clone(),
            created_at_micros: record.created_at.timestamp_micros(),
            created_by: record.created_by.clone(),
        })
    }

    fn into_record(self) -> Result<TimerRecord, SchedulerError> {
        let input: serde_json::Value =
            serde_json::from_str(&self.input_json).map_err(SchedulerError::serialization)?;
        let created_at = Utc
            .timestamp_micros(self.created_at_micros)
            .single()
            .unwrap_or_else(Utc::now);
        Ok(TimerRecord {
            job_id: Uuid::from_bytes(self.job_id_bytes),
            workflow: self.workflow,
            input,
            namespace: self.namespace,
            capabilities: self.capabilities,
            actor: self.actor,
            mode: self.mode,
            created_at,
            created_by: self.created_by,
        })
    }
}

/// Summary view returned by [`RedbScheduler::list`].
#[derive(Debug, Clone, Serialize)]
pub struct JobSummary {
    pub job_id: JobId,
    pub workflow: String,
    pub namespace: String,
    pub next_fire_at: DateTime<Utc>,
    pub mode: TimerMode,
    pub created_at: DateTime<Utc>,
    pub created_by: String,
}

// ---------------------------------------------------------------------------
// Table definitions
// ---------------------------------------------------------------------------

/// Primary: `(fire_at_ms, job_id_bytes) -> postcard(TimerRecord)`.
const TIMERS: TableDefinition<(u64, &[u8]), &[u8]> = TableDefinition::new("scheduler_timers");

/// Secondary: `job_id_bytes -> fire_at_ms`.
const TIMERS_BY_ID: TableDefinition<&[u8], u64> =
    TableDefinition::new("scheduler_timers_by_id");

// ---------------------------------------------------------------------------
// RedbScheduler
// ---------------------------------------------------------------------------

/// Durable scheduler backed by redb. Cheap to clone.
#[derive(Clone)]
pub struct RedbScheduler {
    db: Arc<Database>,
    runtime: Weak<Runtime>,
    poll_interval: Duration,
    shutdown: CancellationToken,
}

impl RedbScheduler {
    /// Construct a scheduler over a redb database and a weak runtime handle.
    ///
    /// The caller is expected to:
    ///
    /// 1. Create storage + runtime.
    /// 2. Construct the scheduler passing `Arc::downgrade(&runtime)`.
    /// 3. Install the scheduler into the runtime via
    ///    [`Runtime::install_scheduler`].
    /// 4. Call [`RedbScheduler::start_polling`] once.
    pub fn new(db: Arc<Database>, runtime: Weak<Runtime>) -> Result<Self, SchedulerError> {
        // Materialize the tables so later read transactions don't fail with
        // "table does not exist".
        let write = db.begin_write().map_err(SchedulerError::storage)?;
        {
            let _ = write.open_table(TIMERS).map_err(SchedulerError::storage)?;
            let _ = write
                .open_table(TIMERS_BY_ID)
                .map_err(SchedulerError::storage)?;
        }
        write.commit().map_err(SchedulerError::storage)?;
        Ok(Self {
            db,
            runtime,
            poll_interval: Duration::from_secs(1),
            shutdown: CancellationToken::new(),
        })
    }

    /// Override the polling interval. Default 1 second.
    pub fn with_poll_interval(mut self, interval: Duration) -> Self {
        self.poll_interval = interval;
        self
    }

    /// Schedule a one-shot timer at `run_at`. Returns the job id.
    pub async fn schedule_once(
        &self,
        mut record: TimerRecord,
        run_at: DateTime<Utc>,
    ) -> Result<JobId, SchedulerError> {
        self.validate_record(&record)?;
        record.mode = TimerMode::Once;
        self.insert_new(record, run_at).await
    }

    /// Schedule a fixed-delay timer that re-fires every `delay_ms`.
    pub async fn schedule_fixed(
        &self,
        mut record: TimerRecord,
        delay_ms: u64,
    ) -> Result<JobId, SchedulerError> {
        if !(MIN_FIXED_DELAY_MS..=MAX_FIXED_DELAY_MS).contains(&delay_ms) {
            return Err(SchedulerError::Invalid(format!(
                "delay_ms={delay_ms} out of range ({MIN_FIXED_DELAY_MS}–{MAX_FIXED_DELAY_MS})"
            )));
        }
        self.validate_record(&record)?;
        record.mode = TimerMode::Fixed { delay_ms };
        let run_at =
            Utc::now() + chrono::Duration::milliseconds(delay_ms as i64);
        self.insert_new(record, run_at).await
    }

    /// Schedule a cron-expression timer.
    pub async fn schedule_cron(
        &self,
        mut record: TimerRecord,
        expr: String,
    ) -> Result<JobId, SchedulerError> {
        self.validate_record(&record)?;
        let schedule: CronSchedule = expr
            .parse()
            .map_err(|e: cron::error::Error| SchedulerError::Invalid(format!("cron: {e}")))?;
        let run_at = schedule
            .upcoming(Utc)
            .next()
            .ok_or_else(|| SchedulerError::Invalid("cron expression has no upcoming fire time".into()))?;
        record.mode = TimerMode::Cron { expr };
        self.insert_new(record, run_at).await
    }

    /// Cancel a timer by id. Returns true if the id existed.
    pub async fn cancel(&self, id: JobId) -> Result<bool, SchedulerError> {
        let db = self.db.clone();
        let id_bytes = *id.as_bytes();
        tokio::task::spawn_blocking(move || -> Result<bool, SchedulerError> {
            let write = db.begin_write().map_err(SchedulerError::storage)?;
            let existed = {
                let mut by_id = write
                    .open_table(TIMERS_BY_ID)
                    .map_err(SchedulerError::storage)?;
                let fire_at = by_id
                    .remove(id_bytes.as_slice())
                    .map_err(SchedulerError::storage)?
                    .map(|g| g.value());
                if let Some(fire_at) = fire_at {
                    let mut timers = write.open_table(TIMERS).map_err(SchedulerError::storage)?;
                    timers
                        .remove(&(fire_at, id_bytes.as_slice()))
                        .map_err(SchedulerError::storage)?;
                    true
                } else {
                    false
                }
            };
            write.commit().map_err(SchedulerError::storage)?;
            Ok(existed)
        })
        .await
        .map_err(|e| SchedulerError::Join(e.to_string()))?
    }

    /// List all scheduled timers, optionally filtered by namespace prefix.
    pub async fn list(
        &self,
        namespace_prefix: Option<String>,
    ) -> Result<Vec<JobSummary>, SchedulerError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<JobSummary>, SchedulerError> {
            let read = db.begin_read().map_err(SchedulerError::storage)?;
            let timers = read.open_table(TIMERS).map_err(SchedulerError::storage)?;
            let mut out = Vec::new();
            for pair in timers.iter().map_err(SchedulerError::storage)? {
                let (k_guard, v_guard) = pair.map_err(SchedulerError::storage)?;
                let (fire_at_ms, _id_bytes) = k_guard.value();
                let stored: StoredTimer =
                    postcard::from_bytes(v_guard.value()).map_err(SchedulerError::serialization)?;
                if let Some(ref prefix) = namespace_prefix {
                    if !stored.namespace.starts_with(prefix.as_str()) {
                        continue;
                    }
                }
                let next_fire_at = Utc
                    .timestamp_millis_opt(fire_at_ms as i64)
                    .single()
                    .unwrap_or_else(Utc::now);
                let record = stored.into_record()?;
                out.push(JobSummary {
                    job_id: record.job_id,
                    workflow: record.workflow.clone(),
                    namespace: record.namespace.clone(),
                    next_fire_at,
                    mode: record.mode.clone(),
                    created_at: record.created_at,
                    created_by: record.created_by.clone(),
                });
            }
            Ok(out)
        })
        .await
        .map_err(|e| SchedulerError::Join(e.to_string()))?
    }

    /// Return how many timers are currently persisted.
    pub async fn len(&self) -> Result<usize, SchedulerError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<usize, SchedulerError> {
            let read = db.begin_read().map_err(SchedulerError::storage)?;
            let timers = read.open_table(TIMERS).map_err(SchedulerError::storage)?;
            timers
                .len()
                .map(|n| n as usize)
                .map_err(SchedulerError::storage)
        })
        .await
        .map_err(|e| SchedulerError::Join(e.to_string()))?
    }

    /// Return true if no timers are persisted.
    pub async fn is_empty(&self) -> Result<bool, SchedulerError> {
        Ok(self.len().await? == 0)
    }

    /// Start the background polling loop. Call once per scheduler.
    ///
    /// The loop ticks on [`RedbScheduler::poll_interval`] intervals. Each
    /// tick reads all due timers in a single read transaction, then
    /// processes them sequentially (fire + reschedule under one write
    /// transaction per timer).
    pub fn start_polling(self: &Arc<Self>) {
        let this = self.clone();
        tokio::spawn(async move {
            debug!("scheduler polling loop started");
            loop {
                tokio::select! {
                    biased;
                    _ = this.shutdown.cancelled() => {
                        info!("scheduler polling loop shutting down");
                        return;
                    }
                    _ = tokio::time::sleep(this.poll_interval) => {}
                }
                if let Err(e) = this.tick().await {
                    warn!(error = %e, "scheduler tick failed");
                }
            }
        });
    }

    /// Request the polling loop stop.
    pub fn shutdown(&self) {
        self.shutdown.cancel();
    }

    // ---- internals ----

    fn validate_record(&self, record: &TimerRecord) -> Result<(), SchedulerError> {
        if record.workflow.is_empty() {
            return Err(SchedulerError::Invalid("workflow id is empty".into()));
        }
        if record.namespace.is_empty() {
            return Err(SchedulerError::Invalid("namespace is empty".into()));
        }
        Ok(())
    }

    async fn insert_new(
        &self,
        record: TimerRecord,
        run_at: DateTime<Utc>,
    ) -> Result<JobId, SchedulerError> {
        let db = self.db.clone();
        let fire_at_ms = run_at.timestamp_millis().max(0) as u64;
        let id = record.job_id;
        let id_bytes = *id.as_bytes();
        let stored = StoredTimer::from_record(&record)?;
        let bytes = postcard::to_allocvec(&stored).map_err(SchedulerError::serialization)?;

        tokio::task::spawn_blocking(move || -> Result<(), SchedulerError> {
            let write = db.begin_write().map_err(SchedulerError::storage)?;
            {
                let mut timers = write.open_table(TIMERS).map_err(SchedulerError::storage)?;
                timers
                    .insert(&(fire_at_ms, id_bytes.as_slice()), bytes.as_slice())
                    .map_err(SchedulerError::storage)?;
            }
            {
                let mut by_id = write
                    .open_table(TIMERS_BY_ID)
                    .map_err(SchedulerError::storage)?;
                by_id
                    .insert(id_bytes.as_slice(), fire_at_ms)
                    .map_err(SchedulerError::storage)?;
            }
            write.commit().map_err(SchedulerError::storage)?;
            Ok(())
        })
        .await
        .map_err(|e| SchedulerError::Join(e.to_string()))??;
        Ok(id)
    }

    /// Run one poll tick: collect due timers, fire and reschedule each.
    async fn tick(&self) -> Result<(), SchedulerError> {
        let now_ms = Utc::now().timestamp_millis().max(0) as u64;
        let due = self.collect_due(now_ms).await?;
        for (old_fire_at, record) in due {
            self.fire_and_reschedule(old_fire_at, record).await?;
        }
        Ok(())
    }

    /// Read all timers with `fire_at <= now` in one read transaction. Sorted
    /// by fire time (ascending).
    async fn collect_due(
        &self,
        now_ms: u64,
    ) -> Result<Vec<(u64, TimerRecord)>, SchedulerError> {
        let db = self.db.clone();
        tokio::task::spawn_blocking(move || -> Result<Vec<(u64, TimerRecord)>, SchedulerError> {
            let read = db.begin_read().map_err(SchedulerError::storage)?;
            let timers = read.open_table(TIMERS).map_err(SchedulerError::storage)?;
            let lower: (u64, &[u8]) = (0, &[]);
            let upper: (u64, &[u8]) = (now_ms.saturating_add(1), &[]);
            let iter = timers
                .range(lower..upper)
                .map_err(SchedulerError::storage)?;
            let mut out = Vec::new();
            for pair in iter {
                let (k_guard, v_guard) = pair.map_err(SchedulerError::storage)?;
                let (fire_at, _id_bytes) = k_guard.value();
                let stored: StoredTimer =
                    postcard::from_bytes(v_guard.value()).map_err(SchedulerError::serialization)?;
                out.push((fire_at, stored.into_record()?));
            }
            Ok(out)
        })
        .await
        .map_err(|e| SchedulerError::Join(e.to_string()))?
    }

    /// Fire the workflow (fire-and-forget) and then reschedule or delete
    /// the timer.
    async fn fire_and_reschedule(
        &self,
        old_fire_at: u64,
        record: TimerRecord,
    ) -> Result<(), SchedulerError> {
        let Some(runtime) = self.runtime.upgrade() else {
            // Runtime has been dropped; scheduler will shut down next tick.
            return Ok(());
        };

        let tool_name = format!("workflow:{}", record.workflow);
        let Some(tool) = runtime.engine().registry().get(&tool_name) else {
            warn!(
                workflow = %record.workflow,
                job_id = %record.job_id,
                "scheduler fire: workflow not in registry — leaving timer in place for future retry"
            );
            runtime.event_bus().emit(crate::event_bus::RunEvent::ScheduleFailed {
                job_id: record.job_id,
                workflow: record.workflow.clone(),
                reason: format!("workflow '{}' not found in registry", record.workflow),
            });
            return Ok(());
        };

        info!(
            workflow = %record.workflow,
            job_id = %record.job_id,
            namespace = %record.namespace,
            "scheduler fire"
        );

        runtime.event_bus().emit(crate::event_bus::RunEvent::ScheduleFired {
            job_id: record.job_id,
            workflow: record.workflow.clone(),
            namespace: record.namespace.clone(),
            fired_at: Utc::now(),
        });

        // Fire-and-forget invocation through the workflow tool. Individual
        // job failures are logged but do not block the poll loop.
        //
        // NOTE: `WorkflowTool::invoke` uses its registration-time default
        // scope rather than the ToolContext's namespace. The per-timer
        // `record.namespace` is therefore currently used only for listing
        // and audit, not enforcement. Per-timer scope enforcement is
        // tracked as multi-tenant hardening in the v2 plan's future work.
        let ctx = konflux::tool::ToolContext {
            capabilities: record.capabilities.clone(),
            workflow_id: "scheduler".into(),
            node_id: format!("scheduled_{}", record.workflow),
            metadata: std::collections::HashMap::from_iter([
                ("namespace".into(), serde_json::Value::String(record.namespace.clone())),
                ("session_id".into(), serde_json::Value::String(format!("scheduler:{}", record.job_id))),
                (
                    "actor_id".into(),
                    serde_json::Value::String(record.actor.id.clone()),
                ),
            ]),
        };
        let input = record.input.clone();
        let wf_id = record.workflow.clone();
        let job_id = record.job_id;
        tokio::spawn(async move {
            if let Err(e) = tool.invoke(input, &ctx).await {
                warn!(workflow = %wf_id, job_id = %job_id, error = %e, "scheduler fire failed");
            }
        });

        // Reschedule or delete the timer entry.
        let next_at = self.next_fire_after(&record.mode);
        self.reschedule(old_fire_at, record, next_at).await
    }

    /// Compute the next fire time for a timer mode (after a successful fire).
    fn next_fire_after(&self, mode: &TimerMode) -> Option<DateTime<Utc>> {
        match mode {
            TimerMode::Once => None,
            TimerMode::Fixed { delay_ms } => {
                Some(Utc::now() + chrono::Duration::milliseconds(*delay_ms as i64))
            }
            TimerMode::Cron { expr } => {
                let schedule: CronSchedule = match expr.parse() {
                    Ok(s) => s,
                    Err(e) => {
                        warn!(cron = %expr, error = %e, "cron parse failed during reschedule; dropping timer");
                        return None;
                    }
                };
                schedule.upcoming(Utc).next()
            }
        }
    }

    /// Apply a reschedule: delete the old `(old_fire_at, id)` entry and, if
    /// `next_at` is `Some`, insert a new entry.
    async fn reschedule(
        &self,
        old_fire_at: u64,
        record: TimerRecord,
        next_at: Option<DateTime<Utc>>,
    ) -> Result<(), SchedulerError> {
        let db = self.db.clone();
        let id_bytes = *record.job_id.as_bytes();
        let new_bytes = match next_at {
            Some(_) => {
                let stored = StoredTimer::from_record(&record)?;
                Some(postcard::to_allocvec(&stored).map_err(SchedulerError::serialization)?)
            }
            None => None,
        };
        let next_ms = next_at.map(|d| d.timestamp_millis().max(0) as u64);

        tokio::task::spawn_blocking(move || -> Result<(), SchedulerError> {
            let write = db.begin_write().map_err(SchedulerError::storage)?;
            {
                let mut timers = write.open_table(TIMERS).map_err(SchedulerError::storage)?;
                timers
                    .remove(&(old_fire_at, id_bytes.as_slice()))
                    .map_err(SchedulerError::storage)?;
                if let (Some(ms), Some(bytes)) = (next_ms, new_bytes.as_ref()) {
                    timers
                        .insert(&(ms, id_bytes.as_slice()), bytes.as_slice())
                        .map_err(SchedulerError::storage)?;
                }
            }
            {
                let mut by_id = write
                    .open_table(TIMERS_BY_ID)
                    .map_err(SchedulerError::storage)?;
                match next_ms {
                    Some(ms) => {
                        by_id
                            .insert(id_bytes.as_slice(), ms)
                            .map_err(SchedulerError::storage)?;
                    }
                    None => {
                        by_id
                            .remove(id_bytes.as_slice())
                            .map_err(SchedulerError::storage)?;
                    }
                }
            }
            write.commit().map_err(SchedulerError::storage)?;
            Ok(())
        })
        .await
        .map_err(|e| SchedulerError::Join(e.to_string()))?
    }
}

/// Helper to build a fresh [`TimerRecord`] for callers that don't want to
/// manage job id generation themselves.
pub fn new_record(
    workflow: impl Into<String>,
    input: serde_json::Value,
    namespace: impl Into<String>,
    capabilities: Vec<String>,
    actor: Actor,
    created_by: impl Into<String>,
) -> TimerRecord {
    TimerRecord {
        job_id: Uuid::new_v4(),
        workflow: workflow.into(),
        input,
        namespace: namespace.into(),
        capabilities,
        actor,
        mode: TimerMode::Once, // overridden by schedule_* methods
        created_at: Utc::now(),
        created_by: created_by.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::ActorRole;
    use crate::storage::{KonfStorage, Retention};
    use tempfile::tempdir;

    fn test_actor() -> Actor {
        Actor {
            id: "test_user".into(),
            role: ActorRole::User,
        }
    }

    async fn storage() -> (Arc<KonfStorage>, tempfile::TempDir) {
        let dir = tempdir().unwrap();
        let path = dir.path().join("konf.redb");
        let s = KonfStorage::open(&path, Retention::default()).await.unwrap();
        (Arc::new(s), dir)
    }

    fn record(workflow: &str, namespace: &str) -> TimerRecord {
        new_record(
            workflow.to_string(),
            serde_json::json!({"x": 1}),
            namespace.to_string(),
            vec!["*".to_string()],
            test_actor(),
            "test".to_string(),
        )
    }

    #[tokio::test]
    async fn schedule_once_persists_timer() {
        let (storage, _dir) = storage().await;
        let sched = Arc::new(RedbScheduler::new(storage.database(), Weak::new()).unwrap());
        let id = sched
            .schedule_once(record("morning_brief", "konf:test:a"), Utc::now() + chrono::Duration::hours(1))
            .await
            .unwrap();
        assert_eq!(sched.len().await.unwrap(), 1);
        let list = sched.list(None).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].job_id, id);
        assert_eq!(list[0].workflow, "morning_brief");
        assert_eq!(list[0].mode, TimerMode::Once);
    }

    #[tokio::test]
    async fn schedule_fixed_validates_bounds() {
        let (storage, _dir) = storage().await;
        let sched = Arc::new(RedbScheduler::new(storage.database(), Weak::new()).unwrap());
        let r = record("w", "konf:test:a");
        let err = sched
            .schedule_fixed(r.clone(), 100)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("out of range"));
        let err = sched
            .schedule_fixed(r, MAX_FIXED_DELAY_MS + 1)
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("out of range"));
    }

    #[tokio::test]
    async fn schedule_fixed_accepts_valid_delay() {
        let (storage, _dir) = storage().await;
        let sched = Arc::new(RedbScheduler::new(storage.database(), Weak::new()).unwrap());
        let id = sched
            .schedule_fixed(record("watcher", "konf:test:a"), 60_000)
            .await
            .unwrap();
        let list = sched.list(None).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].job_id, id);
        assert!(matches!(list[0].mode, TimerMode::Fixed { delay_ms: 60_000 }));
    }

    #[tokio::test]
    async fn schedule_cron_parses_and_persists() {
        let (storage, _dir) = storage().await;
        let sched = Arc::new(RedbScheduler::new(storage.database(), Weak::new()).unwrap());
        // cron crate uses 7-field format: sec min hour day month weekday year
        let id = sched
            .schedule_cron(record("morning_brief", "konf:test:a"), "0 0 8 * * * *".into())
            .await
            .unwrap();
        let list = sched.list(None).await.unwrap();
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].job_id, id);
        assert!(matches!(list[0].mode, TimerMode::Cron { .. }));
    }

    #[tokio::test]
    async fn schedule_cron_rejects_bad_expression() {
        let (storage, _dir) = storage().await;
        let sched = Arc::new(RedbScheduler::new(storage.database(), Weak::new()).unwrap());
        let err = sched
            .schedule_cron(record("w", "konf:test:a"), "not a cron".into())
            .await
            .unwrap_err()
            .to_string();
        assert!(err.contains("cron"));
    }

    #[tokio::test]
    async fn cancel_removes_timer() {
        let (storage, _dir) = storage().await;
        let sched = Arc::new(RedbScheduler::new(storage.database(), Weak::new()).unwrap());
        let id = sched
            .schedule_once(record("w", "konf:test:a"), Utc::now() + chrono::Duration::hours(1))
            .await
            .unwrap();
        assert!(sched.cancel(id).await.unwrap());
        assert_eq!(sched.len().await.unwrap(), 0);
        // Second cancel is a no-op
        assert!(!sched.cancel(id).await.unwrap());
    }

    #[tokio::test]
    async fn list_filters_by_namespace() {
        let (storage, _dir) = storage().await;
        let sched = Arc::new(RedbScheduler::new(storage.database(), Weak::new()).unwrap());
        let when = Utc::now() + chrono::Duration::hours(1);
        sched
            .schedule_once(record("w1", "konf:a:user_1"), when)
            .await
            .unwrap();
        sched
            .schedule_once(record("w2", "konf:b:user_2"), when)
            .await
            .unwrap();
        let a = sched.list(Some("konf:a".into())).await.unwrap();
        let b = sched.list(Some("konf:b".into())).await.unwrap();
        assert_eq!(a.len(), 1);
        assert_eq!(b.len(), 1);
        assert_eq!(a[0].workflow, "w1");
        assert_eq!(b[0].workflow, "w2");
    }

    #[tokio::test]
    async fn timers_persist_across_scheduler_instances() {
        let dir = tempdir().unwrap();
        let path = dir.path().join("konf.redb");

        // First instance: schedule one timer
        {
            let s = Arc::new(KonfStorage::open(&path, Retention::default()).await.unwrap());
            let sched = Arc::new(RedbScheduler::new(s.database(), Weak::new()).unwrap());
            sched
                .schedule_once(record("persistent", "konf:test:a"), Utc::now() + chrono::Duration::hours(1))
                .await
                .unwrap();
        }

        // Second instance over the same file: timer is still there
        {
            let s = Arc::new(KonfStorage::open(&path, Retention::default()).await.unwrap());
            let sched = Arc::new(RedbScheduler::new(s.database(), Weak::new()).unwrap());
            let list = sched.list(None).await.unwrap();
            assert_eq!(list.len(), 1);
            assert_eq!(list[0].workflow, "persistent");
        }
    }
}

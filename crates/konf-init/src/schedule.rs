//! `schedule:create` and `cancel:schedule` tools — thin shells over the
//! durable [`konf_runtime::RedbScheduler`].
//!
//! Input shape (unchanged from the pre-v2 ephemeral tool):
//!
//! ```json
//! { "workflow": "name", "delay_ms": 60000, "input": {...}, "repeat": false }
//! ```
//!
//! New in v2: `cron` as an alternative to `delay_ms`:
//!
//! ```json
//! { "workflow": "name", "cron": "0 0 8 * * * *", "input": {...} }
//! ```
//!
//! Both shapes persist a [`TimerRecord`] into redb so the schedule survives
//! restarts. On restart the scheduler replays any due entries immediately
//! (at-least-once semantics); workflow authors are responsible for
//! idempotency (see `docs/architecture/durability.md`).

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use konflux_substrate::envelope::Envelope;
use konflux_substrate::error::ToolError;
use konflux_substrate::tool::{Tool, ToolInfo};

use konf_runtime::scope::{Actor, ActorRole};
use konf_runtime::{JobId, Runtime, TimerRecord, MAX_FIXED_DELAY_MS, MIN_FIXED_DELAY_MS};

/// `schedule:create` — create a durable timer.
pub struct ScheduleTool {
    runtime: Arc<Runtime>,
}

impl ScheduleTool {
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl Tool for ScheduleTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "schedule:create".into(),
            description: format!(
                "Schedule a workflow to run durably (survives restarts). Returns immediately. \
                 One-shot: set delay_ms ({MIN_FIXED_DELAY_MS}–{MAX_FIXED_DELAY_MS}) with repeat: false. \
                 Repeating: set delay_ms with repeat: true, or set cron to a 7-field cron expression."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workflow": {
                        "type": "string",
                        "description": "Workflow id (resolved as workflow:<id> at each fire)"
                    },
                    "delay_ms": {
                        "type": "integer",
                        "description": format!("Milliseconds. Min: {MIN_FIXED_DELAY_MS}, Max: {MAX_FIXED_DELAY_MS}. Ignored when cron is set.")
                    },
                    "cron": {
                        "type": "string",
                        "description": "7-field cron expression (sec min hour day month weekday year). Mutually exclusive with delay_ms."
                    },
                    "input": {
                        "type": "object",
                        "description": "Input payload (snapshot at schedule time; reused on every fire)"
                    },
                    "repeat": {
                        "type": "boolean",
                        "description": "If true and delay_ms is set, re-fire every delay_ms after completion. Default: false."
                    }
                },
                "required": ["workflow"]
            }),
            output_schema: None,
            capabilities: vec!["schedule:create".into()],
            supports_streaming: false,
            annotations: Default::default(),
        }
    }

    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        let scheduler = self
            .runtime
            .scheduler()
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: "scheduler not available (no persistent storage configured)".into(),
                retryable: false,
            })?;

        let input = &env.payload;

        let workflow_id = input
            .get("workflow")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: "missing required field: workflow".into(),
                retryable: false,
            })?;

        // Fail fast: confirm the workflow exists now (re-resolved at fire time too).
        let tool_name = format!("workflow:{workflow_id}");
        if self.runtime.engine().registry().get(&tool_name).is_none() {
            return Err(ToolError::NotFound { tool_id: tool_name });
        }

        let workflow_input = input.get("input").cloned().unwrap_or_else(|| json!({}));
        let cron_expr = input
            .get("cron")
            .and_then(|v| v.as_str())
            .map(str::to_string);
        let delay_ms = input.get("delay_ms").and_then(|v| {
            v.as_u64()
                .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
        });
        let repeat = input
            .get("repeat")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        // Build the record. Namespace and actor come from the typed
        // envelope fields. The namespace binding should have been
        // injected by VirtualizedTool into the payload.
        let namespace = env.namespace.0.clone();
        let actor = Actor {
            id: env.actor_id.0.clone(),
            role: ActorRole::User,
        };

        let record = TimerRecord {
            job_id: JobId::new_v4(),
            workflow: workflow_id.to_string(),
            input: workflow_input,
            namespace,
            capabilities: env.capabilities.to_patterns(),
            actor,
            mode: konf_runtime::TimerMode::Once, // overridden by schedule_* methods
            created_at: chrono::Utc::now(),
            created_by: "schedule:create".into(),
        };

        let id = match (cron_expr, delay_ms) {
            (Some(expr), _) => scheduler
                .schedule_cron(record, expr)
                .await
                .map_err(scheduler_err)?,
            (None, Some(ms)) if repeat => scheduler
                .schedule_fixed(record, ms)
                .await
                .map_err(scheduler_err)?,
            (None, Some(ms)) => {
                if !(MIN_FIXED_DELAY_MS..=MAX_FIXED_DELAY_MS).contains(&ms) {
                    return Err(ToolError::ExecutionFailed {
                        message: format!(
                            "delay_ms={ms} out of range ({MIN_FIXED_DELAY_MS}–{MAX_FIXED_DELAY_MS})"
                        ),
                        retryable: false,
                    });
                }
                let run_at = chrono::Utc::now() + chrono::Duration::milliseconds(ms as i64);
                scheduler
                    .schedule_once(record, run_at)
                    .await
                    .map_err(scheduler_err)?
            }
            (None, None) => {
                return Err(ToolError::ExecutionFailed {
                    message: "must provide either delay_ms or cron".into(),
                    retryable: false,
                });
            }
        };

        info!(workflow = %workflow_id, schedule_id = %id, "schedule created");
        Ok(env.respond(json!({
            "scheduled": true,
            "workflow": workflow_id,
            "schedule_id": id.to_string(),
        })))
    }
}

/// `cancel:schedule` — cancel a durable timer by its schedule id.
pub struct CancelScheduleTool {
    runtime: Arc<Runtime>,
}

impl CancelScheduleTool {
    pub fn new(runtime: Arc<Runtime>) -> Self {
        Self { runtime }
    }
}

#[async_trait]
impl Tool for CancelScheduleTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "cancel:schedule".into(),
            description: "Cancel a scheduled workflow by its schedule_id (UUID). \
                Stops repeating timers and removes pending one-shot schedules."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "schedule_id": {
                        "type": "string",
                        "description": "The schedule_id (UUID) returned by schedule:create"
                    }
                },
                "required": ["schedule_id"]
            }),
            output_schema: None,
            capabilities: vec!["cancel:schedule".into()],
            supports_streaming: false,
            annotations: Default::default(),
        }
    }

    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        let scheduler = self
            .runtime
            .scheduler()
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: "scheduler not available (no persistent storage configured)".into(),
                retryable: false,
            })?;

        let id_str = env
            .payload
            .get("schedule_id")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: "missing required field: schedule_id".into(),
                retryable: false,
            })?;
        let id: JobId = id_str
            .parse()
            .map_err(|e: uuid::Error| ToolError::ExecutionFailed {
                message: format!("invalid schedule_id (expected UUID): {e}"),
                retryable: false,
            })?;

        let cancelled = scheduler.cancel(id).await.map_err(scheduler_err)?;
        if !cancelled {
            warn!(schedule_id = %id, "cancel:schedule: id not found");
            return Err(ToolError::ExecutionFailed {
                message: format!("schedule_id {id} not found"),
                retryable: false,
            });
        }
        Ok(env.respond(json!({ "cancelled": true, "schedule_id": id.to_string() })))
    }
}

fn scheduler_err(err: konf_runtime::SchedulerError) -> ToolError {
    ToolError::ExecutionFailed {
        message: err.to_string(),
        retryable: false,
    }
}

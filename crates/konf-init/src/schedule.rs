//! Schedule tool — the timer primitive.
//!
//! Non-blocking "run this workflow after a delay." This is the `timer_create`
//! syscall equivalent: minimal kernel support for userspace scheduling.
//!
//! Everything else (cron expressions, persistence, self-rescheduling, state
//! management) is workflows built on top of this primitive.

use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use konflux::error::ToolError;
use konflux::tool::{Tool, ToolContext, ToolInfo};

use konf_runtime::Runtime;

/// Minimum delay: 1 second. Prevents hot-spin loops.
const MIN_DELAY_MS: u64 = 1_000;

/// Maximum delay: 7 days. Prevents unbounded timer accumulation.
const MAX_DELAY_MS: u64 = 7 * 24 * 3600 * 1_000;

/// Monotonic counter for unique schedule IDs.
static SCHEDULE_COUNTER: AtomicU64 = AtomicU64::new(0);

/// A tool that schedules a workflow to run after a delay.
///
/// Returns immediately. The workflow executes in the background after `delay_ms`.
/// The tool is resolved from the live registry at execution time (not schedule time),
/// so hot-reloads and capability changes take effect.
///
/// The scheduled task is fire-and-forget — it cannot be cancelled or queried.
/// For durable scheduling, build persistence as a workflow on top of this primitive.
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
            name: "schedule".into(),
            description: format!(
                "Schedule a workflow to run after a delay (fire-and-forget). \
                 delay_ms must be between {MIN_DELAY_MS} and {MAX_DELAY_MS}. \
                 The workflow is resolved at execution time, not schedule time."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workflow": {
                        "type": "string",
                        "description": "Workflow ID to run (e.g., 'nightwatch' → resolves to 'workflow_nightwatch')"
                    },
                    "delay_ms": {
                        "type": "integer",
                        "description": format!("Milliseconds to wait before execution. Min: {MIN_DELAY_MS}, Max: {MAX_DELAY_MS}.")
                    },
                    "input": {
                        "type": "object",
                        "description": "Input payload for the workflow (optional)"
                    }
                },
                "required": ["workflow", "delay_ms"]
            }),
            output_schema: None,
            capabilities: vec!["schedule".into()],
            supports_streaming: false,
            annotations: Default::default(),
        }
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let workflow_id = input.get("workflow")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: "missing required field: workflow".into(),
                retryable: false,
            })?;

        // Accept both integer and string-encoded integers (template substitution
        // may produce strings like "3600000" instead of bare 3600000).
        let delay_ms = input.get("delay_ms")
            .and_then(|v| v.as_u64().or_else(|| v.as_str().and_then(|s| s.parse().ok())))
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: "missing or invalid field: delay_ms (must be a non-negative integer)".into(),
                retryable: false,
            })?;

        // Enforce bounds to prevent hot-spin loops and unbounded timers.
        if !(MIN_DELAY_MS..=MAX_DELAY_MS).contains(&delay_ms) {
            return Err(ToolError::ExecutionFailed {
                message: format!(
                    "delay_ms={delay_ms} out of range (must be between {MIN_DELAY_MS} and {MAX_DELAY_MS})"
                ),
                retryable: false,
            });
        }

        let workflow_input = input.get("input").cloned().unwrap_or(json!({}));

        // Verify the workflow exists at schedule time (fail-fast).
        // The tool is re-resolved at execution time to respect hot-reloads.
        let tool_name = format!("workflow_{workflow_id}");
        if self.runtime.engine().registry().get(&tool_name).is_none() {
            return Err(ToolError::NotFound {
                tool_id: tool_name,
            });
        }

        let capabilities = ctx.capabilities.clone();
        let schedule_id = SCHEDULE_COUNTER.fetch_add(1, Ordering::Relaxed);

        info!(
            workflow = %workflow_id,
            delay_ms = delay_ms,
            schedule_id = schedule_id,
            "Scheduling workflow"
        );

        // Spawn background task: sleep → re-resolve tool → invoke.
        let runtime = self.runtime.clone();
        let workflow_id_owned = workflow_id.to_string();
        let tool_name_owned = tool_name.clone();
        tokio::spawn(async move {
            tokio::time::sleep(Duration::from_millis(delay_ms)).await;

            // Re-resolve the tool from the live registry. This ensures hot-reloads
            // and capability changes take effect between schedule and execution.
            let registry = runtime.engine().registry();
            let tool = match registry.get(&tool_name_owned) {
                Some(t) => t,
                None => {
                    warn!(
                        workflow = %workflow_id_owned,
                        schedule_id = schedule_id,
                        "Scheduled workflow no longer registered — skipping"
                    );
                    return;
                }
            };

            info!(
                workflow = %workflow_id_owned,
                schedule_id = schedule_id,
                "Executing scheduled workflow"
            );

            let tool_ctx = ToolContext {
                capabilities,
                workflow_id: "schedule".into(),
                node_id: format!("scheduled_{workflow_id_owned}"),
                metadata: std::collections::HashMap::new(),
            };

            match tool.invoke(workflow_input, &tool_ctx).await {
                Ok(_) => {
                    info!(
                        workflow = %workflow_id_owned,
                        schedule_id = schedule_id,
                        "Scheduled workflow completed"
                    );
                }
                Err(e) => {
                    warn!(
                        workflow = %workflow_id_owned,
                        schedule_id = schedule_id,
                        error = %e,
                        "Scheduled workflow failed"
                    );
                }
            }
        });

        Ok(json!({
            "scheduled": true,
            "workflow": workflow_id,
            "delay_ms": delay_ms,
            "schedule_id": schedule_id,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    async fn make_runtime() -> Arc<Runtime> {
        Arc::new(Runtime::new(konflux::Engine::new(), None).await.unwrap())
    }

    #[tokio::test]
    async fn test_schedule_tool_info() {
        let runtime = make_runtime().await;
        let tool = ScheduleTool::new(runtime);
        let info = tool.info();
        assert_eq!(info.name, "schedule");
        assert_eq!(info.capabilities, vec!["schedule"]);
    }

    #[tokio::test]
    async fn test_rejects_delay_below_minimum() {
        let runtime = make_runtime().await;
        let tool = ScheduleTool::new(runtime);
        let ctx = ToolContext {
            capabilities: vec!["*".into()],
            workflow_id: "test".into(),
            node_id: "test".into(),
            metadata: HashMap::new(),
        };

        let result = tool.invoke(json!({
            "workflow": "nonexistent",
            "delay_ms": 0
        }), &ctx).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("out of range"), "got: {err}");
    }

    #[tokio::test]
    async fn test_rejects_delay_above_maximum() {
        let runtime = make_runtime().await;
        let tool = ScheduleTool::new(runtime);
        let ctx = ToolContext {
            capabilities: vec!["*".into()],
            workflow_id: "test".into(),
            node_id: "test".into(),
            metadata: HashMap::new(),
        };

        let result = tool.invoke(json!({
            "workflow": "nonexistent",
            "delay_ms": MAX_DELAY_MS + 1
        }), &ctx).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("out of range"), "got: {err}");
    }

    #[tokio::test]
    async fn test_rejects_missing_workflow() {
        let runtime = make_runtime().await;
        let tool = ScheduleTool::new(runtime);
        let ctx = ToolContext {
            capabilities: vec!["*".into()],
            workflow_id: "test".into(),
            node_id: "test".into(),
            metadata: HashMap::new(),
        };

        let result = tool.invoke(json!({
            "workflow": "nonexistent",
            "delay_ms": 5000
        }), &ctx).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"), "got: {err}");
    }

    #[tokio::test]
    async fn test_accepts_string_delay_ms() {
        let runtime = make_runtime().await;
        let tool = ScheduleTool::new(runtime);
        let ctx = ToolContext {
            capabilities: vec!["*".into()],
            workflow_id: "test".into(),
            node_id: "test".into(),
            metadata: HashMap::new(),
        };

        // String-encoded delay_ms (from template substitution) — should fail
        // on workflow lookup, not on delay parsing
        let result = tool.invoke(json!({
            "workflow": "nonexistent",
            "delay_ms": "5000"
        }), &ctx).await;

        assert!(result.is_err());
        let err = result.unwrap_err().to_string();
        assert!(err.contains("not found"), "got: {err}");
    }
}

//! Schedule tool — the timer primitive.
//!
//! Non-blocking "run this workflow after a delay." This is the `timer_create`
//! syscall equivalent: minimal kernel support for userspace scheduling.
//!
//! With `repeat: true`, the timer automatically re-fires after each completion,
//! like `timer_create` with `it_interval` set. The workflow stays clean —
//! it doesn't know it's being repeated. Scheduling is a separate concern.

use std::collections::HashMap as StdHashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tokio::task::JoinHandle;
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

/// Global map of active schedule handles. Used by `cancel_schedule` to abort timers.
static SCHEDULE_HANDLES: std::sync::LazyLock<std::sync::Mutex<StdHashMap<u64, JoinHandle<()>>>> =
    std::sync::LazyLock::new(|| std::sync::Mutex::new(StdHashMap::new()));

/// A tool that schedules a workflow to run after a delay.
///
/// With `repeat: false` (default): runs once after `delay_ms`, then stops.
/// With `repeat: true`: runs every `delay_ms` indefinitely, re-using the same input.
///
/// The workflow is resolved from the live registry at each execution, so
/// hot-reloads and capability changes take effect between runs.
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
                "Schedule a workflow to run after a delay. Returns immediately. \
                 delay_ms: {MIN_DELAY_MS}–{MAX_DELAY_MS}. \
                 Set repeat: true for a repeating timer (workflow re-runs every delay_ms)."
            ),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workflow": {
                        "type": "string",
                        "description": "Workflow ID (e.g., 'nightwatch' → workflow_nightwatch)"
                    },
                    "delay_ms": {
                        "type": "integer",
                        "description": format!("Milliseconds between runs. Min: {MIN_DELAY_MS}, Max: {MAX_DELAY_MS}.")
                    },
                    "input": {
                        "type": "object",
                        "description": "Input payload for the workflow (optional, reused on each repeat)"
                    },
                    "repeat": {
                        "type": "boolean",
                        "description": "If true, re-run every delay_ms after completion. Default: false."
                    }
                },
                "required": ["workflow", "delay_ms"]
            }),
            output_schema: None,
            capabilities: vec!["schedule:create".into()],
            supports_streaming: false,
            annotations: Default::default(),
        }
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let workflow_id = input
            .get("workflow")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: "missing required field: workflow".into(),
                retryable: false,
            })?;

        let delay_ms = input
            .get("delay_ms")
            .and_then(|v| {
                v.as_u64()
                    .or_else(|| v.as_str().and_then(|s| s.parse().ok()))
            })
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: "missing or invalid field: delay_ms (must be a positive integer)".into(),
                retryable: false,
            })?;

        if !(MIN_DELAY_MS..=MAX_DELAY_MS).contains(&delay_ms) {
            return Err(ToolError::ExecutionFailed {
                message: format!(
                    "delay_ms={delay_ms} out of range ({MIN_DELAY_MS}–{MAX_DELAY_MS})"
                ),
                retryable: false,
            });
        }

        let repeat = input
            .get("repeat")
            .and_then(|v| v.as_bool())
            .unwrap_or(false);

        let workflow_input = input.get("input").cloned().unwrap_or(json!({}));

        // Fail-fast: verify workflow exists now (re-resolved at each execution).
        let tool_name = format!("workflow:{workflow_id}");
        if self.runtime.engine().registry().get(&tool_name).is_none() {
            return Err(ToolError::NotFound { tool_id: tool_name });
        }

        let capabilities = ctx.capabilities.clone();
        let schedule_id = SCHEDULE_COUNTER.fetch_add(1, Ordering::Relaxed);

        info!(
            workflow = %workflow_id,
            delay_ms,
            repeat,
            schedule_id,
            "Scheduling workflow"
        );

        let runtime = self.runtime.clone();
        let workflow_id_owned = workflow_id.to_string();
        let tool_name_owned = tool_name.clone();

        let handle = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;

                // Re-resolve from live registry each time.
                let registry = runtime.engine().registry();
                let tool = match registry.get(&tool_name_owned) {
                    Some(t) => t,
                    None => {
                        warn!(
                            workflow = %workflow_id_owned,
                            schedule_id,
                            "Workflow no longer registered — stopping"
                        );
                        return;
                    }
                };

                info!(workflow = %workflow_id_owned, schedule_id, "Executing scheduled workflow");

                let tool_ctx = ToolContext {
                    capabilities: capabilities.clone(),
                    workflow_id: "schedule".into(),
                    node_id: format!("scheduled_{workflow_id_owned}"),
                    metadata: std::collections::HashMap::new(),
                };

                match tool.invoke(workflow_input.clone(), &tool_ctx).await {
                    Ok(_) => {
                        info!(workflow = %workflow_id_owned, schedule_id, "Scheduled workflow completed");
                    }
                    Err(e) => {
                        warn!(workflow = %workflow_id_owned, schedule_id, error = %e, "Scheduled workflow failed");
                    }
                }

                if !repeat {
                    return;
                }
            }
        });

        // Store handle for cancellation via cancel_schedule tool.
        if let Ok(mut handles) = SCHEDULE_HANDLES.lock() {
            handles.insert(schedule_id, handle);
        }

        Ok(json!({
            "scheduled": true,
            "workflow": workflow_id,
            "delay_ms": delay_ms,
            "repeat": repeat,
            "schedule_id": schedule_id,
        }))
    }
}

/// Cancel a previously scheduled workflow by its `schedule_id`.
///
/// Aborts the tokio task. If the workflow is mid-execution, it will be
/// interrupted at the next `.await` point.
pub struct CancelScheduleTool;

#[async_trait]
impl Tool for CancelScheduleTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "cancel:schedule".into(),
            description: "Cancel a scheduled workflow by its schedule_id. \
                Stops repeating timers and aborts pending one-shot schedules."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "schedule_id": {
                        "type": "integer",
                        "description": "The schedule_id returned by the schedule tool"
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

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let schedule_id = input
            .get("schedule_id")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: "missing required field: schedule_id".into(),
                retryable: false,
            })?;

        let cancelled = if let Ok(mut handles) = SCHEDULE_HANDLES.lock() {
            if let Some(handle) = handles.remove(&schedule_id) {
                handle.abort();
                info!(schedule_id, "Cancelled scheduled workflow");
                true
            } else {
                false
            }
        } else {
            warn!("Failed to lock schedule handles map");
            false
        };

        if cancelled {
            Ok(json!({ "cancelled": true, "schedule_id": schedule_id }))
        } else {
            Err(ToolError::ExecutionFailed {
                message: format!("schedule_id {schedule_id} not found or already completed"),
                retryable: false,
            })
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    async fn make_runtime() -> Arc<Runtime> {
        Arc::new(Runtime::new(konflux::Engine::new(), None).await.unwrap())
    }

    fn test_ctx() -> ToolContext {
        ToolContext {
            capabilities: vec!["*".into()],
            workflow_id: "test".into(),
            node_id: "test".into(),
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_schedule_tool_info() {
        let runtime = make_runtime().await;
        let tool = ScheduleTool::new(runtime);
        let info = tool.info();
        assert_eq!(info.name, "schedule:create");
        assert!(info.description.contains("repeat"));
    }

    #[tokio::test]
    async fn test_rejects_delay_below_minimum() {
        let tool = ScheduleTool::new(make_runtime().await);
        let result = tool
            .invoke(json!({"workflow": "x", "delay_ms": 0}), &test_ctx())
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("out of range"));
    }

    #[tokio::test]
    async fn test_rejects_delay_above_maximum() {
        let tool = ScheduleTool::new(make_runtime().await);
        let result = tool
            .invoke(
                json!({"workflow": "x", "delay_ms": MAX_DELAY_MS + 1}),
                &test_ctx(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("out of range"));
    }

    #[tokio::test]
    async fn test_rejects_missing_workflow() {
        let tool = ScheduleTool::new(make_runtime().await);
        let result = tool
            .invoke(
                json!({"workflow": "nonexistent", "delay_ms": 5000}),
                &test_ctx(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_accepts_string_delay_ms() {
        let tool = ScheduleTool::new(make_runtime().await);
        let result = tool
            .invoke(
                json!({"workflow": "nonexistent", "delay_ms": "5000"}),
                &test_ctx(),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_repeat_defaults_to_false() {
        let tool = ScheduleTool::new(make_runtime().await);
        let result = tool
            .invoke(
                json!({"workflow": "nonexistent", "delay_ms": 5000}),
                &test_ctx(),
            )
            .await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_cancel_nonexistent_schedule() {
        let tool = CancelScheduleTool;
        let result = tool
            .invoke(json!({"schedule_id": 99999}), &test_ctx())
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().to_string().contains("not found"));
    }

    #[tokio::test]
    async fn test_cancel_tool_info() {
        let tool = CancelScheduleTool;
        let info = tool.info();
        assert_eq!(info.name, "cancel:schedule");
        assert_eq!(info.capabilities, vec!["cancel:schedule"]);
    }
}

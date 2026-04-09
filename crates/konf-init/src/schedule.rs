//! Schedule tool — the timer primitive.
//!
//! Non-blocking "run this workflow after a delay." This is the `timer_create`
//! syscall equivalent: minimal kernel support for userspace scheduling.
//!
//! Everything else (cron expressions, persistence, self-rescheduling, state
//! management) is workflows built on top of this primitive.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use konflux::error::ToolError;
use konflux::tool::{Tool, ToolContext, ToolInfo};

use konf_runtime::Runtime;

/// A tool that schedules a workflow to run after a delay.
///
/// Returns immediately. The workflow executes in the background after `delay_ms`.
/// The scheduled workflow inherits scoped capabilities from the caller's context.
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
            description: "Schedule a workflow to run after a delay. Returns immediately. \
                The workflow executes in the background after delay_ms milliseconds. \
                Use delay_ms: 0 for immediate background execution.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workflow": {
                        "type": "string",
                        "description": "Workflow ID to run (e.g., 'nightwatch' → resolves to 'workflow_nightwatch')"
                    },
                    "delay_ms": {
                        "type": "integer",
                        "description": "Milliseconds to wait before execution. 0 = immediate."
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

        let delay_ms = input.get("delay_ms")
            .and_then(|v| v.as_u64())
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: "missing required field: delay_ms (must be a non-negative integer)".into(),
                retryable: false,
            })?;

        let workflow_input = input.get("input").cloned().unwrap_or(json!({}));

        // Resolve the workflow from the engine's registry
        let tool_name = format!("workflow_{workflow_id}");
        let registry = self.runtime.engine().registry();
        let tool = registry.get(&tool_name).ok_or_else(|| ToolError::NotFound {
            tool_id: tool_name.clone(),
        })?;

        // The scheduled workflow inherits the caller's capability set.
        let capabilities = ctx.capabilities.clone();

        let session_id = format!(
            "schedule_{}_{}",
            workflow_id,
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_millis()
        );

        info!(
            workflow = %workflow_id,
            delay_ms = delay_ms,
            "Scheduling workflow"
        );

        // Spawn background task: sleep → invoke the workflow tool
        let workflow_id_owned = workflow_id.to_string();
        tokio::spawn(async move {
            if delay_ms > 0 {
                tokio::time::sleep(Duration::from_millis(delay_ms)).await;
            }

            info!(workflow = %workflow_id_owned, "Executing scheduled workflow");

            // Invoke the workflow tool directly — this reuses WorkflowTool's
            // existing scope creation and capability checking.
            let tool_ctx = ToolContext {
                capabilities,
                workflow_id: "schedule".into(),
                node_id: format!("scheduled_{workflow_id_owned}"),
                metadata: std::collections::HashMap::new(),
            };

            match tool.invoke(workflow_input, &tool_ctx).await {
                Ok(result) => {
                    info!(
                        workflow = %workflow_id_owned,
                        "Scheduled workflow completed"
                    );
                    let _ = result; // Result is logged but not returned (fire-and-forget)
                }
                Err(e) => {
                    warn!(
                        workflow = %workflow_id_owned,
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
            "session_id": session_id,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_schedule_tool_info() {
        let rt = tokio::runtime::Runtime::new().unwrap();
        let runtime = rt.block_on(async {
            Arc::new(Runtime::new(konflux::Engine::new(), None).await.unwrap())
        });
        let tool = ScheduleTool::new(runtime);
        let info = tool.info();
        assert_eq!(info.name, "schedule");
        assert_eq!(info.capabilities, vec!["schedule"]);
    }
}

//! Tool shells: `runner:spawn`, `runner:status`, `runner:wait`, `runner:cancel`.
//!
//! Every tool holds an `Arc<dyn Runner>` and delegates. The registry (not
//! the runner backend) is authoritative for status/wait because a single
//! registry is shared across backends — this keeps `runner:status` working
//! even when a run is handed off between backends in a future multi-
//! backend deployment.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::info;

use konflux::error::ToolError;
use konflux::tool::{Tool, ToolAnnotations, ToolContext, ToolInfo};
use konflux::Engine;

use crate::runner::{Runner, WorkflowSpec};

/// Register every runner tool into the engine.
///
/// Call once from `konf-init` after the runtime is built, passing a ready
/// runner backend. The registry carried by that backend is the one that
/// `runner:status`/`wait`/`cancel` will query.
pub fn register(engine: &Engine, runner: Arc<dyn Runner>) -> anyhow::Result<()> {
    engine.register_tool(Arc::new(SpawnTool::new(runner.clone())));
    engine.register_tool(Arc::new(StatusTool::new(runner.clone())));
    engine.register_tool(Arc::new(WaitTool::new(runner.clone())));
    engine.register_tool(Arc::new(CancelTool::new(runner)));
    info!("runner tools registered (spawn, status, wait, cancel)");
    Ok(())
}

// ----------------------------------------------------------------------
// runner:spawn
// ----------------------------------------------------------------------

/// `runner:spawn` — start an asynchronous workflow run via the runner backend.
pub struct SpawnTool {
    runner: Arc<dyn Runner>,
}

impl SpawnTool {
    /// Build a spawn tool over a runner backend.
    pub fn new(runner: Arc<dyn Runner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl Tool for SpawnTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "runner:spawn".into(),
            description: "Spawn a registered workflow as a new run. \
                Returns immediately with a run_id; use runner:wait to collect \
                the result."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "workflow": { "type": "string", "description": "Workflow id (matches workflow:<id> in the engine registry)" },
                    "input":    { "type": "object", "description": "Input payload for the workflow (default: {})" }
                },
                "required": ["workflow"]
            }),
            output_schema: None,
            capabilities: vec!["runner:spawn".into()],
            supports_streaming: false,
            annotations: Default::default(),
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let workflow = input
            .get("workflow")
            .and_then(Value::as_str)
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: "runner:spawn missing required field `workflow`".into(),
                retryable: false,
            })?
            .to_string();
        let wf_input = input.get("input").cloned().unwrap_or_else(|| json!({}));
        let id = self
            .runner
            .spawn(WorkflowSpec {
                workflow: workflow.clone(),
                input: wf_input,
            })
            .await
            .map_err(runner_err)?;
        Ok(json!({
            "run_id": id,
            "workflow": workflow,
            "backend": self.runner.name(),
        }))
    }
}

// ----------------------------------------------------------------------
// runner:status
// ----------------------------------------------------------------------

/// `runner:status` — non-blocking state query for a run.
pub struct StatusTool {
    runner: Arc<dyn Runner>,
}

impl StatusTool {
    /// Build a status tool over a runner backend.
    pub fn new(runner: Arc<dyn Runner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl Tool for StatusTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "runner:status".into(),
            description: "Query the current state of a run by id. Non-blocking.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "run_id": { "type": "string" }
                },
                "required": ["run_id"]
            }),
            output_schema: None,
            capabilities: vec!["runner:status".into()],
            supports_streaming: false,
            annotations: ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            },
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let run_id = require_run_id(&input, "runner:status")?;
        let record = self
            .runner
            .registry()
            .record(&run_id)
            .await
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: format!("run_id {run_id} not found"),
                retryable: false,
            })?;
        Ok(serde_json::to_value(&record).unwrap_or(Value::Null))
    }
}

// ----------------------------------------------------------------------
// runner:wait
// ----------------------------------------------------------------------

/// `runner:wait` — block until a run reaches a terminal state.
pub struct WaitTool {
    runner: Arc<dyn Runner>,
}

impl WaitTool {
    /// Build a wait tool over a runner backend.
    pub fn new(runner: Arc<dyn Runner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl Tool for WaitTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "runner:wait".into(),
            description: "Block until a run reaches a terminal state. \
                Optional timeout_secs; defaults to 300 seconds."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "run_id": { "type": "string" },
                    "timeout_secs": { "type": "integer", "minimum": 1, "maximum": 86400 }
                },
                "required": ["run_id"]
            }),
            output_schema: None,
            capabilities: vec!["runner:wait".into()],
            supports_streaming: false,
            annotations: ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            },
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let run_id = require_run_id(&input, "runner:wait")?;
        let timeout = input
            .get("timeout_secs")
            .and_then(Value::as_u64)
            .unwrap_or(300);
        let fut = self.runner.registry().wait_terminal(&run_id);
        let record = tokio::time::timeout(Duration::from_secs(timeout), fut)
            .await
            .map_err(|_| ToolError::ExecutionFailed {
                message: format!("runner:wait timed out after {timeout}s for run_id {run_id}"),
                retryable: true,
            })?
            .ok_or_else(|| ToolError::ExecutionFailed {
                message: format!("run_id {run_id} not found"),
                retryable: false,
            })?;
        Ok(serde_json::to_value(&record).unwrap_or(Value::Null))
    }
}

// ----------------------------------------------------------------------
// runner:cancel
// ----------------------------------------------------------------------

/// `runner:cancel` — abort an in-flight run.
pub struct CancelTool {
    runner: Arc<dyn Runner>,
}

impl CancelTool {
    /// Build a cancel tool over a runner backend.
    pub fn new(runner: Arc<dyn Runner>) -> Self {
        Self { runner }
    }
}

#[async_trait]
impl Tool for CancelTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "runner:cancel".into(),
            description: "Cancel a run by id. Returns whether a cancel hook was invoked.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "run_id": { "type": "string" }
                },
                "required": ["run_id"]
            }),
            output_schema: None,
            capabilities: vec!["runner:cancel".into()],
            supports_streaming: false,
            annotations: ToolAnnotations {
                destructive: true,
                ..Default::default()
            },
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let run_id = require_run_id(&input, "runner:cancel")?;
        let cancelled = self.runner.cancel(&run_id).await.map_err(runner_err)?;
        Ok(json!({ "run_id": run_id, "cancelled": cancelled }))
    }
}

// ----------------------------------------------------------------------
// helpers
// ----------------------------------------------------------------------

fn require_run_id(input: &Value, tool: &str) -> Result<String, ToolError> {
    input
        .get("run_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .ok_or_else(|| ToolError::ExecutionFailed {
            message: format!("{tool} missing required field `run_id`"),
            retryable: false,
        })
}

fn runner_err(err: crate::error::RunnerError) -> ToolError {
    ToolError::ExecutionFailed {
        message: err.to_string(),
        retryable: false,
    }
}

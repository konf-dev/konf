//! Execution lifecycle hooks.
//!
//! The `ExecutionHooks` trait allows consumers (like konf-runtime) to observe
//! workflow execution without modifying the engine. Hooks are called synchronously
//! from the executor — keep implementations fast (no blocking I/O in hooks).
//!
//! Default implementations are no-ops, so consumers only override what they need.

use serde_json::Value;

/// Callbacks for workflow execution lifecycle events.
///
/// Implementors receive notifications about node and tool execution.
/// Used by konf-runtime to update the process table and event journal.
pub trait ExecutionHooks: Send + Sync {
    /// Called when a node begins execution.
    fn on_node_start(&self, _node_id: &str, _tool: &str) {}

    /// Called when a node completes successfully.
    fn on_node_complete(&self, _node_id: &str, _tool: &str, _duration_ms: u64, _output: &Value) {}

    /// Called when a node fails (after all retries exhausted).
    fn on_node_failed(&self, _node_id: &str, _tool: &str, _error: &str) {}

    /// Called before a tool retry attempt.
    fn on_tool_retry(&self, _node_id: &str, _tool: &str, _attempt: u32, _error: &str) {}
}

/// No-op hooks implementation (default when no hooks are provided).
pub struct NoopHooks;
impl ExecutionHooks for NoopHooks {}

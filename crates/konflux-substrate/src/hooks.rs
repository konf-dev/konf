//! Execution lifecycle events.
//!
//! The substrate executor emits [`ExecutorEvent`]s during DAG walks. The
//! [`EventRecorder`] trait allows consumers (like konf-runtime) to observe
//! execution without modifying the engine. Recorders are called
//! synchronously from the executor — keep implementations fast (no
//! blocking I/O in the recorder itself).

use serde_json::Value;

/// Events emitted by the substrate executor during DAG walks.
pub enum ExecutorEvent<'a> {
    /// A workflow node began execution.
    NodeStarted { node_id: &'a str, tool: &'a str },
    /// A workflow node completed successfully.
    NodeCompleted {
        node_id: &'a str,
        tool: &'a str,
        duration_ms: u64,
        output: &'a Value,
    },
    /// A workflow node failed (after all retries exhausted).
    NodeFailed {
        node_id: &'a str,
        tool: &'a str,
        error: &'a str,
    },
    /// A tool retry attempt is about to happen.
    ToolRetry {
        node_id: &'a str,
        tool: &'a str,
        attempt: u32,
        error: &'a str,
    },
}

/// Records executor lifecycle events.
///
/// Implementors receive notifications about node and tool execution.
/// Used by konf-runtime to update the process table, event bus, and
/// journal in real-time. Default implementation is a no-op.
pub trait EventRecorder: Send + Sync {
    fn on_event(&self, _event: ExecutorEvent<'_>) {}
}

/// No-op recorder (default when no recorder is provided).
pub struct NoopRecorder;
impl EventRecorder for NoopRecorder {}

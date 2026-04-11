//! Runner error types.

use crate::registry::RunId;

/// Errors raised by the `Runner` trait and its tool wrappers.
#[derive(Debug, thiserror::Error)]
pub enum RunnerError {
    /// The run id is not present in the runner's registry.
    #[error("run not found: {0}")]
    NotFound(RunId),

    /// The caller asked for a workflow name that is not registered in the
    /// engine as a `workflow:<name>` tool.
    #[error("workflow not found: {0}")]
    WorkflowNotFound(String),

    /// The runner refused a call because its underlying backend said so
    /// (for example, the inline runner's handle channel was dropped).
    #[error("runner backend failed: {0}")]
    Backend(String),

    /// Caller-side validation error (bad input shape, missing field, etc.).
    #[error("invalid runner input: {0}")]
    Validation(String),

    /// The feature is not implemented in this runner backend yet.
    #[error("runner operation unsupported: {0}")]
    Unsupported(String),
}

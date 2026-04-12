//! Error types for the konf-runtime.

use uuid::Uuid;

use crate::journal::JournalError;

/// Unique identifier for a workflow run.
pub type RunId = Uuid;

/// Errors from the runtime.
#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error("workflow run not found: {0}")]
    NotFound(RunId),

    #[error("workflow run {0} is not running")]
    NotRunning(RunId),

    #[error("resource limit exceeded: {limit} (max {value})")]
    ResourceLimit { limit: String, value: usize },

    #[error("capability denied: {0}")]
    CapabilityDenied(String),

    /// A tool was invoked successfully (scope permitted it) but the tool
    /// itself returned an error. Used by [`crate::Runtime::invoke_tool`].
    #[error("tool '{tool}' failed: {message}")]
    Tool { tool: String, message: String },

    #[error("engine error: {0}")]
    Engine(#[from] konflux::KonfluxError),

    #[error("join failed: {0}")]
    JoinFailed(String),

    #[error("journal: {0}")]
    Journal(#[from] JournalError),
}

//! Error types for the konf-runtime.

use crate::journal::JournalError;

// Re-export RunId so existing `use crate::error::RunId` paths still work.
pub use crate::journal::RunId;

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

    /// The envelope's deadline has passed before the tool could be invoked.
    #[error("deadline exceeded for tool '{tool}'")]
    DeadlineExceeded { tool: String },

    /// A tool was invoked successfully (scope permitted it) but the tool
    /// itself returned an error. Used by [`crate::Runtime::invoke_tool`].
    #[error("tool '{tool}' failed: {message}")]
    Tool { tool: String, message: String },

    #[error("engine error: {0}")]
    Engine(#[from] konflux_substrate::KonfluxError),

    #[error("join failed: {0}")]
    JoinFailed(String),

    #[error("journal: {0}")]
    Journal(#[from] JournalError),
}

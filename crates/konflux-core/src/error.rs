//! Error types for the konflux engine.

use thiserror::Error;

/// Top-level engine errors.
#[derive(Debug, Error)]
pub enum KonfluxError {
    #[error("parse error: {0}")]
    Parse(#[from] ParseError),

    #[error("validation error: {0}")]
    Validation(#[from] ValidationError),

    #[error("execution error: {0}")]
    Execution(#[from] ExecutionError),

    #[error("tool error: {0}")]
    Tool(#[from] ToolError),

    #[error("capability denied: {0}")]
    CapabilityDenied(String),
}

/// Errors during YAML parsing.
#[derive(Debug, Error)]
pub enum ParseError {
    #[error("invalid YAML: {message}")]
    InvalidYaml { message: String },

    #[error("missing required field '{field}' in {context}")]
    MissingField { field: String, context: String },

    #[error("invalid value for '{field}': {message}")]
    InvalidValue { field: String, message: String },
}

/// Errors during workflow validation (post-parse).
#[derive(Debug, Error)]
pub enum ValidationError {
    #[error("cycle detected: {}", path.join(" → "))]
    CycleDetected { path: Vec<String> },

    #[error("orphaned node '{node}' — not reachable from entry")]
    OrphanedNode { node: String },

    #[error("unknown node '{node}' referenced in edge from '{from}'")]
    UnknownNode { node: String, from: String },

    #[error("unknown tool '{tool}' in node '{node}'")]
    UnknownTool { tool: String, node: String },

    #[error("capability '{capability}' required but not granted")]
    MissingCapability { capability: String },

    #[error("no entry node found")]
    NoEntryNode,

    #[error("no return node found")]
    NoReturnNode,
}

/// Errors during workflow execution.
#[derive(Debug, Error)]
pub enum ExecutionError {
    #[error("[{workflow_id}] node '{node}' failed: {message}")]
    NodeFailed { workflow_id: String, node: String, message: String },

    #[error("[{workflow_id}] node '{node}' timed out after {timeout_ms}ms")]
    Timeout { workflow_id: String, node: String, timeout_ms: u64 },

    #[error("[{workflow_id}] max steps exceeded ({max})")]
    MaxStepsExceeded { workflow_id: String, max: usize },

    #[error("[{workflow_id}] join failed on node '{node}': {message}")]
    JoinFailed { workflow_id: String, node: String, message: String },

    #[error("[{workflow_id}] workflow cancelled")]
    Cancelled { workflow_id: String },
}

/// Errors from tool invocation.
#[derive(Debug, Clone, Error)]
pub enum ToolError {
    #[error("invalid input: {message}")]
    InvalidInput { message: String, field: Option<String> },

    #[error("execution failed: {message}")]
    ExecutionFailed { message: String, retryable: bool },

    #[error("timeout after {after_ms}ms")]
    Timeout { after_ms: u64 },

    #[error("capability denied: {capability}")]
    CapabilityDenied { capability: String },

    #[error("access denied: {message}")]
    AccessDenied { message: String },

    #[error("tool not found: {tool_id}")]
    NotFound { tool_id: String },
}

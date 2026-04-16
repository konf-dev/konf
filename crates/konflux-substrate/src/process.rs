//! Process run types — status tracking for workflow execution.
//!
//! These types represent the mechanism-level state of workflow nodes
//! and runs. The `ProcessTable` (which tracks active runs with
//! runtime-specific fields like `Actor`) stays in konf-runtime.

use chrono::{DateTime, Utc};
use serde::Serialize;

/// Status of a workflow run.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum RunStatus {
    Pending,
    Running,
    Completed {
        duration_ms: u64,
        output: serde_json::Value,
    },
    Failed {
        error: String,
        duration_ms: u64,
    },
    Cancelled {
        reason: String,
        duration_ms: u64,
    },
}

impl RunStatus {
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            Self::Completed { .. } | Self::Failed { .. } | Self::Cancelled { .. }
        )
    }
}

/// A node currently executing within a workflow run.
#[derive(Debug, Clone, Serialize)]
pub struct ActiveNode {
    pub node_id: String,
    pub tool_name: String,
    pub started_at: DateTime<Utc>,
    pub status: NodeStatus,
}

/// Status of an active node.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum NodeStatus {
    Running,
    Retrying { attempt: u32, max: u32 },
    Completed { duration_ms: u64 },
    Failed { error: String },
}

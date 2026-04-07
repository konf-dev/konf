//! Monitoring types — serializable views of runtime state.

use chrono::{DateTime, Utc};
use serde::Serialize;

use crate::error::RunId;
use crate::process::{ActiveNode, RunStatus};
use crate::scope::ActorRole;

/// Summary view of a workflow run (for listings).
#[derive(Debug, Clone, Serialize)]
pub struct RunSummary {
    pub id: RunId,
    pub parent_id: Option<RunId>,
    pub workflow_id: String,
    pub namespace: String,
    pub status: RunStatus,
    pub actor_id: String,
    pub actor_role: ActorRole,
    pub started_at: DateTime<Utc>,
    pub active_node_count: usize,
    pub steps_executed: usize,
}

/// Detailed view of a workflow run.
#[derive(Debug, Clone, Serialize)]
pub struct RunDetail {
    pub summary: RunSummary,
    pub active_nodes: Vec<ActiveNode>,
    pub capabilities: Vec<String>,
    pub children: Vec<RunSummary>,
}

/// Recursive process tree rooted at a run.
#[derive(Debug, Clone, Serialize)]
pub struct ProcessTree {
    pub run: RunSummary,
    pub children: Vec<ProcessTree>,
    pub active_nodes: Vec<ActiveNode>,
}

/// Aggregate runtime metrics.
#[derive(Debug, Clone, Serialize)]
pub struct RuntimeMetrics {
    pub active_runs: usize,
    pub total_completed: u64,
    pub total_failed: u64,
    pub total_cancelled: u64,
    pub uptime_seconds: u64,
}

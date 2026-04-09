//! Process table — tracks active workflow runs.
//!
//! The ProcessTable is an in-memory concurrent hashmap (papaya).
//! It is ephemeral — lost on restart. Completed run history
//! survives in the runtime_events Postgres table.

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::error::RunId;
use crate::scope::Actor;

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
}

/// A tracked workflow execution.
/// Fields that are updated after creation use interior mutability (Mutex).
pub struct WorkflowRun {
    pub id: RunId,
    pub parent_id: Option<RunId>,
    pub workflow_id: String,
    pub namespace: String,
    pub actor: Actor,
    pub capabilities: Vec<String>,
    pub metadata: HashMap<String, Value>,
    pub started_at: DateTime<Utc>,

    // Mutable state (updated from spawned tasks)
    pub status: Mutex<RunStatus>,
    pub completed_at: Mutex<Option<DateTime<Utc>>>,
    pub active_nodes: Mutex<Vec<ActiveNode>>,
    pub steps_executed: AtomicUsize,

    // Control
    pub cancel_token: CancellationToken,
}

impl WorkflowRun {
    /// Create a summary view (no internal state exposed).
    pub fn to_summary(&self) -> crate::monitor::RunSummary {
        let active_nodes = self.active_nodes.lock().unwrap_or_else(|p| p.into_inner());
        let status = self.status.lock().unwrap_or_else(|p| p.into_inner());
        crate::monitor::RunSummary {
            id: self.id,
            parent_id: self.parent_id,
            workflow_id: self.workflow_id.clone(),
            namespace: self.namespace.clone(),
            status: status.clone(),
            actor_id: self.actor.id.clone(),
            actor_role: self.actor.role.clone(),
            started_at: self.started_at,
            active_node_count: active_nodes.len(),
            steps_executed: self.steps_executed.load(Ordering::Relaxed),
        }
    }
}

/// Concurrent process table. Lock-free reads via papaya.
pub struct ProcessTable {
    runs: papaya::HashMap<RunId, WorkflowRun>,
}

impl ProcessTable {
    pub fn new() -> Self {
        Self {
            runs: papaya::HashMap::new(),
        }
    }

    pub fn insert(&self, run: WorkflowRun) {
        self.runs.pin().insert(run.id, run);
    }

    pub fn get<F, R>(&self, id: &RunId, f: F) -> Option<R>
    where
        F: FnOnce(&WorkflowRun) -> R,
    {
        let guard = self.runs.pin();
        guard.get(id).map(f)
    }

    pub fn update<F>(&self, id: &RunId, f: F) -> bool
    where
        F: FnOnce(&WorkflowRun),
    {
        let guard = self.runs.pin();
        if let Some(run) = guard.get(id) {
            f(run);
            true
        } else {
            false
        }
    }

    pub fn remove(&self, id: &RunId) -> bool {
        self.runs.pin().remove(id).is_some()
    }

    /// List runs, optionally filtered by namespace prefix.
    pub fn list(&self, namespace_prefix: Option<&str>) -> Vec<crate::monitor::RunSummary> {
        let guard = self.runs.pin();
        guard
            .iter()
            .filter(|(_, run)| {
                namespace_prefix
                    .map(|prefix| run.namespace.starts_with(prefix))
                    .unwrap_or(true)
            })
            .map(|(_, run)| run.to_summary())
            .collect()
    }

    /// Get children of a parent run.
    pub fn children_of(&self, parent_id: RunId) -> Vec<crate::monitor::RunSummary> {
        let guard = self.runs.pin();
        guard
            .iter()
            .filter(|(_, run)| run.parent_id == Some(parent_id))
            .map(|(_, run)| run.to_summary())
            .collect()
    }

    /// Count of currently running workflows.
    pub fn active_count(&self) -> usize {
        let guard = self.runs.pin();
        guard
            .iter()
            .filter(|(_, run)| {
                matches!(
                    *run.status.lock().unwrap_or_else(|p| p.into_inner()),
                    RunStatus::Running
                )
            })
            .count()
    }

    /// Count of running workflows in a specific namespace.
    pub fn active_count_in_namespace(&self, namespace_prefix: &str) -> usize {
        let guard = self.runs.pin();
        guard
            .iter()
            .filter(|(_, run)| {
                matches!(
                    *run.status.lock().unwrap_or_else(|p| p.into_inner()),
                    RunStatus::Running
                ) && run.namespace.starts_with(namespace_prefix)
            })
            .count()
    }

    /// Remove completed runs older than max_age.
    pub fn gc(&self, max_age: std::time::Duration) {
        let cutoff = Utc::now()
            - chrono::Duration::from_std(max_age).expect("gc max_age must be a valid duration");
        let guard = self.runs.pin();
        let to_remove: Vec<RunId> = guard
            .iter()
            .filter(|(_, run)| {
                let status = run.status.lock().unwrap_or_else(|p| p.into_inner());
                let completed = run.completed_at.lock().unwrap_or_else(|p| p.into_inner());
                status.is_terminal() && completed.map(|t| t < cutoff).unwrap_or(false)
            })
            .map(|(id, _)| *id)
            .collect();
        drop(guard);

        for id in to_remove {
            self.runs.pin().remove(&id);
        }
    }
}

impl Default for ProcessTable {
    fn default() -> Self {
        Self::new()
    }
}

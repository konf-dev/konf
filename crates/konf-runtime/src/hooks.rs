//! Runtime hooks — connects the konflux executor to the process table.
//!
//! Implements `konflux::hooks::ExecutionHooks` to update the process table
//! and event journal in real-time during workflow execution.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use chrono::Utc;
use serde_json::Value;

use konflux::hooks::ExecutionHooks;

use crate::error::RunId;
use crate::journal::EventJournal;
use crate::process::{ActiveNode, NodeStatus, ProcessTable};

/// Hooks implementation that updates the process table and journal.
pub struct RuntimeHooks {
    pub run_id: RunId,
    pub namespace: String,
    pub session_id: String,
    pub table: Arc<ProcessTable>,
    pub journal: Option<Arc<EventJournal>>,
}

impl ExecutionHooks for RuntimeHooks {
    fn on_node_start(&self, node_id: &str, tool: &str) {
        let node = ActiveNode {
            node_id: node_id.to_string(),
            tool_name: tool.to_string(),
            started_at: Utc::now(),
            status: NodeStatus::Running,
        };

        self.table.update(&self.run_id, |run| {
            if let Ok(mut nodes) = run.active_nodes.lock() {
                nodes.push(node.clone());
            }
        });

        let journal = self.journal.as_ref().cloned();
        let entry = crate::journal::JournalEntry {
            run_id: self.run_id,
            session_id: self.session_id.clone(),
            namespace: self.namespace.clone(),
            event_type: "node_started".into(),
            payload: serde_json::json!({
                "node_id": node_id,
                "tool": tool,
            }),
        };
        // Fire-and-forget journal append (don't block the executor)
        if let Some(journal) = journal {
            tokio::spawn(async move {
                if let Err(e) = journal.append(entry).await {
                    tracing::warn!(error = %e, "Failed to append journal entry");
                }
            });
        }
    }

    fn on_node_complete(&self, node_id: &str, tool: &str, duration_ms: u64, _output: &Value) {
        self.table.update(&self.run_id, |run| {
            if let Ok(mut nodes) = run.active_nodes.lock() {
                nodes.retain(|n| n.node_id != node_id);
            }
            run.steps_executed.fetch_add(1, Ordering::Relaxed);
        });

        let journal = self.journal.as_ref().cloned();
        let entry = crate::journal::JournalEntry {
            run_id: self.run_id,
            session_id: self.session_id.clone(),
            namespace: self.namespace.clone(),
            event_type: "node_completed".into(),
            payload: serde_json::json!({
                "node_id": node_id,
                "tool": tool,
                "duration_ms": duration_ms,
            }),
        };
        if let Some(journal) = journal {
            tokio::spawn(async move {
                if let Err(e) = journal.append(entry).await {
                    tracing::warn!(error = %e, "Failed to append journal entry");
                }
            });
        }
    }

    fn on_node_failed(&self, node_id: &str, tool: &str, error: &str) {
        self.table.update(&self.run_id, |run| {
            if let Ok(mut nodes) = run.active_nodes.lock() {
                nodes.retain(|n| n.node_id != node_id);
            }
        });

        let journal = self.journal.as_ref().cloned();
        let entry = crate::journal::JournalEntry {
            run_id: self.run_id,
            session_id: self.session_id.clone(),
            namespace: self.namespace.clone(),
            event_type: "node_failed".into(),
            payload: serde_json::json!({
                "node_id": node_id,
                "tool": tool,
                "error": error,
            }),
        };
        if let Some(journal) = journal {
            tokio::spawn(async move {
                if let Err(e) = journal.append(entry).await {
                    tracing::warn!(error = %e, "Failed to append journal entry");
                }
            });
        }
    }

    fn on_tool_retry(&self, node_id: &str, tool: &str, attempt: u32, error: &str) {
        self.table.update(&self.run_id, |run| {
            if let Ok(mut nodes) = run.active_nodes.lock() {
                if let Some(node) = nodes.iter_mut().find(|n| n.node_id == node_id) {
                    node.status = NodeStatus::Retrying { attempt, max: 0 };
                }
            }
        });

        let journal = self.journal.as_ref().cloned();
        let entry = crate::journal::JournalEntry {
            run_id: self.run_id,
            session_id: self.session_id.clone(),
            namespace: self.namespace.clone(),
            event_type: "tool_retry".into(),
            payload: serde_json::json!({
                "node_id": node_id,
                "tool": tool,
                "attempt": attempt,
                "error": error,
            }),
        };
        if let Some(journal) = journal {
            tokio::spawn(async move {
                if let Err(e) = journal.append(entry).await {
                    tracing::warn!(error = %e, "Failed to append journal entry");
                }
            });
        }
    }
}

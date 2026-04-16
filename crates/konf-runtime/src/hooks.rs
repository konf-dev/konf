//! Runtime hooks — connects the substrate executor to the process table.
//!
//! Implements `konflux_substrate::hooks::EventRecorder` to update the
//! process table and event journal in real-time during workflow execution.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use konflux_substrate::hooks::{EventRecorder, ExecutorEvent};

use crate::error::RunId;
use crate::event_bus::{RunEvent, RunEventBus};
use crate::interaction::{Interaction, InteractionKind, InteractionStatus};
use crate::journal::{JournalEntry, JournalStore};
use crate::process::{ActiveNode, NodeStatus, ProcessTable};
use crate::scope::Actor;

/// Hooks implementation that updates the process table, the event bus, and
/// the journal in real time during workflow execution.
pub struct RuntimeHooks {
    pub run_id: RunId,
    pub namespace: String,
    pub session_id: String,
    pub table: Arc<ProcessTable>,
    pub journal: Option<Arc<dyn JournalStore>>,
    pub event_bus: Arc<RunEventBus>,
    pub actor: Actor,
    pub trace_id: Uuid,
}

impl RuntimeHooks {
    fn build_node_interaction_entry(
        &self,
        node_id: &str,
        tool: &str,
        phase: &str,
        status: InteractionStatus,
        mut attributes: serde_json::Map<String, Value>,
    ) -> JournalEntry {
        attributes.insert("node_id".into(), Value::String(node_id.to_string()));
        attributes.insert("tool".into(), Value::String(tool.to_string()));
        attributes.insert("phase".into(), Value::String(phase.to_string()));

        let interaction = Interaction {
            id: Uuid::new_v4(),
            parent_id: None,
            trace_id: self.trace_id,
            run_id: Some(self.run_id),
            node_id: Some(node_id.to_string()),
            actor: self.actor.clone(),
            namespace: self.namespace.clone(),
            target: format!("node:{node_id}"),
            kind: InteractionKind::NodeLifecycle,
            attributes: Value::Object(attributes),
            edge_rules_fired: Vec::new(),
            status,
            summary: None,
            timestamp: Utc::now(),
            step_index: 0,
            stream_id: String::new(),
            state_before_hash: None,
            state_after_hash: None,
            references: Vec::new(),
            in_reply_to: None,
        };

        JournalEntry {
            run_id: Some(self.run_id),
            session_id: self.session_id.clone(),
            namespace: self.namespace.clone(),
            event_type: "interaction".into(),
            payload: interaction.to_json(),
            valid_to: None,
            idempotency_key: None,
        }
    }

    fn spawn_journal_append(&self, entry: JournalEntry) {
        let journal = match self.journal.as_ref().cloned() {
            Some(j) => j,
            None => return,
        };
        let event_bus = self.event_bus.clone();
        let event_type = entry.event_type.clone();
        let namespace = entry.namespace.clone();
        let run_id_for_event = entry.run_id.unwrap_or(uuid::Uuid::nil());
        tokio::spawn(async move {
            match journal.append(entry).await {
                Ok(sequence) => {
                    event_bus.emit(RunEvent::JournalAppended {
                        sequence,
                        event_type,
                        namespace,
                        run_id: run_id_for_event,
                    });
                }
                Err(e) => {
                    tracing::warn!(error = %e, "Failed to append journal entry");
                }
            }
        });
    }
}

impl EventRecorder for RuntimeHooks {
    fn on_event(&self, event: ExecutorEvent<'_>) {
        match event {
            ExecutorEvent::NodeStarted { node_id, tool } => {
                let now = Utc::now();
                let node = ActiveNode {
                    node_id: node_id.to_string(),
                    tool_name: tool.to_string(),
                    started_at: now,
                    status: NodeStatus::Running,
                };

                self.table.update(&self.run_id, |run| {
                    if let Ok(mut nodes) = run.active_nodes.lock() {
                        nodes.push(node.clone());
                    }
                });

                self.event_bus.emit(RunEvent::NodeStart {
                    run_id: self.run_id,
                    node_id: node_id.to_string(),
                    tool: tool.to_string(),
                    at: now,
                });

                let entry = self.build_node_interaction_entry(
                    node_id,
                    tool,
                    "start",
                    InteractionStatus::Pending,
                    serde_json::Map::new(),
                );
                self.spawn_journal_append(entry);
            }

            ExecutorEvent::NodeCompleted {
                node_id,
                tool,
                duration_ms,
                ..
            } => {
                let now = Utc::now();
                self.table.update(&self.run_id, |run| {
                    if let Ok(mut nodes) = run.active_nodes.lock() {
                        nodes.retain(|n| n.node_id != node_id);
                    }
                    run.steps_executed.fetch_add(1, Ordering::Relaxed);
                });

                self.event_bus.emit(RunEvent::NodeEnd {
                    run_id: self.run_id,
                    node_id: node_id.to_string(),
                    status: NodeStatus::Completed { duration_ms },
                    at: now,
                });

                let mut attrs = serde_json::Map::new();
                attrs.insert("duration_ms".into(), Value::from(duration_ms));
                let entry = self.build_node_interaction_entry(
                    node_id,
                    tool,
                    "end",
                    InteractionStatus::Ok,
                    attrs,
                );
                self.spawn_journal_append(entry);
            }

            ExecutorEvent::NodeFailed {
                node_id,
                tool,
                error,
            } => {
                let now = Utc::now();
                self.table.update(&self.run_id, |run| {
                    if let Ok(mut nodes) = run.active_nodes.lock() {
                        nodes.retain(|n| n.node_id != node_id);
                    }
                });

                self.event_bus.emit(RunEvent::NodeEnd {
                    run_id: self.run_id,
                    node_id: node_id.to_string(),
                    status: NodeStatus::Failed {
                        error: error.to_string(),
                    },
                    at: now,
                });

                let entry = self.build_node_interaction_entry(
                    node_id,
                    tool,
                    "failed",
                    InteractionStatus::Failed {
                        error: error.to_string(),
                    },
                    serde_json::Map::new(),
                );
                self.spawn_journal_append(entry);
            }

            ExecutorEvent::ToolRetry {
                node_id,
                tool,
                attempt,
                error,
            } => {
                self.table.update(&self.run_id, |run| {
                    if let Ok(mut nodes) = run.active_nodes.lock() {
                        if let Some(node) = nodes.iter_mut().find(|n| n.node_id == node_id) {
                            node.status = NodeStatus::Retrying { attempt, max: 0 };
                        }
                    }
                });

                let mut attrs = serde_json::Map::new();
                attrs.insert("attempt".into(), Value::from(attempt));
                attrs.insert("error".into(), Value::String(error.to_string()));
                let entry = self.build_node_interaction_entry(
                    node_id,
                    tool,
                    "retry",
                    InteractionStatus::Pending,
                    attrs,
                );
                self.spawn_journal_append(entry);
            }
        }
    }
}

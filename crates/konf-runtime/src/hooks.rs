//! Runtime hooks — connects the konflux executor to the process table.
//!
//! Implements `konflux::hooks::ExecutionHooks` to update the process table
//! and event journal in real-time during workflow execution.

use std::sync::atomic::Ordering;
use std::sync::Arc;

use chrono::Utc;
use serde_json::Value;
use uuid::Uuid;

use konflux::hooks::ExecutionHooks;

use crate::error::RunId;
use crate::event_bus::{RunEvent, RunEventBus};
use crate::interaction::{Interaction, InteractionKind, InteractionStatus};
use crate::journal::{JournalEntry, JournalStore};
use crate::process::{ActiveNode, NodeStatus, ProcessTable};
use crate::scope::Actor;

/// Hooks implementation that updates the process table, the event bus, and
/// the journal in real time during workflow execution.
///
/// # Node lifecycle emission (B4)
///
/// Prior to the Stigmergic Engine v0 work, [`RunEvent::NodeStart`] and
/// [`RunEvent::NodeEnd`] were defined on the bus but never emitted. The
/// hook now emits them at every node lifecycle transition so SSE monitor
/// subscribers observe workflow-internal motion.
///
/// # Node interaction fidelity (F1, Phase F fix)
///
/// Each hook also appends an [`Interaction`]-shaped `JournalEntry` with
/// `event_type = "interaction"` and `kind = NodeLifecycle`, matching the
/// envelope emitted by [`crate::Runtime::invoke_tool`] for tool dispatch.
/// This closes the asymmetry flagged in Gemini's Phase E audit: the
/// interaction graph now has uniform fidelity across tool dispatches and
/// workflow node transitions.
pub struct RuntimeHooks {
    pub run_id: RunId,
    pub namespace: String,
    pub session_id: String,
    pub table: Arc<ProcessTable>,
    pub journal: Option<Arc<dyn JournalStore>>,
    /// Event bus handle for emitting [`RunEvent::NodeStart`] /
    /// [`RunEvent::NodeEnd`]. Always present (the runtime owns one
    /// unconditionally), but stored as `Arc` so cloning is cheap.
    pub event_bus: Arc<RunEventBus>,
    /// Actor identity — needed for [`Interaction::actor`] on every
    /// node-lifecycle record. Inline for multi-tenant self-auditability.
    pub actor: Actor,
    /// Trace id — propagated from the enclosing
    /// [`crate::ExecutionContext`]. Required (not `Option`) because
    /// `ExecutionContext` guarantees it is set at the transport
    /// boundary before any dispatch begins. R2 closed the per-call
    /// minting bug by making this a substrate invariant.
    pub trace_id: Uuid,
}

impl RuntimeHooks {
    /// Build an Interaction for a node lifecycle transition and wrap it
    /// in a `JournalEntry` suitable for fire-and-forget append.
    ///
    /// `phase` is one of `"start"`, `"end"`, `"failed"`, `"retry"` — the
    /// discriminator tests filter on to distinguish lifecycle transitions.
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
        };

        JournalEntry {
            run_id: self.run_id,
            session_id: self.session_id.clone(),
            namespace: self.namespace.clone(),
            event_type: "interaction".into(),
            payload: interaction.to_json(),
        }
    }

    fn spawn_journal_append(&self, entry: JournalEntry) {
        let journal = match self.journal.as_ref().cloned() {
            Some(j) => j,
            None => return,
        };
        tokio::spawn(async move {
            if let Err(e) = journal.append(entry).await {
                tracing::warn!(error = %e, "Failed to append journal entry");
            }
        });
    }
}

impl ExecutionHooks for RuntimeHooks {
    fn on_node_start(&self, node_id: &str, tool: &str) {
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

        // B4: emit on the event bus so SSE monitor + recorder subscribers
        // see this transition. Fire-and-forget; emit() never blocks.
        self.event_bus.emit(RunEvent::NodeStart {
            run_id: self.run_id,
            node_id: node_id.to_string(),
            tool: tool.to_string(),
            at: now,
        });

        // F1: emit the Interaction-shaped journal entry so node lifecycle
        // events live in the same graph as tool dispatches.
        let entry = self.build_node_interaction_entry(
            node_id,
            tool,
            "start",
            InteractionStatus::Pending,
            serde_json::Map::new(),
        );
        self.spawn_journal_append(entry);
    }

    fn on_node_complete(&self, node_id: &str, tool: &str, duration_ms: u64, _output: &Value) {
        let now = Utc::now();
        self.table.update(&self.run_id, |run| {
            if let Ok(mut nodes) = run.active_nodes.lock() {
                nodes.retain(|n| n.node_id != node_id);
            }
            run.steps_executed.fetch_add(1, Ordering::Relaxed);
        });

        // B4: emit NodeEnd with terminal Completed status.
        self.event_bus.emit(RunEvent::NodeEnd {
            run_id: self.run_id,
            node_id: node_id.to_string(),
            status: NodeStatus::Completed { duration_ms },
            at: now,
        });

        // F1: emit the Interaction-shaped journal entry.
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

    fn on_node_failed(&self, node_id: &str, tool: &str, error: &str) {
        let now = Utc::now();
        self.table.update(&self.run_id, |run| {
            if let Ok(mut nodes) = run.active_nodes.lock() {
                nodes.retain(|n| n.node_id != node_id);
            }
        });

        // B4: emit NodeEnd with terminal Failed status.
        self.event_bus.emit(RunEvent::NodeEnd {
            run_id: self.run_id,
            node_id: node_id.to_string(),
            status: NodeStatus::Failed {
                error: error.to_string(),
            },
            at: now,
        });

        // F1: emit the Interaction-shaped journal entry.
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

    fn on_tool_retry(&self, node_id: &str, tool: &str, attempt: u32, error: &str) {
        self.table.update(&self.run_id, |run| {
            if let Ok(mut nodes) = run.active_nodes.lock() {
                if let Some(node) = nodes.iter_mut().find(|n| n.node_id == node_id) {
                    node.status = NodeStatus::Retrying { attempt, max: 0 };
                }
            }
        });

        // F1: retry is still a NodeLifecycle transition (status stays
        // Pending — the node hasn't terminated). The attempt count lives
        // in attributes.
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

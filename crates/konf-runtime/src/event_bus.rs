//! Runtime event bus — broadcast channel for real-time monitoring.
//!
//! Every mutating operation in the runtime — workflow lifecycle events,
//! node transitions, text deltas, journal appends, scheduler fires,
//! runner-intent replays — emits a [`RunEvent`] on a single
//! [`tokio::sync::broadcast`] channel owned by the [`crate::Runtime`].
//!
//! Subscribers (the HTTP monitor SSE endpoint, tests, local TUIs) call
//! [`RunEventBus::subscribe`] to get a receiver and consume events
//! lazily. The broadcast channel is bounded; slow subscribers receive
//! [`tokio::sync::broadcast::error::RecvError::Lagged`] when they fall
//! behind and are expected to refetch state from the REST API before
//! resuming the stream.
//!
//! # Why broadcast and not mpsc?
//!
//! Multiple subscribers (TUIs, dashboards, tests) may observe the same
//! stream concurrently. `broadcast` fans out to all active receivers
//! with O(1) sender cost and never blocks the emitter. Emission is
//! fire-and-forget — failures to deliver (channel closed, no
//! subscribers, lagged receivers) are swallowed at the send site, which
//! matches the design goal that runtime mutations must never be delayed
//! by observability.

use chrono::{DateTime, Utc};
use serde::Serialize;
use tokio::sync::broadcast;

use crate::error::RunId;
use crate::process::NodeStatus;
use crate::scheduler::JobId;

/// Default broadcast channel capacity. Large enough to absorb brief
/// bursts from a few hundred concurrent workflow runs; slow subscribers
/// that fall further behind will see `Lagged` and refetch.
pub const DEFAULT_EVENT_BUS_CAPACITY: usize = 1024;

/// A real-time event emitted by the runtime for monitoring purposes.
///
/// Events are flat and serializable. The `type` tag is rendered as a
/// snake_case discriminant so an SSE consumer can route on `event:` name
/// without inspecting the payload.
#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum RunEvent {
    /// A workflow run entered [`crate::RunStatus::Running`].
    RunStarted {
        run_id: RunId,
        workflow_id: String,
        namespace: String,
        parent_id: Option<RunId>,
        started_at: DateTime<Utc>,
    },

    /// A node inside a workflow started executing.
    NodeStart {
        run_id: RunId,
        node_id: String,
        tool: String,
        at: DateTime<Utc>,
    },

    /// A node finished (success, retry, or failure).
    NodeEnd {
        run_id: RunId,
        node_id: String,
        status: NodeStatus,
        at: DateTime<Utc>,
    },

    /// A streaming text delta from an LLM or streaming tool.
    TextDelta {
        run_id: RunId,
        node_id: String,
        delta: String,
    },

    /// A workflow run reached [`crate::RunStatus::Completed`].
    RunCompleted {
        run_id: RunId,
        duration_ms: u64,
    },

    /// A workflow run reached [`crate::RunStatus::Failed`].
    RunFailed {
        run_id: RunId,
        duration_ms: u64,
        error: String,
    },

    /// A workflow run was cancelled.
    RunCancelled {
        run_id: RunId,
        reason: String,
    },

    /// A single tool was invoked via [`crate::Runtime::invoke_tool`]
    /// (outside of a workflow). Used by the MCP HTTP transport so direct
    /// tool calls show up in the monitor stream alongside workflow runs.
    ToolInvoked {
        tool: String,
        namespace: String,
        at: DateTime<Utc>,
        success: bool,
    },

    /// A scheduler timer fired and the workflow was spawned.
    ScheduleFired {
        job_id: JobId,
        workflow: String,
        namespace: String,
        fired_at: DateTime<Utc>,
    },

    /// A scheduler timer fired but the workflow was not in the registry.
    ScheduleFailed {
        job_id: JobId,
        workflow: String,
        reason: String,
    },

    /// An unterminated runner intent was replayed on boot.
    IntentReplayed {
        run_id: String,
        workflow: String,
        replay_count: u32,
    },

    /// The journal appended a new entry. The SSE endpoint can use this
    /// as a trigger to refetch audit state incrementally.
    JournalAppended {
        sequence: u64,
        event_type: String,
        namespace: String,
        run_id: RunId,
    },
}

impl RunEvent {
    /// Short snake_case discriminator matching the `#[serde(tag)]` — the
    /// SSE endpoint uses this as the `event:` field name so HTTP clients
    /// can multiplex without parsing JSON.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::RunStarted { .. } => "run_started",
            Self::NodeStart { .. } => "node_start",
            Self::NodeEnd { .. } => "node_end",
            Self::TextDelta { .. } => "text_delta",
            Self::RunCompleted { .. } => "run_completed",
            Self::RunFailed { .. } => "run_failed",
            Self::RunCancelled { .. } => "run_cancelled",
            Self::ToolInvoked { .. } => "tool_invoked",
            Self::ScheduleFired { .. } => "schedule_fired",
            Self::ScheduleFailed { .. } => "schedule_failed",
            Self::IntentReplayed { .. } => "intent_replayed",
            Self::JournalAppended { .. } => "journal_appended",
        }
    }

    /// Return the namespace this event belongs to, for prefix filtering.
    /// Events that are not namespace-scoped (e.g. `IntentReplayed`) return
    /// an empty string which matches the empty prefix only.
    pub fn namespace(&self) -> &str {
        match self {
            Self::RunStarted { namespace, .. }
            | Self::ToolInvoked { namespace, .. }
            | Self::ScheduleFired { namespace, .. }
            | Self::JournalAppended { namespace, .. } => namespace,
            _ => "",
        }
    }
}

/// Shared broadcast channel for runtime events. Cheap to clone (contains
/// an [`Arc`] internally via `broadcast::Sender`).
#[derive(Clone)]
pub struct RunEventBus {
    tx: broadcast::Sender<RunEvent>,
}

impl RunEventBus {
    /// Create a bus with the given in-flight capacity.
    pub fn with_capacity(capacity: usize) -> Self {
        let (tx, _) = broadcast::channel(capacity);
        Self { tx }
    }

    /// Subscribe a new receiver. Returns events emitted after the
    /// subscription point; past events are not replayed.
    pub fn subscribe(&self) -> broadcast::Receiver<RunEvent> {
        self.tx.subscribe()
    }

    /// Emit an event. Never blocks — if there are no subscribers or the
    /// send fails, the event is dropped silently.
    pub fn emit(&self, event: RunEvent) {
        let _ = self.tx.send(event);
    }

    /// Current subscriber count. Useful for tests.
    pub fn receiver_count(&self) -> usize {
        self.tx.receiver_count()
    }
}

impl Default for RunEventBus {
    fn default() -> Self {
        Self::with_capacity(DEFAULT_EVENT_BUS_CAPACITY)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn subscribe_receives_emitted_events() {
        let bus = RunEventBus::with_capacity(16);
        let mut rx = bus.subscribe();
        bus.emit(RunEvent::RunCompleted {
            run_id: RunId::new_v4(),
            duration_ms: 42,
        });
        let ev = rx.recv().await.unwrap();
        assert_eq!(ev.kind(), "run_completed");
    }

    #[tokio::test]
    async fn multiple_subscribers_all_receive() {
        let bus = RunEventBus::with_capacity(16);
        let mut rx1 = bus.subscribe();
        let mut rx2 = bus.subscribe();
        let run_id = RunId::new_v4();
        bus.emit(RunEvent::RunCancelled {
            run_id,
            reason: "test".into(),
        });
        assert_eq!(rx1.recv().await.unwrap().kind(), "run_cancelled");
        assert_eq!(rx2.recv().await.unwrap().kind(), "run_cancelled");
    }

    #[tokio::test]
    async fn emit_without_subscribers_does_not_error() {
        let bus = RunEventBus::with_capacity(4);
        bus.emit(RunEvent::RunCompleted {
            run_id: RunId::new_v4(),
            duration_ms: 0,
        });
    }

    #[tokio::test]
    async fn lagged_subscriber_receives_lagged_error() {
        let bus = RunEventBus::with_capacity(2);
        let mut rx = bus.subscribe();
        // Emit 5 events into a capacity-2 channel — the oldest are
        // dropped. The receiver should see Lagged on its next recv.
        for _ in 0..5 {
            bus.emit(RunEvent::RunCompleted {
                run_id: RunId::new_v4(),
                duration_ms: 0,
            });
        }
        let err = rx.recv().await.unwrap_err();
        assert!(matches!(
            err,
            tokio::sync::broadcast::error::RecvError::Lagged(_)
        ));
    }

    #[tokio::test]
    async fn namespace_filter_helper() {
        let ev = RunEvent::RunStarted {
            run_id: RunId::new_v4(),
            workflow_id: "w".into(),
            namespace: "konf:assistant:bert".into(),
            parent_id: None,
            started_at: Utc::now(),
        };
        assert_eq!(ev.namespace(), "konf:assistant:bert");
        assert!(ev.namespace().starts_with("konf:assistant"));
    }
}

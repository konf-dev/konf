//! Shared run registry: `RunId → RunRecord` plus wait/cancel plumbing.
//!
//! The registry is the single source of truth about every run this process
//! has started via any runner backend. A run's lifecycle is always the
//! same regardless of backend:
//!
//! 1. `spawn()` inserts a `Pending` record with the id.
//! 2. Backend flips the record to `Running` right before the work begins.
//! 3. When the work finishes, the backend stores the terminal state
//!    (`Succeeded`, `Failed`, or `Cancelled`) and notifies any `wait` callers.
//! 4. `cancel()` calls the backend's abort hook and transitions the record
//!    to `Cancelled`.
//!
//! All state transitions go through the registry, so callers always see a
//! consistent view regardless of which backend is in use.

use std::sync::Arc;

use chrono::{DateTime, Utc};
use papaya::HashMap as PapayaMap;
use serde::Serialize;
use serde_json::Value;
use tokio::sync::{Notify, RwLock};

/// Opaque handle to a run. Currently a UUID v4 string; callers should treat
/// it as an opaque token and never parse it.
pub type RunId = String;

/// Lifecycle state of a single run.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "snake_case", tag = "state")]
pub enum RunState {
    /// The run has been registered but the backend has not yet started it.
    Pending,
    /// The backend is actively executing the run.
    Running,
    /// Terminal success. Carries the result value produced by the workflow.
    Succeeded {
        /// JSON result returned by the workflow.
        result: Value,
    },
    /// Terminal failure. Carries a human-readable error message.
    Failed {
        /// The error message the workflow tool returned.
        error: String,
    },
    /// Terminal cancellation. The run was aborted before producing a result.
    Cancelled,
}

impl RunState {
    /// True if the run has reached a terminal state (succeeded, failed, or
    /// cancelled) and will not transition further.
    pub fn is_terminal(&self) -> bool {
        matches!(
            self,
            RunState::Succeeded { .. } | RunState::Failed { .. } | RunState::Cancelled
        )
    }

    /// Short human-readable state label, independent of the payload.
    pub fn label(&self) -> &'static str {
        match self {
            RunState::Pending => "pending",
            RunState::Running => "running",
            RunState::Succeeded { .. } => "succeeded",
            RunState::Failed { .. } => "failed",
            RunState::Cancelled => "cancelled",
        }
    }
}

/// Public record for a run. Serializable so it can be returned from tools.
///
/// `state` is flattened at the top level (via the enum's `#[serde(tag =
/// "state")]`) so the JSON shape stays flat: `{id, workflow, backend,
/// state, result?, error?, ...}`. This avoids one level of nesting in
/// tool outputs.
#[derive(Debug, Clone, Serialize)]
pub struct RunRecord {
    /// Stable run identifier.
    pub id: RunId,
    /// Which workflow was requested.
    pub workflow: String,
    /// Which backend is managing the run.
    pub backend: String,
    /// Current state (flattened: `state` key + optional `result`/`error`).
    #[serde(flatten)]
    pub state: RunState,
    /// Time the run entered the registry.
    pub created_at: DateTime<Utc>,
    /// Time the backend moved the run to `Running`, if applicable.
    pub started_at: Option<DateTime<Utc>>,
    /// Time the run reached a terminal state.
    pub finished_at: Option<DateTime<Utc>>,
}

/// Internal slot tracking a single run, shared between the spawn path, the
/// status/wait path, and the cancel path.
pub(crate) struct RunSlot {
    pub(crate) record: RwLock<RunRecord>,
    pub(crate) done: Notify,
    /// Best-effort cancel hook; backends register their own abort logic here.
    /// Calling it must be idempotent.
    pub(crate) cancel_hook: std::sync::Mutex<Option<Box<dyn FnOnce() + Send + 'static>>>,
}

impl RunSlot {
    pub(crate) fn new(record: RunRecord) -> Self {
        Self {
            record: RwLock::new(record),
            done: Notify::new(),
            cancel_hook: std::sync::Mutex::new(None),
        }
    }
}

/// Shared registry of live and completed runs.
///
/// Cheap to clone (`Arc` inside). Backends, tools, and tests all hold their
/// own clone and coordinate through it.
#[derive(Clone, Default)]
pub struct RunRegistry {
    inner: Arc<PapayaMap<RunId, Arc<RunSlot>>>,
}

impl RunRegistry {
    /// Create an empty registry.
    pub fn new() -> Self {
        Self::default()
    }

    /// Generate a fresh, globally unique run id.
    pub fn fresh_id() -> RunId {
        uuid::Uuid::new_v4().to_string()
    }

    /// Insert a new pending run. Returns the shared slot handle so the
    /// backend can update its state as the run progresses.
    pub(crate) fn insert_pending(&self, workflow: &str, backend: &str) -> (RunId, Arc<RunSlot>) {
        let id = Self::fresh_id();
        self.insert_pending_with_id(id, workflow, backend)
    }

    /// Insert a new pending run with an explicit run id. Used by replay
    /// from persisted intents so the same id survives across restarts
    /// (preserving external references like TUI bookmarks).
    pub(crate) fn insert_pending_with_id(
        &self,
        id: RunId,
        workflow: &str,
        backend: &str,
    ) -> (RunId, Arc<RunSlot>) {
        let record = RunRecord {
            id: id.clone(),
            workflow: workflow.to_string(),
            backend: backend.to_string(),
            state: RunState::Pending,
            created_at: Utc::now(),
            started_at: None,
            finished_at: None,
        };
        let slot = Arc::new(RunSlot::new(record));
        self.inner.pin().insert(id.clone(), slot.clone());
        (id, slot)
    }

    /// Look up a slot by id (internal — outside callers use `record`).
    pub(crate) fn slot(&self, id: &RunId) -> Option<Arc<RunSlot>> {
        self.inner.pin().get(id).cloned()
    }

    /// Public read-only view of a run record.
    pub async fn record(&self, id: &RunId) -> Option<RunRecord> {
        let slot = self.slot(id)?;
        let guard = slot.record.read().await;
        Some(guard.clone())
    }

    /// Wait for the run to reach a terminal state. If it's already terminal,
    /// returns immediately. Returns `None` if the id does not exist.
    ///
    /// The caller controls timeout via `tokio::time::timeout` — keeping the
    /// timeout policy out of the registry keeps this surface minimal.
    pub async fn wait_terminal(&self, id: &RunId) -> Option<RunRecord> {
        let slot = self.slot(id)?;
        loop {
            let current = { slot.record.read().await.clone() };
            if current.state.is_terminal() {
                return Some(current);
            }
            slot.done.notified().await;
        }
    }

    /// Transition a slot to `Running` and stamp `started_at`.
    pub(crate) async fn mark_running(&self, id: &RunId) {
        if let Some(slot) = self.slot(id) {
            let mut rec = slot.record.write().await;
            if matches!(rec.state, RunState::Pending) {
                rec.state = RunState::Running;
                rec.started_at = Some(Utc::now());
            }
        }
    }

    /// Store a terminal state on a slot and notify anyone waiting.
    pub(crate) async fn mark_terminal(&self, id: &RunId, state: RunState) {
        if let Some(slot) = self.slot(id) {
            {
                let mut rec = slot.record.write().await;
                if !rec.state.is_terminal() {
                    rec.state = state;
                    rec.finished_at = Some(Utc::now());
                }
            }
            slot.done.notify_waiters();
        }
    }

    /// Invoke the registered cancel hook (if any) and transition the slot
    /// to `Cancelled`. Returns true if a cancel hook was actually invoked.
    pub(crate) async fn cancel(&self, id: &RunId) -> bool {
        let Some(slot) = self.slot(id) else {
            return false;
        };
        let hook = {
            let Ok(mut lock) = slot.cancel_hook.lock() else {
                return false;
            };
            lock.take()
        };
        let had_hook = hook.is_some();
        if let Some(h) = hook {
            h();
        }
        self.mark_terminal(id, RunState::Cancelled).await;
        had_hook
    }

    /// Register a cancel hook on a slot. Backends call this after they
    /// spawn the underlying task so `runner:cancel` has something to call.
    pub(crate) fn register_cancel_hook<F: FnOnce() + Send + 'static>(&self, id: &RunId, hook: F) {
        if let Some(slot) = self.slot(id) {
            if let Ok(mut lock) = slot.cancel_hook.lock() {
                *lock = Some(Box::new(hook));
            }
        }
    }

    /// Approximate number of tracked runs (may be slightly stale on
    /// concurrent modifications).
    pub fn len(&self) -> usize {
        self.inner.pin().len()
    }

    /// True if no runs are tracked.
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

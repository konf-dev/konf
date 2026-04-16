//! Execution context — runtime state carried through dispatch.
//!
//! Complements [`crate::ExecutionScope`]: where `ExecutionScope` declares
//! *what an actor is allowed to do* (immutable config), `ExecutionContext`
//! carries *what the current dispatch is doing* (mutable runtime state).
//!
//! Splitting these concerns closes a correctness bug in the v0 Stigmergic
//! Engine work (finding C3 in the Phase E audit): `trace_id` was a field
//! on `ExecutionScope` but the dispatch path could only take `&scope`,
//! which meant a fresh `trace_id` was minted per-call whenever the scope
//! had none. The causation DAG the Stigmergic Engine's whole design rests
//! on was unreconstructible as a result.
//!
//! With this split:
//!
//! - `trace_id` is a **required** field on `ExecutionContext` (not
//!   `Option<Uuid>`), minted exactly once at the transport boundary
//!   (HTTP, MCP, runner spawn). It cannot be absent at dispatch time.
//! - `parent_interaction_id` tracks the direct causation ancestor and
//!   is threaded through nested dispatches. Every Interaction's
//!   `parent_id` field is populated from here.
//! - `session_id` identifies the current session (was previously
//!   passed as a separate parameter alongside scope).
//!
//! # Lifecycle
//!
//! ```text
//!  HTTP /v1/chat turn begins
//!        │
//!        ▼
//!  ExecutionContext::new_root(session_id)   ← mints fresh trace_id
//!        │
//!        ▼
//!  Runtime::start(workflow, input, scope, ctx)
//!        │
//!        │   (inside the dispatch, each tool call emits an Interaction
//!        │    whose trace_id = ctx.trace_id, and the outer dispatch's
//!        │    interaction id becomes the next call's parent_interaction_id
//!        │    via ExecutionContext::child)
//!        ▼
//!  …workflow completes…
//! ```

use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Runtime state for a dispatch. See module docs for lifecycle.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutionContext {
    /// Groups related interactions across dispatch + spawn boundaries.
    /// OpenTelemetry `trace_id` analog. **Required** — minted at the
    /// transport boundary and propagated by `child()`; never `None` at
    /// dispatch time.
    pub trace_id: Uuid,

    /// The direct causation ancestor's Interaction id. `None` only at
    /// the absolute root of a turn (first dispatch). Every nested
    /// dispatch sets this to its parent's interaction id via
    /// [`ExecutionContext::child`].
    pub parent_interaction_id: Option<Uuid>,

    /// Session identifier. Typically the HTTP session cookie value or
    /// the MCP session id. Multiple traces can share a session; a trace
    /// never spans sessions.
    pub session_id: String,
}

impl ExecutionContext {
    /// Construct a root context. Mints a fresh `trace_id` with
    /// `Uuid::new_v4()` (CSPRNG — acceptable entropy for a span id).
    ///
    /// Use at transport boundaries:
    /// - HTTP `/v1/chat` handler at turn start
    /// - MCP session setup at session start
    /// - Any other "first dispatch after a trust boundary"
    pub fn new_root(session_id: impl Into<String>) -> Self {
        Self {
            trace_id: Uuid::new_v4(),
            parent_interaction_id: None,
            session_id: session_id.into(),
        }
    }

    /// Construct a root context with an explicit `trace_id`. Use when a
    /// caller external to konf-runtime has already minted a trace id
    /// (e.g. propagating a trace-id HTTP header from an upstream
    /// service) and wants to preserve it through dispatch.
    pub fn with_trace(trace_id: Uuid, session_id: impl Into<String>) -> Self {
        Self {
            trace_id,
            parent_interaction_id: None,
            session_id: session_id.into(),
        }
    }

    /// Derive a child context for a nested dispatch.
    ///
    /// The child inherits `trace_id` unchanged (the whole point of
    /// trace propagation). `parent_interaction_id` becomes the id of
    /// the dispatch that is calling `child()`. Session can be changed
    /// (e.g. for `runner:spawn` into a different session scope) or
    /// kept.
    pub fn child(&self, parent_interaction_id: Uuid, session_id: Option<String>) -> Self {
        Self {
            trace_id: self.trace_id,
            parent_interaction_id: Some(parent_interaction_id),
            session_id: session_id.unwrap_or_else(|| self.session_id.clone()),
        }
    }
}

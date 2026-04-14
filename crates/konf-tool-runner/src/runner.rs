//! The `Runner` trait that every backend implements.
//!
//! Kept intentionally tiny. Each backend owns the mapping between
//! `WorkflowSpec` and its own execution substrate (tokio task, systemd unit,
//! Docker container, …) and reports state through the shared `RunRegistry`.

use async_trait::async_trait;
use konf_runtime::scope::ExecutionScope;
use konf_runtime::ExecutionContext;
use serde_json::Value;

use crate::error::RunnerError;
use crate::registry::{RunId, RunRegistry};

/// What a caller is asking the runner to run.
///
/// R1 (Phase F2 remediation): the spec carries the **parent's** scope
/// and runtime context, not hardcoded defaults. The runner uses these
/// as the parent in the capability-lattice's attenuation: the spawned
/// workflow's effective scope is a subset of the parent. This closes
/// finding C1 from the Phase E audit — previously the inline runner
/// hand-built a `ToolContext` with `capabilities: vec!["*".into()]`,
/// bypassing the lattice entirely.
#[derive(Debug, Clone)]
pub struct WorkflowSpec {
    /// Registered workflow name. Resolved to `workflow:<name>` in the engine
    /// registry at spawn time.
    pub workflow: String,
    /// Input payload forwarded as the workflow tool's JSON argument.
    pub input: Value,
    /// The caller's [`ExecutionScope`]. Used as the parent for
    /// attenuation; the spawned workflow cannot grant itself capabilities
    /// the parent lacked.
    pub parent_scope: ExecutionScope,
    /// The caller's [`ExecutionContext`]. The runner derives a child
    /// context so the spawn inherits `trace_id` and records its
    /// `parent_interaction_id`, preserving the causation DAG across
    /// the spawn boundary.
    pub parent_ctx: ExecutionContext,
}

/// A runner backend. Backends are constructed with a registry handle and
/// whatever they need to resolve workflows (usually an `Arc<Runtime>`).
#[async_trait]
pub trait Runner: Send + Sync {
    /// Short identifier for this backend. Appears in [`crate::registry::RunRecord::backend`].
    fn name(&self) -> &'static str;

    /// Shared registry this backend reports state to. All backends hold a
    /// clone of the same registry so `runner:status`, `runner:wait`, and
    /// `runner:cancel` see a single consistent view regardless of which
    /// backend started the run.
    fn registry(&self) -> &RunRegistry;

    /// Spawn a new run. Returns as soon as the run is registered and the
    /// backend has kicked off the work. The returned [`RunId`] can be fed
    /// to the other trait methods or exposed via `runner:spawn`.
    async fn spawn(&self, spec: WorkflowSpec) -> Result<RunId, RunnerError>;

    /// Cancel a run by id. Returns true if a cancel hook was invoked.
    /// Default implementation delegates to the registry's cancel hook.
    async fn cancel(&self, id: &RunId) -> Result<bool, RunnerError> {
        Ok(self.registry().cancel(id).await)
    }
}

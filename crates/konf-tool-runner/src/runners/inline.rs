//! `InlineRunner` — runs workflows as tokio tasks inside the current process.
//!
//! Behavior:
//!
//! 1. `spawn()` resolves `workflow:<name>` in the runtime's engine registry.
//!    If the workflow doesn't exist, it returns `RunnerError::WorkflowNotFound`.
//! 2. A fresh `RunId` is inserted into the shared registry as `Pending`.
//! 3. A tokio task is launched that:
//!    - Transitions the slot to `Running`.
//!    - Builds a `ToolContext` and invokes the workflow tool.
//!    - Stores the terminal state (`Succeeded` with the JSON result, or
//!      `Failed` with the error message).
//!    - Notifies any `wait_terminal` callers.
//! 4. The task's `AbortHandle` is registered as the slot's cancel hook so
//!    `runner:cancel` can stop in-flight work at the next await point.
//!
//! There is no isolation, no resource limit, no process boundary. Products
//! that need those should graduate to the (not-yet-implemented) systemd or
//! docker backends.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use tracing::{info, warn};

use konf_runtime::Runtime;
use konflux::tool::ToolContext;

use crate::error::RunnerError;
use crate::registry::{RunId, RunRegistry, RunState};
use crate::runner::{Runner, WorkflowSpec};

/// Inline tokio-task runner. Cheap to clone (`Arc` inside).
#[derive(Clone)]
pub struct InlineRunner {
    runtime: Arc<Runtime>,
    registry: RunRegistry,
}

impl InlineRunner {
    /// Build a runner over the given runtime and registry.
    ///
    /// The runner resolves workflows from `runtime.engine().registry()` at
    /// spawn time, so hot-reloads take effect without rebuilding the runner.
    pub fn new(runtime: Arc<Runtime>, registry: RunRegistry) -> Self {
        Self { runtime, registry }
    }

    fn tool_name(workflow: &str) -> String {
        format!("workflow:{workflow}")
    }
}

#[async_trait]
impl Runner for InlineRunner {
    fn name(&self) -> &'static str {
        "inline"
    }

    fn registry(&self) -> &RunRegistry {
        &self.registry
    }

    async fn spawn(&self, spec: WorkflowSpec) -> Result<RunId, RunnerError> {
        let tool_name = Self::tool_name(&spec.workflow);
        let Some(tool) = self.runtime.engine().registry().get(&tool_name) else {
            return Err(RunnerError::WorkflowNotFound(spec.workflow));
        };

        let (run_id, _slot) = self.registry.insert_pending(&spec.workflow, self.name());

        let registry = self.registry.clone();
        let run_id_for_task = run_id.clone();
        let workflow = spec.workflow.clone();
        let input = spec.input;

        let handle = tokio::spawn(async move {
            registry.mark_running(&run_id_for_task).await;

            let ctx = ToolContext {
                capabilities: vec!["*".into()],
                workflow_id: "runner".into(),
                node_id: format!("runner_{workflow}"),
                metadata: HashMap::new(),
            };

            let terminal = match tool.invoke(input, &ctx).await {
                Ok(result) => {
                    info!(run_id = %run_id_for_task, workflow = %workflow, "run succeeded");
                    RunState::Succeeded { result }
                }
                Err(e) => {
                    warn!(run_id = %run_id_for_task, workflow = %workflow, error = %e, "run failed");
                    RunState::Failed {
                        error: e.to_string(),
                    }
                }
            };
            registry.mark_terminal(&run_id_for_task, terminal).await;
        });

        // Register the task's AbortHandle as the slot's cancel hook. The
        // hook is FnOnce so it can only fire once; subsequent cancel calls
        // are no-ops on the already-cancelled run.
        let abort = handle.abort_handle();
        self.registry.register_cancel_hook(&run_id, move || {
            abort.abort();
        });

        Ok(run_id)
    }
}

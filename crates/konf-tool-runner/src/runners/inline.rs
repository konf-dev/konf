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

use konf_runtime::scope::{Actor, ActorRole};
use konf_runtime::{RunnerIntent, RunnerIntentStore, Runtime, TerminalStatus};
use konflux::tool::ToolContext;

use crate::error::RunnerError;
use crate::registry::{RunId, RunRegistry, RunState};
use crate::runner::{Runner, WorkflowSpec};

/// Inline tokio-task runner. Cheap to clone (`Arc` inside).
///
/// If an [`RunnerIntentStore`] is installed, every spawn is persisted to
/// redb before the tokio task starts and marked terminal on completion.
/// This lets `konf-init::boot` replay unterminated intents after a crash.
#[derive(Clone)]
pub struct InlineRunner {
    runtime: Arc<Runtime>,
    registry: RunRegistry,
    intents: Option<Arc<RunnerIntentStore>>,
}

impl InlineRunner {
    /// Build an ephemeral runner with no intent persistence. Spawns made
    /// through this runner are lost on process restart. Use
    /// [`InlineRunner::with_intents`] when a `KonfStorage` is available.
    pub fn new(runtime: Arc<Runtime>, registry: RunRegistry) -> Self {
        Self {
            runtime,
            registry,
            intents: None,
        }
    }

    /// Build a durable runner that persists spawn intents to redb.
    pub fn with_intents(
        runtime: Arc<Runtime>,
        registry: RunRegistry,
        intents: Arc<RunnerIntentStore>,
    ) -> Self {
        Self {
            runtime,
            registry,
            intents: Some(intents),
        }
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
        self.spawn_with_id(None, spec).await
    }
}

impl InlineRunner {
    /// Internal spawn that optionally reuses an existing run id. Called by
    /// the public `spawn` with `None` and by [`InlineRunner::replay`] with
    /// the persisted id.
    async fn spawn_with_id(
        &self,
        existing_id: Option<RunId>,
        spec: WorkflowSpec,
    ) -> Result<RunId, RunnerError> {
        let tool_name = Self::tool_name(&spec.workflow);
        let Some(tool) = self.runtime.engine().registry().get(&tool_name) else {
            return Err(RunnerError::WorkflowNotFound(spec.workflow));
        };

        let (run_id, _slot) = match existing_id {
            Some(id) => self
                .registry
                .insert_pending_with_id(id, &spec.workflow, self.name()),
            None => self.registry.insert_pending(&spec.workflow, self.name()),
        };

        // Persist the spawn intent before starting the task so a crash
        // between here and the task's completion is recoverable on restart.
        if let Some(ref intents) = self.intents {
            let intent = RunnerIntent::new(
                run_id.clone(),
                spec.workflow.clone(),
                spec.input.clone(),
                "konf:runner:inline",
                vec!["*".to_string()],
                Actor {
                    id: "runner".into(),
                    role: ActorRole::System,
                },
                format!("runner:{run_id}"),
            );
            if let Err(e) = intents.insert(intent).await {
                warn!(run_id = %run_id, error = %e, "failed to persist runner intent; continuing without durability");
            }
        }

        let registry = self.registry.clone();
        let run_id_for_task = run_id.clone();
        let workflow = spec.workflow.clone();
        let input = spec.input;
        let intents_for_task = self.intents.clone();

        let handle = tokio::spawn(async move {
            registry.mark_running(&run_id_for_task).await;

            let ctx = ToolContext {
                capabilities: vec!["*".into()],
                workflow_id: "runner".into(),
                node_id: format!("runner_{workflow}"),
                metadata: HashMap::new(),
            };

            let (runner_state, intent_status) = match tool.invoke(input, &ctx).await {
                Ok(result) => {
                    info!(run_id = %run_id_for_task, workflow = %workflow, "run succeeded");
                    (
                        RunState::Succeeded {
                            result: result.clone(),
                        },
                        TerminalStatus::Succeeded,
                    )
                }
                Err(e) => {
                    warn!(run_id = %run_id_for_task, workflow = %workflow, error = %e, "run failed");
                    let msg = e.to_string();
                    (
                        RunState::Failed { error: msg.clone() },
                        TerminalStatus::Failed { error: msg },
                    )
                }
            };
            registry.mark_terminal(&run_id_for_task, runner_state).await;

            if let Some(intents) = intents_for_task {
                if let Err(e) = intents.mark_terminal(&run_id_for_task, intent_status).await {
                    warn!(run_id = %run_id_for_task, error = %e, "failed to mark intent terminal");
                }
            }
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

    /// Replay an unterminated intent after a restart. Uses the same
    /// `run_id` as the original spawn so TUI bookmarks and journal entries
    /// still resolve.
    ///
    /// The workflow is re-run from the top with the original input. This
    /// is NOT checkpoint-and-replay — konf rejects mid-workflow resume
    /// because LLM calls are non-deterministic. Workflow authors must make
    /// their workflows idempotent.
    pub async fn replay(&self, intent: RunnerIntent) -> Result<RunId, RunnerError> {
        let spec = WorkflowSpec {
            workflow: intent.workflow.clone(),
            input: intent.input.clone(),
        };
        let run_id = intent.run_id.clone();

        // Increment replay_count and re-persist before respawning so a
        // crash loop is visible and bounded.
        if let Some(ref intents) = self.intents {
            let mut updated = intent;
            updated.replay_count += 1;
            if updated.replay_count > 10 {
                warn!(run_id = %run_id, "replay loop exceeded limit, marking intent failed");
                let _ = intents
                    .mark_terminal(
                        &run_id,
                        TerminalStatus::Failed {
                            error: "replay loop exceeded limit (>10 restarts)".into(),
                        },
                    )
                    .await;
                return Err(RunnerError::Backend("replay loop exceeded limit".into()));
            }
            if let Err(e) = intents.insert(updated).await {
                warn!(run_id = %run_id, error = %e, "failed to update intent replay_count");
            }
        }

        self.spawn_with_id(Some(run_id), spec).await
    }
}

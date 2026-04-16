//! Runtime — the main entry point for workflow management.
//!
//! Wraps the konflux engine with process lifecycle management,
//! capability routing, monitoring, and event journaling.

use std::collections::HashMap;
use std::sync::atomic::Ordering;
use std::sync::{Arc, OnceLock};
use std::time::Instant;

use chrono::Utc;
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{debug, error, info, warn};

use konflux_substrate::engine::{Engine, EngineConfig};
use konflux_substrate::Workflow;

use crate::context::VirtualizedTool;
use crate::error::{RunId, RuntimeError};
use crate::event_bus::{RunEvent, RunEventBus};
use crate::execution_context::ExecutionContext;
use crate::guard::{DefaultAction, GuardedTool, Rule};
use crate::hooks::RuntimeHooks;
use crate::journal::{JournalEntry, JournalStore};
use crate::monitor::{ProcessTree, RunDetail, RunSummary, RuntimeMetrics};
use crate::process::{ProcessTable, RunStatus, WorkflowRun};
use crate::scheduler::RedbScheduler;
use crate::scope::{ExecutionScope, ResourceLimits};

/// Per-tool guard configuration. Stored in the runtime and applied during
/// per-execution registry construction (same phase as VirtualizedTool wrapping).
#[derive(Debug, Clone, Default)]
pub struct ToolGuardEntry {
    /// Ordered deny/allow rules.
    pub rules: Vec<Rule>,
    /// Behavior when no rule matches.
    pub default_action: DefaultAction,
    /// Optional: redirect to a wrapper workflow tool instead.
    pub alias: Option<String>,
}

/// The workflow management runtime.
pub struct Runtime {
    engine: Engine,
    table: Arc<ProcessTable>,
    journal: Option<Arc<dyn JournalStore>>,
    /// Installed once at boot via [`Runtime::install_scheduler`]. Set only
    /// when a persistent storage backend is configured.
    scheduler: OnceLock<Arc<RedbScheduler>>,
    /// Broadcast channel for real-time monitoring events. Subscribers
    /// (the HTTP `/v1/monitor/stream` endpoint, TUIs) consume
    /// [`RunEvent`]s emitted from every mutating path.
    event_bus: Arc<RunEventBus>,
    _default_limits: ResourceLimits,
    started_at: Instant,
    /// Tool guards from product config, applied during registry construction.
    tool_guards: Arc<std::sync::RwLock<HashMap<String, ToolGuardEntry>>>,
    /// Single-tool dispatcher (capability check + wrapping + journaling).
    dispatcher: crate::dispatcher::Dispatcher,
    // Counters for metrics (Arc-wrapped for sharing with spawned tasks)
    total_completed: Arc<std::sync::atomic::AtomicU64>,
    total_failed: Arc<std::sync::atomic::AtomicU64>,
    total_cancelled: Arc<std::sync::atomic::AtomicU64>,
}

impl Runtime {
    /// Create a new runtime with an optional journal backend.
    ///
    /// If a journal is provided, [`JournalStore::reconcile_zombies`] is
    /// invoked once to surface workflows that were interrupted by a prior
    /// crash. If `journal` is `None` (edge deployment, dev mode with no
    /// persistent state), no events are recorded.
    pub async fn new(
        engine: Engine,
        journal: Option<Arc<dyn JournalStore>>,
    ) -> Result<Self, RuntimeError> {
        Self::with_limits(engine, journal, ResourceLimits::default()).await
    }

    /// Create with custom default resource limits.
    pub async fn with_limits(
        engine: Engine,
        journal: Option<Arc<dyn JournalStore>>,
        default_limits: ResourceLimits,
    ) -> Result<Self, RuntimeError> {
        if let Some(ref j) = journal {
            let reconciled = j.reconcile_zombies().await?;
            if reconciled > 0 {
                info!(
                    count = reconciled,
                    "Reconciled zombie workflows from previous run"
                );
            }
        } else {
            debug!("No journal backend — event journal disabled");
        }

        let event_bus = Arc::new(RunEventBus::default());
        let tool_guards = Arc::new(std::sync::RwLock::new(HashMap::new()));

        let dispatcher = crate::dispatcher::Dispatcher {
            tool_guards: tool_guards.clone(),
            journal: journal.clone(),
            event_bus: event_bus.clone(),
        };

        Ok(Self {
            engine,
            table: Arc::new(ProcessTable::new()),
            journal,
            scheduler: OnceLock::new(),
            event_bus,
            _default_limits: default_limits,
            started_at: Instant::now(),
            tool_guards,
            dispatcher,
            total_completed: Arc::new(0.into()),
            total_failed: Arc::new(0.into()),
            total_cancelled: Arc::new(0.into()),
        })
    }

    /// Access the real-time event bus. Subscribers call `.subscribe()`
    /// to receive a [`tokio::sync::broadcast::Receiver`] over
    /// [`RunEvent`] values emitted by the runtime.
    pub fn event_bus(&self) -> Arc<RunEventBus> {
        self.event_bus.clone()
    }

    /// Install the durable scheduler backed by [`crate::storage::KonfStorage`].
    ///
    /// Must be called before workflows or `schedule:create` tools attempt to
    /// use the scheduler. Subsequent calls are ignored — the scheduler can
    /// only be installed once per runtime instance.
    pub fn install_scheduler(&self, scheduler: Arc<RedbScheduler>) {
        if self.scheduler.set(scheduler).is_err() {
            debug!("scheduler already installed, ignoring duplicate install_scheduler call");
        }
    }

    /// Access the installed scheduler. Returns `None` on edge deployments
    /// that don't configure a storage backend.
    pub fn scheduler(&self) -> Option<&Arc<RedbScheduler>> {
        self.scheduler.get()
    }

    /// Access the engine for tool registration.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Set tool guards from product config. Replaces all existing guards.
    /// Called at boot and on config_reload.
    pub fn set_tool_guards(&self, guards: HashMap<String, ToolGuardEntry>) {
        let mut lock = self.tool_guards.write().expect("tool_guards lock poisoned");
        *lock = guards;
    }

    /// Build a scoped engine with only granted tools, applying VirtualizedTool
    /// (namespace injection) and GuardedTool (deny/allow rules) wrapping.
    ///
    /// Used by both `start` and `start_streaming` to ensure identical security
    /// behavior regardless of execution path.
    fn build_scoped_engine(&self, scope: &ExecutionScope) -> Engine {
        let engine_config = EngineConfig {
            max_steps: scope.limits.max_steps,
            max_workflow_timeout_ms: scope.limits.max_workflow_timeout_ms,
            ..self.engine.config().clone()
        };

        let engine = Engine::with_config(engine_config);

        // Copy only granted tools, wrapping with VirtualizedTool and GuardedTool.
        //
        // Wrapping order (outermost evaluated first):
        //   GuardedTool → VirtualizedTool → inner tool
        //
        // Guards check raw LLM input, then VirtualizedTool injects bindings.
        let source_registry = self.engine.registry();
        let guards = self.tool_guards.read().expect("tool_guards lock poisoned");
        for tool_info in source_registry.list() {
            let tool_name = &tool_info.name;

            // Check if this tool has an alias (redirect to wrapper workflow).
            // The alias is still gated by the scope's capabilities — if the
            // scope doesn't grant the original tool name, the alias is skipped too.
            if let Some(guard_entry) = guards.get(tool_name) {
                if let Some(ref alias) = guard_entry.alias {
                    if scope.check_tool(tool_name).is_err() {
                        debug!(tool = %tool_name, "Aliased tool not granted in scope, skipping");
                        continue;
                    }
                    if let Some(alias_tool) = source_registry.get(alias) {
                        debug!(tool = %tool_name, alias = %alias, "Tool aliased to wrapper");
                        engine.register_tool(alias_tool);
                        continue;
                    } else {
                        warn!(
                            tool = %tool_name, alias = %alias,
                            "Tool guard alias not found in registry, falling back to original"
                        );
                    }
                }
            }

            if let Some(tool) = source_registry.get(tool_name) {
                match scope.check_tool(tool_name) {
                    Ok(bindings) => {
                        // Layer 1: VirtualizedTool (namespace injection)
                        let wrapped: Arc<dyn konflux_substrate::tool::Tool> = if bindings.is_empty()
                        {
                            tool
                        } else {
                            Arc::new(VirtualizedTool::new(tool, bindings))
                        };

                        // Layer 2: GuardedTool (deny/allow rules from config)
                        let wrapped = if let Some(guard_entry) = guards.get(tool_name) {
                            if guard_entry.rules.is_empty() {
                                wrapped
                            } else {
                                debug!(
                                    tool = %tool_name,
                                    rule_count = guard_entry.rules.len(),
                                    "Applying tool guards"
                                );
                                Arc::new(GuardedTool::new(
                                    wrapped,
                                    guard_entry.rules.clone(),
                                    guard_entry.default_action,
                                ))
                            }
                        } else {
                            wrapped
                        };

                        engine.register_tool(wrapped);
                    }
                    Err(_) => {
                        debug!(tool = %tool_name, "Tool not granted in scope, skipping");
                    }
                }
            }
        }

        engine
    }

    /// Build the execution metadata that the substrate executor uses to
    /// populate Envelope fields (trace_id, namespace, actor_id, session_id).
    /// Without this, the executor falls back to "unknown" for all of these.
    fn build_execution_metadata(
        scope: &ExecutionScope,
        ctx: &ExecutionContext,
    ) -> HashMap<String, Value> {
        let mut m = HashMap::with_capacity(5);
        m.insert("trace_id".into(), Value::String(ctx.trace_id.to_string()));
        m.insert("namespace".into(), Value::String(scope.namespace.clone()));
        m.insert("actor_id".into(), Value::String(scope.actor.id.clone()));
        m.insert("session_id".into(), Value::String(ctx.session_id.clone()));
        let actor_role_str = match scope.actor.role {
            crate::scope::ActorRole::InfraAdmin => "infra_admin",
            crate::scope::ActorRole::ProductAdmin => "product_admin",
            crate::scope::ActorRole::User => "user",
            crate::scope::ActorRole::InfraAgent => "infra_agent",
            crate::scope::ActorRole::ProductAgent => "product_agent",
            crate::scope::ActorRole::UserAgent => "user_agent",
            crate::scope::ActorRole::System => "system",
        };
        m.insert("actor_role".into(), Value::String(actor_role_str.into()));
        m.insert(
            "depth".into(),
            Value::Number(serde_json::Number::from(scope.depth as u64)),
        );
        m
    }

    /// Parse a YAML workflow.
    pub fn parse_yaml(&self, yaml: &str) -> Result<Workflow, RuntimeError> {
        self.engine.parse_yaml(yaml).map_err(RuntimeError::Engine)
    }

    /// Invoke a single tool under a scope, applying the same wrapping
    /// (namespace binding injection via [`VirtualizedTool`], deny/allow
    /// rules via [`GuardedTool`]) that workflow execution would apply —
    /// without creating a workflow-run lifecycle entry in the process
    /// table.
    ///
    /// Intended for transport layers (MCP, HTTP) that need to call a
    /// single tool outside of a workflow but still want scope enforcement
    /// and guard rules to apply. This is how the HTTP `/mcp` endpoint
    /// exposes tools safely: instead of calling `engine.registry().get(..)`
    /// and invoking the raw tool directly, it routes the call through
    /// here so dev-mode sessions still pick up tool guards and any
    /// configured namespace bindings.
    ///
    /// Does NOT emit `RunStarted` / `RunCompleted` events (single-tool
    /// calls are tracked via the event bus in phase 5 instead).
    pub async fn invoke_tool(
        &self,
        tool_name: &str,
        input: Value,
        scope: &ExecutionScope,
        ctx: &ExecutionContext,
    ) -> Result<Value, RuntimeError> {
        let registry = self.engine.registry();
        self.dispatcher
            .dispatch_tool(tool_name, input, scope, ctx, &registry)
            .await
    }

    // ---- Execution ----

    /// Start a workflow execution. Returns RunId immediately.
    pub async fn start(
        &self,
        workflow: &Workflow,
        input: Value,
        scope: ExecutionScope,
        ctx: ExecutionContext,
    ) -> Result<RunId, RuntimeError> {
        let session_id = ctx.session_id.clone();
        let trace_id = ctx.trace_id;
        info!(
            workflow_id = %workflow.id,
            namespace = %scope.namespace,
            actor_id = %scope.actor.id,
            "runtime.start"
        );

        // Validate resource limits
        scope.validate_start(&self.table)?;

        let run_id = RunId::new_v4();
        debug!(run_id = %run_id, "workflow run created");
        let cancel_token = CancellationToken::new();

        // Create the run entry
        let run = WorkflowRun {
            id: run_id,
            parent_id: None,
            workflow_id: workflow.id.to_string(),
            namespace: scope.namespace.clone(),
            actor: scope.actor.clone(),
            capabilities: scope.capability_patterns(),
            metadata: HashMap::new(),
            started_at: Utc::now(),
            status: std::sync::Mutex::new(RunStatus::Running),
            completed_at: std::sync::Mutex::new(None),
            active_nodes: std::sync::Mutex::new(Vec::new()),
            steps_executed: 0.into(),
            cancel_token: cancel_token.clone(),
        };

        self.table.insert(run);

        // Emit run_started to the live event bus so subscribers (TUI via
        // /v1/monitor/stream) see the new run immediately.
        self.event_bus.emit(RunEvent::RunStarted {
            run_id,
            workflow_id: workflow.id.to_string(),
            namespace: scope.namespace.clone(),
            parent_id: None,
            started_at: Utc::now(),
        });

        // Journal: workflow started (if journal is available)
        if let Some(ref journal) = self.journal {
            if let Err(e) = journal
                .append(JournalEntry {
                    run_id: Some(run_id),
                    session_id: session_id.clone(),
                    namespace: scope.namespace.clone(),
                    event_type: "workflow_started".into(),
                    payload: serde_json::json!({
                        "workflow_id": workflow.id.to_string(),
                        "namespace": &scope.namespace,
                        "actor_id": &scope.actor.id,
                        "actor_role": &scope.actor.role,
                    }),
                    valid_to: None,
                })
                .await
            {
                warn!(error = %e, run_id = %run_id, "Failed to journal workflow_started event");
            }
        }

        // Build scoped engine with VirtualizedTool + GuardedTool wrapping
        let engine = self.build_scoped_engine(&scope);
        let capability_patterns = scope.capability_patterns();
        let execution_metadata = Self::build_execution_metadata(&scope, &ctx);

        // Create hooks for process table updates + event bus emission +
        // F1 Interaction-shaped journal append.
        let hooks = Arc::new(RuntimeHooks {
            run_id,
            namespace: scope.namespace.clone(),
            session_id: session_id.clone(),
            table: self.table.clone(),
            journal: self.journal.clone(),
            event_bus: self.event_bus.clone(),
            actor: scope.actor.clone(),
            trace_id,
        });

        // Spawn execution
        let table = self.table.clone();
        let journal = self.journal.clone();
        let namespace = scope.namespace.clone();
        let workflow = workflow.clone();
        let total_completed = self.total_completed.clone();
        let total_failed = self.total_failed.clone();
        let total_cancelled = self.total_cancelled.clone();
        let event_bus = self.event_bus.clone();

        tokio::spawn(async move {
            let result = engine
                .run(
                    &workflow,
                    input,
                    &capability_patterns,
                    execution_metadata,
                    Some(cancel_token),
                    Some(hooks),
                )
                .await;

            // Determine terminal status using typed error matching instead of string matching
            let now = Utc::now();
            let is_cancellation = match &result {
                Ok(_) => false,
                Err(e) => matches!(
                    e,
                    konflux_substrate::KonfluxError::Execution(
                        konflux_substrate::error::ExecutionError::Cancelled { .. }
                    )
                ),
            };

            // Compute duration once and reuse it for status + metrics + events.
            let duration_ms = {
                let started_at = table.get(&run_id, |run| run.started_at).unwrap_or(now);
                (now - started_at).num_milliseconds().max(0) as u64
            };

            table.update(&run_id, |run| {
                *run.completed_at.lock().unwrap_or_else(|p| p.into_inner()) = Some(now);
                let new_status = match &result {
                    Ok(output) => RunStatus::Completed {
                        duration_ms,
                        output: output.clone(),
                    },
                    Err(e) if is_cancellation => RunStatus::Cancelled {
                        reason: e.to_string(),
                        duration_ms,
                    },
                    Err(e) => RunStatus::Failed {
                        error: e.to_string(),
                        duration_ms,
                    },
                };
                *run.status.lock().unwrap_or_else(|p| p.into_inner()) = new_status;
            });

            // Increment metrics counters and emit lifecycle events.
            match (&result, is_cancellation) {
                (Ok(_), _) => {
                    total_completed.fetch_add(1, Ordering::Relaxed);
                    event_bus.emit(RunEvent::RunCompleted {
                        run_id,
                        duration_ms,
                    });
                }
                (Err(e), true) => {
                    total_cancelled.fetch_add(1, Ordering::Relaxed);
                    event_bus.emit(RunEvent::RunCancelled {
                        run_id,
                        reason: e.to_string(),
                    });
                }
                (Err(e), false) => {
                    total_failed.fetch_add(1, Ordering::Relaxed);
                    event_bus.emit(RunEvent::RunFailed {
                        run_id,
                        duration_ms,
                        error: e.to_string(),
                    });
                }
            }

            // Journal: workflow completed/failed/cancelled
            let (event_type, payload) = match &result {
                Ok(_) => ("workflow_completed", serde_json::json!({})),
                Err(e) if is_cancellation => (
                    "workflow_cancelled",
                    serde_json::json!({"reason": e.to_string()}),
                ),
                Err(e) => (
                    "workflow_failed",
                    serde_json::json!({"error": e.to_string()}),
                ),
            };
            if let Some(ref journal) = journal {
                if let Err(e) = journal
                    .append(JournalEntry {
                        run_id: Some(run_id),
                        session_id,
                        namespace,
                        event_type: event_type.into(),
                        payload,
                        valid_to: None,
                    })
                    .await
                {
                    error!(error = %e, run_id = %run_id, "Failed to record workflow completion in journal");
                }
            }

            result
        });

        Ok(run_id)
    }

    /// Wait for a workflow to complete.
    pub async fn wait(&self, run_id: RunId) -> Result<Value, RuntimeError> {
        debug!(run_id = %run_id, "runtime.wait");
        // Poll the process table for completion
        loop {
            let status = self.table.get(&run_id, |run| {
                run.status.lock().unwrap_or_else(|p| p.into_inner()).clone()
            });
            match status {
                Some(RunStatus::Completed { output, .. }) => return Ok(output),
                Some(RunStatus::Failed { error, .. }) => {
                    return Err(RuntimeError::Engine(
                        konflux_substrate::KonfluxError::Execution(
                            konflux_substrate::error::ExecutionError::NodeFailed {
                                workflow_id: "runtime".into(),
                                node: "wait".into(),
                                message: error,
                            },
                        ),
                    ));
                }
                Some(RunStatus::Cancelled { reason, .. }) => {
                    return Err(RuntimeError::Engine(
                        konflux_substrate::KonfluxError::Execution(
                            konflux_substrate::error::ExecutionError::Cancelled {
                                workflow_id: reason,
                            },
                        ),
                    ));
                }
                None => return Err(RuntimeError::NotFound(run_id)),
                _ => {
                    // Still running — wait a bit
                    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
                }
            }
        }
    }

    /// Start + wait (convenience).
    pub async fn run(
        &self,
        workflow: &Workflow,
        input: Value,
        scope: ExecutionScope,
        ctx: ExecutionContext,
    ) -> Result<Value, RuntimeError> {
        let run_id = self.start(workflow, input, scope, ctx).await?;
        self.wait(run_id).await
    }

    /// Start with streaming. Returns RunId + stream receiver for real-time events.
    ///
    /// Unlike `start()` which fires-and-forgets, this returns a `StreamReceiver`
    /// that emits real-time events (TextDelta, ToolStart, ToolEnd, Done, Error)
    /// from the engine's executor. Use from SSE endpoints to pipe events to clients.
    ///
    /// The runtime still tracks the run in the ProcessTable and writes to the
    /// journal — this happens via a forwarding task that intercepts the engine's
    /// stream, forwards events to the caller, and updates status on completion.
    pub async fn start_streaming(
        &self,
        workflow: &Workflow,
        input: Value,
        scope: ExecutionScope,
        ctx: ExecutionContext,
    ) -> Result<(RunId, konflux_substrate::StreamReceiver), RuntimeError> {
        let session_id = ctx.session_id.clone();
        let trace_id = ctx.trace_id;
        info!(
            workflow_id = %workflow.id,
            namespace = %scope.namespace,
            actor_id = %scope.actor.id,
            "runtime.start_streaming"
        );

        // Validate resource limits
        scope.validate_start(&self.table)?;

        let run_id = RunId::new_v4();
        debug!(run_id = %run_id, "streaming workflow run created");
        let cancel_token = CancellationToken::new();

        // Create the run entry
        let run = WorkflowRun {
            id: run_id,
            parent_id: None,
            workflow_id: workflow.id.to_string(),
            namespace: scope.namespace.clone(),
            actor: scope.actor.clone(),
            capabilities: scope.capability_patterns(),
            metadata: HashMap::new(),
            started_at: Utc::now(),
            status: std::sync::Mutex::new(RunStatus::Running),
            completed_at: std::sync::Mutex::new(None),
            active_nodes: std::sync::Mutex::new(Vec::new()),
            steps_executed: 0.into(),
            cancel_token: cancel_token.clone(),
        };
        self.table.insert(run);

        // Emit run_started to the event bus.
        self.event_bus.emit(RunEvent::RunStarted {
            run_id,
            workflow_id: workflow.id.to_string(),
            namespace: scope.namespace.clone(),
            parent_id: None,
            started_at: Utc::now(),
        });

        // Journal: workflow started
        if let Some(ref journal) = self.journal {
            if let Err(e) = journal
                .append(JournalEntry {
                    run_id: Some(run_id),
                    session_id: session_id.clone(),
                    namespace: scope.namespace.clone(),
                    event_type: "workflow_started".into(),
                    payload: serde_json::json!({
                        "workflow_id": workflow.id.to_string(),
                        "namespace": &scope.namespace,
                        "actor_id": &scope.actor.id,
                        "streaming": true,
                    }),
                    valid_to: None,
                })
                .await
            {
                warn!(error = %e, run_id = %run_id, "Failed to journal workflow_started event");
            }
        }

        // Build scoped engine with VirtualizedTool + GuardedTool wrapping
        let engine = self.build_scoped_engine(&scope);
        let capability_patterns = scope.capability_patterns();
        let execution_metadata = Self::build_execution_metadata(&scope, &ctx);

        // Create hooks for process table updates + event bus emission +
        // F1 Interaction-shaped journal append.
        let hooks = Arc::new(RuntimeHooks {
            run_id,
            namespace: scope.namespace.clone(),
            session_id: session_id.clone(),
            table: self.table.clone(),
            journal: self.journal.clone(),
            event_bus: self.event_bus.clone(),
            actor: scope.actor.clone(),
            trace_id,
        });

        // Call engine.run_streaming() — returns a StreamReceiver immediately
        let mut engine_rx = engine
            .run_streaming(
                workflow,
                input,
                &capability_patterns,
                execution_metadata,
                Some(cancel_token),
                Some(hooks),
            )
            .await
            .map_err(RuntimeError::Engine)?;

        // Create a forwarding channel: engine_rx → (caller_rx + process table update)
        let (caller_tx, caller_rx) =
            konflux_substrate::stream::stream_channel(self.engine.config().stream_buffer);

        let table = self.table.clone();
        let journal = self.journal.clone();
        let namespace = scope.namespace.clone();
        let total_completed = self.total_completed.clone();
        let total_failed = self.total_failed.clone();
        let total_cancelled = self.total_cancelled.clone();
        let event_bus = self.event_bus.clone();

        // Forwarding task: reads from engine, forwards to caller, updates process table on terminal events
        tokio::spawn(async move {
            let mut final_status = None;

            while let Some(event) = engine_rx.recv().await {
                let is_terminal = matches!(
                    event,
                    konflux_substrate::stream::StreamEvent::Done { .. }
                        | konflux_substrate::stream::StreamEvent::Error { .. }
                );

                match &event {
                    konflux_substrate::stream::StreamEvent::Done { .. } => {
                        final_status = Some(("workflow_completed", serde_json::json!({}), true));
                    }
                    konflux_substrate::stream::StreamEvent::Error { code, message, .. } => {
                        let is_cancel = code == "cancelled" || message.contains("cancelled");
                        if is_cancel {
                            final_status = Some((
                                "workflow_cancelled",
                                serde_json::json!({"reason": message}),
                                false,
                            ));
                        } else {
                            final_status = Some((
                                "workflow_failed",
                                serde_json::json!({"error": message}),
                                false,
                            ));
                        }
                    }
                    _ => {}
                }

                // Forward to caller (drop if caller disconnected)
                if caller_tx.send(event).await.is_err() {
                    break;
                }

                if is_terminal {
                    break;
                }
            }

            // Update process table
            let now = Utc::now();
            let duration_ms = {
                let started_at = table.get(&run_id, |run| run.started_at).unwrap_or(now);
                (now - started_at).num_milliseconds().max(0) as u64
            };
            table.update(&run_id, |run| {
                *run.completed_at.lock().unwrap_or_else(|p| p.into_inner()) = Some(now);
                let new_status = match &final_status {
                    Some((_, _, true)) => RunStatus::Completed {
                        duration_ms,
                        output: Value::Null,
                    },
                    Some(("workflow_cancelled", _, _)) => RunStatus::Cancelled {
                        reason: "cancelled".into(),
                        duration_ms,
                    },
                    Some((_, payload, _)) => RunStatus::Failed {
                        error: payload
                            .get("error")
                            .and_then(|e| e.as_str())
                            .unwrap_or("unknown")
                            .to_string(),
                        duration_ms,
                    },
                    None => RunStatus::Failed {
                        error: "stream ended without terminal event".into(),
                        duration_ms,
                    },
                };
                *run.status.lock().unwrap_or_else(|p| p.into_inner()) = new_status;
            });

            // Increment metrics and emit lifecycle events.
            match &final_status {
                Some((_, _, true)) => {
                    total_completed.fetch_add(1, Ordering::Relaxed);
                    event_bus.emit(RunEvent::RunCompleted {
                        run_id,
                        duration_ms,
                    });
                }
                Some(("workflow_cancelled", payload, _)) => {
                    total_cancelled.fetch_add(1, Ordering::Relaxed);
                    let reason = payload
                        .get("reason")
                        .and_then(|v| v.as_str())
                        .unwrap_or("cancelled")
                        .to_string();
                    event_bus.emit(RunEvent::RunCancelled { run_id, reason });
                }
                Some((_, payload, _)) => {
                    total_failed.fetch_add(1, Ordering::Relaxed);
                    let error = payload
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("unknown")
                        .to_string();
                    event_bus.emit(RunEvent::RunFailed {
                        run_id,
                        duration_ms,
                        error,
                    });
                }
                None => {
                    total_failed.fetch_add(1, Ordering::Relaxed);
                    event_bus.emit(RunEvent::RunFailed {
                        run_id,
                        duration_ms,
                        error: "stream ended without terminal event".into(),
                    });
                }
            }

            // Journal completion
            if let Some((event_type, payload, _)) = final_status {
                if let Some(ref journal) = journal {
                    if let Err(e) = journal
                        .append(JournalEntry {
                            run_id: Some(run_id),
                            session_id,
                            namespace,
                            event_type: event_type.into(),
                            payload,
                            valid_to: None,
                        })
                        .await
                    {
                        tracing::error!(error = %e, run_id = %run_id, "Failed to record workflow completion in journal");
                    }
                }
            }
        });

        Ok((run_id, caller_rx))
    }

    // ---- Lifecycle ----

    /// Graceful cancel (SIGTERM). Propagates to children.
    pub async fn cancel(&self, run_id: RunId, reason: &str) -> Result<(), RuntimeError> {
        info!(run_id = %run_id, reason, "runtime.cancel");
        let is_running = self.table.get(&run_id, |run| {
            matches!(
                *run.status.lock().unwrap_or_else(|p| p.into_inner()),
                RunStatus::Running
            )
        });

        match is_running {
            Some(true) => {}
            Some(false) => return Err(RuntimeError::NotRunning(run_id)),
            None => return Err(RuntimeError::NotFound(run_id)),
        }

        // Cancel the token
        self.table.update(&run_id, |run| {
            run.cancel_token.cancel();
        });

        // Recursively cancel children
        for child in self.table.children_of(run_id) {
            if !child.status.is_terminal() {
                let _ = Box::pin(self.cancel(child.id, reason)).await;
            }
        }

        Ok(())
    }

    // ---- Monitoring ----

    /// List runs, optionally filtered by namespace prefix.
    pub fn list_runs(&self, namespace_prefix: Option<&str>) -> Vec<RunSummary> {
        self.table.list(namespace_prefix)
    }

    /// Get detailed info about a specific run.
    pub fn get_run(&self, run_id: RunId) -> Option<RunDetail> {
        self.table.get(&run_id, |run| {
            let active_nodes = run
                .active_nodes
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            RunDetail {
                summary: run.to_summary(),
                active_nodes,
                capabilities: run.capabilities.clone(),
                children: self.table.children_of(run_id),
            }
        })
    }

    /// Get the process tree rooted at a run.
    pub fn get_tree(&self, run_id: RunId) -> Option<ProcessTree> {
        self.build_tree(run_id)
    }

    fn build_tree(&self, run_id: RunId) -> Option<ProcessTree> {
        self.table.get(&run_id, |run| {
            let active_nodes = run
                .active_nodes
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone();
            let children: Vec<ProcessTree> = self
                .table
                .children_of(run_id)
                .into_iter()
                .filter_map(|child| self.build_tree(child.id))
                .collect();

            ProcessTree {
                run: run.to_summary(),
                children,
                active_nodes,
            }
        })
    }

    /// Get aggregate metrics.
    pub fn metrics(&self) -> RuntimeMetrics {
        RuntimeMetrics {
            active_runs: self.table.active_count(),
            total_completed: self.total_completed.load(Ordering::Relaxed),
            total_failed: self.total_failed.load(Ordering::Relaxed),
            total_cancelled: self.total_cancelled.load(Ordering::Relaxed),
            uptime_seconds: self.started_at.elapsed().as_secs(),
        }
    }

    // ---- Maintenance ----

    /// Remove completed runs older than max_age from the process table.
    pub fn gc(&self, max_age: std::time::Duration) {
        self.table.gc(max_age);
    }

    /// Access the event journal (for admin queries). None on edge deployments.
    pub fn journal(&self) -> Option<&dyn JournalStore> {
        self.journal.as_deref()
    }
}

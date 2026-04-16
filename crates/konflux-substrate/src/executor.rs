//! Parallel Workflow Executor
//!
//! Executes workflows using the dependency graph for maximum parallelism.
//! Nodes with no dependencies on each other run concurrently.
//!
//! ## Cancellation
//!
//! The executor accepts a `CancellationToken`. When cancelled:
//! - The main loop stops spawning new nodes
//! - Active tasks are shut down via `JoinSet::shutdown()`
//! - Returns `ExecutionError::Cancelled`
//!
//! Note: sync Python tools (called via PyO3 `with_gil`) run to completion
//! even after cancellation — the GIL blocks preemption. Cancellation takes
//! effect at the next `.await` point (between nodes, between retries).
//!
//! ## Streaming backpressure
//!
//! Progress events (TextDelta, ToolStart, ToolEnd, Status) use `try_send()`
//! and are dropped if the channel buffer is full. This prevents a slow SSE
//! client from stalling the workflow. Done and Error events use `send().await`
//! to guarantee delivery.

use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};
use tokio::time::timeout;
use tokio_util::sync::CancellationToken;
use tracing::{info_span, warn, Instrument};

use crate::capability;
use crate::engine::EngineConfig;
use crate::envelope::Envelope;
use crate::error::{ExecutionError, KonfluxError, ToolError};
use crate::expr::{ExprEvaluator, ExprValue};
use crate::hooks::{EventRecorder, ExecutorEvent};
use crate::stream::{ProgressType, StreamEvent, StreamSender};
use crate::template;
use crate::tool::{Tool, ToolRegistry};
use crate::workflow::{
    BackoffStrategy, EdgeTarget, ErrorAction, JoinPolicy, RetryPolicy, Step, StepId, Workflow,
};

/// Thread-safe state for workflow execution.
#[derive(Debug, Default)]
struct State {
    /// Node ID -> Output value
    outputs: RwLock<HashMap<String, Value>>,
    /// Node ID -> Error message
    errors: RwLock<HashMap<String, String>>,
}

impl State {
    async fn set_output(&self, node_id: &str, value: Value) {
        self.outputs
            .write()
            .await
            .insert(node_id.to_string(), value);
    }

    async fn set_error(&self, node_id: &str, err: String) {
        self.errors.write().await.insert(node_id.to_string(), err);
    }

    async fn get_outputs(&self) -> HashMap<String, Value> {
        self.outputs.read().await.clone()
    }

    async fn is_completed(&self, node_id: &str) -> bool {
        let outputs = self.outputs.read().await;
        let errors = self.errors.read().await;
        outputs.contains_key(node_id) || errors.contains_key(node_id)
    }
}

/// Bundled context passed to step execution functions.
struct StepContext {
    registry: Arc<ToolRegistry>,
    capabilities: Vec<String>,
    config: EngineConfig,
    metadata: HashMap<String, Value>,
    workflow_id: String,
    cancel_token: CancellationToken,
    hooks: Arc<dyn EventRecorder>,
}

pub struct Executor {
    registry: Arc<ToolRegistry>,
    capabilities: Vec<String>,
    config: EngineConfig,
    metadata: HashMap<String, Value>,
    cancel_token: CancellationToken,
    hooks: Arc<dyn EventRecorder>,
}

impl Executor {
    pub fn new(
        registry: &Arc<ToolRegistry>,
        capabilities: &[String],
        config: &EngineConfig,
        metadata: HashMap<String, Value>,
        cancel_token: CancellationToken,
        hooks: Arc<dyn EventRecorder>,
    ) -> Self {
        Self {
            registry: registry.clone(),
            capabilities: capabilities.to_vec(),
            config: config.clone(),
            metadata,
            cancel_token,
            hooks,
        }
    }

    pub async fn execute(
        &self,
        workflow: &Workflow,
        input: Value,
        tx: StreamSender,
    ) -> Result<(), KonfluxError> {
        let state = Arc::new(State::default());
        state.set_output("input", input).await;

        let workflow_id = workflow.id.to_string();
        let _ = tx.try_send(StreamEvent::Start {
            workflow_id: workflow_id.clone(),
        });

        let mut executed_nodes = HashSet::new();
        let mut reachable_nodes = HashSet::new();
        let mut active_tasks = tokio::task::JoinSet::new();
        let (finished_tx, mut finished_rx) = mpsc::channel(self.config.finished_channel_size);

        let entry_node = workflow.get_step(&workflow.entry).ok_or_else(|| {
            KonfluxError::Execution(ExecutionError::NodeFailed {
                workflow_id: workflow_id.clone(),
                node: workflow.entry.to_string(),
                message: "Entry node not found".to_string(),
            })
        })?;

        reachable_nodes.insert(entry_node.id.clone());
        executed_nodes.insert(entry_node.id.clone());

        let mut total_spawned = 1;
        let mut total_finished = 0;
        let workflow_arc = Arc::new(workflow.clone());

        self.spawn_node(
            entry_node,
            workflow_arc.clone(),
            state.clone(),
            tx.clone(),
            &mut active_tasks,
            finished_tx.clone(),
        );

        let mut steps_count = 0;
        let mut final_output = Value::Null;

        loop {
            tokio::select! {
                // Cancellation — highest priority
                _ = self.cancel_token.cancelled() => {
                    active_tasks.shutdown().await;
                    return Err(KonfluxError::Execution(ExecutionError::Cancelled {
                        workflow_id: workflow_id.clone(),
                    }));
                }
                res = active_tasks.join_next(), if !active_tasks.is_empty() => {
                    if let Some(res) = res {
                        match res {
                            Ok(Ok(node_result)) => {
                                steps_count += node_result.steps;
                                if steps_count > self.config.max_steps {
                                    active_tasks.shutdown().await;
                                    return Err(KonfluxError::Execution(ExecutionError::MaxStepsExceeded {
                                        workflow_id: workflow_id.clone(),
                                        max: self.config.max_steps,
                                    }));
                                }
                                if let Some(output) = node_result.output {
                                    final_output = output;
                                }
                            }
                            Ok(Err(e)) => {
                                active_tasks.shutdown().await;
                                return Err(e);
                            }
                            Err(e) => {
                                let msg = if e.is_panic() {
                                    "A workflow task panicked during execution".to_string()
                                } else {
                                    e.to_string()
                                };
                                return Err(KonfluxError::Execution(ExecutionError::JoinFailed {
                                    workflow_id: workflow_id.clone(),
                                    node: "unknown".into(),
                                    message: msg,
                                }));
                            }
                        }
                    }
                }
                Some(finished_info) = finished_rx.recv() => {
                    match finished_info {
                        FinishedInfo::Node(_node_id) => {
                            total_finished += 1;
                            let current_outputs = state.get_outputs().await;
                            for step_id in &reachable_nodes {
                                // Enforce max_concurrent_nodes: don't spawn if at capacity
                                if active_tasks.len() >= self.config.max_concurrent_nodes {
                                    break; // will retry on next FinishedInfo::Node
                                }
                                if !executed_nodes.contains(step_id) {
                                    if let Some(step) = workflow.get_step(step_id) {
                                        if self.can_run(step, state.clone()).await {
                                            executed_nodes.insert(step.id.clone());
                                            total_spawned += 1;
                                            self.spawn_node(step, workflow_arc.clone(), state.clone(), tx.clone(), &mut active_tasks, finished_tx.clone());
                                        }
                                    }
                                }
                            }
                            let _ = current_outputs;
                        },
                        FinishedInfo::SpawnNext(target_id) => {
                            reachable_nodes.insert(target_id.clone());
                            // Enforce max_concurrent_nodes
                            if active_tasks.len() < self.config.max_concurrent_nodes {
                                if let Some(next_step) = workflow.get_step(&target_id) {
                                    if !executed_nodes.contains(&next_step.id)
                                        && self.can_run(next_step, state.clone()).await
                                    {
                                        executed_nodes.insert(next_step.id.clone());
                                        total_spawned += 1;
                                        self.spawn_node(next_step, workflow_arc.clone(), state.clone(), tx.clone(), &mut active_tasks, finished_tx.clone());
                                    }
                                }
                            }
                            // If at capacity, node stays in reachable_nodes — will be spawned when a slot opens
                        }
                    }
                }
                else => break,
            }
            if total_spawned == total_finished && active_tasks.is_empty() {
                break;
            }
        }

        // Done event must be delivered (use send, not try_send)
        tx.send(StreamEvent::Done {
            output: final_output,
        })
        .await
        .ok();
        Ok(())
    }

    async fn can_run(&self, step: &Step, state: Arc<State>) -> bool {
        if step.depends_on.is_empty() {
            return true;
        }
        let mut completed_deps = 0;
        for dep in &step.depends_on {
            if state.is_completed(dep.as_str()).await {
                completed_deps += 1;
            }
        }
        match step.join {
            JoinPolicy::All => completed_deps == step.depends_on.len(),
            JoinPolicy::Any => completed_deps > 0,
            JoinPolicy::Quorum { min } => completed_deps >= min as usize,
            JoinPolicy::Lenient => true,
        }
    }

    fn spawn_node(
        &self,
        step: &Step,
        workflow: Arc<Workflow>,
        state: Arc<State>,
        tx: StreamSender,
        tasks: &mut tokio::task::JoinSet<Result<NodeExecutionResult, KonfluxError>>,
        finished_tx: mpsc::Sender<FinishedInfo>,
    ) {
        let step = step.clone();
        let step_ctx = Arc::new(StepContext {
            registry: self.registry.clone(),
            capabilities: self.capabilities.clone(),
            config: self.config.clone(),
            metadata: self.metadata.clone(),
            workflow_id: workflow.id.to_string(),
            cancel_token: self.cancel_token.clone(),
            hooks: self.hooks.clone(),
        });

        tasks.spawn(async move {
            let mut current_step = step;
            let mut iteration = 0;
            let mut steps_executed = 0;

            loop {
                // Check cancellation before each step
                if step_ctx.cancel_token.is_cancelled() {
                    let _ = finished_tx
                        .send(FinishedInfo::Node(current_step.id.to_string()))
                        .await;
                    return Err(KonfluxError::Execution(ExecutionError::Cancelled {
                        workflow_id: step_ctx.workflow_id.clone(),
                    }));
                }

                steps_executed += 1;
                let node_id = current_step.id.to_string();
                if let Some(repeat) = &current_step.repeat {
                    if let Some(as_var) = &repeat.as_var {
                        state.set_output(as_var, Value::from(iteration)).await;
                    }
                }

                use futures::FutureExt;
                let node_span = info_span!("node.execute",
                    node_id = %node_id,
                    tool = %current_step.tool,
                );
                let res = std::panic::AssertUnwindSafe(
                    execute_step(
                        &current_step,
                        &workflow,
                        state.clone(),
                        tx.clone(),
                        &step_ctx,
                    )
                    .instrument(node_span),
                )
                .catch_unwind()
                .await
                .unwrap_or_else(|_| {
                    Err(KonfluxError::Execution(ExecutionError::NodeFailed {
                        workflow_id: step_ctx.workflow_id.clone(),
                        node: node_id.clone(),
                        message: "Tool execution panicked".into(),
                    }))
                });

                match res {
                    Ok(val) => {
                        state.set_output(&node_id, val.clone()).await;
                        let final_node_val = val;

                        if let Some(repeat) = &current_step.repeat {
                            let outputs = state.get_outputs().await;
                            let ctx = to_expr_context(&outputs);
                            match ExprEvaluator::new(&ctx).evaluate_as_bool(&repeat.until) {
                                Ok(true) => {}
                                Ok(false) => {
                                    iteration += 1;
                                    if iteration < repeat.max {
                                        continue;
                                    }
                                }
                                Err(e) => {
                                    let _ =
                                        finished_tx.send(FinishedInfo::Node(node_id.clone())).await;
                                    return Err(KonfluxError::Execution(
                                        ExecutionError::NodeFailed {
                                            workflow_id: step_ctx.workflow_id.clone(),
                                            node: node_id,
                                            message: format!(
                                                "Repeat condition evaluation failed: {}",
                                                e
                                            ),
                                        },
                                    ));
                                }
                            }
                        }

                        let outputs = state.get_outputs().await;
                        match evaluate_edges_targets(&current_step, &outputs, &step_ctx.workflow_id)
                        {
                            Ok(targets) => {
                                if targets.len() == 1 {
                                    match &targets[0] {
                                        EdgeTarget::Step(target_id) => {
                                            if let Some(next_step) = workflow.get_step(target_id) {
                                                if next_step.depends_on.len() <= 1 {
                                                    current_step = next_step.clone();
                                                    iteration = 0;
                                                    continue;
                                                }
                                            }
                                            let _ = finished_tx
                                                .send(FinishedInfo::SpawnNext(target_id.clone()))
                                                .await;
                                        }
                                        EdgeTarget::Return => {
                                            let _ = finished_tx
                                                .send(FinishedInfo::Node(node_id.clone()))
                                                .await;
                                            return Ok(NodeExecutionResult {
                                                output: Some(final_node_val),
                                                steps: steps_executed,
                                            });
                                        }
                                    }
                                } else {
                                    for target in targets {
                                        if let EdgeTarget::Step(target_id) = target {
                                            let _ = finished_tx
                                                .send(FinishedInfo::SpawnNext(target_id))
                                                .await;
                                        }
                                    }
                                }
                                let _ = finished_tx.send(FinishedInfo::Node(node_id)).await;
                                return Ok(NodeExecutionResult {
                                    output: Some(final_node_val),
                                    steps: steps_executed,
                                });
                            }
                            Err(e) => {
                                let _ = finished_tx.send(FinishedInfo::Node(node_id)).await;
                                return Err(e);
                            }
                        }
                    }
                    Err(err) => match current_step.on_error {
                        ErrorAction::Fail => {
                            state.set_error(&node_id, err.to_string()).await;
                            let _ = finished_tx.send(FinishedInfo::Node(node_id)).await;
                            return Err(err);
                        }
                        ErrorAction::Skip => {
                            state.set_output(&node_id, Value::Null).await;
                            let _ = finished_tx.send(FinishedInfo::Node(node_id)).await;
                            return Ok(NodeExecutionResult {
                                output: None,
                                steps: steps_executed,
                            });
                        }
                        ErrorAction::Fallback { ref value } => {
                            let val = Value::String(value.clone());
                            state.set_output(&node_id, val.clone()).await;
                            let _ = finished_tx.send(FinishedInfo::Node(node_id)).await;
                            return Ok(NodeExecutionResult {
                                output: Some(val),
                                steps: steps_executed,
                            });
                        }
                        ErrorAction::Goto { ref step } => {
                            if let Some(next_step) = workflow.get_step(step) {
                                current_step = next_step.clone();
                                iteration = 0;
                                continue;
                            } else {
                                let _ = finished_tx.send(FinishedInfo::Node(node_id)).await;
                                return Err(err);
                            }
                        }
                    },
                }
            }
        });
    }
}

enum FinishedInfo {
    Node(String),
    SpawnNext(StepId),
}

struct NodeExecutionResult {
    output: Option<Value>,
    steps: usize,
}

async fn execute_step(
    step: &Step,
    _workflow: &Workflow,
    state: Arc<State>,
    tx: StreamSender,
    ctx: &StepContext,
) -> Result<Value, KonfluxError> {
    let node_id = step.id.to_string();
    let tool_name = step.tool.to_string();
    ctx.hooks.on_event(ExecutorEvent::NodeStarted {
        node_id: &node_id,
        tool: &tool_name,
    });
    let start_time = std::time::Instant::now();

    let outputs = state.get_outputs().await;
    let resolved_inputs = template::resolve_inputs(&step.input, &outputs).map_err(|e| {
        KonfluxError::Execution(ExecutionError::NodeFailed {
            workflow_id: ctx.workflow_id.clone(),
            node: node_id.clone(),
            message: format!("Failed to resolve inputs: {}", e),
        })
    })?;

    capability::check_tool_access(step.tool.as_str(), &ctx.capabilities)
        .map_err(KonfluxError::Tool)?;

    let tool = ctx.registry.get(step.tool.as_str()).ok_or_else(|| {
        KonfluxError::Tool(ToolError::NotFound {
            tool_id: step.tool.to_string(),
        })
    })?;

    let tool_capabilities = if let Some(grant) = &step.grant {
        capability::validate_grant(grant, &ctx.capabilities)
            .map_err(|e| ToolError::CapabilityDenied { capability: e })?;
        grant.clone()
    } else {
        ctx.capabilities.to_vec()
    };

    // Extract typed context from the execution metadata populated by
    // Runtime::build_execution_metadata (Stage 5.a). The "unknown"
    // fallbacks are defense-in-depth for edge cases (e.g. engine used
    // standalone without runtime); in normal runtime dispatch these
    // keys are always present.
    let trace_id = ctx
        .metadata
        .get("trace_id")
        .and_then(|v| v.as_str())
        .and_then(|s| uuid::Uuid::parse_str(s).ok())
        .unwrap_or_else(uuid::Uuid::new_v4);
    let namespace = ctx
        .metadata
        .get("namespace")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let actor_id = ctx
        .metadata
        .get("actor_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");
    let session_id = ctx
        .metadata
        .get("session_id")
        .and_then(|v| v.as_str())
        .unwrap_or("unknown");

    let env = Envelope::for_tool_dispatch(
        step.tool.as_str(),
        Value::Object(resolved_inputs.into_iter().collect()),
        &tool_capabilities,
        trace_id,
        namespace,
        actor_id,
        session_id,
    );

    let result = invoke_with_retry(tool, env, step, ctx, &tx).await;
    let duration_ms = start_time.elapsed().as_millis() as u64;
    match &result {
        Ok(output) => ctx.hooks.on_event(ExecutorEvent::NodeCompleted {
            node_id: &node_id,
            tool: &tool_name,
            duration_ms,
            output: &output.payload,
        }),
        Err(e) => ctx.hooks.on_event(ExecutorEvent::NodeFailed {
            node_id: &node_id,
            tool: &tool_name,
            error: &e.to_string(),
        }),
    }
    // Extract payload from response envelope for the DAG state.
    result.map(|env| env.payload)
}

async fn invoke_with_retry(
    tool: Arc<dyn Tool>,
    env: Envelope<Value>,
    step: &Step,
    ctx: &StepContext,
    tx: &StreamSender,
) -> Result<Envelope<Value>, KonfluxError> {
    let node_id = step.id.to_string();
    let retry_policy = step.retry.as_ref();
    let timeout_duration = step.timeout;
    let max_attempts = retry_policy.map(|r| r.max_attempts + 1).unwrap_or(1);
    let mut last_err = None;

    for attempt in 1..=max_attempts {
        // Check cancellation between retry attempts
        if ctx.cancel_token.is_cancelled() {
            return Err(KonfluxError::Execution(ExecutionError::Cancelled {
                workflow_id: ctx.workflow_id.clone(),
            }));
        }

        // Deadline enforcement — abort before invoking if past deadline.
        if let Some(deadline) = env.deadline {
            if chrono::Utc::now() > deadline {
                return Err(KonfluxError::Tool(ToolError::Timeout { after_ms: 0 }));
            }
        }

        if attempt > 1 {
            let delay = calculate_backoff(retry_policy, attempt - 1, &ctx.config);
            tokio::time::sleep(delay).await;
        }

        // Progress events use try_send — dropped if buffer full (backpressure protection)
        let _ = tx.try_send(StreamEvent::Progress {
            node_id: node_id.to_string(),
            event_type: ProgressType::ToolStart,
            data: serde_json::json!({
                "tool": tool.info().name,
                "attempt": attempt,
            }),
        });

        let tool_span = info_span!("tool.invoke",
            tool = %tool.info().name,
            node_id = %node_id,
            attempt,
        );

        let start_time = std::time::Instant::now();
        let dur = timeout_duration.unwrap_or(Duration::from_millis(ctx.config.default_timeout_ms));
        let invoke_res = match timeout(
            dur,
            tool.invoke_streaming(env.clone(), tx.clone())
                .instrument(tool_span),
        )
        .await
        {
            Ok(res) => res,
            Err(_) => Err(ToolError::Timeout {
                after_ms: dur.as_millis() as u64,
            }),
        };

        match invoke_res {
            Ok(output) => {
                let duration_ms = start_time.elapsed().as_millis() as u64;
                tracing::info!(duration_ms, tool = %tool.info().name, "tool completed");
                // Progress events use try_send
                let _ = tx.try_send(StreamEvent::Progress {
                    node_id: node_id.to_string(),
                    event_type: ProgressType::ToolEnd,
                    data: serde_json::json!({
                        "tool": tool.info().name,
                        "duration_ms": duration_ms,
                    }),
                });
                return Ok(output);
            }
            Err(e) => {
                let retryable = match &e {
                    ToolError::ExecutionFailed { retryable, .. } => *retryable,
                    ToolError::Timeout { .. } => true,
                    _ => false,
                };
                last_err = Some(e.clone());
                if !retryable || attempt >= max_attempts {
                    break;
                }
                ctx.hooks.on_event(ExecutorEvent::ToolRetry {
                    node_id: &node_id,
                    tool: &tool.info().name,
                    attempt,
                    error: &e.to_string(),
                });
                warn!(tool = %tool.info().name, attempt, error = %e, "Tool failed, retrying");
            }
        }
    }

    Err(KonfluxError::Tool(last_err.unwrap_or_else(|| {
        ToolError::ExecutionFailed {
            message: "No execution attempts were permitted by retry policy".to_string(),
            retryable: false,
        }
    })))
}

fn calculate_backoff(
    policy: Option<&RetryPolicy>,
    attempt: u32,
    config: &EngineConfig,
) -> Duration {
    let policy = match policy {
        Some(p) => p,
        None => return Duration::from_millis(config.default_retry_backoff_ms),
    };
    let base = policy.base_delay;
    let max = policy.max_delay;
    let delay = match &policy.backoff {
        BackoffStrategy::Fixed => base,
        BackoffStrategy::Exponential => {
            let multiplier = 2u64.saturating_pow(attempt - 1);
            Duration::from_millis((base.as_millis() as u64).saturating_mul(multiplier))
        }
        BackoffStrategy::Linear { increment } => base.saturating_add(*increment * (attempt - 1)),
    };
    delay.min(max)
}

fn evaluate_edges_targets(
    step: &Step,
    outputs: &HashMap<String, Value>,
    workflow_id: &str,
) -> Result<Vec<EdgeTarget>, KonfluxError> {
    if step.edges.is_empty() {
        return Ok(vec![EdgeTarget::Return]);
    }

    let mut branches: Vec<&crate::workflow::Edge> = step.edges.iter().collect();
    branches.sort_by_key(|e| e.priority);

    let ctx = to_expr_context(outputs);
    let eval = ExprEvaluator::new(&ctx);

    let mut targets = Vec::new();

    for edge in branches {
        if let Some(condition) = &edge.condition {
            let cleaned = condition
                .trim()
                .strip_prefix("{{")
                .and_then(|s| s.strip_suffix("}}"))
                .unwrap_or(condition)
                .trim();
            match eval.evaluate_as_bool(cleaned) {
                Ok(true) => {
                    targets.push(edge.target.clone());
                    return Ok(targets);
                }
                Ok(false) => continue,
                Err(e) => {
                    return Err(KonfluxError::Execution(ExecutionError::NodeFailed {
                        workflow_id: workflow_id.to_string(),
                        node: step.id.to_string(),
                        message: format!("Condition evaluation failed: {}", e),
                    }))
                }
            }
        } else {
            targets.push(edge.target.clone());
        }
    }

    if targets.is_empty() {
        Ok(vec![EdgeTarget::Return])
    } else {
        Ok(targets)
    }
}

fn to_expr_context(outputs: &HashMap<String, Value>) -> HashMap<String, ExprValue> {
    outputs
        .iter()
        .map(|(k, v)| (k.clone(), ExprEvaluator::json_to_expr_value(v)))
        .collect()
}

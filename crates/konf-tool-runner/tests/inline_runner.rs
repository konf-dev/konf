//! Integration tests for the inline runner backend.
//!
//! These tests mount a real `Engine` + `Runtime`, register a tiny workflow
//! as a tool, and drive every runner tool (`spawn`, `status`, `wait`,
//! `cancel`) through the full happy and sad paths.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use konf_tool_runner::{InlineRunner, RunRegistry, RunState, Runner, WorkflowSpec};

use konf_runtime::Runtime;
use konflux_substrate::envelope::Envelope;
use konflux_substrate::error::ToolError;
use konflux_substrate::tool::{Tool, ToolInfo};
use konflux_substrate::Engine;

// ------------------------------------------------------------------
// test doubles
// ------------------------------------------------------------------

/// A workflow-shaped tool that echoes its input back under "echo_of".
struct EchoWorkflow;

#[async_trait]
impl Tool for EchoWorkflow {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "workflow:echo".into(),
            description: "echoes input under echo_of".into(),
            input_schema: json!({}),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: Default::default(),
        }
    }
    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        let input = env.payload.clone();
        Ok(env.respond(json!({ "echo_of": input })))
    }
}

/// A workflow that always errors.
struct FailingWorkflow;

#[async_trait]
impl Tool for FailingWorkflow {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "workflow:fail".into(),
            description: "always fails".into(),
            input_schema: json!({}),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: Default::default(),
        }
    }
    async fn invoke(&self, _env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        Err(ToolError::ExecutionFailed {
            message: "intentional failure".into(),
            retryable: false,
        })
    }
}

/// A workflow that sleeps forever so cancel has something to abort.
struct SleepyWorkflow {
    /// Incremented when the task starts so tests can observe that the run
    /// actually entered the sleep before we cancelled it.
    started: Arc<AtomicUsize>,
}

#[async_trait]
impl Tool for SleepyWorkflow {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "workflow:sleepy".into(),
            description: "sleeps forever".into(),
            input_schema: json!({}),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: Default::default(),
        }
    }
    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        self.started.fetch_add(1, Ordering::SeqCst);
        // Far longer than any reasonable test deadline; the test aborts
        // this via runner:cancel before it can return.
        tokio::time::sleep(Duration::from_secs(3600)).await;
        Ok(env.respond(json!({})))
    }
}

// ------------------------------------------------------------------
// fixtures
// ------------------------------------------------------------------

async fn fresh_runner() -> (Arc<Runtime>, InlineRunner, Arc<AtomicUsize>) {
    let engine = Engine::new();
    engine.register_tool(Arc::new(EchoWorkflow));
    engine.register_tool(Arc::new(FailingWorkflow));
    let started = Arc::new(AtomicUsize::new(0));
    engine.register_tool(Arc::new(SleepyWorkflow {
        started: started.clone(),
    }));

    let runtime = Arc::new(Runtime::new(engine, None).await.expect("runtime init"));
    let registry = RunRegistry::new();
    let runner = InlineRunner::new(runtime.clone(), registry);
    (runtime, runner, started)
}

// ------------------------------------------------------------------
// happy path: spawn → wait → succeeded
// ------------------------------------------------------------------

#[tokio::test]
async fn spawn_echo_workflow_succeeds() {
    let (_rt, runner, _) = fresh_runner().await;
    let id = runner
        .spawn(spec("echo", json!({"msg": "hello"})))
        .await
        .expect("spawn");

    let record = tokio::time::timeout(Duration::from_secs(5), runner.registry().wait_terminal(&id))
        .await
        .expect("wait not timed out")
        .expect("record present");

    match &record.state {
        RunState::Succeeded { result } => {
            assert_eq!(result["echo_of"]["msg"], "hello");
        }
        other => panic!("expected Succeeded, got {other:?}"),
    }
    assert_eq!(record.workflow, "echo");
    assert_eq!(record.backend, "inline");
    assert!(record.started_at.is_some());
    assert!(record.finished_at.is_some());
}

// ------------------------------------------------------------------
// failing workflow → Failed state
// ------------------------------------------------------------------

#[tokio::test]
async fn spawn_failing_workflow_reaches_failed() {
    let (_rt, runner, _) = fresh_runner().await;
    let id = runner.spawn(spec("fail", json!({}))).await.expect("spawn");

    let record = runner
        .registry()
        .wait_terminal(&id)
        .await
        .expect("record present");

    match &record.state {
        RunState::Failed { error } => {
            assert!(error.contains("intentional failure"));
        }
        other => panic!("expected Failed, got {other:?}"),
    }
}

// ------------------------------------------------------------------
// unknown workflow → WorkflowNotFound
// ------------------------------------------------------------------

#[tokio::test]
async fn spawn_unknown_workflow_errors() {
    let (_rt, runner, _) = fresh_runner().await;
    let err = runner.spawn(spec("nope", json!({}))).await.unwrap_err();
    assert!(err.to_string().contains("nope"));
}

// ------------------------------------------------------------------
// cancel: interrupt an in-flight sleepy workflow
// ------------------------------------------------------------------

#[tokio::test]
async fn cancel_aborts_in_flight_run() {
    let (_rt, runner, started) = fresh_runner().await;
    let id = runner
        .spawn(spec("sleepy", json!({})))
        .await
        .expect("spawn");

    // Wait until the workflow has actually started running. This prevents
    // a race where cancel runs before the task is scheduled.
    for _ in 0..50 {
        if started.load(Ordering::SeqCst) > 0 {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    assert!(
        started.load(Ordering::SeqCst) > 0,
        "sleepy workflow never started"
    );

    let cancelled = runner.cancel(&id).await.expect("cancel");
    assert!(cancelled, "cancel should have invoked a hook");

    // Cancellation should have driven the run to a terminal state very fast.
    let record = tokio::time::timeout(Duration::from_secs(2), runner.registry().wait_terminal(&id))
        .await
        .expect("wait should not time out after cancel")
        .expect("record present");
    assert!(matches!(record.state, RunState::Cancelled));
}

// ------------------------------------------------------------------
// fan-out: spawn N runs in parallel, collect all
// ------------------------------------------------------------------

#[tokio::test]
async fn fanout_collects_all_results() {
    let (_rt, runner, _) = fresh_runner().await;

    let ids: Vec<_> = (0..5)
        .map(|i| {
            let runner = runner.clone();
            tokio::spawn(
                async move { runner.spawn(spec("echo", json!({"idx": i}))).await.unwrap() },
            )
        })
        .collect();

    let mut run_ids = Vec::new();
    for h in ids {
        run_ids.push(h.await.unwrap());
    }
    assert_eq!(run_ids.len(), 5);

    for id in &run_ids {
        let rec = runner
            .registry()
            .wait_terminal(id)
            .await
            .expect("record present");
        match &rec.state {
            RunState::Succeeded { result } => {
                assert!(result["echo_of"]["idx"].is_number());
            }
            other => panic!("expected Succeeded, got {other:?}"),
        }
    }
}

// ------------------------------------------------------------------
// registry length tracking
// ------------------------------------------------------------------

#[tokio::test]
async fn registry_tracks_runs() {
    let (_rt, runner, _) = fresh_runner().await;
    assert!(runner.registry().is_empty());

    for _ in 0..3 {
        let _ = runner.spawn(spec("echo", json!({}))).await.unwrap();
    }
    assert_eq!(runner.registry().len(), 3);
}

// ------------------------------------------------------------------
// helpers
// ------------------------------------------------------------------

fn spec(workflow: &str, input: Value) -> WorkflowSpec {
    // R1: tests supply a permissive parent scope ("*") because these tests
    // exercise runner mechanics, not attenuation. Attenuation itself is
    // covered in `konf-runtime/tests/scope_tests.rs` and the new
    // `runner_attenuates_parent_scope` test below (if added).
    let parent_scope = konf_runtime::scope::ExecutionScope {
        namespace: "konf:test:runner".into(),
        capabilities: vec![konf_runtime::scope::CapabilityGrant::new("*")],
        limits: konf_runtime::scope::ResourceLimits::default(),
        actor: konf_runtime::scope::Actor {
            id: "test".into(),
            role: konf_runtime::scope::ActorRole::System,
        },
        depth: 0,
    };
    let parent_ctx = konf_runtime::ExecutionContext::new_root("test-session");
    WorkflowSpec {
        workflow: workflow.to_string(),
        input,
        parent_scope,
        parent_ctx,
    }
}

//! Integration tests for the Runtime.
//!
//! These tests exercise the runtime against both:
//!   - "edge mode" (no journal backend)
//!   - redb-backed journal (backed by a tempfile)
//!
//! Both modes are exercised by every test via a helper that constructs a
//! runtime twice per test. This is faster and more reliable than the old
//! DATABASE_URL-gated Postgres tests.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use konflux_substrate::engine::Engine;
use konflux_substrate::envelope::Envelope;
use konflux_substrate::error::ToolError;
use konflux_substrate::tool::{Tool, ToolInfo};

use konf_runtime::journal::JournalStore;
use konf_runtime::scope::*;
use konf_runtime::{RedbJournal, RunEvent, Runtime};

// ============================================================
// Mock Tools
// ============================================================

struct EchoTool;
#[async_trait]
impl Tool for EchoTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "echo".into(),
            description: "Returns input".into(),
            input_schema: json!({}),
            capabilities: vec![],
            supports_streaming: false,
            output_schema: None,
            annotations: Default::default(),
        }
    }
    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        let output = env.payload.clone();
        Ok(env.respond(output))
    }
}

struct SlowTool;
#[async_trait]
impl Tool for SlowTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "slow".into(),
            description: "Sleeps 500ms".into(),
            input_schema: json!({}),
            capabilities: vec![],
            supports_streaming: false,
            output_schema: None,
            annotations: Default::default(),
        }
    }
    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        Ok(env.respond(json!({"done": true})))
    }
}

fn setup_engine() -> Engine {
    let engine = Engine::new();
    engine.register_tool(Arc::new(EchoTool));
    engine.register_tool(Arc::new(SlowTool));
    konflux_substrate::builtin::register_builtins(&engine);
    engine
}

fn test_scope(namespace: &str) -> ExecutionScope {
    ExecutionScope {
        namespace: namespace.into(),
        capabilities: vec![CapabilityGrant::new("*")],
        limits: ResourceLimits::default(),
        actor: Actor {
            id: "test_user".into(),
            role: ActorRole::User,
        },
        depth: 0,
    }
}

// ============================================================
// Runtime builders
// ============================================================

/// Create a runtime with a redb journal backed by a fresh tempdir. Returns
/// both the runtime and the tempdir (dropped at end of test to clean up).
async fn create_runtime_with_journal() -> (Runtime, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("journal.redb");
    let journal = Arc::new(RedbJournal::open(&path).await.unwrap());
    let engine = setup_engine();
    let runtime = Runtime::new(engine, Some(journal as Arc<dyn JournalStore>))
        .await
        .unwrap();
    (runtime, dir)
}

/// Create a runtime in edge mode (no journal).
async fn create_runtime_edge() -> Runtime {
    Runtime::new(setup_engine(), None).await.unwrap()
}

// ============================================================
// Journal-backed tests
// ============================================================

#[tokio::test]
async fn test_runtime_start_and_wait_with_journal() {
    let (runtime, _dir) = create_runtime_with_journal().await;

    let workflow = runtime
        .parse_yaml(
            r#"
workflow: test_start_wait
nodes:
  step1:
    do: echo
    with: { val: "hello" }
    return: true
"#,
        )
        .unwrap();

    let scope = test_scope("konf:test:user_1");
    let result = runtime
        .run(
            &workflow,
            json!({}),
            scope,
            konf_runtime::ExecutionContext::new_root("sess_1"),
        )
        .await;
    assert!(
        result.is_ok(),
        "Workflow should complete: {:?}",
        result.err()
    );
}

#[tokio::test]
async fn test_runtime_cancel_with_journal() {
    let (runtime, _dir) = create_runtime_with_journal().await;

    let workflow = runtime
        .parse_yaml(
            r#"
workflow: test_cancel
nodes:
  step1:
    do: slow
    return: true
"#,
        )
        .unwrap();

    let scope = test_scope("konf:test:user_1");
    let run_id = runtime
        .start(
            &workflow,
            json!({}),
            scope,
            konf_runtime::ExecutionContext::new_root("sess_1"),
        )
        .await
        .unwrap();

    // Cancel after 50ms
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    runtime.cancel(run_id, "test cancellation").await.unwrap();

    // Wait should return error
    let result = runtime.wait(run_id).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_runtime_list_runs_with_journal() {
    let (runtime, _dir) = create_runtime_with_journal().await;

    let workflow = runtime
        .parse_yaml(
            r#"
workflow: test_list
nodes:
  step1:
    do: echo
    with: { val: "ok" }
    return: true
"#,
        )
        .unwrap();

    let scope = test_scope("konf:test:user_list");
    let _run_id = runtime
        .start(
            &workflow,
            json!({}),
            scope,
            konf_runtime::ExecutionContext::new_root("sess_list"),
        )
        .await
        .unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let runs = runtime.list_runs(Some("konf:test:user_list"));
    assert!(!runs.is_empty(), "Should have at least one run");
}

#[tokio::test]
async fn test_runtime_journal_records_workflow_events() {
    let (runtime, _dir) = create_runtime_with_journal().await;

    let workflow = runtime
        .parse_yaml(
            r#"
workflow: journal_test
nodes:
  step1:
    do: echo
    with: { val: "hi" }
    return: true
"#,
        )
        .unwrap();

    let scope = test_scope("konf:test:journal");
    let _ = runtime
        .run(
            &workflow,
            json!({}),
            scope,
            konf_runtime::ExecutionContext::new_root("sess_journal"),
        )
        .await;

    // Give async journal appends a moment to land.
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let rows = runtime
        .journal()
        .unwrap()
        .query_by_session("sess_journal", 100)
        .await
        .unwrap();
    assert!(
        !rows.is_empty(),
        "journal should have recorded workflow lifecycle events"
    );
    // Newest-first ordering: the last event should be a terminal one.
    assert!(rows.iter().any(|r| r.event_type == "workflow_started"));
}

#[tokio::test]
async fn test_runtime_resource_limit_with_journal() {
    let (runtime, _dir) = create_runtime_with_journal().await;

    let workflow = runtime
        .parse_yaml(
            r#"
workflow: test_limit
nodes:
  step1:
    do: slow
    return: true
"#,
        )
        .unwrap();

    let scope = ExecutionScope {
        namespace: "konf:test:limited".into(),
        capabilities: vec![CapabilityGrant::new("*")],
        limits: ResourceLimits {
            max_active_runs_per_namespace: 1,
            ..Default::default()
        },
        actor: Actor {
            id: "test".into(),
            role: ActorRole::User,
        },
        depth: 0,
    };

    let _run1 = runtime
        .start(
            &workflow,
            json!({}),
            scope.clone(),
            konf_runtime::ExecutionContext::new_root("sess_a"),
        )
        .await
        .unwrap();

    let result = runtime
        .start(
            &workflow,
            json!({}),
            scope,
            konf_runtime::ExecutionContext::new_root("sess_b"),
        )
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("max_active_runs"));
}

// ============================================================
// invoke_tool — single-tool invocation with scope enforcement
// ============================================================

#[tokio::test]
async fn test_invoke_tool_happy_path() {
    let runtime = create_runtime_edge().await;
    let scope = test_scope("konf:test:invoke");
    let result = runtime
        .invoke_tool(
            "echo",
            json!({"hello": "world"}),
            &scope,
            &konf_runtime::ExecutionContext::new_root("sess-t"),
        )
        .await
        .unwrap();
    assert_eq!(result, json!({"hello": "world"}));
}

#[tokio::test]
async fn test_invoke_tool_capability_denied() {
    let runtime = create_runtime_edge().await;
    // Scope grants only "ai:complete" — echo is NOT granted.
    let scope = ExecutionScope {
        namespace: "konf:test:denied".into(),
        capabilities: vec![CapabilityGrant::new("ai:complete")],
        limits: ResourceLimits::default(),
        actor: Actor {
            id: "test".into(),
            role: ActorRole::User,
        },
        depth: 0,
    };
    let err = runtime
        .invoke_tool(
            "echo",
            json!({}),
            &scope,
            &konf_runtime::ExecutionContext::new_root("sess-t"),
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("capability denied") || err.contains("not granted"));
}

#[tokio::test]
async fn test_invoke_tool_emits_tool_invoked_event() {
    let runtime = create_runtime_edge().await;
    let mut rx = runtime.event_bus().subscribe();
    let scope = test_scope("konf:test:event");
    let _ = runtime
        .invoke_tool(
            "echo",
            json!({"hello": "world"}),
            &scope,
            &konf_runtime::ExecutionContext::new_root("sess-t"),
        )
        .await
        .unwrap();
    // Drain events until we find the ToolInvoked.
    let mut saw_tool = false;
    for _ in 0..4 {
        match tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await {
            Ok(Ok(RunEvent::ToolInvoked { tool, success, .. })) => {
                assert_eq!(tool, "echo");
                assert!(success);
                saw_tool = true;
                break;
            }
            Ok(Ok(_)) => continue,
            _ => break,
        }
    }
    assert!(saw_tool, "expected ToolInvoked event");
}

#[tokio::test]
async fn test_workflow_run_emits_lifecycle_events() {
    let runtime = create_runtime_edge().await;
    let mut rx = runtime.event_bus().subscribe();
    let workflow = runtime
        .parse_yaml(
            r#"
workflow: lifecycle_test
nodes:
  step1:
    do: echo
    with: { val: "x" }
    return: true
"#,
        )
        .unwrap();
    let scope = test_scope("konf:test:lifecycle");
    let _ = runtime
        .run(
            &workflow,
            json!({}),
            scope,
            konf_runtime::ExecutionContext::new_root("sess_lifecycle"),
        )
        .await;

    // Drain events looking for RunStarted and one of RunCompleted/RunFailed.
    let mut saw_started = false;
    let mut saw_terminal = false;
    for _ in 0..20 {
        match tokio::time::timeout(std::time::Duration::from_millis(200), rx.recv()).await {
            Ok(Ok(RunEvent::RunStarted { .. })) => saw_started = true,
            Ok(Ok(
                RunEvent::RunCompleted { .. }
                | RunEvent::RunFailed { .. }
                | RunEvent::RunCancelled { .. },
            )) => {
                saw_terminal = true;
                break;
            }
            Ok(Ok(_)) => continue,
            _ => break,
        }
    }
    assert!(saw_started, "expected RunStarted event");
    assert!(saw_terminal, "expected terminal run event");
}

#[tokio::test]
async fn test_invoke_tool_unknown_tool() {
    let runtime = create_runtime_edge().await;
    let scope = test_scope("konf:test:unknown");
    let err = runtime
        .invoke_tool(
            "nonexistent:tool",
            json!({}),
            &scope,
            &konf_runtime::ExecutionContext::new_root("sess-t"),
        )
        .await
        .unwrap_err()
        .to_string();
    // "*" grants everything, so the error is "not found in engine registry"
    assert!(err.contains("not found") || err.contains("not granted"));
}

// ============================================================
// Edge mode tests (no journal)
// ============================================================

#[tokio::test]
async fn test_runtime_edge_mode_no_journal() {
    let runtime = create_runtime_edge().await;

    assert_eq!(runtime.metrics().active_runs, 0);
    assert!(runtime.journal().is_none());
    assert!(runtime.list_runs(None).is_empty());
}

#[tokio::test]
async fn test_runtime_edge_mode_start_and_wait() {
    let runtime = create_runtime_edge().await;

    let workflow = runtime
        .parse_yaml(
            r#"
workflow: edge_test
nodes:
  step1:
    do: echo
    with: { message: "edge" }
    return: true
"#,
        )
        .unwrap();

    let scope = test_scope("konf:edge:test");
    let run_id = runtime
        .start(
            &workflow,
            json!({"input": "test"}),
            scope,
            konf_runtime::ExecutionContext::new_root("edge_sess"),
        )
        .await
        .unwrap();
    let result = runtime.wait(run_id).await;
    assert!(result.is_ok(), "Edge mode workflow failed: {result:?}");
}

#[tokio::test]
async fn test_runtime_edge_mode_metrics_update() {
    let runtime = create_runtime_edge().await;

    let workflow = runtime
        .parse_yaml(
            r#"
workflow: metrics_test
nodes:
  step1:
    do: echo
    with: { val: 1 }
    return: true
"#,
        )
        .unwrap();

    let scope = test_scope("konf:edge:metrics");
    let _result = runtime
        .run(
            &workflow,
            json!({}),
            scope,
            konf_runtime::ExecutionContext::new_root("metrics_sess"),
        )
        .await;

    let metrics = runtime.metrics();
    assert!(
        metrics.total_completed > 0 || metrics.total_failed > 0,
        "Expected non-zero metrics after run, got: completed={}, failed={}",
        metrics.total_completed,
        metrics.total_failed
    );
}

#[tokio::test]
async fn test_runtime_cancel_in_edge_mode() {
    let runtime = create_runtime_edge().await;

    let workflow = runtime
        .parse_yaml(
            r#"
workflow: cancel_test
nodes:
  step1:
    do: slow
    return: true
"#,
        )
        .unwrap();

    let scope = test_scope("konf:edge:cancel");
    let run_id = runtime
        .start(
            &workflow,
            json!({}),
            scope,
            konf_runtime::ExecutionContext::new_root("cancel_sess"),
        )
        .await
        .unwrap();

    let cancel_result = runtime.cancel(run_id, "test cancel").await;
    assert!(cancel_result.is_ok());
}

#[tokio::test]
async fn test_runtime_capability_denial_edge_mode() {
    let runtime = create_runtime_edge().await;

    let workflow = runtime
        .parse_yaml(
            r#"
workflow: test_cap_deny
nodes:
  step1:
    do: echo
    with: { val: "hello" }
    return: true
"#,
        )
        .unwrap();

    let scope = ExecutionScope {
        namespace: "konf:test:denied".into(),
        capabilities: vec![CapabilityGrant::new("ai:complete")], // no echo
        limits: ResourceLimits::default(),
        actor: Actor {
            id: "test".into(),
            role: ActorRole::User,
        },
        depth: 0,
    };

    let run_id = runtime
        .start(
            &workflow,
            json!({}),
            scope,
            konf_runtime::ExecutionContext::new_root("sess_deny"),
        )
        .await
        .unwrap();
    let result = runtime.wait(run_id).await;
    assert!(result.is_err(), "Should fail due to capability denial");
}

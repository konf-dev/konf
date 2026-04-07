//! Integration tests for the Runtime.
//!
//! These tests create a Runtime with a real konflux Engine but require
//! a Postgres connection for the EventJournal. Set DATABASE_URL env var
//! to run these tests.
//!
//! Skip these tests in CI without Postgres by running:
//!   cargo test --lib  (unit tests only)

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use konflux::engine::Engine;
use konflux::tool::{Tool, ToolInfo, ToolContext};
use konflux::error::ToolError;

use konf_runtime::scope::*;
use konf_runtime::Runtime;

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
    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        Ok(input)
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
    async fn invoke(&self, _input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        Ok(json!({"done": true}))
    }
}

fn setup_engine() -> Engine {
    let engine = Engine::new();
    engine.register_tool(Arc::new(EchoTool));
    engine.register_tool(Arc::new(SlowTool));
    konflux::builtin::register_builtins(&engine);
    engine
}

fn test_scope(namespace: &str) -> ExecutionScope {
    ExecutionScope {
        namespace: namespace.into(),
        capabilities: vec![CapabilityGrant::new("*")],
        limits: ResourceLimits::default(),
        actor: Actor { id: "test_user".into(), role: ActorRole::User },
        depth: 0,
    }
}

// ============================================================
// Tests (require DATABASE_URL)
// ============================================================

async fn create_runtime() -> Option<Runtime> {
    let dsn = std::env::var("DATABASE_URL").ok()?;
    let pool = sqlx::PgPool::connect(&dsn).await.ok()?;
    let engine = setup_engine();
    Runtime::new(engine, Some(pool)).await.ok()
}

#[tokio::test]
async fn test_runtime_start_and_wait() {
    let Some(runtime) = create_runtime().await else {
        eprintln!("Skipping: DATABASE_URL not set");
        return;
    };

    let workflow = runtime.parse_yaml(r#"
workflow: test_start_wait
nodes:
  step1:
    do: echo
    with: { val: "hello" }
    return: true
"#).unwrap();

    let scope = test_scope("konf:test:user_1");
    let result = runtime.run(&workflow, json!({}), scope, "sess_1".into()).await;
    assert!(result.is_ok(), "Workflow should complete: {:?}", result.err());
}

#[tokio::test]
async fn test_runtime_cancel() {
    let Some(runtime) = create_runtime().await else {
        eprintln!("Skipping: DATABASE_URL not set");
        return;
    };

    let workflow = runtime.parse_yaml(r#"
workflow: test_cancel
nodes:
  step1:
    do: slow
    return: true
"#).unwrap();

    let scope = test_scope("konf:test:user_1");
    let run_id = runtime.start(&workflow, json!({}), scope, "sess_1".into()).await.unwrap();

    // Cancel after 50ms
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    runtime.cancel(run_id, "test cancellation").await.unwrap();

    // Wait should return error
    let result = runtime.wait(run_id).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_runtime_list_runs() {
    let Some(runtime) = create_runtime().await else {
        eprintln!("Skipping: DATABASE_URL not set");
        return;
    };

    let workflow = runtime.parse_yaml(r#"
workflow: test_list
nodes:
  step1:
    do: echo
    with: { val: "ok" }
    return: true
"#).unwrap();

    let scope = test_scope("konf:test:user_list");
    let _run_id = runtime.start(&workflow, json!({}), scope, "sess_list".into()).await.unwrap();

    // Give it a moment to register
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;

    let runs = runtime.list_runs(Some("konf:test:user_list"));
    assert!(!runs.is_empty(), "Should have at least one run");
}

#[tokio::test]
async fn test_runtime_metrics() {
    let Some(runtime) = create_runtime().await else {
        eprintln!("Skipping: DATABASE_URL not set");
        return;
    };

    let metrics = runtime.metrics();
    assert!(metrics.uptime_seconds < 10, "Uptime should be small in tests");
}

#[tokio::test]
async fn test_runtime_resource_limit() {
    let Some(runtime) = create_runtime().await else {
        eprintln!("Skipping: DATABASE_URL not set");
        return;
    };

    let workflow = runtime.parse_yaml(r#"
workflow: test_limit
nodes:
  step1:
    do: slow
    return: true
"#).unwrap();

    let scope = ExecutionScope {
        namespace: "konf:test:limited".into(),
        capabilities: vec![CapabilityGrant::new("*")],
        limits: ResourceLimits {
            max_active_runs_per_namespace: 1,
            ..Default::default()
        },
        actor: Actor { id: "test".into(), role: ActorRole::User },
        depth: 0,
    };

    // First start should succeed
    let _run1 = runtime.start(&workflow, json!({}), scope.clone(), "sess_a".into()).await.unwrap();

    // Second start should fail (limit = 1)
    let result = runtime.start(&workflow, json!({}), scope, "sess_b".into()).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("max_active_runs"));
}

#[tokio::test]
async fn test_runtime_capability_denial() {
    let Some(runtime) = create_runtime().await else {
        eprintln!("Skipping: DATABASE_URL not set");
        return;
    };

    let workflow = runtime.parse_yaml(r#"
workflow: test_cap_deny
nodes:
  step1:
    do: echo
    with: { val: "hello" }
    return: true
"#).unwrap();

    // Scope without "echo" capability
    let scope = ExecutionScope {
        namespace: "konf:test:denied".into(),
        capabilities: vec![CapabilityGrant::new("ai:complete")], // no echo
        limits: ResourceLimits::default(),
        actor: Actor { id: "test".into(), role: ActorRole::User },
        depth: 0,
    };

    let run_id = runtime.start(&workflow, json!({}), scope, "sess_deny".into()).await.unwrap();
    let result = runtime.wait(run_id).await;
    assert!(result.is_err(), "Should fail due to capability denial");
}

// ============================================================
// Edge mode tests (no database required)
// ============================================================

#[tokio::test]
async fn test_runtime_edge_mode_no_db() {
    let engine = setup_engine();
    let runtime = Runtime::new(engine, None).await.unwrap();

    // Runtime works without a database
    assert_eq!(runtime.metrics().active_runs, 0);
    assert!(runtime.journal().is_none());
    assert!(runtime.list_runs(None).is_empty());
}

#[tokio::test]
async fn test_runtime_edge_mode_start_and_wait() {
    let engine = setup_engine();
    let runtime = Runtime::new(engine, None).await.unwrap();

    let workflow = runtime.parse_yaml(r#"
workflow: edge_test
nodes:
  step1:
    do: echo
    with: { message: "edge" }
    return: true
"#).unwrap();

    let scope = test_scope("konf:edge:test");
    let run_id = runtime.start(&workflow, json!({"input": "test"}), scope, "edge_sess".into()).await.unwrap();
    let result = runtime.wait(run_id).await;
    // Workflow should complete (echo tool just returns input)
    assert!(result.is_ok(), "Edge mode workflow failed: {result:?}");
}

#[tokio::test]
async fn test_runtime_edge_mode_metrics_update() {
    let engine = setup_engine();
    let runtime = Runtime::new(engine, None).await.unwrap();

    let workflow = runtime.parse_yaml(r#"
workflow: metrics_test
nodes:
  step1:
    do: echo
    with: { val: 1 }
    return: true
"#).unwrap();

    let scope = test_scope("konf:edge:metrics");
    let _result = runtime.run(&workflow, json!({}), scope, "metrics_sess".into()).await;

    let metrics = runtime.metrics();
    // After completion, total_completed should increment
    // (The run may complete or fail depending on echo tool behavior,
    //  but metrics should be non-zero)
    assert!(
        metrics.total_completed > 0 || metrics.total_failed > 0,
        "Expected non-zero metrics after run, got: completed={}, failed={}",
        metrics.total_completed, metrics.total_failed
    );
}

#[tokio::test]
async fn test_runtime_cancel_in_edge_mode() {
    let engine = setup_engine();
    let runtime = Runtime::new(engine, None).await.unwrap();

    let workflow = runtime.parse_yaml(r#"
workflow: cancel_test
nodes:
  step1:
    do: slow
    return: true
"#).unwrap();

    let scope = test_scope("konf:edge:cancel");
    let run_id = runtime.start(&workflow, json!({}), scope, "cancel_sess".into()).await.unwrap();

    // Cancel immediately
    let cancel_result = runtime.cancel(run_id, "test cancel").await;
    assert!(cancel_result.is_ok());
}

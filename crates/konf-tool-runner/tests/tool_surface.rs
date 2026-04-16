//! Tests for the public tool surface (spawn / status / wait / cancel)
//! going through the `Tool` trait instead of the `Runner` trait directly.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use konf_tool_runner::{register, InlineRunner, RunRegistry, Runner};

use konf_runtime::Runtime;
use konflux_substrate::envelope::Envelope;
use konflux_substrate::error::ToolError;
use konflux_substrate::tool::{Tool, ToolInfo};
use konflux_substrate::Engine;

struct TrivialWorkflow;

#[async_trait]
impl Tool for TrivialWorkflow {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "workflow:trivial".into(),
            description: "returns {ok: true}".into(),
            input_schema: json!({}),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: Default::default(),
        }
    }
    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        Ok(env.respond(json!({ "ok": true })))
    }
}

#[tokio::test]
async fn register_exposes_four_tools() {
    let engine = Engine::new();
    engine.register_tool(Arc::new(TrivialWorkflow));
    let runtime = Arc::new(Runtime::new(engine, None).await.unwrap());
    let runner: Arc<dyn Runner> = Arc::new(InlineRunner::new(runtime.clone(), RunRegistry::new()));
    register(runtime.engine(), runner).unwrap();

    let reg = runtime.engine().registry();
    assert!(reg.get("runner:spawn").is_some());
    assert!(reg.get("runner:status").is_some());
    assert!(reg.get("runner:wait").is_some());
    assert!(reg.get("runner:cancel").is_some());
}

#[tokio::test]
async fn end_to_end_spawn_then_wait_tool_surface() {
    let engine = Engine::new();
    engine.register_tool(Arc::new(TrivialWorkflow));
    let runtime = Arc::new(Runtime::new(engine, None).await.unwrap());
    let runner: Arc<dyn Runner> = Arc::new(InlineRunner::new(runtime.clone(), RunRegistry::new()));
    register(runtime.engine(), runner).unwrap();

    let reg = runtime.engine().registry();
    let spawn = reg.get("runner:spawn").unwrap();
    let wait = reg.get("runner:wait").unwrap();
    let status = reg.get("runner:status").unwrap();

    // Spawn.
    let spawn_result = spawn
        .invoke(Envelope::test(json!({"workflow": "trivial"})))
        .await
        .unwrap()
        .payload;
    let run_id = spawn_result["run_id"].as_str().unwrap().to_string();
    assert_eq!(spawn_result["backend"], "inline");

    // Wait — should reach terminal Succeeded.
    let wait_result = wait
        .invoke(Envelope::test(
            json!({"run_id": run_id.clone(), "timeout_secs": 5}),
        ))
        .await
        .unwrap()
        .payload;
    assert_eq!(wait_result["state"], "succeeded");
    assert_eq!(wait_result["result"]["ok"], true);

    // Status — should still report terminal.
    let status_result = status
        .invoke(Envelope::test(json!({"run_id": run_id})))
        .await
        .unwrap()
        .payload;
    assert_eq!(status_result["state"], "succeeded");
}

#[tokio::test]
async fn spawn_missing_workflow_field_errors() {
    let engine = Engine::new();
    let runtime = Arc::new(Runtime::new(engine, None).await.unwrap());
    let runner: Arc<dyn Runner> = Arc::new(InlineRunner::new(runtime.clone(), RunRegistry::new()));
    register(runtime.engine(), runner).unwrap();

    let spawn = runtime.engine().registry().get("runner:spawn").unwrap();
    let err = spawn.invoke(Envelope::test(json!({}))).await.unwrap_err();
    assert!(err.to_string().contains("workflow"));
}

#[tokio::test]
async fn wait_unknown_run_errors() {
    let engine = Engine::new();
    let runtime = Arc::new(Runtime::new(engine, None).await.unwrap());
    let runner: Arc<dyn Runner> = Arc::new(InlineRunner::new(runtime.clone(), RunRegistry::new()));
    register(runtime.engine(), runner).unwrap();

    let wait = runtime.engine().registry().get("runner:wait").unwrap();
    let err = wait
        .invoke(Envelope::test(
            json!({"run_id": "does-not-exist", "timeout_secs": 1}),
        ))
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("not found") || msg.contains("timed out"),
        "unexpected error: {msg}"
    );
}

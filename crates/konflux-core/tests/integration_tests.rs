use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

use konflux::engine::{Engine, EngineConfig};
use konflux::error::ToolError;
use konflux::stream::{ProgressType, StreamEvent};
use konflux::tool::{Tool, ToolContext, ToolInfo};

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
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: Default::default(),
        }
    }
    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        Ok(input)
    }
}

struct FailTool;
#[async_trait]
impl Tool for FailTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "fail".into(),
            description: "Always fails".into(),
            input_schema: json!({}),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: Default::default(),
        }
    }
    async fn invoke(&self, _input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        Err(ToolError::ExecutionFailed {
            message: "Intentional failure".into(),
            retryable: true,
        })
    }
}

struct RetryTool {
    attempts: Arc<tokio::sync::Mutex<HashMap<String, u32>>>,
}
#[async_trait]
impl Tool for RetryTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "retry_tool".into(),
            description: "Fails N times then succeeds".into(),
            input_schema: json!({}),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: Default::default(),
        }
    }
    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let mut attempts = self.attempts.lock().await;
        let count = attempts.entry(ctx.node_id.clone()).or_insert(0);
        *count += 1;

        let fail_until = input
            .get("fail_until")
            .and_then(|v| v.as_u64())
            .or_else(|| {
                input
                    .get("fail_until")
                    .and_then(|v| v.as_str())
                    .and_then(|s| s.parse::<u64>().ok())
            })
            .unwrap_or(0) as u32;

        if *count <= fail_until {
            Err(ToolError::ExecutionFailed {
                message: format!("Failure {}/{}", count, fail_until),
                retryable: true,
            })
        } else {
            Ok(json!({ "attempts": *count }))
        }
    }
}

struct StreamingTool;
#[async_trait]
impl Tool for StreamingTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "stream_tool".into(),
            description: "Streams text deltas".into(),
            input_schema: json!({}),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: true,
            annotations: Default::default(),
        }
    }
    async fn invoke(&self, _input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        Ok(json!({ "status": "done" }))
    }
    async fn invoke_streaming(
        &self,
        _input: Value,
        ctx: &ToolContext,
        sender: konflux::stream::StreamSender,
    ) -> Result<Value, ToolError> {
        let chunks = vec!["Hello", " ", "world", "!"];
        for chunk in chunks {
            sender
                .send(StreamEvent::Progress {
                    node_id: ctx.node_id.clone(),
                    event_type: ProgressType::TextDelta,
                    data: json!(chunk),
                })
                .await
                .ok();
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        Ok(json!({ "status": "streamed" }))
    }
}

// ============================================================
// Helper
// ============================================================

fn setup_engine() -> Engine {
    let _ = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_test_writer()
        .try_init();

    let engine = Engine::new();
    engine.register_tool(Arc::new(EchoTool));
    engine.register_tool(Arc::new(FailTool));
    engine.register_tool(Arc::new(RetryTool {
        attempts: Arc::new(tokio::sync::Mutex::new(HashMap::new())),
    }));
    engine.register_tool(Arc::new(StreamingTool));
    konflux::builtin::register_builtins(&engine);
    engine
}

// ============================================================
// Tests
// ============================================================

#[tokio::test]
async fn test_linear_workflow() {
    let engine = setup_engine();
    let yaml = r#"
workflow: linear
nodes:
  step1:
    do: echo
    with:
      val: "hello"
    then: step2
  step2:
    do: echo
    with:
      val: "{{ step1.val }} world"
    return: true
"#;
    let workflow = engine.parse_yaml(yaml).unwrap();
    let result = engine
        .run(
            &workflow,
            json!({}),
            &["*".to_string()],
            HashMap::new(),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(result["val"], "hello world");
}

#[tokio::test]
async fn test_parallel_workflow() {
    let engine = setup_engine();
    let yaml = r#"
workflow: parallel
nodes:
  start:
    do: echo
    with: { val: "start" }
    then: [branch1, branch2]
  branch1:
    do: echo
    with: { val: "b1" }
    then: join
  branch2:
    do: echo
    with: { val: "b2" }
    then: join
  join:
    do: concat
    with:
      parts: ["{{ branch1.val }}", "{{ branch2.val }}"]
      separator: "-"
    return: true
"#;
    let workflow = engine.parse_yaml(yaml).unwrap();
    let result = engine
        .run(
            &workflow,
            json!({}),
            &["*".to_string()],
            HashMap::new(),
            None,
            None,
        )
        .await
        .unwrap();
    let s = result.as_str().unwrap();
    assert!(s == "b1-b2" || s == "b2-b1");
}

#[tokio::test]
async fn test_conditional_routing() {
    let engine = setup_engine();
    let yaml = r#"
workflow: conditional
nodes:
  check:
    do: echo
    with: { val: "{{ input.val }}" }
    then:
      - when: "check.val == 'a'"
        then: branch_a
      - when: "check.val == 'b'"
        then: branch_b
      - else: true
        then: branch_default
  branch_a:
    do: echo
    with: { res: "A" }
    return: true
  branch_b:
    do: echo
    with: { res: "B" }
    return: true
  branch_default:
    do: echo
    with: { res: "D" }
    return: true
"#;
    let workflow = engine.parse_yaml(yaml).unwrap();

    let res_a = engine
        .run(
            &workflow,
            json!({"val": "a"}),
            &["*".to_string()],
            HashMap::new(),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(res_a["res"], "A");

    let res_b = engine
        .run(
            &workflow,
            json!({"val": "b"}),
            &["*".to_string()],
            HashMap::new(),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(res_b["res"], "B");

    let res_d = engine
        .run(
            &workflow,
            json!({"val": "c"}),
            &["*".to_string()],
            HashMap::new(),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(res_d["res"], "D");
}

#[tokio::test]
async fn test_error_handling_catch() {
    let engine = setup_engine();
    let yaml = r#"
workflow: error_handling
nodes:
  fail_step:
    do: fail
    catch:
      - when: true
        then: recovery
  recovery:
    do: echo
    with: { status: "recovered" }
    return: true
"#;
    let workflow = engine.parse_yaml(yaml).unwrap();
    let result = engine
        .run(
            &workflow,
            json!({}),
            &["*".to_string()],
            HashMap::new(),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(result["status"], "recovered");
}

#[tokio::test]
async fn test_retry_mechanism() {
    let engine = setup_engine();
    let yaml = r#"
workflow: retry_test
nodes:
  try_step:
    do: retry_tool
    with:
      fail_until: 2
    retry:
      times: 3
      delay: 10ms
    return: true
"#;
    let workflow = engine.parse_yaml(yaml).unwrap();
    let result = engine
        .run(
            &workflow,
            json!({}),
            &["*".to_string()],
            HashMap::new(),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(result["attempts"], 3);
}

#[tokio::test]
async fn test_bounded_loop_repeat() {
    let engine = setup_engine();
    let yaml = r#"
workflow: loop_test
nodes:
  counter:
    do: echo
    with:
      count: "{{ iteration + 1 }}"
    repeat:
      until: "counter.count >= 5"
      max: 10
      as: iteration
    return: true
"#;
    let workflow = engine.parse_yaml(yaml).unwrap();
    let result = engine
        .run(
            &workflow,
            json!({}),
            &["*".to_string()],
            HashMap::new(),
            None,
            None,
        )
        .await
        .unwrap();
    let c_str = result["count"].to_string();
    assert!(
        c_str.contains('5'),
        "Expected count to contain 5, got {}",
        c_str
    );
}

#[tokio::test]
async fn test_capability_denial() {
    let engine = setup_engine();
    let yaml = r#"
workflow: cap_test
capabilities: ["echo", "fail"]
nodes:
  step1:
    do: echo
    with: { msg: "hi" }
    then: step2
  step2:
    do: fail
    return: true
"#;
    let workflow = engine.parse_yaml(yaml).unwrap();

    let res = engine
        .run(
            &workflow,
            json!({}),
            &["echo".to_string()],
            HashMap::new(),
            None,
            None,
        )
        .await;
    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(
        err.contains("capability denied")
            || err.contains("not granted")
            || err.contains("MissingCapability")
    );
}

#[tokio::test]
async fn test_streaming_passthrough() {
    let engine = setup_engine();
    let yaml = r#"
workflow: stream_test
nodes:
  stream_step:
    do: stream_tool
    return: true
"#;
    let workflow = engine.parse_yaml(yaml).unwrap();
    let mut rx = engine
        .run_streaming(
            &workflow,
            json!({}),
            &["*".to_string()],
            HashMap::new(),
            None,
            None,
        )
        .await
        .unwrap();

    let mut deltas = Vec::new();
    while let Some(event) = rx.recv().await {
        if let StreamEvent::Progress {
            event_type: ProgressType::TextDelta,
            data,
            ..
        } = event
        {
            deltas.push(data.as_str().unwrap().to_string());
        }
    }

    assert_eq!(deltas.join(""), "Hello world!");
}

#[tokio::test]
async fn test_large_workflow_max_steps() {
    let mut config = EngineConfig::default();
    config.max_steps = 10;
    let engine = Engine::with_config(config);
    engine.register_tool(Arc::new(EchoTool));

    let mut yaml = "workflow: large\nnodes:\n".to_string();
    for i in 1..=15 {
        yaml.push_str(&format!("  step{}:\n    do: echo\n", i));
        if i < 15 {
            yaml.push_str(&format!("    then: step{}\n", i + 1));
        } else {
            yaml.push_str("    return: true\n");
        }
    }

    let workflow = engine.parse_yaml(&yaml).unwrap();
    let res = engine
        .run(
            &workflow,
            json!({}),
            &["*".to_string()],
            HashMap::new(),
            None,
            None,
        )
        .await;

    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(err.to_lowercase().contains("max steps exceeded"));
}

#[tokio::test]
async fn test_nested_workflow_capabilities() {
    struct SubWorkflowTool {
        engine: Arc<Engine>,
    }
    #[async_trait]
    impl Tool for SubWorkflowTool {
        fn info(&self) -> ToolInfo {
            ToolInfo {
                name: "workflow_execute".into(),
                description: "Runs a sub-workflow".into(),
                input_schema: json!({}),
                output_schema: None,
                capabilities: vec!["*".to_string()],
                supports_streaming: false,
                annotations: Default::default(),
            }
        }
        async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<Value, ToolError> {
            let yaml = input["yaml"].as_str().ok_or(ToolError::InvalidInput {
                message: "missing yaml".into(),
                field: None,
            })?;
            let workflow =
                self.engine
                    .parse_yaml(yaml)
                    .map_err(|e| ToolError::ExecutionFailed {
                        message: e.to_string(),
                        retryable: false,
                    })?;
            let granted = ctx.capabilities.clone();
            self.engine
                .run(
                    &workflow,
                    input["input"].clone(),
                    &granted,
                    HashMap::new(),
                    None,
                    None,
                )
                .await
                .map_err(|e| ToolError::ExecutionFailed {
                    message: e.to_string(),
                    retryable: false,
                })
        }
    }

    let engine = setup_engine();
    let engine_arc = Arc::new(setup_engine());
    engine.register_tool(Arc::new(SubWorkflowTool { engine: engine_arc }));

    let yaml = r#"
workflow: parent
nodes:
  sub:
    do: "workflow_execute"
    grant: ["echo"]
    with:
      yaml: |
        workflow: child
        capabilities: ["fail"]
        nodes:
          step1:
            do: fail
            return: true
      input: {}
    return: true
"#;

    let workflow = engine.parse_yaml(yaml).unwrap();
    let caps = vec![
        "workflow_execute".to_string(),
        "echo".to_string(),
        "fail".to_string(),
    ];
    let res = engine
        .run(&workflow, json!({}), &caps, HashMap::new(), None, None)
        .await;
    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(
        err.contains("capability")
            || err.contains("denied")
            || err.contains("granted")
            || err.contains("MissingCapability")
    );
}

#[tokio::test]
async fn test_empty_workflow() {
    let engine = setup_engine();
    let yaml = r#"
workflow: empty
nodes: {}
"#;
    let res = engine.parse_yaml(yaml);
    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(
        err.to_lowercase().contains("entry node")
            || err.to_lowercase().contains("at least one node")
            || err.to_lowercase().contains("no nodes")
    );
}

#[tokio::test]
async fn test_workflow_only_return() {
    let engine = setup_engine();
    let yaml = r#"
workflow: single_return
nodes:
  step1:
    return: "static"
"#;
    let workflow = engine.parse_yaml(yaml).unwrap();
    let res = engine
        .run(
            &workflow,
            json!({}),
            &["*".to_string()],
            HashMap::new(),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(res["__return__"], "static");
}

#[tokio::test]
async fn test_panic_tool() {
    struct PanicTool;
    #[async_trait]
    impl Tool for PanicTool {
        fn info(&self) -> ToolInfo {
            ToolInfo {
                name: "panic".into(),
                description: "".into(),
                input_schema: json!({}),
                output_schema: None,
                capabilities: vec![],
                supports_streaming: false,
                annotations: Default::default(),
            }
        }
        async fn invoke(&self, _input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
            panic!("Intentional panic!");
        }
    }

    let engine = setup_engine();
    engine.register_tool(Arc::new(PanicTool));
    let yaml = r#"
workflow: panic
nodes:
  step1:
    do: panic
    return: true
"#;
    let workflow = engine.parse_yaml(yaml).unwrap();
    let res = engine
        .run(
            &workflow,
            json!({}),
            &["*".to_string()],
            HashMap::new(),
            None,
            None,
        )
        .await;
    assert!(res.is_err());
    assert!(res.unwrap_err().to_string().contains("panic"));
}

#[tokio::test]
async fn test_concurrent_workflows_on_same_engine() {
    let engine = Arc::new(setup_engine());
    let yaml = r#"
workflow: concurrent
nodes:
  step1:
    do: echo
    with: { val: "ok" }
    return: true
"#;
    let workflow = engine.parse_yaml(yaml).unwrap();

    let mut handles = Vec::new();
    for _ in 0..10 {
        let e = engine.clone();
        let w = workflow.clone();
        handles.push(tokio::spawn(async move {
            e.run(
                &w,
                json!({}),
                &["*".to_string()],
                HashMap::new(),
                None,
                None,
            )
            .await
            .unwrap()
        }));
    }

    for h in handles {
        let res = h.await.unwrap();
        assert_eq!(res["val"], "ok");
    }
}

#[tokio::test]
async fn test_large_inputs_memory_pressure() {
    let engine = setup_engine();
    let yaml = r#"
workflow: large_inputs
nodes:
  step1:
    do: echo
    with: { large: "{{ input.large }}" }
    return: true
"#;
    let workflow = engine.parse_yaml(yaml).unwrap();

    // Create a 10MB string
    let large_string = "A".repeat(10 * 1024 * 1024);
    let res = engine
        .run(
            &workflow,
            json!({ "large": large_string }),
            &["*".to_string()],
            HashMap::new(),
            None,
            None,
        )
        .await
        .unwrap();
    assert_eq!(res["large"].as_str().unwrap().len(), 10 * 1024 * 1024);
}

#[tokio::test]
async fn test_invalid_timeout_errors() {
    let engine = setup_engine();
    let yaml = r#"
workflow: bad_timeout
nodes:
  step1:
    do: echo
    timeout: "30z"
    return: true
"#;
    let result = engine.parse_yaml(yaml);
    assert!(result.is_err());
    let err = result.unwrap_err().to_string();
    assert!(
        err.contains("invalid") && err.contains("duration"),
        "Expected duration parse error, got: {err}"
    );
}

#[tokio::test]
async fn test_valid_timeout_parses() {
    let engine = setup_engine();
    for (duration, _label) in [
        ("30s", "seconds"),
        ("500ms", "millis"),
        ("2m", "minutes"),
        ("10", "bare number"),
    ] {
        let yaml = format!(
            r#"
workflow: timeout_test
nodes:
  step1:
    do: echo
    with: {{ val: "ok" }}
    timeout: "{duration}"
    return: true
"#
        );
        let result = engine.parse_yaml(&yaml);
        assert!(
            result.is_ok(),
            "Failed to parse timeout '{duration}': {:?}",
            result.err()
        );
    }
}

#[tokio::test]
async fn test_global_workflow_timeout() {
    let config = EngineConfig {
        max_workflow_timeout_ms: 100, // 100ms timeout
        ..EngineConfig::default()
    };
    let engine = Engine::with_config(config);
    engine.register_tool(Arc::new(SlowTool));
    konflux::builtin::register_builtins(&engine);

    let yaml = r#"
workflow: timeout_test
nodes:
  step1:
    do: slow
    return: true
"#;
    let workflow = engine.parse_yaml(yaml).unwrap();
    let res = engine
        .run(
            &workflow,
            json!({}),
            &["*".to_string()],
            HashMap::new(),
            None,
            None,
        )
        .await;
    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(
        err.to_lowercase().contains("timeout") || err.to_lowercase().contains("timed out"),
        "Expected timeout error, got: {err}"
    );
}

#[tokio::test]
async fn test_yaml_size_limit() {
    let config = EngineConfig {
        max_yaml_size: 100, // 100 bytes
        ..EngineConfig::default()
    };
    let engine = Engine::with_config(config);
    let big_yaml = "a".repeat(200);
    let res = engine.parse_yaml(&big_yaml);
    assert!(res.is_err());
    assert!(res.unwrap_err().to_string().contains("exceeds maximum"));
}

#[tokio::test]
async fn test_config_accessible() {
    let config = EngineConfig {
        max_steps: 42,
        default_timeout_ms: 999,
        ..EngineConfig::default()
    };
    let engine = Engine::with_config(config);
    assert_eq!(engine.config().max_steps, 42);
    assert_eq!(engine.config().default_timeout_ms, 999);
}

// --- Cancellation test ---

struct SlowTool;
#[async_trait]
impl Tool for SlowTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "slow".into(),
            description: "Sleeps for 500ms".into(),
            input_schema: json!({}),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: Default::default(),
        }
    }
    async fn invoke(&self, _input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        tokio::time::sleep(std::time::Duration::from_millis(500)).await;
        Ok(json!({"status": "done"}))
    }
}

#[tokio::test]
async fn test_cancellation() {
    let engine = setup_engine();
    engine.register_tool(Arc::new(SlowTool));

    let yaml = r#"
workflow: cancel_test
nodes:
  step1:
    do: slow
    return: true
"#;
    let workflow = engine.parse_yaml(yaml).unwrap();
    let token = tokio_util::sync::CancellationToken::new();
    let token_clone = token.clone();

    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        token_clone.cancel();
    });

    let res = engine
        .run(
            &workflow,
            json!({}),
            &["*".to_string()],
            HashMap::new(),
            Some(token),
            None,
        )
        .await;
    assert!(res.is_err());
    let err = res.unwrap_err().to_string();
    assert!(
        err.contains("cancelled"),
        "Expected cancelled error, got: {err}"
    );
}

// --- Hooks test ---

#[tokio::test]
async fn test_hooks_receive_events() {
    use std::sync::Mutex;

    #[derive(Default)]
    struct TestHooks {
        events: Mutex<Vec<String>>,
    }
    impl konflux::hooks::ExecutionHooks for TestHooks {
        fn on_node_start(&self, node_id: &str, tool: &str) {
            self.events
                .lock()
                .unwrap()
                .push(format!("start:{node_id}:{tool}"));
        }
        fn on_node_complete(&self, node_id: &str, tool: &str, _duration_ms: u64, _output: &Value) {
            self.events
                .lock()
                .unwrap()
                .push(format!("complete:{node_id}:{tool}"));
        }
        fn on_node_failed(&self, node_id: &str, tool: &str, _error: &str) {
            self.events
                .lock()
                .unwrap()
                .push(format!("failed:{node_id}:{tool}"));
        }
    }

    let engine = setup_engine();
    let yaml = r#"
workflow: hooks_test
nodes:
  step1:
    do: echo
    with: { val: "hello" }
    then: step2
  step2:
    do: echo
    with: { val: "world" }
    return: true
"#;
    let workflow = engine.parse_yaml(yaml).unwrap();
    let hooks = Arc::new(TestHooks::default());
    let hooks_clone: Arc<dyn konflux::hooks::ExecutionHooks> = hooks.clone();

    let _result = engine
        .run(
            &workflow,
            json!({}),
            &["*".to_string()],
            HashMap::new(),
            None,
            Some(hooks_clone),
        )
        .await
        .unwrap();

    let events = hooks.events.lock().unwrap();
    assert!(
        events.contains(&"start:step1:echo".to_string()),
        "Missing step1 start, got: {:?}",
        events
    );
    assert!(
        events.contains(&"complete:step1:echo".to_string()),
        "Missing step1 complete, got: {:?}",
        events
    );
    assert!(
        events.contains(&"start:step2:echo".to_string()),
        "Missing step2 start, got: {:?}",
        events
    );
    assert!(
        events.contains(&"complete:step2:echo".to_string()),
        "Missing step2 complete, got: {:?}",
        events
    );
}

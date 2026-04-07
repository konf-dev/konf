//! Context virtualization — namespace injection via VirtualizedTool.
//!
//! Wraps tools to inject bound parameters (e.g., namespace) into tool input
//! before invocation. The LLM never sees these parameters — they're invisible
//! and cannot be overridden. This is the Plan 9-style namespace mounting.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use konflux::tool::{Tool, ToolInfo, ToolContext};
use konflux::error::ToolError;
use konflux::stream::StreamSender;

/// A tool wrapper that injects bound parameters into input before invocation.
///
/// Example: if bindings = {"namespace": "konf:unspool:user_123"},
/// the LLM calls `memory:search(query="exercise")` and the actual invocation
/// becomes `memory:search(query="exercise", namespace="konf:unspool:user_123")`.
///
/// Bindings override any existing keys — the LLM cannot escape its namespace.
pub struct VirtualizedTool {
    inner: Arc<dyn Tool>,
    bindings: HashMap<String, Value>,
}

impl VirtualizedTool {
    pub fn new(inner: Arc<dyn Tool>, bindings: HashMap<String, Value>) -> Self {
        Self { inner, bindings }
    }

    fn inject_bindings(&self, mut input: Value) -> Value {
        if let Value::Object(ref mut map) = input {
            for (k, v) in &self.bindings {
                map.insert(k.clone(), v.clone());
            }
        }
        input
    }
}

#[async_trait]
impl Tool for VirtualizedTool {
    fn info(&self) -> ToolInfo {
        self.inner.info()
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        let input = self.inject_bindings(input);
        self.inner.invoke(input, ctx).await
    }

    async fn invoke_streaming(
        &self,
        input: Value,
        ctx: &ToolContext,
        sender: StreamSender,
    ) -> Result<Value, ToolError> {
        let input = self.inject_bindings(input);
        self.inner.invoke_streaming(input, ctx, sender).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    struct MockTool;

    #[async_trait]
    impl Tool for MockTool {
        fn info(&self) -> ToolInfo {
            ToolInfo {
                name: "mock".into(),
                description: "test".into(),
                input_schema: json!({}),
                output_schema: None,
                capabilities: vec![],
                supports_streaming: false,
                annotations: Default::default(),
            }
        }
        async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
            // Return the input so tests can inspect what was injected
            Ok(input)
        }
    }

    fn test_ctx() -> ToolContext {
        ToolContext {
            capabilities: vec![],
            workflow_id: "test".into(),
            node_id: "test".into(),
            metadata: HashMap::new(),
        }
    }

    #[tokio::test]
    async fn test_injects_namespace() {
        let mut bindings = HashMap::new();
        bindings.insert("namespace".to_string(), json!("konf:unspool:user_123"));

        let tool = VirtualizedTool::new(Arc::new(MockTool), bindings);
        let result = tool.invoke(json!({"query": "hello"}), &test_ctx()).await.unwrap();

        assert_eq!(result["query"], "hello");
        assert_eq!(result["namespace"], "konf:unspool:user_123");
    }

    #[tokio::test]
    async fn test_overrides_llm_set_namespace() {
        let mut bindings = HashMap::new();
        bindings.insert("namespace".to_string(), json!("konf:unspool:user_123"));

        let tool = VirtualizedTool::new(Arc::new(MockTool), bindings);
        // LLM tries to set namespace to something else — should be overridden
        let result = tool.invoke(
            json!({"query": "hello", "namespace": "konf:unspool:admin"}),
            &test_ctx(),
        ).await.unwrap();

        assert_eq!(result["namespace"], "konf:unspool:user_123");
    }

    #[tokio::test]
    async fn test_no_bindings_passthrough() {
        let tool = VirtualizedTool::new(Arc::new(MockTool), HashMap::new());
        let result = tool.invoke(json!({"query": "hello"}), &test_ctx()).await.unwrap();

        assert_eq!(result["query"], "hello");
        assert!(result.get("namespace").is_none());
    }
}

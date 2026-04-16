//! Context virtualization — namespace injection via VirtualizedTool.
//!
//! Wraps tools to inject bound parameters (e.g., namespace) into tool input
//! before invocation. The LLM never sees these parameters — they're invisible
//! and cannot be overridden. This is the Plan 9-style namespace mounting.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use konflux_substrate::envelope::Envelope;
use konflux_substrate::error::ToolError;
use konflux_substrate::stream::StreamSender;
use konflux_substrate::tool::{Tool, ToolInfo};

/// A tool wrapper that injects bound parameters into input before invocation.
///
/// Example: if bindings = {"namespace": "konf:unspool:user_123"},
/// the LLM calls `memory_search(query="exercise")` and the actual invocation
/// becomes `memory_search(query="exercise", namespace="konf:unspool:user_123")`.
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

    fn inject_bindings(&self, env: &mut Envelope<Value>) -> Result<(), ToolError> {
        if self.bindings.is_empty() {
            return Ok(());
        }
        let map = env
            .payload
            .as_object_mut()
            .ok_or_else(|| ToolError::InvalidInput {
                message: "payload must be a JSON object when bindings are present".into(),
                field: None,
            })?;
        for (k, v) in &self.bindings {
            map.insert(k.clone(), v.clone());
        }
        Ok(())
    }
}

#[async_trait]
impl Tool for VirtualizedTool {
    fn info(&self) -> ToolInfo {
        self.inner.info()
    }

    async fn invoke(&self, mut env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        self.inject_bindings(&mut env)?;
        self.inner.invoke(env).await
    }

    fn projection(&self) -> Option<&dyn konflux_substrate::projection::StateProjection> {
        self.inner.projection()
    }

    async fn invoke_streaming(
        &self,
        mut env: Envelope<Value>,
        sender: StreamSender,
    ) -> Result<Envelope<Value>, ToolError> {
        self.inject_bindings(&mut env)?;
        self.inner.invoke_streaming(env, sender).await
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
        async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
            // Return the payload so tests can inspect what was injected
            Ok(env.respond(env.payload.clone()))
        }
    }

    #[tokio::test]
    async fn test_injects_namespace() {
        let mut bindings = HashMap::new();
        bindings.insert("namespace".to_string(), json!("konf:unspool:user_123"));

        let tool = VirtualizedTool::new(Arc::new(MockTool), bindings);
        let result = tool
            .invoke(Envelope::test(json!({"query": "hello"})))
            .await
            .unwrap();

        assert_eq!(result.payload["query"], "hello");
        assert_eq!(result.payload["namespace"], "konf:unspool:user_123");
    }

    #[tokio::test]
    async fn test_overrides_llm_set_namespace() {
        let mut bindings = HashMap::new();
        bindings.insert("namespace".to_string(), json!("konf:unspool:user_123"));

        let tool = VirtualizedTool::new(Arc::new(MockTool), bindings);
        // LLM tries to set namespace to something else — should be overridden
        let result = tool
            .invoke(Envelope::test(
                json!({"query": "hello", "namespace": "konf:unspool:admin"}),
            ))
            .await
            .unwrap();

        assert_eq!(result.payload["namespace"], "konf:unspool:user_123");
    }

    #[tokio::test]
    async fn test_no_bindings_passthrough() {
        let tool = VirtualizedTool::new(Arc::new(MockTool), HashMap::new());
        let result = tool
            .invoke(Envelope::test(json!({"query": "hello"})))
            .await
            .unwrap();

        assert_eq!(result.payload["query"], "hello");
        assert!(result.payload.get("namespace").is_none());
    }
}

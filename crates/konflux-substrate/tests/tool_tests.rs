use async_trait::async_trait;
use konflux_substrate::envelope::Envelope;
use konflux_substrate::error::ToolError;
use konflux_substrate::stream::stream_channel;
use konflux_substrate::tool::{Tool, ToolInfo, ToolRegistry};
use serde_json::json;
use std::sync::Arc;

struct MockTool {
    info: ToolInfo,
}

#[async_trait]
impl Tool for MockTool {
    fn info(&self) -> ToolInfo {
        self.info.clone()
    }

    async fn invoke(
        &self,
        env: Envelope<serde_json::Value>,
    ) -> Result<Envelope<serde_json::Value>, ToolError> {
        let output = env.payload.clone();
        Ok(env.respond(output))
    }
}

#[tokio::test]
async fn test_tool_registry() {
    let mut registry = ToolRegistry::new();
    let tool = Arc::new(MockTool {
        info: ToolInfo {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: json!({}),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: Default::default(),
        },
    });

    registry.register(tool.clone());

    assert!(registry.contains("test_tool"));
    assert_eq!(registry.len(), 1);
    assert!(!registry.is_empty());

    let retrieved = registry.get("test_tool").unwrap();
    assert_eq!(retrieved.info().name, "test_tool");

    let list = registry.list();
    assert_eq!(list.len(), 1);
    assert_eq!(list[0].name, "test_tool");

    assert!(registry.get("non_existent").is_none());
}

#[tokio::test]
async fn test_envelope_test_helper() {
    let env = Envelope::test(json!({"key": "value"}));
    assert_eq!(env.payload["key"], "value");
    assert_eq!(env.actor_id.0, "test");
    assert_eq!(env.namespace.0, "test");
}

#[tokio::test]
async fn test_default_invoke_streaming() {
    let tool = MockTool {
        info: ToolInfo {
            name: "test_tool".to_string(),
            description: "A test tool".to_string(),
            input_schema: json!({}),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: Default::default(),
        },
    };

    let (tx, _rx) = stream_channel(10);
    let env = Envelope::test(json!({"foo": "bar"}));
    let result = tool.invoke_streaming(env, tx).await.unwrap();
    assert_eq!(result.payload, json!({"foo": "bar"}));
}

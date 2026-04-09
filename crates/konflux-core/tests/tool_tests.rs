use async_trait::async_trait;
use konflux::error::ToolError;
use konflux::stream::stream_channel;
use konflux::tool::{Tool, ToolContext, ToolInfo, ToolRegistry};
use serde_json::json;
use std::collections::HashMap;
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
        input: serde_json::Value,
        _ctx: &ToolContext,
    ) -> Result<serde_json::Value, ToolError> {
        Ok(input)
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
async fn test_tool_context() {
    let ctx = ToolContext {
        capabilities: vec!["cap1".to_string()],
        workflow_id: "wf123".to_string(),
        node_id: "node456".to_string(),
        metadata: HashMap::new(),
    };

    assert_eq!(ctx.capabilities, vec!["cap1"]);
    assert_eq!(ctx.workflow_id, "wf123");
    assert_eq!(ctx.node_id, "node456");
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

    let ctx = ToolContext {
        capabilities: vec![],
        workflow_id: "wf".to_string(),
        node_id: "node".to_string(),
        metadata: HashMap::new(),
    };

    let (tx, _rx) = stream_channel(10);
    let result = tool
        .invoke_streaming(json!({"foo": "bar"}), &ctx, tx)
        .await
        .unwrap();
    assert_eq!(result, json!({"foo": "bar"}));
}

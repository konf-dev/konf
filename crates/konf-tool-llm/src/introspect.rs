//! Introspection tool — lists all registered tools with metadata.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use konflux::engine::Engine;
use konflux::error::ToolError as KonfluxToolError;
use konflux::tool::{Tool, ToolAnnotations, ToolContext, ToolInfo};

/// Lists all registered tools with their names, descriptions, input schemas, and annotations.
pub struct IntrospectTool {
    engine: Arc<Engine>,
}

impl IntrospectTool {
    /// Create a new `IntrospectTool` with access to the engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

/// Check if a tool name matches a glob filter pattern.
fn matches_filter(pattern: &str, tool_name: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix(":*") {
        return tool_name.starts_with(prefix)
            && tool_name.get(prefix.len()..prefix.len() + 1) == Some(":");
    }
    pattern == tool_name
}

#[async_trait]
impl Tool for IntrospectTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "system:introspect".into(),
            description: "List all registered tools with their names, descriptions, input schemas, and annotations.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "filter": {
                        "type": "string",
                        "description": "Optional glob pattern to filter tools (e.g. 'memory:*' or 'ai:complete')"
                    }
                }
            }),
            capabilities: vec![],
            supports_streaming: false,
            output_schema: None,
            annotations: ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            },
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, KonfluxToolError> {
        let filter = input.get("filter")
            .and_then(|v| v.as_str());

        let registry = self.engine.registry();
        let all_tools = registry.list();

        let filtered: Vec<&ToolInfo> = match filter {
            Some(pattern) => all_tools.iter()
                .filter(|info| matches_filter(pattern, &info.name))
                .collect(),
            None => all_tools.iter().collect(),
        };

        let tools_json: Vec<Value> = filtered.iter()
            .map(|info| {
                json!({
                    "name": info.name,
                    "description": info.description,
                    "input_schema": info.input_schema,
                    "annotations": {
                        "read_only": info.annotations.read_only,
                        "destructive": info.annotations.destructive,
                        "idempotent": info.annotations.idempotent,
                        "open_world": info.annotations.open_world,
                    }
                })
            })
            .collect();

        let count = tools_json.len();
        Ok(json!({
            "tools": tools_json,
            "count": count,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn make_engine() -> Arc<Engine> {
        Arc::new(Engine::new())
    }

    #[test]
    fn test_introspect_tool_info() {
        let tool = IntrospectTool::new(make_engine());
        let info = tool.info();

        assert_eq!(info.name, "system:introspect");
        assert!(info.capabilities.is_empty());
        assert!(info.annotations.read_only);
        assert!(info.annotations.idempotent);
        assert!(!info.annotations.destructive);
        assert!(!info.annotations.open_world);
    }

    #[test]
    fn test_introspect_returns_registered_tools() {
        let engine = make_engine();

        // Register a dummy tool
        struct DummyTool;
        #[async_trait]
        impl Tool for DummyTool {
            fn info(&self) -> ToolInfo {
                ToolInfo {
                    name: "test:dummy".into(),
                    description: "A dummy tool".into(),
                    input_schema: json!({"type": "object"}),
                    output_schema: None,
                    capabilities: vec![],
                    supports_streaming: false,
                    annotations: ToolAnnotations::default(),
                }
            }
            async fn invoke(&self, _input: Value, _ctx: &ToolContext) -> Result<Value, KonfluxToolError> {
                Ok(Value::Null)
            }
        }

        engine.register_tool(Arc::new(DummyTool));

        let introspect = IntrospectTool::new(engine);
        let ctx = ToolContext {
            capabilities: vec!["*".into()],
            workflow_id: "test".into(),
            node_id: "test".into(),
            metadata: HashMap::new(),
        };

        let rt = tokio::runtime::Runtime::new().unwrap();
        let result = rt.block_on(introspect.invoke(json!({}), &ctx)).unwrap();
        assert_eq!(result["count"], 1);
        assert_eq!(result["tools"][0]["name"], "test:dummy");
    }

    #[test]
    fn test_matches_filter() {
        assert!(matches_filter("memory:*", "memory:search"));
        assert!(matches_filter("memory:*", "memory:store"));
        assert!(!matches_filter("memory:*", "ai:complete"));
        assert!(matches_filter("*", "anything"));
        assert!(matches_filter("ai:complete", "ai:complete"));
        assert!(!matches_filter("ai:complete", "ai:other"));
    }
}

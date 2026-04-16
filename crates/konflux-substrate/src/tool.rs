//! Tool abstraction — the interface between the engine and the outside world.
//!
//! Every tool (Rust, MCP, Python) implements the same [`Tool`] trait and publishes
//! identical [`ToolInfo`] metadata. The engine dispatches them identically —
//! the agent cannot tell where a tool lives.

use std::collections::HashMap;
use std::sync::Arc;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::envelope::Envelope;
use crate::error::ToolError;
use crate::stream::StreamSender;

/// A tool is anything that takes an envelope and returns an envelope.
/// Tools are registered by the consumer, not built into the engine.
///
/// The `Envelope<Value>` carries the payload (tool input/output) plus
/// typed context: trace_id, namespace, capabilities, actor_id, etc.
/// Tools access input via `env.payload` and return output via
/// `env.respond(output)`.
#[async_trait]
pub trait Tool: Send + Sync {
    /// Tool metadata: name, description, schemas, capabilities, annotations.
    fn info(&self) -> ToolInfo;

    /// Execute the tool.
    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError>;

    /// Execute with streaming. Default: non-streaming fallback.
    /// Tools should only push Progress events (e.g., TextDelta) to the sender.
    /// The executor handles ToolStart/ToolEnd wrapping and the final Done event.
    async fn invoke_streaming(
        &self,
        env: Envelope<Value>,
        _sender: StreamSender,
    ) -> Result<Envelope<Value>, ToolError> {
        self.invoke(env).await
    }
}

/// Tool metadata for registration, capability matching, and LLM tool-calling schemas.
/// Fields are MCP-aligned — same metadata regardless of tool source (Rust, MCP, Python).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolInfo {
    /// Unique tool name, e.g. "memory:search", "ai:complete", "workflow:summarize"
    pub name: String,

    /// Human-readable description shown to the LLM
    pub description: String,

    /// JSON Schema defining expected input parameters
    pub input_schema: Value,

    /// JSON Schema defining expected output shape (aids LLM reasoning). Optional.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,

    /// Required capability grants to invoke this tool
    pub capabilities: Vec<String>,

    /// Whether this tool supports streaming via invoke_streaming()
    pub supports_streaming: bool,

    /// Behavioral hints from MCP's tool annotation vocabulary.
    /// Enables smart engine decisions: auto-retry idempotent tools,
    /// warn before destructive ones, skip capability checks for read-only.
    #[serde(default)]
    pub annotations: ToolAnnotations,
}

/// Behavioral hints for a tool, derived from MCP's tool annotation vocabulary.
///
/// These hints enable the engine, UI, and orchestration layers to make
/// informed decisions without inspecting tool internals.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct ToolAnnotations {
    /// Tool has no side effects — safe to call speculatively
    #[serde(default)]
    pub read_only: bool,

    /// Tool deletes or irreversibly modifies data — warn before calling
    #[serde(default)]
    pub destructive: bool,

    /// Calling with the same input produces the same result — safe to retry
    #[serde(default)]
    pub idempotent: bool,

    /// Tool interacts with external services beyond the Konf platform
    #[serde(default)]
    pub open_world: bool,
}

/// Context passed to tools during invocation.
#[derive(Debug, Clone)]
pub struct ToolContext {
    /// Capabilities granted to the current execution scope
    pub capabilities: Vec<String>,

    /// Workflow that initiated this tool call
    pub workflow_id: String,

    /// Node within the workflow
    pub node_id: String,

    /// Arbitrary metadata (session_id, user_id, config_version, etc.)
    pub metadata: HashMap<String, Value>,
}

/// Registry of available tools, keyed by name.
#[derive(Default, Clone)]
pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn new() -> Self {
        Self::default()
    }

    /// Register a tool. Overwrites any existing tool with the same name.
    pub fn register(&mut self, tool: Arc<dyn Tool>) {
        let name = tool.info().name.clone();
        self.tools.insert(name, tool);
    }

    /// Remove a tool by name. Returns true if the tool was present.
    /// Used by hot-reload to toggle tools on/off without restart.
    pub fn remove(&mut self, name: &str) -> bool {
        self.tools.remove(name).is_some()
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }

    pub fn list(&self) -> Vec<ToolInfo> {
        self.tools.values().map(|t| t.info()).collect()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.tools.contains_key(name)
    }

    pub fn len(&self) -> usize {
        self.tools.len()
    }

    pub fn is_empty(&self) -> bool {
        self.tools.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_annotations_default_all_false() {
        let ann = ToolAnnotations::default();
        assert!(!ann.read_only);
        assert!(!ann.destructive);
        assert!(!ann.idempotent);
        assert!(!ann.open_world);
    }

    #[test]
    fn test_tool_annotations_serde_roundtrip() {
        let ann = ToolAnnotations {
            read_only: true,
            destructive: false,
            idempotent: true,
            open_world: false,
        };
        let json = serde_json::to_value(&ann).unwrap();
        assert_eq!(json["read_only"], true);
        assert_eq!(json["idempotent"], true);
        let deserialized: ToolAnnotations = serde_json::from_value(json).unwrap();
        assert_eq!(ann, deserialized);
    }

    #[test]
    fn test_tool_info_output_schema_optional() {
        let info = ToolInfo {
            name: "test".into(),
            description: "test tool".into(),
            input_schema: serde_json::json!({"type": "object"}),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: ToolAnnotations::default(),
        };
        let json = serde_json::to_value(&info).unwrap();
        assert!(json.get("output_schema").is_none()); // skipped when None
    }

    #[test]
    fn test_tool_registry_remove() {
        use std::sync::Arc;

        struct DummyTool;
        #[async_trait]
        impl Tool for DummyTool {
            fn info(&self) -> ToolInfo {
                ToolInfo {
                    name: "test_dummy".into(),
                    description: "".into(),
                    input_schema: serde_json::json!({}),
                    output_schema: None,
                    capabilities: vec![],
                    supports_streaming: false,
                    annotations: ToolAnnotations::default(),
                }
            }
            async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
                Ok(env.respond(Value::Null))
            }
        }

        let mut registry = ToolRegistry::new();
        registry.register(Arc::new(DummyTool));
        assert_eq!(registry.len(), 1);
        assert!(registry.contains("test_dummy"));

        assert!(registry.remove("test_dummy"));
        assert_eq!(registry.len(), 0);
        assert!(!registry.contains("test_dummy"));

        // Remove non-existent returns false
        assert!(!registry.remove("nonexistent"));
    }
}

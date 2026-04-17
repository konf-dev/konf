//! Builtin tools for Konflux.
//!
//! Lightweight, stateless tools for workflow composition.
//! All builtins are read-only and idempotent — safe to retry and call speculatively.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::info;

use crate::engine::Engine;
use crate::envelope::Envelope;
use crate::error::ToolError;
use crate::tool::{Tool, ToolAnnotations, ToolInfo};

/// Annotations shared by all builtins: read-only, idempotent, no external I/O.
const BUILTIN_ANNOTATIONS: ToolAnnotations = ToolAnnotations {
    read_only: true,
    destructive: false,
    idempotent: true,
    open_world: false,
};

// ============================================================
// Echo Tool
// ============================================================

pub struct EchoTool;

#[async_trait]
impl Tool for EchoTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "echo".into(),
            description: "Returns the input as-is.".into(),
            input_schema: json!({"type": "object"}),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: BUILTIN_ANNOTATIONS,
        }
    }

    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        let output = env.payload.clone();
        Ok(env.respond(output))
    }
}

// ============================================================
// JSON Get Tool
// ============================================================

pub struct JsonGetTool;

#[async_trait]
impl Tool for JsonGetTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "json_get".into(),
            description: "Extract a value from JSON using a dot-path.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "data": { "type": "object" },
                    "path": { "type": "string" }
                },
                "required": ["data", "path"]
            }),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: BUILTIN_ANNOTATIONS,
        }
    }

    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        let data = env
            .payload
            .get("data")
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing 'data'".into(),
                field: Some("data".into()),
            })?;
        let path = env
            .payload
            .get("path")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing 'path'".into(),
                field: Some("path".into()),
            })?;

        let parts: Vec<&str> = path.split('.').collect();
        let mut current = data;
        for part in parts {
            if part.is_empty() {
                continue;
            }
            current = current
                .get(part)
                .ok_or_else(|| ToolError::ExecutionFailed {
                    message: format!("Path not found: {}", path),
                    retryable: false,
                })?;
        }

        Ok(env.respond(current.clone()))
    }
}

// ============================================================
// Concat Tool
// ============================================================

pub struct ConcatTool;

#[async_trait]
impl Tool for ConcatTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "concat".into(),
            description: "Concatenate a list of strings.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "parts": { "type": "array", "items": { "type": "string" } },
                    "separator": { "type": "string" }
                },
                "required": ["parts"]
            }),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: BUILTIN_ANNOTATIONS,
        }
    }

    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        let parts = env
            .payload
            .get("parts")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing 'parts' array".into(),
                field: Some("parts".into()),
            })?;
        let separator = env
            .payload
            .get("separator")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        let strings: Vec<String> = parts
            .iter()
            .map(|v| {
                if v.is_string() {
                    v.as_str().unwrap().to_string()
                } else {
                    v.to_string()
                }
            })
            .collect();

        Ok(env.respond(Value::String(strings.join(separator))))
    }
}

// ============================================================
// List Append Tool
// ============================================================

/// Append one or more items to a list, returning the new list. Used by
/// workflows that need to extend an accumulator (e.g. chat history) in
/// YAML without an LLM round-trip.
///
/// `items` is always an array; if you have a single element wrap it in
/// `[...]` at the call site. Keeps the semantics unambiguous — no
/// "single value vs array" branching in the tool.
pub struct ListAppendTool;

#[async_trait]
impl Tool for ListAppendTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "list:append".into(),
            description: "Append items to a list and return the new list. \
                 `list` may be null/absent (treated as empty). `items` must be an array; \
                 its elements are appended in order."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "list":  { "description": "Existing list; null or absent treated as []" },
                    "items": { "type": "array", "description": "Items to append" }
                },
                "required": ["items"]
            }),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: BUILTIN_ANNOTATIONS,
        }
    }

    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        let mut out: Vec<Value> = match env.payload.get("list") {
            None | Some(Value::Null) => Vec::new(),
            Some(Value::Array(arr)) => arr.clone(),
            Some(other) => {
                return Err(ToolError::InvalidInput {
                    message: format!("'list' must be an array or null, got {other}"),
                    field: Some("list".into()),
                });
            }
        };
        let items = env
            .payload
            .get("items")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "'items' must be an array".into(),
                field: Some("items".into()),
            })?;
        out.extend(items.iter().cloned());
        Ok(env.respond(Value::Array(out)))
    }
}

// ============================================================
// Log Tool
// ============================================================

pub struct LogTool;

#[async_trait]
impl Tool for LogTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "log".into(),
            description: "Log a message and return the input.".into(),
            input_schema: json!({"type": "object"}),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: BUILTIN_ANNOTATIONS,
        }
    }

    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        info!(target = %env.target, trace_id = %env.trace_id, "Workflow Log: {:?}", env.payload);
        let output = env.payload.clone();
        Ok(env.respond(output))
    }
}

// ============================================================
// Template Tool
// ============================================================

pub struct TemplateTool;

#[async_trait]
impl Tool for TemplateTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "template".into(),
            description: "Render a Jinja2 template.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "template": { "type": "string" },
                    "vars": { "type": "object" }
                },
                "required": ["template", "vars"]
            }),
            output_schema: None,
            capabilities: vec![],
            supports_streaming: false,
            annotations: BUILTIN_ANNOTATIONS,
        }
    }

    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        let template = env
            .payload
            .get("template")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing 'template'".into(),
                field: Some("template".into()),
            })?;
        let vars = env
            .payload
            .get("vars")
            .and_then(|v| v.as_object())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing 'vars' object".into(),
                field: Some("vars".into()),
            })?;

        let vars_map: std::collections::HashMap<String, Value> =
            vars.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        let rendered = crate::template::render(template, &vars_map).map_err(|e| {
            ToolError::ExecutionFailed {
                message: format!("Template error: {}", e),
                retryable: false,
            }
        })?;

        Ok(env.respond(Value::String(rendered)))
    }
}

/// Register all builtin tools with an engine.
pub fn register_builtins(engine: &Engine) {
    engine.register_tool(Arc::new(EchoTool));
    engine.register_tool(Arc::new(JsonGetTool));
    engine.register_tool(Arc::new(ConcatTool));
    engine.register_tool(Arc::new(ListAppendTool));
    engine.register_tool(Arc::new(LogTool));
    engine.register_tool(Arc::new(TemplateTool));
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::envelope::Envelope;

    #[tokio::test]
    async fn list_append_extends_existing_list() {
        let tool = ListAppendTool;
        let env = Envelope::test(json!({
            "list": [{"role":"user","content":"hi"}],
            "items": [{"role":"assistant","content":"hello"}]
        }));
        let result = tool.invoke(env).await.unwrap();
        assert_eq!(
            result.payload,
            json!([
                {"role":"user","content":"hi"},
                {"role":"assistant","content":"hello"}
            ])
        );
    }

    #[tokio::test]
    async fn list_append_treats_null_list_as_empty() {
        let tool = ListAppendTool;
        let env = Envelope::test(json!({
            "list": null,
            "items": [1, 2, 3]
        }));
        let result = tool.invoke(env).await.unwrap();
        assert_eq!(result.payload, json!([1, 2, 3]));
    }

    #[tokio::test]
    async fn list_append_treats_missing_list_as_empty() {
        let tool = ListAppendTool;
        let env = Envelope::test(json!({ "items": ["first"] }));
        let result = tool.invoke(env).await.unwrap();
        assert_eq!(result.payload, json!(["first"]));
    }

    #[tokio::test]
    async fn list_append_accepts_empty_items() {
        let tool = ListAppendTool;
        let env = Envelope::test(json!({
            "list": [1, 2],
            "items": []
        }));
        let result = tool.invoke(env).await.unwrap();
        assert_eq!(result.payload, json!([1, 2]));
    }

    #[tokio::test]
    async fn list_append_rejects_non_array_list() {
        let tool = ListAppendTool;
        let env = Envelope::test(json!({
            "list": "not an array",
            "items": [1]
        }));
        let result = tool.invoke(env).await;
        assert!(result.is_err(), "expected error, got {result:?}");
    }

    #[tokio::test]
    async fn list_append_rejects_missing_items() {
        let tool = ListAppendTool;
        let env = Envelope::test(json!({ "list": [1, 2] }));
        let result = tool.invoke(env).await;
        assert!(result.is_err(), "expected error, got {result:?}");
    }
}

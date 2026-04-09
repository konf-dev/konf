//! Builtin tools for Konflux.
//!
//! Lightweight, stateless tools for workflow composition.
//! All builtins are read-only and idempotent — safe to retry and call speculatively.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::info;

use crate::engine::Engine;
use crate::error::ToolError;
use crate::tool::{Tool, ToolAnnotations, ToolContext, ToolInfo};

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

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        Ok(input)
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

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let data = input.get("data").ok_or_else(|| ToolError::InvalidInput {
            message: "Missing 'data'".into(),
            field: Some("data".into()),
        })?;
        let path =
            input
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

        Ok(current.clone())
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

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let parts = input
            .get("parts")
            .and_then(|v| v.as_array())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing 'parts' array".into(),
                field: Some("parts".into()),
            })?;
        let separator = input
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

        Ok(Value::String(strings.join(separator)))
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

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        info!(workflow_id = %ctx.workflow_id, node_id = %ctx.node_id, "Workflow Log: {:?}", input);
        Ok(input)
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

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let template = input
            .get("template")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "Missing 'template'".into(),
                field: Some("template".into()),
            })?;
        let vars = input
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

        Ok(Value::String(rendered))
    }
}

/// Register all builtin tools with an engine.
pub fn register_builtins(engine: &Engine) {
    engine.register_tool(Arc::new(EchoTool));
    engine.register_tool(Arc::new(JsonGetTool));
    engine.register_tool(Arc::new(ConcatTool));
    engine.register_tool(Arc::new(LogTool));
    engine.register_tool(Arc::new(TemplateTool));
}

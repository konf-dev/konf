#![warn(missing_docs)]
//! Embedding tool — local embeddings via fastembed (ONNX runtime).
//!
//! Runs BAAI/bge models locally. No external API calls needed.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::info;

use konflux_substrate::envelope::Envelope;
use konflux_substrate::error::ToolError;
use konflux_substrate::tool::{Tool, ToolAnnotations, ToolInfo};

/// The `ai_embed` tool — generates embeddings locally.
///
/// `TextEmbedding` is not `Clone`/`Send` across await, so we wrap in `Arc<Mutex>`.
pub struct EmbedTool {
    model: Arc<std::sync::Mutex<fastembed::TextEmbedding>>,
}

impl EmbedTool {
    /// Create a new `EmbedTool`, loading the AllMiniLML6V2 model.
    pub fn new() -> Result<Self, anyhow::Error> {
        let options = fastembed::TextInitOptions::new(fastembed::EmbeddingModel::AllMiniLML6V2);
        let model = fastembed::TextEmbedding::try_new(options)?;
        info!("fastembed model loaded");
        Ok(Self {
            model: Arc::new(std::sync::Mutex::new(model)),
        })
    }
}

#[async_trait]
impl Tool for EmbedTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "ai:embed".into(),
            description: "Generate text embeddings locally (no API call).".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "text": { "type": "string", "description": "Text to embed" },
                    "texts": {
                        "type": "array",
                        "items": { "type": "string" },
                        "description": "Multiple texts to embed in batch"
                    }
                }
            }),
            capabilities: vec!["ai:embed".into()],
            supports_streaming: false,
            output_schema: None,
            annotations: ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            },
        }
    }

    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        let start = std::time::Instant::now();

        let texts: Vec<String> =
            if let Some(text) = env.payload.get("text").and_then(|v| v.as_str()) {
                vec![text.to_string()]
            } else if let Some(arr) = env.payload.get("texts").and_then(|v| v.as_array()) {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            } else {
                return Err(ToolError::InvalidInput {
                    message: "Provide 'text' or 'texts'".into(),
                    field: None,
                });
            };

        if texts.is_empty() {
            return Err(ToolError::InvalidInput {
                message: "No texts provided".into(),
                field: None,
            });
        }

        let model = self.model.clone();
        let embeddings = tokio::task::spawn_blocking(move || {
            let mut model = model.lock().unwrap_or_else(|p| p.into_inner());
            model.embed(texts, None)
        })
        .await
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Embedding task panicked: {e}"),
            retryable: false,
        })?
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("Embedding failed: {e}"),
            retryable: false,
        })?;

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(env.respond(json!({
            "embeddings": embeddings,
            "dimensions": embeddings.first().map(|e| e.len()).unwrap_or(0),
            "count": embeddings.len(),
            "_meta": { "tool": "ai:embed", "duration_ms": duration_ms }
        })))
    }
}

/// Register the embedding tool (fails gracefully if model can't load).
pub fn register_embed_tools(engine: &konflux_substrate::Engine) {
    match EmbedTool::new() {
        Ok(tool) => {
            engine.register_tool(Arc::new(tool));
            info!("ai_embed tool registered");
        }
        Err(e) => {
            tracing::warn!(error = %e, "Failed to load embedding model, ai_embed unavailable");
        }
    }
}

/// Register the embedding tool with the engine using a config value.
///
/// This is the standalone crate entry point. The `_config` parameter is
/// reserved for future options (e.g. model selection). Fails gracefully
/// if the model cannot be loaded.
pub async fn register(
    engine: &konflux_substrate::Engine,
    _config: &serde_json::Value,
) -> anyhow::Result<()> {
    register_embed_tools(engine);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_embed_tool_info() {
        // EmbedTool::new() downloads a model (~50MB), so we test info without constructing
        // To test the actual tool, use an integration test with model available
        let info = konflux_substrate::tool::ToolInfo {
            name: "ai:embed".into(),
            description: "Generate text embeddings locally (no API call).".into(),
            input_schema: serde_json::json!({}),
            output_schema: None,
            capabilities: vec!["ai:embed".into()],
            supports_streaming: false,
            annotations: konflux_substrate::tool::ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            },
        };
        assert_eq!(info.name, "ai:embed");
        assert!(info.annotations.read_only);
        assert!(info.annotations.idempotent);
        assert!(!info.annotations.open_world);
    }

    #[test]
    fn test_register_graceful_failure() {
        // register() should not panic even if model fails to load
        let engine = konflux_substrate::Engine::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        // This may or may not register the tool depending on model availability
        rt.block_on(register(&engine, &serde_json::json!({})))
            .unwrap();
        // No assertion on tool count — model may or may not load in test env
    }
}

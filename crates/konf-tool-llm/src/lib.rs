#![warn(missing_docs)]
//! LLM tool — wraps rig for multi-provider LLM with tool calling (ReAct loop).
//!
//! The ai:complete tool implements konflux's Tool trait and uses rig internally.
//! Inner tools (memory:search, http:get, etc.) are bridged from konflux::Tool
//! to rig::ToolDyn, enabling the LLM to call them during reasoning.

pub mod validate;
pub mod introspect;

pub use validate::ValidateWorkflowTool;
pub use introspect::IntrospectTool;

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::info;

use konflux::error::ToolError as KonfluxToolError;
use konflux::stream::{StreamSender, StreamEvent, ProgressType};
use konflux::tool::{Tool, ToolAnnotations, ToolContext, ToolInfo};

// ============================================================
// LLM Configuration
// ============================================================

/// Configuration for which LLM provider/model to use.
#[derive(Debug, Clone, serde::Deserialize)]
#[allow(dead_code)] // max_tokens and max_iterations used when rig's completion params are wired
pub struct LlmConfig {
    /// LLM provider name (e.g. "openai", "anthropic", "gemini").
    pub provider: String,
    /// Model identifier (e.g. "gpt-4o", "claude-sonnet-4-20250514").
    pub model: String,
    /// Sampling temperature.
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    /// Maximum tokens in the completion response.
    #[serde(default = "default_max_tokens")]
    pub max_tokens: u64,
    /// Maximum ReAct loop iterations (LLM calls tool → feeds back → repeats).
    #[serde(default = "default_max_iterations")]
    pub max_iterations: usize,
}

fn default_temperature() -> f64 { 0.7 }
fn default_max_tokens() -> u64 { 4096 }
fn default_max_iterations() -> usize { 10 }

impl Default for LlmConfig {
    fn default() -> Self {
        Self {
            provider: "openai".into(),
            model: "gpt-4o".into(),
            temperature: 0.7,
            max_tokens: 4096,
            max_iterations: 10,
        }
    }
}

// ============================================================
// Bridge: konflux::Tool → rig::ToolDyn
// ============================================================

/// Bridges a konflux Tool to rig's dynamic tool interface.
/// This allows the LLM (via rig) to call konflux-registered tools
/// during its ReAct reasoning loop.
struct KonfluxToolBridge {
    inner: Arc<dyn Tool>,
    tool_info: ToolInfo,
}

impl rig::tool::ToolDyn for KonfluxToolBridge {
    fn name(&self) -> String {
        self.tool_info.name.clone()
    }

    fn definition<'a>(
        &'a self,
        _prompt: String,
    ) -> rig::wasm_compat::WasmBoxedFuture<'a, rig::completion::ToolDefinition> {
        let info = self.tool_info.clone();
        Box::pin(async move {
            rig::completion::ToolDefinition {
                name: info.name,
                description: info.description,
                parameters: info.input_schema,
            }
        })
    }

    fn call<'a>(
        &'a self,
        args: String,
    ) -> rig::wasm_compat::WasmBoxedFuture<'a, Result<String, rig::tool::ToolError>> {
        Box::pin(async move {
            let input: Value = serde_json::from_str(&args)
                .map_err(rig::tool::ToolError::JsonError)?;

            let ctx = ToolContext {
                capabilities: vec!["*".into()],
                workflow_id: "react_inner".into(),
                node_id: "react_inner".into(),
                metadata: Default::default(),
            };

            let result = self.inner.invoke(input, &ctx)
                .await
                .map_err(|e| rig::tool::ToolError::ToolCallError(
                    Box::new(std::io::Error::other(e.to_string()))
                ))?;

            serde_json::to_string(&result)
                .map_err(rig::tool::ToolError::JsonError)
        })
    }
}

// ============================================================
// ai:complete Tool
// ============================================================

/// The ai:complete tool — LLM completion with tool calling (ReAct loop).
///
/// When the LLM requests a tool call, rig dispatches it to the bridged
/// konflux tools, feeds the result back, and repeats until the LLM
/// produces a final text response or max_iterations is reached.
pub struct AiCompleteTool {
    config: LlmConfig,
    /// Inner tools available to the LLM during reasoning.
    /// These are bridged from konflux::Tool to rig::ToolDyn.
    inner_tools: Vec<Arc<dyn Tool>>,
}

impl AiCompleteTool {
    /// Create a new `AiCompleteTool` with the given config and inner tools.
    pub fn new(config: LlmConfig, inner_tools: Vec<Arc<dyn Tool>>) -> Self {
        Self { config, inner_tools }
    }

    /// Build rig ToolDyn bridges for all inner tools.
    fn build_rig_tools(&self) -> Vec<Box<dyn rig::tool::ToolDyn>> {
        self.inner_tools
            .iter()
            .map(|tool| {
                let bridge = KonfluxToolBridge {
                    inner: tool.clone(),
                    tool_info: tool.info(),
                };
                Box::new(bridge) as Box<dyn rig::tool::ToolDyn>
            })
            .collect()
    }
}

#[async_trait]
impl Tool for AiCompleteTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "ai:complete".into(),
            description: "Generate an LLM completion with optional tool calling (ReAct loop).".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "system": { "type": "string", "description": "System prompt" },
                    "prompt": { "type": "string", "description": "User prompt" },
                    "messages": {
                        "type": "array",
                        "items": {
                            "type": "object",
                            "properties": {
                                "role": { "type": "string", "enum": ["user", "assistant", "system"] },
                                "content": { "type": "string" }
                            }
                        }
                    },
                    "temperature": { "type": "number" },
                    "max_tokens": { "type": "integer" }
                }
            }),
            capabilities: vec!["ai:complete".into()],
            supports_streaming: true,
            output_schema: None,
            annotations: ToolAnnotations { open_world: true, ..Default::default() },
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, KonfluxToolError> {
        let prompt = input.get("prompt")
            .and_then(|v| v.as_str())
            .unwrap_or("");
        let system = input.get("system")
            .and_then(|v| v.as_str());

        let start = std::time::Instant::now();

        let result = call_rig_with_tools(
            &self.config,
            system,
            prompt,
            self.build_rig_tools(),
        ).await?;

        let duration_ms = start.elapsed().as_millis() as u64;
        info!(
            provider = %self.config.provider,
            model = %self.config.model,
            duration_ms,
            "LLM completion"
        );

        Ok(json!({
            "text": result,
            "_meta": {
                "tool": "ai:complete",
                "provider": self.config.provider,
                "model": self.config.model,
                "duration_ms": duration_ms,
            }
        }))
    }

    async fn invoke_streaming(
        &self,
        input: Value,
        ctx: &ToolContext,
        sender: StreamSender,
    ) -> Result<Value, KonfluxToolError> {
        let result = self.invoke(input, ctx).await?;

        if let Some(text) = result.get("text").and_then(|v| v.as_str()) {
            let _ = sender.try_send(StreamEvent::Progress {
                node_id: ctx.node_id.clone(),
                event_type: ProgressType::TextDelta,
                data: json!(text),
            });
        }

        Ok(result)
    }
}

/// Create a rig agent with tools and call it.
async fn call_rig_with_tools(
    config: &LlmConfig,
    system: Option<&str>,
    prompt: &str,
    tools: Vec<Box<dyn rig::tool::ToolDyn>>,
) -> Result<String, KonfluxToolError> {
    use rig::completion::Prompt;
    use rig::client::{ProviderClient, CompletionClient};

    let err = |e: rig::completion::PromptError| KonfluxToolError::ExecutionFailed {
        message: format!("{} completion failed: {e}", config.provider),
        retryable: true,
    };

    // Always register tools (rig handles empty toolsets gracefully).
    // The agent builder uses typestate — .tools() changes the type,
    // so we always call it to keep one code path.
    match config.provider.as_str() {
        "openai" => {
            let client = rig::providers::openai::Client::from_env();
            let mut builder = client.agent(&config.model).tools(tools);
            if let Some(sys) = system { builder = builder.preamble(sys); }
            builder = builder.temperature(config.temperature);
            builder.build().prompt(prompt).await.map_err(err)
        }
        "anthropic" => {
            let client = rig::providers::anthropic::Client::from_env();
            let mut builder = client.agent(&config.model).tools(tools);
            if let Some(sys) = system { builder = builder.preamble(sys); }
            builder = builder.temperature(config.temperature);
            builder.build().prompt(prompt).await.map_err(err)
        }
        "gemini" => {
            let client = rig::providers::gemini::Client::from_env();
            let mut builder = client.agent(&config.model).tools(tools);
            if let Some(sys) = system { builder = builder.preamble(sys); }
            builder = builder.temperature(config.temperature);
            builder.build().prompt(prompt).await.map_err(err)
        }
        other => Err(KonfluxToolError::ExecutionFailed {
            message: format!("Unknown LLM provider: '{other}'. Supported: openai, anthropic, gemini"),
            retryable: false,
        }),
    }
}

/// Register the ai:complete tool with the engine, deserializing config from a JSON value.
///
/// This creates an [`LlmConfig`] from the provided config, collects all currently
/// registered tools as inner tools for the LLM's ReAct loop, builds the
/// [`AiCompleteTool`], and registers it with the engine.
pub async fn register(engine: &konflux::Engine, config: &serde_json::Value) -> anyhow::Result<()> {
    let llm_config: LlmConfig = serde_json::from_value(config.clone())?;

    let registry = engine.registry();
    let inner_tools: Vec<Arc<dyn Tool>> = registry.list()
        .iter()
        .filter_map(|info| registry.get(&info.name))
        .collect();

    engine.register_tool(Arc::new(AiCompleteTool::new(llm_config, inner_tools)));
    Ok(())
}

/// Register the ai:complete tool with inner tools from the engine registry.
pub fn register_llm_tools(engine: &konflux::Engine, config: &LlmConfig) {
    // Collect all currently registered tools as inner tools for the LLM
    let registry = engine.registry();
    let inner_tools: Vec<Arc<dyn Tool>> = registry.list()
        .iter()
        .filter_map(|info| registry.get(&info.name))
        .collect();

    engine.register_tool(Arc::new(AiCompleteTool::new(config.clone(), inner_tools)));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_llm_config_defaults() {
        let config = LlmConfig::default();
        assert_eq!(config.provider, "openai");
        assert_eq!(config.model, "gpt-4o");
        assert_eq!(config.temperature, 0.7);
        assert_eq!(config.max_iterations, 10);
    }

    #[test]
    fn test_llm_config_from_json() {
        let config: LlmConfig = serde_json::from_value(json!({
            "provider": "anthropic",
            "model": "claude-sonnet-4-20250514",
            "temperature": 0.3
        })).unwrap();
        assert_eq!(config.provider, "anthropic");
        assert_eq!(config.model, "claude-sonnet-4-20250514");
        assert_eq!(config.temperature, 0.3);
        assert_eq!(config.max_iterations, 10);
    }

    #[test]
    fn test_bridge_creation() {
        // Verify that KonfluxToolBridge can be created from a mock tool
        struct MockTool;
        #[async_trait]
        impl Tool for MockTool {
            fn info(&self) -> ToolInfo {
                ToolInfo {
                    name: "mock:test".into(),
                    description: "A mock tool".into(),
                    input_schema: json!({}),
                    output_schema: None,
                    capabilities: vec![],
                    supports_streaming: false,
                    annotations: ToolAnnotations::default(),
                }
            }
            async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, KonfluxToolError> {
                Ok(input)
            }
        }

        let tool = AiCompleteTool::new(LlmConfig::default(), vec![Arc::new(MockTool)]);
        let rig_tools = tool.build_rig_tools();
        assert_eq!(rig_tools.len(), 1);
        assert_eq!(rig_tools[0].name(), "mock:test");
    }
}

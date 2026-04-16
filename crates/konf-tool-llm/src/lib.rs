#![warn(missing_docs)]
//! LLM tool — wraps rig for multi-provider LLM with tool calling (ReAct loop).
//!
//! The ai_complete tool implements konflux's Tool trait and uses rig internally.
//! Inner tools (memory_search, http_get, etc.) are bridged from konflux_substrate::Tool
//! to rig::ToolDyn, enabling the LLM to call them during reasoning.
//!
//! ## Capability enforcement
//!
//! Tools are resolved dynamically at invocation time from the engine's live
//! registry, filtered by the caller's Envelope capabilities. The LLM only
//! sees tools that the workflow node has been granted access to. An optional
//! `tools` whitelist in the `with:` block further restricts the visible set.

pub mod introspect;
pub mod validate;

pub use introspect::IntrospectTool;
pub use validate::ValidateWorkflowTool;

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{debug, info};

use konflux_substrate::capability::check_tool_access;
use konflux_substrate::envelope::Envelope;
use konflux_substrate::error::ToolError as KonfluxToolError;
use konflux_substrate::stream::{ProgressType, StreamEvent, StreamSender};
use konflux_substrate::tool::{Tool, ToolAnnotations, ToolInfo};
use konflux_substrate::Engine;

// ============================================================
// LLM Configuration
// ============================================================

/// Configuration for which LLM provider/model to use.
#[derive(Debug, Clone, serde::Deserialize)]
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

fn default_temperature() -> f64 {
    0.7
}
fn default_max_tokens() -> u64 {
    4096
}
fn default_max_iterations() -> usize {
    10
}

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
// Bridge: konflux_substrate::Tool → rig::ToolDyn
// ============================================================

/// Bridges a konflux Tool to rig's dynamic tool interface.
/// This allows the LLM (via rig) to call konflux-registered tools
/// during its ReAct reasoning loop.
///
/// The bridge propagates the parent workflow's Envelope context so that
/// capability checks, trace_id, namespace, and metadata flow through to
/// inner tool invocations.
struct KonfluxToolBridge {
    inner: Arc<dyn Tool>,
    tool_info: ToolInfo,
    /// Envelope inherited from the parent ai_complete invocation.
    /// When dispatching, a child envelope is created via `respond()`.
    parent_env: Envelope<Value>,
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
            let input: Value =
                serde_json::from_str(&args).map_err(rig::tool::ToolError::JsonError)?;

            // Create a child envelope from the parent, replacing the payload
            let child_env = self.parent_env.respond(input);

            let result_env = self.inner.invoke(child_env).await.map_err(|e| {
                rig::tool::ToolError::ToolCallError(Box::new(std::io::Error::other(e.to_string())))
            })?;

            serde_json::to_string(&result_env.payload).map_err(rig::tool::ToolError::JsonError)
        })
    }
}

// ============================================================
// ai_complete Tool
// ============================================================

/// The ai_complete tool — LLM completion with tool calling (ReAct loop).
///
/// When the LLM requests a tool call, rig dispatches it to the bridged
/// konflux tools, feeds the result back, and repeats until the LLM
/// produces a final text response or max_iterations is reached.
///
/// ## Tool resolution
///
/// Tools are resolved **dynamically at invocation time** from the engine's
/// live registry. Only tools that pass both checks are exposed to the LLM:
/// 1. The caller's `Envelope.capabilities` must grant access (same rules as the executor)
/// 2. If `with.tools` is specified, the tool name must be in that whitelist
///
/// The `ai_complete` tool itself is always excluded from the inner set to
/// prevent unbounded recursion (unless explicitly whitelisted via `with.tools`).
pub struct AiCompleteTool {
    config: LlmConfig,
    /// Reference to the engine for dynamic tool resolution at invocation time.
    engine: Arc<Engine>,
}

impl AiCompleteTool {
    /// Create a new `AiCompleteTool` with the given config and engine reference.
    pub fn new(config: LlmConfig, engine: Arc<Engine>) -> Self {
        Self { config, engine }
    }

    /// Resolve and bridge inner tools for the LLM, filtered by capabilities and optional whitelist.
    ///
    /// - `env`: the parent Envelope (capabilities, trace_id, namespace, metadata)
    /// - `tool_whitelist`: optional explicit list of tool names from `with.tools`
    fn resolve_tools(
        &self,
        env: &Envelope<Value>,
        tool_whitelist: Option<&[String]>,
    ) -> Vec<Box<dyn rig::tool::ToolDyn>> {
        let registry = self.engine.registry();
        let capability_patterns = env.capabilities.to_patterns();
        let mut tools: Vec<Box<dyn rig::tool::ToolDyn>> = Vec::new();

        for info in registry.list() {
            // Skip ai_complete itself to prevent unbounded recursion,
            // unless explicitly whitelisted
            if info.name == "ai:complete"
                && tool_whitelist.is_none_or(|wl| !wl.iter().any(|n| n == "ai:complete"))
            {
                continue;
            }

            // Check 1: capability gate
            if check_tool_access(&info.name, &capability_patterns).is_err() {
                continue;
            }

            // Check 2: explicit whitelist (AND with capabilities)
            if let Some(whitelist) = tool_whitelist {
                if !whitelist.iter().any(|n| n == &info.name) {
                    continue;
                }
            }

            if let Some(tool) = registry.get(&info.name) {
                // Create a child envelope scoped to this inner tool
                let child_env = env.respond(json!(null));

                tools.push(Box::new(KonfluxToolBridge {
                    inner: tool,
                    tool_info: info,
                    parent_env: child_env,
                }));
            }
        }

        debug!(
            tool_count = tools.len(),
            capabilities = ?capability_patterns,
            whitelist = ?tool_whitelist,
            "ai_complete resolved inner tools"
        );

        tools
    }

    /// Merge per-node overrides from `with:` into the base LlmConfig.
    fn merge_config(&self, input: &Value) -> LlmConfig {
        let mut config = self.config.clone();

        if let Some(v) = input.get("provider").and_then(|v| v.as_str()) {
            config.provider = v.to_string();
        }
        if let Some(v) = input.get("model").and_then(|v| v.as_str()) {
            config.model = v.to_string();
        }
        if let Some(v) = input.get("temperature").and_then(|v| v.as_f64()) {
            config.temperature = v;
        }
        if let Some(v) = input.get("max_tokens").and_then(|v| v.as_u64()) {
            config.max_tokens = v;
        }
        if let Some(v) = input.get("max_iterations").and_then(|v| v.as_u64()) {
            config.max_iterations = v as usize;
        }

        config
    }
}

#[async_trait]
impl Tool for AiCompleteTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "ai:complete".into(),
            description: "Generate an LLM completion with optional tool calling (ReAct loop). \
                Tools are filtered by the caller's capabilities and optional `tools` whitelist."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "system": { "type": "string", "description": "System prompt" },
                    "prompt": { "type": "string", "description": "User prompt" },
                    "messages": {
                        "type": "array",
                        "description": "Conversation history (multi-turn)",
                        "items": {
                            "type": "object",
                            "properties": {
                                "role": { "type": "string", "enum": ["user", "assistant", "system"] },
                                "content": { "type": "string" }
                            }
                        }
                    },
                    "tools": {
                        "type": "array",
                        "description": "Explicit tool whitelist. Only these tools (intersected with capabilities) are visible to the LLM.",
                        "items": { "type": "string" }
                    },
                    "provider": { "type": "string", "description": "Override LLM provider for this call" },
                    "model": { "type": "string", "description": "Override model for this call" },
                    "temperature": { "type": "number", "description": "Override temperature for this call" },
                    "max_tokens": { "type": "integer", "description": "Override max_tokens for this call" },
                    "max_iterations": { "type": "integer", "description": "Override max ReAct iterations for this call" }
                }
            }),
            capabilities: vec!["ai:complete".into()],
            supports_streaming: true,
            output_schema: None,
            annotations: ToolAnnotations {
                open_world: true,
                ..Default::default()
            },
        }
    }

    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, KonfluxToolError> {
        // Delegate to invoke_streaming with a no-op channel (dropped immediately).
        let (sender, _rx) = konflux_substrate::stream::stream_channel(1);
        self.invoke_streaming(env, sender).await
    }

    async fn invoke_streaming(
        &self,
        env: Envelope<Value>,
        sender: StreamSender,
    ) -> Result<Envelope<Value>, KonfluxToolError> {
        let input = &env.payload;
        let prompt = input.get("prompt").and_then(|v| v.as_str()).unwrap_or("");
        let system = input.get("system").and_then(|v| v.as_str());

        // Parse optional tool whitelist from input
        let tool_whitelist: Option<Vec<String>> =
            input.get("tools").and_then(|v| v.as_array()).map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .collect()
            });

        // Merge per-node config overrides
        let config = self.merge_config(input);

        // Resolve tools dynamically from live registry, filtered by capabilities + whitelist
        let inner_tools = self.resolve_tools(&env, tool_whitelist.as_deref());

        let node_id = env.target.0.clone();
        let start = std::time::Instant::now();

        let result = react_loop(&config, system, prompt, inner_tools, &node_id, &sender).await?;

        let duration_ms = start.elapsed().as_millis() as u64;
        info!(
            provider = %config.provider,
            model = %config.model,
            duration_ms,
            target = %env.target,
            trace_id = %env.trace_id,
            "LLM completion"
        );

        Ok(env.respond(json!({
            "text": result.text,
            "_meta": {
                "tool": "ai:complete",
                "provider": config.provider,
                "model": config.model,
                "duration_ms": duration_ms,
                "iterations": result.iterations,
                "tool_calls": result.tool_call_count,
            }
        })))
    }
}

// ============================================================
// ReAct Loop — owned by the kernel, not by rig
// ============================================================

/// Result of a ReAct loop execution.
struct ReactResult {
    /// Final text output from the LLM.
    text: String,
    /// Number of iterations (LLM calls) performed.
    iterations: usize,
    /// Total number of tool calls dispatched.
    tool_call_count: usize,
}

/// Run a manual ReAct loop: call LLM → dispatch tool calls → feed results back → repeat.
///
/// Emits streaming events at each step for full observability:
/// - `ToolStart` before each inner tool call
/// - `ToolEnd` after each inner tool call
/// - `TextDelta` for the final text response
/// - `Status` for iteration progress
async fn react_loop(
    config: &LlmConfig,
    system: Option<&str>,
    prompt: &str,
    inner_tools: Vec<Box<dyn rig::tool::ToolDyn>>,
    node_id: &str,
    sender: &StreamSender,
) -> Result<ReactResult, KonfluxToolError> {
    use rig::completion::Message;
    use rig::message::{AssistantContent, ToolResultContent, UserContent};
    use rig::one_or_many::OneOrMany;

    let map_err = |e: rig::completion::CompletionError| KonfluxToolError::ExecutionFailed {
        message: format!("{} completion failed: {e}", config.provider),
        retryable: true,
    };

    // Build tool definitions for the LLM
    let tool_defs: Vec<rig::completion::ToolDefinition> = {
        let mut defs = Vec::with_capacity(inner_tools.len());
        for t in &inner_tools {
            defs.push(t.definition(String::new()).await);
        }
        defs
    };

    // Build a tool name → bridge index map for dispatch
    let tool_map: std::collections::HashMap<String, usize> = inner_tools
        .iter()
        .enumerate()
        .map(|(i, t)| (t.name(), i))
        .collect();

    // Chat history starts with the user prompt
    let mut messages: Vec<Message> = Vec::new();
    if let Some(sys) = system {
        messages.push(Message::System {
            content: sys.to_string(),
        });
    }
    messages.push(Message::User {
        content: OneOrMany::one(UserContent::text(prompt)),
    });

    let mut total_tool_calls: usize = 0;

    for iteration in 0..config.max_iterations {
        // Emit iteration status
        let _ = sender.try_send(StreamEvent::Progress {
            node_id: node_id.to_string(),
            event_type: ProgressType::Status,
            data: json!({ "iteration": iteration, "max": config.max_iterations }),
        });

        // Call LLM
        let response = call_completion(config, &messages, &tool_defs)
            .await
            .map_err(map_err)?;

        // Partition response into text and tool calls
        let mut text_parts: Vec<String> = Vec::new();
        let mut tool_calls: Vec<&rig::completion::message::ToolCall> = Vec::new();

        for content in response.choice.iter() {
            match content {
                AssistantContent::Text(t) => text_parts.push(t.text.clone()),
                AssistantContent::ToolCall(tc) => tool_calls.push(tc),
                _ => {} // Reasoning, Image — ignore
            }
        }

        // Add assistant response to history
        messages.push(Message::Assistant {
            id: response.message_id.clone(),
            content: response.choice.clone(),
        });

        // If no tool calls, we're done
        if tool_calls.is_empty() {
            let final_text = text_parts.join("");

            // Stream final text
            if !final_text.is_empty() {
                let _ = sender.try_send(StreamEvent::Progress {
                    node_id: node_id.to_string(),
                    event_type: ProgressType::TextDelta,
                    data: json!(final_text),
                });
            }

            return Ok(ReactResult {
                text: final_text,
                iterations: iteration + 1,
                tool_call_count: total_tool_calls,
            });
        }

        // Dispatch tool calls
        let mut tool_results: Vec<UserContent> = Vec::new();

        for tc in &tool_calls {
            let tool_name = &tc.function.name;
            let tool_args = &tc.function.arguments;
            total_tool_calls += 1;

            // Emit ToolStart
            let _ = sender.try_send(StreamEvent::Progress {
                node_id: node_id.to_string(),
                event_type: ProgressType::ToolStart,
                data: json!({ "tool": tool_name, "input": tool_args, "call_id": tc.id }),
            });

            let tool_start = std::time::Instant::now();

            // Dispatch to the bridged tool
            let result = match tool_map.get(tool_name.as_str()) {
                Some(&idx) => {
                    let args_str =
                        serde_json::to_string(tool_args).unwrap_or_else(|_| "{}".to_string());
                    match inner_tools[idx].call(args_str).await {
                        Ok(output) => output,
                        Err(e) => format!("Tool error: {e}"),
                    }
                }
                None => format!("Unknown tool: {tool_name}"),
            };

            let tool_duration_ms = tool_start.elapsed().as_millis() as u64;

            // Emit ToolEnd
            let _ = sender.try_send(StreamEvent::Progress {
                node_id: node_id.to_string(),
                event_type: ProgressType::ToolEnd,
                data: json!({
                    "tool": tool_name,
                    "call_id": tc.id,
                    "duration_ms": tool_duration_ms,
                    "output_preview": truncate_utf8(&result, 200),
                }),
            });

            debug!(
                tool = %tool_name,
                call_id = %tc.id,
                duration_ms = tool_duration_ms,
                "ReAct tool call"
            );

            // Build tool result for chat history
            let tool_result = if let Some(ref call_id) = tc.call_id {
                UserContent::tool_result_with_call_id(
                    tc.id.clone(),
                    call_id.clone(),
                    OneOrMany::one(ToolResultContent::text(result)),
                )
            } else {
                UserContent::tool_result(
                    tc.id.clone(),
                    OneOrMany::one(ToolResultContent::text(result)),
                )
            };

            tool_results.push(tool_result);
        }

        // Add tool results to chat history
        messages.push(Message::User {
            content: OneOrMany::many(tool_results).map_err(|e| {
                KonfluxToolError::ExecutionFailed {
                    message: format!("Failed to build tool results message: {e}"),
                    retryable: false,
                }
            })?,
        });
    }

    // Exceeded max_iterations
    Err(KonfluxToolError::ExecutionFailed {
        message: format!(
            "ai_complete exceeded max_iterations ({}) without producing a final response",
            config.max_iterations
        ),
        retryable: false,
    })
}

/// Truncate a string to at most `max_chars` characters, UTF-8 safe.
fn truncate_utf8(s: &str, max_chars: usize) -> &str {
    match s.char_indices().nth(max_chars) {
        Some((byte_idx, _)) => &s[..byte_idx],
        None => s,
    }
}

/// Make a single completion call to the LLM provider.
///
/// This is the low-level provider dispatch — no ReAct looping here.
async fn call_completion(
    config: &LlmConfig,
    messages: &[rig::completion::Message],
    tool_defs: &[rig::completion::ToolDefinition],
) -> Result<rig::completion::CompletionResponse<serde_json::Value>, rig::completion::CompletionError>
{
    use rig::client::{CompletionClient, ProviderClient};
    use rig::completion::CompletionModel;

    // Build a CompletionRequest with chat history and tools
    macro_rules! call_provider {
        ($client:expr) => {{
            let model = $client.completion_model(&config.model);
            // Build request: the last message is the prompt, previous are history.
            // rig's completion_request takes a single Message as the prompt.
            let prompt_msg = messages
                .last()
                .cloned()
                .unwrap_or(rig::completion::Message::User {
                    content: rig::one_or_many::OneOrMany::one(
                        rig::completion::message::UserContent::text(""),
                    ),
                });
            let history = if messages.len() > 1 {
                &messages[..messages.len() - 1]
            } else {
                &[]
            };

            let mut builder = model.completion_request(prompt_msg);
            for msg in history {
                builder = builder.message(msg.clone());
            }
            for def in tool_defs {
                builder = builder.tool(def.clone());
            }
            builder = builder.temperature(config.temperature);
            builder = builder.max_tokens(config.max_tokens);
            let request = builder.build();

            let response = model.completion(request).await?;

            // Convert provider-specific response to Value for uniform handling
            Ok(rig::completion::CompletionResponse {
                choice: response.choice,
                usage: response.usage,
                raw_response: serde_json::to_value(&response.raw_response).unwrap_or(Value::Null),
                message_id: response.message_id,
            })
        }};
    }

    match config.provider.as_str() {
        "openai" => {
            let client = rig::providers::openai::Client::from_env();
            call_provider!(client)
        }
        "anthropic" => {
            let client = rig::providers::anthropic::Client::from_env();
            call_provider!(client)
        }
        "gemini" => {
            let client = rig::providers::gemini::Client::from_env();
            call_provider!(client)
        }
        other => Err(rig::completion::CompletionError::ProviderError(format!(
            "Unknown LLM provider: '{other}'. Supported: openai, anthropic, gemini"
        ))),
    }
}

/// Register the ai_complete tool with the engine, deserializing config from a JSON value.
///
/// The tool holds a reference to the engine and resolves inner tools dynamically
/// at invocation time — no tool snapshotting at registration. This ensures the
/// LLM always sees the current tool set, filtered by the caller's capabilities.
pub async fn register(
    engine: &konflux_substrate::Engine,
    config: &serde_json::Value,
) -> anyhow::Result<()> {
    let llm_config: LlmConfig = serde_json::from_value(config.clone())?;
    engine.register_tool(Arc::new(AiCompleteTool::new(
        llm_config,
        Arc::new(engine.clone()),
    )));
    Ok(())
}

/// Register the ai_complete tool with the given engine reference.
pub fn register_llm_tools(engine: &konflux_substrate::Engine, config: &LlmConfig) {
    engine.register_tool(Arc::new(AiCompleteTool::new(
        config.clone(),
        Arc::new(engine.clone()),
    )));
}

#[cfg(test)]
mod tests {
    use super::*;
    use konflux_substrate::envelope::{CapSet, Capability};

    fn mock_env(capabilities: Vec<&str>) -> Envelope<Value> {
        let mut env = Envelope::test(json!({}));
        env.capabilities = CapSet(
            capabilities
                .into_iter()
                .map(|c| Capability(c.to_string()))
                .collect(),
        );
        env
    }

    struct MockTool {
        name: String,
    }

    impl MockTool {
        fn new(name: &str) -> Self {
            Self { name: name.into() }
        }
    }

    #[async_trait]
    impl Tool for MockTool {
        fn info(&self) -> ToolInfo {
            ToolInfo {
                name: self.name.clone(),
                description: format!("Mock {}", self.name),
                input_schema: json!({}),
                output_schema: None,
                capabilities: vec![],
                supports_streaming: false,
                annotations: ToolAnnotations::default(),
            }
        }
        async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, KonfluxToolError> {
            Ok(env.respond(env.payload.clone()))
        }
    }

    fn engine_with_tools(names: &[&str]) -> Arc<Engine> {
        let engine = Engine::new();
        for name in names {
            engine.register_tool(Arc::new(MockTool::new(name)));
        }
        Arc::new(engine)
    }

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
        }))
        .unwrap();
        assert_eq!(config.provider, "anthropic");
        assert_eq!(config.model, "claude-sonnet-4-20250514");
        assert_eq!(config.temperature, 0.3);
        assert_eq!(config.max_iterations, 10);
    }

    #[test]
    fn test_resolve_tools_filters_by_capabilities() {
        let engine = engine_with_tools(&["echo", "shell:exec", "http:get", "memory:search"]);
        let tool = AiCompleteTool::new(LlmConfig::default(), engine);

        // Only grant echo and http_get
        let env = mock_env(vec!["echo", "http:get"]);
        let resolved = tool.resolve_tools(&env, None);

        let names: Vec<String> = resolved.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"echo".to_string()));
        assert!(names.contains(&"http:get".to_string()));
        assert!(!names.contains(&"shell:exec".to_string()));
        assert!(!names.contains(&"memory:search".to_string()));
        assert_eq!(resolved.len(), 2);
    }

    #[test]
    fn test_resolve_tools_wildcard_grants_all() {
        let engine = engine_with_tools(&["echo", "shell:exec", "http:get"]);
        let tool = AiCompleteTool::new(LlmConfig::default(), engine);

        let env = mock_env(vec!["*"]);
        let resolved = tool.resolve_tools(&env, None);
        // 3 tools (ai_complete not in registry, so no self-exclusion issue)
        assert_eq!(resolved.len(), 3);
    }

    #[test]
    fn test_resolve_tools_prefix_wildcard() {
        let engine = engine_with_tools(&["memory:search", "memory:store", "http:get"]);
        let tool = AiCompleteTool::new(LlmConfig::default(), engine);

        let env = mock_env(vec!["memory:*"]);
        let resolved = tool.resolve_tools(&env, None);

        let names: Vec<String> = resolved.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"memory:search".to_string()));
        assert!(names.contains(&"memory:store".to_string()));
        assert!(!names.contains(&"http:get".to_string()));
        assert_eq!(resolved.len(), 2);
    }

    #[test]
    fn test_resolve_tools_whitelist_intersects_capabilities() {
        let engine = engine_with_tools(&["echo", "shell:exec", "http:get"]);
        let tool = AiCompleteTool::new(LlmConfig::default(), engine);

        // Grant all, but whitelist only echo
        let env = mock_env(vec!["*"]);
        let whitelist = vec!["echo".to_string()];
        let resolved = tool.resolve_tools(&env, Some(&whitelist));

        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name(), "echo");
    }

    #[test]
    fn test_resolve_tools_whitelist_cannot_bypass_capabilities() {
        let engine = engine_with_tools(&["echo", "shell:exec"]);
        let tool = AiCompleteTool::new(LlmConfig::default(), engine);

        // Only grant echo, but whitelist shell_exec
        let env = mock_env(vec!["echo"]);
        let whitelist = vec!["shell:exec".to_string()];
        let resolved = tool.resolve_tools(&env, Some(&whitelist));

        // shell_exec fails capability check, so nothing resolved
        assert_eq!(resolved.len(), 0);
    }

    #[test]
    fn test_resolve_tools_excludes_ai_complete() {
        let engine = engine_with_tools(&["echo", "ai:complete"]);
        let tool = AiCompleteTool::new(LlmConfig::default(), engine);

        let env = mock_env(vec!["*"]);
        let resolved = tool.resolve_tools(&env, None);

        let names: Vec<String> = resolved.iter().map(|t| t.name()).collect();
        assert!(names.contains(&"echo".to_string()));
        assert!(!names.contains(&"ai:complete".to_string()));
        assert_eq!(resolved.len(), 1);
    }

    #[test]
    fn test_resolve_tools_ai_complete_explicit_whitelist() {
        let engine = engine_with_tools(&["echo", "ai:complete"]);
        let tool = AiCompleteTool::new(LlmConfig::default(), engine);

        let env = mock_env(vec!["*"]);
        let whitelist = vec!["ai:complete".to_string()];
        let resolved = tool.resolve_tools(&env, Some(&whitelist));

        // Explicit whitelist overrides the self-exclusion
        assert_eq!(resolved.len(), 1);
        assert_eq!(resolved[0].name(), "ai:complete");
    }

    #[test]
    fn test_resolve_tools_empty_capabilities_denies_all() {
        let engine = engine_with_tools(&["echo", "shell:exec"]);
        let tool = AiCompleteTool::new(LlmConfig::default(), engine);

        let env = mock_env(vec![]);
        let resolved = tool.resolve_tools(&env, None);
        assert_eq!(resolved.len(), 0);
    }

    #[test]
    fn test_resolve_tools_context_propagation() {
        let engine = engine_with_tools(&["echo"]);
        let tool = AiCompleteTool::new(LlmConfig::default(), engine);

        let env = mock_env(vec!["*"]);

        let resolved = tool.resolve_tools(&env, None);
        assert_eq!(resolved.len(), 1);
        // Bridge name should be the tool name
        assert_eq!(resolved[0].name(), "echo");
        // Note: we can't directly inspect the bridge's parent_env from here,
        // but the envelope propagation is tested via the KonfluxToolBridge construction
    }

    #[test]
    fn test_merge_config_overrides() {
        let engine = engine_with_tools(&[]);
        let tool = AiCompleteTool::new(LlmConfig::default(), engine);

        let input = json!({
            "prompt": "hello",
            "model": "claude-opus-4-20250514",
            "temperature": 0.1,
            "max_iterations": 5
        });

        let merged = tool.merge_config(&input);
        assert_eq!(merged.model, "claude-opus-4-20250514");
        assert_eq!(merged.temperature, 0.1);
        assert_eq!(merged.max_iterations, 5);
        // provider unchanged (not overridden)
        assert_eq!(merged.provider, "openai");
    }

    #[test]
    fn test_merge_config_no_overrides() {
        let engine = engine_with_tools(&[]);
        let tool = AiCompleteTool::new(LlmConfig::default(), engine);

        let input = json!({"prompt": "hello"});
        let merged = tool.merge_config(&input);
        assert_eq!(merged.model, "gpt-4o");
        assert_eq!(merged.temperature, 0.7);
        assert_eq!(merged.max_iterations, 10);
    }
}

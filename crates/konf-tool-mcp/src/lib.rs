#![warn(missing_docs)]
//! MCP client manager — connects to MCP servers and registers their tools.
//!
//! Uses rmcp for stdio transport. MCP servers are spawned as child processes
//! and their tools are discovered and wrapped as konflux::Tool implementations.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{info, warn};

use konflux::error::ToolError;
use konflux::tool::{Tool, ToolAnnotations, ToolContext, ToolInfo};

use rmcp::ServiceExt;
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};

/// Configuration for an MCP server from tools.yaml.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct McpServerConfig {
    /// Human-readable name used as a namespace prefix for tool names.
    pub name: String,
    /// Transport protocol. Only `"stdio"` is currently supported.
    #[serde(default = "default_transport")]
    pub transport: String,
    /// Command to spawn the MCP server process.
    pub command: String,
    /// Arguments passed to the server command.
    #[serde(default)]
    pub args: Vec<String>,
    /// Environment variables injected into the server process.
    /// Values wrapped in `${VAR}` are resolved from the host environment.
    #[serde(default)]
    pub env: std::collections::HashMap<String, String>,
    /// Glob patterns that filter which discovered tools are registered.
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Seconds of inactivity before the server may be recycled (reserved for future use).
    #[serde(default = "default_idle_timeout")]
    pub idle_timeout: u64,
}

fn default_transport() -> String { "stdio".into() }
fn default_idle_timeout() -> u64 { 600 }

/// Manages MCP server processes and their tool registrations.
/// Holds client handles to keep child processes alive for the server's lifetime.
pub struct McpManager {
    configs: Vec<McpServerConfig>,
    #[allow(dead_code)] // Clients kept alive by ownership, dropped on shutdown
    clients: std::sync::Mutex<Vec<rmcp::service::RunningService<rmcp::service::RoleClient, ()>>>,
}

impl McpManager {
    /// Create a new manager from the given server configurations.
    pub fn new(configs: Vec<McpServerConfig>) -> Self {
        Self {
            configs,
            clients: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// Discover tools from all configured MCP servers and register them.
    pub async fn discover_and_register(&self, engine: &konflux::Engine) -> anyhow::Result<()> {
        for config in &self.configs {
            match self.connect_and_register(config, engine).await {
                Ok(count) => {
                    info!(server = %config.name, tools = count, "MCP server tools registered");
                }
                Err(e) => {
                    warn!(server = %config.name, error = %e, "Failed to connect to MCP server, skipping");
                }
            }
        }
        Ok(())
    }

    async fn connect_and_register<'a>(
        &'a self,
        config: &McpServerConfig,
        engine: &konflux::Engine,
    ) -> anyhow::Result<usize> {
        if config.transport != "stdio" {
            anyhow::bail!("MCP server '{}': only 'stdio' transport is supported, got '{}'", config.name, config.transport);
        }

        // idle_timeout is tracked for future process GC (not enforced in v1)
        let _idle_timeout = config.idle_timeout;

        let transport = TokioChildProcess::new(
            tokio::process::Command::new(&config.command).configure(|cmd| {
                for arg in &config.args {
                    cmd.arg(arg);
                }
                for (k, v) in &config.env {
                    cmd.env(k, resolve_env_var(v));
                }
            }),
        )?;

        let client = ().serve(transport).await?;
        let tools = client.list_all_tools().await?;
        let mut registered = 0;

        for tool in &tools {
            let full_name = format!("{}:{}", config.name, tool.name);

            // Check capability filter
            if !config.capabilities.is_empty()
                && !config.capabilities.iter().any(|cap| {
                    konf_runtime::scope::CapabilityGrant::new(cap)
                        .matches(&full_name)
                        .is_some()
                })
            {
                continue;
            }

            // Map MCP annotations to Konf ToolAnnotations
            // readOnlyHint -> read_only, destructiveHint -> destructive, etc.
            let annotations = tool.annotations.as_ref().map(|ann| {
                ToolAnnotations {
                    read_only: ann.read_only_hint.unwrap_or(false),
                    destructive: ann.destructive_hint.unwrap_or(false),
                    idempotent: ann.idempotent_hint.unwrap_or(false),
                    open_world: ann.open_world_hint.unwrap_or(true),
                }
            }).unwrap_or_default();

            let wrapper = McpToolWrapper {
                name: full_name.clone(),
                description: tool.description.as_ref().map(|d| d.to_string()).unwrap_or_default(),
                input_schema: Value::Object(tool.input_schema.as_ref().clone()),
                annotations,
                server_name: config.name.clone(),
                tool_name: tool.name.to_string(),
                client: client.peer().clone(),
            };

            engine.register_tool(Arc::new(wrapper));
            registered += 1;
        }

        // Store client handle so the child process stays alive until McpManager is dropped
        self.clients.lock().unwrap_or_else(|p| p.into_inner()).push(client);

        Ok(registered)
    }
}

/// Deserialize MCP server configs from a JSON value and register all discovered tools.
///
/// Expects `config` to contain an `"mcp_servers"` key with an array of
/// [`McpServerConfig`] objects.
pub async fn register(engine: &konflux::Engine, config: &serde_json::Value) -> anyhow::Result<()> {
    let servers: Vec<McpServerConfig> = serde_json::from_value(
        config
            .get("mcp_servers")
            .cloned()
            .unwrap_or(Value::Array(Vec::new())),
    )?;

    if servers.is_empty() {
        return Ok(());
    }

    let manager = McpManager::new(servers);
    manager.discover_and_register(engine).await?;

    // Leak the manager so client handles (and child processes) live for the program's lifetime.
    // In production this is owned by the application's top-level state instead.
    std::mem::forget(manager);

    Ok(())
}

fn resolve_env_var(value: &str) -> String {
    if let Some(var_name) = value.strip_prefix("${").and_then(|s| s.strip_suffix('}')) {
        match std::env::var(var_name) {
            Ok(v) => v,
            Err(_) => {
                tracing::error!(var = var_name, "MCP env var not set — MCP server may fail to authenticate. Set the variable or remove it from config.");
                String::new()
            }
        }
    } else {
        value.to_string()
    }
}

/// Wraps an MCP tool as a [`konflux::Tool`].
struct McpToolWrapper {
    name: String,
    description: String,
    input_schema: Value,
    annotations: ToolAnnotations,
    server_name: String,
    tool_name: String,
    client: rmcp::service::Peer<rmcp::service::RoleClient>,
}

#[async_trait]
impl Tool for McpToolWrapper {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: self.name.clone(),
            description: self.description.clone(),
            input_schema: self.input_schema.clone(),
            output_schema: None,
            capabilities: vec![self.name.clone()],
            supports_streaming: false,
            annotations: self.annotations.clone(),
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let start = std::time::Instant::now();

        let mut params = rmcp::model::CallToolRequestParams::new(self.tool_name.clone());
        if let Some(obj) = input.as_object() {
            if !obj.is_empty() {
                params = params.with_arguments(obj.clone());
            }
        }

        let result = self.client
            .call_tool(params)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("MCP tool '{}' failed: {e}", self.name),
                retryable: true,
            })?;

        let duration_ms = start.elapsed().as_millis() as u64;

        let content: Vec<Value> = result.content
            .iter()
            .map(|c| serde_json::to_value(c).unwrap_or(json!({"raw": format!("{c:?}")})))
            .collect();

        Ok(json!({
            "content": content,
            "is_error": result.is_error.unwrap_or(false),
            "_meta": {
                "tool": self.name,
                "server": self.server_name,
                "duration_ms": duration_ms,
            }
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_resolve_env_var_plain() {
        assert_eq!(resolve_env_var("plain_value"), "plain_value");
    }

    #[test]
    fn test_resolve_env_var_missing() {
        assert_eq!(resolve_env_var("${NONEXISTENT_VAR_XYZ}"), "");
    }

    #[test]
    fn test_resolve_env_var_syntax() {
        // Verify the ${VAR} extraction logic without mutating env
        assert_eq!(resolve_env_var("no_braces"), "no_braces");
        assert_eq!(resolve_env_var("${NONEXISTENT_TEST_VAR_12345}"), "");
    }

    #[test]
    fn test_mcp_config_deserialization() {
        let config: McpServerConfig = serde_json::from_value(json!({
            "name": "brave",
            "command": "npx",
            "args": ["-y", "@anthropic/mcp-server-brave"],
            "env": { "BRAVE_API_KEY": "${BRAVE_API_KEY}" },
            "capabilities": ["search:*"]
        })).unwrap();

        assert_eq!(config.name, "brave");
        assert_eq!(config.transport, "stdio");
        assert_eq!(config.idle_timeout, 600);
        assert_eq!(config.capabilities, vec!["search:*"]);
    }
}

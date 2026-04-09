//! Shell execution tool (`shell_exec`) for the Konf platform.
//!
//! Runs commands inside a Docker sandbox container via `docker exec`.
//! All commands execute as the `konf-agent` user in the `/workspace` directory.
#![warn(missing_docs)]

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::info;

use konflux::error::ToolError;
use konflux::tool::{Tool, ToolAnnotations, ToolContext, ToolInfo};
use konflux::Engine;

/// Configuration for the shell tool, deserialized from the engine config.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct ShellConfig {
    /// Docker container name to execute commands in.
    pub container: String,
    /// Default timeout in milliseconds for command execution.
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
}

fn default_timeout_ms() -> u64 {
    30_000
}

/// Register the `shell_exec` tool in the engine.
pub async fn register(engine: &Engine, config: &Value) -> anyhow::Result<()> {
    let shell_config: ShellConfig = serde_json::from_value(config.clone())?;
    let tool = ShellExecTool::new(shell_config.container, shell_config.timeout_ms);
    engine.register_tool(Arc::new(tool));
    Ok(())
}

/// Tool that executes shell commands inside a sandboxed Docker container.
pub struct ShellExecTool {
    container_name: String,
    default_timeout_ms: u64,
}

impl ShellExecTool {
    /// Create a new `ShellExecTool`.
    ///
    /// - `container_name`: the Docker container to run commands in.
    /// - `default_timeout_ms`: default timeout for command execution in milliseconds.
    pub fn new(container_name: impl Into<String>, default_timeout_ms: u64) -> Self {
        Self {
            container_name: container_name.into(),
            default_timeout_ms,
        }
    }
}

#[async_trait]
impl Tool for ShellExecTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "shell_exec".into(),
            description: "Execute a shell command inside the sandboxed container. \
                Returns stdout, stderr, and exit code."
                .into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "command": {
                        "type": "string",
                        "description": "Shell command to execute"
                    },
                    "timeout_ms": {
                        "type": "integer",
                        "description": "Timeout in milliseconds (overrides default)"
                    }
                },
                "required": ["command"]
            }),
            output_schema: None,
            capabilities: vec!["shell_exec".into()],
            supports_streaming: false,
            annotations: ToolAnnotations {
                destructive: true,
                ..Default::default()
            },
        }
    }

    async fn invoke(&self, input: Value, _ctx: &ToolContext) -> Result<Value, ToolError> {
        let command = input
            .get("command")
            .and_then(|v| v.as_str())
            .ok_or_else(|| ToolError::InvalidInput {
                message: "missing 'command'".into(),
                field: Some("command".into()),
            })?;

        let timeout_ms = input
            .get("timeout_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or(self.default_timeout_ms);

        info!(
            container = %self.container_name,
            command = %command,
            timeout_ms,
            "shell_exec invoked"
        );

        // When container is "host", run directly on the host (no Docker).
        // This is used by trusted products like devkit that need host access.
        // Untrusted products use a Docker container for isolation.
        let child = if self.container_name == "host" {
            tokio::process::Command::new("bash")
                .args(["-c", command])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
        } else {
            tokio::process::Command::new("docker")
                .args([
                    "exec",
                    "-u",
                    "konf-agent",
                    "-w",
                    "/workspace",
                    &self.container_name,
                    "bash",
                    "-c",
                    command,
                ])
                .stdout(std::process::Stdio::piped())
                .stderr(std::process::Stdio::piped())
                .spawn()
        }
        .map_err(|e| ToolError::ExecutionFailed {
            message: format!("failed to spawn shell command: {e}"),
            retryable: false,
        })?;

        let timeout_duration = std::time::Duration::from_millis(timeout_ms);
        let output = tokio::time::timeout(timeout_duration, child.wait_with_output())
            .await
            .map_err(|_| ToolError::ExecutionFailed {
                message: format!(
                    "command timed out after {timeout_ms}ms: {command}"
                ),
                retryable: false,
            })?
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("failed to wait for docker exec: {e}"),
                retryable: false,
            })?;

        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
        let exit_code = output.status.code().unwrap_or(-1);

        info!(
            container = %self.container_name,
            exit_code,
            stdout_len = stdout.len(),
            stderr_len = stderr.len(),
            "shell_exec completed"
        );

        Ok(json!({
            "stdout": stdout,
            "stderr": stderr,
            "exit_code": exit_code,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_shell_config_defaults() {
        let config: ShellConfig =
            serde_json::from_value(json!({ "container": "my-sandbox" }))
                .expect("should deserialize with defaults");
        assert_eq!(config.container, "my-sandbox");
        assert_eq!(config.timeout_ms, 30_000);
    }

    #[test]
    fn test_shell_tool_info() {
        let tool = ShellExecTool::new("test-container", 5000);
        let info = tool.info();

        assert_eq!(info.name, "shell_exec");
        assert_eq!(info.capabilities, vec!["shell_exec"]);
        assert!(info.annotations.destructive);
        assert!(!info.annotations.read_only);
        assert!(!info.annotations.idempotent);
        assert!(!info.annotations.open_world);
    }

    #[test]
    fn test_shell_exec_timeout_zero() {
        let tool = ShellExecTool::new("test-container", 0);
        let info = tool.info();

        let schema = &info.input_schema;
        assert_eq!(schema["type"], "object");
        assert!(schema["properties"]["command"]["type"] == "string");
        assert!(schema["properties"]["timeout_ms"]["type"] == "integer");
        let required = schema["required"]
            .as_array()
            .expect("required should be an array");
        assert!(required.contains(&json!("command")));
    }
}

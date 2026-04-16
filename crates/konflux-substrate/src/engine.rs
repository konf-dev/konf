//! Engine — the public API for running workflows.

use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio_util::sync::CancellationToken;
use tracing::{info_span, Instrument};

use crate::capability;
use crate::error::KonfluxError;
use crate::executor::Executor;
use crate::hooks::{EventRecorder, NoopRecorder};
use crate::prompt::{Prompt, PromptRegistry};
use crate::resource::{Resource, ResourceRegistry};
use crate::stream::{stream_channel, StreamEvent, StreamReceiver};
use crate::tool::{Tool, ToolRegistry};
use crate::workflow::Workflow;

/// Receiver for tool-list-changed notifications.
pub type ToolChangedReceiver = tokio::sync::watch::Receiver<u64>;

/// Configuration for the engine. All values are configurable — no hardcoded defaults
/// in the executor or other modules. Change these to tune behavior.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(default)]
pub struct EngineConfig {
    /// Maximum number of steps before aborting (prevents infinite loops).
    pub max_steps: usize,
    /// Default timeout per tool invocation in milliseconds (0 = no timeout).
    pub default_timeout_ms: u64,
    /// Maximum total workflow execution time in milliseconds (0 = no limit).
    pub max_workflow_timeout_ms: u64,
    /// Stream channel buffer size (Progress events dropped if full).
    pub stream_buffer: usize,
    /// Internal channel size for node completion signals.
    pub finished_channel_size: usize,
    /// Default retry backoff delay in milliseconds (when no RetryPolicy is specified).
    pub default_retry_backoff_ms: u64,
    /// Maximum YAML size in bytes (prevents DoS from huge workflow definitions).
    pub max_yaml_size: usize,
    /// Maximum concurrent nodes executing in parallel (caps JoinSet size).
    pub max_concurrent_nodes: usize,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            max_steps: 1000,
            default_timeout_ms: 30_000,
            max_workflow_timeout_ms: 300_000, // 5 minutes
            stream_buffer: 256,
            finished_channel_size: 100,
            default_retry_backoff_ms: 250,
            max_yaml_size: 10 * 1024 * 1024, // 10 MB
            max_concurrent_nodes: 50,
        }
    }
}

impl EngineConfig {
    /// Validate that all config values are sane.
    pub fn validate(&self) -> Result<(), String> {
        if self.max_steps == 0 {
            return Err("max_steps must be > 0".into());
        }
        if self.default_timeout_ms == 0 {
            return Err("default_timeout_ms must be > 0".into());
        }
        if self.stream_buffer == 0 {
            return Err("stream_buffer must be > 0".into());
        }
        if self.finished_channel_size == 0 {
            return Err("finished_channel_size must be > 0".into());
        }
        if self.max_yaml_size == 0 {
            return Err("max_yaml_size must be > 0".into());
        }
        if self.max_concurrent_nodes == 0 {
            return Err("max_concurrent_nodes must be > 0".into());
        }
        // max_workflow_timeout_ms == 0 means no limit (intentional)
        Ok(())
    }
}

/// The workflow execution engine.
///
/// Manages three registries (Tools, Resources, Prompts) and executes
/// YAML-defined workflows as DAGs of tool calls.
#[derive(Clone)]
pub struct Engine {
    tools: Arc<std::sync::RwLock<ToolRegistry>>,
    resources: Arc<std::sync::RwLock<ResourceRegistry>>,
    prompts: Arc<std::sync::RwLock<PromptRegistry>>,
    config: EngineConfig,
    /// Monotonic counter incremented when the tool list changes.
    /// Listeners (e.g., MCP server) watch this to send notifications.
    tool_changed_tx: Arc<tokio::sync::watch::Sender<u64>>,
    tool_changed_rx: ToolChangedReceiver,
}

impl Engine {
    pub fn new() -> Self {
        let (tx, rx) = tokio::sync::watch::channel(0u64);
        Self {
            tools: Arc::new(std::sync::RwLock::new(ToolRegistry::new())),
            resources: Arc::new(std::sync::RwLock::new(ResourceRegistry::new())),
            prompts: Arc::new(std::sync::RwLock::new(PromptRegistry::new())),
            config: EngineConfig::default(),
            tool_changed_tx: Arc::new(tx),
            tool_changed_rx: rx,
        }
    }

    pub fn with_config(config: EngineConfig) -> Self {
        let (tx, rx) = tokio::sync::watch::channel(0u64);
        Self {
            tools: Arc::new(std::sync::RwLock::new(ToolRegistry::new())),
            resources: Arc::new(std::sync::RwLock::new(ResourceRegistry::new())),
            prompts: Arc::new(std::sync::RwLock::new(PromptRegistry::new())),
            config,
            tool_changed_tx: Arc::new(tx),
            tool_changed_rx: rx,
        }
    }

    /// Get the engine configuration.
    pub fn config(&self) -> &EngineConfig {
        &self.config
    }

    /// Signal that the tool list has changed. Listeners (e.g., MCP server)
    /// will be notified to send `notifications/tools/list_changed`.
    pub fn notify_tools_changed(&self) {
        let prev = *self.tool_changed_tx.borrow();
        // send is infallible when there is at least one receiver (we hold one)
        let _ = self.tool_changed_tx.send(prev.wrapping_add(1));
    }

    /// Subscribe to tool-list-changed notifications.
    pub fn subscribe_tool_changes(&self) -> ToolChangedReceiver {
        self.tool_changed_rx.clone()
    }

    // ---- Tool registry ----

    /// Register a tool. Thread-safe — can be called from any thread.
    /// If a tool with the same name already exists, it is replaced.
    pub fn register_tool(&self, tool: Arc<dyn Tool>) {
        self.tools
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .register(tool);
    }

    /// Remove a tool by name. Returns true if the tool was present.
    /// Used by config_reload to remove stale workflow tools before re-registering.
    pub fn remove_tool(&self, name: &str) -> bool {
        self.tools
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .remove(name)
    }

    /// Snapshot the registry for executor use (cheap Arc clone of inner data).
    fn registry_snapshot(&self) -> Arc<ToolRegistry> {
        Arc::new(self.tools.read().unwrap_or_else(|p| p.into_inner()).clone())
    }

    // ---- Resource registry ----

    /// Register a resource. Thread-safe.
    pub fn register_resource(&self, resource: Arc<dyn Resource>) {
        self.resources
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .register(resource);
    }

    /// Get a snapshot of the resource registry.
    pub fn resources(&self) -> ResourceRegistry {
        self.resources
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    // ---- Prompt registry ----

    /// Register a prompt. Thread-safe.
    pub fn register_prompt(&self, prompt: Arc<dyn Prompt>) {
        self.prompts
            .write()
            .unwrap_or_else(|p| p.into_inner())
            .register(prompt);
    }

    /// Get a snapshot of the prompt registry.
    pub fn prompts(&self) -> PromptRegistry {
        self.prompts
            .read()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    /// Run a workflow to completion, returning the final output.
    ///
    /// - `cancel_token`: if provided, the workflow can be cancelled. If `None`, uses a default.
    /// - `hooks`: if provided, lifecycle callbacks are fired during execution.
    pub async fn run(
        &self,
        workflow: &Workflow,
        input: Value,
        granted_capabilities: &[String],
        execution_metadata: HashMap<String, Value>,
        cancel_token: Option<CancellationToken>,
        hooks: Option<Arc<dyn EventRecorder>>,
    ) -> Result<Value, KonfluxError> {
        let span = info_span!("workflow.run", workflow_id = %workflow.id);

        let fut = async {
            capability::validate_grant(&workflow.capabilities, granted_capabilities)
                .map_err(KonfluxError::CapabilityDenied)?;

            let token = cancel_token.unwrap_or_default();
            let hooks: Arc<dyn EventRecorder> = hooks.unwrap_or_else(|| Arc::new(NoopRecorder));
            let (tx, mut rx) = stream_channel(self.config.stream_buffer);
            let registry = self.registry_snapshot();
            let executor = Executor::new(
                &registry,
                granted_capabilities,
                &self.config,
                execution_metadata,
                token,
                hooks,
            );

            let workflow = workflow.clone();
            let wf_id = workflow.id.to_string();

            let exec_handle =
                tokio::spawn(async move { executor.execute(&workflow, input, tx).await });

            let mut final_output = Value::Null;
            let mut error = None;

            while let Some(event) = rx.recv().await {
                match event {
                    StreamEvent::Done { output } => {
                        final_output = output;
                    }
                    StreamEvent::Error { message, .. } => {
                        error = Some(message);
                    }
                    _ => {}
                }
            }

            let exec_res = exec_handle.await.map_err(|e| {
                KonfluxError::Execution(crate::error::ExecutionError::JoinFailed {
                    workflow_id: wf_id.clone(),
                    node: "engine".into(),
                    message: e.to_string(),
                })
            })?;

            exec_res?;

            if let Some(msg) = error {
                return Err(KonfluxError::Execution(
                    crate::error::ExecutionError::NodeFailed {
                        workflow_id: wf_id,
                        node: "stream".into(),
                        message: msg,
                    },
                ));
            }

            Ok(final_output)
        };

        // Apply global workflow timeout if configured
        if self.config.max_workflow_timeout_ms > 0 {
            match tokio::time::timeout(
                Duration::from_millis(self.config.max_workflow_timeout_ms),
                fut.instrument(span),
            )
            .await
            {
                Ok(result) => result,
                Err(_) => Err(KonfluxError::Execution(
                    crate::error::ExecutionError::Timeout {
                        workflow_id: workflow.id.to_string(),
                        node: "workflow".into(),
                        timeout_ms: self.config.max_workflow_timeout_ms,
                    },
                )),
            }
        } else {
            fut.instrument(span).await
        }
    }

    /// Run a workflow with streaming, returning the stream receiver.
    pub async fn run_streaming(
        &self,
        workflow: &Workflow,
        input: Value,
        granted_capabilities: &[String],
        execution_metadata: HashMap<String, Value>,
        cancel_token: Option<CancellationToken>,
        hooks: Option<Arc<dyn EventRecorder>>,
    ) -> Result<StreamReceiver, KonfluxError> {
        let span = info_span!("workflow.run_streaming", workflow_id = %workflow.id);

        async {
            capability::validate_grant(&workflow.capabilities, granted_capabilities)
                .map_err(KonfluxError::CapabilityDenied)?;

            let token = cancel_token.unwrap_or_default();
            let hooks: Arc<dyn EventRecorder> = hooks.unwrap_or_else(|| Arc::new(NoopRecorder));
            let (tx, rx) = stream_channel(self.config.stream_buffer);
            let registry = self.registry_snapshot();
            let executor = Executor::new(
                &registry,
                granted_capabilities,
                &self.config,
                execution_metadata,
                token,
                hooks,
            );

            let workflow = workflow.clone();
            tokio::spawn(async move {
                if let Err(e) = executor.execute(&workflow, input, tx.clone()).await {
                    // Error event must be delivered
                    let _ = tx
                        .send(StreamEvent::Error {
                            code: "execution_error".into(),
                            message: e.to_string(),
                            retryable: false,
                        })
                        .await;
                }
            });

            Ok(rx)
        }
        .instrument(span)
        .await
    }

    /// Parse a YAML string into a Workflow. Enforces max_yaml_size.
    pub fn parse_yaml(&self, yaml: &str) -> Result<Workflow, KonfluxError> {
        if yaml.len() > self.config.max_yaml_size {
            return Err(KonfluxError::Parse(crate::error::ParseError::InvalidYaml {
                message: format!(
                    "YAML size ({} bytes) exceeds maximum ({} bytes)",
                    yaml.len(),
                    self.config.max_yaml_size
                ),
            }));
        }
        crate::parser::parse(yaml)
    }

    /// Get a snapshot of the tool registry (for inspection).
    pub fn registry(&self) -> ToolRegistry {
        self.tools.read().unwrap_or_else(|p| p.into_inner()).clone()
    }
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::prompt::{Message, Prompt, PromptArgument, PromptError, PromptInfo};
    use crate::resource::{Resource, ResourceError, ResourceInfo};
    use async_trait::async_trait;

    struct TestResource;
    #[async_trait]
    impl Resource for TestResource {
        fn info(&self) -> ResourceInfo {
            ResourceInfo {
                uri: "konf://test/config".into(),
                name: "Test Config".into(),
                description: "test".into(),
                mime_type: "application/json".into(),
            }
        }
        async fn read(&self) -> Result<serde_json::Value, ResourceError> {
            Ok(serde_json::json!({"test": true}))
        }
    }

    struct TestPrompt;
    #[async_trait]
    impl Prompt for TestPrompt {
        fn info(&self) -> PromptInfo {
            PromptInfo {
                name: "test_prompt".into(),
                description: "test".into(),
                arguments: vec![PromptArgument {
                    name: "input".into(),
                    description: "test input".into(),
                    required: true,
                }],
            }
        }
        async fn expand(&self, _args: serde_json::Value) -> Result<Vec<Message>, PromptError> {
            Ok(vec![Message {
                role: "user".into(),
                content: serde_json::Value::String("test".into()),
            }])
        }
    }

    #[test]
    fn test_engine_three_registries() {
        let engine = Engine::new();

        // All start empty
        assert!(engine.registry().is_empty());
        assert!(engine.resources().is_empty());
        assert!(engine.prompts().is_empty());

        // Register resource
        engine.register_resource(Arc::new(TestResource));
        assert_eq!(engine.resources().len(), 1);
        assert!(engine.resources().get("konf://test/config").is_some());

        // Register prompt
        engine.register_prompt(Arc::new(TestPrompt));
        assert_eq!(engine.prompts().len(), 1);
        assert!(engine.prompts().get("test_prompt").is_some());

        // Tools still empty
        assert!(engine.registry().is_empty());
    }
}

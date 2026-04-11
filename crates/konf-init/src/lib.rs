//! konf-init — the init system for the Konf platform.
//!
//! Like Linux's systemd, konf-init reads configuration and boots the platform.
//! It creates the engine, registers tools from konf-tools crates, wires the
//! runtime, and returns a ready-to-use [`KonfInstance`].
//!
//! Both konf-backend (HTTP) and konf-mcp (MCP) call [`boot()`] to get a
//! `KonfInstance`, then serve their respective protocols over it.
#![warn(missing_docs)]

pub mod config;
mod schedule;

use std::path::Path;
use std::sync::Arc;

use arc_swap::ArcSwap;
use tracing::info;

use konf_runtime::Runtime;
use konflux::engine::Engine;

pub use config::{
    AuthConfig, PlatformConfig, ProductConfig, RoleConfig, ServerConfig, ShellConfig,
    ToolGuardConfig, ToolsConfig,
};

/// A fully booted Konf instance, ready for transport shells to serve.
pub struct KonfInstance {
    /// The runtime with process management and optional journal.
    /// Access the engine via `runtime.engine()` — this is the canonical
    /// tool registry containing all tools including workflow tools.
    pub runtime: Arc<Runtime>,

    /// The loaded and validated platform configuration
    pub config: Arc<PlatformConfig>,

    /// Product config (tools.yaml, workflows, prompts) — hot-reloadable via ArcSwap
    pub product_config: Arc<ArcSwap<ProductConfig>>,

    /// Database pool (if configured). Shared with runtime journal and available
    /// for scheduler or other server-only components. None on edge deployments.
    #[cfg(feature = "postgres")]
    pub pool: Option<sqlx::PgPool>,
}

/// Boot the Konf platform from a config directory.
///
/// This is the single entry point for all deployment scenarios.
/// The 12-step boot sequence:
/// 1. Load platform config (konf.toml + env vars)
/// 2. Validate platform config
/// 3. Load product config (tools.yaml)
/// 4. Create engine with empty registries
/// 5. Register builtin tools
/// 6. Register tools from tools.yaml
/// 7. Register workflows as tools
/// 8. Register resources and prompts
/// 9. Connect to database (if configured)
/// 10. Create runtime (engine + optional journal)
/// 11. Return KonfInstance
pub async fn boot(config_dir: &Path) -> anyhow::Result<KonfInstance> {
    // 1. Load platform config
    let config = PlatformConfig::load(config_dir)
        .map_err(|e| anyhow::anyhow!("Failed to load platform config: {e}"))?;

    // 2. Validate
    if let Err(errors) = config.validate() {
        for err in &errors {
            tracing::error!("Config validation error: {err}");
        }
        anyhow::bail!("Config validation failed with {} errors", errors.len());
    }

    info!(
        host = %config.server.host,
        port = config.server.port,
        config_dir = %config_dir.display(),
        "Booting Konf"
    );

    // 3. Load product config
    let tools_yaml_path = config_dir.join("tools.yaml");
    let product_config = if tools_yaml_path.exists() {
        let contents = std::fs::read_to_string(&tools_yaml_path)
            .map_err(|e| anyhow::anyhow!("Failed to read tools.yaml: {e}"))?;
        let interpolated = interpolate_env_vars(&contents);
        serde_yaml::from_str::<ProductConfig>(&interpolated)
            .map_err(|e| anyhow::anyhow!("Failed to parse tools.yaml: {e}"))?
    } else {
        ProductConfig::default()
    };

    // 4. Create engine
    let engine = Engine::with_config(config.engine.clone());

    // 5. Register builtins
    konflux::builtin::register_builtins(&engine);

    // 6. Register tools from config
    register_tools(&engine, &product_config.tools).await?;

    // 6b. Register architect tools (shell, introspect, validate)
    if let Some(ref shell_config) = product_config.tools.shell {
        let shell_tool =
            konf_tool_shell::ShellExecTool::new(&shell_config.container, shell_config.timeout_ms);
        engine.register_tool(Arc::new(shell_tool));
        info!(container = %shell_config.container, "Shell tool registered");
    }

    // system_introspect — always available (read-only metadata)
    engine.register_tool(Arc::new(konf_tool_llm::IntrospectTool::new(Arc::new(
        engine.clone(),
    ))));

    // yaml_validate_workflow — always available
    engine.register_tool(Arc::new(konf_tool_llm::ValidateWorkflowTool::new(
        Arc::new(engine.clone()),
    )));

    if let Some(ref secret_config) = product_config.tools.secret {
        konf_tool_secret::register(&engine, secret_config);
        info!(allowed_keys = ?secret_config.allowed_keys, "Secret tools registered");
    }

    let tool_count = engine.registry().len();
    info!(tool_count, "Tools registered");

    // 7. Register config files as resources
    register_resources(&engine, config_dir);

    // 8. Connect to database (if configured)
    #[cfg(feature = "postgres")]
    let pool = match &config.database {
        Some(db) => {
            let pool = sqlx::PgPool::connect(&db.url)
                .await
                .map_err(|e| anyhow::anyhow!("Failed to connect to database: {e}"))?;
            info!("Connected to database");
            Some(pool)
        }
        _ => None,
    };
    #[cfg(not(feature = "postgres"))]
    let pool: Option<()> = None;

    if pool.is_none() {
        info!("No database configured — journal disabled, scheduling unavailable");
    }

    // 9. Create runtime
    #[cfg(feature = "postgres")]
    let runtime_pool = pool.clone();
    #[cfg(not(feature = "postgres"))]
    let runtime_pool = None;

    let runtime = Arc::new(
        Runtime::new(engine.clone(), runtime_pool)
            .await
            .map_err(|e| anyhow::anyhow!("Failed to initialize runtime: {e}"))?,
    );
    info!("Runtime initialized");

    // 9b. Register config_reload tool into the runtime's engine.
    // ConfigReloadTool must operate on the same engine the runtime uses
    // so that reloaded workflow tools are visible in child scopes.
    runtime
        .engine()
        .register_tool(Arc::new(ConfigReloadTool::new(
            runtime.clone(),
            config_dir.to_path_buf(),
        )));

    // 9c. Register schedule + cancel_schedule tools (timer primitives for autonomous agents).
    runtime
        .engine()
        .register_tool(Arc::new(schedule::ScheduleTool::new(runtime.clone())));
    runtime
        .engine()
        .register_tool(Arc::new(schedule::CancelScheduleTool));

    // 9d. Register runner tools (runner:spawn/status/wait/cancel). The inline
    // backend runs workflows as tokio tasks against the same runtime; future
    // systemd/docker backends will plug in here without changing this call.
    // Finding 014 principle: a new tool family, not a kernel change.
    let runner_registry = konf_tool_runner::RunRegistry::new();
    let inline_runner: std::sync::Arc<dyn konf_tool_runner::Runner> = std::sync::Arc::new(
        konf_tool_runner::InlineRunner::new(runtime.clone(), runner_registry),
    );
    konf_tool_runner::register(runtime.engine(), inline_runner)?;

    // 10. Register workflows as tools (needs runtime for WorkflowTool)
    // IMPORTANT: Register into the runtime's engine, not the original clone.
    // The runtime owns its own Engine instance (cloned at step 9). WorkflowTool
    // runs via runtime.run() which copies tools from runtime.engine — so workflow
    // tools MUST be in the runtime's engine to be available in child scopes.
    let workflows_dir = config_dir.join("workflows");
    if workflows_dir.is_dir() {
        register_workflows(runtime.engine(), &runtime, &workflows_dir)?;
    }

    // 10b. Apply tool guards from product config
    apply_tool_guards(&runtime, &product_config);

    let final_tool_count = runtime.engine().registry().len();
    info!(
        tools = final_tool_count,
        resources = engine.resources().len(),
        "Registration complete"
    );

    // 11. Return instance
    let config = Arc::new(config);
    let product_config = Arc::new(ArcSwap::from_pointee(product_config));

    // Note: The `engine` field is dropped — all tools (including workflow tools
    // and config_reload) are registered into the runtime's engine. Consumers
    // should use `instance.runtime.engine()` for the canonical tool registry.
    // The original `engine` was only used during boot for steps 1-8 registration.
    drop(engine);

    Ok(KonfInstance {
        runtime,
        config,
        product_config,
        #[cfg(feature = "postgres")]
        pool,
    })
}

/// Replace `${VAR}` and `${VAR:-default}` patterns with environment variable values.
///
/// - `${VAR}` is replaced with the value of env var `VAR`, or empty string if unset.
/// - `${VAR:-default}` is replaced with the value of env var `VAR`, or `"default"` if unset.
///
/// This function is infallible: missing variables produce their default or empty string.
fn interpolate_env_vars(input: &str) -> String {
    let mut result = String::with_capacity(input.len());
    let mut chars = input.chars().peekable();

    while let Some(ch) = chars.next() {
        if ch == '$' && chars.peek() == Some(&'{') {
            chars.next(); // consume '{'
            let mut inner = String::new();
            let mut found_close = false;
            for c in chars.by_ref() {
                if c == '}' {
                    found_close = true;
                    break;
                }
                inner.push(c);
            }
            if !found_close {
                // Unclosed brace — emit literally
                result.push('$');
                result.push('{');
                result.push_str(&inner);
                continue;
            }
            let (var_name, default_val) = match inner.find(":-") {
                Some(pos) => (&inner[..pos], Some(&inner[pos + 2..])),
                None => (inner.as_str(), None),
            };
            match std::env::var(var_name) {
                Ok(val) => result.push_str(&val),
                Err(_) => {
                    if let Some(def) = default_val {
                        result.push_str(def);
                    }
                }
            }
        } else {
            result.push(ch);
        }
    }

    result
}

/// Register tools based on product config.
async fn register_tools(engine: &Engine, tools: &ToolsConfig) -> anyhow::Result<()> {
    // Memory backend
    // NOTE: Memory backend implementations (smrti, SurrealDB, SQLite) are external
    // dependencies, not part of this monorepo. To add a backend, depend on its crate
    // (e.g., konf-tool-memory-smrti from konf-dev/smrti) and add a match arm here.
    if let Some(ref mem_config) = tools.memory {
        match mem_config.backend.as_str() {
            #[cfg(feature = "memory-smrti")]
            "smrti" => {
                let backend = konf_tool_memory_smrti::connect(&mem_config.config).await?;
                konf_tool_memory::register(engine, backend).await?;
                info!("Memory backend: smrti (Postgres + pgvector)");
            }
            other => {
                #[allow(unused_mut)]
                let mut available: Vec<&str> = Vec::new();
                #[cfg(feature = "memory-smrti")]
                available.push("smrti");
                if available.is_empty() {
                    anyhow::bail!(
                        "Unknown memory backend: '{other}'. \
                         No memory backends are compiled in. \
                         Add a backend crate (e.g., konf-tool-memory-smrti) as a dependency."
                    );
                } else {
                    anyhow::bail!(
                        "Unknown memory backend: '{other}'. Available: {}",
                        available.join(", ")
                    );
                }
            }
        }
    }

    // HTTP tools
    if let Some(ref http_config) = tools.http {
        konf_tool_http::register(engine, http_config).await?;
    } else {
        // HTTP tools enabled by default
        konf_tool_http::register(engine, &serde_json::json!({})).await?;
    }

    // LLM tools
    if let Some(ref llm_config) = tools.llm {
        konf_tool_llm::register(engine, llm_config).await?;
    }

    // Embed tools
    if let Some(ref embed_config) = tools.embed {
        konf_tool_embed::register(engine, embed_config).await?;
    }

    // MCP servers
    if let Some(ref mcp_config) = tools.mcp_servers {
        konf_tool_mcp::register(engine, mcp_config).await?;
    }

    Ok(())
}

/// Register config files as readable Resources in the engine.
fn register_resources(engine: &Engine, config_dir: &std::path::Path) {
    // Register tools.yaml as a resource
    let tools_path = config_dir.join("tools.yaml");
    if tools_path.exists() {
        engine.register_resource(Arc::new(FileResource {
            uri: "konf://config/tools.yaml".into(),
            name: "Tools Configuration".into(),
            description: "Product tool and backend configuration".into(),
            mime_type: "application/yaml".into(),
            path: tools_path,
        }));
    }

    // Register konf.toml as a resource
    let toml_path = config_dir.join("konf.toml");
    if toml_path.exists() {
        engine.register_resource(Arc::new(FileResource {
            uri: "konf://config/konf.toml".into(),
            name: "Platform Configuration".into(),
            description: "Platform server, auth, and engine settings".into(),
            mime_type: "application/toml".into(),
            path: toml_path,
        }));
    }
}

/// A resource backed by a file on disk. Reads the file content on each access.
struct FileResource {
    uri: String,
    name: String,
    description: String,
    mime_type: String,
    path: std::path::PathBuf,
}

#[async_trait::async_trait]
impl konflux::Resource for FileResource {
    fn info(&self) -> konflux::ResourceInfo {
        konflux::ResourceInfo {
            uri: self.uri.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            mime_type: self.mime_type.clone(),
        }
    }

    async fn read(&self) -> Result<serde_json::Value, konflux::ResourceError> {
        let content = std::fs::read_to_string(&self.path).map_err(|e| {
            konflux::ResourceError::ReadFailed(format!("{}: {e}", self.path.display()))
        })?;
        Ok(serde_json::Value::String(content))
    }
}

// ============================================================
// config_reload Tool
// ============================================================

/// Tool that triggers a hot-reload of product configuration from disk.
/// Re-scans the workflows directory, re-parses all YAML files, and
/// re-registers workflow tools in the engine.
pub struct ConfigReloadTool {
    runtime: Arc<Runtime>,
    config_dir: std::path::PathBuf,
}

impl ConfigReloadTool {
    /// Create a new config_reload tool.
    ///
    /// Uses the runtime's engine for tool registration so that reloaded
    /// workflow tools are visible in child scopes during nested execution.
    pub fn new(runtime: Arc<Runtime>, config_dir: std::path::PathBuf) -> Self {
        Self {
            runtime,
            config_dir,
        }
    }
}

#[async_trait::async_trait]
impl konflux::tool::Tool for ConfigReloadTool {
    fn info(&self) -> konflux::tool::ToolInfo {
        konflux::tool::ToolInfo {
            name: "config:reload".into(),
            description: "Reload product configuration (workflows, prompts, tools) from disk. Re-parses all workflow YAML files and re-registers them as tools.".into(),
            input_schema: serde_json::json!({
                "type": "object",
                "properties": {}
            }),
            capabilities: vec!["config:reload".into()],
            supports_streaming: false,
            output_schema: None,
            annotations: konflux::tool::ToolAnnotations::default(),
        }
    }

    async fn invoke(
        &self,
        _input: serde_json::Value,
        _ctx: &konflux::tool::ToolContext,
    ) -> Result<serde_json::Value, konflux::error::ToolError> {
        let workflows_dir = self.config_dir.join("workflows");

        if !workflows_dir.is_dir() {
            return Ok(serde_json::json!({
                "reloaded": true,
                "workflows_loaded": 0,
                "message": "No workflows directory found"
            }));
        }

        // Remove existing workflow_* tools before re-registering
        let existing_tools = self.runtime.engine().registry().list();
        let workflow_tool_names: Vec<String> = existing_tools
            .iter()
            .filter(|t| t.name.starts_with("workflow:"))
            .map(|t| t.name.clone())
            .collect();

        for name in &workflow_tool_names {
            self.runtime.engine().remove_tool(name);
        }

        // Re-register workflows from disk
        match register_workflows(self.runtime.engine(), &self.runtime, &workflows_dir) {
            Ok(()) => {
                let new_workflow_count = self
                    .runtime
                    .engine()
                    .registry()
                    .list()
                    .iter()
                    .filter(|t| t.name.starts_with("workflow:"))
                    .count();

                let total_tools = self.runtime.engine().registry().len();

                // Reload tool guards from tools.yaml
                let tools_yaml_path = self.config_dir.join("tools.yaml");
                if tools_yaml_path.exists() {
                    match std::fs::read_to_string(&tools_yaml_path) {
                        Ok(contents) => {
                            let interpolated = interpolate_env_vars(&contents);
                            match serde_yaml::from_str::<ProductConfig>(&interpolated) {
                                Ok(product_config) => {
                                    apply_tool_guards(&self.runtime, &product_config);
                                }
                                Err(e) => {
                                    tracing::warn!(
                                        error = %e,
                                        "Failed to parse tools.yaml for guards — keeping existing guards"
                                    );
                                }
                            }
                        }
                        Err(e) => {
                            tracing::warn!(
                                error = %e,
                                "Failed to read tools.yaml for guards — keeping existing guards"
                            );
                        }
                    }
                }

                // Signal MCP server (and any other listeners) that tools changed
                self.runtime.engine().notify_tools_changed();

                info!(
                    config_dir = %self.config_dir.display(),
                    removed = workflow_tool_names.len(),
                    registered = new_workflow_count,
                    total_tools,
                    "Config reloaded"
                );

                Ok(serde_json::json!({
                    "reloaded": true,
                    "workflows_removed": workflow_tool_names.len(),
                    "workflows_registered": new_workflow_count,
                    "total_tools": total_tools,
                }))
            }
            Err(e) => Err(konflux::error::ToolError::ExecutionFailed {
                message: format!("Config reload failed: {e}"),
                retryable: true,
            }),
        }
    }
}

/// Scan workflows/ directory and register eligible workflows as tools.
/// Convert product config tool guards to runtime format and apply them.
fn apply_tool_guards(runtime: &Arc<Runtime>, product_config: &ProductConfig) {
    use konf_runtime::runtime::ToolGuardEntry;

    if product_config.tool_guards.is_empty() {
        return;
    }

    let guards: std::collections::HashMap<String, ToolGuardEntry> = product_config
        .tool_guards
        .iter()
        .map(|(name, cfg)| {
            (
                name.clone(),
                ToolGuardEntry {
                    rules: cfg.rules.clone(),
                    default_action: cfg.default,
                    alias: cfg.alias.clone(),
                },
            )
        })
        .collect();

    let count = guards.len();
    runtime.set_tool_guards(guards);
    info!(
        guard_count = count,
        "Tool guards applied from product config"
    );
}

fn register_workflows(
    engine: &Engine,
    runtime: &Arc<Runtime>,
    workflows_dir: &std::path::Path,
) -> anyhow::Result<()> {
    let entries = std::fs::read_dir(workflows_dir)
        .map_err(|e| anyhow::anyhow!("Failed to read workflows directory: {e}"))?;

    for entry in entries {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|e| e.to_str()) != Some("yaml")
            && path.extension().and_then(|e| e.to_str()) != Some("yml")
        {
            continue;
        }

        let yaml = std::fs::read_to_string(&path)
            .map_err(|e| anyhow::anyhow!("Failed to read {}: {e}", path.display()))?;

        let workflow = match engine.parse_yaml(&yaml) {
            Ok(w) => w,
            Err(e) => {
                tracing::warn!(file = %path.display(), error = %e, "Failed to parse workflow, skipping");
                continue;
            }
        };

        // Register as a resource (browseable)
        let file_name = path.file_name().unwrap_or_default().to_string_lossy();
        engine.register_resource(Arc::new(FileResource {
            uri: format!("konf://workflows/{file_name}"),
            name: format!("Workflow: {}", workflow.id),
            description: workflow.description.clone().unwrap_or_default(),
            mime_type: "application/yaml".into(),
            path: path.clone(),
        }));

        // Register as a tool if flagged
        if workflow.register_as_tool {
            let scope = konf_runtime::scope::ExecutionScope {
                namespace: "konf:system".into(),
                capabilities: workflow
                    .capabilities
                    .iter()
                    .map(|c| konf_runtime::scope::CapabilityGrant::new(c.as_str()))
                    .collect(),
                limits: konf_runtime::scope::ResourceLimits::default(),
                actor: konf_runtime::scope::Actor {
                    id: "system".into(),
                    role: konf_runtime::scope::ActorRole::System,
                },
                depth: 0,
            };

            let tool = konf_runtime::WorkflowTool::new(workflow.clone(), runtime.clone(), scope);

            engine.register_tool(Arc::new(tool));
            info!(workflow = %workflow.id, "Registered workflow as tool: workflow:{}", workflow.id);
        }
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use konflux::Resource;

    #[test]
    fn test_default_boot_config() {
        let config = PlatformConfig::default();
        assert!(config.validate().is_ok());
        assert!(config.database.is_none());
        assert_eq!(config.server.port, 8000);
    }

    #[tokio::test]
    async fn test_register_tools_empty_config() {
        let engine = Engine::new();
        let tools = ToolsConfig::default();
        register_tools(&engine, &tools).await.unwrap();
        // HTTP tools registered by default
        assert!(engine.registry().contains("http:get"));
        assert!(engine.registry().contains("http:post"));
    }

    #[test]
    fn test_register_resources_with_temp_dir() {
        let dir = tempfile::tempdir().unwrap();

        // Write a tools.yaml
        std::fs::write(dir.path().join("tools.yaml"), "memory:\n  backend: smrti").unwrap();

        let engine = Engine::new();
        register_resources(&engine, dir.path());

        let resources = engine.resources();
        assert_eq!(resources.len(), 1);
        assert!(resources.get("konf://config/tools.yaml").is_some());
    }

    #[test]
    fn test_register_resources_skips_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        // Empty dir — no tools.yaml, no konf.toml

        let engine = Engine::new();
        register_resources(&engine, dir.path());
        assert_eq!(engine.resources().len(), 0);
    }

    #[tokio::test]
    async fn test_file_resource_reads_content() {
        let dir = tempfile::tempdir().unwrap();
        let file_path = dir.path().join("test.yaml");
        std::fs::write(&file_path, "key: value\n").unwrap();

        let resource = FileResource {
            uri: "konf://test".into(),
            name: "Test".into(),
            description: "test".into(),
            mime_type: "application/yaml".into(),
            path: file_path,
        };

        let content = resource.read().await.unwrap();
        assert_eq!(content.as_str().unwrap(), "key: value\n");
    }

    #[test]
    fn test_register_workflows_with_temp_dir() {
        let dir = tempfile::tempdir().unwrap();
        let workflows_dir = dir.path().join("workflows");
        std::fs::create_dir(&workflows_dir).unwrap();

        // Write a simple workflow
        std::fs::write(
            workflows_dir.join("echo.yaml"),
            r#"
workflow: echo_test
description: "Test workflow"
register_as_tool: true
capabilities: []
nodes:
  step1:
    do: echo
    return: true
"#,
        )
        .unwrap();

        // Write a non-tool workflow
        std::fs::write(
            workflows_dir.join("helper.yaml"),
            r#"
workflow: helper
nodes:
  step1:
    do: echo
    return: true
"#,
        )
        .unwrap();

        let engine = Engine::new();
        konflux::builtin::register_builtins(&engine);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let runtime =
            rt.block_on(async { Arc::new(Runtime::new(Engine::new(), None).await.unwrap()) });

        register_workflows(&engine, &runtime, &workflows_dir).unwrap();

        // echo_test should be registered as a tool
        assert!(
            engine.registry().contains("workflow:echo_test"),
            "Expected workflow:echo_test in registry, got: {:?}",
            engine
                .registry()
                .list()
                .iter()
                .map(|t| &t.name)
                .collect::<Vec<_>>()
        );

        // helper should NOT be registered as a tool (no register_as_tool)
        assert!(!engine.registry().contains("workflow:helper"));

        // Both should be registered as resources
        assert!(engine
            .resources()
            .get("konf://workflows/echo.yaml")
            .is_some());
        assert!(engine
            .resources()
            .get("konf://workflows/helper.yaml")
            .is_some());
    }

    #[test]
    fn test_register_workflows_skips_invalid_yaml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(
            dir.path().join("bad.yaml"),
            "not: valid: workflow_ yaml: {{{{",
        )
        .unwrap();

        let engine = Engine::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let runtime =
            rt.block_on(async { Arc::new(Runtime::new(Engine::new(), None).await.unwrap()) });

        // Should not panic, should skip the bad file
        register_workflows(&engine, &runtime, dir.path()).unwrap();
        // No tools registered from bad file
        assert!(engine.registry().is_empty());
    }

    #[test]
    fn test_interpolate_existing_var() {
        std::env::set_var("KONF_TEST_INTERP_A", "hello");
        let result = interpolate_env_vars("model: ${KONF_TEST_INTERP_A}");
        assert_eq!(result, "model: hello");
        std::env::remove_var("KONF_TEST_INTERP_A");
    }

    #[test]
    fn test_interpolate_missing_var_becomes_empty() {
        std::env::remove_var("KONF_TEST_INTERP_MISSING");
        let result = interpolate_env_vars("model: ${KONF_TEST_INTERP_MISSING}");
        assert_eq!(result, "model: ");
    }

    #[test]
    fn test_interpolate_missing_var_uses_default() {
        std::env::remove_var("KONF_TEST_INTERP_MISS2");
        let result = interpolate_env_vars("model: ${KONF_TEST_INTERP_MISS2:-qwen3:8b}");
        assert_eq!(result, "model: qwen3:8b");
    }

    #[test]
    fn test_interpolate_existing_var_ignores_default() {
        std::env::set_var("KONF_TEST_INTERP_B", "custom");
        let result = interpolate_env_vars("model: ${KONF_TEST_INTERP_B:-fallback}");
        assert_eq!(result, "model: custom");
        std::env::remove_var("KONF_TEST_INTERP_B");
    }

    #[test]
    fn test_interpolate_no_patterns_unchanged() {
        let input = "plain: value\nno_vars: here";
        assert_eq!(interpolate_env_vars(input), input);
    }

    #[test]
    fn test_interpolate_multiple_vars() {
        std::env::set_var("KONF_TEST_INTERP_C", "aaa");
        std::env::remove_var("KONF_TEST_INTERP_D");
        let result = interpolate_env_vars("${KONF_TEST_INTERP_C} and ${KONF_TEST_INTERP_D:-bbb}");
        assert_eq!(result, "aaa and bbb");
        std::env::remove_var("KONF_TEST_INTERP_C");
    }

    #[test]
    fn test_interpolate_unclosed_brace_literal() {
        let input = "model: ${UNCLOSED";
        assert_eq!(interpolate_env_vars(input), input);
    }

    #[test]
    fn test_register_workflows_ignores_non_yaml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("readme.md"), "# Not a workflow").unwrap();
        std::fs::write(dir.path().join("data.json"), "{}").unwrap();

        let engine = Engine::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let runtime =
            rt.block_on(async { Arc::new(Runtime::new(Engine::new(), None).await.unwrap()) });

        register_workflows(&engine, &runtime, dir.path()).unwrap();
        assert!(engine.registry().is_empty());
        assert!(engine.resources().is_empty());
    }
}

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

use std::path::Path;
use std::sync::Arc;

use arc_swap::ArcSwap;
use tracing::info;

use konflux::engine::Engine;
use konf_runtime::Runtime;

pub use config::{PlatformConfig, ProductConfig, ToolsConfig, AuthConfig, ServerConfig};

/// A fully booted Konf instance, ready for transport shells to serve.
pub struct KonfInstance {
    /// The engine with all tools, resources, and prompts registered
    pub engine: Arc<Engine>,

    /// The runtime with process management and optional journal
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
        serde_yaml::from_str::<ProductConfig>(&contents)
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

    let tool_count = engine.registry().len();
    info!(tool_count, "Tools registered");

    // 7. Register config files as resources
    register_resources(&engine, config_dir);

    // 8. Connect to database (if configured)
    #[cfg(feature = "postgres")]
    let pool = match &config.database {
        Some(db) => {
            let pool = sqlx::PgPool::connect(&db.url).await
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
        Runtime::new(engine.clone(), runtime_pool).await
            .map_err(|e| anyhow::anyhow!("Failed to initialize runtime: {e}"))?
    );
    info!("Runtime initialized");

    // 10. Register workflows as tools (needs runtime for WorkflowTool)
    let workflows_dir = config_dir.join("workflows");
    if workflows_dir.is_dir() {
        register_workflows(&engine, &runtime, &workflows_dir)?;
    }

    let final_tool_count = engine.registry().len();
    info!(tools = final_tool_count, resources = engine.resources().len(), "Registration complete");

    // 11. Return instance
    let config = Arc::new(config);
    let product_config = Arc::new(ArcSwap::from_pointee(product_config));

    Ok(KonfInstance {
        engine: Arc::new(engine),
        runtime,
        config,
        product_config,
        #[cfg(feature = "postgres")]
        pool,
    })
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
                        "Unknown memory backend: '{other}'. Available: {}", available.join(", ")
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
        let content = std::fs::read_to_string(&self.path)
            .map_err(|e| konflux::ResourceError::ReadFailed(format!("{}: {e}", self.path.display())))?;
        Ok(serde_json::Value::String(content))
    }
}

/// Scan workflows/ directory and register eligible workflows as tools.
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
                capabilities: workflow.capabilities.iter()
                    .map(|c| konf_runtime::scope::CapabilityGrant::new(c.as_str()))
                    .collect(),
                limits: konf_runtime::scope::ResourceLimits::default(),
                actor: konf_runtime::scope::Actor {
                    id: "system".into(),
                    role: konf_runtime::scope::ActorRole::System,
                },
                depth: 0,
            };

            let tool = konf_runtime::WorkflowTool::new(
                workflow.clone(),
                runtime.clone(),
                scope,
            );

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
        std::fs::write(workflows_dir.join("echo.yaml"), r#"
workflow: echo_test
description: "Test workflow"
register_as_tool: true
capabilities: []
nodes:
  step1:
    do: echo
    return: true
"#).unwrap();

        // Write a non-tool workflow
        std::fs::write(workflows_dir.join("helper.yaml"), r#"
workflow: helper
nodes:
  step1:
    do: echo
    return: true
"#).unwrap();

        let engine = Engine::new();
        konflux::builtin::register_builtins(&engine);

        let rt = tokio::runtime::Runtime::new().unwrap();
        let runtime = rt.block_on(async {
            Arc::new(Runtime::new(Engine::new(), None).await.unwrap())
        });

        register_workflows(&engine, &runtime, &workflows_dir).unwrap();

        // echo_test should be registered as a tool
        assert!(engine.registry().contains("workflow:echo_test"),
            "Expected workflow:echo_test in registry, got: {:?}",
            engine.registry().list().iter().map(|t| &t.name).collect::<Vec<_>>());

        // helper should NOT be registered as a tool (no register_as_tool)
        assert!(!engine.registry().contains("workflow:helper"));

        // Both should be registered as resources
        assert!(engine.resources().get("konf://workflows/echo.yaml").is_some());
        assert!(engine.resources().get("konf://workflows/helper.yaml").is_some());
    }

    #[test]
    fn test_register_workflows_skips_invalid_yaml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("bad.yaml"), "not: valid: workflow: yaml: {{{{").unwrap();

        let engine = Engine::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let runtime = rt.block_on(async {
            Arc::new(Runtime::new(Engine::new(), None).await.unwrap())
        });

        // Should not panic, should skip the bad file
        register_workflows(&engine, &runtime, dir.path()).unwrap();
        // No tools registered from bad file
        assert!(engine.registry().is_empty());
    }

    #[test]
    fn test_register_workflows_ignores_non_yaml() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("readme.md"), "# Not a workflow").unwrap();
        std::fs::write(dir.path().join("data.json"), "{}").unwrap();

        let engine = Engine::new();
        let rt = tokio::runtime::Runtime::new().unwrap();
        let runtime = rt.block_on(async {
            Arc::new(Runtime::new(Engine::new(), None).await.unwrap())
        });

        register_workflows(&engine, &runtime, dir.path()).unwrap();
        assert!(engine.registry().is_empty());
        assert!(engine.resources().is_empty());
    }
}

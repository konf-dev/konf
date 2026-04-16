//! konf-mcp — MCP server for the Konf platform.
//!
//! Exposes the engine's tools, resources, and prompts to MCP clients
//! (Claude Desktop, Cursor, other Konf instances). Supports stdio transport.
//!
//! Implements [`rmcp::handler::server::ServerHandler`] to translate between
//! Konf's internal registries and the MCP wire protocol (rmcp 1.3.0).

use std::sync::Arc;

use rmcp::handler::server::ServerHandler;
use rmcp::model::{AnnotateAble, ErrorData};
use rmcp::model::{
    CallToolRequestParams, CallToolResult, Content, Implementation, ListResourcesResult,
    ListToolsResult, PaginatedRequestParams, RawResource, ReadResourceRequestParams,
    ReadResourceResult, ResourceContents, ServerCapabilities, Tool as McpTool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::ServiceExt;

/// Re-exports of the rmcp items used by `konf-backend` to mount the
/// Streamable HTTP endpoint. Keeping them here means downstream crates
/// don't need a direct rmcp dependency.
pub mod http {
    pub use rmcp::transport::streamable_http_server::{
        session::local::LocalSessionManager, StreamableHttpService,
    };
}

use konf_runtime::scope::{Actor, ActorRole, CapabilityGrant, ExecutionScope, ResourceLimits};
use konf_runtime::Runtime;
use konflux_substrate::tool::ToolInfo;
use konflux_substrate::Engine;

/// MCP server info type alias (rmcp names it InitializeResult).
type ServerInfo = rmcp::model::InitializeResult;

/// The Konf MCP server. Translates engine registries to MCP wire protocol.
pub struct KonfMcpServer {
    engine: Arc<Engine>,
    runtime: Arc<Runtime>,
    /// Capability patterns for this MCP session. Controls which tools are visible
    /// and what capabilities tool calls receive. Default: `["*"]` (dev mode).
    session_capabilities: Vec<String>,
}

impl KonfMcpServer {
    /// Create a new MCP server backed by the given engine and runtime.
    /// Defaults to full access (dev mode). Use `with_capabilities` for scoped sessions.
    pub fn new(engine: Arc<Engine>, runtime: Arc<Runtime>) -> Self {
        Self {
            engine,
            runtime,
            session_capabilities: vec!["*".into()],
        }
    }

    /// Create a scoped MCP server with specific capability patterns.
    /// Tools not matching the patterns are hidden from `list_tools` and
    /// denied on `call_tool`.
    pub fn with_capabilities(
        engine: Arc<Engine>,
        runtime: Arc<Runtime>,
        capabilities: Vec<String>,
    ) -> Self {
        Self {
            engine,
            runtime,
            session_capabilities: capabilities,
        }
    }

    /// Serve MCP over stdio (for CLI / Claude Desktop integration).
    pub async fn serve_stdio(self) -> anyhow::Result<()> {
        tracing::info!("Starting MCP server on stdio");
        let mut tool_changed_rx = self.engine.subscribe_tool_changes();
        let transport = rmcp::transport::stdio();
        let service = self
            .serve(transport)
            .await
            .map_err(|e| anyhow::anyhow!("MCP stdio server failed: {e}"))?;

        // Spawn a task that watches for tool-list changes and notifies the MCP client.
        let peer = service.peer().clone();
        tokio::spawn(async move {
            while tool_changed_rx.changed().await.is_ok() {
                tracing::info!("Tool list changed, sending notifications/tools/list_changed");
                if let Err(e) = peer.notify_tool_list_changed().await {
                    tracing::warn!(error = %e, "Failed to send tool_list_changed notification");
                }
            }
        });

        service
            .waiting()
            .await
            .map_err(|e| anyhow::anyhow!("MCP stdio server error: {e}"))?;
        Ok(())
    }

    /// Get a reference to the engine.
    pub fn engine(&self) -> &Engine {
        &self.engine
    }

    /// Get a reference to the runtime.
    pub fn runtime(&self) -> &Runtime {
        &self.runtime
    }

    /// Build the execution scope used when routing tool calls through
    /// [`Runtime::invoke_tool`]. The scope's capability grants mirror the
    /// session's capability patterns; in dev mode that's `["*"]`, in a
    /// future multi-tenant deployment a custom `SessionManager` will
    /// build narrower scopes per authenticated user.
    ///
    /// The namespace is a stable `konf:mcp:http` prefix which means any
    /// tool that honors `VirtualizedTool`'s namespace binding (currently
    /// just the memory tools) will automatically scope its reads and
    /// writes to that namespace. This is enough to keep MCP-originated
    /// memory operations from leaking into other tenants' namespaces
    /// even in the loose dev-mode configuration.
    fn mcp_session_scope(&self) -> ExecutionScope {
        let mut bindings = std::collections::HashMap::new();
        bindings.insert(
            "namespace".to_string(),
            serde_json::Value::String("konf:mcp:http".to_string()),
        );
        let capabilities = self
            .session_capabilities
            .iter()
            .map(|pattern| CapabilityGrant::with_bindings(pattern.clone(), bindings.clone()))
            .collect();
        ExecutionScope {
            namespace: "konf:mcp:http".into(),
            capabilities,
            limits: ResourceLimits::default(),
            actor: Actor {
                id: "mcp-http".into(),
                role: ActorRole::System,
            },
            depth: 0,
        }
    }
}

/// Convert a kernel tool name to MCP-safe name.
///
/// The MCP spec (SEP-986) restricts tool names to `[A-Za-z0-9_\-.]`.
/// The kernel uses colons for namespacing (`memory:search`, `ai:complete`).
/// MCP clients see underscores (`memory_search`, `ai_complete`).
/// Translation happens only at this boundary.
fn kernel_to_mcp_name(name: &str) -> String {
    name.replace(':', "_")
}

/// Convert a Konf ToolInfo to an MCP Tool definition.
fn tool_info_to_mcp(info: &ToolInfo) -> McpTool {
    let schema_obj = info.input_schema.as_object().cloned().unwrap_or_default();
    let mcp_name = kernel_to_mcp_name(&info.name);

    // NOTE: Do NOT add .with_annotations() here.
    // Claude Code silently drops ALL tools when annotations are present
    // in the tools/list response (anthropics/claude-code#25081).
    McpTool::new(mcp_name, info.description.clone(), Arc::new(schema_obj))
}

/// Check if a tool name matches session capability patterns.
/// Delegates to the engine's authoritative capability matching to avoid divergence.
fn tool_allowed_by_session(tool_name: &str, capabilities: &[String]) -> bool {
    konflux_substrate::capability::check_tool_access(tool_name, capabilities).is_ok()
}

impl ServerHandler for KonfMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_tool_list_changed()
                .enable_resources()
                .build(),
        )
        .with_server_info(Implementation::new("konf", env!("CARGO_PKG_VERSION")))
        .with_instructions(
            "Konf AI agent platform. Provides tools for memory, LLM, HTTP, and workflow execution.",
        )
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let registry = self.engine.registry();
        let tools: Vec<McpTool> = registry
            .list()
            .iter()
            .filter(|info| tool_allowed_by_session(&info.name, &self.session_capabilities))
            .map(tool_info_to_mcp)
            .collect();

        tracing::debug!(count = tools.len(), "MCP tools/list");

        Ok(ListToolsResult {
            tools,
            next_cursor: None,
            meta: None,
        })
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        let mcp_name = &request.name;
        tracing::info!(tool = %mcp_name, "MCP tools/call");

        // Resolve MCP name → kernel name. MCP clients send underscored names
        // (e.g., "memory_search"), kernel uses colons ("memory:search").
        // Build reverse map from kernel registry to find the match.
        let registry = self.engine.registry();
        let kernel_name = registry
            .list()
            .iter()
            .find(|info| kernel_to_mcp_name(&info.name) == *mcp_name)
            .map(|info| info.name.clone())
            .unwrap_or_else(|| mcp_name.to_string()); // fallback to exact match

        let input: serde_json::Value = match request.arguments {
            Some(args) => serde_json::Value::Object(args),
            None => serde_json::Value::Object(serde_json::Map::new()),
        };

        // Session-level visibility filter: hide tools the session isn't
        // allowed to see. This is a cheap early reject; the runtime will
        // enforce the same check again inside `invoke_tool`.
        if !tool_allowed_by_session(&kernel_name, &self.session_capabilities) {
            return Err(ErrorData::invalid_params(
                format!("Tool not permitted in this session: {mcp_name}"),
                None,
            ));
        }

        // Route through the runtime so the call picks up:
        //   - capability checks against the session's scope
        //   - VirtualizedTool namespace binding injection
        //   - GuardedTool deny/allow rules from tools.yaml::tool_guards
        //
        // In dev mode the session caps are ["*"] so the capability check
        // is a no-op, but the guard enforcement and namespace bindings
        // still apply. A future multi-tenant auth layer will swap the
        // scope for a per-session narrower one via SessionManager.
        let scope = self.mcp_session_scope();
        // R2: construct an ExecutionContext at the MCP transport boundary.
        // Each MCP tool call is a fresh root trace. Future multi-turn MCP
        // work might thread trace_id through an MCP session header.
        let exec_ctx = konf_runtime::ExecutionContext::new_root("konf:mcp:http");
        match self
            .runtime
            .invoke_tool(&kernel_name, input, &scope, &exec_ctx)
            .await
        {
            Ok(output) => {
                let text =
                    serde_json::to_string_pretty(&output).unwrap_or_else(|_| output.to_string());
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            Err(e) => {
                tracing::warn!(tool = %kernel_name, error = %e, "MCP tool call failed");
                Ok(CallToolResult::error(vec![Content::text(e.to_string())]))
            }
        }
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        let registry = self.engine.resources();
        let resources = registry
            .list()
            .iter()
            .map(|info| {
                RawResource::new(info.uri.clone(), info.name.clone())
                    .with_description(info.description.clone())
                    .with_mime_type(info.mime_type.clone())
                    .no_annotation()
            })
            .collect();

        tracing::debug!("MCP resources/list");

        Ok(ListResourcesResult {
            resources,
            next_cursor: None,
            meta: None,
        })
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        let uri = &request.uri;
        tracing::info!(uri = %uri, "MCP resources/read");

        let registry = self.engine.resources();
        let resource = registry
            .get(uri)
            .ok_or_else(|| ErrorData::invalid_params(format!("Resource not found: {uri}"), None))?;

        let content = resource.read().await.map_err(|e| {
            ErrorData::internal_error(format!("Failed to read resource: {e}"), None)
        })?;

        let text = serde_json::to_string_pretty(&content).unwrap_or_else(|_| content.to_string());

        Ok(ReadResourceResult::new(vec![ResourceContents::text(
            text,
            uri.clone(),
        )]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_tool_info_to_mcp_no_annotations() {
        // Annotations are intentionally NOT included in MCP responses
        // because Claude Code silently drops all tools when annotations
        // are present (anthropics/claude-code#25081).
        let info = ToolInfo {
            name: "test_tool".into(),
            description: "A test tool".into(),
            input_schema: serde_json::json!({"type": "object", "properties": {}}),
            output_schema: None,
            capabilities: vec!["test_tool".into()],
            supports_streaming: false,
            annotations: konflux_substrate::tool::ToolAnnotations {
                read_only: true,
                destructive: false,
                idempotent: true,
                open_world: false,
            },
        };

        let mcp_tool = tool_info_to_mcp(&info);
        assert_eq!(mcp_tool.name.as_ref(), "test_tool");
        assert!(mcp_tool.annotations.is_none());
    }

    #[test]
    fn test_server_info() {
        let engine = Arc::new(Engine::new());
        let rt = tokio::runtime::Runtime::new().unwrap();
        let runtime =
            rt.block_on(async { Arc::new(Runtime::new(Engine::new(), None).await.unwrap()) });
        let server = KonfMcpServer::new(engine, runtime);
        let info = server.get_info();
        assert_eq!(info.server_info.name, "konf");
    }
}

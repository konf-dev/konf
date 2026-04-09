//! konf-mcp — MCP server for the Konf platform.
//!
//! Exposes the engine's tools, resources, and prompts to MCP clients
//! (Claude Desktop, Cursor, other Konf instances). Supports stdio transport.
//!
//! Implements [`rmcp::handler::server::ServerHandler`] to translate between
//! Konf's internal registries and the MCP wire protocol (rmcp 1.3.0).

use std::sync::Arc;

use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResult,
    Content,
    Implementation,
    ListResourcesResult, ListToolsResult,
    PaginatedRequestParams,
    ReadResourceRequestParams, ReadResourceResult,
    ResourceContents,
    RawResource,
    ServerCapabilities,
    Tool as McpTool,
};
use rmcp::service::{RequestContext, RoleServer};
use rmcp::model::{AnnotateAble, ErrorData};
use rmcp::ServiceExt;

use konflux::Engine;
use konflux::tool::ToolInfo;
use konf_runtime::Runtime;

/// MCP server info type alias (rmcp names it InitializeResult).
type ServerInfo = rmcp::model::InitializeResult;

/// The Konf MCP server. Translates engine registries to MCP wire protocol.
pub struct KonfMcpServer {
    engine: Arc<Engine>,
    runtime: Arc<Runtime>,
}

impl KonfMcpServer {
    /// Create a new MCP server backed by the given engine and runtime.
    pub fn new(engine: Arc<Engine>, runtime: Arc<Runtime>) -> Self {
        Self { engine, runtime }
    }

    /// Serve MCP over stdio (for CLI / Claude Desktop integration).
    pub async fn serve_stdio(self) -> anyhow::Result<()> {
        tracing::info!("Starting MCP server on stdio");
        let transport = rmcp::transport::stdio();
        let service = self.serve(transport).await
            .map_err(|e| anyhow::anyhow!("MCP stdio server failed: {e}"))?;
        service.waiting().await
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
}

/// Convert a Konf ToolInfo to an MCP Tool definition.
fn tool_info_to_mcp(info: &ToolInfo) -> McpTool {
    let schema_obj = info.input_schema.as_object().cloned().unwrap_or_default();

    // NOTE: Do NOT add .with_annotations() here.
    // Claude Code silently drops ALL tools when annotations are present
    // in the tools/list response (anthropics/claude-code#25081).
    McpTool::new(
        info.name.clone(),
        info.description.clone(),
        Arc::new(schema_obj),
    )
}

/// Build a ToolContext for an MCP tool call.
fn mcp_tool_context(_tool_name: &str) -> konflux::tool::ToolContext {
    // TODO: Replace with per-session capability grants when MCP auth is implemented.
    // For now, MCP clients get infra-level access (all capabilities).
    // This is acceptable for the architect use case (trusted, single-user)
    // but must be scoped before multi-tenant MCP access.
    konflux::tool::ToolContext {
        capabilities: vec!["*".into()],
        workflow_id: "mcp".into(),
        node_id: "mcp_call".into(),
        metadata: std::collections::HashMap::new(),
    }
}

impl ServerHandler for KonfMcpServer {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(
            ServerCapabilities::builder()
                .enable_tools()
                .enable_resources()
                .build()
        )
        .with_server_info(Implementation::new("konf", env!("CARGO_PKG_VERSION")))
        .with_instructions("Konf AI agent platform. Provides tools for memory, LLM, HTTP, and workflow execution.")
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, ErrorData> {
        let registry = self.engine.registry();
        let tools: Vec<McpTool> = registry.list()
            .iter()
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
        let tool_name = &request.name;
        tracing::info!(tool = %tool_name, "MCP tools/call");

        let registry = self.engine.registry();
        let tool = registry.get(tool_name).ok_or_else(|| {
            ErrorData::invalid_params(format!("Tool not found: {tool_name}"), None)
        })?;

        let input: serde_json::Value = match request.arguments {
            Some(args) => serde_json::Value::Object(args),
            None => serde_json::Value::Object(serde_json::Map::new()),
        };

        let ctx = mcp_tool_context(tool_name);

        match tool.invoke(input, &ctx).await {
            Ok(output) => {
                let text = serde_json::to_string_pretty(&output)
                    .unwrap_or_else(|_| output.to_string());
                Ok(CallToolResult::success(vec![Content::text(text)]))
            }
            Err(e) => {
                tracing::warn!(tool = %tool_name, error = %e, "MCP tool call failed");
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
        let resources = registry.list()
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
        let resource = registry.get(uri).ok_or_else(|| {
            ErrorData::invalid_params(format!("Resource not found: {uri}"), None)
        })?;

        let content = resource.read().await.map_err(|e| {
            ErrorData::internal_error(format!("Failed to read resource: {e}"), None)
        })?;

        let text = serde_json::to_string_pretty(&content)
            .unwrap_or_else(|_| content.to_string());

        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(text, uri.clone())
        ]))
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
            annotations: konflux::tool::ToolAnnotations {
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
        let runtime = rt.block_on(async {
            Arc::new(Runtime::new(Engine::new(), None).await.unwrap())
        });
        let server = KonfMcpServer::new(engine, runtime);
        let info = server.get_info();
        assert_eq!(info.server_info.name, "konf");
    }
}

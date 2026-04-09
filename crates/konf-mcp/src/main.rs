//! konf-mcp binary — standalone MCP server for the Konf platform.
//!
//! Runs independently of konf-backend:
//! - `konf-mcp --config ./config` — stdio mode (default, for Claude Desktop / CLI)
//!
//! SSE transport is available when mounted in konf-backend at /mcp.

use std::path::PathBuf;
use std::sync::Arc;

use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // MCP servers MUST NOT write to stdout (corrupts JSON-RPC).
    // Route all tracing to stderr.
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let args: Vec<String> = std::env::args().collect();
    let config_dir = args
        .iter()
        .position(|a| a == "--config")
        .and_then(|i| args.get(i + 1))
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("./config"));

    // Boot the platform
    let instance = konf_init::boot(&config_dir).await?;

    let engine = instance.runtime.engine();
    tracing::info!(
        tools = engine.registry().len(),
        resources = engine.resources().len(),
        "Konf MCP server ready"
    );

    // Serve via stdio (Claude Desktop, CLI, piped connections)
    let server = konf_mcp::KonfMcpServer::new(Arc::new(engine.clone()), instance.runtime.clone());
    server.serve_stdio().await?;

    Ok(())
}

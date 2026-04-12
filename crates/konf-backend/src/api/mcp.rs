//! `/mcp` — Streamable HTTP MCP endpoint mounted on the backend's axum router.
//!
//! This module is the Phase 4 split-brain fix: MCP clients (Claude Code,
//! mcp-inspector, Gemini CLI) connect to `http://host:port/mcp` and share
//! the same `Arc<Runtime>` as the REST API. Runs started via MCP appear in
//! the shared [`konf_runtime::ProcessTable`], are recorded in the same
//! journal, and stream through the same monitor bus as any other run.
//!
//! # Dev-mode only
//!
//! This endpoint is gated behind the `KONF_MCP_HTTP=1` environment variable
//! (handled in [`main`](crate::main)). When enabled, every session gets
//! capabilities `["*"]`. This is intentional for v1: the single-node,
//! single-user local deployment doesn't need per-session scoping, and a
//! production MCP-over-HTTP auth story is tracked as future work in
//! `docs/plans/konf-v2.md` §16.
//!
//! **Never enable `KONF_MCP_HTTP=1` on a network-exposed deployment.**
//!
//! # Implementation
//!
//! The wiring is very thin — rmcp 1.3's `StreamableHttpService` implements
//! Tower's `Service` trait, so axum's `nest_service` picks it up directly.
//! The factory closure captures an `Arc<Runtime>` and builds a fresh
//! [`konf_mcp::KonfMcpServer`] per session; the runtime/engine themselves
//! are shared (cheap Arc clones).

use std::sync::Arc;

use axum::Router;
use konf_mcp::http::{LocalSessionManager, StreamableHttpService};
use konf_runtime::Runtime;

/// Build an axum router that serves the MCP Streamable HTTP endpoint at
/// `/mcp`. The router is ready to be `.merge`d into the main app router.
pub fn routes(runtime: Arc<Runtime>) -> Router {
    let service = StreamableHttpService::new(
        move || {
            let engine = Arc::new(runtime.engine().clone());
            Ok(konf_mcp::KonfMcpServer::new(engine, runtime.clone()))
        },
        Arc::new(LocalSessionManager::default()),
        Default::default(),
    );

    Router::new().nest_service("/mcp", service)
}

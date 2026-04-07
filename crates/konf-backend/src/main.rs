//! konf-backend — HTTP server for the Konf agentic AI platform.
//!
//! A thin transport shell over the engine. Uses konf-init for bootstrapping.
//! Contains zero tool implementations — all tools live in konf-tools crates.

mod api;
mod auth;
mod error;
mod scheduling;
mod templates;

use std::sync::Arc;
use std::path::PathBuf;

use axum::{Router, routing::{get, post}, middleware};
use tower_http::cors::{Any, CorsLayer};
use tower_http::trace::TraceLayer;
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::auth::jwt::JwtVerifier;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    // 1. Initialize tracing
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    // 2. Boot the platform via konf-init
    let config_dir = std::env::var("KONF_CONFIG_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from("./config"));

    let instance = konf_init::boot(&config_dir).await?;

    // 3. Set up auth
    let verifier = Arc::new(JwtVerifier::new(&instance.config.auth));

    // 4. Set up scheduler (only if DB pool is available from konf-init)
    #[cfg(feature = "scheduling")]
    if let Some(ref pool) = instance.pool {
        let scheduler = Arc::new(scheduling::Scheduler::new(pool.clone(), instance.runtime.clone()));
        scheduler.migrate().await?;
        scheduler.clone().start_polling(10);
        info!("Scheduler started");
    }

    // 5. Build app state
    let app_state = api::chat::AppState {
        runtime: instance.runtime.clone(),
        default_workflow_yaml: templates::CHAT_WORKFLOW.to_string(),
        default_capabilities: vec![
            konf_runtime::scope::CapabilityGrant::new("*"),
        ],
        default_limits: instance.config.runtime.clone(),
        namespace_template: "konf:default:${user_id}".into(),
    };

    // 6. Build router
    let protected = Router::new()
        .route("/v1/me", get(api::me::me))
        .route("/v1/chat", post(api::chat::chat))
        .route("/v1/monitor/runs", get(api::monitor::list_runs))
        .route("/v1/monitor/runs/{id}", get(api::monitor::get_run).delete(api::monitor::cancel_run))
        .route("/v1/monitor/runs/{id}/tree", get(api::monitor::get_tree))
        .route("/v1/monitor/metrics", get(api::monitor::metrics))
        .route("/v1/admin/config", get(api::admin::get_config))
        .route("/v1/admin/audit", get(api::admin::get_audit))
        .layer(middleware::from_fn({
            let v = verifier.clone();
            move |req, next| {
                let v = v.clone();
                auth::middleware::auth_middleware(v, req, next)
            }
        }))
        .with_state(app_state);

    // CORS: configurable origins. Empty list = allow all (dev). Production should set explicitly.
    let cors = if instance.config.server.cors_origins.is_empty() {
        CorsLayer::new()
            .allow_origin(Any)
            .allow_methods(Any)
            .allow_headers(Any)
    } else {
        let origins: Vec<_> = instance.config.server.cors_origins.iter()
            .filter_map(|o| o.parse::<axum::http::HeaderValue>().ok())
            .collect();
        CorsLayer::new()
            .allow_origin(origins)
            .allow_methods(Any)
            .allow_headers(Any)
    };

    // MCP is served via konf-mcp binary (stdio transport), not embedded in HTTP server.
    // SSE transport for MCP will be added when rmcp's StreamableHttpService is integrated.

    let app = Router::new()
        .route("/v1/health", get(api::health::health))
        .merge(protected)
        .layer(cors)
        .layer(TraceLayer::new_for_http());

    // 8. Bind and serve with graceful shutdown
    let addr = format!("{}:{}", instance.config.server.host, instance.config.server.port);
    let listener = tokio::net::TcpListener::bind(&addr).await?;
    info!(addr = %addr, "Listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await?;

    info!("Server shut down gracefully");
    Ok(())
}

async fn shutdown_signal() {
    let ctrl_c = async {
        tokio::signal::ctrl_c()
            .await
            .expect("failed to install Ctrl+C handler");
    };

    #[cfg(unix)]
    let terminate = async {
        tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
            .expect("failed to install SIGTERM handler")
            .recv()
            .await;
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => { info!("Received Ctrl+C, shutting down"); }
        _ = terminate => { info!("Received SIGTERM, shutting down"); }
    }
}

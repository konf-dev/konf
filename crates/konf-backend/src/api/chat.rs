//! Chat endpoint — POST /v1/chat with SSE streaming.
//!
//! Flow:
//! 1. Auth middleware verifies JWT → user_id
//! 2. Build ExecutionScope from config + user
//! 3. Parse workflow from product config
//! 4. runtime.start_streaming() → (RunId, StreamReceiver)
//! 5. Pipe StreamEvents to SSE events
//! 6. Stream to client

use std::convert::Infallible;
use std::sync::Arc;

use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::Json;
use futures::stream::Stream;
use serde::Deserialize;
use serde_json::json;
use tracing::info;

use konflux::stream::{StreamEvent, ProgressType};
use konf_runtime::scope::{Actor, ActorRole, CapabilityGrant, ExecutionScope, ResourceLimits};

use crate::auth::middleware::AuthUser;
use crate::error::AppError;

/// Shared application state passed to handlers.
#[derive(Clone)]
pub struct AppState {
    /// The runtime for executing workflows
    pub runtime: Arc<konf_runtime::Runtime>,
    /// Default workflow YAML to execute for chat messages
    pub default_workflow_yaml: String,
    /// Default capabilities granted to chat users
    pub default_capabilities: Vec<CapabilityGrant>,
    /// Default resource limits for chat executions
    pub default_limits: ResourceLimits,
    /// Template for namespace, e.g. "konf:default:${user_id}"
    pub namespace_template: String,
}

/// Chat request body.
#[derive(Debug, Deserialize)]
pub struct ChatRequest {
    /// The user's message
    pub message: String,
    /// Session ID (generated if not provided)
    #[serde(default = "default_session_id")]
    pub session_id: String,
}

fn default_session_id() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// POST /v1/chat — streaming chat endpoint.
pub async fn chat(
    AuthUser(claims): AuthUser,
    State(state): State<AppState>,
    Json(req): Json<ChatRequest>,
) -> Result<Sse<impl Stream<Item = Result<Event, Infallible>>>, AppError> {
    let user_id = &claims.sub;
    let namespace = state.namespace_template
        .replace("${user_id}", user_id);

    info!(
        user_id = %user_id,
        namespace = %namespace,
        session_id = %req.session_id,
        "Chat request"
    );

    // Build execution scope
    let scope = ExecutionScope {
        namespace: namespace.clone(),
        capabilities: state.default_capabilities.clone(),
        limits: state.default_limits.clone(),
        actor: Actor {
            id: user_id.clone(),
            role: claims.role.as_deref()
                .and_then(|r| serde_json::from_value(json!(r)).ok())
                .unwrap_or(ActorRole::User),
        },
        depth: 0,
    };

    // Parse workflow
    let workflow = state.runtime.parse_yaml(&state.default_workflow_yaml)
        .map_err(|e| AppError::BadRequest(format!("Invalid workflow: {e}")))?;

    // Build input
    let input = json!({
        "message": req.message,
        "user_id": user_id,
        "session_id": req.session_id,
    });

    // Start streaming execution
    let (run_id, mut rx) = state.runtime
        .start_streaming(&workflow, input, scope, req.session_id.clone())
        .await
        .map_err(|e| AppError::Internal(format!("Failed to start workflow: {e}")))?;

    info!(run_id = %run_id, "Workflow started (streaming)");

    // Pipe StreamReceiver → SSE events
    let stream = async_stream::stream! {
        // Send start event
        yield Ok(Event::default()
            .event("start")
            .data(json!({"run_id": run_id.to_string()}).to_string()));

        // Stream events from the engine
        while let Some(event) = rx.recv().await {
            match event {
                StreamEvent::Start { workflow_id } => {
                    yield Ok(Event::default()
                        .event("workflow_start")
                        .data(json!({"workflow_id": workflow_id}).to_string()));
                }
                StreamEvent::Progress { node_id, event_type, data } => {
                    let sse_event = match event_type {
                        ProgressType::TextDelta => "text_delta",
                        ProgressType::ToolStart => "tool_start",
                        ProgressType::ToolEnd => "tool_end",
                        ProgressType::Status => "status",
                    };
                    yield Ok(Event::default()
                        .event(sse_event)
                        .data(json!({
                            "node_id": node_id,
                            "data": data,
                        }).to_string()));
                }
                StreamEvent::Done { output } => {
                    yield Ok(Event::default()
                        .event("done")
                        .data(json!({
                            "run_id": run_id.to_string(),
                            "output": output,
                        }).to_string()));
                    break;
                }
                StreamEvent::Error { code, message, retryable } => {
                    yield Ok(Event::default()
                        .event("error")
                        .data(json!({
                            "code": code,
                            "message": message,
                            "retryable": retryable,
                        }).to_string()));
                    break;
                }
            }
        }
    };

    Ok(Sse::new(stream).keep_alive(KeepAlive::default()))
}

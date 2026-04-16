//! Dispatcher — unified single-tool dispatch with capability check, wrapping, and journaling.
//!
//! Extracted from `Runtime::invoke_tool` in Stage 5.b. This is the single
//! entry point for ad-hoc tool invocation (MCP, HTTP transport layers).
//! Workflow-node dispatch goes through the substrate executor, which has its
//! own capability check and envelope construction.

use std::collections::HashMap;
use std::sync::Arc;

use chrono::Utc;
use serde_json::Value;
use tracing::debug;

use konflux_substrate::tool::ToolRegistry;

use crate::context::VirtualizedTool;
use crate::error::RuntimeError;
use crate::event_bus::{RunEvent, RunEventBus};
use crate::execution_context::ExecutionContext;
use crate::guard::GuardedTool;
use crate::interaction::{Interaction, InteractionKind, InteractionStatus};
use crate::journal::{JournalEntry, JournalStore};
use crate::runtime::ToolGuardEntry;
use crate::scope::ExecutionScope;

/// Handles single-tool dispatch with capability check, VirtualizedTool +
/// GuardedTool wrapping, envelope construction, invocation, Interaction
/// recording, and event emission.
pub(crate) struct Dispatcher {
    pub(crate) tool_guards: Arc<std::sync::RwLock<HashMap<String, ToolGuardEntry>>>,
    pub(crate) journal: Option<Arc<dyn JournalStore>>,
    pub(crate) event_bus: Arc<RunEventBus>,
}

impl Dispatcher {
    /// Dispatch a single tool invocation under the given scope and context.
    ///
    /// `registry` is a snapshot of the engine's tool registry at call time.
    pub async fn dispatch_tool(
        &self,
        tool_name: &str,
        input: Value,
        scope: &ExecutionScope,
        ctx: &ExecutionContext,
        registry: &ToolRegistry,
    ) -> Result<Value, RuntimeError> {
        // 1. Capability check — returns the bindings (for namespace
        //    injection) if the scope grants this tool.
        let bindings = scope.check_tool(tool_name)?;
        let namespace_injected = bindings.contains_key("namespace");

        // 2. Resolve the raw tool from the engine registry.
        let raw_tool = registry.get(tool_name).ok_or_else(|| {
            RuntimeError::CapabilityDenied(format!(
                "tool '{tool_name}' not found in engine registry"
            ))
        })?;

        // 3. Layer 1: VirtualizedTool wraps the raw tool to inject the
        //    scope's parameter bindings (e.g. namespace) before the tool
        //    sees the input.
        let wrapped: Arc<dyn konflux_substrate::tool::Tool> = if bindings.is_empty() {
            raw_tool
        } else {
            Arc::new(VirtualizedTool::new(raw_tool, bindings))
        };

        // 4. Layer 2: GuardedTool applies deny/allow rules from
        //    tools.yaml::tool_guards.
        let wrapped = {
            let guards = self.tool_guards.read().expect("tool_guards lock poisoned");
            if let Some(guard_entry) = guards.get(tool_name) {
                if guard_entry.rules.is_empty() {
                    wrapped
                } else {
                    debug!(
                        tool = %tool_name,
                        rule_count = guard_entry.rules.len(),
                        "dispatch_tool: applying guards"
                    );
                    Arc::new(GuardedTool::new(
                        wrapped,
                        guard_entry.rules.clone(),
                        guard_entry.default_action,
                    ))
                }
            } else {
                wrapped
            }
        };

        // 5. Build the dispatch envelope and invoke.
        let actor_role_str = match scope.actor.role {
            crate::scope::ActorRole::InfraAdmin => "infra_admin",
            crate::scope::ActorRole::ProductAdmin => "product_admin",
            crate::scope::ActorRole::User => "user",
            crate::scope::ActorRole::InfraAgent => "infra_agent",
            crate::scope::ActorRole::ProductAgent => "product_agent",
            crate::scope::ActorRole::UserAgent => "user_agent",
            crate::scope::ActorRole::System => "system",
        };
        let mut env = konflux_substrate::envelope::Envelope::for_tool_dispatch(
            tool_name,
            input,
            &scope.capability_patterns(),
            ctx.trace_id,
            &scope.namespace,
            &scope.actor.id,
            &ctx.session_id,
        );
        env.metadata
            .0
            .insert("actor_role".into(), Value::String(actor_role_str.into()));
        env.metadata
            .0
            .insert("session_id".into(), Value::String(ctx.session_id.clone()));
        env.metadata.0.insert(
            "depth".into(),
            Value::Number(serde_json::Number::from(scope.depth as u64)),
        );

        let started_at = Utc::now();
        let edge_rules_fired = {
            let mut v = vec![format!("cap_check:{tool_name}")];
            if namespace_injected {
                v.push(format!("ns_inject:{}", scope.namespace));
            }
            v
        };
        let trace_id = ctx.trace_id;
        let interaction_id = uuid::Uuid::new_v4();

        let result = wrapped.invoke(env).await;
        let ended_at = Utc::now();
        let duration_ms = (ended_at - started_at).num_milliseconds().max(0) as u64;

        self.event_bus.emit(RunEvent::ToolInvoked {
            tool: tool_name.to_string(),
            namespace: scope.namespace.clone(),
            at: ended_at,
            success: result.is_ok(),
        });

        // Append an Interaction-shaped JournalEntry for this dispatch.
        if let Some(journal) = self.journal.as_ref().cloned() {
            let interaction = Interaction {
                id: interaction_id,
                parent_id: ctx.parent_interaction_id,
                trace_id,
                run_id: None,
                node_id: None,
                actor: scope.actor.clone(),
                namespace: scope.namespace.clone(),
                target: format!("tool:{tool_name}"),
                kind: InteractionKind::ToolDispatch,
                attributes: serde_json::json!({
                    "tool_name": tool_name,
                    "duration_ms": duration_ms,
                }),
                edge_rules_fired,
                status: match &result {
                    Ok(_) => InteractionStatus::Ok,
                    Err(e) => InteractionStatus::Failed {
                        error: e.to_string(),
                    },
                },
                summary: None,
                timestamp: started_at,
            };
            let entry = JournalEntry {
                run_id: None,
                session_id: ctx.session_id.clone(),
                namespace: scope.namespace.clone(),
                event_type: "interaction".into(),
                payload: interaction.to_json(),
            };
            if let Err(e) = journal.append(entry).await {
                tracing::warn!(error = %e, "dispatch_tool: failed to append interaction");
            }
        }

        result
            .map(|env| env.payload)
            .map_err(|e| RuntimeError::Tool {
                tool: tool_name.to_string(),
                message: e.to_string(),
            })
    }
}

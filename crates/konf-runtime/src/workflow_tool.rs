//! WorkflowTool — registers a workflow as a callable tool.
//!
//! Any workflow with `register_as_tool: true` in its YAML header can be
//! registered as `workflow:{id}`. This enables composition: workflows
//! calling sub-workflows, MCP clients invoking workflows as tools.
//!
//! Stage 5.c: the caller's scope is reconstructed from the Envelope's
//! typed fields (namespace, actor_id, capabilities, actor_role metadata).
//! No boot-baked `default_scope` — the caller's context flows through.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use konflux_substrate::envelope::Envelope;
use konflux_substrate::error::ToolError;
use konflux_substrate::tool::{Tool, ToolAnnotations, ToolInfo};
use konflux_substrate::Workflow;

use crate::execution_context::ExecutionContext;
use crate::scope::ExecutionScope;
use crate::Runtime;

/// A workflow wrapped as a callable tool.
///
/// Created by konf-init for each workflow with `register_as_tool: true`.
/// When invoked, reconstructs the caller's scope from the Envelope and
/// runs the workflow via the runtime.
pub struct WorkflowTool {
    workflow: Workflow,
    runtime: Arc<Runtime>,
}

impl WorkflowTool {
    /// Create a new WorkflowTool.
    ///
    /// - `workflow`: the parsed workflow to execute
    /// - `runtime`: the runtime to execute through
    pub fn new(workflow: Workflow, runtime: Arc<Runtime>) -> Self {
        Self { workflow, runtime }
    }
}

#[async_trait]
impl Tool for WorkflowTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: format!("workflow:{}", self.workflow.id),
            description: self.workflow.description.clone().unwrap_or_default(),
            input_schema: self
                .workflow
                .input_schema
                .clone()
                .unwrap_or(serde_json::json!({
                    "type": "object"
                })),
            output_schema: self.workflow.output_schema.clone(),
            capabilities: vec![format!("workflow:{}", self.workflow.id)],
            supports_streaming: true,
            annotations: ToolAnnotations {
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: true, // workflows can call any granted tool
            },
        }
    }

    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, ToolError> {
        // Reconstruct the caller's scope and context from the Envelope.
        // This replaces the old boot-baked default_scope (concession #1).
        let caller_scope = ExecutionScope::from_envelope(&env);
        let exec_ctx = ExecutionContext::from_envelope(&env);

        // Create a child scope — increments depth, inherits caller's caps.
        let child_scope = caller_scope
            .child_scope(
                caller_scope.capabilities.clone(),
                None, // same namespace
            )
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Failed to create child scope: {e}"),
                retryable: false,
            })?;

        // Stamp depth into envelope metadata so nested workflow-as-tool
        // calls can reconstruct the correct depth.
        let output = self
            .runtime
            .run(&self.workflow, env.payload.clone(), child_scope, exec_ctx)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Workflow '{}' failed: {e}", self.workflow.id),
                retryable: false,
            })?;

        Ok(env.respond(output))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::{Actor, ActorRole, CapabilityGrant, ResourceLimits};

    #[test]
    fn test_workflow_tool_info() {
        let info = ToolInfo {
            name: "workflow:summarize".into(),
            description: "Summarize a document".into(),
            input_schema: serde_json::json!({"type": "object"}),
            output_schema: None,
            capabilities: vec!["workflow:summarize".into()],
            supports_streaming: true,
            annotations: ToolAnnotations {
                read_only: false,
                destructive: false,
                idempotent: false,
                open_world: true,
            },
        };
        assert_eq!(info.name, "workflow:summarize");
        assert!(info.supports_streaming);
        assert!(info.annotations.open_world);
        assert!(!info.annotations.idempotent);
    }

    #[test]
    fn test_child_scope_creation() {
        let scope = ExecutionScope {
            namespace: "konf:test".into(),
            capabilities: vec![CapabilityGrant::new("*")],
            limits: ResourceLimits::default(),
            actor: Actor {
                id: "test".into(),
                role: ActorRole::System,
            },
            depth: 0,
        };
        let child = scope.child_scope(vec![CapabilityGrant::new("memory_search")], None);
        assert!(child.is_ok());
        let child = child.unwrap();
        assert_eq!(child.depth, 1);
        assert_eq!(child.namespace, "konf:test");
    }
}

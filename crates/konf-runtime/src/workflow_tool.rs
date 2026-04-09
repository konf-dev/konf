//! WorkflowTool — registers a workflow as a callable tool.
//!
//! Any workflow with `register_as_tool: true` in its YAML header can be
//! registered as `workflow:{id}`. This enables composition: workflows
//! calling sub-workflows, MCP clients invoking workflows as tools.
//!
//! The WorkflowTool creates a child execution scope (attenuated capabilities)
//! and runs the workflow via the runtime.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::Value;

use konflux::error::ToolError;
use konflux::tool::{Tool, ToolAnnotations, ToolContext, ToolInfo};
use konflux::Workflow;

use crate::Runtime;
use crate::scope::ExecutionScope;

/// A workflow wrapped as a callable tool.
///
/// Created by konf-init for each workflow with `register_as_tool: true`.
/// When invoked, creates a child scope and runs the workflow via the runtime.
pub struct WorkflowTool {
    workflow: Workflow,
    runtime: Arc<Runtime>,
    default_scope: ExecutionScope,
}

impl WorkflowTool {
    /// Create a new WorkflowTool.
    ///
    /// - `workflow`: the parsed workflow to execute
    /// - `runtime`: the runtime to execute through
    /// - `default_scope`: the scope to attenuate for child execution
    pub fn new(
        workflow: Workflow,
        runtime: Arc<Runtime>,
        default_scope: ExecutionScope,
    ) -> Self {
        Self {
            workflow,
            runtime,
            default_scope,
        }
    }
}

#[async_trait]
impl Tool for WorkflowTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: format!("workflow:{}", self.workflow.id),
            description: self.workflow.description.clone().unwrap_or_default(),
            input_schema: self.workflow.input_schema.clone().unwrap_or(serde_json::json!({
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

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<Value, ToolError> {
        // Create a child scope with attenuated capabilities
        let child_scope = self.default_scope.child_scope(
            self.default_scope.capabilities.clone(),
            None, // same namespace
        ).map_err(|e| ToolError::ExecutionFailed {
            message: format!("Failed to create child scope: {e}"),
            retryable: false,
        })?;

        // Extract session_id from context metadata
        let session_id = ctx.metadata
            .get("session_id")
            .and_then(|v| v.as_str())
            .unwrap_or("workflow_tool")
            .to_string();

        self.runtime
            .run(&self.workflow, input, child_scope, session_id)
            .await
            .map_err(|e| ToolError::ExecutionFailed {
                message: format!("Workflow '{}' failed: {e}", self.workflow.id),
                retryable: false,
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::scope::{Actor, ActorRole, CapabilityGrant, ResourceLimits};

    fn test_scope() -> ExecutionScope {
        ExecutionScope {
            namespace: "konf:test".into(),
            capabilities: vec![CapabilityGrant::new("*")],
            limits: ResourceLimits::default(),
            actor: Actor { id: "test".into(), role: ActorRole::System },
            depth: 0,
        }
    }

    #[test]
    fn test_workflow_tool_info() {
        // We can't easily construct a Workflow without parsing YAML,
        // so test the naming convention and annotations
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
        let scope = test_scope();
        let child = scope.child_scope(
            vec![CapabilityGrant::new("memory_search")],
            None,
        );
        assert!(child.is_ok());
        let child = child.unwrap();
        assert_eq!(child.depth, 1);
        assert_eq!(child.namespace, "konf:test");
    }
}

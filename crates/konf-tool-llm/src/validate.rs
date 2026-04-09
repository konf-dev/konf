//! Workflow validation tool — validates YAML workflows against the Konf kernel.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use konflux::engine::Engine;
use konflux::error::ToolError as KonfluxToolError;
use konflux::tool::{Tool, ToolAnnotations, ToolContext, ToolInfo};

/// Validates a workflow YAML string against the Konf kernel.
///
/// Checks syntax, schema, tool references, and capability requirements.
pub struct ValidateWorkflowTool {
    engine: Arc<Engine>,
}

impl ValidateWorkflowTool {
    /// Create a new `ValidateWorkflowTool` with access to the engine.
    pub fn new(engine: Arc<Engine>) -> Self {
        Self { engine }
    }
}

/// Check if a capability pattern matches a required capability.
/// Uses the same logic as `konf_runtime::scope::matches_capability_pattern`.
fn matches_capability_pattern(pattern: &str, capability: &str) -> bool {
    if pattern == "*" {
        return true;
    }
    if let Some(prefix) = pattern.strip_suffix(":*") {
        return capability.starts_with(prefix)
            && capability.get(prefix.len()..prefix.len() + 1) == Some(":");
    }
    pattern == capability
}

#[async_trait]
impl Tool for ValidateWorkflowTool {
    fn info(&self) -> ToolInfo {
        ToolInfo {
            name: "yaml:validate_workflow".into(),
            description: "Validate a workflow YAML string against the Konf kernel. Checks syntax, schema, tool references, and capability requirements.".into(),
            input_schema: json!({
                "type": "object",
                "properties": {
                    "yaml": {
                        "type": "string",
                        "description": "The workflow YAML string to validate"
                    }
                },
                "required": ["yaml"]
            }),
            capabilities: vec!["yaml:validate_workflow".into()],
            supports_streaming: false,
            output_schema: None,
            annotations: ToolAnnotations {
                read_only: true,
                idempotent: true,
                ..Default::default()
            },
        }
    }

    async fn invoke(&self, input: Value, ctx: &ToolContext) -> Result<Value, KonfluxToolError> {
        let yaml = input.get("yaml").and_then(|v| v.as_str()).ok_or_else(|| {
            KonfluxToolError::InvalidInput {
                message: "Missing required field 'yaml' (string)".into(),
                field: Some("yaml".into()),
            }
        })?;

        // Step 1: Parse the YAML into a Workflow
        let workflow = match self.engine.parse_yaml(yaml) {
            Ok(wf) => wf,
            Err(e) => {
                return Ok(json!({
                    "valid": false,
                    "errors": [format!("Parse error: {e}")]
                }));
            }
        };

        let mut errors: Vec<String> = Vec::new();
        let registry = self.engine.registry();

        // Step 2: Check that each step's tool is registered
        for step in &workflow.steps {
            let tool_name = step.tool.as_str();
            if !registry.contains(tool_name) {
                errors.push(format!(
                    "Tool '{}' (used in step '{}') is not registered",
                    tool_name, step.id
                ));
            }
        }

        // Step 3: Check capability attenuation — caller must cover workflow capabilities
        for cap in &workflow.capabilities {
            let covered = ctx
                .capabilities
                .iter()
                .any(|grant| matches_capability_pattern(grant, cap));
            if !covered {
                errors.push(format!(
                    "Workflow requires '{}' but caller does not have a matching grant",
                    cap
                ));
            }
        }

        if errors.is_empty() {
            let capabilities_required: Vec<&str> =
                workflow.capabilities.iter().map(|s| s.as_str()).collect();

            Ok(json!({
                "valid": true,
                "workflow_id": workflow.id.as_str(),
                "node_count": workflow.steps.len(),
                "capabilities_required": capabilities_required,
            }))
        } else {
            Ok(json!({
                "valid": false,
                "errors": errors,
            }))
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_engine() -> Arc<Engine> {
        Arc::new(Engine::new())
    }

    #[test]
    fn test_validate_workflow_tool_info() {
        let tool = ValidateWorkflowTool::new(make_engine());
        let info = tool.info();

        assert_eq!(info.name, "yaml:validate_workflow");
        assert_eq!(info.capabilities, vec!["yaml:validate_workflow"]);
        assert!(info.annotations.read_only);
        assert!(info.annotations.idempotent);
        assert!(!info.annotations.destructive);
        assert!(!info.annotations.open_world);
    }

    #[test]
    fn test_matches_capability_pattern_exact() {
        assert!(matches_capability_pattern("ai:complete", "ai:complete"));
        assert!(!matches_capability_pattern("ai:complete", "ai:other"));
    }

    #[test]
    fn test_matches_capability_pattern_glob() {
        assert!(matches_capability_pattern("ai:*", "ai:complete"));
        assert!(matches_capability_pattern("ai:*", "ai:other"));
        assert!(!matches_capability_pattern("ai:*", "memory:search"));
    }

    #[test]
    fn test_matches_capability_pattern_wildcard() {
        assert!(matches_capability_pattern("*", "anything"));
        assert!(matches_capability_pattern("*", "ai:complete"));
    }
}

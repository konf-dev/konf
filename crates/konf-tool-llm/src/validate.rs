//! Workflow validation tool — validates YAML workflows against the Konf kernel.

use std::sync::Arc;

use async_trait::async_trait;
use serde_json::{json, Value};

use konflux_substrate::engine::Engine;
use konflux_substrate::envelope::Envelope;
use konflux_substrate::error::ToolError as KonfluxToolError;
use konflux_substrate::tool::{Tool, ToolAnnotations, ToolInfo};

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

use konflux_substrate::envelope::Capability;

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

    async fn invoke(&self, env: Envelope<Value>) -> Result<Envelope<Value>, KonfluxToolError> {
        let yaml = env
            .payload
            .get("yaml")
            .and_then(|v| v.as_str())
            .ok_or_else(|| KonfluxToolError::InvalidInput {
                message: "Missing required field 'yaml' (string)".into(),
                field: Some("yaml".into()),
            })?;

        // Step 1: Parse the YAML into a Workflow
        let workflow = match self.engine.parse_yaml(yaml) {
            Ok(wf) => wf,
            Err(e) => {
                return Ok(env.respond(json!({
                    "valid": false,
                    "errors": [format!("Parse error: {e}")]
                })));
            }
        };

        let mut errors: Vec<String> = Vec::new();
        let registry = self.engine.registry();
        let capability_patterns = env.capabilities.to_patterns();

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
            let covered = capability_patterns
                .iter()
                .any(|grant| Capability::new(grant.as_str()).matches(cap));
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

            Ok(env.respond(json!({
                "valid": true,
                "workflow_id": workflow.id.as_str(),
                "node_count": workflow.steps.len(),
                "capabilities_required": capabilities_required,
            })))
        } else {
            Ok(env.respond(json!({
                "valid": false,
                "errors": errors,
            })))
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
        assert!(Capability::new("ai:complete").matches("ai:complete"));
        assert!(!Capability::new("ai:complete").matches("ai:other"));
    }

    #[test]
    fn test_matches_capability_pattern_glob() {
        assert!(Capability::new("ai:*").matches("ai:complete"));
        assert!(Capability::new("ai:*").matches("ai:other"));
        assert!(!Capability::new("ai:*").matches("memory:search"));
    }

    #[test]
    fn test_matches_capability_pattern_wildcard() {
        assert!(Capability::new("*").matches("anything"));
        assert!(Capability::new("*").matches("ai:complete"));
    }
}

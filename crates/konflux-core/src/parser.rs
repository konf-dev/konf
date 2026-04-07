//! YAML workflow parser.

pub mod schema;
pub mod compiler;
pub mod validator;
pub mod graph;

use tracing::{info_span, debug};

use crate::error::KonfluxError;
use crate::workflow::Workflow;
use crate::parser::schema::WorkflowSchema;

/// Parse a YAML string into a Workflow IR.
pub fn parse(yaml: &str) -> Result<Workflow, KonfluxError> {
    let _span = info_span!("workflow.parse", yaml_size = yaml.len()).entered();
    let start = std::time::Instant::now();

    let schema: WorkflowSchema = serde_yaml::from_str(yaml).map_err(|e| {
        crate::error::ParseError::InvalidYaml { message: e.to_string() }
    })?;

    debug!(workflow = %schema.workflow, "YAML deserialized");

    validator::validate(&schema)?;
    debug!("validation passed");

    let graph = graph::DependencyGraph::build(&schema);

    let workflow = compiler::compile(schema, &graph)?;

    debug!(
        workflow_id = %workflow.id,
        steps = workflow.steps.len(),
        duration_ms = start.elapsed().as_millis() as u64,
        "workflow parsed"
    );

    Ok(workflow)
}

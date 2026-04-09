//! YAML schema types for Konflux workflows.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use indexmap::IndexMap;

/// Root structure of a workflow YAML file.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct WorkflowSchema {
    pub workflow: String,
    
    #[serde(default = "default_version")]
    pub version: String,
    
    pub description: Option<String>,

    #[serde(default)]
    pub capabilities: Vec<String>,

    /// JSON Schema for workflow input (used by WorkflowTool)
    pub input_schema: Option<serde_json::Value>,

    /// JSON Schema for workflow output (used by WorkflowTool)
    pub output_schema: Option<serde_json::Value>,

    /// Whether this workflow should register as a callable tool
    #[serde(default)]
    pub register_as_tool: bool,

    pub nodes: IndexMap<String, NodeSchema>,
}

fn default_version() -> String {
    "0.1.0".to_string()
}

/// A single node in the workflow.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct NodeSchema {
    #[serde(rename = "do")]
    pub do_: Option<DoBlock>,

    #[serde(default)]
    pub with: Option<serde_json::Value>,

    #[serde(default)]
    pub pipe: Vec<PipeStepSchema>,

    #[serde(default)]
    pub then: Option<ThenBlock>,

    #[serde(default)]
    pub catch: CatchBlock,

    #[serde(default)]
    pub retry: Option<RetryConfigSchema>,

    pub timeout: Option<String>,

    pub entry: Option<bool>,

    pub repeat: Option<RepeatConfigSchema>,

    #[serde(default)]
    pub stream: StreamConfigSchema,

    #[serde(rename = "return")]
    pub return_: Option<serde_json::Value>,

    pub join: Option<JoinConfigSchema>,

    #[serde(default)]
    pub credentials: HashMap<String, String>,
    
    pub grant: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum DoBlock {
    Single(String),
    Parallel(Vec<IndexMap<String, ParallelTaskConfig>>),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ParallelTaskConfig {
    pub tool: String,
    #[serde(default)]
    pub with: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum PipeStepSchema {
    Simple(String),
    WithArgs(HashMap<String, serde_json::Value>),
    Full {
        #[serde(rename = "do")]
        do_: String,
        #[serde(default)]
        with: Option<serde_json::Value>,
    },
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum ThenBlock {
    Unconditional(String),
    Multiple(Vec<String>),
    Conditional(Vec<ThenBranchSchema>),
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ThenBranchSchema {
    pub when: Option<String>,
    pub then: Option<String>,
    pub goto: Option<String>,
    #[serde(rename = "else")]
    pub else_: Option<bool>,
}

/// Error handling: simple target node OR array of conditional branches.
///
/// # YAML formats
///
/// ```yaml
/// # Simple: jump to a node on any error
/// catch: fallback_node
///
/// # Branches: conditional error handling
/// catch:
///   - when: "true"
///     then: recovery_node
///   - do: skip          # or do: "fallback:default value"
/// ```
#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(untagged)]
pub enum CatchBlock {
    /// Simple string — jump to this node on any error.
    Simple(String),
    /// Array of catch branches with conditions.
    Branches(Vec<CatchBranchSchema>),
}

impl Default for CatchBlock {
    fn default() -> Self {
        Self::Branches(Vec::new())
    }
}

impl CatchBlock {
    /// Returns true if no catch handling is configured.
    pub fn is_empty(&self) -> bool {
        match self {
            CatchBlock::Simple(_) => false,
            CatchBlock::Branches(v) => v.is_empty(),
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct CatchBranchSchema {
    /// Condition: matches if missing, or if value is `true` / `"true"`.
    pub when: Option<serde_json::Value>,
    #[serde(rename = "do")]
    pub do_: Option<String>,
    #[serde(default)]
    pub with: Option<serde_json::Value>,
    pub then: Option<String>,
    #[serde(rename = "else")]
    pub else_: Option<bool>,
}

impl CatchBranchSchema {
    /// Check if this branch's `when` condition is satisfied.
    /// A branch matches if: `when` is absent, or is `true` (bool), or is `"true"` (string).
    pub fn is_match(&self) -> bool {
        if self.else_.unwrap_or(false) {
            return true;
        }
        match &self.when {
            None => true,
            Some(serde_json::Value::Bool(b)) => *b,
            Some(serde_json::Value::String(s)) => s == "true",
            _ => false,
        }
    }
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RetryConfigSchema {
    pub times: u32,
    pub delay: Option<String>,
    pub backoff: Option<String>,
    pub max_delay: Option<String>,
    pub on: Option<Vec<String>>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct RepeatConfigSchema {
    pub until: String,
    pub max: u32,
    #[serde(rename = "as")]
    pub as_: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct JoinConfigSchema {
    pub wait_for: Vec<String>,
    pub policy: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
#[serde(untagged)]
pub enum StreamConfigSchema {
    #[default]
    None,
    Enabled(bool),
    Mode(String),
}

impl StreamConfigSchema {
    pub fn to_mode(&self) -> crate::workflow::StreamMode {
        match self {
            StreamConfigSchema::None => crate::workflow::StreamMode::Default,
            StreamConfigSchema::Enabled(true) => crate::workflow::StreamMode::Passthrough,
            StreamConfigSchema::Enabled(false) => crate::workflow::StreamMode::Default,
            StreamConfigSchema::Mode(s) => {
                match s.to_lowercase().as_str() {
                    "passthrough" | "pass" | "stream" => crate::workflow::StreamMode::Passthrough,
                    _ => crate::workflow::StreamMode::Default,
                }
            }
        }
    }
}

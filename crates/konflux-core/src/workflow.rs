//! Workflow IR (Intermediate Representation)
//!
//! This module defines the data structures that represent a workflow.

use crate::error::ValidationError;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::time::Duration;

// ============================================================
// ID Newtypes
// ============================================================

macro_rules! define_id {
    ($name:ident, $doc:expr) => {
        #[doc = $doc]
        #[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            pub fn new(id: impl Into<String>) -> Self {
                Self(id.into())
            }

            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl From<&str> for $name {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }

        impl From<String> for $name {
            fn from(s: String) -> Self {
                Self(s)
            }
        }
    };
}

define_id!(WorkflowId, "Unique identifier for a workflow");
define_id!(StepId, "Unique identifier for a workflow step");
define_id!(ToolId, "Unique identifier for a tool");

// ============================================================
// WORKFLOW - The top-level container
// ============================================================

/// A workflow is a DAG of steps that execute tools
#[derive(Debug, Clone)]
pub struct Workflow {
    pub id: WorkflowId,
    pub name: String,
    pub version: String,
    pub entry: StepId,
    pub steps: Vec<Step>,
    pub capabilities: Vec<String>,
    pub metadata: HashMap<String, String>,
    /// Human-readable description (from YAML header). Used by WorkflowTool.
    pub description: Option<String>,
    /// JSON Schema for workflow input (from YAML header). Used by WorkflowTool.
    pub input_schema: Option<serde_json::Value>,
    /// JSON Schema for workflow output (from YAML header). Used by WorkflowTool.
    pub output_schema: Option<serde_json::Value>,
    /// Whether this workflow should register as a callable tool.
    pub register_as_tool: bool,
}

// ============================================================
// STEP - A single unit of work
// ============================================================

/// A single step in the workflow - calls one tool
#[derive(Debug, Clone)]
pub struct Step {
    pub id: StepId,
    pub tool: ToolId,

    /// Input arguments (key → expression that resolves from state)
    pub input: HashMap<String, Expr>,

    /// Outgoing edges (where to go next)
    pub edges: Vec<Edge>,

    /// Steps that must complete before this step runs
    pub depends_on: Vec<StepId>,

    /// How to handle waiting for dependencies
    pub join: JoinPolicy,

    /// What to do when this step fails
    pub on_error: ErrorAction,

    /// Retry policy for transient failures
    pub retry: Option<RetryPolicy>,

    /// Maximum time to wait for this step
    pub timeout: Option<Duration>,

    /// Credentials this step requires
    pub credentials: HashMap<String, String>,

    /// Grants for nested workflows
    pub grant: Option<Vec<String>>,

    /// Post-processing pipeline
    pub pipe: Vec<PipeStep>,

    /// Streaming mode for this step
    pub stream: StreamMode,

    /// Repeat configuration for bounded loops
    pub repeat: Option<RepeatConfig>,
}

// ============================================================
// EXPRESSIONS
// ============================================================

/// Expression for resolving input values from state
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum Expr {
    Literal(String),
    Ref(String),
    Template(String),
    Json(serde_json::Value),
}

// ============================================================
// EDGES
// ============================================================

#[derive(Debug, Clone)]
pub struct Edge {
    pub target: EdgeTarget,
    pub condition: Option<String>,
    pub priority: i32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EdgeTarget {
    Step(StepId),
    Return,
}

// ============================================================
// POLICIES & ENUMS
// ============================================================

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum JoinPolicy {
    #[default]
    All,
    Any,
    Quorum {
        min: u32,
    },
    Lenient,
}

#[derive(Debug, Clone, Default)]
pub enum ErrorAction {
    #[default]
    Fail,
    Skip,
    Fallback {
        value: String,
    },
    Goto {
        step: StepId,
    },
}

#[derive(Debug, Clone)]
pub struct RetryPolicy {
    pub max_attempts: u32,
    pub backoff: BackoffStrategy,
    pub base_delay: Duration,
    pub max_delay: Duration,
}

#[derive(Debug, Clone, Default)]
pub enum BackoffStrategy {
    Fixed,
    #[default]
    Exponential,
    Linear {
        increment: Duration,
    },
}

#[derive(Debug, Clone)]
pub struct PipeStep {
    pub tool: ToolId,
    pub input: HashMap<String, Expr>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum StreamMode {
    #[default]
    Default,
    Passthrough,
}

#[derive(Debug, Clone)]
pub struct RepeatConfig {
    pub until: String,
    pub max: u32,
    pub as_var: Option<String>,
}

// ============================================================
// IMPLEMENTATIONS
// ============================================================

impl Workflow {
    pub fn new(id: impl Into<String>, name: impl Into<String>, entry: impl Into<String>) -> Self {
        Self {
            id: WorkflowId::new(id),
            name: name.into(),
            version: "0.1.0".into(),
            entry: StepId::new(entry),
            steps: Vec::new(),
            capabilities: Vec::new(),
            metadata: HashMap::new(),
            description: None,
            input_schema: None,
            output_schema: None,
            register_as_tool: false,
        }
    }

    pub fn add_step(mut self, step: Step) -> Self {
        self.steps.push(step);
        self
    }

    pub fn with_step(self, step: Step) -> Self {
        self.add_step(step)
    }

    pub fn with_capabilities(mut self, capabilities: Vec<String>) -> Self {
        self.capabilities = capabilities;
        self
    }

    pub fn with_metadata(mut self, key: impl Into<String>, value: impl Into<String>) -> Self {
        self.metadata.insert(key.into(), value.into());
        self
    }

    pub fn get_step(&self, id: &StepId) -> Option<&Step> {
        self.steps.iter().find(|s| &s.id == id)
    }

    pub fn validate(&self) -> Result<(), ValidationError> {
        if self.get_step(&self.entry).is_none() {
            return Err(ValidationError::NoEntryNode);
        }

        let mut has_return = false;
        for step in &self.steps {
            for edge in &step.edges {
                match &edge.target {
                    EdgeTarget::Step(target_id) => {
                        if self.get_step(target_id).is_none() {
                            return Err(ValidationError::UnknownNode {
                                node: target_id.to_string(),
                                from: step.id.to_string(),
                            });
                        }
                    }
                    EdgeTarget::Return => {
                        has_return = true;
                    }
                }
            }

            for dep in &step.depends_on {
                if self.get_step(dep).is_none() {
                    return Err(ValidationError::UnknownNode {
                        node: dep.to_string(),
                        from: format!("depends_on:{}", step.id),
                    });
                }
            }
        }

        if !has_return {
            return Err(ValidationError::NoReturnNode);
        }

        self.detect_cycles()?;
        Ok(())
    }

    fn detect_cycles(&self) -> Result<(), ValidationError> {
        let mut adjacency: HashMap<&StepId, Vec<&StepId>> = HashMap::new();
        for step in &self.steps {
            let targets: Vec<&StepId> = step
                .edges
                .iter()
                .filter_map(|e| match &e.target {
                    EdgeTarget::Step(id) => Some(id),
                    EdgeTarget::Return => None,
                })
                .collect();
            adjacency.insert(&step.id, targets);
        }

        let mut color: HashMap<&StepId, Color> = HashMap::new();
        let mut path: Vec<StepId> = Vec::new();

        if self.dfs_detect_cycle(&self.entry, &adjacency, &mut color, &mut path) {
            return Err(ValidationError::CycleDetected {
                path: path.into_iter().map(|id| id.to_string()).collect(),
            });
        }

        for step in &self.steps {
            if !color.contains_key(&step.id)
                && self.dfs_detect_cycle(&step.id, &adjacency, &mut color, &mut path)
            {
                return Err(ValidationError::CycleDetected {
                    path: path.into_iter().map(|id| id.to_string()).collect(),
                });
            }
        }

        Ok(())
    }

    fn dfs_detect_cycle<'a>(
        &'a self,
        node: &'a StepId,
        adjacency: &HashMap<&'a StepId, Vec<&'a StepId>>,
        color: &mut HashMap<&'a StepId, Color>,
        path: &mut Vec<StepId>,
    ) -> bool {
        color.insert(node, Color::Gray);
        path.push(node.clone());

        if let Some(neighbors) = adjacency.get(node) {
            for neighbor in neighbors {
                match color.get(neighbor) {
                    Some(Color::Gray) => {
                        path.push((*neighbor).clone());
                        return true;
                    }
                    Some(Color::Black) => continue,
                    None => {
                        if self.dfs_detect_cycle(neighbor, adjacency, color, path) {
                            return true;
                        }
                    }
                }
            }
        }

        color.insert(node, Color::Black);
        path.pop();
        false
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Color {
    Gray,
    Black,
}

impl Step {
    pub fn new(id: impl Into<String>, tool: impl Into<String>) -> Self {
        Self {
            id: StepId::new(id),
            tool: ToolId::new(tool),
            input: HashMap::new(),
            edges: Vec::new(),
            depends_on: Vec::new(),
            join: JoinPolicy::default(),
            on_error: ErrorAction::default(),
            retry: None,
            timeout: None,
            credentials: HashMap::new(),
            grant: None,
            pipe: Vec::new(),
            stream: StreamMode::default(),
            repeat: None,
        }
    }

    pub fn with_input(mut self, key: impl Into<String>, expr: Expr) -> Self {
        self.input.insert(key.into(), expr);
        self
    }

    pub fn with_edge(mut self, target: EdgeTarget) -> Self {
        self.edges.push(Edge {
            target,
            condition: None,
            priority: 0,
        });
        self
    }

    pub fn with_depends_on(mut self, dep: impl Into<String>) -> Self {
        self.depends_on.push(StepId::new(dep));
        self
    }
}

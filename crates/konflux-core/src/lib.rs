//! Konflux — YAML Workflow Execution Engine
//!
//! A library for parsing and executing YAML-defined workflows.
//! Konflux dispatches tools, enforces capability lattices,
//! and streams results. It has no opinions about what tools do.

pub mod error;
pub mod workflow;
pub mod expr;
pub mod parser;
pub mod tool;
pub mod resource;
pub mod prompt;
pub mod executor;
pub mod stream;
pub mod engine;
pub mod capability;
pub mod template;
pub mod builtin;
pub mod hooks;

pub use engine::{Engine, EngineConfig};
pub use workflow::Workflow;
pub use tool::{Tool, ToolInfo, ToolAnnotations, ToolContext, ToolRegistry};
pub use resource::{Resource, ResourceInfo, ResourceRegistry, ResourceError, ResourceChanged};
pub use prompt::{Prompt, PromptInfo, PromptArgument, PromptRegistry, PromptError, Message};
pub use stream::{StreamEvent, ProgressType, StreamReceiver};
pub use error::KonfluxError;

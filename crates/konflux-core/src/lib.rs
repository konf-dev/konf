//! Konflux — YAML Workflow Execution Engine
//!
//! A library for parsing and executing YAML-defined workflows.
//! Konflux dispatches tools, enforces capability lattices,
//! and streams results. It has no opinions about what tools do.

pub mod builtin;
pub mod capability;
pub mod engine;
pub mod error;
pub mod executor;
pub mod expr;
pub mod hooks;
pub mod parser;
pub mod prompt;
pub mod resource;
pub mod stream;
pub mod template;
pub mod tool;
pub mod workflow;

pub use engine::{Engine, EngineConfig, ToolChangedReceiver};
pub use error::KonfluxError;
pub use prompt::{Message, Prompt, PromptArgument, PromptError, PromptInfo, PromptRegistry};
pub use resource::{Resource, ResourceChanged, ResourceError, ResourceInfo, ResourceRegistry};
pub use stream::{ProgressType, StreamEvent, StreamReceiver};
pub use tool::{Tool, ToolAnnotations, ToolContext, ToolInfo, ToolRegistry};
pub use workflow::Workflow;

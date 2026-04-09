//! konf-runtime — OS-like workflow management for the Konf AI platform.
//!
//! Wraps the konflux workflow engine with process lifecycle management,
//! capability-based permission routing, monitoring, and event journaling.
//!
//! # Architecture
//!
//! ```text
//! Runtime
//! ├── ProcessTable (papaya — concurrent, lock-free)
//! ├── Engine (konflux — workflow execution)
//! ├── EventJournal (sqlx → Postgres)
//! └── ResourceLimits (configurable)
//! ```
//!
//! # Key concepts
//!
//! - **ExecutionScope**: namespace + capabilities + resource limits per workflow run
//! - **CapabilityGrant**: parameterized tool access with namespace injection
//! - **VirtualizedTool**: wraps tools to inject bound parameters (e.g., namespace)
//! - **ProcessTable**: in-memory map of active workflow runs
//! - **EventJournal**: append-only Postgres log for audit and monitoring

pub mod context;
pub mod error;
pub mod guard;
pub mod hooks;
pub mod journal;
pub mod monitor;
pub mod process;
pub mod runtime;
pub mod scope;
pub mod workflow_tool;

pub use context::VirtualizedTool;
pub use error::{RunId, RuntimeError};
pub use guard::{DefaultAction, GuardedTool, Predicate, Rule};
pub use monitor::{ProcessTree, RunDetail, RunSummary, RuntimeMetrics};
pub use process::{ActiveNode, NodeStatus, ProcessTable, RunStatus, WorkflowRun};
pub use runtime::Runtime;
pub use scope::{
    dev_scope, scope_from_role, Actor, CapabilityGrant, ExecutionScope, ResourceLimits,
};
pub use workflow_tool::WorkflowTool;

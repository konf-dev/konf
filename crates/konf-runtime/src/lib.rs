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

pub mod error;
pub mod process;
pub mod scope;
pub mod context;
pub mod guard;
pub mod hooks;
pub mod journal;
pub mod monitor;
pub mod runtime;
pub mod workflow_tool;

pub use error::{RuntimeError, RunId};
pub use process::{WorkflowRun, RunStatus, ActiveNode, NodeStatus, ProcessTable};
pub use scope::{ExecutionScope, CapabilityGrant, ResourceLimits, Actor, scope_from_role, dev_scope};
pub use context::VirtualizedTool;
pub use guard::{GuardedTool, Rule, Predicate, DefaultAction};
pub use monitor::{RunSummary, RunDetail, ProcessTree, RuntimeMetrics};
pub use runtime::Runtime;
pub use workflow_tool::WorkflowTool;

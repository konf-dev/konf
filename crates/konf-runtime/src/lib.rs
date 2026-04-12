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
//! ├── JournalStore (trait, default impl backed by redb)
//! └── ResourceLimits (configurable)
//!
//! KonfStorage (owns one redb::Database)
//! ├── Journal (RedbJournal — audit log)
//! ├── Scheduler  [phase 2]
//! └── Runner Intents  [phase 3]
//! ```
//!
//! # Key concepts
//!
//! - **ExecutionScope**: namespace + capabilities + resource limits per workflow run
//! - **CapabilityGrant**: parameterized tool access with namespace injection
//! - **VirtualizedTool**: wraps tools to inject bound parameters (e.g., namespace)
//! - **ProcessTable**: in-memory map of active workflow runs
//! - **JournalStore**: append-only audit log trait, default impl is redb

pub mod context;
pub mod error;
pub mod event_bus;
pub mod guard;
pub mod hooks;
pub mod journal;
pub mod monitor;
pub mod process;
pub mod runner_intents;
pub mod runtime;
pub mod scheduler;
pub mod scope;
pub mod storage;
pub mod workflow_tool;

pub use context::VirtualizedTool;
pub use error::{RunId, RuntimeError};
pub use event_bus::{RunEvent, RunEventBus, DEFAULT_EVENT_BUS_CAPACITY};
pub use guard::{DefaultAction, GuardedTool, Predicate, Rule};
pub use journal::{JournalEntry, JournalError, JournalRow, JournalStore, RedbJournal};
pub use monitor::{ProcessTree, RunDetail, RunSummary, RuntimeMetrics};
pub use process::{ActiveNode, NodeStatus, ProcessTable, RunStatus, WorkflowRun};
pub use runner_intents::{IntentError, IntentId, RunnerIntent, RunnerIntentStore, TerminalStatus};
pub use runtime::Runtime;
pub use scheduler::{
    new_record as new_timer_record, JobId, JobSummary, RedbScheduler, SchedulerError,
    TimerMode, TimerRecord, MAX_FIXED_DELAY_MS, MIN_FIXED_DELAY_MS,
};
pub use scope::{
    dev_scope, scope_from_role, Actor, CapabilityGrant, ExecutionScope, ResourceLimits,
};
pub use storage::{KonfStorage, Retention, StorageError};
pub use workflow_tool::WorkflowTool;

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
//! ├── RunEventBus (tokio broadcast — real-time observability)
//! ├── JournalStore (trait, default redb; optionally wrapped in Fanout)
//! │   └── FanoutJournalStore (primary redb + optional secondaries)
//! └── ResourceLimits (configurable)
//!
//! KonfStorage (owns one redb::Database)
//! ├── Journal (RedbJournal — audit log; carries Interactions)
//! ├── Scheduler
//! └── Runner Intents
//! ```
//!
//! # Key concepts
//!
//! - **ExecutionScope**: namespace + capabilities + resource limits + optional
//!   `trace_id` per workflow run. Attenuates on `child_scope`, never amplifies.
//! - **Interaction**: typed envelope for one edge-traversal (tool dispatch,
//!   workflow node lifecycle, run lifecycle, user input, LLM response, error).
//!   OpenTelemetry-aligned naming. Serialized into `JournalEntry.payload`.
//! - **CapabilityGrant**: parameterized tool access with namespace injection
//! - **VirtualizedTool**: wraps tools to inject bound parameters (e.g., namespace)
//! - **ProcessTable**: in-memory map of active workflow runs
//! - **JournalStore**: append-only audit log trait. Default impl is redb.
//!   `FanoutJournalStore` composes one primary + N secondaries with failure
//!   isolation (primary-succeeds-only acknowledgment; secondary drops tracked
//!   via `FanoutMetrics`).

pub mod bisimulation;
pub mod context;
pub(crate) mod dispatcher;
pub mod error;
pub mod event_bus;
pub mod execution_context;
pub mod guard;
pub mod hooks;
pub mod interaction;
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
pub use execution_context::ExecutionContext;
pub use guard::{DefaultAction, GuardedTool, Predicate, Rule};
pub use interaction::{Interaction, InteractionKind, InteractionStatus};
pub use journal::{
    FanoutJournalStore, FanoutMetrics, JournalEntry, JournalError, JournalRow, JournalStore,
    RedbJournal,
};
pub use monitor::{ProcessTree, RunDetail, RunSummary, RuntimeMetrics};
pub use process::{ActiveNode, NodeStatus, ProcessTable, RunStatus, WorkflowRun};
pub use runner_intents::{IntentError, IntentId, RunnerIntent, RunnerIntentStore, TerminalStatus};
pub use runtime::Runtime;
pub use scheduler::{
    new_record as new_timer_record, JobId, JobSummary, RedbScheduler, SchedulerError, TimerMode,
    TimerRecord, MAX_FIXED_DELAY_MS, MIN_FIXED_DELAY_MS,
};
pub use scope::{
    dev_scope, scope_from_role, Actor, CapabilityGrant, ExecutionScope, ResourceLimits,
};
pub use storage::{KonfStorage, Retention, StorageError};
pub use workflow_tool::WorkflowTool;

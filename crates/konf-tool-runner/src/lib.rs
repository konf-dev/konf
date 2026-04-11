//! Runner tool family for konf.
//!
//! A runner is a thing that can **start a workflow run, track it, and let
//! callers ask about it afterwards**. The shape is deliberately similar to
//! `schedule` (konf-init/src/schedule.rs): a tool that creates an
//! asynchronous workflow run and returns a handle, with companion tools to
//! check status, block until completion, and cancel.
//!
//! ## Why a tool family, not a kernel primitive
//!
//! Finding 014 (`konf-experiments/findings/014-only-one-new-rust-tool.md`)
//! said the only new kernel addition needed for autonomous behavior was the
//! `schedule` tool — and schedule is itself a tool, not a trait in
//! `konf-runtime`. The runner is the same shape at a different axis:
//! schedule handles "when", the runner handles "where / how isolated". Both
//! can be expressed as tools over the existing engine without growing the
//! kernel.
//!
//! The `Runner` trait therefore lives **inside** this crate, not in
//! `konf-runtime`. Callers never see the trait — they see the five tools.
//!
//! ## v1 scope
//!
//! - `InlineRunner`: runs each spawned workflow as a plain tokio task inside
//!   the same process, against the same `Runtime`. No isolation, no resource
//!   limits, no process boundary. It is the floor that lets workflows
//!   compose and fan out; later runners can add isolation.
//! - Tools shipped: `runner:spawn`, `runner:status`, `runner:wait`,
//!   `runner:cancel`. Streaming logs are deferred to v2.
//! - `SystemdRunner` and `DockerRunner` are not implemented yet. Their
//!   shape is documented in plan `serene-tumbling-gizmo` Phase 3 and they
//!   will land as independent commits against the same `Runner` trait.
#![warn(missing_docs)]

mod error;
mod registry;
mod runner;
mod runners;
mod tools;

pub use error::RunnerError;
pub use registry::{RunId, RunRecord, RunRegistry, RunState};
pub use runner::{Runner, WorkflowSpec};
pub use runners::inline::InlineRunner;
pub use tools::register;

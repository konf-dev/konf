//! Interaction envelope — the uniform storage primitive of the Stigmergic Engine.
//!
//! An [`Interaction`] is a typed view over a [`crate::journal::JournalEntry`]
//! payload. Every edge-traversal in the system (tool dispatch, node lifecycle,
//! run lifecycle, user input, LLM response, error) produces one Interaction
//! record that is appended to the journal and fanned out to any configured
//! secondary backends (e.g. SurrealDB's `event` table for long-term queryable
//! graph storage).
//!
//! # Name alignment with OpenTelemetry
//!
//! Field names are chosen to map 1:1 onto OpenTelemetry span fields where
//! possible. This preserves an explicit future path for exporting interactions
//! to Jaeger / Tempo / Honeycomb via an OTLP exporter.
//!
//! | This crate        | OTel span field     |
//! |-------------------|---------------------|
//! | [`Interaction::id`]         | `span_id`           |
//! | [`Interaction::parent_id`]  | `parent_span_id`    |
//! | [`Interaction::trace_id`]   | `trace_id`          |
//! | [`Interaction::target`]     | `name`              |
//! | [`Interaction::timestamp`]  | `start_time`        |
//! | [`Interaction::status`]     | `status.code`       |
//!
//! # Multi-tenant invariants
//!
//! The following fields are always set by the **substrate** (not by actor
//! input) and must never be elided; they preserve per-record self-auditability
//! in multi-tenant deployments:
//!
//! - [`Interaction::actor`] — who emitted (id + role)
//! - [`Interaction::namespace`] — which tenant namespace
//! - [`Interaction::edge_rules_fired`] — which capability / guard checks ran
//!
//! # Schema doc
//!
//! See `konf-genesis/docs/INTERACTION_SCHEMA.md` for the full taxonomy of
//! [`InteractionKind`] variants and per-kind attribute conventions.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::RunId;
use crate::scope::Actor;

/// One edge-traversal in the system.
///
/// Serializable via serde to/from JSON for storage in
/// [`crate::journal::JournalEntry::payload`].
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Interaction {
    /// Unique id for this interaction. OTel analog: `span_id`.
    pub id: Uuid,

    /// Direct causal ancestor dispatch. `None` at the root of a trace.
    /// OTel analog: `parent_span_id`.
    pub parent_id: Option<Uuid>,

    /// Groups related interactions across spawn boundaries.
    /// Inherited from caller when set; minted fresh when a new top-level turn
    /// or run begins without an enclosing trace.
    /// OTel analog: `trace_id`.
    pub trace_id: Uuid,

    /// Workflow run id if this interaction occurred inside a
    /// [`crate::Runtime::start_workflow`] invocation.
    pub run_id: Option<RunId>,

    /// Workflow node id if applicable.
    pub node_id: Option<String>,

    /// Who emitted. Inline for multi-tenant self-auditability.
    pub actor: Actor,

    /// Tenant namespace. Inline for multi-tenant self-auditability.
    pub namespace: String,

    /// What was invoked. Follows a prefix convention
    /// (`tool:`, `node:`, `run:`, `input:`, `llm:`, `error:`, or
    /// `product:<name>:`). See INTERACTION_SCHEMA.md §target conventions.
    pub target: String,

    /// Bounded taxonomy; see [`InteractionKind`] variants.
    pub kind: InteractionKind,

    /// Kind-specific structured data. Conventions documented per-variant in
    /// INTERACTION_SCHEMA.md.
    pub attributes: serde_json::Value,

    /// Which capability / guard checks fired at this edge. Populated by the
    /// substrate when dispatching through [`crate::context::VirtualizedTool`]
    /// or [`crate::guard::GuardedTool`]. Inline for audit purposes.
    pub edge_rules_fired: Vec<String>,

    /// Terminal / non-terminal outcome. See [`InteractionStatus`].
    pub status: InteractionStatus,

    /// One-line synopsis for bird's-eye queries. Optional; populated by
    /// product workflows (e.g. LLM self-reports). The substrate never
    /// fabricates summaries.
    pub summary: Option<String>,

    /// Emit time (monotonic within a trace on a single host; best-effort
    /// across hosts via system clock).
    pub timestamp: DateTime<Utc>,

    /// Monotonic per-trace ordinal. Propagated from Envelope.step_index.
    #[serde(default)]
    pub step_index: u64,

    /// Channel disambiguation for parallel calls. Propagated from Envelope.stream_id.
    #[serde(default)]
    pub stream_id: String,

    /// SHA-256 hash of the actor's observable state BEFORE this dispatch.
    /// None for stateless tools or tools without StateProjection.
    #[serde(default)]
    pub state_before_hash: Option<[u8; 32]>,

    /// SHA-256 hash of the actor's observable state AFTER this dispatch.
    #[serde(default)]
    pub state_after_hash: Option<[u8; 32]>,

    /// Non-parent semantic antecedents (e.g. "this response was informed by these prior interactions").
    #[serde(default)]
    pub references: Vec<Uuid>,

    /// Request/reply correlation. Points to the interaction this is responding to.
    #[serde(default)]
    pub in_reply_to: Option<Uuid>,
}

/// Bounded taxonomy of interaction kinds.
///
/// The substrate recorder emits one of the first six variants. Products emit
/// [`InteractionKind::ProductDefined`] explicitly by writing to the memory
/// tool — the substrate recorder never emits `ProductDefined`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractionKind {
    /// A single tool dispatch through [`crate::Runtime::invoke_tool`].
    ToolDispatch,

    /// Workflow node lifecycle (start / end / failed); discriminate by
    /// [`Interaction::status`].
    NodeLifecycle,

    /// Workflow run lifecycle (started / completed / failed / cancelled).
    RunLifecycle,

    /// An error that was not caught by the normal status=Failed path (e.g. a
    /// panic caught by `tokio::JoinError`).
    Error,

    /// A human or external-system input crossing into the tenant boundary.
    UserInput,

    /// A completion from an LLM tool. Distinct from [`InteractionKind::ToolDispatch`]
    /// so bird's-eye queries can cheaply filter "what did the LLMs say" without
    /// parsing attributes.
    LlmResponse,

    /// Escape hatch for product-level kinds written via memory tools.
    /// Convention: use a stable string id; version with a suffix if the
    /// payload schema changes (e.g. `"observation"`, `"observation.v2"`).
    ///
    /// Serialized as `{"type": "product_defined", "name": "<id>"}`.
    ProductDefined { name: String },
}

/// Terminal / non-terminal outcome of an interaction.
///
/// Maps onto OpenTelemetry span status codes:
/// - [`InteractionStatus::Pending`] → OTel `UNSET`
/// - [`InteractionStatus::Ok`] / [`InteractionStatus::Observed`] → OTel `OK`
/// - [`InteractionStatus::Failed`] → OTel `ERROR`
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum InteractionStatus {
    /// Emitted at start; not yet terminal.
    Pending,
    /// Terminal success.
    Ok,
    /// Terminal failure with a reason.
    Failed { error: String },
    /// Inherently terminal at emit time (UserInput, LlmResponse — these are
    /// observations, not calls that can be pending).
    Observed,
}

impl Interaction {
    /// Serialize this interaction to a JSON value suitable for storage in
    /// [`crate::journal::JournalEntry::payload`]. Infallible: the type's
    /// structure is serde-compatible by construction.
    pub fn to_json(&self) -> serde_json::Value {
        // serde_json::to_value cannot fail for the types we derive; if it
        // does, our derive is broken and a panic is the right behavior in
        // a test. Production code should not call this on malformed input.
        serde_json::to_value(self).expect("Interaction serde derive is infallible")
    }

    /// Deserialize an interaction from a JSON value. Returns `Err` if the
    /// payload does not match the schema.
    pub fn from_json(value: serde_json::Value) -> Result<Self, serde_json::Error> {
        serde_json::from_value(value)
    }
}

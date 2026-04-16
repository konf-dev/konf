//! Typed envelope — the substrate's unit of cross-actor interaction.
//!
//! Every dispatch goes through an `Envelope<P>`. The substrate enforces:
//! - Capability-checked authority (`capabilities` ⊆ parent)
//! - Causal propagation (`trace_id`, `parent_id`, `namespace`)
//! - Journaling (every envelope is recorded)
//!
//! V2 shape from `RFC_ENVELOPE.md`. Unused in this checkpoint (4.b);
//! wired into dispatch at 4.e–4.f.

use std::collections::BTreeMap;
use std::fmt;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

// ── Identity + causality ────────────────────────────────────────────

/// Unique identifier for a single dispatch (span_id analog).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct EnvelopeId(pub Uuid);

/// Groups a causal chain of dispatches (OTel trace grouping).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TraceId(pub Uuid);

// ── Actor + tenancy ─────────────────────────────────────────────────

/// Opaque actor identity. Substrate does not interpret; runtime resolves
/// via an actor table (Decision 1: option b).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct ActorId(pub String);

/// Opaque namespace. Substrate partitions on it but does not interpret
/// the string's meaning.
///
/// Invariant: `child.namespace == parent.namespace` unless an explicit
/// `ns_override` capability is held.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Namespace(pub String);

// ── Dispatch ────────────────────────────────────────────────────────

/// Routing key — which tool/workflow to dispatch to (e.g. "memory:search").
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct TargetId(pub String);

/// Typed dispatch key, distinct from `TargetId`. Discriminates payload
/// shape without conflating it with routing (amendment 3).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PayloadType(pub String);

/// Channel disambiguation for concurrent parallel calls (ITM multi-stream).
/// Explicit — no default (amendment 3).
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct StreamId(pub String);

// ── Authority ───────────────────────────────────────────────────────

/// A single capability grant. String-backed in V2; typed handle in Stage 6.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct Capability(pub String);

/// Set of capabilities carried by an envelope.
///
/// `Vec<Capability>` in V2 (amendment 4); typed-handle table in Stage 6.
///
/// Invariant: `child.capabilities ⊆ parent.capabilities`.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapSet(pub Vec<Capability>);

// ── Execution control ───────────────────────────────────────────────

/// Opaque idempotency key for dedupe window lookup.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct IdempotencyKey(pub String);

// ── Extension ───────────────────────────────────────────────────────

/// Typed metadata map. `serde_json::Value` values in V2 (amendment 6);
/// sealed enum in Stage 9.
///
/// Keys declared with a `MetadataPropagation` policy; substrate enforces
/// the declared policy on child envelope creation.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Metadata(pub BTreeMap<String, serde_json::Value>);

/// Propagation policy for a metadata key.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum MetadataPropagation {
    /// Copied to direct child envelope.
    Inherit,
    /// Stays on this envelope only.
    Local,
    /// Copied to all descendants (transitive).
    Ambient,
}

// ── The Envelope ────────────────────────────────────────────────────

/// V2 typed envelope — the substrate's unit of cross-actor interaction.
///
/// Single-ownership: moved across dispatch, not shared. Rust's affine
/// types enforce FBP-style single-ownership by construction.
///
/// # RFC deviations
///
/// - `deadline` uses `DateTime<Utc>` instead of `Instant`. `Instant` is
///   process-relative and not serializable; both Temporal and Restate use
///   absolute wall-clock deadlines. Needs RFC amendment.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Envelope<P> {
    // ── Identity + causality ──
    pub id: EnvelopeId,
    pub trace_id: TraceId,
    pub parent_id: Option<EnvelopeId>,

    // ── Actor + tenancy ──
    pub actor_id: ActorId,
    pub namespace: Namespace,

    // ── Authority ──
    pub capabilities: CapSet,

    // ── Dispatch ──
    pub target: TargetId,
    pub payload_type: PayloadType,
    pub payload: P,

    // ── Observability ──
    pub emitted_at: DateTime<Utc>,

    // ── V2: ITM stream identity + PTM ordinal ──
    /// Monotonic per trace; orders concurrent branches.
    pub step_index: u64,
    /// Channel disambiguation; explicit, no default (amendment 3).
    pub stream_id: StreamId,

    // ── V2: Akl rate discipline ──
    /// Child deadline ≤ parent deadline (substrate-enforced).
    pub deadline: Option<DateTime<Utc>>,
    /// Dedupe window lookup key.
    pub idempotency_key: Option<IdempotencyKey>,

    // ── V2: Stage 8 forward-compat reservation ──
    /// Semantic antecedents; `None` in V2, populated in Stage 8 (amendment 8).
    pub references: Option<Vec<EnvelopeId>>,

    // ── V2: Extension point ──
    /// Typed metadata map; propagation rules per-key.
    pub metadata: Metadata,
}

// ── Envelope methods ────────────────────────────────────────────────

impl<P> Envelope<P> {
    /// Create a response envelope from this envelope, replacing the payload.
    ///
    /// Propagates: trace_id, namespace, capabilities, stream_id, deadline,
    /// metadata (per propagation rules — for V2, cloned wholesale).
    /// Sets: new id, parent_id = self.id, step_index + 1, emitted_at = now.
    /// Clears: idempotency_key, references.
    pub fn respond<Q>(&self, payload: Q) -> Envelope<Q> {
        Envelope {
            id: EnvelopeId(Uuid::new_v4()),
            trace_id: self.trace_id,
            parent_id: Some(self.id),
            actor_id: self.actor_id.clone(),
            namespace: self.namespace.clone(),
            capabilities: self.capabilities.clone(),
            target: self.target.clone(),
            payload_type: self.payload_type.clone(),
            payload,
            emitted_at: Utc::now(),
            step_index: self.step_index + 1,
            stream_id: self.stream_id.clone(),
            deadline: self.deadline,
            idempotency_key: None,
            references: None,
            metadata: self.metadata.clone(),
        }
    }
}

impl Envelope<serde_json::Value> {
    /// Construct a tool-dispatch envelope from available context.
    ///
    /// Used by the executor and runtime to bridge existing context into
    /// a typed Envelope for the Tool trait. Transitional — will be
    /// replaced by proper Envelope flow in 4.f/4.g.
    pub fn for_tool_dispatch(
        target: &str,
        payload: serde_json::Value,
        capabilities: &[String],
        trace_id: Uuid,
        namespace: &str,
        actor_id: &str,
        stream_id: &str,
    ) -> Self {
        Envelope {
            id: EnvelopeId(Uuid::new_v4()),
            trace_id: TraceId(trace_id),
            parent_id: None,
            actor_id: ActorId(actor_id.to_string()),
            namespace: Namespace(namespace.to_string()),
            capabilities: CapSet(capabilities.iter().map(|c| Capability(c.clone())).collect()),
            target: TargetId(target.to_string()),
            payload_type: PayloadType("tool_input".to_string()),
            payload,
            emitted_at: Utc::now(),
            step_index: 0,
            stream_id: StreamId(stream_id.to_string()),
            deadline: None,
            idempotency_key: None,
            references: None,
            metadata: Metadata::default(),
        }
    }

    /// Construct a minimal test envelope. Intended for `#[cfg(test)]` only.
    pub fn test(payload: serde_json::Value) -> Self {
        Envelope {
            id: EnvelopeId(Uuid::new_v4()),
            trace_id: TraceId(Uuid::new_v4()),
            parent_id: None,
            actor_id: ActorId("test".to_string()),
            namespace: Namespace("test".to_string()),
            capabilities: CapSet(vec![Capability("*".to_string())]),
            target: TargetId("test".to_string()),
            payload_type: PayloadType("test".to_string()),
            payload,
            emitted_at: Utc::now(),
            step_index: 0,
            stream_id: StreamId("test".to_string()),
            deadline: None,
            idempotency_key: None,
            references: None,
            metadata: Metadata::default(),
        }
    }
}

impl CapSet {
    /// Get the capability patterns as a `Vec<String>` for bridging
    /// with legacy code that expects `Vec<String>`.
    pub fn to_patterns(&self) -> Vec<String> {
        self.0.iter().map(|c| c.0.clone()).collect()
    }
}

// ── Display impls for newtypes (logging) ────────────────────────────

impl fmt::Display for EnvelopeId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for TraceId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl fmt::Display for ActorId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for Namespace {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for TargetId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

impl fmt::Display for StreamId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

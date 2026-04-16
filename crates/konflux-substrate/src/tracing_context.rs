//! Causal tracing context — the subset of Envelope identity needed for
//! trace propagation without the full envelope.
//!
//! Used when code needs to establish or extend a causal chain but doesn't
//! have (or need) the full `Envelope<P>`.

use serde::{Deserialize, Serialize};

use crate::envelope::{EnvelopeId, TraceId};

/// Lightweight causal context for trace propagation.
///
/// When a parent dispatch spawns a child, the substrate creates a new
/// `EnvelopeId` but inherits the parent's `TraceId`. This struct carries
/// just enough to establish the causal chain without dragging in the
/// full envelope's generic parameter.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TracingContext {
    /// Inherited from the root of the causal chain.
    pub trace_id: TraceId,
    /// Direct dispatch ancestor (`None` for root dispatches).
    pub parent_id: Option<EnvelopeId>,
}

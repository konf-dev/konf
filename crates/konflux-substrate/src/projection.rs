//! State projection — mechanism for recording actor observable state.
//!
//! Tools that implement [`StateProjection`] declare what bits of their
//! internal state constitute the observable worktape. The substrate
//! hashes the projection and records it as `state_before_hash` /
//! `state_after_hash` on Interaction records, enabling bisimulation
//! (Wegner PTM equivalence checking).

use sha2::{Digest, Sha256};

/// Canonical bytes representing an actor's observable state at a moment.
///
/// The substrate does not interpret the bytes — it only hashes them.
/// Each actor kind defines what goes into its projection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Projection(Vec<u8>);

impl Projection {
    /// Create a projection from raw bytes.
    pub fn new(bytes: Vec<u8>) -> Self {
        Self(bytes)
    }

    /// Empty projection (stateless actors).
    pub fn empty() -> Self {
        Self(Vec::new())
    }

    /// SHA-256 hash of this projection's bytes.
    pub fn hash(&self) -> [u8; 32] {
        let mut hasher = Sha256::new();
        hasher.update(&self.0);
        hasher.finalize().into()
    }

    /// Whether this projection has no data.
    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    /// Access the raw bytes.
    pub fn as_bytes(&self) -> &[u8] {
        &self.0
    }
}

/// Trait for actors that can project their observable state.
///
/// Implementations must be deterministic: the same observable environment
/// must always produce the same projection bytes.
///
/// Lives in the substrate; per-kind implementations live in runtime or
/// tool crates.
pub trait StateProjection: Send + Sync {
    /// Project the current observable state. Returns `None` if the actor
    /// has no meaningful state to project (stateless tools).
    fn project(&self) -> Option<Projection>;
}

//! Fan-out journal — writes each [`JournalEntry`] to a primary store plus
//! zero or more secondary stores with failure isolation.
//!
//! # Semantics
//!
//! - **Primary**: write is awaited synchronously. A primary failure surfaces
//!   as an error from [`FanoutJournalStore::append`], guaranteeing the
//!   caller that audit integrity is preserved.
//! - **Secondaries**: writes are spawned onto the tokio runtime
//!   fire-and-forget. Failures are logged via `tracing::warn!` and counted
//!   via [`FanoutMetrics::dropped_secondary_writes`]. They MUST NOT cause
//!   [`append`] to fail, block, or propagate.
//! - **Query methods**: delegated to the primary only. Secondaries are
//!   write-side replicas (e.g., a long-term SurrealDB `event` table mirror
//!   of a short-retention redb journal); the primary remains the source of
//!   truth for audit queries.
//!
//! # Why this lives in `konf-runtime`
//!
//! This is substrate: it composes implementations of the pre-existing
//! [`JournalStore`] trait without introducing a new trait of its own.
//! konf doctrine #1 ("new Rust must be impossible to express as a workflow
//! using existing primitives") admits this as the minimum addition required
//! to run two journal backends in parallel.
//!
//! The concrete secondary impl for SurrealDB lives in
//! `konf-tool-memory-surreal` to avoid a runtime → storage-backend
//! dependency loop.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use async_trait::async_trait;

use crate::journal::RunId;
use crate::journal::{JournalEntry, JournalError, JournalRow, JournalStore};

/// Counters exposed to observability. All atomics use `Ordering::Relaxed`
/// because counters are read at observation time; no cross-counter
/// invariants are claimed.
#[derive(Debug, Default)]
pub struct FanoutMetrics {
    /// Number of secondary append tasks that completed with an error.
    ///
    /// This counter is best-effort: if a spawned task panics before
    /// incrementing it, the drop will be missed. A panic hook could be
    /// added later if that becomes load-bearing.
    pub dropped_secondary_writes: AtomicU64,
}

impl FanoutMetrics {
    /// Current count of dropped secondary writes.
    pub fn dropped_secondary_writes(&self) -> u64 {
        self.dropped_secondary_writes.load(Ordering::Relaxed)
    }
}

/// Composes one primary [`JournalStore`] with zero or more secondaries.
///
/// ```ignore
/// let fanout = FanoutJournalStore::new(
///     Arc::new(RedbJournal::open(...)?),
///     vec![Arc::new(SurrealJournalStore::new(db))],
/// );
/// runtime_builder.journal(Arc::new(fanout));
/// ```
pub struct FanoutJournalStore {
    primary: Arc<dyn JournalStore>,
    secondaries: Vec<Arc<dyn JournalStore>>,
    metrics: Arc<FanoutMetrics>,
}

impl FanoutJournalStore {
    /// Construct a new fan-out store. `secondaries` may be empty; in that
    /// case the store behaves identically to its primary.
    pub fn new(primary: Arc<dyn JournalStore>, secondaries: Vec<Arc<dyn JournalStore>>) -> Self {
        Self {
            primary,
            secondaries,
            metrics: Arc::new(FanoutMetrics::default()),
        }
    }

    /// Shared handle to the metrics counters. Safe to clone and inspect
    /// from any thread.
    pub fn metrics(&self) -> Arc<FanoutMetrics> {
        self.metrics.clone()
    }
}

#[async_trait]
impl JournalStore for FanoutJournalStore {
    async fn append(&self, entry: JournalEntry) -> Result<u64, JournalError> {
        let seq = self.primary.append(entry.clone()).await?;
        for secondary in &self.secondaries {
            let secondary = secondary.clone();
            let entry = entry.clone();
            let metrics = self.metrics.clone();
            tokio::spawn(async move {
                if let Err(e) = secondary.append(entry).await {
                    tracing::warn!(error = %e, "secondary journal write failed");
                    metrics
                        .dropped_secondary_writes
                        .fetch_add(1, Ordering::Relaxed);
                }
            });
        }
        Ok(seq)
    }

    async fn query_by_run(&self, run_id: RunId) -> Result<Vec<JournalRow>, JournalError> {
        self.primary.query_by_run(run_id).await
    }

    async fn query_by_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<JournalRow>, JournalError> {
        self.primary.query_by_session(session_id, limit).await
    }

    async fn recent(&self, limit: usize) -> Result<Vec<JournalRow>, JournalError> {
        self.primary.recent(limit).await
    }

    async fn reconcile_zombies(&self) -> Result<u64, JournalError> {
        self.primary.reconcile_zombies().await
    }
}

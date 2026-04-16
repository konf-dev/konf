//! TTL sweep — background task that physically deletes expired journal entries.

use std::sync::Arc;
use std::time::Duration;

use tokio::task::JoinHandle;
use tracing::{debug, warn};

use super::JournalStore;

/// Background sweeper that periodically calls `delete_expired()` on the
/// journal store. Owned by the runtime; stopped on drop via `abort()`.
pub struct TtlSweeper {
    handle: JoinHandle<()>,
}

impl TtlSweeper {
    /// Spawn the sweeper. It runs `delete_expired()` every `interval`,
    /// logging results and swallowing errors.
    pub fn spawn(store: Arc<dyn JournalStore>, interval: Duration) -> Self {
        let handle = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                match store.delete_expired().await {
                    Ok(0) => {}
                    Ok(n) => debug!(deleted = n, "TTL sweep completed"),
                    Err(e) => warn!(error = %e, "TTL sweep failed"),
                }
            }
        });
        Self { handle }
    }

    /// Stop the sweeper.
    pub fn stop(&self) {
        self.handle.abort();
    }
}

impl Drop for TtlSweeper {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

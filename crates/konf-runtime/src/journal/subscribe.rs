//! Journal subscription — replay from journal + live delivery via event bus.

use std::sync::Arc;

use tokio::sync::mpsc;
use tracing::{debug, warn};

use super::{JournalFilter, JournalRow, JournalStore};
use crate::event_bus::{RunEvent, RunEventBus};

/// A live subscription to journal entries matching a filter.
///
/// On creation, replays existing entries from the journal (up to `replay_limit`),
/// then bridges new entries via the `RunEventBus` `JournalAppended` events.
pub struct JournalSubscription;

impl JournalSubscription {
    /// Start a subscription. Returns an mpsc receiver that yields matching
    /// `JournalRow` values. The subscription runs in a background task and
    /// stops when the receiver is dropped.
    ///
    /// Flow:
    /// 1. Replay: query the journal with the filter, send all matches.
    /// 2. Live: listen on the event bus for `JournalAppended`, look up the
    ///    entry by sequence, filter, and forward matches.
    pub fn start(
        store: Arc<dyn JournalStore>,
        event_bus: &RunEventBus,
        filter: JournalFilter,
        replay_limit: usize,
    ) -> mpsc::Receiver<JournalRow> {
        let (tx, rx) = mpsc::channel(256);
        let mut bus_rx = event_bus.subscribe();

        tokio::spawn(async move {
            // Phase 1: replay
            match store.query(&filter, replay_limit).await {
                Ok(rows) => {
                    for row in rows {
                        if tx.send(row).await.is_err() {
                            return; // receiver dropped
                        }
                    }
                }
                Err(e) => {
                    warn!(error = %e, "Journal subscription replay failed");
                }
            }

            debug!("Journal subscription replay complete, switching to live");

            // Phase 2: live — bridge JournalAppended events into journal lookups
            loop {
                match bus_rx.recv().await {
                    Ok(RunEvent::JournalAppended {
                        sequence,
                        event_type,
                        namespace,
                        ..
                    }) => {
                        // Quick pre-filter on fields available in the event
                        if let Some(ref fns) = filter.namespace {
                            if *fns != namespace {
                                continue;
                            }
                        }
                        if let Some(ref fet) = filter.event_type {
                            if *fet != event_type {
                                continue;
                            }
                        }

                        // Look up the full entry by querying recent entries
                        // and finding the one with matching sequence.
                        // (A direct get-by-id would be better but JournalStore
                        // doesn't have that method yet.)
                        match store.recent(1).await {
                            Ok(rows) => {
                                for row in rows {
                                    if row.id == sequence
                                        && filter.matches(&row)
                                        && tx.send(row).await.is_err()
                                    {
                                        return;
                                    }
                                }
                            }
                            Err(e) => {
                                warn!(error = %e, seq = sequence, "Failed to look up journal entry for subscription");
                            }
                        }
                    }
                    Ok(_) => {} // ignore other events
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(
                            lagged = n,
                            "Journal subscription lagged, some events may be missed"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        debug!("Event bus closed, ending journal subscription");
                        return;
                    }
                }
            }
        });

        rx
    }
}

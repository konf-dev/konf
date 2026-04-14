//! B2 — FanoutJournalStore failure isolation tests (Phase B).
//!
//! Validates the semantics documented in
//! `crates/konf-runtime/src/journal/fanout.rs`:
//!
//! - Primary succeeds ⇒ fanout returns Ok even if secondaries fail
//! - Primary fails    ⇒ fanout returns Err; secondaries are not contacted
//! - Secondary failures never block, propagate, or poison the primary
//! - Dropped secondary writes are counted in metrics
//! - Fanout with zero secondaries behaves identically to its primary

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use konf_runtime::{
    FanoutJournalStore, JournalEntry, JournalError, JournalRow, JournalStore, RunId,
};
use serde_json::json;
use uuid::Uuid;

/// In-memory mock journal for test isolation. Records every append; can be
/// toggled to fail via `set_fail(true)`.
#[derive(Default)]
struct MockJournal {
    entries: Mutex<Vec<JournalRow>>,
    fail: AtomicBool,
    appends_observed: AtomicUsize,
}

impl MockJournal {
    fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    fn set_fail(&self, fail: bool) {
        self.fail.store(fail, Ordering::Relaxed);
    }

    fn len(&self) -> usize {
        self.entries.lock().unwrap().len()
    }

    fn appends_observed(&self) -> usize {
        self.appends_observed.load(Ordering::Relaxed)
    }
}

#[async_trait]
impl JournalStore for MockJournal {
    async fn append(&self, entry: JournalEntry) -> Result<u64, JournalError> {
        self.appends_observed.fetch_add(1, Ordering::Relaxed);
        if self.fail.load(Ordering::Relaxed) {
            return Err(JournalError::storage(std::io::Error::other(
                "mock configured to fail",
            )));
        }
        let mut entries = self.entries.lock().unwrap();
        let id = entries.len() as u64;
        entries.push(JournalRow {
            id,
            run_id: entry.run_id,
            session_id: entry.session_id,
            namespace: entry.namespace,
            event_type: entry.event_type,
            payload: entry.payload,
            created_at: chrono::Utc::now(),
        });
        Ok(id)
    }

    async fn query_by_run(&self, run_id: RunId) -> Result<Vec<JournalRow>, JournalError> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .iter()
            .filter(|r| r.run_id == run_id)
            .cloned()
            .collect())
    }

    async fn query_by_session(
        &self,
        session_id: &str,
        limit: usize,
    ) -> Result<Vec<JournalRow>, JournalError> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .iter()
            .rev()
            .filter(|r| r.session_id == session_id)
            .take(limit)
            .cloned()
            .collect())
    }

    async fn recent(&self, limit: usize) -> Result<Vec<JournalRow>, JournalError> {
        Ok(self
            .entries
            .lock()
            .unwrap()
            .iter()
            .rev()
            .take(limit)
            .cloned()
            .collect())
    }

    async fn reconcile_zombies(&self) -> Result<u64, JournalError> {
        Ok(0)
    }
}

fn sample_entry() -> JournalEntry {
    JournalEntry {
        run_id: Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
        session_id: "session-a".to_string(),
        namespace: "konf:test".to_string(),
        event_type: "tool_invoked".to_string(),
        payload: json!({"tool": "memory:search"}),
    }
}

/// Busy-wait until the predicate is true or `timeout` elapses. Returns
/// whether the predicate became true. Tests secondary fan-out completion
/// without introducing flakiness from arbitrary sleeps.
async fn wait_until(
    timeout: Duration,
    mut check: impl FnMut() -> bool,
) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if check() {
            return true;
        }
        tokio::time::sleep(Duration::from_millis(1)).await;
    }
    check()
}

#[tokio::test]
async fn fanout_writes_to_primary_and_all_secondaries_on_success() {
    let primary = MockJournal::new();
    let s1 = MockJournal::new();
    let s2 = MockJournal::new();
    let fanout = FanoutJournalStore::new(
        primary.clone(),
        vec![s1.clone(), s2.clone()],
    );

    fanout.append(sample_entry()).await.expect("primary ok");

    assert_eq!(primary.len(), 1, "primary must have exactly one entry");
    assert!(
        wait_until(Duration::from_secs(1), || s1.len() == 1 && s2.len() == 1).await,
        "both secondaries must receive the entry"
    );
}

#[tokio::test]
async fn fanout_primary_failure_returns_error() {
    let primary = MockJournal::new();
    primary.set_fail(true);
    let secondary = MockJournal::new();
    let fanout =
        FanoutJournalStore::new(primary.clone(), vec![secondary.clone()]);

    let result = fanout.append(sample_entry()).await;
    assert!(result.is_err(), "primary failure must surface as error");
    // Give any stray secondary tasks time to run — none should have been
    // spawned because the primary failed before fan-out.
    tokio::time::sleep(Duration::from_millis(20)).await;
    assert_eq!(
        secondary.appends_observed(),
        0,
        "secondary must not be contacted when primary fails"
    );
}

#[tokio::test]
async fn fanout_secondary_failure_does_not_return_error() {
    let primary = MockJournal::new();
    let secondary = MockJournal::new();
    secondary.set_fail(true);
    let fanout =
        FanoutJournalStore::new(primary.clone(), vec![secondary.clone()]);

    let result = fanout.append(sample_entry()).await;
    assert!(
        result.is_ok(),
        "secondary failure must not propagate to append result"
    );
    assert_eq!(
        primary.len(),
        1,
        "primary must have succeeded independently"
    );
}

#[tokio::test]
async fn fanout_secondary_failure_increments_drop_metric() {
    let primary = MockJournal::new();
    let secondary = MockJournal::new();
    secondary.set_fail(true);
    let fanout =
        FanoutJournalStore::new(primary.clone(), vec![secondary.clone()]);
    let metrics = fanout.metrics();

    fanout.append(sample_entry()).await.unwrap();

    assert!(
        wait_until(Duration::from_secs(1), || metrics
            .dropped_secondary_writes() == 1)
        .await,
        "drop metric must increment on secondary failure"
    );
}

#[tokio::test]
async fn fanout_multiple_secondary_failures_are_independent() {
    let primary = MockJournal::new();
    let s_ok = MockJournal::new();
    let s_fail1 = MockJournal::new();
    s_fail1.set_fail(true);
    let s_fail2 = MockJournal::new();
    s_fail2.set_fail(true);

    let fanout = FanoutJournalStore::new(
        primary.clone(),
        vec![s_ok.clone(), s_fail1.clone(), s_fail2.clone()],
    );
    let metrics = fanout.metrics();

    fanout.append(sample_entry()).await.unwrap();

    assert!(
        wait_until(Duration::from_secs(1), || s_ok.len() == 1
            && metrics.dropped_secondary_writes() == 2)
        .await,
        "independent secondaries must proceed independently: one ok, two drops"
    );
    assert_eq!(primary.len(), 1);
}

#[tokio::test]
async fn fanout_with_zero_secondaries_behaves_like_primary() {
    let primary = MockJournal::new();
    let fanout = FanoutJournalStore::new(primary.clone(), vec![]);

    let id = fanout.append(sample_entry()).await.unwrap();

    assert_eq!(id, 0, "first append returns seq 0");
    assert_eq!(primary.len(), 1);
    assert_eq!(
        fanout.metrics().dropped_secondary_writes(),
        0,
        "no secondaries means no drop metric activity"
    );
}

#[tokio::test]
async fn fanout_query_methods_delegate_to_primary() {
    let primary = MockJournal::new();
    let secondary = MockJournal::new();
    let fanout =
        FanoutJournalStore::new(primary.clone(), vec![secondary.clone()]);

    fanout.append(sample_entry()).await.unwrap();
    // Let secondary fanout settle so we can reason about its state.
    assert!(
        wait_until(Duration::from_secs(1), || secondary.len() == 1).await
    );

    let by_run = fanout
        .query_by_run(Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap())
        .await
        .unwrap();
    assert_eq!(
        by_run.len(),
        1,
        "query_by_run must delegate to primary only"
    );

    let recent = fanout.recent(10).await.unwrap();
    assert_eq!(recent.len(), 1, "recent must delegate to primary only");

    // Now add an entry only to the secondary directly (bypassing fanout).
    // The fanout's query methods must still return only the primary's view.
    secondary
        .append(sample_entry())
        .await
        .expect("direct secondary write");
    let recent_after = fanout.recent(10).await.unwrap();
    assert_eq!(
        recent_after.len(),
        1,
        "fanout.recent must NOT see entries written only to secondaries"
    );
}

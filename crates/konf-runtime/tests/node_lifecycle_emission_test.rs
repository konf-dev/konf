//! B4 — Node lifecycle emission tests (Phase B).
//!
//! Before this bug fix, [`RunEvent::NodeStart`] and [`RunEvent::NodeEnd`]
//! were defined on the event bus but never emitted. These tests validate
//! that [`RuntimeHooks`] now emits them on every node lifecycle transition
//! (start / complete / failed) and that the emissions reach live bus
//! subscribers.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use konflux::hooks::ExecutionHooks;
use konf_runtime::process::{NodeStatus, ProcessTable};
use konf_runtime::scope::{Actor, ActorRole};
use konf_runtime::{
    JournalEntry, JournalError, JournalRow, JournalStore, RunEvent, RunEventBus, RunId,
};
use serde_json::json;
use uuid::Uuid;

/// In-memory journal that records every append. Used to assert the
/// coexistence of event-bus emission and journal append.
#[derive(Default)]
struct CaptureJournal {
    entries: Mutex<Vec<JournalEntry>>,
}

#[async_trait]
impl JournalStore for CaptureJournal {
    async fn append(&self, entry: JournalEntry) -> Result<u64, JournalError> {
        let mut entries = self.entries.lock().unwrap();
        let id = entries.len() as u64;
        entries.push(entry);
        Ok(id)
    }
    async fn query_by_run(&self, _: RunId) -> Result<Vec<JournalRow>, JournalError> {
        Ok(vec![])
    }
    async fn query_by_session(
        &self,
        _: &str,
        _: usize,
    ) -> Result<Vec<JournalRow>, JournalError> {
        Ok(vec![])
    }
    async fn recent(&self, _: usize) -> Result<Vec<JournalRow>, JournalError> {
        Ok(vec![])
    }
    async fn reconcile_zombies(&self) -> Result<u64, JournalError> {
        Ok(0)
    }
}

fn hooks_with_bus(
    bus: Arc<RunEventBus>,
    journal: Option<Arc<dyn JournalStore>>,
) -> konf_runtime::hooks::RuntimeHooks {
    konf_runtime::hooks::RuntimeHooks {
        run_id: Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap(),
        namespace: "konf:test".to_string(),
        session_id: "sess-a".to_string(),
        table: Arc::new(ProcessTable::new()),
        journal,
        event_bus: bus,
        actor: Actor {
            id: "test_actor".to_string(),
            role: ActorRole::System,
        },
        trace_id: Uuid::parse_str("00000000-0000-0000-0000-0000000000ff").unwrap(),
    }
}

/// Wait for `expected_count` events on a pre-opened subscriber, or fail.
async fn collect_events(
    mut rx: tokio::sync::broadcast::Receiver<RunEvent>,
    expected_count: usize,
    timeout: Duration,
) -> Vec<RunEvent> {
    let mut out = Vec::with_capacity(expected_count);
    let deadline = tokio::time::Instant::now() + timeout;
    while out.len() < expected_count {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, rx.recv()).await {
            Ok(Ok(ev)) => out.push(ev),
            Ok(Err(_)) => break, // channel closed
            Err(_) => break,     // timeout
        }
    }
    out
}

#[tokio::test]
async fn on_node_start_emits_run_event_node_start() {
    let bus = Arc::new(RunEventBus::default());
    let rx = bus.subscribe();
    let hooks = hooks_with_bus(bus, None);

    hooks.on_node_start("step_1", "tool:memory:search");

    let events = collect_events(rx, 1, Duration::from_millis(200)).await;
    assert_eq!(events.len(), 1, "exactly one event");
    match &events[0] {
        RunEvent::NodeStart {
            node_id,
            tool,
            run_id,
            ..
        } => {
            assert_eq!(node_id, "step_1");
            assert_eq!(tool, "tool:memory:search");
            assert_eq!(
                *run_id,
                Uuid::parse_str("00000000-0000-0000-0000-000000000001").unwrap()
            );
        }
        other => panic!("expected NodeStart, got {other:?}"),
    }
}

#[tokio::test]
async fn on_node_complete_emits_run_event_node_end_with_completed_status() {
    let bus = Arc::new(RunEventBus::default());
    let rx = bus.subscribe();
    let hooks = hooks_with_bus(bus, None);

    hooks.on_node_complete("step_1", "tool:memory:search", 142, &json!({"ok": true}));

    let events = collect_events(rx, 1, Duration::from_millis(200)).await;
    assert_eq!(events.len(), 1);
    match &events[0] {
        RunEvent::NodeEnd {
            node_id,
            status,
            ..
        } => {
            assert_eq!(node_id, "step_1");
            match status {
                NodeStatus::Completed { duration_ms } => assert_eq!(*duration_ms, 142),
                other => panic!("expected Completed, got {other:?}"),
            }
        }
        other => panic!("expected NodeEnd, got {other:?}"),
    }
}

#[tokio::test]
async fn on_node_failed_emits_run_event_node_end_with_failed_status() {
    let bus = Arc::new(RunEventBus::default());
    let rx = bus.subscribe();
    let hooks = hooks_with_bus(bus, None);

    hooks.on_node_failed("step_1", "tool:memory:search", "out of memory");

    let events = collect_events(rx, 1, Duration::from_millis(200)).await;
    assert_eq!(events.len(), 1);
    match &events[0] {
        RunEvent::NodeEnd {
            node_id, status, ..
        } => {
            assert_eq!(node_id, "step_1");
            match status {
                NodeStatus::Failed { error } => assert_eq!(error, "out of memory"),
                other => panic!("expected Failed, got {other:?}"),
            }
        }
        other => panic!("expected NodeEnd, got {other:?}"),
    }
}

#[tokio::test]
async fn multiple_subscribers_all_receive_node_events() {
    let bus = Arc::new(RunEventBus::default());
    let rx1 = bus.subscribe();
    let rx2 = bus.subscribe();
    let rx3 = bus.subscribe();
    let hooks = hooks_with_bus(bus, None);

    hooks.on_node_start("step_1", "tool:a");
    hooks.on_node_complete("step_1", "tool:a", 10, &json!(null));

    // Each subscriber must observe both events independently.
    for (name, rx) in [("rx1", rx1), ("rx2", rx2), ("rx3", rx3)] {
        let events = collect_events(rx, 2, Duration::from_millis(200)).await;
        assert_eq!(events.len(), 2, "{name} must see both events");
    }
}

#[tokio::test]
async fn events_emit_alongside_journal_append() {
    let bus = Arc::new(RunEventBus::default());
    let rx = bus.subscribe();
    let journal = Arc::new(CaptureJournal::default());
    let hooks = hooks_with_bus(bus, Some(journal.clone() as Arc<dyn JournalStore>));

    hooks.on_node_start("step_1", "tool:memory:search");
    hooks.on_node_complete("step_1", "tool:memory:search", 42, &json!(null));

    // Event bus: both events visible.
    let events = collect_events(rx, 2, Duration::from_millis(200)).await;
    assert_eq!(events.len(), 2);

    // Journal: both appends happened too (fire-and-forget — allow brief settle).
    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    loop {
        let n = journal.entries.lock().unwrap().len();
        if n == 2 {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            panic!("expected 2 journal entries; got {n}");
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let entries = journal.entries.lock().unwrap().clone();
    // F1 fix: both entries now carry the uniform Interaction envelope
    // with event_type="interaction". The phase discriminator moved into
    // attributes.phase.
    assert_eq!(entries[0].event_type, "interaction");
    assert_eq!(entries[1].event_type, "interaction");
    assert_eq!(entries[0].payload["attributes"]["phase"], "start");
    assert_eq!(entries[1].payload["attributes"]["phase"], "end");
    assert_eq!(entries[0].payload["kind"]["type"], "node_lifecycle");
    assert_eq!(entries[1].payload["kind"]["type"], "node_lifecycle");
}

#[tokio::test]
async fn node_start_uses_monotonic_timestamp_relative_to_completion() {
    // Sanity: NodeStart emitted before NodeEnd, timestamps should be ordered.
    let bus = Arc::new(RunEventBus::default());
    let rx = bus.subscribe();
    let hooks = hooks_with_bus(bus, None);

    hooks.on_node_start("step_1", "tool:a");
    // small gap so timestamps are definitely distinct
    tokio::time::sleep(Duration::from_millis(2)).await;
    hooks.on_node_complete("step_1", "tool:a", 2, &json!(null));

    let events = collect_events(rx, 2, Duration::from_millis(200)).await;
    let start_at = match &events[0] {
        RunEvent::NodeStart { at, .. } => *at,
        other => panic!("expected NodeStart first, got {other:?}"),
    };
    let end_at = match &events[1] {
        RunEvent::NodeEnd { at, .. } => *at,
        other => panic!("expected NodeEnd second, got {other:?}"),
    };
    assert!(end_at >= start_at, "end timestamp >= start timestamp");
}

// ============================================================
// F1 — Node Interaction fidelity assertions (Phase F fix)
// ============================================================

/// F1: on_node_start emits an Interaction-shaped journal entry with
/// kind=NodeLifecycle, status=Pending.
#[tokio::test]
async fn on_node_start_emits_interaction_shaped_journal_entry() {
    let bus = Arc::new(RunEventBus::default());
    let journal = Arc::new(CaptureJournal::default());
    let hooks = hooks_with_bus(bus, Some(journal.clone() as Arc<dyn JournalStore>));

    hooks.on_node_start("step_1", "tool:memory:search");

    // Wait for fire-and-forget journal append.
    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    while tokio::time::Instant::now() < deadline {
        if !journal.entries.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }

    let entries = journal.entries.lock().unwrap().clone();
    assert_eq!(entries.len(), 1);
    assert_eq!(entries[0].event_type, "interaction");

    let payload = &entries[0].payload;
    assert_eq!(payload["kind"]["type"], "node_lifecycle");
    assert_eq!(payload["status"]["type"], "pending");
    assert_eq!(payload["target"], "node:step_1");
    assert_eq!(payload["node_id"], "step_1");
    assert_eq!(payload["attributes"]["phase"], "start");
    assert_eq!(payload["attributes"]["tool"], "tool:memory:search");
    assert!(payload["actor"]["id"].is_string(), "actor inline for audit");
    assert!(payload["namespace"].is_string(), "namespace inline for audit");
}

/// F1: on_node_complete emits NodeLifecycle with status=Ok and duration.
#[tokio::test]
async fn on_node_complete_emits_interaction_with_ok_status() {
    let bus = Arc::new(RunEventBus::default());
    let journal = Arc::new(CaptureJournal::default());
    let hooks = hooks_with_bus(bus, Some(journal.clone() as Arc<dyn JournalStore>));

    hooks.on_node_complete("step_1", "tool:memory:search", 142, &json!(null));

    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    while tokio::time::Instant::now() < deadline {
        if !journal.entries.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let entries = journal.entries.lock().unwrap().clone();
    let payload = &entries[0].payload;
    assert_eq!(payload["kind"]["type"], "node_lifecycle");
    assert_eq!(payload["status"]["type"], "ok");
    assert_eq!(payload["attributes"]["phase"], "end");
    assert_eq!(payload["attributes"]["duration_ms"], 142);
}

/// F1: on_node_failed emits NodeLifecycle with status=Failed carrying error.
#[tokio::test]
async fn on_node_failed_emits_interaction_with_failed_status() {
    let bus = Arc::new(RunEventBus::default());
    let journal = Arc::new(CaptureJournal::default());
    let hooks = hooks_with_bus(bus, Some(journal.clone() as Arc<dyn JournalStore>));

    hooks.on_node_failed("step_1", "tool:memory:search", "oom");

    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    while tokio::time::Instant::now() < deadline {
        if !journal.entries.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let entries = journal.entries.lock().unwrap().clone();
    let payload = &entries[0].payload;
    assert_eq!(payload["kind"]["type"], "node_lifecycle");
    assert_eq!(payload["status"]["type"], "failed");
    assert_eq!(payload["status"]["error"], "oom");
    assert_eq!(payload["attributes"]["phase"], "failed");
}

/// F1: parity with invoke_tool — both tool dispatches and node lifecycle
/// entries share event_type="interaction", making bird's-eye queries
/// symmetric.
#[tokio::test]
async fn tool_dispatch_and_node_lifecycle_share_event_type() {
    // Both kinds produce journal entries with event_type="interaction".
    // We can't exercise invoke_tool directly here without a full Runtime,
    // but we verify the node-lifecycle side produces the right event_type;
    // automatic_capture_test.rs covers the invoke_tool side with the same
    // assertion. Together they prove parity.
    let bus = Arc::new(RunEventBus::default());
    let journal = Arc::new(CaptureJournal::default());
    let hooks = hooks_with_bus(bus, Some(journal.clone() as Arc<dyn JournalStore>));

    hooks.on_node_start("x", "tool:y");
    let deadline = tokio::time::Instant::now() + Duration::from_millis(500);
    while tokio::time::Instant::now() < deadline {
        if !journal.entries.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
    let entries = journal.entries.lock().unwrap().clone();
    assert_eq!(
        entries[0].event_type, "interaction",
        "parity: node lifecycle uses same event_type as invoke_tool"
    );
}

/// Proves the pre-existing bug: before this fix, a subscriber saw zero
/// node events even while the hooks were invoked. With the fix in place,
/// this test now demonstrates the bug is closed by asserting emission
/// occurs on a fresh subscriber that never had a chance to miss anything.
#[tokio::test]
async fn pre_existing_bug_closed_node_events_emitted_after_subscribe() {
    let bus = Arc::new(RunEventBus::default());
    let observed = Arc::new(AtomicUsize::new(0));

    // Spawn a subscriber task that counts node events for 100ms.
    let observed_clone = observed.clone();
    let mut rx = bus.subscribe();
    let subscriber = tokio::spawn(async move {
        let deadline = tokio::time::Instant::now() + Duration::from_millis(100);
        while tokio::time::Instant::now() < deadline {
            let remaining =
                deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                break;
            }
            if let Ok(Ok(ev)) = tokio::time::timeout(remaining, rx.recv()).await {
                if matches!(ev, RunEvent::NodeStart { .. } | RunEvent::NodeEnd { .. }) {
                    observed_clone.fetch_add(1, Ordering::Relaxed);
                }
            } else {
                break;
            }
        }
    });

    let hooks = hooks_with_bus(bus, None);
    hooks.on_node_start("step_1", "tool:a");
    hooks.on_node_complete("step_1", "tool:a", 1, &json!(null));
    hooks.on_node_start("step_2", "tool:b");
    hooks.on_node_failed("step_2", "tool:b", "whoops");

    subscriber.await.unwrap();
    assert_eq!(
        observed.load(Ordering::Relaxed),
        4,
        "bug closed: NodeStart/NodeEnd/NodeStart/NodeEnd all emitted and observed"
    );
}

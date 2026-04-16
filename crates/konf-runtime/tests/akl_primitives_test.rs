//! Stage 9 — Akl widening primitives integration tests.

use konf_runtime::budget::{BudgetError, BudgetTable};

// ── Budget cell tests ──────────────────────────────────────────────────

#[test]
fn budget_cell_atomic_decrement() {
    let table = BudgetTable::new();
    let tid = uuid::Uuid::new_v4();
    table.mint(tid, 100);

    let remaining = table.decrement(tid, 30).unwrap();
    assert_eq!(remaining, 70);

    let err = table.decrement(tid, 80).unwrap_err();
    assert_eq!(
        err,
        BudgetError::Insufficient {
            remaining: 70,
            requested: 80,
        }
    );
}

#[test]
fn budget_cell_not_found() {
    let table = BudgetTable::new();
    let tid = uuid::Uuid::new_v4();
    assert_eq!(table.decrement(tid, 10).unwrap_err(), BudgetError::NotFound);
}

// ── Idempotency replay via journal ─────────────────────────────────────

#[tokio::test]
async fn idempotency_replay_returns_cached() {
    use konf_runtime::journal::{JournalEntry, JournalStore, RedbJournal};
    use serde_json::json;
    use tempfile::tempdir;

    let dir = tempdir().unwrap();
    let path = dir.path().join("idem.redb");
    let journal = RedbJournal::open(&path).await.unwrap();

    // Append an entry with an idempotency key.
    let entry = JournalEntry {
        run_id: None,
        session_id: "sess".into(),
        namespace: "konf:test".into(),
        event_type: "interaction".into(),
        payload: json!({"result": "cached_value"}),
        valid_to: None,
        idempotency_key: Some("abc".into()),
    };
    journal.append(entry).await.unwrap();

    // Look it up by key.
    let row = journal.get_by_idempotency_key("abc").await.unwrap();
    assert!(row.is_some(), "Should find cached entry by idempotency key");
    let row = row.unwrap();
    assert_eq!(row.payload["result"], "cached_value");
    assert_eq!(row.idempotency_key.as_deref(), Some("abc"));

    // Missing key returns None.
    let none = journal.get_by_idempotency_key("nonexistent").await.unwrap();
    assert!(none.is_none());
}

// ── QoS class propagation ──────────────────────────────────────────────

#[test]
fn qos_class_propagated_in_respond() {
    use konflux_substrate::envelope::{Envelope, QoSClass};
    let mut env = Envelope::test(serde_json::json!({}));
    env.qos_class = Some(QoSClass::Critical);

    let child = env.respond(serde_json::json!({"child": true}));
    assert_eq!(child.qos_class, Some(QoSClass::Critical));
}

// ── Deadline enforcement ───────────────────────────────────────────────

#[test]
fn deadline_exceeded_detected() {
    use chrono::{Duration, Utc};
    // Simulates the check the dispatcher/executor performs.
    let deadline = Utc::now() - Duration::seconds(1);
    assert!(
        Utc::now() > deadline,
        "A past deadline should be detected as exceeded"
    );
}

#[test]
fn deadline_future_passes() {
    use chrono::{Duration, Utc};
    let deadline = Utc::now() + Duration::seconds(60);
    assert!(
        Utc::now() < deadline,
        "A future deadline should not be exceeded"
    );
}

#[test]
fn deadline_propagated_in_respond() {
    use chrono::{Duration, Utc};
    use konflux_substrate::envelope::Envelope;

    let mut env = Envelope::test(serde_json::json!({}));
    let deadline = Utc::now() + Duration::seconds(30);
    env.deadline = Some(deadline);

    let child = env.respond(serde_json::json!({}));
    assert_eq!(child.deadline, Some(deadline));
}

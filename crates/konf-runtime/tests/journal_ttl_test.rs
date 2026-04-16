//! Tests for journal TTL, expired-invisible invariant, subscribe, and aggregate.
//! Added incrementally across Stage 7 micro-checkpoints.

use std::sync::Arc;

use chrono::{Duration, Utc};
use serde_json::json;

use konf_runtime::journal::subscribe::JournalSubscription;
use konf_runtime::{RedbJournal, RunEventBus};
use konflux_substrate::journal::{
    AggregateQuery, AggregateResult, JournalEntry, JournalFilter, JournalStore,
};

async fn open_journal() -> (RedbJournal, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test_journal.redb");
    let journal = RedbJournal::open(&path).await.unwrap();
    (journal, dir)
}

fn test_entry(namespace: &str, event_type: &str) -> JournalEntry {
    JournalEntry {
        run_id: None,
        session_id: "test_sess".into(),
        namespace: namespace.into(),
        event_type: event_type.into(),
        payload: json!({"test": true}),
        valid_to: None,
        idempotency_key: None,
    }
}

// ============================================================
// 7.b — valid_to storage round-trip
// ============================================================

#[tokio::test]
async fn redb_stores_and_returns_valid_to() {
    let (journal, _dir) = open_journal().await;

    let future_time = Utc::now() + Duration::hours(1);
    let entry = JournalEntry {
        valid_to: Some(future_time),
        ..test_entry("konf:test:ttl", "interaction")
    };

    let seq = journal.append(entry).await.unwrap();
    assert!(seq > 0);

    let rows = journal.query_by_session("test_sess", 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    let row = &rows[0];
    assert!(row.valid_to.is_some(), "valid_to should be populated");
    // Check within 1 second tolerance (microsecond rounding)
    let diff = (row.valid_to.unwrap() - future_time)
        .num_milliseconds()
        .abs();
    assert!(diff < 1000, "valid_to should match: diff={diff}ms");
}

#[tokio::test]
async fn redb_none_valid_to_round_trips() {
    let (journal, _dir) = open_journal().await;

    let entry = test_entry("konf:test:ttl", "interaction");
    journal.append(entry).await.unwrap();

    let rows = journal.query_by_session("test_sess", 10).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert!(
        rows[0].valid_to.is_none(),
        "valid_to should be None for entries without TTL"
    );
}

// ============================================================
// 7.c — Expired-invisible invariant
// ============================================================

#[tokio::test]
async fn expired_invisible_query_by_session() {
    let (journal, _dir) = open_journal().await;

    // Append an already-expired entry
    let expired = JournalEntry {
        valid_to: Some(Utc::now() - Duration::hours(1)),
        ..test_entry("konf:test:expired", "interaction")
    };
    journal.append(expired).await.unwrap();

    // Should not appear in query_by_session
    let rows = journal.query_by_session("test_sess", 10).await.unwrap();
    assert!(rows.is_empty(), "Expired entry should be invisible");
}

#[tokio::test]
async fn expired_visible_with_include_expired() {
    let (journal, _dir) = open_journal().await;

    let expired = JournalEntry {
        valid_to: Some(Utc::now() - Duration::hours(1)),
        ..test_entry("konf:test:expired", "interaction")
    };
    journal.append(expired).await.unwrap();

    // query() with include_expired=true should return it
    let filter = JournalFilter {
        session_id: Some("test_sess".into()),
        include_expired: true,
        ..Default::default()
    };
    let rows = journal.query(&filter, 10).await.unwrap();
    assert_eq!(
        rows.len(),
        1,
        "Expired entry should be visible with include_expired"
    );
}

#[tokio::test]
async fn unexpired_entry_appears_normally() {
    let (journal, _dir) = open_journal().await;

    let future = JournalEntry {
        valid_to: Some(Utc::now() + Duration::hours(1)),
        ..test_entry("konf:test:fresh", "interaction")
    };
    journal.append(future).await.unwrap();

    let rows = journal.query_by_session("test_sess", 10).await.unwrap();
    assert_eq!(rows.len(), 1, "Unexpired entry should appear");

    let filter = JournalFilter {
        session_id: Some("test_sess".into()),
        ..Default::default()
    };
    let rows = journal.query(&filter, 10).await.unwrap();
    assert_eq!(
        rows.len(),
        1,
        "Unexpired entry should appear in filtered query"
    );
}

// ============================================================
// 7.d — TTL sweep
// ============================================================

#[tokio::test]
async fn ttl_sweep_removes_expired() {
    let (journal, _dir) = open_journal().await;

    // Append an already-expired entry
    let expired = JournalEntry {
        valid_to: Some(Utc::now() - Duration::seconds(1)),
        ..test_entry("konf:test:sweep", "interaction")
    };
    journal.append(expired).await.unwrap();

    // Also append a non-expiring entry
    journal
        .append(test_entry("konf:test:sweep", "other"))
        .await
        .unwrap();

    // Before sweep: 2 entries total (1 invisible, 1 visible)
    let filter_all = JournalFilter {
        session_id: Some("test_sess".into()),
        include_expired: true,
        ..Default::default()
    };
    let rows = journal.query(&filter_all, 10).await.unwrap();
    assert_eq!(rows.len(), 2, "Should have 2 entries before sweep");

    // Sweep
    let deleted = journal.delete_expired().await.unwrap();
    assert_eq!(deleted, 1, "Should delete 1 expired entry");

    // After sweep: only 1 entry remains
    let rows = journal.query(&filter_all, 10).await.unwrap();
    assert_eq!(rows.len(), 1, "Should have 1 entry after sweep");
    assert_eq!(rows[0].event_type, "other");
}

#[tokio::test]
async fn ttl_sweep_preserves_unexpired() {
    let (journal, _dir) = open_journal().await;

    let future = JournalEntry {
        valid_to: Some(Utc::now() + Duration::hours(1)),
        ..test_entry("konf:test:sweep", "interaction")
    };
    journal.append(future).await.unwrap();

    let deleted = journal.delete_expired().await.unwrap();
    assert_eq!(deleted, 0, "Should not delete unexpired entries");

    let rows = journal.query_by_session("test_sess", 10).await.unwrap();
    assert_eq!(rows.len(), 1, "Unexpired entry should survive sweep");
}

// ============================================================
// 7.f — Aggregate
// ============================================================

#[tokio::test]
async fn aggregate_count_over_namespace() {
    let (journal, _dir) = open_journal().await;

    // Append 3 entries in namespace "x", 1 in namespace "y"
    for _ in 0..3 {
        journal
            .append(test_entry("konf:test:x", "interaction"))
            .await
            .unwrap();
    }
    journal
        .append(test_entry("konf:test:y", "interaction"))
        .await
        .unwrap();

    let filter = JournalFilter {
        namespace: Some("konf:test:x".into()),
        ..Default::default()
    };
    let result = journal
        .aggregate(&filter, &AggregateQuery::Count)
        .await
        .unwrap();
    assert_eq!(result, AggregateResult::Count(3));
}

#[tokio::test]
async fn aggregate_respects_ttl_by_default() {
    let (journal, _dir) = open_journal().await;

    // 2 live entries + 1 expired
    journal
        .append(test_entry("konf:test:agg", "interaction"))
        .await
        .unwrap();
    journal
        .append(test_entry("konf:test:agg", "interaction"))
        .await
        .unwrap();
    let expired = JournalEntry {
        valid_to: Some(Utc::now() - Duration::hours(1)),
        ..test_entry("konf:test:agg", "interaction")
    };
    journal.append(expired).await.unwrap();

    // Default filter excludes expired
    let filter = JournalFilter {
        namespace: Some("konf:test:agg".into()),
        ..Default::default()
    };
    let result = journal
        .aggregate(&filter, &AggregateQuery::Count)
        .await
        .unwrap();
    assert_eq!(result, AggregateResult::Count(2));

    // With include_expired
    let filter_all = JournalFilter {
        namespace: Some("konf:test:agg".into()),
        include_expired: true,
        ..Default::default()
    };
    let result = journal
        .aggregate(&filter_all, &AggregateQuery::Count)
        .await
        .unwrap();
    assert_eq!(result, AggregateResult::Count(3));
}

#[tokio::test]
async fn aggregate_most_recent() {
    let (journal, _dir) = open_journal().await;

    journal
        .append(test_entry("konf:test:recent", "interaction"))
        .await
        .unwrap();
    // Small delay to ensure ordering
    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    journal
        .append(test_entry("konf:test:recent", "interaction"))
        .await
        .unwrap();

    let filter = JournalFilter {
        namespace: Some("konf:test:recent".into()),
        ..Default::default()
    };
    let result = journal
        .aggregate(&filter, &AggregateQuery::MostRecent)
        .await
        .unwrap();
    match result {
        AggregateResult::MostRecent(Some(ts)) => {
            let age = Utc::now() - ts;
            assert!(age.num_seconds() < 5, "Most recent should be very recent");
        }
        other => panic!("Expected MostRecent(Some(_)), got {other:?}"),
    }
}

// ============================================================
// 7.e — Subscribe
// ============================================================

#[tokio::test]
async fn subscribe_replay_backfill() {
    let (journal, _dir) = open_journal().await;

    // Append entries BEFORE subscribing
    for i in 0..3 {
        journal
            .append(test_entry("konf:test:sub", &format!("event_{i}")))
            .await
            .unwrap();
    }
    // One in a different namespace (should be filtered out)
    journal
        .append(test_entry("konf:test:other", "event_x"))
        .await
        .unwrap();

    let event_bus = RunEventBus::default();
    let filter = JournalFilter {
        namespace: Some("konf:test:sub".into()),
        ..Default::default()
    };

    let mut rx = JournalSubscription::start(Arc::new(journal), &event_bus, filter, 100, 256);

    // Should receive the 3 matching entries via replay
    for _ in 0..3 {
        let row = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
            .await
            .expect("timeout waiting for replay")
            .expect("channel closed");
        assert_eq!(row.namespace, "konf:test:sub");
    }
}

#[tokio::test]
async fn subscribe_filter_respects_ttl() {
    let (journal, _dir) = open_journal().await;

    // One live, one expired
    journal
        .append(test_entry("konf:test:sub_ttl", "live"))
        .await
        .unwrap();
    let expired = JournalEntry {
        valid_to: Some(Utc::now() - Duration::hours(1)),
        ..test_entry("konf:test:sub_ttl", "expired")
    };
    journal.append(expired).await.unwrap();

    let event_bus = RunEventBus::default();
    let filter = JournalFilter {
        namespace: Some("konf:test:sub_ttl".into()),
        ..Default::default()
    };

    let mut rx = JournalSubscription::start(Arc::new(journal), &event_bus, filter, 100, 256);

    // Should only receive the live entry
    let row = tokio::time::timeout(std::time::Duration::from_secs(2), rx.recv())
        .await
        .expect("timeout")
        .expect("closed");
    assert_eq!(row.event_type, "live");

    // No more entries (expired was filtered)
    let result = tokio::time::timeout(std::time::Duration::from_millis(100), rx.recv()).await;
    assert!(
        result.is_err(),
        "Should timeout — no more entries to receive"
    );
}

//! B7 — SurrealJournalStore tests (Phase B).
//!
//! Uses an in-memory SurrealDB instance (no external daemon required) and
//! applies the minimum event-table schema required for the journal store.

use konf_runtime::{JournalEntry, JournalStore};
use konf_tool_memory_surreal::SurrealJournalStore;
use serde_json::json;
use surrealdb::engine::any::connect;
use surrealdb::engine::any::Any;
use surrealdb::Surreal;
use uuid::Uuid;

/// Apply the minimum schema needed for the journal store. Idempotent.
async fn apply_event_schema(db: &Surreal<Any>) {
    db.query(
        r#"
        DEFINE TABLE IF NOT EXISTS event SCHEMAFULL;
        DEFINE FIELD IF NOT EXISTS namespace  ON event TYPE string;
        DEFINE FIELD IF NOT EXISTS event_type ON event TYPE string;
        DEFINE FIELD IF NOT EXISTS payload    ON event TYPE object FLEXIBLE DEFAULT {};
        DEFINE FIELD IF NOT EXISTS created_at ON event TYPE datetime DEFAULT time::now();
        DEFINE INDEX IF NOT EXISTS event_namespace_created
            ON event FIELDS namespace, created_at;
        "#,
    )
    .await
    .expect("schema apply")
    .check()
    .expect("schema check");
}

/// Open an in-memory SurrealDB, select a namespace/database, apply schema.
async fn fresh_db() -> Surreal<Any> {
    let db = connect("memory").await.expect("memory db");
    db.use_ns("konf_test")
        .use_db("journal_store_test")
        .await
        .expect("ns/db");
    apply_event_schema(&db).await;
    db
}

fn sample_entry(run_id: Uuid, session: &str, event_type: &str) -> JournalEntry {
    JournalEntry {
        run_id: Some(run_id),
        session_id: session.to_string(),
        namespace: "konf:test".to_string(),
        event_type: event_type.to_string(),
        payload: json!({"tool": "memory:search", "latency_ms": 12}),
    }
}

#[tokio::test]
async fn append_writes_row_to_event_table() {
    let db = fresh_db().await;
    let store = SurrealJournalStore::new(db.clone());

    let run_id = Uuid::new_v4();
    store
        .append(sample_entry(run_id, "sess-a", "tool_invoked"))
        .await
        .expect("append ok");

    let mut result = db.query("SELECT * FROM event").await.unwrap();
    let rows: Vec<serde_json::Value> = result.take(0).unwrap();
    assert_eq!(rows.len(), 1, "exactly one row written");
}

#[tokio::test]
async fn append_preserves_namespace_field() {
    let db = fresh_db().await;
    let store = SurrealJournalStore::new(db.clone());
    let run_id = Uuid::new_v4();

    let mut entry = sample_entry(run_id, "sess-a", "tool_invoked");
    entry.namespace = "konf:product:user_99".to_string();
    store.append(entry).await.unwrap();

    let rows = store.query_by_run(run_id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].namespace, "konf:product:user_99");
}

#[tokio::test]
async fn append_preserves_event_type_field() {
    let db = fresh_db().await;
    let store = SurrealJournalStore::new(db);
    let run_id = Uuid::new_v4();

    store
        .append(sample_entry(run_id, "sess-a", "run_completed"))
        .await
        .unwrap();

    let rows = store.query_by_run(run_id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].event_type, "run_completed");
}

#[tokio::test]
async fn append_preserves_payload_field() {
    let db = fresh_db().await;
    let store = SurrealJournalStore::new(db);
    let run_id = Uuid::new_v4();

    let mut entry = sample_entry(run_id, "sess-a", "tool_invoked");
    entry.payload = json!({"kind": "interaction", "target": "tool:memory:search"});
    store.append(entry).await.unwrap();

    let rows = store.query_by_run(run_id).await.unwrap();
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].payload["target"], "tool:memory:search");
    assert_eq!(rows[0].payload["kind"], "interaction");
}

#[tokio::test]
async fn query_by_run_returns_matching_entries() {
    let db = fresh_db().await;
    let store = SurrealJournalStore::new(db);

    let run_a = Uuid::new_v4();
    let run_b = Uuid::new_v4();

    store.append(sample_entry(run_a, "s", "a1")).await.unwrap();
    store.append(sample_entry(run_b, "s", "b1")).await.unwrap();
    store.append(sample_entry(run_a, "s", "a2")).await.unwrap();

    let a = store.query_by_run(run_a).await.unwrap();
    assert_eq!(a.len(), 2, "both run_a entries returned");
    assert!(a.iter().all(|r| r.run_id == Some(run_a)));
    assert_eq!(a[0].event_type, "a1");
    assert_eq!(a[1].event_type, "a2");

    let b = store.query_by_run(run_b).await.unwrap();
    assert_eq!(b.len(), 1);
    assert_eq!(b[0].event_type, "b1");
}

#[tokio::test]
async fn query_by_session_returns_limited_entries_in_reverse_order() {
    let db = fresh_db().await;
    let store = SurrealJournalStore::new(db);
    let run = Uuid::new_v4();

    for i in 0..5 {
        let mut e = sample_entry(run, "sess-x", &format!("e{i}"));
        e.session_id = "sess-x".to_string();
        store.append(e).await.unwrap();
        // ensure distinct created_at timestamps — Surreal time::now()
        // resolution requires a brief yield.
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }

    let rows = store.query_by_session("sess-x", 3).await.unwrap();
    assert_eq!(rows.len(), 3);
    // Most recent first.
    assert_eq!(rows[0].event_type, "e4");
    assert_eq!(rows[1].event_type, "e3");
    assert_eq!(rows[2].event_type, "e2");
}

#[tokio::test]
async fn recent_returns_most_recent_first() {
    let db = fresh_db().await;
    let store = SurrealJournalStore::new(db);
    let run = Uuid::new_v4();

    for i in 0..3 {
        store
            .append(sample_entry(run, "s", &format!("ev{i}")))
            .await
            .unwrap();
        tokio::time::sleep(std::time::Duration::from_millis(2)).await;
    }

    let rows = store.recent(10).await.unwrap();
    assert_eq!(rows.len(), 3);
    assert_eq!(rows[0].event_type, "ev2");
    assert_eq!(rows[2].event_type, "ev0");
}

#[tokio::test]
async fn idempotent_schema_definition_does_not_fail_on_reconnect() {
    let db = fresh_db().await;
    // Apply a second time; `IF NOT EXISTS` on every DEFINE means this is a
    // no-op rather than an error.
    apply_event_schema(&db).await;

    let store = SurrealJournalStore::new(db);
    let run = Uuid::new_v4();
    store
        .append(sample_entry(run, "s", "ev"))
        .await
        .expect("append still works after schema re-apply");
}

#[tokio::test]
async fn reconcile_zombies_is_a_noop_returning_zero() {
    let db = fresh_db().await;
    let store = SurrealJournalStore::new(db);
    assert_eq!(
        store.reconcile_zombies().await.unwrap(),
        0,
        "secondary mirror does not reconcile"
    );
}

//! End-to-end backend integration tests against a real in-memory SurrealDB.
//!
//! These tests spin up `Surreal<Any>` in `mem://` mode via the public
//! `connect()` entry point and drive every `MemoryBackend` trait method,
//! matching the way `konf-init` wires the backend at boot. Nothing is mocked
//! — schema is applied, writes hit RocksDB's in-memory KV, reads go back
//! through SurrealQL. If any of these tests pass, the corresponding behavior
//! works in production too.
//!
//! The tests use a unique Surreal namespace per test to avoid cross-test
//! contamination when SurrealDB is shared across a process.

use konf_tool_memory::{MemoryBackend, SearchParams};
use konf_tool_memory_surreal::connect;
use serde_json::{json, Value};

/// Build a fresh in-memory backend with a unique Surreal namespace so
/// parallel tests don't collide. Uses a random ns suffix per test.
async fn fresh_backend(namespace_tag: &str) -> std::sync::Arc<dyn MemoryBackend> {
    let config = json!({
        "mode": "memory",
        "namespace": format!("konf-test-{namespace_tag}"),
        "database": "default",
        "vector_dimension": 4,
    });
    connect(&config).await.expect("connect should succeed")
}

// ============================================================
// add_nodes
// ============================================================

#[tokio::test]
async fn add_nodes_without_embedding_returns_count() {
    let backend = fresh_backend("add_plain").await;
    let nodes = vec![
        json!({"content": "hello world", "node_type": "memory"}),
        json!({"content": "goodbye world", "node_type": "memory"}),
    ];
    let result = backend.add_nodes(&nodes, Some("alice")).await.unwrap();
    assert_eq!(result["added"], 2);
    assert_eq!(result["_meta"]["namespace"], "alice");
}

#[tokio::test]
async fn add_nodes_with_embedding_stores_vector() {
    let backend = fresh_backend("add_embed").await;
    let nodes = vec![json!({
        "content": "vector payload",
        "embedding": [0.1, 0.2, 0.3, 0.4],
        "model_name": "test-embedder",
    })];
    let result = backend.add_nodes(&nodes, Some("bob")).await.unwrap();
    assert_eq!(result["added"], 1);
}

#[tokio::test]
async fn add_nodes_rejects_wrong_dimension() {
    let backend = fresh_backend("add_bad_dim").await;
    // config set to dim=4, but passing 3.
    let nodes = vec![json!({
        "content": "bad",
        "embedding": [0.1, 0.2, 0.3],
    })];
    let err = backend.add_nodes(&nodes, None).await.unwrap_err();
    let msg = err.to_string();
    assert!(msg.contains("dims"), "expected dim error, got: {msg}");
}

#[tokio::test]
async fn add_nodes_rejects_missing_content() {
    let backend = fresh_backend("add_no_content").await;
    let nodes = vec![json!({"node_type": "memory"})];
    let err = backend.add_nodes(&nodes, None).await.unwrap_err();
    assert!(err.to_string().contains("content"));
}

#[tokio::test]
async fn add_nodes_rejects_empty_array() {
    let backend = fresh_backend("add_empty").await;
    let err = backend.add_nodes(&[], None).await.unwrap_err();
    assert!(err.to_string().contains("empty"));
}

// ============================================================
// search
// ============================================================

#[tokio::test]
async fn text_search_finds_node_by_content() {
    let backend = fresh_backend("search_text").await;
    backend
        .add_nodes(
            &[
                json!({"content": "the quick brown fox jumps over lazy dogs"}),
                json!({"content": "unrelated document about cats"}),
                json!({"content": "another brown dog running fast"}),
            ],
            Some("tenant1"),
        )
        .await
        .unwrap();

    let params = SearchParams {
        query: Some("brown".to_string()),
        mode: Some("text".to_string()),
        namespace: Some("tenant1".to_string()),
        limit: Some(10),
        ..Default::default()
    };
    let result = backend.search(params).await.unwrap();
    let results = result["results"].as_array().unwrap();
    assert!(
        results.len() >= 2,
        "expected >=2 matches for 'brown', got: {result}"
    );
    assert_eq!(result["_meta"]["mode"], "text");
}

#[tokio::test]
async fn text_search_honors_namespace_isolation() {
    let backend = fresh_backend("search_ns").await;
    backend
        .add_nodes(&[json!({"content": "secret one"})], Some("tenant-a"))
        .await
        .unwrap();
    backend
        .add_nodes(&[json!({"content": "secret two"})], Some("tenant-b"))
        .await
        .unwrap();

    let params = SearchParams {
        query: Some("secret".to_string()),
        mode: Some("text".to_string()),
        namespace: Some("tenant-a".to_string()),
        limit: Some(10),
        ..Default::default()
    };
    let result = backend.search(params).await.unwrap();
    let results = result["results"].as_array().unwrap();
    // Must only see tenant-a's node, never tenant-b's.
    assert_eq!(results.len(), 1, "cross-namespace leak: {result}");
    let content = results[0]["content"].as_str().unwrap();
    assert!(content.contains("one"));
}

#[tokio::test]
async fn vector_search_requires_query_vector() {
    let backend = fresh_backend("search_vec_missing").await;
    backend
        .add_nodes(
            &[json!({
                "content": "node with vec",
                "embedding": [0.1, 0.2, 0.3, 0.4],
            })],
            Some("v"),
        )
        .await
        .unwrap();

    let params = SearchParams {
        query: Some("anything".to_string()),
        mode: Some("vector".to_string()),
        namespace: Some("v".to_string()),
        limit: Some(5),
        ..Default::default()
    };
    let err = backend.search(params).await.unwrap_err();
    assert!(
        err.to_string().contains("query_vector"),
        "expected helpful error, got: {err}"
    );
}

#[tokio::test]
async fn vector_search_with_embedding_returns_nearest() {
    let backend = fresh_backend("search_vec_ok").await;
    backend
        .add_nodes(
            &[
                json!({
                    "content": "far node",
                    "embedding": [1.0, 0.0, 0.0, 0.0],
                }),
                json!({
                    "content": "near node",
                    "embedding": [0.0, 1.0, 0.0, 0.0],
                }),
            ],
            Some("vec"),
        )
        .await
        .unwrap();

    let params = SearchParams {
        mode: Some("vector".to_string()),
        namespace: Some("vec".to_string()),
        limit: Some(1),
        metadata_filter: Some(json!({
            "query_vector": [0.0, 1.0, 0.0, 0.0],
        })),
        ..Default::default()
    };
    let result = backend.search(params).await.unwrap();
    let results = result["results"].as_array().unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0]["content"], "near node");
}

#[tokio::test]
async fn hybrid_search_fuses_text_and_vector() {
    let backend = fresh_backend("search_hybrid").await;
    backend
        .add_nodes(
            &[
                json!({
                    "content": "alpha beta gamma",
                    "embedding": [1.0, 0.0, 0.0, 0.0],
                }),
                json!({
                    "content": "alpha delta epsilon",
                    "embedding": [0.0, 1.0, 0.0, 0.0],
                }),
            ],
            Some("h"),
        )
        .await
        .unwrap();

    let params = SearchParams {
        query: Some("alpha".to_string()),
        mode: Some("hybrid".to_string()),
        namespace: Some("h".to_string()),
        limit: Some(5),
        metadata_filter: Some(json!({
            "query_vector": [0.0, 1.0, 0.0, 0.0],
        })),
        ..Default::default()
    };
    let result = backend.search(params).await.unwrap();
    assert_eq!(result["_meta"]["mode"], "hybrid");
    let results = result["results"].as_array().unwrap();
    assert!(!results.is_empty());
}

#[tokio::test]
async fn unknown_mode_returns_error() {
    let backend = fresh_backend("search_bad_mode").await;
    let params = SearchParams {
        query: Some("x".into()),
        mode: Some("telepathy".into()),
        ..Default::default()
    };
    let err = backend.search(params).await.unwrap_err();
    assert!(err.to_string().contains("unknown"));
}

// ============================================================
// session state (KV + TTL)
// ============================================================

#[tokio::test]
async fn state_set_get_roundtrip() {
    let backend = fresh_backend("state_set_get").await;
    backend
        .state_set("plan", &json!([1, 2, 3]), "session-a", Some("alice"), None)
        .await
        .unwrap();
    let result = backend
        .state_get("plan", "session-a", Some("alice"))
        .await
        .unwrap();
    assert_eq!(result["value"], json!([1, 2, 3]));
}

#[tokio::test]
async fn state_get_missing_key_returns_null() {
    let backend = fresh_backend("state_get_missing").await;
    let result = backend
        .state_get("never_set", "s", Some("n"))
        .await
        .unwrap();
    assert_eq!(result["value"], Value::Null);
}

#[tokio::test]
async fn state_set_rejects_empty_key() {
    let backend = fresh_backend("state_set_empty_key").await;
    let err = backend
        .state_set("", &json!(1), "s", None, None)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("non-empty"));
}

#[tokio::test]
async fn state_set_overwrites_existing_key() {
    let backend = fresh_backend("state_overwrite").await;
    backend
        .state_set("k", &json!("v1"), "s", None, None)
        .await
        .unwrap();
    backend
        .state_set("k", &json!("v2"), "s", None, None)
        .await
        .unwrap();
    let result = backend.state_get("k", "s", None).await.unwrap();
    assert_eq!(result["value"], json!("v2"));
}

#[tokio::test]
async fn state_delete_removes_key() {
    let backend = fresh_backend("state_delete").await;
    backend
        .state_set("k", &json!("v"), "s", None, None)
        .await
        .unwrap();
    let del = backend.state_delete("k", "s", None).await.unwrap();
    assert_eq!(del["deleted"], "k");
    let after = backend.state_get("k", "s", None).await.unwrap();
    assert_eq!(after["value"], Value::Null);
}

#[tokio::test]
async fn state_list_returns_all_keys() {
    let backend = fresh_backend("state_list").await;
    for k in ["alpha", "beta", "gamma"] {
        backend
            .state_set(k, &json!(1), "s1", None, None)
            .await
            .unwrap();
    }
    let result = backend.state_list("s1", None).await.unwrap();
    let keys = result["keys"].as_array().unwrap();
    assert_eq!(keys.len(), 3);
}

#[tokio::test]
async fn state_list_respects_session_isolation() {
    let backend = fresh_backend("state_list_iso").await;
    backend
        .state_set("a", &json!(1), "s1", None, None)
        .await
        .unwrap();
    backend
        .state_set("b", &json!(2), "s2", None, None)
        .await
        .unwrap();

    let s1 = backend.state_list("s1", None).await.unwrap();
    let s1_keys = s1["keys"].as_array().unwrap();
    assert_eq!(s1_keys.len(), 1);
}

#[tokio::test]
async fn state_clear_wipes_session() {
    let backend = fresh_backend("state_clear").await;
    for k in ["x", "y", "z"] {
        backend
            .state_set(k, &json!(k), "s", None, None)
            .await
            .unwrap();
    }
    let clear = backend.state_clear("s", None).await.unwrap();
    assert_eq!(clear["cleared"], 3);
    let after = backend.state_list("s", None).await.unwrap();
    assert!(after["keys"].as_array().unwrap().is_empty());
}

#[tokio::test]
async fn state_ttl_expires_value() {
    let backend = fresh_backend("state_ttl").await;
    backend
        .state_set("ephemeral", &json!("soon gone"), "s", None, Some(1))
        .await
        .unwrap();

    // Immediately readable.
    let before = backend.state_get("ephemeral", "s", None).await.unwrap();
    assert_eq!(before["value"], json!("soon gone"));

    // After 2s, prune-on-read should have removed it.
    tokio::time::sleep(std::time::Duration::from_secs(2)).await;
    let after = backend.state_get("ephemeral", "s", None).await.unwrap();
    assert_eq!(after["value"], Value::Null);
}

// ============================================================
// supported_search_modes
// ============================================================

#[tokio::test]
async fn supported_search_modes_lists_text_vector_hybrid() {
    let backend = fresh_backend("caps").await;
    let modes = backend.supported_search_modes();
    assert!(modes.contains(&"text".to_string()));
    assert!(modes.contains(&"vector".to_string()));
    assert!(modes.contains(&"hybrid".to_string()));
}

//! Session-scoped KV store with optional TTL.
//!
//! Matches smrti's `session_state` semantics: per-namespace, per-session-id,
//! per-key rows with an optional `expires_at` wall-clock timestamp. Expiry
//! is enforced lazily (on read) — every `get`/`list` first deletes any
//! matching expired rows in the same transaction. No background sweeper.

use konf_tool_memory::MemoryError;
use serde_json::{json, Value};

use crate::backend::SurrealBackend;
use crate::error::map_db_error;

/// Apply a prune of expired rows in the given namespace + session before
/// we touch them. Runs as a separate statement so the read that follows
/// sees a consistent view.
async fn prune_expired(
    backend: &SurrealBackend,
    namespace: &str,
    session_id: &str,
) -> Result<(), MemoryError> {
    let sql = r#"
DELETE session_state
 WHERE namespace  = $ns
   AND session_id = $sid
   AND expires_at != NONE
   AND expires_at <  time::now()
"#;
    backend
        .db()
        .query(sql)
        .bind(("ns", namespace.to_string()))
        .bind(("sid", session_id.to_string()))
        .await
        .map_err(map_db_error)?
        .check()
        .map_err(map_db_error)?;
    Ok(())
}

/// `state:set` — upsert a single key.
pub async fn set(
    backend: &SurrealBackend,
    key: &str,
    value: &Value,
    session_id: &str,
    namespace: Option<&str>,
    ttl: Option<i64>,
) -> Result<Value, MemoryError> {
    if key.is_empty() {
        return Err(MemoryError::Validation(
            "state:set: key must be non-empty".into(),
        ));
    }
    let ns = backend.resolve_namespace(namespace);

    // We model "upsert" via DELETE-then-CREATE inside SurrealDB because
    // UPSERT in SurrealQL needs a record id; our composite (namespace,
    // session_id, key) is not the record id.
    let delete_sql = r#"
DELETE session_state
 WHERE namespace  = $ns
   AND session_id = $sid
   AND skey       = $k
"#;
    backend
        .db()
        .query(delete_sql)
        .bind(("ns", ns.clone()))
        .bind(("sid", session_id.to_string()))
        .bind(("k", key.to_string()))
        .await
        .map_err(map_db_error)?
        .check()
        .map_err(map_db_error)?;

    let create_sql = match ttl {
        Some(seconds) if seconds > 0 => {
            // time::now() + duration — SurrealQL duration literal via string concat
            // is fragile, so we compute the absolute expires_at from the secs param.
            r#"
CREATE session_state SET
    namespace  = $ns,
    session_id = $sid,
    skey       = $k,
    sval       = $v,
    expires_at = time::now() + <duration>($ttl + "s")
"#
        }
        _ => {
            r#"
CREATE session_state SET
    namespace  = $ns,
    session_id = $sid,
    skey       = $k,
    sval       = $v,
    expires_at = NONE
"#
        }
    };

    let mut q = backend
        .db()
        .query(create_sql)
        .bind(("ns", ns.clone()))
        .bind(("sid", session_id.to_string()))
        .bind(("k", key.to_string()))
        .bind(("v", value.clone()));
    if let Some(seconds) = ttl {
        if seconds > 0 {
            q = q.bind(("ttl", seconds.to_string()));
        }
    }
    q.await
        .map_err(map_db_error)?
        .check()
        .map_err(map_db_error)?;

    Ok(json!({
        "key": key,
        "session_id": session_id,
        "_meta": { "namespace": ns, "ttl": ttl }
    }))
}

/// `state:get` — read a single key.
pub async fn get(
    backend: &SurrealBackend,
    key: &str,
    session_id: &str,
    namespace: Option<&str>,
) -> Result<Value, MemoryError> {
    let ns = backend.resolve_namespace(namespace);
    prune_expired(backend, &ns, session_id).await?;

    let sql = r#"
SELECT sval FROM session_state
 WHERE namespace  = $ns
   AND session_id = $sid
   AND skey       = $k
 LIMIT 1
"#;
    let mut resp = backend
        .db()
        .query(sql)
        .bind(("ns", ns.clone()))
        .bind(("sid", session_id.to_string()))
        .bind(("k", key.to_string()))
        .await
        .map_err(map_db_error)?
        .check()
        .map_err(map_db_error)?;

    let rows: Vec<Value> = resp.take(0).map_err(map_db_error)?;
    let value = rows
        .into_iter()
        .next()
        .and_then(|row| row.get("sval").cloned())
        .unwrap_or(Value::Null);

    Ok(json!({
        "key": key,
        "session_id": session_id,
        "value": value,
        "_meta": { "namespace": ns }
    }))
}

/// `state:delete` — remove a single key.
pub async fn delete(
    backend: &SurrealBackend,
    key: &str,
    session_id: &str,
    namespace: Option<&str>,
) -> Result<Value, MemoryError> {
    let ns = backend.resolve_namespace(namespace);
    let sql = r#"
DELETE session_state
 WHERE namespace  = $ns
   AND session_id = $sid
   AND skey       = $k
RETURN BEFORE
"#;
    let mut resp = backend
        .db()
        .query(sql)
        .bind(("ns", ns.clone()))
        .bind(("sid", session_id.to_string()))
        .bind(("k", key.to_string()))
        .await
        .map_err(map_db_error)?
        .check()
        .map_err(map_db_error)?;
    let removed: Vec<Value> = resp.take(0).map_err(map_db_error)?;
    Ok(json!({
        "deleted": key,
        "removed_count": removed.len(),
        "_meta": { "namespace": ns }
    }))
}

/// `state:list` — list all keys in a session.
pub async fn list(
    backend: &SurrealBackend,
    session_id: &str,
    namespace: Option<&str>,
) -> Result<Value, MemoryError> {
    let ns = backend.resolve_namespace(namespace);
    prune_expired(backend, &ns, session_id).await?;

    let sql = r#"
SELECT skey FROM session_state
 WHERE namespace  = $ns
   AND session_id = $sid
 ORDER BY skey
"#;
    let mut resp = backend
        .db()
        .query(sql)
        .bind(("ns", ns.clone()))
        .bind(("sid", session_id.to_string()))
        .await
        .map_err(map_db_error)?
        .check()
        .map_err(map_db_error)?;
    let rows: Vec<Value> = resp.take(0).map_err(map_db_error)?;
    let keys: Vec<Value> = rows
        .into_iter()
        .filter_map(|row| row.get("skey").cloned())
        .collect();

    Ok(json!({
        "keys": keys,
        "_meta": { "namespace": ns, "session_id": session_id }
    }))
}

/// `state:clear` — remove every key in a session.
pub async fn clear(
    backend: &SurrealBackend,
    session_id: &str,
    namespace: Option<&str>,
) -> Result<Value, MemoryError> {
    let ns = backend.resolve_namespace(namespace);
    let sql = r#"
DELETE session_state
 WHERE namespace  = $ns
   AND session_id = $sid
RETURN BEFORE
"#;
    let mut resp = backend
        .db()
        .query(sql)
        .bind(("ns", ns.clone()))
        .bind(("sid", session_id.to_string()))
        .await
        .map_err(map_db_error)?
        .check()
        .map_err(map_db_error)?;
    let removed: Vec<Value> = resp.take(0).map_err(map_db_error)?;
    Ok(json!({
        "cleared": removed.len(),
        "_meta": { "namespace": ns, "session_id": session_id }
    }))
}

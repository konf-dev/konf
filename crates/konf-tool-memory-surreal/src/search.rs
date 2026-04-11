//! Memory search dispatch: text, vector, hybrid.
//!
//! **Text mode** uses SurrealDB's FULLTEXT BM25 index built over `node.content`
//! with the `konf_ft` analyzer. Scoring via `search::score`.
//!
//! **Vector mode** uses the HNSW index on `node_embedding.embedding`. The
//! caller must supply a pre-computed query vector; this backend does not embed
//! text itself. The escape hatch lives in `SearchParams.metadata_filter`:
//! when it contains a `query_vector: [f64; N]` array, vector mode becomes
//! available. Without it, vector mode returns `MemoryError::Validation`.
//!
//! **Hybrid mode** fuses text and vector results with Reciprocal Rank Fusion
//! (k = config.rrf_k, default 60). Both a text query and a pre-computed
//! `query_vector` must be supplied; if either is missing, hybrid degrades
//! gracefully to whichever single mode has its input.

use konf_tool_memory::{MemoryError, SearchParams};
use serde_json::{json, Value};

use crate::backend::SurrealBackend;
use crate::error::map_db_error;

/// Public entry — dispatches on `params.mode`.
pub async fn run(backend: &SurrealBackend, params: SearchParams) -> Result<Value, MemoryError> {
    let mode = params.mode.as_deref().unwrap_or("text").to_lowercase();

    let query_vector = extract_query_vector(&params, backend)?;

    match mode.as_str() {
        "text" => text_search(backend, &params).await,
        "vector" => vector_search(backend, &params, query_vector).await,
        "hybrid" => hybrid_search(backend, &params, query_vector).await,
        other => Err(MemoryError::Unsupported(format!(
            "unknown search mode `{other}` — supported: text, vector, hybrid"
        ))),
    }
}

/// Pull `query_vector` out of `params.metadata_filter` and validate its
/// dimension against the config. Returns `None` if not present. Returns
/// `Err(Validation)` if present but malformed or wrong length.
fn extract_query_vector(
    params: &SearchParams,
    backend: &SurrealBackend,
) -> Result<Option<Vec<f64>>, MemoryError> {
    let Some(mf) = params.metadata_filter.as_ref() else {
        return Ok(None);
    };
    let Some(arr) = mf.get("query_vector").and_then(Value::as_array) else {
        return Ok(None);
    };
    let vec: Vec<f64> = arr.iter().filter_map(Value::as_f64).collect();
    if vec.len() != arr.len() {
        return Err(MemoryError::Validation(
            "metadata_filter.query_vector must be an array of numbers".into(),
        ));
    }
    if vec.len() != backend.cfg().vector_dimension {
        return Err(MemoryError::Validation(format!(
            "query_vector has {} dims, expected {}",
            vec.len(),
            backend.cfg().vector_dimension
        )));
    }
    Ok(Some(vec))
}

async fn text_search(
    backend: &SurrealBackend,
    params: &SearchParams,
) -> Result<Value, MemoryError> {
    let query_text = params
        .query
        .as_deref()
        .ok_or_else(|| MemoryError::Validation("text search requires `query`".into()))?;

    let ns = backend.resolve_namespace(params.namespace.as_deref());
    let limit = params.limit.unwrap_or(backend.cfg().default_limit).max(1);

    let mut sql = String::from(
        r#"
SELECT id, node_type, content, metadata, search::score(0) AS score
  FROM node
 WHERE namespace = $ns
   AND is_retracted = false
   AND content @0@ $q
"#,
    );
    if params.node_type.is_some() {
        sql.push_str("   AND node_type = $nt\n");
    }
    sql.push_str(" ORDER BY score DESC\n LIMIT $lim;\n");

    let mut q = backend
        .db()
        .query(&sql)
        .bind(("ns", ns.clone()))
        .bind(("q", query_text.to_string()))
        .bind(("lim", limit));
    if let Some(nt) = params.node_type.clone() {
        q = q.bind(("nt", nt));
    }
    let mut resp = q
        .await
        .map_err(map_db_error)?
        .check()
        .map_err(map_db_error)?;
    let rows: Vec<Value> = resp.take(0).map_err(map_db_error)?;

    Ok(json!({
        "results": rows,
        "_meta": { "mode": "text", "namespace": ns, "count": resp_count(&Value::Null) }
    })
    .merged_count())
}

async fn vector_search(
    backend: &SurrealBackend,
    params: &SearchParams,
    query_vector: Option<Vec<f64>>,
) -> Result<Value, MemoryError> {
    let qv = query_vector.ok_or_else(|| {
        MemoryError::Validation(
            "vector search requires `metadata_filter.query_vector` (pre-embedded query)".into(),
        )
    })?;
    let ns = backend.resolve_namespace(params.namespace.as_deref());
    let limit = params.limit.unwrap_or(backend.cfg().default_limit).max(1);
    let ef = backend.cfg().hybrid_candidate_pool.max(limit);

    // SurrealDB HNSW KNN operator: <|K,EF|> returns K nearest, visiting EF.
    // We can't parameterize K and EF — they must be in the query text.
    let sql = format!(
        r#"
SELECT
    node.id           AS id,
    node.node_type    AS node_type,
    node.content      AS content,
    node.metadata     AS metadata,
    vector::similarity::cosine(embedding, $qv) AS score
  FROM node_embedding
 WHERE namespace = $ns
   AND embedding <|{k},{ef}|> $qv
 ORDER BY score DESC
 LIMIT $lim;
"#,
        k = limit,
        ef = ef
    );

    let mut resp = backend
        .db()
        .query(&sql)
        .bind(("ns", ns.clone()))
        .bind(("qv", qv))
        .bind(("lim", limit))
        .await
        .map_err(map_db_error)?
        .check()
        .map_err(map_db_error)?;
    let rows: Vec<Value> = resp.take(0).map_err(map_db_error)?;
    let count = rows.len();

    Ok(json!({
        "results": rows,
        "_meta": { "mode": "vector", "namespace": ns, "count": count }
    }))
}

async fn hybrid_search(
    backend: &SurrealBackend,
    params: &SearchParams,
    query_vector: Option<Vec<f64>>,
) -> Result<Value, MemoryError> {
    let text_q = params.query.as_deref();
    match (text_q, query_vector) {
        (None, None) => Err(MemoryError::Validation(
            "hybrid search requires either `query` or `metadata_filter.query_vector` or both"
                .into(),
        )),
        (Some(_), None) => text_search(backend, params).await,
        (None, Some(qv)) => vector_search(backend, params, Some(qv)).await,
        (Some(_), Some(qv)) => rrf_fuse(backend, params, qv).await,
    }
}

/// Reciprocal Rank Fusion over two ranked lists: vector results and text results.
///
/// Runs both sub-queries against the same SurrealDB handle, materializes the
/// two ranked lists in Rust, and fuses with `score = Σ 1/(k + rank_i)`. This
/// avoids SurrealQL complexity at the cost of two round-trips; for v1 the
/// simplicity is worth it. If latency becomes a concern later, this can be
/// pushed into a single SurrealQL statement.
async fn rrf_fuse(
    backend: &SurrealBackend,
    params: &SearchParams,
    query_vector: Vec<f64>,
) -> Result<Value, MemoryError> {
    let ns = backend.resolve_namespace(params.namespace.as_deref());
    let limit = params.limit.unwrap_or(backend.cfg().default_limit).max(1);
    let pool = backend.cfg().hybrid_candidate_pool.max(limit);
    let rrf_k = backend.cfg().rrf_k;

    // Candidate pool per mode. We ask for `pool` results from each side so
    // RRF has room to reorder.
    let mut text_params = params.clone();
    text_params.limit = Some(pool);
    let text_json = text_search(backend, &text_params).await?;

    let mut vec_params = params.clone();
    vec_params.limit = Some(pool);
    let vec_json = vector_search(backend, &vec_params, Some(query_vector)).await?;

    let text_rows = extract_results(&text_json);
    let vec_rows = extract_results(&vec_json);

    // id -> (rrf_score, row)
    let mut fused: std::collections::HashMap<String, (f64, Value)> =
        std::collections::HashMap::new();

    for (rank, row) in text_rows.iter().enumerate() {
        let id = row_id(row);
        let contribution = 1.0 / (rrf_k as f64 + (rank as f64 + 1.0));
        fused
            .entry(id)
            .and_modify(|(s, _)| *s += contribution)
            .or_insert((contribution, row.clone()));
    }
    for (rank, row) in vec_rows.iter().enumerate() {
        let id = row_id(row);
        let contribution = 1.0 / (rrf_k as f64 + (rank as f64 + 1.0));
        fused
            .entry(id)
            .and_modify(|(s, _)| *s += contribution)
            .or_insert((contribution, row.clone()));
    }

    let mut ranked: Vec<(f64, Value)> = fused.into_values().collect();
    ranked.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
    let results: Vec<Value> = ranked
        .into_iter()
        .take(limit as usize)
        .map(|(score, mut row)| {
            if let Some(obj) = row.as_object_mut() {
                obj.insert("score".into(), json!(score));
            }
            row
        })
        .collect();

    Ok(json!({
        "results": results.clone(),
        "_meta": {
            "mode": "hybrid",
            "namespace": ns,
            "count": results.len(),
            "rrf_k": rrf_k,
            "pool": pool,
        }
    }))
}

fn extract_results(envelope: &Value) -> Vec<Value> {
    envelope
        .get("results")
        .and_then(Value::as_array)
        .cloned()
        .unwrap_or_default()
}

fn row_id(row: &Value) -> String {
    row.get("id").map(|v| v.to_string()).unwrap_or_default()
}

#[allow(dead_code)]
fn resp_count(_v: &Value) -> usize {
    0
}

// Small trait extension so text_search can inject a count cleanly.
trait MergedCount {
    fn merged_count(self) -> Self;
}
impl MergedCount for Value {
    fn merged_count(mut self) -> Self {
        if let Some(obj) = self.as_object_mut() {
            if let Some(results) = obj.get("results").cloned() {
                let len = results.as_array().map(|a| a.len()).unwrap_or(0);
                if let Some(meta) = obj.get_mut("_meta").and_then(Value::as_object_mut) {
                    meta.insert("count".into(), json!(len));
                }
            }
        }
        self
    }
}

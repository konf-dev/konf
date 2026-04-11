//! End-to-end experiment for the SurrealDB memory backend.
//!
//! Drives every `MemoryBackend` trait method against a **real embedded
//! RocksDB file** (not the in-memory engine used by unit tests), then
//! re-opens the file in a fresh process to prove on-disk persistence. It is
//! the load-bearing "does it work end to end" check for Phase 2 of plan
//! `serene-tumbling-gizmo`.
//!
//! ## Running
//!
//! ```bash
//! cargo run -p konf-tool-memory-surreal --example experiment
//! ```
//!
//! The experiment creates a temporary directory for the RocksDB file, runs
//! two subprocess phases against it, and deletes the directory on success.
//! If any step fails, the directory is left in place so operators can
//! inspect it manually.
//!
//! ## Why subprocesses
//!
//! SurrealDB's `kv-rocksdb` engine holds the RocksDB file lock for the
//! lifetime of the process. Dropping the `Surreal<Any>` handle from the
//! parent does not release the lock, because internal Arc clones inside
//! the engine keep the underlying `DB` alive until the process exits. The
//! honest way to verify "two independent opens of the same on-disk file"
//! is to actually use two processes. The orchestrator below holds **no**
//! database handle; it only spawns `write` and `verify` child processes
//! and checks their exit codes.
//!
//! ## Phases
//!
//! | Phase    | What runs                                                    |
//! |----------|--------------------------------------------------------------|
//! | write    | connect → add_nodes (plain + embedding) → namespace writes → |
//! |          | session KV set/get/list/delete/clear → TTL expiry →          |
//! |          | text + vector + hybrid RRF searches                          |
//! | verify   | reopen the same RocksDB file, run the same searches, assert |
//! |          | the data written in `write` is still there                   |

use std::env;
use std::process::Command;
use std::time::Duration;

use konf_tool_memory::{MemoryBackend, SearchParams};
use konf_tool_memory_surreal::connect;
use serde_json::{json, Value};

const PHASE_ENV: &str = "KONF_SURREAL_EXPERIMENT_PHASE";
const PATH_ENV: &str = "KONF_SURREAL_EXPERIMENT_PATH";
const TENANT: &str = "tenant-primary";

fn main() -> anyhow::Result<()> {
    // Dispatch on phase without a tokio runtime at the top level — each
    // phase builds its own runtime so nothing async survives past the
    // phase boundary. This is what makes the reopen cross-process check
    // honest: the parent never holds a DB handle.
    match env::var(PHASE_ENV).ok().as_deref() {
        Some("write") => run_async(write_phase()),
        Some("verify") => run_async(verify_phase()),
        _ => orchestrate(),
    }
}

fn run_async<F>(fut: F) -> anyhow::Result<()>
where
    F: std::future::Future<Output = anyhow::Result<()>>,
{
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?;
    rt.block_on(fut)
}

/// Parent phase: no DB handles, just two subprocess spawns.
fn orchestrate() -> anyhow::Result<()> {
    let tmp = tempfile::tempdir()?;
    let db_path = tmp.path().join("experiment.db");
    let db_path_str = db_path.to_string_lossy().to_string();

    println!("experiment: RocksDB path = {db_path_str}");
    println!("experiment: phase 1 of 2 — write");
    spawn_phase("write", &db_path_str)?;

    println!("experiment: phase 2 of 2 — verify (reopen in fresh process)");
    spawn_phase("verify", &db_path_str)?;

    println!("\nexperiment: ALL PHASES PASSED");
    drop(tmp);
    Ok(())
}

fn spawn_phase(phase: &str, path: &str) -> anyhow::Result<()> {
    let exe = env::current_exe()?;
    let status = Command::new(exe)
        .env(PHASE_ENV, phase)
        .env(PATH_ENV, path)
        .status()?;
    if !status.success() {
        anyhow::bail!(
            "experiment phase `{phase}` failed with exit code {:?}",
            status.code()
        );
    }
    Ok(())
}

/// Child process: open a fresh DB, run every write/search operation that
/// Phase 2 of the plan promises, exit 0 on success.
async fn write_phase() -> anyhow::Result<()> {
    let path = env::var(PATH_ENV)?;
    let backend = open(&path).await?;
    let mut report = Report::new("write");

    // 01 — schema applies on fresh RocksDB.
    report.step("01 connect", true, "schema applied on fresh RocksDB");

    // 02 — plain nodes + event append.
    let added_plain = backend
        .add_nodes(
            &[
                json!({"content": "the quick brown fox"}),
                json!({"content": "a lazy dog sleeps"}),
                json!({"content": "another brown animal"}),
            ],
            Some(TENANT),
        )
        .await?;
    report.step(
        "02 add_nodes (plain)",
        added_plain["added"] == 3,
        &format!("added={}", added_plain["added"]),
    );

    // 03 — nodes with embeddings. Each row has a unique distinguishing
    // word so later searches can target exactly one.
    let added_embed = backend
        .add_nodes(
            &[
                json!({
                    "content": "payload alpha north",
                    "embedding": [1.0, 0.0, 0.0, 0.0],
                    "model_name": "experiment-embedder",
                }),
                json!({
                    "content": "payload beta east",
                    "embedding": [0.0, 1.0, 0.0, 0.0],
                    "model_name": "experiment-embedder",
                }),
                json!({
                    "content": "payload gamma south",
                    "embedding": [0.0, 0.0, 1.0, 0.0],
                    "model_name": "experiment-embedder",
                }),
            ],
            Some(TENANT),
        )
        .await?;
    report.step(
        "03 add_nodes (with embedding)",
        added_embed["added"] == 3,
        &format!("added={}", added_embed["added"]),
    );

    // 04 — text search via BM25.
    let text_hit = search_text(&backend, "brown", 10).await?;
    let text_count = text_hit["results"].as_array().map(|a| a.len()).unwrap_or(0);
    report.step(
        "04 text search (BM25)",
        text_count >= 2,
        &format!("matches={text_count} (expected >=2)"),
    );

    // 05 — vector search: nearest to [0,1,0,0] is "payload beta east".
    let vec_top = search_vector_top(&backend, [0.0, 1.0, 0.0, 0.0]).await?;
    report.step(
        "05 vector search (HNSW cosine)",
        vec_top == "payload beta east",
        &format!("top={vec_top}"),
    );

    // 06 — hybrid RRF: text "south" matches only gamma; query vector
    // [0,0,1,0] also ranks gamma first. Both signals agree → gamma must
    // be at position 0 regardless of tie-breaking.
    let hybrid_first = search_hybrid_top(&backend, "south", [0.0, 0.0, 1.0, 0.0]).await?;
    report.step(
        "06 hybrid RRF",
        hybrid_first == "payload gamma south",
        &format!("top={hybrid_first}"),
    );

    // 07 — namespace isolation. Store a secret in ns-2, search from ns-1.
    backend
        .add_nodes(
            &[json!({"content": "ns-2 secret"})],
            Some("tenant-secondary"),
        )
        .await?;
    let ns_leak = search_text(&backend, "secret", 10).await?;
    let leaked = ns_leak["results"]
        .as_array()
        .map(|rows| {
            rows.iter().any(|r| {
                r.get("content")
                    .and_then(Value::as_str)
                    .unwrap_or("")
                    .contains("ns-2")
            })
        })
        .unwrap_or(false);
    report.step(
        "07 namespace isolation",
        !leaked,
        "ns-1 query did not return ns-2 rows",
    );

    // 08 — session KV set/get roundtrip.
    backend
        .state_set(
            "plan",
            &json!([1, 2, 3, 4]),
            "experiment-session",
            Some(TENANT),
            None,
        )
        .await?;
    let got = backend
        .state_get("plan", "experiment-session", Some(TENANT))
        .await?;
    report.step(
        "08 state_set+get",
        got["value"] == json!([1, 2, 3, 4]),
        &format!("value={}", got["value"]),
    );

    // 09 — state_list returns the single key.
    let listed = backend
        .state_list("experiment-session", Some(TENANT))
        .await?;
    let list_len = listed["keys"].as_array().map(|a| a.len()).unwrap_or(0);
    report.step("09 state_list", list_len == 1, &format!("keys={list_len}"));

    // 10 — TTL expiry.
    backend
        .state_set(
            "ephemeral",
            &json!("soon gone"),
            "experiment-session",
            Some(TENANT),
            Some(1),
        )
        .await?;
    tokio::time::sleep(Duration::from_secs(2)).await;
    let expired = backend
        .state_get("ephemeral", "experiment-session", Some(TENANT))
        .await?;
    report.step(
        "10 state TTL expiry",
        expired["value"] == Value::Null,
        &format!("value_after_ttl={}", expired["value"]),
    );

    // 11 — state_delete + state_clear.
    backend
        .state_delete("plan", "experiment-session", Some(TENANT))
        .await?;
    backend
        .state_set(
            "residual",
            &json!("a"),
            "experiment-session",
            Some(TENANT),
            None,
        )
        .await?;
    let cleared = backend
        .state_clear("experiment-session", Some(TENANT))
        .await?;
    let left = backend
        .state_list("experiment-session", Some(TENANT))
        .await?;
    let left_count = left["keys"].as_array().map(|a| a.len()).unwrap_or(0);
    report.step(
        "11 state_delete + state_clear",
        cleared["cleared"].as_u64().unwrap_or(0) >= 1 && left_count == 0,
        &format!(
            "cleared={} residual_keys={}",
            cleared["cleared"], left_count
        ),
    );

    report.finish()
}

/// Child process: reopen the RocksDB file and prove the write phase's data
/// survived the restart.
async fn verify_phase() -> anyhow::Result<()> {
    let path = env::var(PATH_ENV)?;
    let backend = open(&path).await?;
    let mut report = Report::new("verify");

    // 12 — the "brown" text search still returns the two rows written in
    // phase 1. This is the load-bearing persistence check.
    let after = search_text(&backend, "brown", 10).await?;
    let after_count = after["results"].as_array().map(|a| a.len()).unwrap_or(0);
    report.step(
        "12 reopen + text search",
        after_count >= 2,
        &format!("rows_after_reopen={after_count}"),
    );

    // 13 — vector search still finds the right neighbor.
    let vec_top = search_vector_top(&backend, [0.0, 1.0, 0.0, 0.0]).await?;
    report.step(
        "13 reopen + vector search",
        vec_top == "payload beta east",
        &format!("top={vec_top}"),
    );

    // 14 — hybrid RRF still produces the same answer.
    let hybrid_first = search_hybrid_top(&backend, "south", [0.0, 0.0, 1.0, 0.0]).await?;
    report.step(
        "14 reopen + hybrid RRF",
        hybrid_first == "payload gamma south",
        &format!("top={hybrid_first}"),
    );

    report.finish()
}

// ------------------------------------------------------------------
// helpers
// ------------------------------------------------------------------

async fn open(path: &str) -> anyhow::Result<std::sync::Arc<dyn MemoryBackend>> {
    let cfg = json!({
        "mode": "embedded",
        "path": path,
        "namespace": "experiment",
        "database": "default",
        "vector_dimension": 4,
    });
    connect(&cfg).await
}

async fn search_text(
    backend: &std::sync::Arc<dyn MemoryBackend>,
    query: &str,
    limit: i64,
) -> anyhow::Result<Value> {
    backend
        .search(SearchParams {
            query: Some(query.into()),
            mode: Some("text".into()),
            namespace: Some(TENANT.into()),
            limit: Some(limit),
            ..Default::default()
        })
        .await
        .map_err(|e| anyhow::anyhow!("text search failed: {e}"))
}

async fn search_vector_top(
    backend: &std::sync::Arc<dyn MemoryBackend>,
    query_vector: [f64; 4],
) -> anyhow::Result<String> {
    let hit = backend
        .search(SearchParams {
            mode: Some("vector".into()),
            namespace: Some(TENANT.into()),
            limit: Some(1),
            metadata_filter: Some(json!({ "query_vector": query_vector })),
            ..Default::default()
        })
        .await
        .map_err(|e| anyhow::anyhow!("vector search failed: {e}"))?;
    Ok(hit["results"]
        .get(0)
        .and_then(|r| r.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string())
}

async fn search_hybrid_top(
    backend: &std::sync::Arc<dyn MemoryBackend>,
    text: &str,
    query_vector: [f64; 4],
) -> anyhow::Result<String> {
    let hit = backend
        .search(SearchParams {
            query: Some(text.into()),
            mode: Some("hybrid".into()),
            namespace: Some(TENANT.into()),
            limit: Some(3),
            metadata_filter: Some(json!({ "query_vector": query_vector })),
            ..Default::default()
        })
        .await
        .map_err(|e| anyhow::anyhow!("hybrid search failed: {e}"))?;
    Ok(hit["results"]
        .get(0)
        .and_then(|r| r.get("content"))
        .and_then(Value::as_str)
        .unwrap_or("")
        .to_string())
}

struct Report {
    phase: &'static str,
    passed: usize,
    failed: usize,
}

impl Report {
    fn new(phase: &'static str) -> Self {
        Self {
            phase,
            passed: 0,
            failed: 0,
        }
    }
    fn step(&mut self, name: &str, ok: bool, detail: &str) {
        if ok {
            self.passed += 1;
            println!("  PASS  [{}] {name}  {detail}", self.phase);
        } else {
            self.failed += 1;
            println!("  FAIL  [{}] {name}  {detail}", self.phase);
        }
    }
    fn finish(self) -> anyhow::Result<()> {
        println!(
            "phase {}: {} passed, {} failed",
            self.phase, self.passed, self.failed
        );
        if self.failed > 0 {
            std::process::exit(1);
        }
        Ok(())
    }
}

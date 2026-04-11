//! SurrealQL schema definitions applied at `connect()` time.
//!
//! All `DEFINE` statements use `IF NOT EXISTS` so applying the schema is
//! idempotent across restarts. The shape mirrors smrti's Postgres schema
//! (`smrti/smrti-core/src/sql/migrations/v001_initial.sql` and `v002_session_state.sql`)
//! conceptually, but uses SurrealDB's native primitives: typed relation tables
//! for edges, HNSW index for vectors, FULLTEXT analyzer for text.

use crate::config::SurrealConfig;

/// Build the full schema for a given config. Returns one SurrealQL statement
/// string suitable for passing to `db.query(...)`.
///
/// The returned string embeds the vector dimension from the config so every
/// product can pin its own embedding size. All other tunables (RRF k, limits)
/// are applied at query time, not schema time.
pub fn build_schema(config: &SurrealConfig) -> String {
    let dim = config.vector_dimension;
    format!(
        r#"
-- =================================================================
-- nodes: the knowledge-graph vertex table
-- =================================================================
DEFINE TABLE IF NOT EXISTS node SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS namespace    ON node TYPE string;
DEFINE FIELD IF NOT EXISTS node_type    ON node TYPE string DEFAULT "memory";
DEFINE FIELD IF NOT EXISTS content      ON node TYPE string;
DEFINE FIELD IF NOT EXISTS metadata     ON node TYPE object FLEXIBLE DEFAULT {{}};
DEFINE FIELD IF NOT EXISTS is_retracted ON node TYPE bool DEFAULT false;
DEFINE FIELD IF NOT EXISTS created_at   ON node TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS updated_at   ON node TYPE datetime VALUE time::now();

DEFINE INDEX IF NOT EXISTS node_namespace      ON node FIELDS namespace;
DEFINE INDEX IF NOT EXISTS node_namespace_type ON node FIELDS namespace, node_type;

-- Full-text BM25 index on node content.
-- SurrealDB 3.x: use FULLTEXT (the SEARCH form is legacy).
DEFINE ANALYZER IF NOT EXISTS konf_ft TOKENIZERS class FILTERS lowercase, ascii, snowball(english);
DEFINE INDEX IF NOT EXISTS node_content_fts ON node FIELDS content FULLTEXT ANALYZER konf_ft BM25 HIGHLIGHTS;

-- =================================================================
-- node_embedding: multi-model embeddings, separate from node rows
-- =================================================================
-- One row per (node, model_name). This matches smrti's design so a single
-- product can carry multiple embedding spaces (e.g. BGE + nomic) side by side.
DEFINE TABLE IF NOT EXISTS node_embedding SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS node       ON node_embedding TYPE record<node>;
DEFINE FIELD IF NOT EXISTS namespace  ON node_embedding TYPE string;
DEFINE FIELD IF NOT EXISTS model_name ON node_embedding TYPE string;
DEFINE FIELD IF NOT EXISTS embedding  ON node_embedding TYPE array<float>;
DEFINE FIELD IF NOT EXISTS created_at ON node_embedding TYPE datetime DEFAULT time::now();

DEFINE INDEX IF NOT EXISTS node_embedding_unique
    ON node_embedding FIELDS node, model_name UNIQUE;

-- HNSW vector index — enables <|K,EF|> KNN queries.
DEFINE INDEX IF NOT EXISTS node_embedding_hnsw
    ON node_embedding FIELDS embedding
    HNSW DIMENSION {dim} DIST COSINE;

-- =================================================================
-- edge: typed, temporal relation between two nodes
-- =================================================================
DEFINE TABLE IF NOT EXISTS edge TYPE RELATION FROM node TO node SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS edge_type    ON edge TYPE string;
DEFINE FIELD IF NOT EXISTS namespace    ON edge TYPE string;
DEFINE FIELD IF NOT EXISTS metadata     ON edge TYPE object FLEXIBLE DEFAULT {{}};
DEFINE FIELD IF NOT EXISTS valid_from   ON edge TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS valid_to     ON edge TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS is_retracted ON edge TYPE bool DEFAULT false;

DEFINE INDEX IF NOT EXISTS edge_namespace_type ON edge FIELDS namespace, edge_type;

-- =================================================================
-- event: append-only audit trail
-- =================================================================
-- Every mutation (add node, add edge, retract, state_set, ...) appends a row
-- here in the same transaction as the mutation itself. Matches smrti's
-- event-sourced shape without pretending to be a full event store.
DEFINE TABLE IF NOT EXISTS event SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS namespace  ON event TYPE string;
DEFINE FIELD IF NOT EXISTS event_type ON event TYPE string;
DEFINE FIELD IF NOT EXISTS payload    ON event TYPE object FLEXIBLE DEFAULT {{}};
DEFINE FIELD IF NOT EXISTS created_at ON event TYPE datetime DEFAULT time::now();

DEFINE INDEX IF NOT EXISTS event_namespace_created
    ON event FIELDS namespace, created_at;

-- =================================================================
-- session_state: session-scoped KV with optional TTL
-- =================================================================
-- Prune happens on-read in state_get/state_list — no background sweeper.
DEFINE TABLE IF NOT EXISTS session_state SCHEMAFULL;
DEFINE FIELD IF NOT EXISTS namespace  ON session_state TYPE string;
DEFINE FIELD IF NOT EXISTS session_id ON session_state TYPE string;
DEFINE FIELD IF NOT EXISTS skey       ON session_state TYPE string;
DEFINE FIELD IF NOT EXISTS sval       ON session_state TYPE any;
DEFINE FIELD IF NOT EXISTS expires_at ON session_state TYPE option<datetime>;
DEFINE FIELD IF NOT EXISTS created_at ON session_state TYPE datetime DEFAULT time::now();
DEFINE FIELD IF NOT EXISTS updated_at ON session_state TYPE datetime VALUE time::now();

DEFINE INDEX IF NOT EXISTS session_state_pk
    ON session_state FIELDS namespace, session_id, skey UNIQUE;
"#
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::SurrealMode;

    fn test_config() -> SurrealConfig {
        SurrealConfig {
            mode: SurrealMode::Memory,
            path: None,
            endpoint: None,
            username: None,
            password: None,
            namespace: "konf".into(),
            database: "default".into(),
            vector_dimension: 384,
            rrf_k: 60,
            hybrid_candidate_pool: 100,
            default_limit: 10,
        }
    }

    #[test]
    fn schema_embeds_vector_dimension() {
        let sql = build_schema(&test_config());
        assert!(sql.contains("DIMENSION 384"));
        assert!(sql.contains("DIST COSINE"));
    }

    #[test]
    fn schema_uses_if_not_exists_everywhere() {
        let sql = build_schema(&test_config());
        // Every DEFINE statement must be idempotent.
        let define_count = sql.matches("DEFINE ").count();
        let ine_count = sql.matches("IF NOT EXISTS").count();
        assert_eq!(define_count, ine_count);
    }

    #[test]
    fn schema_defines_all_tables() {
        let sql = build_schema(&test_config());
        for t in &["node", "node_embedding", "edge", "event", "session_state"] {
            assert!(
                sql.contains(&format!("DEFINE TABLE IF NOT EXISTS {t}")),
                "missing table: {t}"
            );
        }
    }
}

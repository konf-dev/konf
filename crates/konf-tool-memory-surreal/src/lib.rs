//! SurrealDB-backed memory backend for konf.
//!
//! Implements [`konf_tool_memory::MemoryBackend`] over
//! [SurrealDB](https://surrealdb.com) in either embedded mode (RocksDB or
//! in-memory) or remote mode (WebSocket to a Surreal server). The same
//! [`SurrealBackend`] type handles both — the config selects the connection
//! strategy at `connect()` time; all query code is identical.
//!
//! ## Why SurrealDB
//!
//! - **Graph-native**: typed edges via `RELATE` + relation tables, not SQL-modeled.
//! - **Vector search**: built-in HNSW index (`DEFINE INDEX ... HNSW`).
//! - **Full-text search**: built-in BM25 analyzer (`DEFINE INDEX ... FULLTEXT`).
//! - **Embedded + server**: one codebase, two deployment modes, identical
//!   SurrealQL semantics.
//! - **BUSL-1.1**: license twin with konf; auto-converts to Apache on 2030-01-01.
//!
//! ## Usage
//!
//! ```no_run
//! # async fn run() -> anyhow::Result<()> {
//! let config = serde_json::json!({
//!     "mode": "embedded",
//!     "path": "./memory.db",
//!     "namespace": "konf",
//!     "database": "default"
//! });
//! let backend = konf_tool_memory_surreal::connect(&config).await?;
//! // then: konf_tool_memory::register(engine, backend).await?;
//! # Ok(()) }
//! ```
#![warn(missing_docs)]

mod backend;
mod config;
mod connect;
mod error;
mod schema;
mod search;
mod session;

pub use backend::SurrealBackend;
pub use config::{SurrealConfig, SurrealMode};
pub use connect::connect;
pub use error::map_db_error;

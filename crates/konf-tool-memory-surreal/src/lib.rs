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
//!
//! ## Journal secondary for the Stigmergic Engine
//!
//! In addition to the `MemoryBackend`, this crate exposes
//! [`SurrealJournalStore`] — an implementation of
//! [`konf_runtime::JournalStore`] over the SurrealDB `event` table. When a
//! deployment configures both a redb primary (short-retention audit) and a
//! SurrealDB memory backend, `konf-init` wires them into a
//! `FanoutJournalStore` so every interaction lands in both the local
//! audit log and the long-term queryable graph. Call
//! [`connect_journal`] to build one directly; see
//! `konf-genesis/docs/STIGMERGIC_ENGINE.md` for the broader design.
#![warn(missing_docs)]

mod backend;
mod config;
mod connect;
mod error;
mod journal_store;
mod schema;
mod search;
mod session;

pub use backend::SurrealBackend;
pub use config::{SurrealConfig, SurrealMode};
pub use connect::{connect, connect_journal};
pub use error::map_db_error;
pub use journal_store::SurrealJournalStore;

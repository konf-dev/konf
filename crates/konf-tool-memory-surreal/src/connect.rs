//! Connection strategy: dispatch embedded/memory/remote and apply schema.

use std::sync::Arc;

use konf_tool_memory::{MemoryBackend, MemoryError};
use serde_json::Value;
use surrealdb::engine::any::{connect as surreal_connect, Any};
use surrealdb::opt::auth::Root;
use surrealdb::Surreal;
use tracing::info;

use crate::backend::SurrealBackend;
use crate::config::{SurrealConfig, SurrealMode};
use crate::schema::build_schema;

/// Connect to a SurrealDB instance and return a [`MemoryBackend`].
///
/// This is the entry point called by `konf-init` when wiring the memory
/// backend at boot. Shape mirrors `konf_tool_memory_smrti::connect`: takes
/// a raw JSON config, returns `Arc<dyn MemoryBackend>`, and does I/O.
///
/// On success, the schema has already been applied (idempotent) and the
/// returned backend is ready to serve trait calls.
pub async fn connect(config: &Value) -> anyhow::Result<Arc<dyn MemoryBackend>> {
    let cfg: SurrealConfig = serde_json::from_value(config.clone())
        .map_err(|e| anyhow::anyhow!("invalid SurrealConfig: {e}"))?;
    cfg.validate()
        .map_err(|e| anyhow::anyhow!("invalid SurrealConfig: {e}"))?;

    let db = open_connection(&cfg).await?;

    db.use_ns(cfg.namespace.clone())
        .use_db(cfg.database.clone())
        .await
        .map_err(|e| {
            anyhow::anyhow!(
                "failed to select namespace={}, database={}: {e}",
                cfg.namespace,
                cfg.database
            )
        })?;

    let schema_sql = build_schema(&cfg);
    db.query(schema_sql)
        .await
        .map_err(|e| anyhow::anyhow!("failed to apply SurrealDB schema: {e}"))?
        .check()
        .map_err(|e| anyhow::anyhow!("schema query reported error: {e}"))?;

    let mode_label = match cfg.mode {
        SurrealMode::Embedded => format!("embedded rocksdb at {:?}", cfg.path),
        SurrealMode::Memory => "in-memory".to_string(),
        SurrealMode::Remote => {
            format!("remote {}", cfg.endpoint.as_deref().unwrap_or("?"))
        }
    };
    info!("surrealdb memory backend connected ({mode_label})");

    Ok(Arc::new(SurrealBackend::new(db, cfg)))
}

/// Open a raw SurrealDB connection per the chosen mode.
///
/// Uses `surrealdb::engine::any` so one connection type (`Surreal<Any>`) can
/// back all three modes — embedded, memory, and remote — without ceremony.
/// The connection-string conventions are SurrealDB's own: `rocksdb://path`,
/// `mem://`, `ws://host:port`.
async fn open_connection(cfg: &SurrealConfig) -> anyhow::Result<Surreal<Any>> {
    let connection_string = match cfg.mode {
        SurrealMode::Embedded => {
            let path = cfg
                .path
                .as_deref()
                .ok_or_else(|| anyhow::anyhow!("embedded mode requires path"))?;
            format!("rocksdb://{path}")
        }
        SurrealMode::Memory => "mem://".to_string(),
        SurrealMode::Remote => cfg
            .endpoint
            .clone()
            .ok_or_else(|| anyhow::anyhow!("remote mode requires endpoint"))?,
    };

    let db = surreal_connect(&connection_string).await.map_err(|e| {
        anyhow::anyhow!("failed to open SurrealDB connection ({connection_string}): {e}")
    })?;

    if let (SurrealMode::Remote, Some(user), Some(pass)) =
        (&cfg.mode, cfg.username.as_deref(), cfg.password.as_deref())
    {
        db.signin(Root {
            username: user.to_string(),
            password: pass.to_string(),
        })
        .await
        .map_err(|e| anyhow::anyhow!("surrealdb signin failed: {e}"))?;
    }

    Ok(db)
}

/// Helper used by other modules to re-apply a schema fragment after a
/// transient schema change. Currently unused but kept `pub(crate)` so the
/// backend can invoke it in recovery paths without reopening a connection.
#[allow(dead_code)]
pub(crate) async fn reapply_schema(
    db: &Surreal<Any>,
    cfg: &SurrealConfig,
) -> Result<(), MemoryError> {
    let sql = build_schema(cfg);
    db.query(sql)
        .await
        .map_err(|e| MemoryError::OperationFailed(e.to_string()))?
        .check()
        .map_err(|e| MemoryError::OperationFailed(e.to_string()))?;
    Ok(())
}

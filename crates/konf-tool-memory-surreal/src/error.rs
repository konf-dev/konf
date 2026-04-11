//! SurrealDB error → `MemoryError` mapping.
//!
//! SurrealDB's error type is deep and variant-rich; for the memory backend
//! surface, we collapse it into [`konf_tool_memory::MemoryError`] buckets:
//!
//! - Connection / bootstrap failures → `Unavailable`
//! - Input validation (malformed node, missing namespace) → `Validation`
//! - Everything else (query failure, constraint violation, type mismatch)
//!   → `OperationFailed`
//!
//! The full upstream error text is always preserved in the message so product
//! operators can debug without needing to re-run with tracing enabled.

use konf_tool_memory::MemoryError;

/// Map any `surrealdb::Error` (or adjacent error) to `MemoryError::OperationFailed`.
///
/// Use this at every call site that awaits a SurrealDB operation after the
/// backend is connected. Bootstrap/connection failures in `connect()` should
/// use `MemoryError::Unavailable` directly instead.
pub fn map_db_error(err: impl std::fmt::Display) -> MemoryError {
    MemoryError::OperationFailed(err.to_string())
}

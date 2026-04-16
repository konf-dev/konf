//! Budget cells — per-trace shared budget with atomic decrement.
//!
//! A parent mints a cell with an initial amount; children attempt atomic
//! decrements. In-memory only (not persisted) — acceptable for the initial
//! implementation since budgets are short-lived (trace lifetime).

use std::collections::HashMap;
use std::fmt;
use std::sync::Mutex;

use uuid::Uuid;

/// Per-trace shared budget cells.
pub struct BudgetTable {
    cells: Mutex<HashMap<Uuid, i64>>,
}

impl BudgetTable {
    /// Create a new empty budget table.
    pub fn new() -> Self {
        Self {
            cells: Mutex::new(HashMap::new()),
        }
    }

    /// Mint a new budget cell for a trace. Returns an error if a cell already exists.
    pub fn mint(&self, trace_id: Uuid, amount: i64) -> Result<(), BudgetError> {
        use std::collections::hash_map::Entry;
        let mut cells = self.cells.lock().expect("budget lock poisoned");
        match cells.entry(trace_id) {
            Entry::Occupied(_) => Err(BudgetError::AlreadyExists),
            Entry::Vacant(e) => {
                e.insert(amount);
                Ok(())
            }
        }
    }

    /// Attempt to decrement. Returns `Ok(remaining)` or `Err` if insufficient
    /// or the trace has no budget cell.
    pub fn decrement(&self, trace_id: Uuid, amount: i64) -> Result<i64, BudgetError> {
        let mut cells = self.cells.lock().expect("budget lock poisoned");
        let current = cells.get(&trace_id).copied().ok_or(BudgetError::NotFound)?;
        if current < amount {
            return Err(BudgetError::Insufficient {
                remaining: current,
                requested: amount,
            });
        }
        let new = current - amount;
        cells.insert(trace_id, new);
        Ok(new)
    }

    /// Query remaining budget for a trace. Returns `None` if no cell exists.
    pub fn remaining(&self, trace_id: Uuid) -> Option<i64> {
        self.cells
            .lock()
            .expect("budget lock poisoned")
            .get(&trace_id)
            .copied()
    }

    /// Remove the budget cell for a trace (cleanup after trace completes).
    pub fn remove(&self, trace_id: Uuid) -> Option<i64> {
        self.cells
            .lock()
            .expect("budget lock poisoned")
            .remove(&trace_id)
    }
}

impl Default for BudgetTable {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Debug for BudgetTable {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let count = self.cells.lock().map(|c| c.len()).unwrap_or(0);
        f.debug_struct("BudgetTable")
            .field("active_cells", &count)
            .finish()
    }
}

/// Errors from budget operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BudgetError {
    /// No budget cell exists for this trace.
    NotFound,
    /// A budget cell already exists for this trace.
    AlreadyExists,
    /// Insufficient budget remaining.
    Insufficient { remaining: i64, requested: i64 },
}

impl fmt::Display for BudgetError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            BudgetError::NotFound => write!(f, "no budget cell found for trace"),
            BudgetError::AlreadyExists => write!(f, "budget cell already exists for trace"),
            BudgetError::Insufficient {
                remaining,
                requested,
            } => {
                write!(
                    f,
                    "insufficient budget: {remaining} remaining, {requested} requested"
                )
            }
        }
    }
}

impl std::error::Error for BudgetError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mint_and_decrement() {
        let table = BudgetTable::new();
        let tid = Uuid::new_v4();
        table.mint(tid, 100).unwrap();
        assert_eq!(table.remaining(tid), Some(100));

        let remaining = table.decrement(tid, 30).unwrap();
        assert_eq!(remaining, 70);
        assert_eq!(table.remaining(tid), Some(70));
    }

    #[test]
    fn decrement_insufficient() {
        let table = BudgetTable::new();
        let tid = Uuid::new_v4();
        table.mint(tid, 50).unwrap();

        let err = table.decrement(tid, 80).unwrap_err();
        assert_eq!(
            err,
            BudgetError::Insufficient {
                remaining: 50,
                requested: 80,
            }
        );
        // Budget should be unchanged after failed decrement.
        assert_eq!(table.remaining(tid), Some(50));
    }

    #[test]
    fn decrement_not_found() {
        let table = BudgetTable::new();
        let tid = Uuid::new_v4();
        assert_eq!(table.decrement(tid, 10).unwrap_err(), BudgetError::NotFound);
    }

    #[test]
    fn remaining_none_for_unknown_trace() {
        let table = BudgetTable::new();
        assert_eq!(table.remaining(Uuid::new_v4()), None);
    }

    #[test]
    fn remove_cleans_up() {
        let table = BudgetTable::new();
        let tid = Uuid::new_v4();
        table.mint(tid, 100).unwrap();
        assert_eq!(table.remove(tid), Some(100));
        assert_eq!(table.remaining(tid), None);
    }
}

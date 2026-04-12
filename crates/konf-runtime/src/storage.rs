//! Unified persistent-storage handle.
//!
//! [`KonfStorage`] owns a single [`redb::Database`] and exposes three logical
//! stores backed by it:
//!
//! - **Journal** (`RedbJournal`) — append-only audit log, from Phase 1.
//! - **Scheduler** (`RedbScheduler`) — durable timers, added in Phase 2.
//! - **Runner intents** (`RunnerIntentStore`) — spawn intents for restart
//!   replay, added in Phase 3.
//!
//! All three live in the same redb file so related state sits together and
//! there is only one path for operators to back up. Each is a separate set
//! of redb tables; they never read or write each other's tables.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use redb::Database;
#[allow(unused_imports)] // Trait import needed for begin_read/begin_write method resolution.
use redb::ReadableDatabase;

use crate::journal::{JournalError, JournalStore, RedbJournal};
use crate::runner_intents::{IntentError, RunnerIntentStore};

/// Errors raised by [`KonfStorage`] construction.
#[derive(Debug, thiserror::Error)]
pub enum StorageError {
    /// The path could not be opened as a redb database.
    #[error("failed to open redb database at {path}: {source}")]
    Open {
        path: PathBuf,
        #[source]
        source: Box<dyn std::error::Error + Send + Sync>,
    },

    /// The journal subsystem failed to initialise.
    #[error("journal init: {0}")]
    Journal(#[from] JournalError),

    /// The runner intent store failed to initialise.
    #[error("runner intent init: {0}")]
    Intent(#[from] IntentError),

    /// An internal task failed to join (tokio blocking-task panic).
    #[error("blocking task join: {0}")]
    Join(String),
}

/// Retention window for time-gated GC.
///
/// Applies uniformly to journal entries and terminated runner intents. 7
/// days is the default; configurable via `konf.toml → database.retention_days`.
#[derive(Debug, Clone, Copy)]
pub struct Retention {
    pub days: u32,
}

impl Default for Retention {
    fn default() -> Self {
        Self { days: 7 }
    }
}

/// Single point of access to konf's persistent state.
///
/// Cheap to clone (`Arc` inside). Owns one redb database exposing three
/// logical stores:
///
/// - Journal — [`RedbJournal`]
/// - Runner intents — [`RunnerIntentStore`]
/// - Scheduler — owned by [`crate::Runtime::install_scheduler`], not here,
///   because the scheduler needs a `Weak<Runtime>` that isn't available
///   until after the runtime is constructed.
#[derive(Clone)]
pub struct KonfStorage {
    db: Arc<Database>,
    journal: Arc<RedbJournal>,
    runner_intents: Arc<RunnerIntentStore>,
    retention: Retention,
}

impl KonfStorage {
    /// Open (or create) the redb database at `path` and initialise all
    /// stores over it.
    pub async fn open(
        path: impl AsRef<Path>,
        retention: Retention,
    ) -> Result<Self, StorageError> {
        let path_buf = path.as_ref().to_path_buf();
        let path_for_task = path_buf.clone();
        let db = tokio::task::spawn_blocking(move || Database::create(&path_for_task))
            .await
            .map_err(|e| StorageError::Join(e.to_string()))?
            .map_err(|e| StorageError::Open {
                path: path_buf.clone(),
                source: Box::new(std::io::Error::other(e.to_string())),
            })?;
        let db = Arc::new(db);
        let journal = Arc::new(RedbJournal::from_database(db.clone())?);
        let runner_intents = Arc::new(RunnerIntentStore::open(db.clone())?);
        Ok(Self {
            db,
            journal,
            runner_intents,
            retention,
        })
    }

    /// Access the underlying redb database. Exposed so Phase 2's scheduler
    /// can add its own tables inside the same database file.
    pub fn database(&self) -> Arc<Database> {
        self.db.clone()
    }

    /// Access the journal as a concrete type (for direct method calls that
    /// bypass the trait).
    pub fn journal(&self) -> &RedbJournal {
        &self.journal
    }

    /// Access the journal as a trait object (for consumers that only need
    /// the trait surface).
    pub fn journal_arc(&self) -> Arc<dyn JournalStore> {
        self.journal.clone() as Arc<dyn JournalStore>
    }

    /// Access the runner intent store as a concrete type.
    pub fn runner_intents(&self) -> &RunnerIntentStore {
        &self.runner_intents
    }

    /// Access the runner intent store as a shared Arc (for callers that
    /// need to hold a clone).
    pub fn runner_intents_arc(&self) -> Arc<RunnerIntentStore> {
        self.runner_intents.clone()
    }

    /// Retention window applied by time-gated GC tasks.
    pub fn retention(&self) -> Retention {
        self.retention
    }
}

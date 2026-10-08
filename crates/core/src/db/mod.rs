//! Storage. One SQLite file, owned by the user, holding everything.

pub mod read;
pub mod schema;
pub mod write;

use std::path::{Path, PathBuf};

use rusqlite::Connection;

use crate::error::Result;

/// A connection to a xitter-dl library.
///
/// Not `Clone` and not internally locked: the callers that need concurrency
/// (the Tauri app's loopback receiver, the background embedder) each own
/// their own connection and rely on WAL rather than on a mutex. That keeps
/// a slow read from blocking an ingest.
pub struct Library {
    conn: Connection,
    path: Option<PathBuf>,
}

impl Library {
    /// Open (or create) a library at `path`.
    pub fn open(path: impl AsRef<Path>) -> Result<Self> {
        let path = path.as_ref().to_path_buf();
        if let Some(parent) = path.parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent).map_err(|e| crate::error::Error::io(parent, e))?;
            }
        }

        let mut conn = Connection::open(&path)?;
        configure(&conn)?;
        migrate(&mut conn)?;

        Ok(Self { conn, path: Some(path) })
    }

    /// An ephemeral in-memory library, for tests and for `xdl import --dry-run`.
    pub fn open_in_memory() -> Result<Self> {
        let mut conn = Connection::open_in_memory()?;
        configure(&conn)?;
        migrate(&mut conn)?;
        Ok(Self { conn, path: None })
    }

    pub fn conn(&self) -> &Connection {
        &self.conn
    }

    pub fn conn_mut(&mut self) -> &mut Connection {
        &mut self.conn
    }

    pub fn path(&self) -> Option<&Path> {
        self.path.as_deref()
    }

    /// Total bookmarks held. Cheap enough to call on every UI refresh.
    pub fn count_bookmarks(&self) -> Result<i64> {
        Ok(self
            .conn
            .query_row("SELECT count(*) FROM bookmarks", [], |r| r.get(0))?)
    }
}

/// Pragmas applied to every connection.
///
/// Order matters slightly: `foreign_keys` is per-connection and off by
/// default, so it must be set before any write. `journal_mode` is persistent
/// in the file header, but setting it is idempotent and cheap.
pub fn configure(conn: &Connection) -> Result<()> {
    // WAL lets the app read while an import writes, which is the difference
    // between the live-capture panel staying responsive and freezing.
    // For :memory: databases SQLite reports "memory" instead — that is not an
    // error, which is why the result is read and discarded rather than
    // asserted.
    let _mode: String = conn.query_row("PRAGMA journal_mode = WAL", [], |r| r.get(0))?;

    conn.execute_batch(
        "PRAGMA foreign_keys = ON;
         -- NORMAL rather than FULL: this is a local archive, not a ledger.
         -- The worst case is losing the last transaction on a power cut, and
         -- the cost of FULL is a fsync per commit during a 10k-row import.
         PRAGMA synchronous = NORMAL;
         PRAGMA busy_timeout = 5000;
         -- Keep temp tables in memory; sorting search results is the main use.
         PRAGMA temp_store = MEMORY;",
    )?;

    Ok(())
}

/// Apply any unapplied migrations. Returns how many ran.
pub fn migrate(conn: &mut Connection) -> Result<usize> {
    let current: i32 = conn.query_row("PRAGMA user_version", [], |r| r.get(0))?;
    let mut applied = 0usize;

    for (index, sql) in schema::MIGRATIONS.iter().enumerate() {
        let version = (index + 1) as i32;
        if version <= current {
            continue;
        }

        // Each migration is atomic with its version bump, so an interrupted
        // upgrade leaves the database on the old version rather than half
        // migrated.
        let tx = conn.transaction()?;
        tx.execute_batch(sql)?;
        tx.pragma_update(None, "user_version", version)?;
        tx.commit()?;

        applied += 1;
    }

    Ok(applied)
}

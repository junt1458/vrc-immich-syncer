//! SQLite-backed upload ledger.
//!
//! The state DB is the source of truth for "already handled": files recorded
//! as `uploaded` or `duplicate` must never be uploaded again, even if the
//! corresponding asset is later deleted on the Immich server.

use std::path::{Path, PathBuf};

use chrono::{DateTime, Utc};
use rusqlite::{params, Connection, OptionalExtension, Row};

/// Status of a tracked file in the upload ledger.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FileStatus {
    /// Discovered locally, not yet uploaded.
    Pending,
    /// Upload currently in progress.
    Uploading,
    /// Successfully uploaded to Immich.
    Uploaded,
    /// Server already had this asset (checksum match); treated as handled.
    Duplicate,
    /// Upload failed; eligible for retry once `next_retry_at` has passed.
    Failed,
}

impl FileStatus {
    fn as_str(self) -> &'static str {
        match self {
            FileStatus::Pending => "pending",
            FileStatus::Uploading => "uploading",
            FileStatus::Uploaded => "uploaded",
            FileStatus::Duplicate => "duplicate",
            FileStatus::Failed => "failed",
        }
    }

    fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(FileStatus::Pending),
            "uploading" => Some(FileStatus::Uploading),
            "uploaded" => Some(FileStatus::Uploaded),
            "duplicate" => Some(FileStatus::Duplicate),
            "failed" => Some(FileStatus::Failed),
            _ => None,
        }
    }

    /// A file in a terminal "handled" status must never be re-uploaded, even
    /// if the corresponding server asset is later deleted.
    fn is_terminal_handled(self) -> bool {
        matches!(self, FileStatus::Uploaded | FileStatus::Duplicate)
    }
}

impl rusqlite::ToSql for FileStatus {
    fn to_sql(&self) -> rusqlite::Result<rusqlite::types::ToSqlOutput<'_>> {
        Ok(rusqlite::types::ToSqlOutput::from(self.as_str()))
    }
}

impl rusqlite::types::FromSql for FileStatus {
    fn column_result(value: rusqlite::types::ValueRef<'_>) -> rusqlite::types::FromSqlResult<Self> {
        let s = value.as_str()?;
        FileStatus::from_str(s).ok_or_else(|| {
            rusqlite::types::FromSqlError::Other(format!("invalid file status: {s}").into())
        })
    }
}

/// A row in the `files` upload ledger.
#[derive(Debug, Clone, PartialEq)]
pub struct FileRecord {
    pub id: i64,
    pub path: PathBuf,
    pub size: i64,
    pub mtime: i64,
    pub sha1: Option<String>,
    pub status: FileStatus,
    pub asset_id: Option<String>,
    pub retry_count: i64,
    pub next_retry_at: Option<i64>,
    pub last_error: Option<String>,
    pub uploaded_at: Option<i64>,
}

fn row_to_record(row: &Row<'_>) -> rusqlite::Result<FileRecord> {
    let path: String = row.get("path")?;
    Ok(FileRecord {
        id: row.get("id")?,
        path: PathBuf::from(path),
        size: row.get("size")?,
        mtime: row.get("mtime")?,
        sha1: row.get("sha1")?,
        status: row.get("status")?,
        asset_id: row.get("asset_id")?,
        retry_count: row.get("retry_count")?,
        next_retry_at: row.get("next_retry_at")?,
        last_error: row.get("last_error")?,
        uploaded_at: row.get("uploaded_at")?,
    })
}

/// Returns the default state DB path in the platform data/config directory:
/// `<config_dir>/state.sqlite3`.
pub fn default_state_db_path() -> Option<PathBuf> {
    directories::ProjectDirs::from("", "", "vrc-immich-syncer")
        .map(|dirs| dirs.config_dir().join("state.sqlite3"))
}

/// SQLite-backed upload ledger tracking the status of every file discovered
/// in the watched folders.
pub struct StateDb {
    conn: Connection,
}

impl StateDb {
    /// Opens (creating if necessary) a state DB at the given path, running
    /// schema migrations (`CREATE TABLE IF NOT EXISTS`).
    pub fn open(path: &Path) -> anyhow::Result<StateDb> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        let db = StateDb { conn };
        db.migrate()?;
        Ok(db)
    }

    /// Opens an in-memory state DB. Useful for tests.
    pub fn open_in_memory() -> anyhow::Result<StateDb> {
        let conn = Connection::open_in_memory()?;
        let db = StateDb { conn };
        db.migrate()?;
        Ok(db)
    }

    fn migrate(&self) -> anyhow::Result<()> {
        self.conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS files (
                id              INTEGER PRIMARY KEY AUTOINCREMENT,
                path            TEXT NOT NULL UNIQUE,
                size            INTEGER NOT NULL,
                mtime           INTEGER NOT NULL,
                sha1            TEXT,
                status          TEXT NOT NULL,
                asset_id        TEXT,
                retry_count     INTEGER NOT NULL DEFAULT 0,
                next_retry_at   INTEGER,
                last_error      TEXT,
                uploaded_at     INTEGER
            );
            CREATE INDEX IF NOT EXISTS idx_files_sha1 ON files(sha1);
            CREATE INDEX IF NOT EXISTS idx_files_status ON files(status);
            ",
        )?;
        Ok(())
    }

    /// A file recorded as `uploaded` or `duplicate` is already handled and
    /// must never be uploaded again, even if the server-side asset was
    /// later deleted.
    pub fn is_already_handled(&self, path: &Path) -> anyhow::Result<bool> {
        Ok(self
            .find_by_path(path)?
            .is_some_and(|r| r.status.is_terminal_handled()))
    }

    /// Inserts a new `pending` row, or updates size/mtime of an existing row
    /// that has not yet reached a terminal handled status (`uploaded` /
    /// `duplicate`). Rows already `uploaded` or `duplicate` are left
    /// untouched, by construction.
    pub fn upsert_pending(&self, path: &Path, size: i64, mtime: i64) -> anyhow::Result<()> {
        let path_str = path_to_string(path)?;
        let existing = self.find_by_path(path)?;

        match existing {
            None => {
                self.conn.execute(
                    "INSERT INTO files (path, size, mtime, status, retry_count) \
                     VALUES (?1, ?2, ?3, ?4, 0)",
                    params![path_str, size, mtime, FileStatus::Pending],
                )?;
            }
            Some(record) if record.status.is_terminal_handled() => {
                // Never resurrect a handled file back into the pending queue.
            }
            Some(_) => {
                self.conn.execute(
                    "UPDATE files SET size = ?1, mtime = ?2 WHERE path = ?3",
                    params![size, mtime, path_str],
                )?;
            }
        }
        Ok(())
    }

    /// Marks a file as currently uploading.
    pub fn mark_uploading(&self, path: &Path) -> anyhow::Result<()> {
        let path_str = path_to_string(path)?;
        self.conn.execute(
            "UPDATE files SET status = ?1 WHERE path = ?2",
            params![FileStatus::Uploading, path_str],
        )?;
        Ok(())
    }

    /// Marks a file as successfully uploaded.
    pub fn mark_uploaded(
        &self,
        path: &Path,
        sha1: &str,
        asset_id: &str,
        uploaded_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let path_str = path_to_string(path)?;
        self.conn.execute(
            "UPDATE files SET status = ?1, sha1 = ?2, asset_id = ?3, uploaded_at = ?4, \
             last_error = NULL WHERE path = ?5",
            params![
                FileStatus::Uploaded,
                sha1,
                asset_id,
                uploaded_at.timestamp(),
                path_str
            ],
        )?;
        Ok(())
    }

    /// Marks a file as a server-side duplicate (already present on Immich by
    /// checksum). Treated as handled, same as `uploaded`.
    pub fn mark_duplicate(&self, path: &Path, sha1: &str, asset_id: &str) -> anyhow::Result<()> {
        let path_str = path_to_string(path)?;
        self.conn.execute(
            "UPDATE files SET status = ?1, sha1 = ?2, asset_id = ?3, last_error = NULL \
             WHERE path = ?4",
            params![FileStatus::Duplicate, sha1, asset_id, path_str],
        )?;
        Ok(())
    }

    /// Marks a file as failed, incrementing its retry count and setting the
    /// next retry time.
    pub fn mark_failed(
        &self,
        path: &Path,
        error: &str,
        next_retry_at: DateTime<Utc>,
    ) -> anyhow::Result<()> {
        let path_str = path_to_string(path)?;
        self.conn.execute(
            "UPDATE files SET status = ?1, last_error = ?2, next_retry_at = ?3, \
             retry_count = retry_count + 1 WHERE path = ?4",
            params![
                FileStatus::Failed,
                error,
                next_retry_at.timestamp(),
                path_str
            ],
        )?;
        Ok(())
    }

    /// Returns all rows currently in `pending` status.
    pub fn get_pending(&self) -> anyhow::Result<Vec<FileRecord>> {
        self.query_by_status(FileStatus::Pending)
    }

    /// Returns all `failed` rows whose `next_retry_at` has passed (i.e. is
    /// less than or equal to `now`).
    pub fn get_due_retries(&self, now: DateTime<Utc>) -> anyhow::Result<Vec<FileRecord>> {
        let mut stmt = self.conn.prepare(
            "SELECT * FROM files WHERE status = ?1 AND next_retry_at IS NOT NULL \
             AND next_retry_at <= ?2 ORDER BY id",
        )?;
        let rows = stmt
            .query_map(params![FileStatus::Failed, now.timestamp()], row_to_record)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    fn query_by_status(&self, status: FileStatus) -> anyhow::Result<Vec<FileRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM files WHERE status = ?1 ORDER BY id")?;
        let rows = stmt
            .query_map(params![status], row_to_record)?
            .collect::<Result<Vec<_>, _>>()?;
        Ok(rows)
    }

    /// Looks up a file record by its path.
    pub fn find_by_path(&self, path: &Path) -> anyhow::Result<Option<FileRecord>> {
        let path_str = path_to_string(path)?;
        let mut stmt = self.conn.prepare("SELECT * FROM files WHERE path = ?1")?;
        let record = stmt
            .query_row(params![path_str], row_to_record)
            .optional()?;
        Ok(record)
    }

    /// Looks up a file record by its SHA-1 checksum. Used to recognize a
    /// locally moved/renamed file with unchanged content as already handled.
    pub fn find_by_checksum(&self, sha1: &str) -> anyhow::Result<Option<FileRecord>> {
        let mut stmt = self
            .conn
            .prepare("SELECT * FROM files WHERE sha1 = ?1 ORDER BY id LIMIT 1")?;
        let record = stmt.query_row(params![sha1], row_to_record).optional()?;
        Ok(record)
    }
}

fn path_to_string(path: &Path) -> anyhow::Result<String> {
    path.to_str()
        .map(str::to_owned)
        .ok_or_else(|| anyhow::anyhow!("path is not valid UTF-8: {path:?}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::TimeZone;

    fn ts(secs: i64) -> DateTime<Utc> {
        Utc.timestamp_opt(secs, 0).unwrap()
    }

    #[test]
    fn upsert_pending_inserts_new_row() {
        let db = StateDb::open_in_memory().unwrap();
        let path = PathBuf::from("/screenshots/a.png");
        db.upsert_pending(&path, 100, 1000).unwrap();

        let record = db.find_by_path(&path).unwrap().expect("row should exist");
        assert_eq!(record.status, FileStatus::Pending);
        assert_eq!(record.size, 100);
        assert_eq!(record.mtime, 1000);
        assert_eq!(record.retry_count, 0);
    }

    #[test]
    fn uploaded_file_never_re_enters_pending_via_upsert() {
        let db = StateDb::open_in_memory().unwrap();
        let path = PathBuf::from("/screenshots/a.png");
        db.upsert_pending(&path, 100, 1000).unwrap();
        db.mark_uploaded(&path, "deadbeef", "asset-1", ts(2000))
            .unwrap();

        // Simulate a rescan seeing the same (or changed) file again.
        db.upsert_pending(&path, 999, 9999).unwrap();

        let record = db.find_by_path(&path).unwrap().unwrap();
        assert_eq!(record.status, FileStatus::Uploaded);
        // Size/mtime must not have been overwritten either.
        assert_eq!(record.size, 100);
        assert_eq!(record.mtime, 1000);
        assert!(db.is_already_handled(&path).unwrap());
    }

    #[test]
    fn duplicate_file_never_re_enters_pending_via_upsert() {
        let db = StateDb::open_in_memory().unwrap();
        let path = PathBuf::from("/screenshots/b.png");
        db.upsert_pending(&path, 100, 1000).unwrap();
        db.mark_duplicate(&path, "cafebabe", "asset-2").unwrap();

        db.upsert_pending(&path, 200, 2000).unwrap();

        let record = db.find_by_path(&path).unwrap().unwrap();
        assert_eq!(record.status, FileStatus::Duplicate);
        assert_eq!(record.size, 100);
        assert!(db.is_already_handled(&path).unwrap());
    }

    #[test]
    fn upsert_pending_updates_non_terminal_row() {
        let db = StateDb::open_in_memory().unwrap();
        let path = PathBuf::from("/screenshots/c.png");
        db.upsert_pending(&path, 100, 1000).unwrap();
        db.mark_failed(&path, "network error", ts(5000)).unwrap();

        // A failed row is not terminal-handled, so a rescan should refresh it.
        db.upsert_pending(&path, 150, 1500).unwrap();

        let record = db.find_by_path(&path).unwrap().unwrap();
        assert_eq!(record.size, 150);
        assert_eq!(record.mtime, 1500);
        // Status/retry bookkeeping from mark_failed is left as-is by upsert.
        assert_eq!(record.status, FileStatus::Failed);
    }

    #[test]
    fn mark_failed_increments_retry_count_and_sets_next_retry() {
        let db = StateDb::open_in_memory().unwrap();
        let path = PathBuf::from("/screenshots/d.png");
        db.upsert_pending(&path, 100, 1000).unwrap();

        db.mark_failed(&path, "boom 1", ts(100)).unwrap();
        let record = db.find_by_path(&path).unwrap().unwrap();
        assert_eq!(record.retry_count, 1);
        assert_eq!(record.next_retry_at, Some(100));
        assert_eq!(record.last_error.as_deref(), Some("boom 1"));
        assert_eq!(record.status, FileStatus::Failed);

        db.mark_failed(&path, "boom 2", ts(200)).unwrap();
        let record = db.find_by_path(&path).unwrap().unwrap();
        assert_eq!(record.retry_count, 2);
        assert_eq!(record.next_retry_at, Some(200));
        assert_eq!(record.last_error.as_deref(), Some("boom 2"));
    }

    #[test]
    fn find_by_checksum_survives_rename() {
        let db = StateDb::open_in_memory().unwrap();
        let old_path = PathBuf::from("/screenshots/2024-01/old-name.png");
        db.upsert_pending(&old_path, 100, 1000).unwrap();
        db.mark_uploaded(&old_path, "abc123", "asset-9", ts(3000))
            .unwrap();

        let found = db
            .find_by_checksum("abc123")
            .unwrap()
            .expect("should find record by checksum");
        assert_eq!(found.path, old_path);
        assert_eq!(found.status, FileStatus::Uploaded);
        assert_eq!(found.asset_id.as_deref(), Some("asset-9"));

        // A rename shows up locally as a new path with the same content hash;
        // the scanner (M3) is expected to consult find_by_checksum before
        // calling upsert_pending on the new path.
        assert!(db.find_by_checksum("does-not-exist").unwrap().is_none());
    }

    #[test]
    fn get_due_retries_only_returns_past_due_rows() {
        let db = StateDb::open_in_memory().unwrap();
        let due_path = PathBuf::from("/screenshots/due.png");
        let not_due_path = PathBuf::from("/screenshots/not-due.png");
        let pending_path = PathBuf::from("/screenshots/pending.png");

        db.upsert_pending(&due_path, 1, 1).unwrap();
        db.mark_failed(&due_path, "err", ts(1000)).unwrap();

        db.upsert_pending(&not_due_path, 1, 1).unwrap();
        db.mark_failed(&not_due_path, "err", ts(5000)).unwrap();

        db.upsert_pending(&pending_path, 1, 1).unwrap();

        let due = db.get_due_retries(ts(2000)).unwrap();
        assert_eq!(due.len(), 1);
        assert_eq!(due[0].path, due_path);

        // Exactly-equal next_retry_at should count as due.
        let due_exact = db.get_due_retries(ts(1000)).unwrap();
        assert_eq!(due_exact.len(), 1);
        assert_eq!(due_exact[0].path, due_path);
    }

    #[test]
    fn get_pending_returns_only_pending_rows() {
        let db = StateDb::open_in_memory().unwrap();
        let p1 = PathBuf::from("/screenshots/p1.png");
        let p2 = PathBuf::from("/screenshots/p2.png");
        db.upsert_pending(&p1, 1, 1).unwrap();
        db.upsert_pending(&p2, 1, 1).unwrap();
        db.mark_uploading(&p2).unwrap();

        let pending = db.get_pending().unwrap();
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].path, p1);
    }

    #[test]
    fn open_creates_parent_dirs_and_persists_across_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let db_path = dir.path().join("nested").join("state.sqlite3");

        {
            let db = StateDb::open(&db_path).unwrap();
            db.upsert_pending(&PathBuf::from("/x.png"), 1, 1).unwrap();
        }

        assert!(db_path.exists());

        let db = StateDb::open(&db_path).unwrap();
        let record = db
            .find_by_path(&PathBuf::from("/x.png"))
            .unwrap()
            .expect("row should persist across reopen");
        assert_eq!(record.status, FileStatus::Pending);
    }
}

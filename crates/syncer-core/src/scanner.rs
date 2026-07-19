//! Full recursive scan of the configured watch folders, diffing against the
//! state DB and enqueueing newly discovered files as `pending`.

use std::path::Path;

use tokio::sync::mpsc::UnboundedSender;
use walkdir::WalkDir;

use crate::config::Config;
use crate::events::{emit, SyncEvent};
use crate::state::StateDb;

/// Summary counters returned by [`scan_once`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ScanSummary {
    /// Total number of files whose extension matched `allowed_extensions`.
    pub files_seen: usize,
    /// Number of files newly enqueued as `pending` in this scan.
    pub newly_enqueued: usize,
}

/// Returns true if `path`'s extension (case-insensitive) is present in
/// `allowed_extensions` (which are stored lowercase, without the dot).
pub(crate) fn extension_allowed(path: &Path, allowed_extensions: &[String]) -> bool {
    let Some(ext) = path.extension().and_then(|e| e.to_str()) else {
        return false;
    };
    let ext = ext.to_ascii_lowercase();
    allowed_extensions
        .iter()
        .any(|allowed| allowed.eq_ignore_ascii_case(&ext))
}

/// Shared "consider this path" logic used by both the scanner and the
/// watcher: if the extension matches and the file is not already handled,
/// upsert it as `pending` using its current size/mtime. Returns `true` if the
/// file was eligible (extension matched), regardless of whether it was
/// already handled or newly enqueued vs. refreshed.
///
/// Returns `Ok(true)` when a new `pending` row was inserted (i.e. the path
/// was not already known to the DB), which the caller can use to track
/// "newly enqueued" counts. Already-known non-terminal rows are refreshed but
/// not counted as newly enqueued; already-handled rows are left untouched.
pub(crate) fn consider_path(
    db: &StateDb,
    config: &Config,
    path: &Path,
) -> anyhow::Result<ConsiderOutcome> {
    if !extension_allowed(path, &config.allowed_extensions) {
        return Ok(ConsiderOutcome::Skipped);
    }

    if db.is_already_handled(path)? {
        return Ok(ConsiderOutcome::AlreadyHandled);
    }

    let metadata = match std::fs::metadata(path) {
        Ok(m) => m,
        Err(_) => {
            // File may have been removed/renamed between discovery and stat;
            // non-fatal, just skip it.
            return Ok(ConsiderOutcome::Skipped);
        }
    };
    let size = metadata.len() as i64;
    let mtime = mtime_secs(&metadata);

    let is_new = db.find_by_path(path)?.is_none();
    db.upsert_pending(path, size, mtime)?;

    Ok(if is_new {
        ConsiderOutcome::Enqueued
    } else {
        ConsiderOutcome::Refreshed
    })
}

/// Outcome of [`consider_path`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ConsiderOutcome {
    /// Extension not in the allowed list.
    Skipped,
    /// Already `uploaded`/`duplicate`; left untouched.
    AlreadyHandled,
    /// A new `pending` row was inserted.
    Enqueued,
    /// An existing non-terminal row had its size/mtime refreshed.
    Refreshed,
}

fn mtime_secs(metadata: &std::fs::Metadata) -> i64 {
    metadata
        .modified()
        .ok()
        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Walks all `config.watch_folders` recursively, filtering to files whose
/// extension is in `config.allowed_extensions`, and enqueues newly
/// discovered (or previously non-terminal) files as `pending` in `db`.
///
/// Synchronous/blocking by design (`walkdir` is sync); the caller should wrap
/// this in `tokio::task::spawn_blocking` if called from async context.
pub fn scan_once(config: &Config, db: &StateDb) -> anyhow::Result<ScanSummary> {
    scan_once_with_events(config, db, None)
}

/// Same as [`scan_once`], optionally emitting a [`SyncEvent::ScanFinished`]
/// event when done.
pub fn scan_once_with_events(
    config: &Config,
    db: &StateDb,
    events: Option<&UnboundedSender<SyncEvent>>,
) -> anyhow::Result<ScanSummary> {
    let mut summary = ScanSummary::default();

    for folder in &config.watch_folders {
        if !folder.exists() {
            continue;
        }
        for entry in WalkDir::new(folder)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if !extension_allowed(path, &config.allowed_extensions) {
                continue;
            }
            summary.files_seen += 1;
            match consider_path(db, config, path)? {
                ConsiderOutcome::Enqueued => summary.newly_enqueued += 1,
                ConsiderOutcome::Skipped
                | ConsiderOutcome::AlreadyHandled
                | ConsiderOutcome::Refreshed => {}
            }
        }
    }

    emit(
        events,
        SyncEvent::ScanFinished {
            files_seen: summary.files_seen,
            newly_enqueued: summary.newly_enqueued,
        },
    );

    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Utc;
    use std::fs;

    fn test_config(folders: Vec<std::path::PathBuf>) -> Config {
        Config {
            watch_folders: folders,
            allowed_extensions: vec!["png".to_string(), "jpg".to_string()],
            ..Config::default()
        }
    }

    #[test]
    fn matching_extensions_are_enqueued_as_pending() {
        let dir = tempfile::tempdir().unwrap();
        let png_path = dir.path().join("a.png");
        fs::write(&png_path, b"fake png bytes").unwrap();

        let db = StateDb::open_in_memory().unwrap();
        let config = test_config(vec![dir.path().to_path_buf()]);

        let summary = scan_once(&config, &db).unwrap();
        assert_eq!(summary.files_seen, 1);
        assert_eq!(summary.newly_enqueued, 1);

        let record = db.find_by_path(&png_path).unwrap().expect("row exists");
        assert_eq!(record.status, crate::state::FileStatus::Pending);
    }

    #[test]
    fn non_matching_extensions_are_skipped() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("notes.txt"), b"hello").unwrap();

        let db = StateDb::open_in_memory().unwrap();
        let config = test_config(vec![dir.path().to_path_buf()]);

        let summary = scan_once(&config, &db).unwrap();
        assert_eq!(summary.files_seen, 0);
        assert_eq!(summary.newly_enqueued, 0);
        assert!(db
            .find_by_path(&dir.path().join("notes.txt"))
            .unwrap()
            .is_none());
    }

    #[test]
    fn already_uploaded_files_are_not_re_enqueued() {
        let dir = tempfile::tempdir().unwrap();
        let png_path = dir.path().join("done.png");
        fs::write(&png_path, b"fake png bytes").unwrap();

        let db = StateDb::open_in_memory().unwrap();
        db.upsert_pending(&png_path, 1, 1).unwrap();
        db.mark_uploaded(&png_path, "sha1hash", "asset-1", Utc::now())
            .unwrap();

        let config = test_config(vec![dir.path().to_path_buf()]);
        let summary = scan_once(&config, &db).unwrap();

        assert_eq!(summary.files_seen, 1);
        assert_eq!(summary.newly_enqueued, 0);

        let record = db.find_by_path(&png_path).unwrap().unwrap();
        assert_eq!(record.status, crate::state::FileStatus::Uploaded);
    }

    #[test]
    fn nested_subfolders_are_walked() {
        let dir = tempfile::tempdir().unwrap();
        let nested = dir.path().join("2024-01");
        fs::create_dir_all(&nested).unwrap();
        let nested_file = nested.join("screenshot.jpg");
        fs::write(&nested_file, b"jpg bytes").unwrap();

        let db = StateDb::open_in_memory().unwrap();
        let config = test_config(vec![dir.path().to_path_buf()]);

        let summary = scan_once(&config, &db).unwrap();
        assert_eq!(summary.files_seen, 1);
        assert_eq!(summary.newly_enqueued, 1);
        assert!(db.find_by_path(&nested_file).unwrap().is_some());
    }

    #[test]
    fn nonexistent_watch_folder_is_skipped_without_error() {
        let db = StateDb::open_in_memory().unwrap();
        let config = test_config(vec![std::path::PathBuf::from(
            "/does/not/exist/at/all/vrc-immich-syncer-test",
        )]);

        let summary = scan_once(&config, &db).unwrap();
        assert_eq!(summary.files_seen, 0);
        assert_eq!(summary.newly_enqueued, 0);
    }
}

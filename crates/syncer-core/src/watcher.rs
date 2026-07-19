//! Debounced recursive filesystem watcher over the configured watch folders.
//!
//! VRChat may still be writing a screenshot PNG when the `Create` event
//! fires, so events are debounced (via `notify-debouncer-full`'s built-in
//! timeout) before the corresponding file is considered stable enough to
//! enqueue. The watcher is restartable (call `stop()`, then build and start a
//! fresh `Watcher`) so the app layer can rebuild it when the folder list in
//! `Config` changes.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use notify::{RecursiveMode, Watcher as _};
use notify_debouncer_full::{new_debouncer, DebounceEventResult, Debouncer, FileIdMap};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::Config;
use crate::events::{emit, SyncEvent};
use crate::scanner::{consider_path, ConsiderOutcome};
use crate::state::StateDb;

/// Debounce window: VRChat/VRCX may take a moment to finish writing a
/// screenshot, so we wait this long after the last filesystem event for a
/// given path before treating it as stable and enqueueing it.
const DEBOUNCE_WINDOW: Duration = Duration::from_secs(2);

/// A debounced recursive filesystem watcher covering a set of folders.
/// Construct, [`start`](Watcher::start), and [`stop`](Watcher::stop) (or just
/// drop) to tear down — designed to be rebuilt whenever the configured watch
/// folders change.
pub struct Watcher {
    folders: Vec<PathBuf>,
    debouncer: Option<Debouncer<notify::RecommendedWatcher, FileIdMap>>,
}

impl Watcher {
    /// Creates a new (not-yet-started) watcher over `config.watch_folders`.
    pub fn new(config: &Config) -> Self {
        Self {
            folders: config.watch_folders.clone(),
            debouncer: None,
        }
    }

    /// Starts watching all configured folders. Folders that don't (yet)
    /// exist are skipped (non-fatal); the app layer can restart the watcher
    /// after creating them or on the next config save.
    ///
    /// On a stabilized create/modify event for a file whose extension is in
    /// `config.allowed_extensions`, the shared `consider_path` helper is
    /// invoked so the file is enqueued as `pending` in `db`, exactly like the
    /// scanner would.
    ///
    /// `db` is wrapped in a `std::sync::Mutex` (rather than passed bare)
    /// because `rusqlite::Connection` is `!Sync`, but the debouncer's event
    /// handler runs on its own background thread and needs `Send + Sync`
    /// access.
    pub fn start(
        &mut self,
        config: Config,
        db: Arc<Mutex<StateDb>>,
        events: Option<UnboundedSender<SyncEvent>>,
    ) -> anyhow::Result<()> {
        let handler_config = config.clone();

        let mut debouncer = new_debouncer(
            DEBOUNCE_WINDOW,
            None,
            move |result: DebounceEventResult| match result {
                Ok(debounced_events) => {
                    for event in debounced_events {
                        handle_debounced_event(&event.event, &handler_config, &db, events.as_ref());
                    }
                }
                Err(errors) => {
                    for error in errors {
                        tracing::warn!("filesystem watch error: {error}");
                    }
                }
            },
        )?;

        for folder in &self.folders {
            if !folder.exists() {
                tracing::warn!(
                    "watch folder {folder:?} does not exist yet; skipping (will not be watched \
                     until the watcher is restarted)"
                );
                continue;
            }
            debouncer
                .watcher()
                .watch(folder, RecursiveMode::Recursive)?;
            debouncer.cache().add_root(folder, RecursiveMode::Recursive);
        }

        self.debouncer = Some(debouncer);
        Ok(())
    }

    /// Stops the watcher, tearing down the underlying OS watch handles.
    /// Blocks briefly for the debouncer thread to join.
    pub fn stop(&mut self) {
        if let Some(debouncer) = self.debouncer.take() {
            debouncer.stop();
        }
    }
}

impl Drop for Watcher {
    fn drop(&mut self) {
        self.stop();
    }
}

fn handle_debounced_event(
    event: &notify::Event,
    config: &Config,
    db: &Mutex<StateDb>,
    events: Option<&UnboundedSender<SyncEvent>>,
) {
    use notify::EventKind;

    if !matches!(event.kind, EventKind::Create(_) | EventKind::Modify(_)) {
        return;
    }

    for path in &event.paths {
        if !path.is_file() {
            continue;
        }
        consider_stabilized_path(path, config, db, events);
    }
}

/// Considers a single path that has already passed the debouncer's timeout
/// window. As an extra safety margin against partially-written files (e.g.
/// on filesystems/backends where the debounce window alone isn't a reliable
/// stability signal), this re-checks the file size after a short delay and
/// only proceeds if it is unchanged.
fn consider_stabilized_path(
    path: &Path,
    config: &Config,
    db: &Mutex<StateDb>,
    events: Option<&UnboundedSender<SyncEvent>>,
) {
    if !is_size_stable(path) {
        tracing::debug!("skipping {path:?}: size not yet stable");
        return;
    }

    let db = match db.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };

    match consider_path(&db, config, path) {
        Ok(ConsiderOutcome::Enqueued) => {
            emit(
                events,
                SyncEvent::ScanFinished {
                    files_seen: 1,
                    newly_enqueued: 1,
                },
            );
        }
        Ok(_) => {}
        Err(e) => tracing::warn!("failed to consider watched path {path:?}: {e}"),
    }
}

/// Polls the file size twice with a short delay in between and returns
/// whether it stayed the same, i.e. the file is no longer being written to.
fn is_size_stable(path: &Path) -> bool {
    const POLL_DELAY: Duration = Duration::from_millis(300);

    let Ok(first) = std::fs::metadata(path).map(|m| m.len()) else {
        return false;
    };
    std::thread::sleep(POLL_DELAY);
    let Ok(second) = std::fs::metadata(path).map(|m| m.len()) else {
        return false;
    };
    first == second
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::sync::Arc;
    use std::time::Instant;

    fn test_config(folder: PathBuf) -> Config {
        Config {
            watch_folders: vec![folder],
            allowed_extensions: vec!["png".to_string()],
            ..Config::default()
        }
    }

    #[test]
    fn start_and_stop_on_tempdir_does_not_error() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path().to_path_buf());
        let db = Arc::new(Mutex::new(StateDb::open_in_memory().unwrap()));

        let mut watcher = Watcher::new(&config);
        watcher
            .start(config, db, None)
            .expect("start should succeed");
        watcher.stop();
    }

    #[test]
    fn dropping_watcher_does_not_panic() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path().to_path_buf());
        let db = Arc::new(Mutex::new(StateDb::open_in_memory().unwrap()));

        let mut watcher = Watcher::new(&config);
        watcher
            .start(config, db, None)
            .expect("start should succeed");
        drop(watcher);
    }

    #[test]
    fn new_file_is_enqueued_after_debounce() {
        let dir = tempfile::tempdir().unwrap();
        let config = test_config(dir.path().to_path_buf());
        let db = Arc::new(Mutex::new(StateDb::open_in_memory().unwrap()));

        let mut watcher = Watcher::new(&config);
        watcher
            .start(config, db.clone(), None)
            .expect("start should succeed");

        let file_path = dir.path().join("screenshot.png");
        fs::write(&file_path, b"fake png bytes").unwrap();

        // Poll for up to a few seconds (debounce window is 2s + stability
        // check ~300ms) rather than a single fixed sleep, to keep the test
        // fast without being flaky.
        let deadline = Instant::now() + Duration::from_secs(8);
        let mut found = false;
        while Instant::now() < deadline {
            if let Ok(Some(record)) = db.lock().unwrap().find_by_path(&file_path) {
                assert_eq!(record.status, crate::state::FileStatus::Pending);
                found = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }

        watcher.stop();
        assert!(
            found,
            "expected file to be enqueued as pending within the timeout"
        );
    }
}

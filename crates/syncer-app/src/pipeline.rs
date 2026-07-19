//! Wires syncer-core's scanner, watcher and uploader together into a single
//! background task running on its own dedicated OS thread with a
//! current-thread Tokio runtime (`LocalSet`/`spawn_local`), rather than on
//! the app's shared multi-thread runtime.
//!
//! This is required because `Uploader::run_pending` holds a `&StateDb`
//! across network awaits, and `rusqlite::Connection` is `Send` but not
//! `Sync` (it uses interior `RefCell`s), so a future holding that borrow
//! cannot be `Send` — which rules out `Runtime::spawn` on a multi-thread
//! runtime. `spawn_local` on a `LocalSet` has no such requirement.
//!
//! Uses two separate `StateDb` connections onto the same SQLite file: one
//! for the watcher/scanner (behind a `std::sync::Mutex`, since the
//! debouncer's callback runs on its own OS thread), and one owned outright
//! by the upload loop. SQLite handles multiple connections to the same file
//! natively. Progress is reported to the UI thread as
//! [`syncer_core::SyncEvent`]s over an unbounded channel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::mpsc::UnboundedSender;
use tokio::sync::watch;

use immich_client::ImmichClient;
use syncer_core::{scan_once_with_events, StateDb, SyncEvent, Uploader, Watcher};

/// Handle to a running pipeline task. The watcher owns OS-level watch
/// handles, so call [`PipelineHandle::shutdown`] on app exit to tear it down
/// cleanly rather than relying solely on process termination.
pub struct PipelineHandle {
    /// Pushes config updates (folder list, intervals, toggles) to the
    /// running loop; it restarts its watcher and picks up new values on its
    /// next iteration.
    config_tx: watch::Sender<syncer_core::Config>,
    shutdown: Arc<AtomicBool>,
    /// Keeps the dedicated pipeline thread alive for the app's lifetime;
    /// joined implicitly on drop only if the thread has already exited
    /// (`JoinHandle`'s drop does not block).
    _thread: std::thread::JoinHandle<()>,
}

impl PipelineHandle {
    pub fn update_config(&self, config: syncer_core::Config) {
        let _ = self.config_tx.send(config);
    }

    pub fn shutdown(&self) {
        self.shutdown.store(true, Ordering::SeqCst);
    }
}

/// Spawns the background scan/watch/upload loop on a dedicated thread.
///
/// `paused` is shared with the tray's Pause/Resume menu item: while set, the
/// loop stops running new scans/uploads (the filesystem watcher keeps
/// enqueueing new files either way, so nothing is missed — it just isn't
/// uploaded until resumed).
pub fn spawn(
    config: syncer_core::Config,
    db_path: std::path::PathBuf,
    events: UnboundedSender<SyncEvent>,
    paused: Arc<AtomicBool>,
) -> PipelineHandle {
    let (config_tx, config_rx) = watch::channel(config);
    let shutdown = Arc::new(AtomicBool::new(false));
    let shutdown_for_thread = shutdown.clone();

    let thread = std::thread::Builder::new()
        .name("vrc-immich-syncer-pipeline".to_string())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(err) => {
                    tracing::error!("pipeline: failed to start dedicated runtime: {err}");
                    return;
                }
            };
            let local = tokio::task::LocalSet::new();
            local.block_on(
                &runtime,
                run_loop(db_path, config_rx, events, paused, shutdown_for_thread),
            );
        })
        .expect("failed to spawn pipeline thread");

    PipelineHandle {
        config_tx,
        shutdown,
        _thread: thread,
    }
}

async fn run_loop(
    db_path: std::path::PathBuf,
    mut config_rx: watch::Receiver<syncer_core::Config>,
    events: UnboundedSender<SyncEvent>,
    paused: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
) {
    let watch_db = match StateDb::open(&db_path) {
        Ok(db) => Arc::new(Mutex::new(db)),
        Err(err) => {
            tracing::error!("pipeline: failed to open state database at {db_path:?}: {err}");
            return;
        }
    };
    let upload_db = match StateDb::open(&db_path) {
        Ok(db) => db,
        Err(err) => {
            tracing::error!("pipeline: failed to open second state database handle: {err}");
            return;
        }
    };

    let mut config = config_rx.borrow().clone();
    let mut watcher = start_watcher(&config, &watch_db, &events);
    let mut last_scan = std::time::Instant::now() - Duration::from_secs(config.scan_interval_secs);
    let mut last_upload_pass = std::time::Instant::now();

    loop {
        if shutdown.load(Ordering::SeqCst) {
            if let Some(w) = watcher.as_mut() {
                w.stop();
            }
            return;
        }

        if config_rx.has_changed().unwrap_or(false) {
            config = config_rx.borrow_and_update().clone();
            tracing::info!("pipeline: config changed, restarting watcher");
            if let Some(mut w) = watcher.take() {
                w.stop();
            }
            watcher = start_watcher(&config, &watch_db, &events);
        }

        if paused.load(Ordering::SeqCst) {
            tokio::time::sleep(Duration::from_millis(500)).await;
            continue;
        }

        if last_scan.elapsed() >= Duration::from_secs(config.scan_interval_secs.max(1)) {
            last_scan = std::time::Instant::now();
            run_scan(&config, &watch_db, &events).await;
        }

        // The uploader drains pending + due-retry rows; run it at least as
        // often as the retry interval so failed uploads actually retry.
        if last_upload_pass.elapsed() >= Duration::from_secs(config.retry_interval_secs.max(1)) {
            last_upload_pass = std::time::Instant::now();
            run_upload_pass(&config, &upload_db, &events).await;
        }

        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

fn start_watcher(
    config: &syncer_core::Config,
    db: &Arc<Mutex<StateDb>>,
    events: &UnboundedSender<SyncEvent>,
) -> Option<Watcher> {
    let mut watcher = Watcher::new(config);
    match watcher.start(config.clone(), db.clone(), Some(events.clone())) {
        Ok(()) => Some(watcher),
        Err(err) => {
            tracing::warn!("pipeline: failed to start filesystem watcher: {err}");
            None
        }
    }
}

async fn run_scan(
    config: &syncer_core::Config,
    db: &Arc<Mutex<StateDb>>,
    events: &UnboundedSender<SyncEvent>,
) {
    let config = config.clone();
    let db = db.clone();
    let events = events.clone();
    let result = tokio::task::spawn_blocking(move || {
        let db = match db.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        };
        scan_once_with_events(&config, &db, Some(&events))
    })
    .await;

    match result {
        Ok(Ok(summary)) => {
            tracing::debug!(
                "pipeline: scan finished, {} seen, {} newly enqueued",
                summary.files_seen,
                summary.newly_enqueued
            );
        }
        Ok(Err(err)) => tracing::warn!("pipeline: scan failed: {err}"),
        Err(err) => tracing::warn!("pipeline: scan task panicked: {err}"),
    }
}

async fn run_upload_pass(
    config: &syncer_core::Config,
    db: &StateDb,
    events: &UnboundedSender<SyncEvent>,
) {
    if config.server_url.trim().is_empty() {
        return;
    }

    let client = ImmichClient::new(config.server_url.clone(), config.api_key.clone());
    let mut uploader = Uploader::new(client, config.clone(), Some(events.clone()));

    match uploader.run_pending(db).await {
        Ok(summary) => {
            if summary.attempted > 0 {
                tracing::debug!(
                    "pipeline: upload pass processed {} ({} ok, {} failed)",
                    summary.attempted,
                    summary.succeeded,
                    summary.failed
                );
            }
        }
        Err(err) => tracing::warn!("pipeline: upload pass failed: {err}"),
    }
}

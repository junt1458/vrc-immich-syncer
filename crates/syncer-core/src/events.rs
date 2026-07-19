//! Lightweight sync events emitted by the core for consumption by the app
//! layer (UI, tray state, notifications).
//!
//! The core never depends on `syncer-app`; it only exposes an
//! `mpsc::UnboundedSender<SyncEvent>` that callers may optionally pass in.
//! Emission is best-effort: if the receiver has been dropped, sends are
//! silently ignored.

use std::path::PathBuf;

/// An event describing progress of the scan/watch/upload pipeline. Consumed
/// by the app layer to drive UI, tray state, and notifications.
#[derive(Debug, Clone, PartialEq)]
pub enum SyncEvent {
    /// A full (or watcher-triggered) scan finished.
    ScanFinished {
        files_seen: usize,
        newly_enqueued: usize,
    },
    /// Upload of a file started.
    UploadStarted { path: PathBuf },
    /// Upload of a file completed successfully (created or recognized as a
    /// server-side duplicate).
    UploadSucceeded { path: PathBuf, asset_id: String },
    /// Upload of a file failed and has been scheduled for retry.
    UploadFailed { path: PathBuf, error: String },
    /// Connectivity to the Immich server changed.
    ConnectionState { connected: bool },
}

/// Sends an event over an optional channel, ignoring send errors (a dropped
/// receiver just means nobody is listening right now).
pub(crate) fn emit(
    sender: Option<&tokio::sync::mpsc::UnboundedSender<SyncEvent>>,
    event: SyncEvent,
) {
    if let Some(sender) = sender {
        let _ = sender.send(event);
    }
}

//! Core sync logic: config, state ledger, scanner, watcher, upload pipeline.
//!
//! UI-free so it stays testable without a display. Implemented across
//! milestones M2b (config/state) and M3 (scanner/watcher/uploader/metadata).

mod config;
mod events;
mod scanner;
mod state;
mod uploader;
mod vrc_metadata;
mod watcher;

pub use config::{
    default_config_path, Config, DEFAULT_RETRY_INTERVAL_SECS, DEFAULT_SCAN_INTERVAL_SECS,
};
pub use events::SyncEvent;
pub use scanner::{scan_once, scan_once_with_events, ScanSummary};
pub use state::{default_state_db_path, FileRecord, FileStatus, StateDb};
pub use uploader::{ImmichUploader, UploadRunSummary, Uploader};
pub use vrc_metadata::{format_description, parse_png_metadata, ScreenshotMetadata};
pub use watcher::Watcher;

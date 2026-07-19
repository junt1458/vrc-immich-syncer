//! Core sync logic: config, state ledger, scanner, watcher, upload pipeline.
//!
//! UI-free so it stays testable without a display. Implemented across
//! milestones M2b (config/state) and M3 (scanner/watcher/uploader/metadata).

mod config;
mod state;

pub use config::{
    default_config_path, Config, DEFAULT_RETRY_INTERVAL_SECS, DEFAULT_SCAN_INTERVAL_SECS,
};
pub use state::{default_state_db_path, FileRecord, FileStatus, StateDb};

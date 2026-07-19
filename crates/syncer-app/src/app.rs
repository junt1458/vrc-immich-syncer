//! App state and runtime wiring: holds `Config`, a `StateDb` handle, a
//! background tokio runtime for async Immich calls, the background scan/
//! watch/upload pipeline (`pipeline.rs`), and the channel used to bring
//! results back onto the egui thread.
//!
//! Event handling is isolated behind [`App::handle_event`] and the local
//! [`AppEvent`] enum rather than being spread across UI widget code.
//! `AppEvent::Sync` wraps the real `syncer_core::SyncEvent`, forwarded here
//! from the pipeline task via `event_tx`.

use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
#[cfg(feature = "tray")]
use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::sync::mpsc;

use immich_client::{ImmichClient, ServerAboutResponse, UserResponse};
use syncer_core::{Config, StateDb};

use crate::notifications::RateLimiter;
use crate::pipeline::PipelineHandle;
#[cfg(feature = "tray")]
use crate::tray::{TrayAction, TrayHandle, TrayState};

/// Which tab of the main window is currently shown.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tab {
    Status,
    RecentUploads,
    Settings,
}

/// Outcome of an async connection/account status probe.
#[derive(Debug, Clone)]
pub struct StatusSnapshot {
    pub connected: bool,
    pub server_version: Option<String>,
    pub account_email: Option<String>,
    pub quota_used_bytes: Option<i64>,
    pub quota_total_bytes: Option<i64>,
    pub error: Option<String>,
}

/// Events fed into [`App::handle_event`]. Kept as our own local enum (rather
/// than coupling UI code directly to `syncer_core::SyncEvent`'s variant
/// names) so that full event-bus wiring in M5 only touches the translation
/// at the call site, not the UI rendering code.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// A background status probe completed.
    StatusUpdated(Box<StatusSnapshot>),
    /// The Settings tab's "Test connection" probe completed, with a
    /// human-readable result message.
    TestConnectionCompleted(String),
    /// A core sync event forwarded from the background pipeline task.
    Sync(syncer_core::SyncEvent),
}

/// Background async work handed to the tokio runtime; results come back via
/// `AppEvent` over `event_tx`/`event_rx`.
enum Command {
    RefreshStatus { server_url: String, api_key: String },
}

/// Top-level application state, implementing `eframe::App`.
pub struct App {
    pub config: Config,
    pub config_error: Option<String>,
    /// Owned directly (not behind `Arc`) since `StateDb` wraps a
    /// `rusqlite::Connection`, which is `Send` but not `Sync`; the DB is
    /// only ever touched from the main egui thread (queries run
    /// synchronously in `tick`/`refresh_queue_counts`, not on the tokio
    /// runtime), so no shared-ownership wrapper is needed.
    pub state_db: Option<StateDb>,
    pub state_db_error: Option<String>,

    pub current_tab: Tab,
    pub status: Option<StatusSnapshot>,
    pub status_pending: bool,
    pub last_scan_time: Option<DateTime<Utc>>,
    pub pending_count: usize,
    pub failed_count: i64,

    /// Editable copies of config fields for the Settings tab (kept separate
    /// from `config` until "Save" is pressed).
    pub settings_draft: SettingsDraft,
    pub test_connection_result: Option<String>,
    pub test_connection_pending: bool,
    pub auto_start_error: Option<String>,
    pub save_result: Option<String>,

    pub notification_rate_limiter: RateLimiter,

    /// Shared with the background pipeline task; while set, scanning/
    /// uploading pauses (the filesystem watcher keeps enqueueing new files
    /// regardless, so nothing is missed, it's just not uploaded yet).
    /// Toggled by the tray's "Pause sync" menu item when the `tray` feature
    /// is enabled.
    pub paused: Arc<AtomicBool>,
    /// Set by the tray "Quit" menu item to request a real process exit.
    pub quit_requested: Arc<AtomicBool>,
    /// Whether the main window should currently be shown. The OS close
    /// button hides to tray instead of exiting (see `update`).
    pub window_visible: bool,

    #[cfg(feature = "tray")]
    pub tray: Option<TrayHandle>,

    /// Handle to the background scan/watch/upload task; `None` if the state
    /// database couldn't be opened (surfaced via `state_db_error` instead).
    pipeline: Option<PipelineHandle>,

    runtime: tokio::runtime::Runtime,
    command_tx: mpsc::UnboundedSender<Command>,
    event_tx: mpsc::UnboundedSender<AppEvent>,
    event_rx: mpsc::UnboundedReceiver<AppEvent>,

    last_status_poll: Option<Instant>,
    last_queue_poll: Option<Instant>,
}

/// Editable draft of [`Config`] backing the Settings tab widgets. Text
/// fields for list-like config values (extensions) are kept as raw strings
/// while being edited.
#[derive(Debug, Clone)]
pub struct SettingsDraft {
    pub server_url: String,
    pub api_key: String,
    pub watch_folders: Vec<PathBuf>,
    pub album_auto_add: bool,
    pub album_name: String,
    pub metadata_to_description: bool,
    pub allowed_extensions_text: String,
    pub scan_interval_secs: String,
    pub retry_interval_secs: String,
    pub auto_start: bool,
}

impl From<&Config> for SettingsDraft {
    fn from(config: &Config) -> Self {
        Self {
            server_url: config.server_url.clone(),
            api_key: config.api_key.clone(),
            watch_folders: config.watch_folders.clone(),
            album_auto_add: config.album_auto_add,
            album_name: config.album_name.clone().unwrap_or_default(),
            metadata_to_description: config.metadata_to_description,
            allowed_extensions_text: crate::format::format_extensions(&config.allowed_extensions),
            scan_interval_secs: config.scan_interval_secs.to_string(),
            retry_interval_secs: config.retry_interval_secs.to_string(),
            auto_start: config.auto_start,
        }
    }
}

impl SettingsDraft {
    /// Applies this draft onto a `Config`, parsing numeric/list fields with
    /// fallbacks to the previous value on parse failure.
    pub fn apply_to(&self, config: &mut Config) {
        config.server_url = self.server_url.trim().to_string();
        config.api_key = self.api_key.clone();
        config.watch_folders = self.watch_folders.clone();
        config.album_auto_add = self.album_auto_add;
        config.album_name = if self.album_name.trim().is_empty() {
            None
        } else {
            Some(self.album_name.trim().to_string())
        };
        config.metadata_to_description = self.metadata_to_description;
        config.allowed_extensions = crate::format::parse_extensions(&self.allowed_extensions_text);
        if let Ok(secs) = self.scan_interval_secs.trim().parse::<u64>() {
            config.scan_interval_secs = secs;
        }
        if let Ok(secs) = self.retry_interval_secs.trim().parse::<u64>() {
            config.retry_interval_secs = secs;
        }
        config.auto_start = self.auto_start;
    }
}

impl App {
    pub fn new(start_minimized: bool) -> Self {
        let (config, config_error) = match Config::load() {
            Ok(config) => (config, None),
            Err(err) => {
                tracing::warn!("failed to load config, falling back to defaults: {err}");
                (Config::default(), Some(format!("{err}")))
            }
        };

        let (state_db, state_db_error) = match syncer_core::default_state_db_path() {
            Some(path) => match StateDb::open(&path) {
                Ok(db) => (Some(db), None),
                Err(err) => (None, Some(format!("failed to open state database: {err}"))),
            },
            None => (
                None,
                Some("could not determine platform data directory for state database".into()),
            ),
        };

        let runtime = tokio::runtime::Runtime::new().expect("failed to start tokio runtime");
        let (command_tx, mut command_rx) = mpsc::unbounded_channel::<Command>();
        let (event_tx, event_rx) = mpsc::unbounded_channel::<AppEvent>();

        // Background task loop: consumes `Command`s, performs async Immich
        // calls, and reports results back as `AppEvent`s.
        let worker_event_tx = event_tx.clone();
        runtime.spawn(async move {
            while let Some(command) = command_rx.recv().await {
                match command {
                    Command::RefreshStatus {
                        server_url,
                        api_key,
                    } => {
                        let snapshot = fetch_status(&server_url, &api_key).await;
                        let _ = worker_event_tx.send(AppEvent::StatusUpdated(Box::new(snapshot)));
                    }
                }
            }
        });

        #[cfg(feature = "tray")]
        let tray = match TrayHandle::new() {
            Ok(handle) => Some(handle),
            Err(err) => {
                tracing::warn!("tray icon initialization failed, continuing window-only: {err}");
                None
            }
        };

        let mut settings_draft = SettingsDraft::from(&config);
        // Prefer the OS's actual auto-start registration state over the
        // persisted config flag, since the two can drift (e.g. the user
        // removed the entry manually). Best-effort: falls back to the
        // config value if the platform backend can't answer.
        if let Ok(actual) = crate::autostart::is_auto_start_enabled() {
            settings_draft.auto_start = actual;
        }

        let paused = Arc::new(AtomicBool::new(false));

        // The pipeline runs on its own dedicated thread/runtime (see
        // pipeline.rs docs) and opens its own StateDb connections onto the
        // same path `state_db` above was opened from. Its `SyncEvent`s
        // arrive on a separate channel and are forwarded into `event_tx` as
        // `AppEvent::Sync` by a small relay task on the shared runtime.
        let pipeline = syncer_core::default_state_db_path().map(|db_path| {
            let (sync_tx, mut sync_rx) = mpsc::unbounded_channel::<syncer_core::SyncEvent>();
            let relay_event_tx = event_tx.clone();
            runtime.spawn(async move {
                while let Some(sync_event) = sync_rx.recv().await {
                    let _ = relay_event_tx.send(AppEvent::Sync(sync_event));
                }
            });
            crate::pipeline::spawn(config.clone(), db_path, sync_tx, paused.clone())
        });

        Self {
            settings_draft,
            config,
            config_error,
            state_db,
            state_db_error,
            current_tab: Tab::Status,
            status: None,
            status_pending: false,
            last_scan_time: None,
            pending_count: 0,
            failed_count: 0,
            test_connection_result: None,
            test_connection_pending: false,
            auto_start_error: None,
            save_result: None,
            notification_rate_limiter: RateLimiter::new(Duration::from_secs(300)),
            paused,
            quit_requested: Arc::new(AtomicBool::new(false)),
            window_visible: !start_minimized,
            #[cfg(feature = "tray")]
            tray,
            pipeline,
            runtime,
            command_tx,
            event_tx,
            event_rx,
            last_status_poll: None,
            last_queue_poll: None,
        }
    }

    /// Central event-handling entry point (see module docs): translates an
    /// `AppEvent` into state mutations. UI code should never reach into
    /// `syncer_core::SyncEvent` directly; it goes through here.
    pub fn handle_event(&mut self, event: AppEvent) {
        match event {
            AppEvent::StatusUpdated(snapshot) => {
                self.status_pending = false;
                self.status = Some(*snapshot);
            }
            AppEvent::TestConnectionCompleted(message) => {
                self.test_connection_pending = false;
                self.test_connection_result = Some(message);
            }
            AppEvent::Sync(sync_event) => {
                tracing::debug!("received sync event: {sync_event:?}");
                let is_failure = matches!(sync_event, syncer_core::SyncEvent::UploadFailed { .. });
                let is_success =
                    matches!(sync_event, syncer_core::SyncEvent::UploadSucceeded { .. });

                if let syncer_core::SyncEvent::ScanFinished { .. } = &sync_event {
                    self.last_scan_time = Some(Utc::now());
                }

                if is_failure && self.notification_rate_limiter.should_send(Instant::now()) {
                    crate::notifications::notify_upload_failure(1);
                }
                // A success after a failure streak is treated as recovery;
                // resets the limiter so the next failure batch is reported
                // promptly, and shows a "recovered" toast if a failure
                // toast had actually been shown before.
                if is_success && self.notification_rate_limiter.reset() {
                    crate::notifications::notify_recovered();
                }

                // A finished scan or upload is a good time to refresh the
                // queue counts shown on the Status tab without waiting for
                // the next 5s poll.
                if is_success
                    || is_failure
                    || matches!(sync_event, syncer_core::SyncEvent::ScanFinished { .. })
                {
                    self.refresh_queue_counts();
                }
            }
        }
    }

    /// Drains any events that arrived on the channel since the last frame.
    pub fn drain_events(&mut self) {
        while let Ok(event) = self.event_rx.try_recv() {
            self.handle_event(event);
        }
    }

    /// Kicks off an async status refresh if the server URL/API key are set.
    /// Safe to call repeatedly; only enqueues a new probe if one isn't
    /// already in flight.
    pub fn request_status_refresh(&mut self) {
        if self.status_pending {
            return;
        }
        if self.config.server_url.trim().is_empty() {
            return;
        }
        self.status_pending = true;
        let _ = self.command_tx.send(Command::RefreshStatus {
            server_url: self.config.server_url.clone(),
            api_key: self.config.api_key.clone(),
        });
    }

    /// Re-reads pending/failed upload counts from the state DB. Cheap enough
    /// to call on a timer; failures are logged and leave the previous count.
    pub fn refresh_queue_counts(&mut self) {
        if let Some(db) = &self.state_db {
            match db.get_pending() {
                Ok(rows) => self.pending_count = rows.len(),
                Err(err) => tracing::warn!("failed to query pending uploads: {err}"),
            }
            match db.count_failed() {
                Ok(count) => self.failed_count = count,
                Err(err) => tracing::warn!("failed to query failed uploads: {err}"),
            }
        }
    }

    /// Called once per frame from `update()` to do lightweight periodic
    /// polling (status + queue counts) without spamming the server/DB.
    pub fn tick(&mut self) {
        self.drain_events();

        let now = Instant::now();

        let should_poll_status = self
            .last_status_poll
            .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(60));
        if should_poll_status {
            self.last_status_poll = Some(now);
            self.request_status_refresh();
        }

        let should_poll_queue = self
            .last_queue_poll
            .is_none_or(|t| now.duration_since(t) >= Duration::from_secs(5));
        if should_poll_queue {
            self.last_queue_poll = Some(now);
            self.refresh_queue_counts();
        }
    }

    /// Runs a one-off "Test connection" probe from the Settings tab, using
    /// the *draft* server URL/API key rather than the saved config.
    pub fn test_connection(&mut self) {
        if self.test_connection_pending {
            return;
        }
        self.test_connection_pending = true;
        self.test_connection_result = None;

        let server_url = self.settings_draft.server_url.clone();
        let api_key = self.settings_draft.api_key.clone();
        let event_tx = self.event_tx.clone();

        self.runtime.spawn(async move {
            let client = ImmichClient::new(server_url, api_key);
            let result = match client.validate_token().await {
                Ok(resp) if resp.auth_status => "Connection OK: API key is valid.".to_string(),
                Ok(_) => "Connected, but the API key was rejected.".to_string(),
                Err(err) => format!("Connection failed: {err}"),
            };
            let _ = event_tx.send(AppEvent::TestConnectionCompleted(result));
        });
    }

    /// Saves the current settings draft to `self.config`, persists it, and
    /// live-applies it to the running pipeline (restarts the watcher with
    /// the new folder list, picks up new intervals/toggles).
    pub fn save_settings(&mut self) {
        self.settings_draft.apply_to(&mut self.config);
        match self.config.save() {
            Ok(()) => {
                self.save_result = Some("Settings saved.".to_string());
                if let Some(pipeline) = &self.pipeline {
                    pipeline.update_config(self.config.clone());
                }
            }
            Err(err) => {
                self.save_result = Some(format!("Failed to save settings: {err}"));
            }
        }

        match crate::autostart::set_auto_start(self.config.auto_start) {
            Ok(()) => self.auto_start_error = None,
            Err(err) => self.auto_start_error = Some(format!("{err}")),
        }
    }

    /// Handles a tray action (see `tray.rs`), applied uniformly regardless
    /// of whether it came from the menu or a left-click.
    #[cfg(feature = "tray")]
    pub fn handle_tray_action(&mut self, action: TrayAction) {
        match action {
            TrayAction::Open => {
                self.window_visible = true;
            }
            TrayAction::TogglePause => {
                let was_paused = self.paused.fetch_xor(true, Ordering::SeqCst);
                tracing::info!("sync {}", if was_paused { "resumed" } else { "paused" });
            }
            TrayAction::Quit => {
                if let Some(pipeline) = &self.pipeline {
                    pipeline.shutdown();
                }
                self.quit_requested.store(true, Ordering::SeqCst);
            }
        }
    }

    /// Whether the pipeline is currently paused (only settable via the tray
    /// menu, but readable regardless so the Status tab can reflect it).
    pub fn is_paused(&self) -> bool {
        self.paused.load(std::sync::atomic::Ordering::SeqCst)
    }

    #[cfg(feature = "tray")]
    pub fn set_tray_state(&mut self, state: TrayState) {
        if let Some(tray) = &mut self.tray {
            if let Err(err) = tray.set_state(state) {
                tracing::warn!("failed to update tray icon state: {err}");
            }
        }
    }
}

impl Drop for App {
    fn drop(&mut self) {
        if let Some(pipeline) = &self.pipeline {
            pipeline.shutdown();
        }
    }
}

async fn fetch_status(server_url: &str, api_key: &str) -> StatusSnapshot {
    let client = ImmichClient::new(server_url, api_key);

    let ping_ok = client.ping().await.is_ok();

    let about: Option<ServerAboutResponse> = client.server_about().await.ok();
    let user: Option<UserResponse> = client.get_my_user().await.ok();

    let error = if !ping_ok {
        Some("Could not reach the Immich server.".to_string())
    } else if user.is_none() {
        Some("Connected, but failed to fetch account info (check API key).".to_string())
    } else {
        None
    };

    StatusSnapshot {
        connected: ping_ok,
        server_version: about.map(|a| a.version),
        account_email: user.as_ref().map(|u| u.email.clone()),
        quota_used_bytes: user.as_ref().and_then(|u| u.quota_usage_in_bytes),
        quota_total_bytes: user.as_ref().and_then(|u| u.quota_size_in_bytes),
        error,
    }
}

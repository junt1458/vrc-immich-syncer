//! Tray icon integration (`tray-icon` crate), feature-gated behind `tray`.
//!
//! Builds simple generated idle/syncing/error icons (solid-color squares —
//! real artwork is M6 polish), a menu with Open / Pause-Resume / Quit, and
//! exposes a small polling API the main `App::update` loop calls each frame
//! to react to tray icon / menu events. If tray init fails, `TrayHandle::new`
//! returns `Err` and the caller falls back to window-only operation, per
//! PLAN.md 3.3 ("app must still run with the window alone if tray init
//! fails").

use tray_icon::menu::{Menu, MenuEvent, MenuId, MenuItem, PredefinedMenuItem};
use tray_icon::{TrayIcon, TrayIconBuilder, TrayIconEvent};

/// Visual/semantic state of the tray icon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayState {
    Idle,
    Syncing,
    Error,
}

/// Actions the tray menu/icon can request of the main app loop, surfaced by
/// [`TrayHandle::poll`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayAction {
    /// Open/focus the main window (menu item or left-click on the icon).
    Open,
    /// Toggle pause/resume of the sync pipeline.
    TogglePause,
    /// Exit the application entirely.
    Quit,
}

/// Owns the live tray icon and menu item ids so incoming events can be
/// matched back to actions.
pub struct TrayHandle {
    _tray_icon: TrayIcon,
    open_id: MenuId,
    pause_id: MenuId,
    quit_id: MenuId,
    pause_item: MenuItem,
    paused: bool,
}

impl TrayHandle {
    /// Builds the tray icon and menu. Returns `Err` if tray initialization
    /// fails (e.g. no system tray host available), in which case the caller
    /// should continue running window-only.
    pub fn new() -> anyhow::Result<Self> {
        let open_item = MenuItem::new("Open", true, None);
        let pause_item = MenuItem::new("Pause sync", true, None);
        let quit_item = MenuItem::new("Quit", true, None);

        let open_id = open_item.id().clone();
        let pause_id = pause_item.id().clone();
        let quit_id = quit_item.id().clone();

        let menu = Menu::new();
        menu.append(&open_item)
            .map_err(|e| anyhow::anyhow!("failed to build tray menu: {e}"))?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|e| anyhow::anyhow!("failed to build tray menu: {e}"))?;
        menu.append(&pause_item)
            .map_err(|e| anyhow::anyhow!("failed to build tray menu: {e}"))?;
        menu.append(&PredefinedMenuItem::separator())
            .map_err(|e| anyhow::anyhow!("failed to build tray menu: {e}"))?;
        menu.append(&quit_item)
            .map_err(|e| anyhow::anyhow!("failed to build tray menu: {e}"))?;

        let icon = build_icon(TrayState::Idle)?;

        let tray_icon = TrayIconBuilder::new()
            .with_menu(Box::new(menu))
            .with_tooltip("vrc-immich-syncer")
            .with_icon(icon)
            .build()
            .map_err(|e| anyhow::anyhow!("failed to create tray icon: {e}"))?;

        Ok(Self {
            _tray_icon: tray_icon,
            open_id,
            pause_id,
            quit_id,
            pause_item,
            paused: false,
        })
    }

    /// Non-blocking poll of pending tray/menu events, translated into at
    /// most one [`TrayAction`] per call. Call this once per UI frame.
    pub fn poll(&mut self) -> Option<TrayAction> {
        if let Ok(event) = MenuEvent::receiver().try_recv() {
            if event.id() == &self.open_id {
                return Some(TrayAction::Open);
            }
            if event.id() == &self.pause_id {
                self.paused = !self.paused;
                self.pause_item.set_text(if self.paused {
                    "Resume sync"
                } else {
                    "Pause sync"
                });
                return Some(TrayAction::TogglePause);
            }
            if event.id() == &self.quit_id {
                return Some(TrayAction::Quit);
            }
        }

        if let Ok(TrayIconEvent::Click { .. }) = TrayIconEvent::receiver().try_recv() {
            return Some(TrayAction::Open);
        }

        None
    }

    /// Updates the tray icon image to reflect the given state (idle /
    /// syncing / error).
    pub fn set_state(&mut self, state: TrayState) -> anyhow::Result<()> {
        let icon = build_icon(state)?;
        self._tray_icon
            .set_icon(Some(icon))
            .map_err(|e| anyhow::anyhow!("failed to update tray icon: {e}"))
    }
}

/// Builds a simple solid-color 32x32 RGBA square icon for the given state.
/// No external image assets needed; real artwork is deferred to M6 polish.
fn build_icon(state: TrayState) -> anyhow::Result<tray_icon::Icon> {
    const SIZE: u32 = 32;
    let (r, g, b) = match state {
        TrayState::Idle => (0x3a, 0x7c, 0xd8),    // blue
        TrayState::Syncing => (0x2f, 0xa8, 0x4f), // green
        TrayState::Error => (0xd8, 0x3a, 0x3a),   // red
    };

    let mut rgba = Vec::with_capacity((SIZE * SIZE * 4) as usize);
    for _ in 0..(SIZE * SIZE) {
        rgba.extend_from_slice(&[r, g, b, 0xff]);
    }

    tray_icon::Icon::from_rgba(rgba, SIZE, SIZE)
        .map_err(|e| anyhow::anyhow!("failed to build tray icon: {e}"))
}

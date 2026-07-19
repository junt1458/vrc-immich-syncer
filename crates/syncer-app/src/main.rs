//! vrc-immich-syncer resident client entry point.
//!
//! Wires syncer-core's watch/scan/upload pipeline to an egui window, tray
//! icon and OS notifications. The scan/watch/upload pipeline itself is
//! integrated in M5; this milestone (M4) builds the window/tray/
//! notification/settings skeleton.

mod app;
mod autostart;
mod eframe_app;
mod format;
mod icon;
mod notifications;
mod pipeline;
#[cfg(feature = "tray")]
mod tray;
mod ui;

use app::App;

/// Window/taskbar icon color: matches the tray icon's idle (blue) state.
const WINDOW_ICON_COLOR: (u8, u8, u8) = (0x3a, 0x7c, 0xd8);
const WINDOW_ICON_SIZE: u32 = 64;

fn main() {
    tracing_subscriber::fmt::init();

    let start_minimized = std::env::args().any(|arg| arg == "--minimized");

    let icon_rgba = icon::generate_icon_rgba(WINDOW_ICON_COLOR, WINDOW_ICON_SIZE);
    let icon_data = egui::IconData {
        rgba: icon_rgba,
        width: WINDOW_ICON_SIZE,
        height: WINDOW_ICON_SIZE,
    };

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([760.0, 560.0])
        .with_title("vrc-immich-syncer")
        .with_icon(icon_data);
    if start_minimized {
        viewport = viewport.with_visible(false);
    }

    let native_options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };

    let result = eframe::run_native(
        "vrc-immich-syncer",
        native_options,
        Box::new(move |_cc| Ok(Box::new(App::new(start_minimized)))),
    );

    if let Err(err) = result {
        tracing::error!("eframe exited with error: {err}");
        std::process::exit(1);
    }
}

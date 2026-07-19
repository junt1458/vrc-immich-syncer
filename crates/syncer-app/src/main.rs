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
mod notifications;
mod pipeline;
#[cfg(feature = "tray")]
mod tray;
mod ui;

use app::App;

fn main() {
    tracing_subscriber::fmt::init();

    let start_minimized = std::env::args().any(|arg| arg == "--minimized");

    let mut viewport = egui::ViewportBuilder::default()
        .with_inner_size([760.0, 560.0])
        .with_title("vrc-immich-syncer");
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

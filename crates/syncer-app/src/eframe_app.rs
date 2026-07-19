//! `eframe::App` implementation: frame loop, close-to-tray handling, and
//! dispatch into the per-tab UI rendering functions in `ui.rs`.

use std::sync::atomic::Ordering;
use std::time::Duration;

use crate::app::{App, Tab};
use crate::ui;

#[cfg(feature = "tray")]
use crate::tray::TrayState;

impl eframe::App for App {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.tick();

        #[cfg(feature = "tray")]
        {
            if let Some(mut tray) = self.tray.take() {
                if let Some(action) = tray.poll() {
                    self.tray = Some(tray);
                    self.handle_tray_action(action);
                } else {
                    self.tray = Some(tray);
                }
            }

            let state = if self.status.as_ref().is_some_and(|s| !s.connected) {
                TrayState::Error
            } else if self.status_pending {
                TrayState::Syncing
            } else {
                TrayState::Idle
            };
            self.set_tray_state(state);
        }

        if self.quit_requested.load(Ordering::SeqCst) {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            std::process::exit(0);
        }

        // Intercept the OS close button: hide to tray instead of exiting,
        // unless there is no tray (feature off or init failed) in which
        // case closing the window is the only way to quit, so let it
        // through.
        let close_requested = ctx.input(|i| i.viewport().close_requested());
        if close_requested {
            #[cfg(feature = "tray")]
            let has_tray = self.tray.is_some();
            #[cfg(not(feature = "tray"))]
            let has_tray = false;

            if has_tray {
                self.window_visible = false;
                ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                ctx.send_viewport_cmd(egui::ViewportCommand::Visible(false));
            }
            // else: no tray available, allow the close to proceed (app exits).
        }

        ctx.send_viewport_cmd(egui::ViewportCommand::Visible(self.window_visible));

        if !self.window_visible {
            // Still tick periodically so status/queue polling keeps working
            // while hidden in the tray; avoid a busy loop.
            ctx.request_repaint_after(Duration::from_millis(500));
            return;
        }

        egui::TopBottomPanel::top("tabs").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.selectable_value(&mut self.current_tab, Tab::Status, "Status");
                ui.selectable_value(&mut self.current_tab, Tab::RecentUploads, "Recent uploads");
                ui.selectable_value(&mut self.current_tab, Tab::Settings, "Settings");
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| match self.current_tab {
            Tab::Status => ui::status::show(self, ui),
            Tab::RecentUploads => ui::recent::show(self, ui),
            Tab::Settings => ui::settings::show(self, ui),
        });

        ctx.request_repaint_after(Duration::from_millis(500));
    }
}

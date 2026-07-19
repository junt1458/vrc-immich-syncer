//! Status tab: connection state, server version, account info, storage
//! quota, queue counts, last scan time.

use crate::app::App;
use crate::format::format_quota;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Status");
    ui.add_space(8.0);

    if let Some(err) = &app.config_error {
        ui.colored_label(
            egui::Color32::from_rgb(0xd8, 0x3a, 0x3a),
            format!("Config failed to load, using defaults: {err}"),
        );
        ui.add_space(4.0);
    }
    if let Some(err) = &app.state_db_error {
        ui.colored_label(
            egui::Color32::from_rgb(0xd8, 0x3a, 0x3a),
            format!("State database unavailable: {err}"),
        );
        ui.add_space(4.0);
    }

    egui::Grid::new("status_grid")
        .num_columns(2)
        .spacing([16.0, 6.0])
        .show(ui, |ui| {
            ui.label("Connection:");
            match &app.status {
                Some(status) if status.connected => {
                    ui.colored_label(egui::Color32::from_rgb(0x2f, 0xa8, 0x4f), "Connected");
                }
                Some(_) => {
                    ui.colored_label(egui::Color32::from_rgb(0xd8, 0x3a, 0x3a), "Disconnected");
                }
                None if app.config.server_url.trim().is_empty() => {
                    ui.label("Not configured (set a server URL in Settings)");
                }
                None => {
                    ui.label(if app.status_pending {
                        "Checking..."
                    } else {
                        "Unknown"
                    });
                }
            }
            ui.end_row();

            ui.label("Server version:");
            ui.label(
                app.status
                    .as_ref()
                    .and_then(|s| s.server_version.as_deref())
                    .unwrap_or("-"),
            );
            ui.end_row();

            ui.label("Account:");
            ui.label(
                app.status
                    .as_ref()
                    .and_then(|s| s.account_email.as_deref())
                    .unwrap_or("-"),
            );
            ui.end_row();

            ui.label("Storage quota:");
            match &app.status {
                Some(status) => {
                    ui.label(format_quota(
                        status.quota_used_bytes,
                        status.quota_total_bytes,
                    ));
                }
                None => {
                    ui.label("-");
                }
            }
            ui.end_row();

            ui.label("Pending uploads:");
            ui.label(app.pending_count.to_string());
            ui.end_row();

            ui.label("Failed uploads:");
            ui.label(app.failed_count.to_string());
            ui.end_row();

            ui.label("Last scan:");
            ui.label(
                app.last_scan_time
                    .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                    .unwrap_or_else(|| "never yet".to_string()),
            );
            ui.end_row();

            ui.label("Sync:");
            if app.is_paused() {
                ui.colored_label(egui::Color32::from_rgb(0xd8, 0x8a, 0x3a), "Paused");
            } else {
                ui.label("Running");
            }
            ui.end_row();
        });

    if let Some(status) = &app.status {
        if let Some(error) = &status.error {
            ui.add_space(8.0);
            ui.colored_label(egui::Color32::from_rgb(0xd8, 0x3a, 0x3a), error);
        }
    }

    ui.add_space(12.0);
    if ui.button("Refresh now").clicked() {
        app.status_pending = false; // allow an immediate re-request
        app.request_status_refresh();
        app.refresh_queue_counts();
    }
}

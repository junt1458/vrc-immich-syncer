//! Recent uploads tab: the latest uploaded/duplicate files from the state
//! DB, newest first. Clicking a row opens the asset in the browser.

use crate::app::App;

const RECENT_LIMIT: u32 = 50;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Recent uploads");
    ui.add_space(8.0);

    let Some(db) = &app.state_db else {
        ui.label("State database unavailable.");
        return;
    };

    let records = match db.get_recent_uploads(RECENT_LIMIT) {
        Ok(records) => records,
        Err(err) => {
            ui.colored_label(
                egui::Color32::from_rgb(0xd8, 0x3a, 0x3a),
                format!("Failed to load recent uploads: {err}"),
            );
            return;
        }
    };

    if records.is_empty() {
        ui.label("No uploads yet.");
        return;
    }

    let server_url = app
        .config
        .server_url
        .trim()
        .trim_end_matches('/')
        .to_string();

    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("recent_uploads_grid")
            .num_columns(3)
            .spacing([16.0, 4.0])
            .striped(true)
            .show(ui, |ui| {
                for record in &records {
                    let file_name = record
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_else(|| record.path.display().to_string());
                    ui.label(file_name);

                    let uploaded_at = record
                        .uploaded_at
                        .and_then(|ts| chrono::DateTime::from_timestamp(ts, 0))
                        .map(|t| t.format("%Y-%m-%d %H:%M:%S UTC").to_string())
                        .unwrap_or_else(|| "-".to_string());
                    ui.label(uploaded_at);

                    if let Some(asset_id) = &record.asset_id {
                        if !server_url.is_empty() {
                            if ui.link("Open").clicked() {
                                let url = format!("{server_url}/photos/{asset_id}");
                                if let Err(err) = open::that(&url) {
                                    tracing::warn!("failed to open {url}: {err}");
                                }
                            }
                        } else {
                            ui.weak("(set server URL)");
                        }
                    } else {
                        ui.weak("-");
                    }
                    ui.end_row();
                }
            });
    });
}

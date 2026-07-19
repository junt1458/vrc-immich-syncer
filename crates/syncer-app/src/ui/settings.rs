//! Settings tab: server connection, watch folders, album/metadata options,
//! extensions, intervals, auto-start, and a test-notification button.
//!
//! NOTE: Saving persists to disk via `Config::save()` but does not live-apply
//! (restarting watchers, rescheduling the scanner) — that requires running
//! watcher/scanner handles that don't exist until M3/M5 wire them in.

use crate::app::App;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Settings");
    ui.add_space(8.0);

    egui::ScrollArea::vertical().show(ui, |ui| {
        ui.label("Server URL:");
        ui.text_edit_singleline(&mut app.settings_draft.server_url);
        ui.add_space(6.0);

        ui.label("API key:");
        ui.add(
            egui::TextEdit::singleline(&mut app.settings_draft.api_key)
                .password(true)
                .desired_width(f32::INFINITY),
        );
        ui.add_space(4.0);

        ui.horizontal(|ui| {
            if ui.button("Test connection").clicked() {
                app.test_connection();
            }
            if app.test_connection_pending {
                ui.spinner();
            }
        });
        if !app.test_connection_pending {
            if let Some(msg) = &app.test_connection_result {
                ui.label(msg);
            }
        }

        ui.add_space(12.0);
        ui.separator();
        ui.label("Watch folders:");
        let mut remove_index = None;
        for (idx, folder) in app.settings_draft.watch_folders.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.label(folder.display().to_string());
                if ui.small_button("Remove").clicked() {
                    remove_index = Some(idx);
                }
            });
        }
        if let Some(idx) = remove_index {
            app.settings_draft.watch_folders.remove(idx);
        }
        if ui.button("Add folder...").clicked() {
            if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                if !app.settings_draft.watch_folders.contains(&dir) {
                    app.settings_draft.watch_folders.push(dir);
                }
            }
        }

        ui.add_space(12.0);
        ui.separator();
        ui.checkbox(
            &mut app.settings_draft.album_auto_add,
            "Auto-add uploads to an album",
        );
        ui.add_enabled_ui(app.settings_draft.album_auto_add, |ui| {
            ui.horizontal(|ui| {
                ui.label("Album name:");
                ui.text_edit_singleline(&mut app.settings_draft.album_name);
            });
        });

        ui.add_space(8.0);
        ui.checkbox(
            &mut app.settings_draft.metadata_to_description,
            "Write VRChat/VRCX screenshot metadata to the asset description",
        );

        ui.add_space(12.0);
        ui.separator();
        ui.label("Allowed extensions (comma-separated):");
        ui.text_edit_singleline(&mut app.settings_draft.allowed_extensions_text);

        ui.add_space(12.0);
        ui.separator();
        egui::Grid::new("intervals_grid")
            .num_columns(2)
            .spacing([16.0, 6.0])
            .show(ui, |ui| {
                ui.label("Scan interval (seconds):");
                ui.text_edit_singleline(&mut app.settings_draft.scan_interval_secs);
                ui.end_row();

                ui.label("Retry interval (seconds):");
                ui.text_edit_singleline(&mut app.settings_draft.retry_interval_secs);
                ui.end_row();
            });

        ui.add_space(12.0);
        ui.separator();
        if ui
            .checkbox(
                &mut app.settings_draft.auto_start,
                "Start automatically at login",
            )
            .changed()
        {
            // Applied immediately (not deferred to Save) since it's an OS
            // registration action independent of the rest of the config;
            // errors are surfaced rather than panicking.
            if let Err(err) = crate::autostart::set_auto_start(app.settings_draft.auto_start) {
                app.auto_start_error = Some(format!("{err}"));
            } else {
                app.auto_start_error = None;
            }
        }
        if let Some(err) = &app.auto_start_error {
            ui.colored_label(egui::Color32::from_rgb(0xd8, 0x3a, 0x3a), err);
        }

        ui.add_space(16.0);
        ui.separator();
        if ui.button("Send test notification").clicked() {
            crate::notifications::notify_test();
        }

        ui.add_space(16.0);
        ui.separator();
        if ui.button("Save").clicked() {
            app.save_settings();
        }
        if let Some(msg) = &app.save_result {
            ui.label(msg);
        }
    });
}

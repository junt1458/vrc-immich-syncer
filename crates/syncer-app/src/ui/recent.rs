//! Recent uploads tab.
//!
//! `StateDb::get_pending()` is the only query safely available from
//! syncer-app right now; there is no "recently uploaded, ordered by
//! uploaded_at DESC" query exposed yet. Rather than reaching around the
//! crate boundary with an ad-hoc raw SELECT here (which would violate
//! AGENTS.md's "keep UI-free logic inside syncer-core" rule), this tab is a
//! placeholder empty state. Follow-up for M5: add a
//! `StateDb::get_recent_uploads(limit)` query in syncer-core and wire it in
//! here, with each row clickable to open
//! `{server_url}/photos/{asset_id}` via `open::that`.

use crate::app::App;

pub fn show(app: &mut App, ui: &mut egui::Ui) {
    ui.heading("Recent uploads");
    ui.add_space(8.0);

    ui.label("No uploads yet.");
    ui.add_space(4.0);
    ui.weak(
        "This list will populate once the scanner/uploader pipeline (M3) and a \
         recent-uploads query (M5) are wired in. Click a row will open the asset \
         in the browser once populated.",
    );

    // Keep `app` used (avoids an unused-parameter lint if the tab ever
    // becomes purely static); also a natural spot to trigger a manual
    // refresh once the real query exists.
    let _ = &app.pending_count;
}

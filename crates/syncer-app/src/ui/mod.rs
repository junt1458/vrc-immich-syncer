//! Per-tab UI rendering. Kept separate from `app.rs` (state) and
//! `eframe_app.rs` (frame loop) so each tab's egui code stays small and
//! easy to scan.

pub mod recent;
pub mod settings;
pub mod status;

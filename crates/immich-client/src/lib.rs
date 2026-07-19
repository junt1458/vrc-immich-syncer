//! Typed async client for the Immich REST API.
//!
//! This crate contains no application logic — only HTTP calls and their
//! request/response types. Authentication uses the `x-api-key` header.

mod client;
mod error;
mod models;
#[cfg(test)]
mod tests;

pub use client::ImmichClient;
pub use error::ImmichError;
pub use models::{
    Album, AlbumAddAssetsResult, AssetId, AssetResponse, AssetUploadResponse, AssetUploadStatus,
    BulkUploadAction, BulkUploadCheckItem, BulkUploadCheckResult, PingResponse,
    ServerAboutResponse, UploadAssetRequest, UserResponse, ValidateTokenResponse,
};

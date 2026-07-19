//! Request/response types for the Immich REST API.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

/// Response from `GET /api/server/ping`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PingResponse {
    pub res: String,
}

/// Response from `GET /api/server/about`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ServerAboutResponse {
    pub version: String,
    #[serde(default)]
    pub version_url: Option<String>,
    #[serde(default)]
    pub repository: Option<String>,
    #[serde(default)]
    pub build: Option<String>,
    #[serde(default)]
    pub build_url: Option<String>,
    #[serde(default)]
    pub source_ref: Option<String>,
    #[serde(default)]
    pub source_commit: Option<String>,
    #[serde(default)]
    pub source_url: Option<String>,
    #[serde(default)]
    pub exiftool: Option<String>,
    #[serde(default)]
    pub ffmpeg: Option<String>,
    #[serde(default)]
    pub libvips: Option<String>,
    #[serde(default)]
    pub imagemagick: Option<String>,
    #[serde(default)]
    pub nodejs: Option<String>,
}

/// Response from `POST /api/auth/validateToken`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ValidateTokenResponse {
    #[serde(rename = "authStatus")]
    pub auth_status: bool,
}

/// Response from `GET /api/users/me`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UserResponse {
    pub id: String,
    pub email: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub quota_size_in_bytes: Option<i64>,
    #[serde(default)]
    pub quota_usage_in_bytes: Option<i64>,
    #[serde(default)]
    pub is_admin: bool,
}

/// Outcome of uploading (or attempting to upload) an asset.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AssetUploadStatus {
    Created,
    Duplicate,
    /// Reserved for forward-compat with server responses we don't expect;
    /// any status string not recognized above falls back to this variant.
    #[serde(other)]
    Unknown,
}

/// Response from `POST /api/assets`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
pub struct AssetUploadResponse {
    pub id: String,
    pub status: AssetUploadStatus,
}

/// Metadata fields sent alongside the file bytes when uploading an asset.
#[derive(Debug, Clone)]
pub struct UploadAssetRequest {
    /// Raw file bytes.
    pub file_bytes: Vec<u8>,
    /// File name to report to the server (used for content-type sniffing too).
    pub file_name: String,
    /// Stable per-device identifier for this asset (e.g. the local file path).
    pub device_asset_id: String,
    /// Stable identifier for this client device.
    pub device_id: String,
    /// Original file creation time.
    pub file_created_at: DateTime<Utc>,
    /// Original file modification time.
    pub file_modified_at: DateTime<Utc>,
    /// Whether this asset should be favorited on upload.
    pub is_favorite: bool,
}

impl UploadAssetRequest {
    pub fn new(
        file_bytes: Vec<u8>,
        file_name: impl Into<String>,
        device_asset_id: impl Into<String>,
        device_id: impl Into<String>,
        file_created_at: DateTime<Utc>,
        file_modified_at: DateTime<Utc>,
    ) -> Self {
        Self {
            file_bytes,
            file_name: file_name.into(),
            device_asset_id: device_asset_id.into(),
            device_id: device_id.into(),
            file_created_at,
            file_modified_at,
            is_favorite: false,
        }
    }
}

/// One item in a `POST /api/assets/bulk-upload-check` request.
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkUploadCheckItem {
    /// Caller-chosen identifier (echoed back in the response); typically the
    /// local `deviceAssetId` or file path so results can be matched back up.
    pub id: String,
    /// SHA-1 checksum of the file, as a hex string.
    pub checksum: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BulkUploadCheckRequest {
    pub assets: Vec<BulkUploadCheckItem>,
}

/// Per-item duplicate/accept action returned by bulk-upload-check.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BulkUploadAction {
    Accept,
    Reject,
    #[serde(other)]
    Unknown,
}

/// Result for a single asset from `POST /api/assets/bulk-upload-check`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BulkUploadCheckResult {
    pub id: String,
    pub action: BulkUploadAction,
    /// Present when `action == Reject` with reason `"duplicate"`.
    #[serde(default)]
    pub reason: Option<String>,
    /// The existing asset id, present when the asset is a duplicate.
    #[serde(default)]
    pub asset_id: Option<String>,
}

impl BulkUploadCheckResult {
    /// True if the server already has this checksum on file.
    pub fn is_duplicate(&self) -> bool {
        self.action == BulkUploadAction::Reject && self.reason.as_deref() == Some("duplicate")
    }
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct BulkUploadCheckResponse {
    pub results: Vec<BulkUploadCheckResult>,
}

/// An Immich album summary as returned by `GET /api/albums`.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Album {
    pub id: String,
    pub album_name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub asset_count: i64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct CreateAlbumRequest {
    pub album_name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct AddAssetsToAlbumRequest {
    pub ids: Vec<String>,
}

/// Per-asset result of adding assets to an album.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AlbumAddAssetsResult {
    pub id: String,
    pub success: bool,
    #[serde(default)]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct UpdateAssetRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub is_favorite: Option<bool>,
}

/// Response from `PUT /api/assets/{id}`.
#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AssetResponse {
    pub id: String,
    #[serde(default)]
    pub description: Option<String>,
}

/// Helper newtype so callers can pass either a `Uuid` or a `String` id where
/// convenient. Not strictly required by the API (ids are plain strings on
/// the wire) but keeps call sites honest.
pub type AssetId = Uuid;

//! The [`ImmichClient`] — a thin async wrapper over the Immich REST API.

use reqwest::{Client, StatusCode};
use serde::de::DeserializeOwned;

use crate::error::{ImmichError, ImmichErrorBody};
use crate::models::{
    AddAssetsToAlbumRequest, Album, AlbumAddAssetsResult, AssetResponse, AssetUploadResponse,
    BulkUploadCheckItem, BulkUploadCheckRequest, BulkUploadCheckResponse, BulkUploadCheckResult,
    CreateAlbumRequest, PingResponse, ServerAboutResponse, UpdateAssetRequest, UploadAssetRequest,
    UserResponse, ValidateTokenResponse,
};

const API_KEY_HEADER: &str = "x-api-key";

/// Async client for the Immich REST API, authenticated via an `x-api-key`
/// header. Contains no application logic beyond building requests and
/// decoding responses.
#[derive(Debug, Clone)]
pub struct ImmichClient {
    http: Client,
    base_url: String,
    api_key: String,
}

impl ImmichClient {
    /// Build a new client. `base_url` should be the server root, e.g.
    /// `https://immich.example.com` (a trailing slash is tolerated).
    pub fn new(base_url: impl Into<String>, api_key: impl Into<String>) -> Self {
        Self::with_http_client(Client::new(), base_url, api_key)
    }

    /// Build a new client backed by a caller-provided `reqwest::Client`
    /// (useful for custom TLS settings, timeouts, or test doubles).
    pub fn with_http_client(
        http: Client,
        base_url: impl Into<String>,
        api_key: impl Into<String>,
    ) -> Self {
        let mut base_url = base_url.into();
        if base_url.ends_with('/') {
            base_url.pop();
        }
        Self {
            http,
            base_url,
            api_key: api_key.into(),
        }
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    /// Turn a response into `Result<T, ImmichError>`, handling non-2xx
    /// statuses (including a dedicated `Unauthorized` variant for 401) and
    /// JSON decode failures uniformly.
    async fn decode<T: DeserializeOwned>(response: reqwest::Response) -> Result<T, ImmichError> {
        let status = response.status();
        let bytes = response.bytes().await?;

        if !status.is_success() {
            if status == StatusCode::UNAUTHORIZED {
                return Err(ImmichError::Unauthorized);
            }
            let message = serde_json::from_slice::<ImmichErrorBody>(&bytes)
                .ok()
                .and_then(ImmichErrorBody::into_message)
                .unwrap_or_else(|| String::from_utf8_lossy(&bytes).into_owned());
            return Err(ImmichError::Api { status, message });
        }

        serde_json::from_slice(&bytes).map_err(ImmichError::Decode)
    }

    /// `GET /api/server/ping` — connectivity check.
    pub async fn ping(&self) -> Result<PingResponse, ImmichError> {
        let resp = self
            .http
            .get(self.url("/api/server/ping"))
            .header(API_KEY_HEADER, &self.api_key)
            .send()
            .await?;
        Self::decode(resp).await
    }

    /// `GET /api/server/about` — server version/info.
    pub async fn server_about(&self) -> Result<ServerAboutResponse, ImmichError> {
        let resp = self
            .http
            .get(self.url("/api/server/about"))
            .header(API_KEY_HEADER, &self.api_key)
            .send()
            .await?;
        Self::decode(resp).await
    }

    /// `POST /api/auth/validateToken` — validate the configured API key.
    pub async fn validate_token(&self) -> Result<ValidateTokenResponse, ImmichError> {
        let resp = self
            .http
            .post(self.url("/api/auth/validateToken"))
            .header(API_KEY_HEADER, &self.api_key)
            .send()
            .await?;
        Self::decode(resp).await
    }

    /// `GET /api/users/me` — account info including storage quota usage.
    pub async fn get_my_user(&self) -> Result<UserResponse, ImmichError> {
        let resp = self
            .http
            .get(self.url("/api/users/me"))
            .header(API_KEY_HEADER, &self.api_key)
            .send()
            .await?;
        Self::decode(resp).await
    }

    /// `POST /api/assets` — multipart upload of a single asset.
    pub async fn upload_asset(
        &self,
        request: UploadAssetRequest,
    ) -> Result<AssetUploadResponse, ImmichError> {
        let file_part = reqwest::multipart::Part::bytes(request.file_bytes)
            .file_name(request.file_name.clone());

        let form = reqwest::multipart::Form::new()
            .text("deviceAssetId", request.device_asset_id)
            .text("deviceId", request.device_id)
            .text("fileCreatedAt", request.file_created_at.to_rfc3339())
            .text("fileModifiedAt", request.file_modified_at.to_rfc3339())
            .text("isFavorite", request.is_favorite.to_string())
            .part("assetData", file_part);

        let resp = self
            .http
            .post(self.url("/api/assets"))
            .header(API_KEY_HEADER, &self.api_key)
            .multipart(form)
            .send()
            .await?;
        Self::decode(resp).await
    }

    /// `POST /api/assets/bulk-upload-check` — pre-check a batch of checksums
    /// so the uploader can skip server-side duplicates before doing a full
    /// upload.
    pub async fn bulk_upload_check(
        &self,
        items: Vec<BulkUploadCheckItem>,
    ) -> Result<Vec<BulkUploadCheckResult>, ImmichError> {
        let resp = self
            .http
            .post(self.url("/api/assets/bulk-upload-check"))
            .header(API_KEY_HEADER, &self.api_key)
            .json(&BulkUploadCheckRequest { assets: items })
            .send()
            .await?;
        let decoded: BulkUploadCheckResponse = Self::decode(resp).await?;
        Ok(decoded.results)
    }

    /// `GET /api/albums` — list albums.
    pub async fn list_albums(&self) -> Result<Vec<Album>, ImmichError> {
        let resp = self
            .http
            .get(self.url("/api/albums"))
            .header(API_KEY_HEADER, &self.api_key)
            .send()
            .await?;
        Self::decode(resp).await
    }

    /// `POST /api/albums` — create an album by name.
    pub async fn create_album(&self, album_name: impl Into<String>) -> Result<Album, ImmichError> {
        let resp = self
            .http
            .post(self.url("/api/albums"))
            .header(API_KEY_HEADER, &self.api_key)
            .json(&CreateAlbumRequest {
                album_name: album_name.into(),
                description: None,
            })
            .send()
            .await?;
        Self::decode(resp).await
    }

    /// `PUT /api/albums/{id}/assets` — add asset ids to an album.
    pub async fn add_assets_to_album(
        &self,
        album_id: &str,
        asset_ids: Vec<String>,
    ) -> Result<Vec<AlbumAddAssetsResult>, ImmichError> {
        let resp = self
            .http
            .put(self.url(&format!("/api/albums/{album_id}/assets")))
            .header(API_KEY_HEADER, &self.api_key)
            .json(&AddAssetsToAlbumRequest { ids: asset_ids })
            .send()
            .await?;
        Self::decode(resp).await
    }

    /// `PUT /api/assets/{id}` — update an asset (used to set the
    /// `description` field from parsed screenshot metadata).
    pub async fn update_asset_description(
        &self,
        asset_id: &str,
        description: impl Into<String>,
    ) -> Result<AssetResponse, ImmichError> {
        let resp = self
            .http
            .put(self.url(&format!("/api/assets/{asset_id}")))
            .header(API_KEY_HEADER, &self.api_key)
            .json(&UpdateAssetRequest {
                description: Some(description.into()),
                is_favorite: None,
            })
            .send()
            .await?;
        Self::decode(resp).await
    }
}

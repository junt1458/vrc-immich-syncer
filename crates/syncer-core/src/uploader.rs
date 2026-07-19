//! Async upload pipeline: consumes `pending`/due-retry rows from the
//! [`StateDb`] and drives them through an [`ImmichUploader`] (normally
//! [`immich_client::ImmichClient`]).
//!
//! Pipeline per file (see PLAN.md 3.2):
//! 1. Compute SHA-1 (blocking hash work is moved off the async runtime via
//!    `spawn_blocking`).
//! 2. `bulk_upload_check` — if the server already has this checksum, mark
//!    `duplicate` and skip the actual upload.
//! 3. Otherwise `upload_asset`; branch on `Created` vs `Duplicate`.
//! 4. If album auto-add is enabled, resolve (and cache) the album id and add
//!    the asset to it.
//! 5. If metadata→description is enabled, best-effort parse + set the asset
//!    description.
//! 6. On error in steps 1-3, mark `failed` with exponential backoff.
//!
//! Errors in steps 4/5 are logged and swallowed — the asset is already
//! safely uploaded at that point, so they must not trigger a retry.

use std::collections::HashMap;
use std::path::Path;

use async_trait::async_trait;
use chrono::{DateTime, Utc};
use immich_client::{
    AssetUploadResponse, AssetUploadStatus, BulkUploadCheckItem, BulkUploadCheckResult,
    ImmichClient, ImmichError, UploadAssetRequest,
};
use sha1::{Digest, Sha1};
use tokio::sync::mpsc::UnboundedSender;

use crate::config::Config;
use crate::events::{emit, SyncEvent};
use crate::state::{FileRecord, StateDb};
use crate::vrc_metadata::{format_description, parse_png_metadata};

/// The subset of [`immich_client::ImmichClient`] methods the uploader needs.
/// Exists so tests can inject a fake implementation instead of hitting a
/// real (or even mocked-over-HTTP) server.
#[async_trait]
pub trait ImmichUploader: Send + Sync {
    async fn bulk_upload_check(
        &self,
        items: Vec<BulkUploadCheckItem>,
    ) -> Result<Vec<BulkUploadCheckResult>, ImmichError>;

    async fn upload_asset(
        &self,
        request: UploadAssetRequest,
    ) -> Result<AssetUploadResponse, ImmichError>;

    async fn list_albums(&self) -> Result<Vec<immich_client::Album>, ImmichError>;

    async fn create_album(&self, album_name: &str) -> Result<immich_client::Album, ImmichError>;

    async fn add_assets_to_album(
        &self,
        album_id: &str,
        asset_ids: Vec<String>,
    ) -> Result<Vec<immich_client::AlbumAddAssetsResult>, ImmichError>;

    async fn update_asset_description(
        &self,
        asset_id: &str,
        description: &str,
    ) -> Result<(), ImmichError>;
}

#[async_trait]
impl ImmichUploader for ImmichClient {
    async fn bulk_upload_check(
        &self,
        items: Vec<BulkUploadCheckItem>,
    ) -> Result<Vec<BulkUploadCheckResult>, ImmichError> {
        ImmichClient::bulk_upload_check(self, items).await
    }

    async fn upload_asset(
        &self,
        request: UploadAssetRequest,
    ) -> Result<AssetUploadResponse, ImmichError> {
        ImmichClient::upload_asset(self, request).await
    }

    async fn list_albums(&self) -> Result<Vec<immich_client::Album>, ImmichError> {
        ImmichClient::list_albums(self).await
    }

    async fn create_album(&self, album_name: &str) -> Result<immich_client::Album, ImmichError> {
        ImmichClient::create_album(self, album_name).await
    }

    async fn add_assets_to_album(
        &self,
        album_id: &str,
        asset_ids: Vec<String>,
    ) -> Result<Vec<immich_client::AlbumAddAssetsResult>, ImmichError> {
        ImmichClient::add_assets_to_album(self, album_id, asset_ids).await
    }

    async fn update_asset_description(
        &self,
        asset_id: &str,
        description: &str,
    ) -> Result<(), ImmichError> {
        ImmichClient::update_asset_description(self, asset_id, description).await?;
        Ok(())
    }
}

/// Summary counters returned by [`Uploader::run_pending`].
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct UploadRunSummary {
    pub attempted: usize,
    pub succeeded: usize,
    pub failed: usize,
}

/// Multiplier cap applied to `config.retry_interval_secs` when computing
/// exponential backoff, so a persistently failing file doesn't end up with an
/// absurdly long retry delay. Chosen so the max backoff is a reasonable
/// "check back later" cadence (30x the base interval — e.g. 30 minutes for
/// the default 60s interval) without waiting for a full day.
const BACKOFF_CAP_MULTIPLIER: u64 = 30;

/// Computes the next retry time using exponential backoff based on the
/// record's *current* (pre-increment) retry count: `base * 2^retry_count`,
/// capped at `base * BACKOFF_CAP_MULTIPLIER`.
fn compute_next_retry_at(
    retry_interval_secs: u64,
    retry_count: i64,
    now: DateTime<Utc>,
) -> DateTime<Utc> {
    let base = retry_interval_secs.max(1);
    let exponent = retry_count.clamp(0, 32) as u32;
    let backoff = base.saturating_mul(1u64.checked_shl(exponent).unwrap_or(u64::MAX));
    let capped = backoff.min(base.saturating_mul(BACKOFF_CAP_MULTIPLIER));
    now + chrono::Duration::seconds(capped as i64)
}

/// Async upload pipeline, generic over the Immich client trait so it can be
/// tested against a fake.
pub struct Uploader<C> {
    client: C,
    config: Config,
    events: Option<UnboundedSender<SyncEvent>>,
    /// Album name -> resolved album id. Avoids calling `list_albums` /
    /// `create_album` on every single upload.
    album_id_cache: HashMap<String, String>,
    /// Stable per-device identifier reported to Immich as `deviceId`.
    device_id: String,
}

impl<C: ImmichUploader> Uploader<C> {
    /// Creates a new uploader. `device_id` should be stable across runs
    /// (e.g. derived from the machine) so re-uploads of the same file are
    /// recognized as coming from the same device; any stable string works
    /// for correctness (the state DB, not Immich, is the primary dedup
    /// source of truth on this client).
    pub fn new(client: C, config: Config, events: Option<UnboundedSender<SyncEvent>>) -> Self {
        Self {
            client,
            config,
            events,
            album_id_cache: HashMap::new(),
            device_id: "vrc-immich-syncer".to_string(),
        }
    }

    /// Drains all `pending` rows and all due `failed` retries, processing
    /// each one via [`process_one`](Self::process_one).
    pub async fn run_pending(&mut self, db: &StateDb) -> anyhow::Result<UploadRunSummary> {
        let mut summary = UploadRunSummary::default();

        let mut records = db.get_pending()?;
        records.extend(db.get_due_retries(Utc::now())?);

        for record in records {
            summary.attempted += 1;
            match self.process_one(db, &record).await {
                Ok(()) => summary.succeeded += 1,
                Err(e) => {
                    tracing::warn!("failed to process {:?}: {e}", record.path);
                    summary.failed += 1;
                }
            }
        }

        Ok(summary)
    }

    /// Processes a single file record through the full upload pipeline.
    /// Hashing/upload failures are recorded via `mark_failed` (with
    /// exponential backoff) and returned as `Ok(())` here (the failure is
    /// reflected in the DB and via a `SyncEvent`, not as an `Err` — `Err` is
    /// reserved for unexpected DB/IO errors that prevent bookkeeping itself).
    pub async fn process_one(&mut self, db: &StateDb, record: &FileRecord) -> anyhow::Result<()> {
        let path = record.path.clone();
        db.mark_uploading(&path)?;
        emit(
            self.events.as_ref(),
            SyncEvent::UploadStarted { path: path.clone() },
        );

        match self.upload_and_hash(&path).await {
            Ok((sha1, asset_id, is_duplicate)) => {
                if is_duplicate {
                    db.mark_duplicate(&path, &sha1, &asset_id)?;
                } else {
                    db.mark_uploaded(&path, &sha1, &asset_id, Utc::now())?;
                }

                emit(
                    self.events.as_ref(),
                    SyncEvent::UploadSucceeded {
                        path: path.clone(),
                        asset_id: asset_id.clone(),
                    },
                );

                // Steps 4/5: best-effort, must not fail the upload.
                if self.config.album_auto_add {
                    if let Err(e) = self.add_to_album(&asset_id).await {
                        tracing::warn!("album auto-add failed for {path:?}: {e}");
                    }
                }
                if self.config.metadata_to_description {
                    self.apply_metadata_description(&path, &asset_id).await;
                }

                Ok(())
            }
            Err(error) => {
                let error_string = error.to_string();
                let current_retry_count =
                    db.find_by_path(&path)?.map(|r| r.retry_count).unwrap_or(0);
                let next_retry_at = compute_next_retry_at(
                    self.config.retry_interval_secs,
                    current_retry_count,
                    Utc::now(),
                );
                db.mark_failed(&path, &error_string, next_retry_at)?;

                emit(
                    self.events.as_ref(),
                    SyncEvent::UploadFailed {
                        path: path.clone(),
                        error: error_string,
                    },
                );

                Ok(())
            }
        }
    }

    /// Runs steps 1-3: hash, bulk-check, upload. Returns `(sha1, asset_id,
    /// is_duplicate)` on success.
    async fn upload_and_hash(&self, path: &Path) -> anyhow::Result<(String, String, bool)> {
        let sha1 = hash_file(path).await?;

        let device_asset_id = path
            .to_str()
            .map(str::to_owned)
            .unwrap_or_else(|| path.to_string_lossy().into_owned());

        let bulk_results = self
            .client
            .bulk_upload_check(vec![BulkUploadCheckItem {
                id: device_asset_id.clone(),
                checksum: sha1.clone(),
            }])
            .await?;

        if let Some(result) = bulk_results.into_iter().find(|r| r.id == device_asset_id) {
            if result.is_duplicate() {
                let asset_id = result.asset_id.unwrap_or_default();
                return Ok((sha1, asset_id, true));
            }
        }

        let file_bytes = tokio::fs::read(path).await?;
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("upload")
            .to_string();
        let metadata = tokio::fs::metadata(path).await?;
        let modified = metadata
            .modified()
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(|_| Utc::now());
        let created = metadata
            .created()
            .map(DateTime::<Utc>::from)
            .unwrap_or(modified);

        let request = UploadAssetRequest::new(
            file_bytes,
            file_name,
            device_asset_id,
            self.device_id.clone(),
            created,
            modified,
        );

        let response = self.client.upload_asset(request).await?;
        let is_duplicate = matches!(response.status, AssetUploadStatus::Duplicate);
        Ok((sha1, response.id, is_duplicate))
    }

    /// Resolves (creating and caching if necessary) the configured album and
    /// adds `asset_id` to it. A no-op if `album_name` is unset.
    async fn add_to_album(&mut self, asset_id: &str) -> anyhow::Result<()> {
        let Some(album_name) = self.config.album_name.clone() else {
            return Ok(());
        };
        if album_name.is_empty() {
            return Ok(());
        }

        let album_id = self.resolve_album_id(&album_name).await?;
        self.client
            .add_assets_to_album(&album_id, vec![asset_id.to_string()])
            .await?;
        Ok(())
    }

    async fn resolve_album_id(&mut self, album_name: &str) -> anyhow::Result<String> {
        if let Some(id) = self.album_id_cache.get(album_name) {
            return Ok(id.clone());
        }

        let albums = self.client.list_albums().await?;
        let found = albums.into_iter().find(|a| a.album_name == album_name);

        let album_id = match found {
            Some(album) => album.id,
            None => self.client.create_album(album_name).await?.id,
        };

        self.album_id_cache
            .insert(album_name.to_string(), album_id.clone());
        Ok(album_id)
    }

    /// Best-effort: parse VRChat/VRCX metadata and set the asset description.
    /// Any failure (parse or API) is logged and swallowed.
    async fn apply_metadata_description(&self, path: &Path, asset_id: &str) {
        let path_owned = path.to_path_buf();
        let meta = tokio::task::spawn_blocking(move || parse_png_metadata(&path_owned)).await;

        let meta = match meta {
            Ok(Some(meta)) => meta,
            Ok(None) => return,
            Err(e) => {
                tracing::warn!("metadata parse task panicked for {path:?}: {e}");
                return;
            }
        };

        let description = format_description(&meta);
        if description.is_empty() {
            return;
        }

        if let Err(e) = self
            .client
            .update_asset_description(asset_id, &description)
            .await
        {
            tracing::warn!("failed to set description for asset {asset_id}: {e}");
        }
    }
}

/// Computes the SHA-1 of a file's contents, off the async runtime.
async fn hash_file(path: &Path) -> anyhow::Result<String> {
    let path = path.to_path_buf();
    tokio::task::spawn_blocking(move || -> anyhow::Result<String> {
        let mut file = std::fs::File::open(&path)?;
        let mut hasher = Sha1::new();
        std::io::copy(&mut file, &mut hasher)?;
        Ok(hex::encode(hasher.finalize()))
    })
    .await?
}

/// Minimal hex encoding helper (avoids pulling in the `hex` crate for a
/// single call site).
mod hex {
    pub fn encode(bytes: impl AsRef<[u8]>) -> String {
        use std::fmt::Write;
        bytes.as_ref().iter().fold(String::new(), |mut acc, b| {
            let _ = write!(acc, "{b:02x}");
            acc
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use immich_client::{Album, AlbumAddAssetsResult, BulkUploadAction};
    use std::path::PathBuf;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Mutex;

    #[derive(Default)]
    struct FakeClient {
        /// checksum -> duplicate asset id, if it should be reported as a
        /// server-side duplicate via bulk_upload_check.
        bulk_duplicates: HashMap<String, String>,
        /// If true, `upload_asset` itself reports `Duplicate` status.
        upload_reports_duplicate: bool,
        /// If true, `upload_asset` returns an error (simulating a failed
        /// upload).
        upload_fails: bool,
        upload_asset_id: String,
        list_albums_calls: AtomicUsize,
        create_album_calls: AtomicUsize,
        albums: Mutex<Vec<Album>>,
        add_to_album_calls: AtomicUsize,
        description_calls: Mutex<Vec<(String, String)>>,
        description_fails: bool,
    }

    #[async_trait]
    impl ImmichUploader for FakeClient {
        async fn bulk_upload_check(
            &self,
            items: Vec<BulkUploadCheckItem>,
        ) -> Result<Vec<BulkUploadCheckResult>, ImmichError> {
            Ok(items
                .into_iter()
                .map(|item| {
                    if let Some(asset_id) = self.bulk_duplicates.get(&item.checksum) {
                        BulkUploadCheckResult {
                            id: item.id,
                            action: BulkUploadAction::Reject,
                            reason: Some("duplicate".to_string()),
                            asset_id: Some(asset_id.clone()),
                        }
                    } else {
                        BulkUploadCheckResult {
                            id: item.id,
                            action: BulkUploadAction::Accept,
                            reason: None,
                            asset_id: None,
                        }
                    }
                })
                .collect())
        }

        async fn upload_asset(
            &self,
            _request: UploadAssetRequest,
        ) -> Result<AssetUploadResponse, ImmichError> {
            if self.upload_fails {
                return Err(ImmichError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    message: "simulated failure".to_string(),
                });
            }
            Ok(AssetUploadResponse {
                id: self.upload_asset_id.clone(),
                status: if self.upload_reports_duplicate {
                    AssetUploadStatus::Duplicate
                } else {
                    AssetUploadStatus::Created
                },
            })
        }

        async fn list_albums(&self) -> Result<Vec<Album>, ImmichError> {
            self.list_albums_calls.fetch_add(1, Ordering::SeqCst);
            Ok(self.albums.lock().unwrap().clone())
        }

        async fn create_album(&self, album_name: &str) -> Result<Album, ImmichError> {
            self.create_album_calls.fetch_add(1, Ordering::SeqCst);
            let album = Album {
                id: format!("album-{album_name}"),
                album_name: album_name.to_string(),
                description: String::new(),
                asset_count: 0,
            };
            self.albums.lock().unwrap().push(album.clone());
            Ok(album)
        }

        async fn add_assets_to_album(
            &self,
            _album_id: &str,
            asset_ids: Vec<String>,
        ) -> Result<Vec<AlbumAddAssetsResult>, ImmichError> {
            self.add_to_album_calls.fetch_add(1, Ordering::SeqCst);
            Ok(asset_ids
                .into_iter()
                .map(|id| AlbumAddAssetsResult {
                    id,
                    success: true,
                    error: None,
                })
                .collect())
        }

        async fn update_asset_description(
            &self,
            asset_id: &str,
            description: &str,
        ) -> Result<(), ImmichError> {
            if self.description_fails {
                return Err(ImmichError::Api {
                    status: reqwest::StatusCode::INTERNAL_SERVER_ERROR,
                    message: "description failure".to_string(),
                });
            }
            self.description_calls
                .lock()
                .unwrap()
                .push((asset_id.to_string(), description.to_string()));
            Ok(())
        }
    }

    fn test_config() -> Config {
        Config {
            metadata_to_description: false,
            album_auto_add: false,
            retry_interval_secs: 10,
            ..Config::default()
        }
    }

    fn write_file(dir: &tempfile::TempDir, name: &str, contents: &[u8]) -> PathBuf {
        let path = dir.path().join(name);
        std::fs::write(&path, contents).unwrap();
        path
    }

    #[tokio::test]
    async fn successful_upload_marks_uploaded() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(&dir, "a.png", b"hello world");
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_pending(&path, 11, 1).unwrap();

        let client = FakeClient {
            upload_asset_id: "asset-1".to_string(),
            ..Default::default()
        };
        let mut uploader = Uploader::new(client, test_config(), None);

        let record = db.find_by_path(&path).unwrap().unwrap();
        uploader.process_one(&db, &record).await.unwrap();

        let record = db.find_by_path(&path).unwrap().unwrap();
        assert_eq!(record.status, crate::state::FileStatus::Uploaded);
        assert_eq!(record.asset_id.as_deref(), Some("asset-1"));
        assert!(record.sha1.is_some());
    }

    #[tokio::test]
    async fn bulk_check_duplicate_marks_duplicate_without_uploading() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(&dir, "b.png", b"duplicate content");
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_pending(&path, 1, 1).unwrap();

        // sha1("duplicate content")
        let sha1_hex = {
            let mut hasher = Sha1::new();
            hasher.update(b"duplicate content");
            hex::encode(hasher.finalize())
        };

        let mut bulk_duplicates = HashMap::new();
        bulk_duplicates.insert(sha1_hex, "existing-asset".to_string());

        let client = FakeClient {
            bulk_duplicates,
            upload_fails: true, // would fail the test if upload_asset were called
            ..Default::default()
        };
        let mut uploader = Uploader::new(client, test_config(), None);

        let record = db.find_by_path(&path).unwrap().unwrap();
        uploader.process_one(&db, &record).await.unwrap();

        let record = db.find_by_path(&path).unwrap().unwrap();
        assert_eq!(record.status, crate::state::FileStatus::Duplicate);
        assert_eq!(record.asset_id.as_deref(), Some("existing-asset"));
    }

    #[tokio::test]
    async fn upload_response_duplicate_marks_duplicate() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(&dir, "c.png", b"content c");
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_pending(&path, 1, 1).unwrap();

        let client = FakeClient {
            upload_reports_duplicate: true,
            upload_asset_id: "dup-asset".to_string(),
            ..Default::default()
        };
        let mut uploader = Uploader::new(client, test_config(), None);

        let record = db.find_by_path(&path).unwrap().unwrap();
        uploader.process_one(&db, &record).await.unwrap();

        let record = db.find_by_path(&path).unwrap().unwrap();
        assert_eq!(record.status, crate::state::FileStatus::Duplicate);
        assert_eq!(record.asset_id.as_deref(), Some("dup-asset"));
    }

    #[tokio::test]
    async fn failing_upload_marks_failed_with_growing_backoff() {
        let dir = tempfile::tempdir().unwrap();
        let path = write_file(&dir, "d.png", b"content d");
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_pending(&path, 1, 1).unwrap();

        let client = FakeClient {
            upload_fails: true,
            ..Default::default()
        };
        let mut uploader = Uploader::new(client, test_config(), None);

        let record = db.find_by_path(&path).unwrap().unwrap();
        uploader.process_one(&db, &record).await.unwrap();
        let record1 = db.find_by_path(&path).unwrap().unwrap();
        assert_eq!(record1.status, crate::state::FileStatus::Failed);
        assert_eq!(record1.retry_count, 1);
        assert!(record1.last_error.is_some());

        uploader.process_one(&db, &record1).await.unwrap();
        let record2 = db.find_by_path(&path).unwrap().unwrap();
        assert_eq!(record2.retry_count, 2);

        // Backoff must grow (or at least not shrink) across repeated
        // failures.
        assert!(record2.next_retry_at.unwrap() >= record1.next_retry_at.unwrap());
    }

    #[tokio::test]
    async fn album_auto_add_resolves_and_caches_id_across_uploads() {
        let dir = tempfile::tempdir().unwrap();
        let path1 = write_file(&dir, "e1.png", b"content e1");
        let path2 = write_file(&dir, "e2.png", b"content e2");
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_pending(&path1, 1, 1).unwrap();
        db.upsert_pending(&path2, 1, 1).unwrap();

        let client = FakeClient {
            upload_asset_id: "asset-x".to_string(),
            ..Default::default()
        };
        let mut config = test_config();
        config.album_auto_add = true;
        config.album_name = Some("VRChat".to_string());
        let mut uploader = Uploader::new(client, config, None);

        let record1 = db.find_by_path(&path1).unwrap().unwrap();
        uploader.process_one(&db, &record1).await.unwrap();
        let record2 = db.find_by_path(&path2).unwrap().unwrap();
        uploader.process_one(&db, &record2).await.unwrap();

        assert_eq!(
            uploader.client.list_albums_calls.load(Ordering::SeqCst),
            1,
            "list_albums should only be called once due to caching"
        );
        assert_eq!(
            uploader.client.create_album_calls.load(Ordering::SeqCst),
            1,
            "create_album should only be called once due to caching"
        );
        assert_eq!(uploader.client.add_to_album_calls.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn metadata_description_failure_does_not_fail_upload() {
        let dir = tempfile::tempdir().unwrap();
        // Not a real PNG, so parse_png_metadata will return None.
        let path = write_file(&dir, "f.png", b"not a real png");
        let db = StateDb::open_in_memory().unwrap();
        db.upsert_pending(&path, 1, 1).unwrap();

        let client = FakeClient {
            upload_asset_id: "asset-f".to_string(),
            description_fails: true,
            ..Default::default()
        };
        let mut config = test_config();
        config.metadata_to_description = true;
        let mut uploader = Uploader::new(client, config, None);

        let record = db.find_by_path(&path).unwrap().unwrap();
        uploader.process_one(&db, &record).await.unwrap();

        // Upload itself must still be recorded as successful.
        let record = db.find_by_path(&path).unwrap().unwrap();
        assert_eq!(record.status, crate::state::FileStatus::Uploaded);
        // parse_png_metadata returned None for non-PNG bytes, so no
        // description call should have even been attempted.
        assert!(uploader.client.description_calls.lock().unwrap().is_empty());
    }

    #[test]
    fn compute_next_retry_at_grows_exponentially_and_caps() {
        let now = Utc::now();
        let base = 10u64;

        let r0 = compute_next_retry_at(base, 0, now);
        let r1 = compute_next_retry_at(base, 1, now);
        let r2 = compute_next_retry_at(base, 2, now);

        assert_eq!((r0 - now).num_seconds(), 10);
        assert_eq!((r1 - now).num_seconds(), 20);
        assert_eq!((r2 - now).num_seconds(), 40);

        // Cap at BACKOFF_CAP_MULTIPLIER * base regardless of how large
        // retry_count gets.
        let capped = compute_next_retry_at(base, 20, now);
        assert_eq!(
            (capped - now).num_seconds(),
            (base * BACKOFF_CAP_MULTIPLIER) as i64
        );
    }
}

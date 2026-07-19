//! Unit tests for [`crate::ImmichClient`] against a mocked Immich server.

use chrono::Utc;
use serde_json::json;
use wiremock::matchers::{body_json, header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use crate::{
    AssetUploadStatus, BulkUploadAction, BulkUploadCheckItem, ImmichClient, ImmichError,
    UploadAssetRequest,
};

const API_KEY: &str = "test-api-key";

async fn mock_server_and_client() -> (MockServer, ImmichClient) {
    let server = MockServer::start().await;
    let client = ImmichClient::new(server.uri(), API_KEY);
    (server, client)
}

#[tokio::test]
async fn ping_sends_api_key_header_and_decodes_response() {
    let (server, client) = mock_server_and_client().await;

    Mock::given(method("GET"))
        .and(path("/api/server/ping"))
        .and(header("x-api-key", API_KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "res": "pong" })))
        .expect(1)
        .mount(&server)
        .await;

    let resp = client.ping().await.expect("ping should succeed");
    assert_eq!(resp.res, "pong");
}

#[tokio::test]
async fn server_about_decodes_response() {
    let (server, client) = mock_server_and_client().await;

    Mock::given(method("GET"))
        .and(path("/api/server/about"))
        .and(header("x-api-key", API_KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "version": "v1.118.0",
            "versionUrl": "https://github.com/immich-app/immich/releases/tag/v1.118.0",
        })))
        .mount(&server)
        .await;

    let resp = client
        .server_about()
        .await
        .expect("server_about should succeed");
    assert_eq!(resp.version, "v1.118.0");
    assert_eq!(
        resp.version_url.as_deref(),
        Some("https://github.com/immich-app/immich/releases/tag/v1.118.0")
    );
}

#[tokio::test]
async fn validate_token_posts_and_decodes_auth_status() {
    let (server, client) = mock_server_and_client().await;

    Mock::given(method("POST"))
        .and(path("/api/auth/validateToken"))
        .and(header("x-api-key", API_KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({ "authStatus": true })))
        .mount(&server)
        .await;

    let resp = client
        .validate_token()
        .await
        .expect("validate_token should succeed");
    assert!(resp.auth_status);
}

#[tokio::test]
async fn validate_token_unauthorized_maps_to_unauthorized_error() {
    let (server, client) = mock_server_and_client().await;

    Mock::given(method("POST"))
        .and(path("/api/auth/validateToken"))
        .respond_with(ResponseTemplate::new(401).set_body_json(json!({
            "message": "Invalid API key",
            "error": "Unauthorized",
            "statusCode": 401
        })))
        .mount(&server)
        .await;

    let err = client
        .validate_token()
        .await
        .expect_err("expected an error");
    assert!(matches!(err, ImmichError::Unauthorized));
}

#[tokio::test]
async fn get_my_user_decodes_quota_fields() {
    let (server, client) = mock_server_and_client().await;

    Mock::given(method("GET"))
        .and(path("/api/users/me"))
        .and(header("x-api-key", API_KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "user-1",
            "email": "user@example.com",
            "name": "Test User",
            "quotaSizeInBytes": 1_000_000_000i64,
            "quotaUsageInBytes": 12_345i64,
            "isAdmin": false,
        })))
        .mount(&server)
        .await;

    let user = client
        .get_my_user()
        .await
        .expect("get_my_user should succeed");
    assert_eq!(user.id, "user-1");
    assert_eq!(user.email, "user@example.com");
    assert_eq!(user.quota_size_in_bytes, Some(1_000_000_000));
    assert_eq!(user.quota_usage_in_bytes, Some(12_345));
    assert!(!user.is_admin);
}

#[tokio::test]
async fn upload_asset_sends_multipart_fields_and_reports_created_status() {
    let (server, client) = mock_server_and_client().await;

    Mock::given(method("POST"))
        .and(path("/api/assets"))
        .and(header("x-api-key", API_KEY))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "asset-1",
            "status": "created",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let now = Utc::now();
    let req = UploadAssetRequest::new(
        b"fake-png-bytes".to_vec(),
        "VRChat_2026-07-19_00-00-00.png",
        "/path/to/VRChat_2026-07-19_00-00-00.png",
        "device-123",
        now,
        now,
    );

    let resp = client
        .upload_asset(req)
        .await
        .expect("upload_asset should succeed");
    assert_eq!(resp.id, "asset-1");
    assert_eq!(resp.status, AssetUploadStatus::Created);
}

#[tokio::test]
async fn upload_asset_reports_duplicate_status() {
    let (server, client) = mock_server_and_client().await;

    Mock::given(method("POST"))
        .and(path("/api/assets"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "existing-asset-1",
            "status": "duplicate",
        })))
        .mount(&server)
        .await;

    let now = Utc::now();
    let req = UploadAssetRequest::new(
        b"fake-png-bytes".to_vec(),
        "dup.png",
        "dup-device-asset-id",
        "device-123",
        now,
        now,
    );

    let resp = client
        .upload_asset(req)
        .await
        .expect("upload_asset should succeed");
    assert_eq!(resp.id, "existing-asset-1");
    assert_eq!(resp.status, AssetUploadStatus::Duplicate);
}

#[tokio::test]
async fn upload_asset_non_2xx_maps_to_api_error_with_parsed_message() {
    let (server, client) = mock_server_and_client().await;

    Mock::given(method("POST"))
        .and(path("/api/assets"))
        .respond_with(ResponseTemplate::new(500).set_body_json(json!({
            "message": "Internal server error",
            "statusCode": 500
        })))
        .mount(&server)
        .await;

    let now = Utc::now();
    let req = UploadAssetRequest::new(b"bytes".to_vec(), "f.png", "id", "dev", now, now);

    let err = client.upload_asset(req).await.expect_err("expected error");
    match err {
        ImmichError::Api { status, message } => {
            assert_eq!(status.as_u16(), 500);
            assert_eq!(message, "Internal server error");
        }
        other => panic!("expected ImmichError::Api, got {other:?}"),
    }
}

#[tokio::test]
async fn bulk_upload_check_sends_checksums_and_reports_duplicates() {
    let (server, client) = mock_server_and_client().await;

    let expected_body = json!({
        "assets": [
            { "id": "file-a", "checksum": "aaaa" },
            { "id": "file-b", "checksum": "bbbb" },
        ]
    });

    Mock::given(method("POST"))
        .and(path("/api/assets/bulk-upload-check"))
        .and(header("x-api-key", API_KEY))
        .and(body_json(&expected_body))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "results": [
                { "id": "file-a", "action": "reject", "reason": "duplicate", "assetId": "server-asset-a" },
                { "id": "file-b", "action": "accept" },
            ]
        })))
        .expect(1)
        .mount(&server)
        .await;

    let items = vec![
        BulkUploadCheckItem {
            id: "file-a".into(),
            checksum: "aaaa".into(),
        },
        BulkUploadCheckItem {
            id: "file-b".into(),
            checksum: "bbbb".into(),
        },
    ];

    let results = client
        .bulk_upload_check(items)
        .await
        .expect("bulk_upload_check should succeed");

    assert_eq!(results.len(), 2);

    let a = results.iter().find(|r| r.id == "file-a").unwrap();
    assert_eq!(a.action, BulkUploadAction::Reject);
    assert!(a.is_duplicate());
    assert_eq!(a.asset_id.as_deref(), Some("server-asset-a"));

    let b = results.iter().find(|r| r.id == "file-b").unwrap();
    assert_eq!(b.action, BulkUploadAction::Accept);
    assert!(!b.is_duplicate());
}

#[tokio::test]
async fn list_albums_decodes_array() {
    let (server, client) = mock_server_and_client().await;

    Mock::given(method("GET"))
        .and(path("/api/albums"))
        .and(header("x-api-key", API_KEY))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "album-1", "albumName": "VRChat", "description": "", "assetCount": 42 }
        ])))
        .mount(&server)
        .await;

    let albums = client.list_albums().await.expect("list_albums succeed");
    assert_eq!(albums.len(), 1);
    assert_eq!(albums[0].id, "album-1");
    assert_eq!(albums[0].album_name, "VRChat");
    assert_eq!(albums[0].asset_count, 42);
}

#[tokio::test]
async fn create_album_sends_name_and_decodes_new_album() {
    let (server, client) = mock_server_and_client().await;

    Mock::given(method("POST"))
        .and(path("/api/albums"))
        .and(header("x-api-key", API_KEY))
        .and(body_json(json!({ "albumName": "VRChat" })))
        .respond_with(ResponseTemplate::new(201).set_body_json(json!({
            "id": "new-album-1",
            "albumName": "VRChat",
            "description": "",
            "assetCount": 0
        })))
        .expect(1)
        .mount(&server)
        .await;

    let album = client
        .create_album("VRChat")
        .await
        .expect("create_album should succeed");
    assert_eq!(album.id, "new-album-1");
    assert_eq!(album.album_name, "VRChat");
}

#[tokio::test]
async fn add_assets_to_album_sends_ids_and_decodes_per_item_results() {
    let (server, client) = mock_server_and_client().await;

    Mock::given(method("PUT"))
        .and(path("/api/albums/album-1/assets"))
        .and(header("x-api-key", API_KEY))
        .and(body_json(json!({ "ids": ["asset-1", "asset-2"] })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!([
            { "id": "asset-1", "success": true },
            { "id": "asset-2", "success": false, "error": "duplicate" },
        ])))
        .expect(1)
        .mount(&server)
        .await;

    let results = client
        .add_assets_to_album("album-1", vec!["asset-1".into(), "asset-2".into()])
        .await
        .expect("add_assets_to_album should succeed");

    assert_eq!(results.len(), 2);
    assert!(results[0].success);
    assert!(!results[1].success);
    assert_eq!(results[1].error.as_deref(), Some("duplicate"));
}

#[tokio::test]
async fn update_asset_description_sends_put_with_description() {
    let (server, client) = mock_server_and_client().await;

    Mock::given(method("PUT"))
        .and(path("/api/assets/asset-1"))
        .and(header("x-api-key", API_KEY))
        .and(body_json(json!({ "description": "World: Test World" })))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "id": "asset-1",
            "description": "World: Test World",
        })))
        .expect(1)
        .mount(&server)
        .await;

    let resp = client
        .update_asset_description("asset-1", "World: Test World")
        .await
        .expect("update_asset_description should succeed");
    assert_eq!(resp.id, "asset-1");
    assert_eq!(resp.description.as_deref(), Some("World: Test World"));
}

#[tokio::test]
async fn decode_error_on_malformed_json_body() {
    let (server, client) = mock_server_and_client().await;

    Mock::given(method("GET"))
        .and(path("/api/server/ping"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let err = client.ping().await.expect_err("expected decode error");
    assert!(matches!(err, ImmichError::Decode(_)));
}

#[tokio::test]
async fn non_2xx_without_parsable_body_falls_back_to_raw_text() {
    let (server, client) = mock_server_and_client().await;

    Mock::given(method("GET"))
        .and(path("/api/server/ping"))
        .respond_with(ResponseTemplate::new(503).set_body_string("Service Unavailable"))
        .mount(&server)
        .await;

    let err = client.ping().await.expect_err("expected api error");
    match err {
        ImmichError::Api { status, message } => {
            assert_eq!(status.as_u16(), 503);
            assert_eq!(message, "Service Unavailable");
        }
        other => panic!("expected ImmichError::Api, got {other:?}"),
    }
}

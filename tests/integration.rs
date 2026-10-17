//! Memory Platform — integration tests.
//!
//! Tests API endpoints via the Axum router directly.
//! Tests that don't need a database use a minimal AppState.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use memory_platform::db::postgres::PostgresDb;
use memory_platform::queue::PendingWriteQueue;
use memory_platform::search::SearchEngine;
use memory_platform::{api, config::Config, AppState};
use tower::ServiceExt;

/// `minimal_state()`'s db is `PostgresDb::new_empty()` — lazily connected to
/// a nonexistent database, so `db.health()` is false here by construction.
/// Since 1c56c04, `/health` reports that honestly as 503 rather than a
/// silent 200; this asserts the current, correct contract.
#[tokio::test]
async fn health_endpoint_returns_503_when_db_unavailable() {
    let state = minimal_state();
    let app = api::router().with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/health")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::SERVICE_UNAVAILABLE);
}

/// Test that the root endpoint returns 200 OK.
#[tokio::test]
async fn root_endpoint_returns_200() {
    let state = minimal_state();
    let app = api::router().with_state(state);

    let response = app
        .oneshot(Request::builder().uri("/").body(Body::empty()).unwrap())
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::OK);
}

/// Test that an unknown route returns 404.
#[tokio::test]
async fn unknown_route_returns_404() {
    let state = minimal_state();
    let app = api::router().with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .uri("/nonexistent")
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::NOT_FOUND);
}

/// Test that protected endpoints reject missing X-API-Key when auth is configured.
#[tokio::test]
async fn auth_rejects_missing_api_key() {
    let mut config = Config::default();
    config.api_key = "test-secret-key".to_string();

    let state = Arc::new(AppState {
        config,
        db: Arc::new(PostgresDb::new_empty()),
        search: Arc::new(SearchEngine::new_empty()),
        neo4j_client: None,
        redis_cache: None,
        contradiction_detector: None,
        embedding_service: None,
        experience_service: None,
        ingestion_service: None,
        procedure_service: None,
        pending_writes: Arc::new(PendingWriteQueue::new_empty()),
    });

    let app = api::router().with_state(state);

    let response = app
        .oneshot(
            Request::builder()
                .method("POST")
                .uri("/memories")
                .header("content-type", "application/json")
                .body(Body::from(
                    serde_json::json!({"content": "test", "content_type": "note"}).to_string(),
                ))
                .unwrap(),
        )
        .await
        .unwrap();

    assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// /mcp (transport-http): auth, Content-Type and Origin checks. DB-free: `ping`
// never touches storage.
// ---------------------------------------------------------------------------

#[cfg(feature = "transport-http")]
async fn mcp_post(api_key: &str, headers: &[(&str, &str)]) -> (StatusCode, String) {
    let mut config = Config::default();
    config.api_key = api_key.to_string();
    let state = state_with_config(config);
    let mut req = Request::builder().method("POST").uri("/mcp");
    for (k, v) in headers {
        req = req.header(*k, *v);
    }
    let body = serde_json::json!({"jsonrpc": "2.0", "id": 1, "method": "ping"}).to_string();
    let response = api::router()
        .with_state(state)
        .oneshot(req.body(Body::from(body)).unwrap())
        .await
        .unwrap();
    let status = response.status();
    let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).into_owned())
}

#[cfg(feature = "transport-http")]
const JSON: (&str, &str) = ("content-type", "application/json");

#[cfg(feature = "transport-http")]
#[tokio::test]
async fn mcp_accepts_valid_request() {
    // No key configured (loopback dev mode), no Origin (non-browser client).
    let (status, body) = mcp_post("", &[JSON]).await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body.contains("\"result\""), "{body}");

    // Key configured and supplied, loopback browser Origin, charset param.
    let (status, body) = mcp_post(
        "k",
        &[
            ("content-type", "application/json; charset=utf-8"),
            ("x-api-key", "k"),
            ("origin", "http://localhost:8000"),
        ],
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    for origin in [
        "http://127.0.0.1",
        "http://127.0.0.1:8000",
        "http://[::1]:8000",
    ] {
        let (status, _) = mcp_post("", &[JSON, ("origin", origin)]).await;
        assert_eq!(status, StatusCode::OK, "{origin}");
    }
}

#[cfg(feature = "transport-http")]
#[tokio::test]
async fn mcp_rejects_non_json_content_type() {
    for ct in [
        "text/plain",
        "application/x-www-form-urlencoded",
        "multipart/form-data",
    ] {
        let (status, _) = mcp_post("", &[("content-type", ct)]).await;
        assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "{ct}");
    }
    let (status, _) = mcp_post("", &[]).await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE, "missing");
}

#[cfg(feature = "transport-http")]
#[tokio::test]
async fn mcp_rejects_foreign_origin() {
    for origin in [
        "http://evil.example",
        "http://127.0.0.1.evil.example:8000",
        "https://localhost",
        "null",
    ] {
        let (status, _) = mcp_post("", &[JSON, ("origin", origin)]).await;
        assert_eq!(status, StatusCode::FORBIDDEN, "{origin}");
    }
}

#[cfg(feature = "transport-http")]
#[tokio::test]
async fn mcp_rejects_missing_or_wrong_key_when_configured() {
    let (status, _) = mcp_post("k", &[JSON]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, _) = mcp_post("k", &[JSON, ("x-api-key", "wrong")]).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
}

// ---------------------------------------------------------------------------
// Tests requiring a live database — ignored by default.
// Set DATABASE_URL (or the integration test env) to run them:
//   DATABASE_URL=postgresql://... cargo test --test integration -- --include-ignored
// ---------------------------------------------------------------------------

fn has_live_db() -> bool {
    std::env::var("DATABASE_URL").is_ok()
}

fn live_db_config() -> memory_platform::config::Config {
    let mut config = memory_platform::config::Config::default();
    if let Ok(url) = std::env::var("DATABASE_URL") {
        config.database_url = url;
    }
    config
}

/// Test that database health check passes with a live DB.
#[tokio::test]
#[ignore]
async fn db_health_check() {
    if !has_live_db() {
        return;
    }
    let config = live_db_config();
    let db = memory_platform::db::postgres::PostgresDb::connect(&config)
        .await
        .expect("Failed to connect to database");

    let healthy = db.health().await;
    assert!(healthy, "Health check should succeed");
}

/// Test that migration runs without error on a live DB.
#[tokio::test]
#[ignore]
async fn db_migration_runs_cleanly() {
    if !has_live_db() {
        return;
    }
    let config = live_db_config();
    let db = memory_platform::db::postgres::PostgresDb::connect(&config)
        .await
        .expect("Failed to connect to database");

    memory_platform::migrations::Migrator::run(&db.pool)
        .await
        .expect("Migration should succeed");
}

/// Build a minimal AppState with all services disabled.
fn minimal_state() -> Arc<AppState> {
    state_with_config(Config::default())
}

fn state_with_config(config: Config) -> Arc<AppState> {
    Arc::new(AppState {
        config,
        db: Arc::new(PostgresDb::new_empty()),
        search: Arc::new(SearchEngine::new_empty()),
        neo4j_client: None,
        redis_cache: None,
        contradiction_detector: None,
        embedding_service: None,
        experience_service: None,
        ingestion_service: None,
        procedure_service: None,
        pending_writes: Arc::new(PendingWriteQueue::new_empty()),
    })
}

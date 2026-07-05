//! Memory Platform — integration tests.
//!
//! Tests API endpoints via the Axum router directly.
//! Tests that don't need a database use a minimal AppState.

use std::sync::Arc;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use memory_platform::db::postgres::PostgresDb;
use memory_platform::search::SearchEngine;
use memory_platform::{api, config::Config, AppState};
use tower::ServiceExt;

/// Test that the health endpoint returns 200 OK.
#[tokio::test]
async fn health_endpoint_returns_200() {
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

    assert_eq!(response.status(), StatusCode::OK);
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
        context_service: None,
        contradiction_detector: None,
        decay_engine: None,
        embedding_service: None,
        experience_service: None,
        ingestion_service: None,
        procedure_service: None,
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
    Arc::new(AppState {
        config: Config::default(),
        db: Arc::new(PostgresDb::new_empty()),
        search: Arc::new(SearchEngine::new_empty()),
        neo4j_client: None,
        redis_cache: None,
        context_service: None,
        contradiction_detector: None,
        decay_engine: None,
        embedding_service: None,
        experience_service: None,
        ingestion_service: None,
        procedure_service: None,
    })
}

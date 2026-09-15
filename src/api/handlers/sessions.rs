use axum::{extract::State, Json};
use std::sync::Arc;

use crate::api::auth::Auth;
use crate::api::dto::{SessionCreateRequest, SessionCreateResponse};
use crate::AppState;

pub async fn create_session(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<SessionCreateRequest>,
) -> Json<SessionCreateResponse> {
    todo!("create_session")
}

pub async fn get_session(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    axum::extract::Path(_session_id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    todo!("get_session")
}

pub async fn summarize_session(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    axum::extract::Path(_session_id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    todo!("summarize_session")
}

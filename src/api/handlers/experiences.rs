use axum::{Json, extract::State};
use std::sync::Arc;

use crate::AppState;
use crate::api::auth::Auth;
use crate::api::dto::{ConfidenceUpdateRequest, FindRelevantParams, ListExperiencesParams};

pub async fn list_experiences(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    axum::extract::Query(_params): axum::extract::Query<ListExperiencesParams>,
) -> Json<serde_json::Value> {
    todo!("list_experiences")
}

pub async fn find_relevant_experiences(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    axum::extract::Query(_params): axum::extract::Query<FindRelevantParams>,
) -> Json<serde_json::Value> {
    todo!("find_relevant_experiences")
}

pub async fn update_experience_confidence(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    axum::extract::Path(_experience_id): axum::extract::Path<String>,
    Json(_body): Json<ConfidenceUpdateRequest>,
) -> Json<serde_json::Value> {
    todo!("update_experience_confidence")
}

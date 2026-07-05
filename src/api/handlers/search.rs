use axum::{extract::State, Json};
use std::sync::Arc;

use crate::api::auth::Auth;
use crate::api::dto::{SearchParams, SearchSimilarParams};
use crate::AppState;

pub async fn search(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    axum::extract::Query(_params): axum::extract::Query<SearchParams>,
) -> Json<serde_json::Value> {
    todo!("search")
}

pub async fn search_similar(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    axum::extract::Query(_params): axum::extract::Query<SearchSimilarParams>,
) -> Json<serde_json::Value> {
    todo!("search_similar")
}

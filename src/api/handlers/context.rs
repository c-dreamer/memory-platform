use axum::{extract::State, Json};
use std::sync::Arc;

use crate::api::auth::Auth;
use crate::api::dto::ContextParams;
use crate::AppState;

pub async fn get_context(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    axum::extract::Query(_params): axum::extract::Query<ContextParams>,
) -> Json<serde_json::Value> {
    todo!("get_context")
}

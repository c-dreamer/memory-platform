use axum::{Json, extract::State};
use std::sync::Arc;

use crate::AppState;
use crate::api::auth::Auth;
use crate::api::dto::ContextParams;

pub async fn get_context(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    axum::extract::Query(_params): axum::extract::Query<ContextParams>,
) -> Json<serde_json::Value> {
    todo!("get_context")
}

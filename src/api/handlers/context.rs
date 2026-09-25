use axum::{extract::State, http::StatusCode, Json};
use std::sync::Arc;

use crate::api::auth::Auth;
use crate::api::dto::ContextParams;
use crate::AppState;

fn err(e: impl std::fmt::Display) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": e.to_string()})),
    )
}

pub async fn get_context(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<ContextParams>,
) -> (StatusCode, Json<serde_json::Value>) {
    let context = match &state.embedding_service {
        Some(svc) => match svc.embed(&params.query).await {
            Ok(e) => {
                state
                    .db
                    .get_context_for_query(&params.query, &e.as_vec().to_vec(), 10)
                    .await
            }
            Err(e) => return err(e),
        },
        None => {
            state
                .db
                .get_keyword_context_for_query(&params.query, 10)
                .await
        }
    };

    let context = match context {
        Ok(c) => c,
        Err(e) => return err(e),
    };

    match serde_json::to_value(&context) {
        Ok(v) => (StatusCode::OK, Json(v)),
        Err(e) => err(e),
    }
}

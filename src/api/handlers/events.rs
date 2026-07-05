use axum::{extract::State, Json};
use std::sync::Arc;

use crate::api::auth::Auth;
use crate::api::dto::{EventCreate, EventResponse};
use crate::AppState;

pub async fn ingest_event(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<EventCreate>,
) -> Json<EventResponse> {
    todo!("ingest_event")
}

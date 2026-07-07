use axum::{Json, extract::State};
use std::sync::Arc;

use crate::AppState;
use crate::api::auth::Auth;
use crate::api::dto::{AgentRegisterRequest, AgentRegisterResponse};

pub async fn register_agent(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<AgentRegisterRequest>,
) -> Json<AgentRegisterResponse> {
    todo!("register_agent")
}

pub async fn get_agent(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    axum::extract::Path(_agent_id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    todo!("get_agent")
}

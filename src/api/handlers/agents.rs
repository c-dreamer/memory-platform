use axum::{extract::State, http::StatusCode, Json};
use std::sync::Arc;

use crate::api::auth::Auth;
use crate::api::dto::{AgentRegisterRequest, AgentRegisterResponse};
use crate::AppState;

pub async fn register_agent(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<AgentRegisterRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let metadata = serde_json::Value::Object(body.metadata.into_iter().collect());
    match state
        .db
        .register_agent(&body.name, &body.agent_type, &body.capabilities, &metadata)
        .await
    {
        Ok(agent) => (
            StatusCode::OK,
            Json(
                serde_json::to_value(AgentRegisterResponse {
                    id: agent.id.to_string(),
                    name: agent.name,
                    status: "registered".into(),
                })
                .expect("AgentRegisterResponse always serializes"),
            ),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

pub async fn get_agent(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(agent_id): axum::extract::Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    // Accept either a UUID or the agent's unique name, matching how
    // `register_agent` upserts on `name` — a caller that only has the name
    // it registered with shouldn't have to look up the UUID separately.
    let found = match uuid::Uuid::parse_str(&agent_id) {
        Ok(id) => state.db.get_agent(id).await,
        Err(_) => state.db.get_agent_by_name(&agent_id).await,
    };

    match found {
        Ok(Some(agent)) => match serde_json::to_value(agent) {
            Ok(v) => (StatusCode::OK, Json(v)),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(serde_json::json!({"error": e.to_string()})),
            ),
        },
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "agent not found"})),
        ),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(serde_json::json!({"error": e.to_string()})),
        ),
    }
}

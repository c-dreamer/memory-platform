use axum::{extract::State, http::StatusCode, Json};
use std::sync::Arc;

use crate::api::auth::Auth;
use crate::api::dto::{
    ConfidenceUpdateRequest, ExperienceConfidenceResponse, ExperienceListResponse,
    FindRelevantParams, ListExperiencesParams,
};
use crate::AppState;

fn err(e: impl std::fmt::Display) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": e.to_string()})),
    )
}

pub async fn list_experiences(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<ListExperiencesParams>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state
        .db
        .list_experiences(params.limit.clamp(1, 500) as i64)
        .await
    {
        Ok(experiences) => {
            let experiences: Vec<serde_json::Value> = experiences
                .into_iter()
                .filter_map(|e| serde_json::to_value(e).ok())
                .collect();
            let total = experiences.len();
            (
                StatusCode::OK,
                Json(
                    serde_json::to_value(ExperienceListResponse { experiences, total })
                        .expect("ExperienceListResponse always serializes"),
                ),
            )
        }
        Err(e) => err(e),
    }
}

pub async fn find_relevant_experiences(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<FindRelevantParams>,
) -> (StatusCode, Json<serde_json::Value>) {
    let Some(svc) = &state.experience_service else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "experience service unavailable"})),
        );
    };

    match svc
        .find_relevant(&params.goal, params.limit.clamp(1, 100))
        .await
    {
        Ok(experiences) => {
            let experiences: Vec<serde_json::Value> = experiences
                .into_iter()
                .filter_map(|e| serde_json::to_value(e).ok())
                .collect();
            let total = experiences.len();
            (
                StatusCode::OK,
                Json(
                    serde_json::to_value(ExperienceListResponse { experiences, total })
                        .expect("ExperienceListResponse always serializes"),
                ),
            )
        }
        Err(e) => err(e),
    }
}

pub async fn update_experience_confidence(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(experience_id): axum::extract::Path<String>,
    Json(body): Json<ConfidenceUpdateRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let Some(svc) = &state.experience_service else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"error": "experience service unavailable"})),
        );
    };

    // ExperienceService::update_confidence only applies one of two fixed
    // deltas (success/failure), not an arbitrary caller-supplied value, and
    // `experiences` has no column to persist a free-text reason in — so the
    // request's exact `delta` magnitude and `reason` are used only to pick a
    // direction, not stored verbatim. Widening the service to support
    // arbitrary deltas/reasons is a real schema change, not this endpoint's
    // call to make silently.
    let success = body.delta > 0.0;
    match svc.update_confidence(&experience_id, success).await {
        Ok(()) => {
            let id = match uuid::Uuid::parse_str(&experience_id) {
                Ok(id) => id,
                Err(_) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(serde_json::json!({"error": "invalid experience_id: not a UUID"})),
                    )
                }
            };
            match state.db.get_experience(id).await {
                Ok(Some(experience)) => (
                    StatusCode::OK,
                    Json(
                        serde_json::to_value(ExperienceConfidenceResponse {
                            status: "updated".into(),
                            experience: serde_json::to_value(experience).unwrap_or_default(),
                        })
                        .expect("ExperienceConfidenceResponse always serializes"),
                    ),
                ),
                Ok(None) => (
                    StatusCode::NOT_FOUND,
                    Json(serde_json::json!({"error": "experience not found"})),
                ),
                Err(e) => err(e),
            }
        }
        Err(e) => err(e),
    }
}

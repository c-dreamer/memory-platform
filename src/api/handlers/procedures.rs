use axum::{extract::State, http::StatusCode, Json};
use std::sync::Arc;
use uuid::Uuid;

use crate::api::auth::Auth;
use crate::api::dto::{
    ProcedureCreateRequest, ProcedureCreateResponse, ProcedureExecuteRequest,
    ProcedureExecuteResponse, ProcedureListResponse,
};
use crate::AppState;

fn err(e: impl std::fmt::Display) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": e.to_string()})),
    )
}

fn service_unavailable() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(serde_json::json!({"error": "procedure service unavailable"})),
    )
}

pub async fn list_procedures(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    match state.db.list_procedures().await {
        Ok(procedures) => {
            let procedures: Vec<serde_json::Value> = procedures
                .into_iter()
                .filter_map(|p| serde_json::to_value(p).ok())
                .collect();
            let total = procedures.len();
            (
                StatusCode::OK,
                Json(
                    serde_json::to_value(ProcedureListResponse { procedures, total })
                        .expect("ProcedureListResponse always serializes"),
                ),
            )
        }
        Err(e) => err(e),
    }
}

pub async fn get_procedure(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(procedure_id): axum::extract::Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let id = match Uuid::parse_str(&procedure_id) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid procedure_id: not a UUID"})),
            )
        }
    };

    match state.db.get_procedure(id).await {
        Ok(Some(procedure)) => match serde_json::to_value(procedure) {
            Ok(v) => (StatusCode::OK, Json(v)),
            Err(e) => err(e),
        },
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": "procedure not found"})),
        ),
        Err(e) => err(e),
    }
}

pub async fn create_procedure(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<ProcedureCreateRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let steps = match serde_json::to_value(&body.steps) {
        Ok(v) => v,
        Err(e) => return err(e),
    };

    match state
        .db
        .create_procedure(
            &body.name,
            body.description.as_deref(),
            &steps,
            body.trigger_pattern.as_deref(),
            None,
            &body.tags,
        )
        .await
    {
        Ok(procedure) => (
            StatusCode::OK,
            Json(
                serde_json::to_value(ProcedureCreateResponse {
                    id: procedure.id.to_string(),
                    name: procedure.name,
                    status: "created".into(),
                })
                .expect("ProcedureCreateResponse always serializes"),
            ),
        ),
        Err(e) => err(e),
    }
}

pub async fn execute_procedure(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    Json(body): Json<ProcedureExecuteRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let Some(svc) = &state.procedure_service else {
        return service_unavailable();
    };

    // Mirrors the MCP `procedure_run` tool: resolve by exact name via the
    // same candidate search, then execute by the resolved ID.
    let candidates = match svc.find_candidates(&body.name).await {
        Ok(c) => c,
        Err(e) => return err(e),
    };
    let Some(procedure) = candidates
        .into_iter()
        .find(|p| p.name.eq_ignore_ascii_case(&body.name))
    else {
        return (
            StatusCode::NOT_FOUND,
            Json(serde_json::json!({"error": format!("Procedure not found: {}", body.name)})),
        );
    };

    run_procedure(svc, procedure.id).await
}

pub async fn execute_procedure_by_id(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    axum::extract::Path(procedure_id): axum::extract::Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    let Some(svc) = &state.procedure_service else {
        return service_unavailable();
    };
    let id = match Uuid::parse_str(&procedure_id) {
        Ok(id) => id,
        Err(_) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(serde_json::json!({"error": "invalid procedure_id: not a UUID"})),
            )
        }
    };

    run_procedure(svc, id).await
}

async fn run_procedure(
    svc: &crate::services::procedure::ProcedureService,
    id: Uuid,
) -> (StatusCode, Json<serde_json::Value>) {
    match svc.execute(&id.to_string()).await {
        Ok(result) => (
            StatusCode::OK,
            Json(
                serde_json::to_value(ProcedureExecuteResponse {
                    status: if result.success { "success" } else { "failed" }.into(),
                    procedure_id: id.to_string(),
                    message: result.output,
                })
                .expect("ProcedureExecuteResponse always serializes"),
            ),
        ),
        Err(e) => err(e),
    }
}

pub async fn detect_procedure_candidates(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
) -> (StatusCode, Json<serde_json::Value>) {
    let Some(svc) = &state.procedure_service else {
        return service_unavailable();
    };

    match svc.promote_from_experiences(0.85).await {
        Ok(promoted) => (
            StatusCode::OK,
            Json(serde_json::json!({ "promoted": promoted, "count": promoted.len() })),
        ),
        Err(e) => err(e),
    }
}

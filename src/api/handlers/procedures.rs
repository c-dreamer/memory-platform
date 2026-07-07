use axum::{Json, extract::State};
use std::sync::Arc;

use crate::AppState;
use crate::api::auth::Auth;
use crate::api::dto::{ProcedureCreateRequest, ProcedureExecuteRequest};

pub async fn list_procedures(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    todo!("list_procedures")
}

pub async fn get_procedure(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    axum::extract::Path(_procedure_id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    todo!("get_procedure")
}

pub async fn create_procedure(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<ProcedureCreateRequest>,
) -> Json<serde_json::Value> {
    todo!("create_procedure")
}

pub async fn execute_procedure(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    Json(_body): Json<ProcedureExecuteRequest>,
) -> Json<serde_json::Value> {
    todo!("execute_procedure")
}

pub async fn execute_procedure_by_id(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
    axum::extract::Path(_procedure_id): axum::extract::Path<String>,
) -> Json<serde_json::Value> {
    todo!("execute_procedure_by_id")
}

pub async fn detect_procedure_candidates(
    _auth: Auth,
    State(_state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    todo!("detect_procedure_candidates")
}

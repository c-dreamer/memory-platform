use axum::{extract::State, http::StatusCode, Json};
use std::sync::Arc;

use crate::api::auth::Auth;
use crate::api::dto::{SearchParams, SearchResponse, SearchResultItem, SearchSimilarParams};
use crate::AppState;

fn err(e: impl std::fmt::Display) -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(serde_json::json!({"error": e.to_string()})),
    )
}

fn to_item(r: crate::search::SearchResult) -> SearchResultItem {
    SearchResultItem {
        id: r.id.to_string(),
        content: r.content,
        score: r.score,
        source_info: Some(r.source_info),
        decay_factor: r.decay_factor,
        vec_rank: r.vec_rank.map(|v| v as usize),
        kw_rank: r.kw_rank.map(|v| v as usize),
    }
}

pub async fn search(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<SearchParams>,
) -> (StatusCode, Json<serde_json::Value>) {
    let start = std::time::Instant::now();

    // No embedding service → force keyword-only, same fallback tool_memory_search
    // uses. Without this, "vector"/"hybrid" mode would hit VectorSearch with an
    // empty embedding, which pgvector rejects as a dimension mismatch (500).
    let (embedding, mode) = match &state.embedding_service {
        Some(svc) => match svc.embed(&params.q).await {
            Ok(e) => (e.as_vec().to_vec(), params.mode),
            Err(e) => return err(e),
        },
        None => (vec![], "keyword".to_string()),
    };

    match state
        .search
        .hybrid_search(
            "memories",
            &params.q,
            &embedding,
            &mode,
            params.limit.clamp(1, 500) as i64,
        )
        .await
    {
        Ok(results) => {
            let decay_applied =
                params.decay && state.config.decay_enabled && state.config.decay_apply_to_search;
            let total = results.len();
            let response = SearchResponse {
                results: results.into_iter().map(to_item).collect(),
                total,
                query_time_ms: start.elapsed().as_secs_f64() * 1000.0,
                mode,
                decay_applied,
                decay_half_life_days: decay_applied.then_some(state.config.decay_half_life_days),
            };
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).expect("SearchResponse always serializes")),
            )
        }
        Err(e) => err(e),
    }
}

pub async fn search_similar(
    _auth: Auth,
    State(state): State<Arc<AppState>>,
    axum::extract::Query(params): axum::extract::Query<SearchSimilarParams>,
) -> (StatusCode, Json<serde_json::Value>) {
    let start = std::time::Instant::now();

    let embedding = match &state.embedding_service {
        Some(svc) => match svc.embed(&params.q).await {
            Ok(e) => e.as_vec().to_vec(),
            Err(e) => return err(e),
        },
        None => {
            return (
                StatusCode::SERVICE_UNAVAILABLE,
                Json(serde_json::json!({"error": "embedding service unavailable"})),
            )
        }
    };

    match state
        .search
        .hybrid_search(
            "memories",
            &params.q,
            &embedding,
            "vector",
            params.limit.clamp(1, 500) as i64,
        )
        .await
    {
        Ok(results) => {
            let total = results.len();
            let response = SearchResponse {
                results: results.into_iter().map(to_item).collect(),
                total,
                query_time_ms: start.elapsed().as_secs_f64() * 1000.0,
                mode: "vector".into(),
                decay_applied: false,
                decay_half_life_days: None,
            };
            (
                StatusCode::OK,
                Json(serde_json::to_value(response).expect("SearchResponse always serializes")),
            )
        }
        Err(e) => err(e),
    }
}

//! Auth — X-API-Key header verification.
//!
//! Mirrors the Python `verify_api_key` dependency:
//! - If `api_key` config is empty (dev mode), allow all requests.
//! - Otherwise, require `X-API-Key` header to match configured key.

use axum::{
    extract::FromRequestParts,
    http::{StatusCode, request::Parts},
    response::{IntoResponse, Response},
};
use std::sync::Arc;

use crate::AppState;

/// Auth extractor that validates the X-API-Key header.
#[derive(Debug, Clone)]
pub struct Auth;

/// Error returned when authentication fails.
#[derive(Debug)]
pub struct AuthError;

impl IntoResponse for AuthError {
    fn into_response(self) -> Response {
        let body = serde_json::json!({
            "error": "unauthorized",
            "detail": "Invalid or missing X-API-Key header"
        });
        (StatusCode::UNAUTHORIZED, axum::Json(body)).into_response()
    }
}

impl FromRequestParts<Arc<AppState>> for Auth {
    type Rejection = AuthError;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        // Dev mode — auth disabled when api_key is empty
        if state.config.api_key.is_empty() {
            return Ok(Auth);
        }

        // Extract X-API-Key header
        let api_key = parts
            .headers
            .get("X-API-Key")
            .and_then(|v| v.to_str().ok())
            .ok_or(AuthError)?;

        if api_key != state.config.api_key {
            return Err(AuthError);
        }

        Ok(Auth)
    }
}

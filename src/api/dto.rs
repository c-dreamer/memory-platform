//! Request/Response DTOs for all 23 HTTP routes.
//!
//! Mirrors Pydantic models from the Python FastAPI server.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

// ─── Agents ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct AgentRegisterRequest {
    pub name: String,
    pub agent_type: String,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct AgentRegisterResponse {
    pub id: String,
    pub name: String,
    pub status: String,
}

// ─── Sessions ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SessionCreateRequest {
    pub agent_id: String,
    #[serde(default)]
    pub goal: Option<String>,
    #[serde(default)]
    pub parent_session_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SessionCreateResponse {
    pub id: String,
    pub goal: Option<String>,
    pub status: String,
}

#[derive(Debug, Deserialize)]
pub struct SummarizeRequest {
    pub summary: Option<String>,
}

// ─── Memories ─────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct MemoryCreateRequest {
    pub content: String,
    #[serde(default = "default_content_type")]
    pub content_type: String,
    #[serde(default = "default_importance")]
    pub importance: f64,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub metadata: HashMap<String, serde_json::Value>,
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
}

fn default_content_type() -> String {
    "note".to_string()
}

fn default_importance() -> f64 {
    0.5
}

#[derive(Debug, Serialize)]
pub struct MemoryCreateResponse {
    pub id: String,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contradictions_detected: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub contradiction_with: Option<Vec<String>>,
}

// ─── Search ────────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct SearchParams {
    pub q: String,
    #[serde(default = "default_search_limit")]
    pub limit: usize,
    #[serde(default = "default_search_mode")]
    pub mode: String,
    #[serde(default = "default_true")]
    pub decay: bool,
}

fn default_search_limit() -> usize {
    10
}

fn default_search_mode() -> String {
    "hybrid".to_string()
}

fn default_true() -> bool {
    true
}

#[derive(Debug, Deserialize)]
pub struct SearchSimilarParams {
    pub q: String,
    #[serde(default = "default_similar_limit")]
    pub limit: usize,
}

fn default_similar_limit() -> usize {
    5
}

#[derive(Debug, Serialize)]
pub struct SearchResultItem {
    pub id: String,
    pub content: String,
    pub score: f64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_info: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decay_factor: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub vec_rank: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub kw_rank: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct SearchResponse {
    pub results: Vec<SearchResultItem>,
    pub total: usize,
    pub query_time_ms: f64,
    pub mode: String,
    pub decay_applied: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decay_half_life_days: Option<f64>,
}

// ─── Context ──────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ContextParams {
    pub query: String,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(default)]
    pub agent_id: Option<String>,
}

// ─── Procedures ───────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ProcedureCreateRequest {
    pub name: String,
    #[serde(default)]
    pub description: Option<String>,
    pub steps: Vec<HashMap<String, serde_json::Value>>,
    #[serde(default)]
    pub trigger_pattern: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct ProcedureExecuteRequest {
    pub name: String,
    #[serde(default)]
    pub params: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct ProcedureCreateResponse {
    pub id: String,
    pub name: String,
    pub status: String,
}

#[derive(Debug, Serialize)]
pub struct ProcedureExecuteResponse {
    pub status: String,
    pub procedure_id: String,
    pub message: String,
}

#[derive(Debug, Serialize)]
pub struct ProcedureListResponse {
    pub procedures: Vec<serde_json::Value>,
    pub total: usize,
}

// ─── Ingestion ────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct IngestRequest {
    #[serde(default = "default_ingest_source")]
    pub source: String,
    #[serde(default = "default_ingest_path")]
    pub path: String,
    #[serde(default = "default_true")]
    pub recursive: bool,
}

fn default_ingest_source() -> String {
    "filesystem".to_string()
}

fn default_ingest_path() -> String {
    "/vault".to_string()
}

// ─── Events ───────────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct EventCreate {
    #[serde(default)]
    pub agent_id: Option<String>,
    #[serde(default)]
    pub session_id: Option<String>,
    #[serde(rename = "type")]
    pub event_type: String,
    pub payload: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Serialize)]
pub struct EventResponse {
    pub event_id: String,
    pub memory_id: String,
    pub status: String,
    pub table: String,
}

// ─── Experiences ──────────────────────────────────────────────────────────────

#[derive(Debug, Deserialize)]
pub struct ListExperiencesParams {
    #[serde(default = "default_experience_limit")]
    pub limit: usize,
}

fn default_experience_limit() -> usize {
    20
}

#[derive(Debug, Deserialize)]
pub struct FindRelevantParams {
    pub goal: String,
    #[serde(default = "default_relevant_limit")]
    pub limit: usize,
}

fn default_relevant_limit() -> usize {
    3
}

#[derive(Debug, Deserialize)]
pub struct ConfidenceUpdateRequest {
    pub delta: f64,
    pub reason: String,
}

#[derive(Debug, Serialize)]
pub struct ExperienceListResponse {
    pub experiences: Vec<serde_json::Value>,
    pub total: usize,
}

#[derive(Debug, Serialize)]
pub struct ExperienceConfidenceResponse {
    pub status: String,
    pub experience: serde_json::Value,
}

// ─── Health ───────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct HealthResponse {
    pub status: String,
    pub postgres: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub redis: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub neo4j: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stats: Option<serde_json::Value>,
}

// ─── Root ─────────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct RootResponse {
    pub service: String,
    pub version: String,
    pub docs: String,
    pub health: String,
}

// ─── Generic ──────────────────────────────────────────────────────────────────

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

//! Codex session ingestion — mirrors local `~/.codex/sessions/**/*.jsonl`
//! into the memory platform as searchable session summaries.

use crate::ingest::{IngestEngine, IngestReport};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};
use walkdir::WalkDir;

#[derive(Debug, Clone)]
struct CodexSessionFile {
    path: PathBuf,
    modified_at: DateTime<Utc>,
    checksum: String,
    content: String,
}

#[derive(Debug, Default, Clone)]
pub struct CodexSessionSummary {
    pub scanned: usize,
    pub imported: usize,
    pub skipped_duplicate: usize,
    pub errors: Vec<String>,
}

fn scan_sessions(root: &Path) -> Result<Vec<CodexSessionFile>> {
    let mut files = Vec::new();

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| entry.ok())
    {
        let path = entry.path();
        if path.extension().and_then(|ext| ext.to_str()) != Some("jsonl") {
            continue;
        }

        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(err) => {
                warn!("Failed to read Codex session {:?}: {err}", path);
                continue;
            }
        };

        let checksum = hex::encode(Sha256::digest(content.as_bytes()));
        let modified_at = fs::metadata(path)
            .ok()
            .and_then(|meta| meta.modified().ok())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(Utc::now);

        files.push(CodexSessionFile {
            path: path.to_path_buf(),
            modified_at,
            checksum,
            content,
        });
    }

    files.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.path.cmp(&b.path))
    });

    Ok(files)
}

fn session_id_from_summary(summary: &CodexSessionSummaryData, fallback: &Path) -> String {
    summary.session_id.clone().unwrap_or_else(|| {
        fallback
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or("codex-session")
            .to_string()
    })
}

#[derive(Debug, Default, Clone)]
struct CodexSessionSummaryData {
    session_id: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    cwd: Option<String>,
    originator: Option<String>,
    source: Option<String>,
    thread_source: Option<String>,
    model_provider: Option<String>,
    cli_version: Option<String>,
    line_count: usize,
    message_count: usize,
    tool_call_count: usize,
    user_excerpt: Option<String>,
    assistant_excerpt: Option<String>,
}

fn parse_session_summary(content: &str) -> CodexSessionSummaryData {
    let mut summary = CodexSessionSummaryData::default();

    for line in content.lines() {
        summary.line_count += 1;
        let parsed: Value = match serde_json::from_str(line) {
            Ok(value) => value,
            Err(_) => continue,
        };

        match parsed.get("type").and_then(Value::as_str) {
            Some("session_meta") => {
                if let Some(payload) = parsed.get("payload") {
                    summary.session_id = payload
                        .get("session_id")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                    summary.timestamp = payload
                        .get("timestamp")
                        .and_then(Value::as_str)
                        .and_then(|ts| DateTime::parse_from_rfc3339(ts).ok())
                        .map(|dt| dt.with_timezone(&Utc));
                    summary.cwd = payload
                        .get("cwd")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                    summary.originator = payload
                        .get("originator")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                    summary.source = payload
                        .get("source")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                    summary.thread_source = payload
                        .get("thread_source")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                    summary.model_provider = payload
                        .get("model_provider")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                    summary.cli_version = payload
                        .get("cli_version")
                        .and_then(Value::as_str)
                        .map(ToString::to_string);
                }
            }
            Some("response_item") => {
                summary.message_count += 1;
                if let Some(payload) = parsed.get("payload") {
                    if payload.get("type").and_then(Value::as_str) == Some("message") {
                        if let Some(content) = payload.get("content").and_then(Value::as_array) {
                            for chunk in content {
                                let chunk_type = chunk.get("type").and_then(Value::as_str);
                                let text = chunk.get("text").and_then(Value::as_str);
                                match (chunk_type, text) {
                                    (Some("input_text"), Some(text))
                                        if summary.user_excerpt.is_none() =>
                                    {
                                        summary.user_excerpt = Some(truncate(text, 240));
                                    }
                                    (Some("output_text"), Some(text))
                                        if summary.assistant_excerpt.is_none() =>
                                    {
                                        summary.assistant_excerpt = Some(truncate(text, 240));
                                    }
                                    _ => {}
                                }
                            }
                        }
                    }

                    if payload.get("type").and_then(Value::as_str) == Some("function_call") {
                        summary.tool_call_count += 1;
                    }
                }
            }
            Some("event_msg") => {
                if parsed
                    .get("payload")
                    .and_then(|payload| payload.get("type"))
                    .and_then(Value::as_str)
                    == Some("token_count")
                {
                    continue;
                }
            }
            _ => {}
        }
    }

    summary
}

pub async fn ingest_codex_sessions(
    engine: &IngestEngine,
    sessions_root: &Path,
    report: &mut IngestReport,
) -> Result<CodexSessionSummary> {
    info!("Ingesting Codex sessions from: {}", sessions_root.display());

    if !sessions_root.is_dir() {
        anyhow::bail!(
            "Codex sessions directory not found: {}",
            sessions_root.display()
        );
    }

    let files = scan_sessions(sessions_root)?;
    let mut summary = CodexSessionSummary {
        scanned: files.len(),
        ..Default::default()
    };

    for file in files {
        let session_summary = parse_session_summary(&file.content);
        let session_id = session_id_from_summary(&session_summary, &file.path);
        let rel_path = file
            .path
            .strip_prefix(sessions_root)
            .unwrap_or(&file.path)
            .to_string_lossy()
            .replace('\\', "/");
        let doc_path = format!("codex://{rel_path}");

        let doc_exists: Option<(uuid::Uuid, String)> =
            sqlx::query_as("SELECT id, checksum FROM documents WHERE path = $1 LIMIT 1")
                .bind(&doc_path)
                .fetch_optional(engine.pool())
                .await
                .with_context(|| format!("checking Codex document: {}", file.path.display()))?;

        if doc_exists
            .as_ref()
            .is_some_and(|(_, checksum)| checksum == &file.checksum)
        {
            summary.skipped_duplicate += 1;
            continue;
        }

        let title = format!(
            "Codex session {}",
            file.path
                .file_stem()
                .and_then(|stem| stem.to_str())
                .unwrap_or("session")
        );

        let imported_at = Utc::now();
        let age_seconds = imported_at
            .signed_duration_since(file.modified_at)
            .num_seconds()
            .max(0);
        let meta = serde_json::json!({
            "source": "codex-session",
            "codex_session_id": session_id,
            "path": file.path.to_string_lossy(),
            "relative_path": rel_path,
            "source_last_modified_at": file.modified_at.to_rfc3339(),
            "source_age_seconds": age_seconds,
            "cwd": session_summary.cwd.clone(),
            "originator": session_summary.originator.clone(),
            "source_system": session_summary.source.clone(),
            "thread_source": session_summary.thread_source.clone(),
            "model_provider": session_summary.model_provider.clone(),
            "cli_version": session_summary.cli_version.clone(),
            "line_count": session_summary.line_count,
            "message_count": session_summary.message_count,
            "tool_call_count": session_summary.tool_call_count,
            "imported_at": imported_at.to_rfc3339(),
        });

        let summary_text = format!(
            "Codex session {session_id}\n\
             Path: {}\n\
             CWD: {}\n\
             Originator: {}\n\
             Source: {}\n\
             Thread source: {}\n\
             Model provider: {}\n\
             CLI version: {}\n\
             Lines: {}\n\
             Messages: {}\n\
             Tool calls: {}\n\
             User excerpt: {}\n\
             Assistant excerpt: {}",
            file.path.display(),
            session_summary.cwd.as_deref().unwrap_or("unknown"),
            session_summary.originator.as_deref().unwrap_or("unknown"),
            session_summary.source.as_deref().unwrap_or("unknown"),
            session_summary
                .thread_source
                .as_deref()
                .unwrap_or("unknown"),
            session_summary
                .model_provider
                .as_deref()
                .unwrap_or("unknown"),
            session_summary.cli_version.as_deref().unwrap_or("unknown"),
            session_summary.line_count,
            session_summary.message_count,
            session_summary.tool_call_count,
            session_summary.user_excerpt.as_deref().unwrap_or("n/a"),
            session_summary
                .assistant_excerpt
                .as_deref()
                .unwrap_or("n/a"),
        );

        let upsert = engine
            .insert_document(
                &doc_path,
                Some("codex"),
                Some(&title),
                &summary_text,
                Some(&file.checksum),
                &meta,
                Some(summary_text.len() as i32),
                Some(file.modified_at),
            )
            .await;

        match upsert {
            Ok(_) => {
                let started_at = session_summary.timestamp.unwrap_or(file.modified_at);
                let mem_session_id = engine
                    .upsert_source_session(
                        &format!("codex:{session_id}"),
                        &title,
                        "completed",
                        Some(&format!(
                            "Mirrored Codex session from {}",
                            file.path.display()
                        )),
                        started_at,
                        Some(file.modified_at),
                    )
                    .await
                    .context("inserting Codex session row")?;

                let tags = vec![
                    "codex".to_string(),
                    "codex-session".to_string(),
                    session_summary
                        .originator
                        .clone()
                        .unwrap_or_else(|| "unknown".to_string()),
                ];

                let experience_text = format!(
                    "Codex session mirrored from {}\n\
                     Session ID: {}\n\
                     Messages: {}\n\
                     Tool calls: {}\n\
                     CWD: {}\n\
                     User excerpt: {}\n\
                     Assistant excerpt: {}",
                    file.path.display(),
                    session_id,
                    session_summary.message_count,
                    session_summary.tool_call_count,
                    session_summary.cwd.as_deref().unwrap_or("unknown"),
                    session_summary.user_excerpt.as_deref().unwrap_or("n/a"),
                    session_summary
                        .assistant_excerpt
                        .as_deref()
                        .unwrap_or("n/a"),
                );

                let _ = engine
                    .insert_experience(
                        Some(mem_session_id),
                        &title,
                        Some("Codex session mirror"),
                        Some(&experience_text),
                        Some("Codex session archived for memory search"),
                        &tags,
                        Some((file.modified_at - started_at).num_seconds().max(0) as i32),
                        Some("codex"),
                    )
                    .await;

                let _ = engine
                    .insert_memory(
                        Some(mem_session_id),
                        &summary_text,
                        "session",
                        0.7,
                        &tags,
                        &meta,
                    )
                    .await;

                summary.imported += 1;
                info!("Imported Codex session: {}", file.path.display());
            }
            Err(err) => {
                let message = format!(
                    "Failed to import Codex session {}: {err}",
                    file.path.display()
                );
                warn!("{message}");
                summary.errors.push(message);
            }
        }
    }

    report
        .sources_processed
        .insert("codex-sessions".to_string(), summary.scanned as u64);
    report.sessions_created += summary.imported as u64;
    report.experiences_created += summary.imported as u64;
    report.memories_created += summary.imported as u64;
    report.errors += summary.errors.len() as u64;

    Ok(summary)
}

fn truncate(value: &str, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        return value.to_string();
    }
    let mut truncated = value
        .chars()
        .take(max_chars.saturating_sub(1))
        .collect::<String>();
    truncated.push_str("...");
    truncated
}

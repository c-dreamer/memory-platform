//! Codex session ingestion — mirrors local `~/.codex/sessions/**/*.jsonl`
//! into the memory platform as searchable session summaries.

use crate::ingest::session_safety::{redact_secrets, stable_source_uuid};
use crate::ingest::{IngestEngine, IngestReport};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{HashMap, HashSet};
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
    pub sessions_reconciled: usize,
    pub memories_reconciled: usize,
    pub experiences_reconciled: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct CodexInventory {
    pub source_file_count: usize,
    pub source_count: usize,
    pub database_source_count: usize,
    pub missing_source_ids: Vec<String>,
    pub extra_source_ids: Vec<String>,
    pub duplicate_source_ids: HashMap<String, usize>,
    pub duplicate_source_keys: HashMap<String, i64>,
    pub missing_documents: Vec<String>,
    pub changed_documents: Vec<String>,
    pub database_only_documents: Vec<String>,
    pub active_path: Option<String>,
    pub active_age_seconds: Option<i64>,
    pub settled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CodexReconcilePlan {
    update_document: bool,
    reconcile_derived_rows: bool,
}

fn reconcile_plan(document_checksum_matches: bool) -> CodexReconcilePlan {
    CodexReconcilePlan {
        update_document: !document_checksum_matches,
        // Side rows must be repaired even if the source document was already indexed.
        reconcile_derived_rows: true,
    }
}

fn scan_sessions(root: &Path) -> Result<Vec<CodexSessionFile>> {
    if !root.is_dir() {
        anyhow::bail!("Codex sessions directory not found");
    }
    let mut files = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(error) => {
                warn!("Skipping unreadable Codex archive entry: {error}");
                None
            }
        })
    {
        let path = entry.path();
        if !entry.file_type().is_file()
            || path.extension().and_then(|ext| ext.to_str()) != Some("jsonl")
        {
            continue;
        }
        let content = match fs::read_to_string(path) {
            Ok(content) => content,
            Err(_) => {
                warn!("Skipping unreadable Codex archive file");
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
    files.sort_by(|left, right| {
        right
            .modified_at
            .cmp(&left.modified_at)
            .then_with(|| left.path.cmp(&right.path))
    });
    Ok(files)
}

fn relative_path(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Compare source identities and document fingerprints without mutating the DB.
pub async fn inventory_codex_sessions(
    engine: &IngestEngine,
    sessions_root: &Path,
    settle_seconds: i64,
) -> Result<CodexInventory> {
    let files = scan_sessions(sessions_root)?;
    let mut source_id_counts = HashMap::<String, usize>::new();
    let mut source_documents = HashMap::<String, String>::new();
    for file in &files {
        let parsed = parse_session_summary(&file.content);
        let session_id = session_id_from_summary(&parsed, &file.path);
        *source_id_counts.entry(session_id).or_default() += 1;
        let path = format!("codex://{}", relative_path(sessions_root, &file.path));
        source_documents.insert(path, file.checksum.clone());
    }
    let source_ids: HashSet<String> = source_id_counts.keys().cloned().collect();

    let rows = sqlx::query_as::<_, (String, i64)>(
        "SELECT source_key, count(*)::bigint FROM sessions WHERE source_key LIKE 'codex:%' GROUP BY source_key",
    )
    .fetch_all(engine.pool())
    .await?;
    let duplicate_source_keys = rows.into_iter().filter(|(_, count)| *count > 1).collect();
    let db_ids: HashSet<String> = sqlx::query_scalar::<_, String>(
        "SELECT DISTINCT substring(source_key from 7) FROM sessions WHERE source_key LIKE 'codex:%'",
    )
    .fetch_all(engine.pool())
    .await?
    .into_iter()
    .collect();

    let db_documents = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT path, checksum FROM documents WHERE path LIKE 'codex://%'",
    )
    .fetch_all(engine.pool())
    .await?
    .into_iter()
    .collect::<HashMap<_, _>>();
    let mut missing_documents = Vec::new();
    let mut changed_documents = Vec::new();
    for (path, checksum) in &source_documents {
        match db_documents.get(path) {
            None => missing_documents.push(path.clone()),
            Some(db_checksum) if db_checksum.as_deref() != Some(checksum.as_str()) => {
                changed_documents.push(path.clone())
            }
            Some(_) => {}
        }
    }
    let mut database_only_documents: Vec<String> = db_documents
        .keys()
        .filter(|path| !source_documents.contains_key(*path))
        .cloned()
        .collect();

    let mut missing_source_ids: Vec<_> = source_ids.difference(&db_ids).cloned().collect();
    let mut extra_source_ids: Vec<_> = db_ids.difference(&source_ids).cloned().collect();
    missing_source_ids.sort();
    extra_source_ids.sort();
    missing_documents.sort();
    changed_documents.sort();
    database_only_documents.sort();
    let duplicate_source_ids = source_id_counts
        .into_iter()
        .filter(|(_, count)| *count > 1)
        .collect();

    let (active_path, active_age_seconds) = files
        .first()
        .map(|file| {
            (
                Some(relative_path(sessions_root, &file.path)),
                Some(
                    Utc::now()
                        .signed_duration_since(file.modified_at)
                        .num_seconds()
                        .max(0),
                ),
            )
        })
        .unwrap_or((None, None));
    let settled = active_age_seconds
        .map(|age| age >= settle_seconds)
        .unwrap_or(true);
    Ok(CodexInventory {
        source_file_count: files.len(),
        source_count: source_ids.len(),
        database_source_count: db_ids.len(),
        missing_source_ids,
        extra_source_ids,
        duplicate_source_ids,
        duplicate_source_keys,
        missing_documents,
        changed_documents,
        database_only_documents,
        active_path,
        active_age_seconds,
        settled,
    })
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
    redaction_count: usize,
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
                        let role = payload.get("role").and_then(Value::as_str);
                        let channel = payload
                            .get("channel")
                            .or_else(|| parsed.get("channel"))
                            .and_then(Value::as_str)
                            .unwrap_or("")
                            .to_ascii_lowercase();
                        let hidden_channel = matches!(
                            channel.as_str(),
                            "analysis"
                                | "thinking"
                                | "reasoning"
                                | "justify"
                                | "confidence"
                                | "delegate_summary"
                                | "juicepred"
                                | "redacted_thinking"
                        );
                        if !hidden_channel {
                            if let Some(content) = payload.get("content").and_then(Value::as_array)
                            {
                                for chunk in content {
                                    let chunk_type = chunk.get("type").and_then(Value::as_str);
                                    let text = chunk.get("text").and_then(Value::as_str);
                                    match (role, chunk_type, text) {
                                        (Some("user"), Some("input_text"), Some(text))
                                            if summary.user_excerpt.is_none() =>
                                        {
                                            let (safe, redactions) = redact_secrets(text);
                                            summary.redaction_count += redactions;
                                            summary.user_excerpt = Some(truncate(&safe, 240));
                                        }
                                        (Some("assistant"), Some("output_text"), Some(text))
                                            if summary.assistant_excerpt.is_none() =>
                                        {
                                            let (safe, redactions) = redact_secrets(text);
                                            summary.redaction_count += redactions;
                                            summary.assistant_excerpt = Some(truncate(&safe, 240));
                                        }
                                        _ => {}
                                    }
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
        let source_session_id = session_id_from_summary(&session_summary, &file.path);
        let rel_path = relative_path(sessions_root, &file.path);
        let doc_path = format!("codex://{rel_path}");
        let title = format!("Codex session {source_session_id}");
        let started_at = session_summary.timestamp.unwrap_or(file.modified_at);

        let doc_exists: Option<(uuid::Uuid, Option<String>)> =
            sqlx::query_as("SELECT id, checksum FROM documents WHERE path = $1 LIMIT 1")
                .bind(&doc_path)
                .fetch_optional(engine.pool())
                .await
                .with_context(|| format!("checking Codex document {rel_path}"))?;
        let document_checksum_matches = doc_exists
            .as_ref()
            .is_some_and(|(_, checksum)| checksum.as_deref() == Some(file.checksum.as_str()));
        let plan = reconcile_plan(document_checksum_matches);

        // Reconcile the canonical session row independently of the document
        // checksum. A partial previous run may have saved the document but
        // failed before creating its session row.
        let session_row_id = match engine
            .upsert_source_session(
                &format!("codex:{source_session_id}"),
                &title,
                "completed",
                Some("Mirrored from the local Codex session archive"),
                started_at,
                Some(file.modified_at),
            )
            .await
        {
            Ok(id) => {
                summary.sessions_reconciled += 1;
                id
            }
            Err(_) => {
                summary
                    .errors
                    .push(format!("session row failed for {rel_path}"));
                warn!(source_path = %rel_path, "Codex session row reconciliation failed");
                continue;
            }
        };

        let safe_user = session_summary.user_excerpt.clone().unwrap_or_default();
        let safe_assistant = session_summary
            .assistant_excerpt
            .clone()
            .unwrap_or_default();
        let meta = serde_json::json!({
            "source": "codex-session",
            "codex_session_id": source_session_id,
            "relative_path": rel_path,
            "source_last_modified_at": file.modified_at.to_rfc3339(),
            "source_checksum": file.checksum,
            "originator": session_summary.originator,
            "source_system": session_summary.source,
            "thread_source": session_summary.thread_source,
            "model_provider": session_summary.model_provider,
            "cli_version": session_summary.cli_version,
            "line_count": session_summary.line_count,
            "message_count": session_summary.message_count,
            "tool_call_count": session_summary.tool_call_count,
            "redacted_excerpt_count": session_summary.redaction_count
        });
        let summary_text = format!(
            "Codex session {source_session_id}\n\
             Source archive: {rel_path}\n\
             Originator: {}\n\
             Source: {}\n\
             Messages: {}\n\
             Tool calls: {}\n\
             User excerpt: {}\n\
             Assistant excerpt: {}",
            session_summary.originator.as_deref().unwrap_or("unknown"),
            session_summary.source.as_deref().unwrap_or("unknown"),
            session_summary.message_count,
            session_summary.tool_call_count,
            safe_user,
            safe_assistant,
        );
        let (summary_text, additional_redactions) = redact_secrets(&summary_text);
        let mut meta = meta;
        meta["redacted_excerpt_count"] =
            serde_json::json!(session_summary.redaction_count + additional_redactions);

        if plan.update_document {
            match engine
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
                .await
            {
                Ok(_) => summary.imported += 1,
                Err(_) => {
                    summary
                        .errors
                        .push(format!("document write failed for {rel_path}"));
                    warn!(source_path = %rel_path, "Codex document upsert failed");
                    continue;
                }
            }
        } else {
            summary.skipped_duplicate += 1;
        }

        if plan.reconcile_derived_rows {
            let tags = vec![
                "codex".to_string(),
                "codex-session".to_string(),
                session_summary
                    .originator
                    .clone()
                    .unwrap_or_else(|| "unknown".to_string()),
            ];
            let experience_text = format!(
                "Codex session mirrored from {rel_path}\n\
                 Session ID: {source_session_id}\n\
                 Messages: {}\n\
                 Tool calls: {}\n\
                 User excerpt: {}\n\
                 Assistant excerpt: {}",
                session_summary.message_count,
                session_summary.tool_call_count,
                safe_user,
                safe_assistant,
            );
            if upsert_codex_memory(
                engine,
                session_row_id,
                &doc_path,
                &summary_text,
                &tags,
                &meta,
            )
            .await
            .is_ok()
            {
                summary.memories_reconciled += 1;
            } else {
                summary
                    .errors
                    .push(format!("memory row failed for {rel_path}"));
                warn!(source_path = %rel_path, "Codex memory row reconciliation failed");
            }
            if upsert_codex_experience(
                engine,
                session_row_id,
                &doc_path,
                &title,
                &experience_text,
                &tags,
                Some((file.modified_at - started_at).num_seconds().max(0) as i32),
            )
            .await
            .is_ok()
            {
                summary.experiences_reconciled += 1;
            } else {
                summary
                    .errors
                    .push(format!("experience row failed for {rel_path}"));
                warn!(source_path = %rel_path, "Codex experience row reconciliation failed");
            }
        }
    }

    report
        .sources_processed
        .insert("codex-sessions".to_string(), summary.scanned as u64);
    report.documents_created += summary.imported as u64;
    report.sessions_created += summary.sessions_reconciled as u64;
    report.experiences_created += summary.experiences_reconciled as u64;
    report.memories_created += summary.memories_reconciled as u64;
    report.errors += summary.errors.len() as u64;
    Ok(summary)
}

async fn upsert_codex_memory(
    engine: &IngestEngine,
    session_id: uuid::Uuid,
    doc_path: &str,
    content: &str,
    tags: &[String],
    metadata: &Value,
) -> Result<()> {
    let relative_path = doc_path.strip_prefix("codex://").unwrap_or(doc_path);
    let existing_id: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM memories WHERE metadata->>'source' = 'codex-session' AND metadata->>'relative_path' = $1 ORDER BY created_at LIMIT 1",
    )
    .bind(relative_path)
    .fetch_optional(engine.pool())
    .await?;
    let id = existing_id.unwrap_or_else(|| stable_source_uuid("codex-memory", doc_path));
    sqlx::query(
        r#"INSERT INTO memories (id,session_id,content,content_type,importance,tags,metadata)
           VALUES ($1,$2,$3,'session',0.7,$4,$5)
           ON CONFLICT (id) DO UPDATE SET session_id=EXCLUDED.session_id,
             content=EXCLUDED.content, content_type=EXCLUDED.content_type,
             importance=EXCLUDED.importance, tags=EXCLUDED.tags, metadata=EXCLUDED.metadata,
             updated_at=now()
           WHERE (memories.session_id,memories.content,memories.content_type,memories.importance,memories.tags,memories.metadata)
             IS DISTINCT FROM (EXCLUDED.session_id,EXCLUDED.content,EXCLUDED.content_type,EXCLUDED.importance,EXCLUDED.tags,EXCLUDED.metadata)"#,
    )
    .bind(id)
    .bind(session_id)
    .bind(content)
    .bind(tags)
    .bind(metadata)
    .execute(engine.pool())
    .await?;
    Ok(())
}

async fn upsert_codex_experience(
    engine: &IngestEngine,
    session_id: uuid::Uuid,
    doc_path: &str,
    goal: &str,
    result: &str,
    tags: &[String],
    duration_seconds: Option<i32>,
) -> Result<()> {
    let existing_id: Option<uuid::Uuid> = sqlx::query_scalar(
        "SELECT id FROM experiences WHERE session_id=$1 AND goal=$2 AND related_project='codex' AND tags @> ARRAY['codex-session']::text[] ORDER BY created_at LIMIT 1",
    )
    .bind(session_id)
    .bind(goal)
    .fetch_optional(engine.pool())
    .await?;
    let id = existing_id.unwrap_or_else(|| stable_source_uuid("codex-experience", doc_path));
    sqlx::query(
        r#"INSERT INTO experiences (id,session_id,goal,reasoning_summary,result,lessons_learned,tags,duration_seconds,related_project)
           VALUES ($1,$2,$3,'Codex session mirror',$4,'Codex session indexed for local memory search',$5,$6,'codex')
           ON CONFLICT (id) DO UPDATE SET session_id=EXCLUDED.session_id,goal=EXCLUDED.goal,
             reasoning_summary=EXCLUDED.reasoning_summary,result=EXCLUDED.result,
             lessons_learned=EXCLUDED.lessons_learned,tags=EXCLUDED.tags,
             duration_seconds=EXCLUDED.duration_seconds,related_project=EXCLUDED.related_project
           WHERE (experiences.session_id,experiences.goal,experiences.reasoning_summary,experiences.result,experiences.lessons_learned,experiences.tags,experiences.duration_seconds,experiences.related_project)
             IS DISTINCT FROM (EXCLUDED.session_id,EXCLUDED.goal,EXCLUDED.reasoning_summary,EXCLUDED.result,EXCLUDED.lessons_learned,EXCLUDED.tags,EXCLUDED.duration_seconds,EXCLUDED.related_project)"#,
    )
    .bind(id)
    .bind(session_id)
    .bind(goal)
    .bind(result)
    .bind(tags)
    .bind(duration_seconds)
    .execute(engine.pool())
    .await?;
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn checksum_match_still_reconciles_session_and_derived_rows() {
        let plan = reconcile_plan(true);
        assert!(!plan.update_document);
        assert!(plan.reconcile_derived_rows);
    }

    #[test]
    fn changed_document_reconciles_all_source_rows() {
        let plan = reconcile_plan(false);
        assert!(plan.update_document);
        assert!(plan.reconcile_derived_rows);
    }

    #[test]
    fn parser_redacts_secrets_and_ignores_hidden_reasoning_and_tool_payloads() {
        let fixture = concat!(
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"user\",\"content\":[{\"type\":\"input_text\",\"text\":\"api_key=sk-test01234567890123456789\"}]}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"channel\":\"analysis\",\"content\":[{\"type\":\"output_text\",\"text\":\"HIDDEN-THOUGHT\"}]}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"function_call\",\"name\":\"Read\",\"arguments\":\"TOOL-ARGUMENTS\"}}\n",
            "{\"type\":\"response_item\",\"payload\":{\"type\":\"message\",\"role\":\"assistant\",\"content\":[{\"type\":\"output_text\",\"text\":\"Visible answer\"}]}}\n"
        );
        let parsed = parse_session_summary(fixture);
        assert!(parsed
            .user_excerpt
            .as_deref()
            .unwrap()
            .contains("[REDACTED]"));
        assert_eq!(parsed.assistant_excerpt.as_deref(), Some("Visible answer"));
        assert_eq!(parsed.tool_call_count, 1);
        assert!(parsed.redaction_count > 0);
        assert_eq!(parsed.message_count, 4);
    }
}

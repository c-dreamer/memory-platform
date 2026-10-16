//! Local, privacy-filtered importer for Claude Code JSONL session archives.
//!
//! Only visible user/assistant text is indexed. Tool payloads and hidden
//! reasoning blocks are excluded, common secrets are redacted, and opaque IDs
//! replace project paths. No embedding provider is called.

use crate::ingest::session_safety::{redact_secrets, stable_source_uuid};
use crate::ingest::{IngestEngine, IngestReport};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{info, warn};
use walkdir::WalkDir;

const MAX_TRANSCRIPT_CHARS: usize = 1_000_000;

#[derive(Debug, Clone)]
struct ArchiveFile {
    relative_path: PathBuf,
    artifact_id: String,
    modified_at: DateTime<Utc>,
    checksum: String,
    parsed: Transcript,
}

#[derive(Debug, Default, Clone)]
struct Transcript {
    session_id: Option<String>,
    agent_id: Option<String>,
    timestamp: Option<DateTime<Utc>>,
    message_count: usize,
    tool_call_count: usize,
    text: String,
    redaction_count: usize,
    truncated: bool,
}

#[derive(Debug, Default, Clone)]
pub struct ClaudeSessionSummary {
    pub scanned: usize,
    pub imported: usize,
    pub skipped_unchanged: usize,
    pub sessions_reconciled: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, serde::Serialize)]
pub struct ClaudeInventory {
    pub source_file_count: usize,
    pub database_document_count: usize,
    pub missing_artifact_ids: Vec<String>,
    pub extra_artifact_ids: Vec<String>,
    pub changed_artifact_ids: Vec<String>,
    pub missing_session_artifact_ids: Vec<String>,
    pub extra_session_artifact_ids: Vec<String>,
}

fn artifact_id(relative_path: &Path) -> String {
    stable_source_uuid("claude-archive", &relative_path.to_string_lossy())
        .simple()
        .to_string()
}

fn scan_sessions(root: &Path) -> Result<Vec<ArchiveFile>> {
    if !root.is_dir() {
        anyhow::bail!("Claude projects directory not found");
    }
    let mut files = Vec::new();
    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(error) => {
                warn!("Skipping unreadable Claude archive entry: {error}");
                None
            }
        })
    {
        let path = entry.path();
        if !entry.file_type().is_file()
            || path.extension().and_then(|ext| ext.to_str()) != Some("jsonl")
            || path.file_name().and_then(|name| name.to_str()) == Some("journal.jsonl")
        {
            continue;
        }
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(_) => {
                warn!("Skipping unreadable Claude archive file");
                continue;
            }
        };
        let source = match std::str::from_utf8(&bytes) {
            Ok(source) => source,
            Err(_) => {
                warn!("Skipping non-UTF8 Claude archive file");
                continue;
            }
        };
        let relative_path = path.strip_prefix(root).unwrap_or(path).to_path_buf();
        let modified_at = fs::metadata(path)
            .ok()
            .and_then(|meta| meta.modified().ok())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(Utc::now);
        files.push(ArchiveFile {
            artifact_id: artifact_id(&relative_path),
            relative_path,
            modified_at,
            checksum: hex::encode(Sha256::digest(&bytes)),
            parsed: parse_transcript(source),
        });
    }
    files.sort_by(|left, right| left.relative_path.cmp(&right.relative_path));
    Ok(files)
}

fn parse_transcript(source: &str) -> Transcript {
    let mut result = Transcript::default();
    for line in source.lines() {
        let record: Value = match serde_json::from_str(line) {
            Ok(record) => record,
            Err(_) => continue,
        };
        if result.session_id.is_none() {
            result.session_id = record
                .get("sessionId")
                .or_else(|| record.get("session_id"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
        }
        if result.agent_id.is_none() {
            result.agent_id = record
                .get("agentId")
                .or_else(|| record.get("agent_id"))
                .and_then(Value::as_str)
                .map(ToOwned::to_owned);
        }
        if result.timestamp.is_none() {
            result.timestamp = record
                .get("timestamp")
                .and_then(Value::as_str)
                .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
                .map(|value| value.with_timezone(&Utc));
        }
        let record_type = record.get("type").and_then(Value::as_str).unwrap_or("");
        if record_type == "assistant" {
            result.tool_call_count += record
                .get("message")
                .and_then(|message| message.get("content"))
                .and_then(Value::as_array)
                .map(|blocks| {
                    blocks
                        .iter()
                        .filter(|block| {
                            block.get("type").and_then(Value::as_str) == Some("tool_use")
                        })
                        .count()
                })
                .unwrap_or(0);
        }
        if !matches!(record_type, "user" | "assistant")
            || record.get("isMeta").and_then(Value::as_bool) == Some(true)
        {
            continue;
        }
        let Some(message) = record.get("message") else {
            continue;
        };
        if message.get("isMeta").and_then(Value::as_bool) == Some(true) {
            continue;
        }
        let role = message
            .get("role")
            .and_then(Value::as_str)
            .unwrap_or(record_type);
        if !matches!(role, "user" | "assistant") {
            continue;
        }
        let mut visible = Vec::new();
        match message.get("content") {
            Some(Value::String(text)) => visible.push(text.as_str()),
            Some(Value::Array(blocks)) => {
                for block in blocks {
                    if block.get("type").and_then(Value::as_str) == Some("text") {
                        if let Some(text) = block.get("text").and_then(Value::as_str) {
                            visible.push(text);
                        }
                    }
                }
            }
            _ => {}
        }
        for text in visible.into_iter().filter(|text| !text.trim().is_empty()) {
            let current = result.text.chars().count();
            if current >= MAX_TRANSCRIPT_CHARS {
                result.truncated = true;
                break;
            }
            let (safe_text, count) = redact_secrets(text);
            result.redaction_count += count;
            let safe_text = truncate(&safe_text, MAX_TRANSCRIPT_CHARS - current);
            result.text.push_str(role);
            result.text.push_str(":\n");
            result.text.push_str(&safe_text);
            result.text.push_str("\n\n");
            result.message_count += 1;
            if result.text.chars().count() >= MAX_TRANSCRIPT_CHARS {
                result.truncated = true;
                break;
            }
        }
    }
    result
}

/// Read-only archive-vs-database fingerprint report.
pub async fn inventory_claude_sessions(
    engine: &IngestEngine,
    sessions_root: &Path,
) -> Result<ClaudeInventory> {
    let files = scan_sessions(sessions_root)?;
    let local: HashMap<String, String> = files
        .iter()
        .map(|file| (file.artifact_id.clone(), file.checksum.clone()))
        .collect();
    let database: HashMap<String, Option<String>> = sqlx::query_as::<_, (String, Option<String>)>(
        "SELECT substring(path from 10), checksum FROM documents WHERE path LIKE 'claude://%'",
    )
    .fetch_all(engine.pool())
    .await?
    .into_iter()
    .collect();
    let mut missing_artifact_ids: Vec<_> = local
        .keys()
        .filter(|id| !database.contains_key(*id))
        .cloned()
        .collect();
    let mut changed_artifact_ids: Vec<_> = local
        .iter()
        .filter_map(|(id, checksum)| {
            database
                .get(id)
                .filter(|stored| stored.as_deref() != Some(checksum.as_str()))
                .map(|_| id.clone())
        })
        .collect();
    let mut extra_artifact_ids: Vec<_> = database
        .keys()
        .filter(|id| !local.contains_key(*id))
        .cloned()
        .collect();
    let stored_session_keys: std::collections::HashSet<String> = sqlx::query_scalar(
        "SELECT DISTINCT substring(source_key from 8) FROM sessions WHERE source_key LIKE 'claude:%'",
    )
    .fetch_all(engine.pool())
    .await?
    .into_iter()
    .collect();
    let mut missing_session_artifact_ids: Vec<_> = local
        .keys()
        .filter(|id| !stored_session_keys.contains(*id))
        .cloned()
        .collect();
    let mut extra_session_artifact_ids: Vec<_> = stored_session_keys
        .iter()
        .filter(|id| !local.contains_key(*id))
        .cloned()
        .collect();
    missing_artifact_ids.sort();
    changed_artifact_ids.sort();
    extra_artifact_ids.sort();
    missing_session_artifact_ids.sort();
    extra_session_artifact_ids.sort();
    Ok(ClaudeInventory {
        source_file_count: files.len(),
        database_document_count: database.len(),
        missing_artifact_ids,
        extra_artifact_ids,
        changed_artifact_ids,
        missing_session_artifact_ids,
        extra_session_artifact_ids,
    })
}

/// Import local Claude Code archives after a CLI sync-capture safety preflight.
pub async fn ingest_claude_sessions(
    engine: &IngestEngine,
    sessions_root: &Path,
    report: &mut IngestReport,
) -> Result<ClaudeSessionSummary> {
    info!("Ingesting local Claude Code session archives");
    let files = scan_sessions(sessions_root)?;
    let mut summary = ClaudeSessionSummary {
        scanned: files.len(),
        ..Default::default()
    };
    for file in files {
        let source_key = format!("claude:{}", file.artifact_id);
        let doc_path = format!("claude://{}", file.artifact_id);
        let title = format!("Claude session {}", file.artifact_id);
        let started_at = file.parsed.timestamp.unwrap_or(file.modified_at);
        let meta = serde_json::json!({
            "source": "claude-session",
            "artifact_id": file.artifact_id,
            "session_id": file.parsed.session_id,
            "agent_id": file.parsed.agent_id,
            "source_checksum": file.checksum,
            "source_last_modified_at": file.modified_at.to_rfc3339(),
            "message_count": file.parsed.message_count,
            "tool_call_count": file.parsed.tool_call_count,
            "redaction_count": file.parsed.redaction_count,
            "transcript_truncated": file.parsed.truncated,
        });
        let stored_checksum: Option<String> =
            sqlx::query_scalar("SELECT checksum FROM documents WHERE path = $1 LIMIT 1")
                .bind(&doc_path)
                .fetch_optional(engine.pool())
                .await
                .with_context(|| format!("checking Claude artifact {}", file.artifact_id))?;
        let changed = stored_checksum.as_deref() != Some(file.checksum.as_str());
        if engine
            .upsert_source_session(
                &source_key,
                &title,
                "completed",
                Some("Imported from local Claude Code archive"),
                started_at,
                Some(file.modified_at),
            )
            .await
            .is_err()
        {
            summary
                .errors
                .push(format!("session row failed for {}", file.artifact_id));
            warn!(artifact_id = %file.artifact_id, "Claude session row upsert failed");
            continue;
        }
        summary.sessions_reconciled += 1;
        if !changed {
            summary.skipped_unchanged += 1;
            continue;
        }
        let body = if file.parsed.text.trim().is_empty() {
            "Claude archive contains no visible user or assistant text.".to_string()
        } else {
            file.parsed.text.clone()
        };
        if engine
            .insert_document(
                &doc_path,
                Some("claude-session"),
                Some(&title),
                &body,
                Some(&file.checksum),
                &meta,
                Some(body.len().min(i32::MAX as usize) as i32),
                Some(file.modified_at),
            )
            .await
            .is_err()
        {
            summary
                .errors
                .push(format!("document write failed for {}", file.artifact_id));
            warn!(artifact_id = %file.artifact_id, "Claude archive document upsert failed");
            continue;
        }
        summary.imported += 1;
    }
    report
        .sources_processed
        .insert("claude-sessions".to_string(), summary.scanned as u64);
    report.documents_created += summary.imported as u64;
    report.sessions_created += summary.sessions_reconciled as u64;
    report.errors += summary.errors.len() as u64;
    Ok(summary)
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_root() -> PathBuf {
        let stamp = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_nanos();
        std::env::temp_dir().join(format!("claude-fixture-{stamp}"))
    }

    #[test]
    fn parser_keeps_visible_text_and_filters_secrets_reasoning_and_tool_payloads() {
        let lines = concat!(
            "{\"type\":\"user\",\"sessionId\":\"synthetic-id\",\"message\":{\"role\":\"user\",\"content\":\"api_key=sk-test01234567890123456789\"}}\n",
            "{\"type\":\"assistant\",\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"thinking\",\"thinking\":\"HIDDEN-THOUGHT\"},{\"type\":\"tool_use\",\"input\":{\"secret\":\"TOOL-PAYLOAD\"}},{\"type\":\"text\",\"text\":\"Visible synthetic answer\"}]}}\n",
            "{\"type\":\"user\",\"message\":{\"role\":\"user\",\"content\":[{\"type\":\"tool_result\",\"content\":\"TOOL-RESULT\"}]}}\n",
            "{\"type\":\"assistant\",\"isMeta\":true,\"message\":{\"role\":\"assistant\",\"content\":[{\"type\":\"text\",\"text\":\"META-ONLY\"}]}}\n"
        );
        let parsed = parse_transcript(lines);
        assert_eq!(parsed.session_id.as_deref(), Some("synthetic-id"));
        assert_eq!(parsed.message_count, 2);
        assert_eq!(parsed.tool_call_count, 1);
        assert!(parsed.redaction_count > 0);
        assert!(parsed.text.contains("[REDACTED]"));
        assert!(parsed.text.contains("Visible synthetic answer"));
        for excluded in [
            "sk-test01234567890123456789",
            "HIDDEN-THOUGHT",
            "TOOL-PAYLOAD",
            "TOOL-RESULT",
            "META-ONLY",
        ] {
            assert!(!parsed.text.contains(excluded));
        }
    }

    #[test]
    fn scanner_includes_agent_archives_excludes_journal_and_hashes_project_paths() {
        let root = temp_root();
        fs::create_dir_all(root.join("project/subagents")).unwrap();
        fs::write(root.join("project/main.jsonl"), "{\"type\":\"user\",\"sessionId\":\"main\",\"message\":{\"role\":\"user\",\"content\":\"fixture\"}}\n").unwrap();
        fs::write(root.join("project/subagents/agent-a.jsonl"), "{\"type\":\"assistant\",\"sessionId\":\"main\",\"agentId\":\"agent-a\",\"message\":{\"role\":\"assistant\",\"content\":\"fixture answer\"}}\n").unwrap();
        fs::write(root.join("project/journal.jsonl"), "synthetic journal\n").unwrap();
        let files = scan_sessions(&root).unwrap();
        assert_eq!(files.len(), 2);
        assert!(files.iter().all(|file| file.artifact_id.len() == 32));
        assert!(files
            .iter()
            .all(|file| !file.artifact_id.contains("project")));
        assert!(files
            .iter()
            .any(|file| file.parsed.agent_id.as_deref() == Some("agent-a")));
        fs::remove_dir_all(root).unwrap();
    }
}

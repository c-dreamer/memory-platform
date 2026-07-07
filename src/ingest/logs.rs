//! OpenCode log ingestion — parses `opencode.log` for session activity,
//! model usage, tool invocations, and errors, and stores them as experiences.

use crate::ingest::{IngestEngine, IngestReport};
use anyhow::{Context, Result};
use chrono::Utc;
use regex::Regex;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::fs;
use std::path::Path;
use tracing::info;

/// Ingest OpenCode log file, extracting session activity, model usage, and errors.
pub async fn ingest_logs(
    engine: &IngestEngine,
    log_path: &Path,
    report: &mut IngestReport,
) -> Result<()> {
    info!("Ingesting OpenCode logs from: {}", log_path.display());

    if !log_path.exists() {
        anyhow::bail!("Log file not found: {}", log_path.display());
    }

    let content = fs::read_to_string(log_path).context("Failed to read opencode.log")?;
    let modified_at = fs::metadata(log_path)
        .ok()
        .and_then(|m| m.modified().ok())
        .map(chrono::DateTime::<Utc>::from)
        .unwrap_or_else(Utc::now);
    let imported_at = Utc::now();

    let lines: Vec<&str> = content.lines().collect();
    info!("Log file has {} lines", lines.len());

    // --- Extract session IDs, model IDs, agents, and errors ---
    let session_re = Regex::new(r"session\.id=(ses_\w+)").expect("Invalid session ID regex");
    let model_re = Regex::new(r"modelID=([\w.-]+)").expect("Invalid model ID regex");
    let agent_re = Regex::new(r#"agent="([^"]+)""#).expect("Invalid agent regex");
    let error_re = Regex::new(r"\blevel=ERROR\b").expect("Invalid error level regex");

    let mut sessions_found = HashSet::new();
    let mut models_found = HashSet::new();
    let mut agents_found = HashSet::new();
    let mut errors_found: u64 = 0;

    for line in &lines {
        if let Some(caps) = session_re.captures(line) {
            sessions_found.insert(caps[1].to_string());
        }
        if let Some(caps) = model_re.captures(line) {
            models_found.insert(caps[1].to_string());
        }
        if let Some(caps) = agent_re.captures(line) {
            agents_found.insert(caps[1].to_string());
        }
        if error_re.is_match(line) {
            errors_found += 1;
        }
    }

    info!(
        "Log summary: {} sessions, {} models, {} agents, {} errors",
        sessions_found.len(),
        models_found.len(),
        agents_found.len(),
        errors_found
    );

    // --- Store log analysis as an experience ---
    let models_list: Vec<String> = models_found.iter().cloned().collect();
    let agents_list: Vec<String> = agents_found.iter().cloned().collect();
    let session_count = sessions_found.len();

    let analysis = format!(
        "OpenCode Log Analysis\n\
         Total log lines: {}\n\
         Unique sessions: {}\n\
         Models used: {}\n\
         Agents used: {}\n\
         Errors found: {}\n\n\
         Models: {}\n\
         Agents: {}",
        lines.len(),
        session_count,
        models_found.len(),
        agents_found.len(),
        errors_found,
        models_list.join(", "),
        agents_list.join(", "),
    );

    let checksum = format!("{:x}", Sha256::digest(content.as_bytes()));

    // Store as document
    engine
        .insert_document(
            "log://opencode.log",
            Some("logs"),
            Some("OpenCode Runtime Log"),
            &content,
            Some(&checksum),
            &serde_json::json!({
                "source": "opencode-log",
                "path": log_path.to_string_lossy(),
                "file_type": "log",
                "line_count": lines.len(),
                "session_count": session_count,
                "error_count": errors_found,
                "source_last_modified_at": modified_at.to_rfc3339(),
                "source_age_seconds": imported_at.signed_duration_since(modified_at).num_seconds().max(0),
                "imported_at": imported_at.to_rfc3339(),
            }),
            Some(content.len() as i32),
            None,
        )
        .await
        .context("Failed to insert log document")?;
    report.documents_created += 1;

    // Store as experience
    let tags: Vec<String> = vec!["opencode-log".to_string(), "analysis".to_string()];
    engine
        .insert_experience(
            None,
            "OpenCode Log Analysis",
            Some(&analysis),
            Some(&format!(
                "{} lines analyzed, {} sessions, {} models, {} errors",
                lines.len(),
                session_count,
                models_found.len(),
                errors_found,
            )),
            None,
            &tags,
            None,
            Some("opencode"),
        )
        .await
        .context("Failed to insert log experience")?;
    report.experiences_created += 1;

    report
        .sources_processed
        .insert("opencode-log".to_string(), lines.len() as u64);

    info!("Log ingestion complete");
    Ok(())
}

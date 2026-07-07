//! OpenCode session ingestion — reads `opencode.db` SQLite via Python extraction script
//! and stores each session + its message parts as experiences / memories.

use crate::ingest::{IngestEngine, IngestReport};
use anyhow::{Context, Result};
use chrono::Utc;
use serde_json::Value;
use std::io::BufRead;
use std::path::Path;
use std::process::{Command, Stdio};
use tracing::{info, warn};

/// All sessions recognized during ingestion (used for dedup across runs).
pub struct SessionBatch {
    pub total: u64,
    pub skipped: u64,
    pub new: u64,
}

/// Find the extraction script relative to the project root.
fn find_script() -> Result<String> {
    let candidates = vec![
        "scripts/extract_sessions.py",
        "../scripts/extract_sessions.py",
        "/home/humanoracle26/memory-platform-rust/scripts/extract_sessions.py",
    ];

    for c in &candidates {
        if Path::new(c).exists() {
            return Ok(c.to_string());
        }
    }

    anyhow::bail!(
        "extract_sessions.py not found in expected locations: {:?}",
        candidates
    );
}

/// Ingest all OpenCode sessions by shelling out to Python for SQLite extraction.
pub async fn ingest_sessions(
    engine: &IngestEngine,
    opencode_db_path: &Path,
    report: &mut IngestReport,
) -> Result<SessionBatch> {
    info!(
        "Extracting sessions from: {} (via Python script)",
        opencode_db_path.display()
    );

    if !opencode_db_path.exists() {
        anyhow::bail!("Session DB not found: {}", opencode_db_path.display());
    }

    let script_path = find_script()?;

    // Run the Python extraction script
    let mut child = Command::new("python3")
        .arg(&script_path)
        .arg(opencode_db_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .context("Failed to spawn Python extraction script")?;

    let stdout = child
        .stdout
        .take()
        .context("No stdout from Python extraction script")?;

    let reader = std::io::BufReader::new(stdout);

    let mut batch = SessionBatch {
        total: 0,
        skipped: 0,
        new: 0,
    };

    let mut current_session: Option<(Value, Vec<Value>, Vec<Value>)> = None;

    for line in reader.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }

        let parsed: Value = serde_json::from_str(&line)
            .with_context(|| format!("Failed to parse JSON: {}", &line[..line.len().min(100)]))?;

        let record_type = parsed["type"].as_str().unwrap_or("").to_string();
        let data = &parsed["data"];

        match record_type.as_str() {
            "session" => {
                if let Some((ses_data, parts, todos)) = current_session.take() {
                    process_session(engine, &mut batch, ses_data, parts, todos).await;
                }
                current_session = Some((data.clone(), Vec::new(), Vec::new()));
                batch.total += 1;
            }
            "part" => {
                if let Some((_, ref mut parts, _)) = current_session {
                    parts.push(data.clone());
                }
            }
            "todo" => {
                if let Some((_, _, ref mut todos)) = current_session {
                    todos.push(data.clone());
                }
            }
            "metadata" => {
                info!(
                    "Extraction: {} sessions, {} parts, {} todos",
                    data["total_sessions"].as_u64().unwrap_or(0),
                    data["total_parts"].as_u64().unwrap_or(0),
                    data["total_todos"].as_u64().unwrap_or(0),
                );
            }
            _ => info!("Unknown record type: {record_type}"),
        }
    }

    // Process last session
    if let Some((ses_data, parts, todos)) = current_session.take() {
        process_session(engine, &mut batch, ses_data, parts, todos).await;
    }

    // Wait for Python process
    let status = child
        .wait()
        .context("Failed to wait for Python extraction script")?;

    if !status.success() {
        let stderr = child
            .stderr
            .take()
            .map(|s| {
                let mut buf = String::new();
                std::io::BufReader::new(s).read_line(&mut buf).ok();
                buf
            })
            .unwrap_or_default();
        warn!("Python script exited with error: {status}\n{stderr}");
    }

    report
        .sources_processed
        .insert("opencode-sessions".to_string(), batch.total);
    report.memories_created += batch.new;
    report.experiences_created += batch.new;
    report.sessions_created += batch.new;

    info!(
        "Session ingestion complete: {} total, {} new, {} skipped",
        batch.total, batch.new, batch.skipped
    );

    Ok(batch)
}

/// Process a single session: dedup check, insert session + experience + memories.
async fn process_session(
    engine: &IngestEngine,
    batch: &mut SessionBatch,
    ses_data: Value,
    parts: Vec<Value>,
    _todos: Vec<Value>,
) {
    let ses_id = ses_data["id"].as_str().unwrap_or("unknown");

    match engine.session_already_ingested(ses_id).await {
        Ok(true) => {
            batch.skipped += 1;
            return;
        }
        Ok(false) => {}
        Err(e) => warn!("  Dedup check error for {ses_id}: {e}"),
    }

    let title = ses_data["title"].as_str().unwrap_or("Untitled");
    let agent_name = ses_data["agent_name"].as_str().unwrap_or("unknown");
    let model_name = ses_data["model_name"].as_str().unwrap_or("unknown");
    let tokens_in = ses_data["tokens_input"].as_i64().unwrap_or(0);
    let tokens_out = ses_data["tokens_output"].as_i64().unwrap_or(0);
    let cost = ses_data["cost"].as_f64().unwrap_or(0.0);

    let created_iso = ses_data["time_created_iso"].as_str().unwrap_or("");
    let updated_iso = ses_data["time_updated_iso"].as_str().unwrap_or("");
    let created_dt = created_iso
        .parse::<chrono::DateTime<Utc>>()
        .unwrap_or(Utc::now());
    let updated_dt = updated_iso
        .parse::<chrono::DateTime<Utc>>()
        .unwrap_or(Utc::now());
    let duration_secs = (updated_dt - created_dt).num_seconds() as i32;
    let imported_at = Utc::now();
    let source_age_seconds = imported_at
        .signed_duration_since(updated_dt)
        .num_seconds()
        .max(0);

    // Extract user messages
    let user_texts: Vec<String> = parts
        .iter()
        .filter(|p| {
            p["role"].as_str() == Some("user")
                && p["parsed_data"]["text"]
                    .as_str()
                    .map_or(false, |t| t.len() > 50)
        })
        .map(|p| p["parsed_data"]["text"].as_str().unwrap_or("").to_string())
        .collect();

    // Insert session record
    let mem_session_id = engine
        .insert_session(title, "completed", None, created_dt, Some(updated_dt))
        .await
        .unwrap_or_else(|e| {
            warn!("  Failed to insert session '{title}': {e}");
            uuid::Uuid::nil()
        });

    let mem_session_opt = if mem_session_id.is_nil() {
        None
    } else {
        Some(mem_session_id)
    };

    let meta = serde_json::json!({
        "opencode_session_id": ses_id,
        "source": "opencode-session",
        "agent": agent_name,
        "model": model_name,
        "tokens_input": tokens_in,
        "tokens_output": tokens_out,
        "cost": cost,
        "source_created_at": created_dt.to_rfc3339(),
        "source_updated_at": updated_dt.to_rfc3339(),
        "source_age_seconds": source_age_seconds,
        "imported_at": imported_at.to_rfc3339(),
    });

    let tags: Vec<String> = vec![
        agent_name.to_string(),
        model_name.to_string(),
        "opencode-session".to_string(),
    ];

    // Insert experience
    let result_summary = format!(
        "Session: {title} | Agent: {agent_name} | Model: {model_name} | \
         Tokens: {tokens_in} in / {tokens_out} out | Cost: ${cost:.4} | \
         Messages: {} | User prompts: {}",
        parts.len(),
        user_texts.len(),
    );

    engine
        .insert_experience(
            mem_session_opt,
            title,
            None,
            Some(&result_summary),
            None,
            &tags,
            Some(duration_secs),
            Some("opencode"),
        )
        .await
        .unwrap_or_else(|e| {
            warn!("  Failed to insert experience for '{title}': {e}");
            uuid::Uuid::nil()
        });

    // Store user messages as memories
    for text in &user_texts {
        let mem_tags: Vec<String> = vec![
            agent_name.to_string(),
            model_name.to_string(),
            "user-prompt".to_string(),
        ];
        engine
            .insert_memory(mem_session_opt, text, "conversation", 0.4, &mem_tags, &meta)
            .await
            .unwrap_or_else(|e| {
                warn!("  Failed to insert user memory: {e}");
                uuid::Uuid::nil()
            });
    }

    // Store session summary as a memory
    let session_summary = format!(
        "OpenCode session: {title}\nAgent: {agent_name}\nModel: {model_name}\n\
         Messages: {msg_count}\nUser prompts: {user_count}\n\
         Tokens: {tokens_in} in / {tokens_out} out\nCost: ${cost:.4}",
        msg_count = parts.len(),
        user_count = user_texts.len(),
    );

    let sum_tags: Vec<String> = vec![
        agent_name.to_string(),
        model_name.to_string(),
        "session-summary".to_string(),
    ];

    engine
        .insert_memory(
            mem_session_opt,
            &session_summary,
            "session",
            0.6,
            &sum_tags,
            &meta,
        )
        .await
        .unwrap_or_else(|e| {
            warn!("  Failed to insert session memory for '{title}': {e}");
            uuid::Uuid::nil()
        });

    batch.new += 1;

    if batch.total % 20 == 0 {
        info!(
            "  Progress: {} total, {} new, {} skipped",
            batch.total, batch.new, batch.skipped
        );
    }
}

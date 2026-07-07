//! OpenCode configuration and rule ingestion.
//!
//! Reads:
//! - `opencode.jsonc` — main OpenCode configuration
//! - `rules/` — rule files (coding-style, patterns, testing, git-workflow, security)
//! - `skills/` — skill markdown files
//! - `oh-my-openagent.jsonc` — agent configuration

use crate::ingest::{IngestEngine, IngestReport};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use tracing::{info, warn};

/// Ingest OpenCode configuration files, rules, and skills.
pub async fn ingest_config(
    engine: &IngestEngine,
    config_dir: &Path,
    report: &mut IngestReport,
) -> Result<()> {
    info!("Ingesting OpenCode config from: {}", config_dir.display());

    if !config_dir.is_dir() {
        anyhow::bail!("Config directory not found: {}", config_dir.display());
    }

    let opencode_jsonc = config_dir.join("opencode.jsonc");
    let ohmy_jsonc = config_dir.join("oh-my-openagent.jsonc");
    let rules_dir = config_dir.join("rules");
    let skills_dir = config_dir.join("skills");

    // 1. Ingest opencode.jsonc
    if opencode_jsonc.exists() {
        let content =
            fs::read_to_string(&opencode_jsonc).context("Failed to read opencode.jsonc")?;
        let modified_at = fs::metadata(&opencode_jsonc)
            .ok()
            .and_then(|m| m.modified().ok())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(Utc::now);
        let imported_at = Utc::now();

        let checksum = format!("{:x}", Sha256::digest(content.as_bytes()));
        let meta = serde_json::json!({
            "source": "opencode-config",
            "path": opencode_jsonc.to_string_lossy(),
            "file_type": "jsonc",
            "source_last_modified_at": modified_at.to_rfc3339(),
            "source_age_seconds": imported_at.signed_duration_since(modified_at).num_seconds().max(0),
            "imported_at": imported_at.to_rfc3339(),
        });

        // Store as document (path = config://opencode.jsonc)
        engine
            .insert_document(
                "config://opencode.jsonc",
                Some("config"),
                Some("OpenCode Main Configuration"),
                &content,
                Some(&checksum),
                &meta,
                Some(content.len() as i32),
                None,
            )
            .await
            .context("Failed to insert opencode.jsonc as document")?;
        report.documents_created += 1;

        // Also store as a memory with key details extracted
        let config_summary = summarize_config(&content);
        let tags = vec!["opencode-config".to_string(), "configuration".to_string()];
        engine
            .insert_memory(None, &config_summary, "config", 0.7, &tags, &meta)
            .await
            .context("Failed to insert config summary memory")?;
        report.memories_created += 1;

        report
            .sources_processed
            .entry("opencode.jsonc".to_string())
            .and_modify(|c| *c += 1)
            .or_insert(1);
        info!("  Ingested opencode.jsonc ({} bytes)", content.len());
    } else {
        warn!("opencode.jsonc not found at {}", opencode_jsonc.display());
    }

    // 2. Ingest oh-my-openagent.jsonc
    if ohmy_jsonc.exists() {
        let content =
            fs::read_to_string(&ohmy_jsonc).context("Failed to read oh-my-openagent.jsonc")?;
        let modified_at = fs::metadata(&ohmy_jsonc)
            .ok()
            .and_then(|m| m.modified().ok())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(Utc::now);
        let imported_at = Utc::now();

        let checksum = format!("{:x}", Sha256::digest(content.as_bytes()));
        let meta = serde_json::json!({
            "source": "opencode-agent-config",
            "path": ohmy_jsonc.to_string_lossy(),
            "file_type": "jsonc",
            "source_last_modified_at": modified_at.to_rfc3339(),
            "source_age_seconds": imported_at.signed_duration_since(modified_at).num_seconds().max(0),
            "imported_at": imported_at.to_rfc3339(),
        });

        engine
            .insert_document(
                "config://oh-my-openagent.jsonc",
                Some("config"),
                Some("OhMyOpenAgent Configuration"),
                &content,
                Some(&checksum),
                &meta,
                Some(content.len() as i32),
                None,
            )
            .await
            .context("Failed to insert oh-my-openagent.jsonc")?;
        report.documents_created += 1;

        let tags = vec!["opencode-config".to_string(), "agents".to_string()];
        engine
            .insert_memory(
                None,
                &format!("OpenCode agent configuration ({} bytes)", content.len()),
                "config",
                0.6,
                &tags,
                &meta,
            )
            .await
            .context("Failed to insert agent config memory")?;
        report.memories_created += 1;

        report
            .sources_processed
            .entry("oh-my-openagent.jsonc".to_string())
            .and_modify(|c| *c += 1)
            .or_insert(1);
        info!("  Ingested oh-my-openagent.jsonc ({} bytes)", content.len());
    }

    // 3. Ingest rule files
    if rules_dir.is_dir() {
        let entries = fs::read_dir(&rules_dir).context("Failed to read rules directory")?;

        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let content = fs::read_to_string(&path)
                .with_context(|| format!("Failed to read rule file: {}", path.display()))?;
            let modified_at = fs::metadata(&path)
                .ok()
                .and_then(|m| m.modified().ok())
                .map(DateTime::<Utc>::from)
                .unwrap_or_else(Utc::now);
            let imported_at = Utc::now();

            let file_name = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("unknown");

            let rel_path = format!("rules://{file_name}");
            let checksum = format!("{:x}", Sha256::digest(content.as_bytes()));
            let parent_dir = path
                .parent()
                .and_then(|p| p.file_name())
                .and_then(|n| n.to_str())
                .unwrap_or("common");

            let meta = serde_json::json!({
                "source": "opencode-rule",
                "path": path.to_string_lossy(),
                "file_type": "md",
                "category": parent_dir,
                "source_last_modified_at": modified_at.to_rfc3339(),
                "source_age_seconds": imported_at.signed_duration_since(modified_at).num_seconds().max(0),
                "imported_at": imported_at.to_rfc3339(),
            });

            engine
                .insert_document(
                    &rel_path,
                    Some("rules"),
                    Some(file_name),
                    &content,
                    Some(&checksum),
                    &meta,
                    Some(content.len() as i32),
                    None,
                )
                .await
                .context("Failed to insert rule document")?;
            report.documents_created += 1;

            report
                .sources_processed
                .entry("rules".to_string())
                .and_modify(|c| *c += 1)
                .or_insert(1);
            info!("  Ingested rule: {file_name}");
        }
    }

    // 4. Ingest skill files
    if skills_dir.is_dir() {
        ingest_skills_recursive(engine, &skills_dir, report)
            .await
            .context("Failed to ingest skills")?;
    }

    info!("Config ingestion complete");
    Ok(())
}

/// Recursively ingest skill files from a directory.
async fn ingest_skills_recursive(
    engine: &IngestEngine,
    dir: &Path,
    report: &mut IngestReport,
) -> Result<()> {
    let entries = fs::read_dir(dir)
        .with_context(|| format!("Failed to read skills directory: {}", dir.display()))?;

    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            Box::pin(ingest_skills_recursive(engine, &path, report)).await?;
            continue;
        }

        if !path.is_file() {
            continue;
        }

        let content = fs::read_to_string(&path)
            .with_context(|| format!("Failed to read skill file: {}", path.display()))?;
        let modified_at = fs::metadata(&path)
            .ok()
            .and_then(|m| m.modified().ok())
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(Utc::now);
        let imported_at = Utc::now();

        let rel_path = path
            .strip_prefix("/")
            .unwrap_or(&path)
            .to_string_lossy()
            .to_string();

        let rel_key = format!("skill://{}", rel_path);
        let checksum = format!("{:x}", Sha256::digest(content.as_bytes()));
        let file_name = path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let meta = serde_json::json!({
            "source": "opencode-skill",
            "path": path.to_string_lossy(),
            "file_type": "md",
            "source_last_modified_at": modified_at.to_rfc3339(),
            "source_age_seconds": imported_at.signed_duration_since(modified_at).num_seconds().max(0),
            "imported_at": imported_at.to_rfc3339(),
        });

        engine
            .insert_document(
                &rel_key,
                Some("skills"),
                Some(file_name),
                &content,
                Some(&checksum),
                &meta,
                Some(content.len() as i32),
                None,
            )
            .await
            .context("Failed to insert skill document")?;
        report.documents_created += 1;

        report
            .sources_processed
            .entry("skills".to_string())
            .and_modify(|c| *c += 1)
            .or_insert(1);
        info!("  Ingested skill: {file_name}");
    }

    Ok(())
}

/// Extract a human-readable summary from opencode.jsonc for memory storage.
fn summarize_config(content: &str) -> String {
    let mut agents = 0u32;
    let mut commands = 0u32;
    let mut mcps = 0u32;
    let mut rules = 0u32;

    for line in content.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with('"') || trimmed.starts_with('\'') {
            // Count top-level keys
            if trimmed.contains("\"agent\"") || trimmed.contains("'agent'") {
                // skip, this is the key
            }
        }
        if trimmed.contains("\"description\"") || trimmed.contains("'description'") {
            // not what we want to count
        }
    }

    // Simpler heuristic: count sections
    if let Ok(v) = serde_json::from_str::<serde_json::Value>(content) {
        if let Some(agent_obj) = v.get("agent").and_then(|a| a.as_object()) {
            agents = agent_obj.len() as u32;
        }
        if let Some(cmd_obj) = v.get("command").and_then(|c| c.as_object()) {
            commands = cmd_obj.len() as u32;
        }
        if let Some(mcp_obj) = v.get("mcp").and_then(|m| m.as_object()) {
            mcps = mcp_obj.len() as u32;
        }
        if let Some(inst) = v.get("instructions").and_then(|i| i.as_array()) {
            rules = inst.len() as u32;
        }
    }

    format!(
        "OpenCode Configuration: {agents} agents, {commands} commands, \
         {mcps} MCP servers, {rules} rule files"
    )
}

//! Vault ingestion service.
//!
//! Walks an Obsidian vault directory, parses markdown files, chunks them,
//! generates embeddings, and upserts documents to the database.

use std::path::Path;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use chrono::Utc;
use pulldown_cmark::{Event, HeadingLevel, Parser, Tag, TagEnd};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use walkdir::WalkDir;

use crate::db::postgres::PostgresDb;
use crate::models::Document;
use crate::services::embedding::EmbeddingService;

/// Report produced after an ingestion run.
#[derive(Debug, Clone)]
pub struct IngestionReport {
    pub files_scanned: u64,
    pub files_ingested: u64,
    pub chunks_created: u64,
    pub errors: Vec<String>,
    pub duration_ms: u64,
}

/// Service for ingesting Obsidian vault markdown files into the database.
pub struct IngestionService {
    db: PostgresDb,
    embedder: Arc<dyn EmbeddingService>,
}

impl IngestionService {
    /// Create a new ingestion service with a database pool and embedding backend.
    #[must_use]
    pub fn new(pool: PgPool, embedder: Arc<dyn EmbeddingService>) -> Self {
        Self {
            db: PostgresDb { pool },
            embedder,
        }
    }

    /// Walk an Obsidian vault directory, parse all `.md` files, chunk, embed, and upsert.
    pub async fn ingest_vault(&self, vault_path: &Path, agent_id: &str) -> Result<IngestionReport> {
        let start = Instant::now();
        let mut files_scanned: u64 = 0;
        let mut files_ingested: u64 = 0;
        let mut chunks_created: u64 = 0;
        let mut errors: Vec<String> = Vec::new();

        for entry in WalkDir::new(vault_path)
            .follow_links(false)
            .into_iter()
            .filter_map(|e| e.ok())
        {
            if !entry.file_type().is_file() {
                continue;
            }
            let path = entry.path();
            if path.extension().map_or(true, |ext| ext != "md") {
                continue;
            }
            files_scanned += 1;

            match self.ingest_file(path, vault_path, agent_id).await {
                Ok(chunk_count) => {
                    files_ingested += 1;
                    chunks_created += chunk_count;
                }
                Err(e) => {
                    errors.push(format!("{}: {:#}", path.display(), e));
                }
            }
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(IngestionReport {
            files_scanned,
            files_ingested,
            chunks_created,
            errors,
            duration_ms,
        })
    }

    /// Ingest a single markdown file: parse frontmatter, chunk by headers, embed, upsert.
    ///
    /// Returns the number of chunks created.
    pub async fn ingest_file(
        &self,
        file_path: &Path,
        vault_root: &Path,
        _agent_id: &str,
    ) -> Result<u64> {
        let raw = std::fs::read_to_string(file_path)
            .with_context(|| format!("Failed to read {}", file_path.display()))?;

        let (frontmatter, body) = parse_frontmatter(&raw);
        let vault_section = relative_vault_section(file_path, vault_root);
        let file_modified = std::fs::metadata(file_path)
            .ok()
            .and_then(|m| m.modified().ok())
            .map(|t| {
                chrono::DateTime::from_timestamp(
                    t.duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs() as i64,
                    0,
                )
                .unwrap_or_else(Utc::now)
            });

        let chunks = chunk_markdown(&body);
        if chunks.is_empty() {
            // Single chunk for the whole body
            let checksum = compute_checksum(&body);
            let embedding = self.embedder.embed(&body).await?;
            let chunk_path = relative_path_str(file_path, vault_root);

            self.db
                .upsert_document(
                    &chunk_path,
                    vault_section.as_deref(),
                    None,
                    &body,
                    &checksum,
                    &frontmatter,
                    Some(body.len() as i32),
                    Some(raw.len() as i32),
                    file_modified,
                    Some(embedding.as_vec()),
                )
                .await
                .with_context(|| {
                    format!("Failed to upsert document for {}", file_path.display())
                })?;

            return Ok(1);
        }

        let mut chunk_count: u64 = 0;
        for (i, (heading, text)) in chunks.iter().enumerate() {
            let chunk_path = format!("{}#chunk_{}", relative_path_str(file_path, vault_root), i);
            let checksum = compute_checksum(text);
            let embedding = self.embedder.embed(text).await?;

            self.db
                .upsert_document(
                    &chunk_path,
                    vault_section.as_deref(),
                    Some(heading),
                    text,
                    &checksum,
                    &frontmatter,
                    Some(text.len() as i32),
                    Some(raw.len() as i32),
                    file_modified,
                    Some(embedding.as_vec()),
                )
                .await
                .with_context(|| {
                    format!("Failed to upsert chunk {} for {}", i, file_path.display())
                })?;

            chunk_count += 1;
        }

        Ok(chunk_count)
    }

    /// Re-embed all documents that have a null embedding column.
    pub async fn reindex_all(&self, _agent_id: &str) -> Result<IngestionReport> {
        let start = Instant::now();
        let files_scanned: u64;
        let mut files_ingested: u64 = 0;
        let mut chunks_created: u64 = 0;
        let mut errors: Vec<String> = Vec::new();

        // Find documents with null embedding
        let docs: Vec<Document> = sqlx::query_as::<_, Document>(
            "SELECT id, path, vault_section, title, content, checksum, frontmatter, embedding, \
                    token_count, file_size_bytes, file_modified_at, created_at, updated_at \
             FROM documents WHERE embedding IS NULL",
        )
        .fetch_all(&self.db.pool)
        .await
        .context("Failed to query documents with null embedding")?;

        files_scanned = docs.len() as u64;

        for doc in &docs {
            match self.embedder.embed(&doc.content).await {
                Ok(embedding) => {
                    let result = self
                        .db
                        .upsert_document(
                            &doc.path,
                            doc.vault_section.as_deref(),
                            doc.title.as_deref(),
                            &doc.content,
                            doc.checksum.as_deref().unwrap_or(""),
                            &doc.frontmatter,
                            doc.token_count,
                            doc.file_size_bytes,
                            doc.file_modified_at,
                            Some(embedding.as_vec()),
                        )
                        .await;

                    match result {
                        Ok(_) => {
                            files_ingested += 1;
                            chunks_created += 1;
                        }
                        Err(e) => {
                            errors.push(format!("{}: {:#}", doc.path, e));
                        }
                    }
                }
                Err(e) => {
                    errors.push(format!("{} (embed): {:#}", doc.path, e));
                }
            }
        }

        let duration_ms = start.elapsed().as_millis() as u64;

        Ok(IngestionReport {
            files_scanned,
            files_ingested,
            chunks_created,
            errors,
            duration_ms,
        })
    }
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

/// Parse YAML frontmatter delimited by `---` at the start of a markdown file.
///
/// Returns `(frontmatter_json, body_without_frontmatter)`.
fn parse_frontmatter(raw: &str) -> (serde_json::Value, String) {
    let trimmed = raw.trim_start();
    if !trimmed.starts_with("---") {
        return (serde_json::json!({}), raw.to_string());
    }

    // Find the closing `---`
    let after_first = &trimmed[3..];
    let end = after_first
        .find("\n---")
        .or_else(|| after_first.find("\r\n---"));
    let end_idx = match end {
        Some(i) => i,
        None => return (serde_json::json!({}), raw.to_string()),
    };

    let fm_text = &after_first[..end_idx];
    let body_start = end_idx + 4; // skip "\n---"
    let body = if body_start < after_first.len() {
        after_first[body_start..].to_string()
    } else {
        String::new()
    };

    let mut map = serde_json::Map::new();
    for line in fm_text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some((key, value)) = line.split_once(':') {
            let key = key.trim().trim_matches('"').trim_matches('\'');
            let value = value.trim().trim_matches('"').trim_matches('\'');
            // Try to parse as number or bool, fall back to string
            let json_val = if let Ok(n) = value.parse::<i64>() {
                serde_json::Value::Number(n.into())
            } else if let Ok(f) = value.parse::<f64>() {
                if let Some(num) = serde_json::Number::from_f64(f) {
                    serde_json::Value::Number(num)
                } else {
                    serde_json::Value::String(value.to_string())
                }
            } else if value.eq_ignore_ascii_case("true") {
                serde_json::Value::Bool(true)
            } else if value.eq_ignore_ascii_case("false") {
                serde_json::Value::Bool(false)
            } else {
                serde_json::Value::String(value.to_string())
            };
            map.insert(key.to_string(), json_val);
        }
    }

    (serde_json::Value::Object(map), body)
}

/// Chunk markdown body by `##` (H2) and `###` (H3) headers.
///
/// Returns a list of `(heading_text, chunk_content)` pairs.
fn chunk_markdown(body: &str) -> Vec<(String, String)> {
    let parser = Parser::new(body);
    let mut chunks: Vec<(String, String)> = Vec::new();
    let mut current_heading = String::new();
    let mut current_text = String::new();
    let mut in_heading = false;
    let mut heading_level: Option<HeadingLevel> = None;

    for event in parser {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                if level == HeadingLevel::H2 || level == HeadingLevel::H3 {
                    // Flush previous chunk
                    if !current_text.trim().is_empty() {
                        let heading = if current_heading.is_empty() {
                            String::from("Untitled")
                        } else {
                            current_heading.clone()
                        };
                        chunks.push((heading, current_text.trim().to_string()));
                    }
                    current_heading.clear();
                    current_text.clear();
                    in_heading = true;
                    heading_level = Some(level);
                }
            }
            Event::End(TagEnd::Heading { .. }) => {
                if heading_level == Some(HeadingLevel::H2)
                    || heading_level == Some(HeadingLevel::H3)
                {
                    in_heading = false;
                }
            }
            Event::Text(text) => {
                if in_heading {
                    current_heading.push_str(&text);
                } else {
                    current_text.push_str(&text);
                }
            }
            Event::Code(code) => {
                if !in_heading {
                    current_text.push('`');
                    current_text.push_str(&code);
                    current_text.push('`');
                }
            }
            Event::SoftBreak | Event::HardBreak => {
                if !in_heading {
                    current_text.push('\n');
                }
            }
            _ => {}
        }
    }

    // Flush final chunk
    if !current_text.trim().is_empty() {
        let heading = if current_heading.is_empty() {
            String::from("Untitled")
        } else {
            current_heading
        };
        chunks.push((heading, current_text.trim().to_string()));
    }

    chunks
}

/// Compute SHA-256 checksum of content, returned as hex string.
fn compute_checksum(content: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(content.as_bytes());
    format!("{:x}", hasher.finalize())
}

/// Derive the vault section from the relative path's first directory component.
fn relative_vault_section(file_path: &Path, vault_root: &Path) -> Option<String> {
    let rel = file_path.strip_prefix(vault_root).ok()?;
    rel.components()
        .next()
        .and_then(|c| c.as_os_str().to_str())
        .map(String::from)
}

/// Get the relative path string from a file path within the vault root.
fn relative_path_str(file_path: &Path, vault_root: &Path) -> String {
    file_path
        .strip_prefix(vault_root)
        .unwrap_or(file_path)
        .to_string_lossy()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    // ------------------------------------------------------------------
    // parse_frontmatter
    // ------------------------------------------------------------------

    #[test]
    fn parse_frontmatter_basic() {
        let input = "---\ntitle: My Note\ntags: rust, memory\n---\n\n# Heading\nContent here.";
        let (fm, body) = parse_frontmatter(input);
        assert_eq!(fm["title"], "My Note");
        assert_eq!(fm["tags"], "rust, memory");
        assert!(body.contains("# Heading"));
        assert!(body.contains("Content here."));
    }

    #[test]
    fn parse_frontmatter_numeric() {
        let input = "---\ncount: 42\nscore: 0.95\nactive: true\n---\n\nBody.";
        let (fm, _body) = parse_frontmatter(input);
        assert_eq!(fm["count"], 42);
        assert_eq!(fm["score"], 0.95);
        assert_eq!(fm["active"], true);
    }

    #[test]
    fn parse_frontmatter_no_frontmatter() {
        let input = "# Just a heading\nSome content.";
        let (fm, body) = parse_frontmatter(input);
        assert_eq!(fm, serde_json::json!({}));
        assert_eq!(body, input);
    }

    #[test]
    fn parse_frontmatter_empty() {
        let input = "---\n---\nBody.";
        let (fm, body) = parse_frontmatter(input);
        assert_eq!(fm, serde_json::json!({}));
        assert_eq!(body.trim(), "Body.");
    }

    // ------------------------------------------------------------------
    // chunk_markdown
    // ------------------------------------------------------------------

    #[test]
    fn chunk_markdown_single_section() {
        let input = "Some text without headers.";
        let chunks = chunk_markdown(input);
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].0, "Untitled");
        assert_eq!(chunks[0].1, "Some text without headers.");
    }

    #[test]
    fn chunk_markdown_h2_splits() {
        let input = "## Intro\nIntro text.\n\n## Body\nBody text.\n\n## Conclusion\nEnd text.";
        let chunks = chunk_markdown(input);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].0, "Intro");
        assert_eq!(chunks[0].1, "Intro text.");
        assert_eq!(chunks[1].0, "Body");
        assert_eq!(chunks[1].1, "Body text.");
        assert_eq!(chunks[2].0, "Conclusion");
        assert_eq!(chunks[2].1, "End text.");
    }

    #[test]
    fn chunk_markdown_h3_splits() {
        let input = "### Step 1\nFirst step.\n\n### Step 2\nSecond step.";
        let chunks = chunk_markdown(input);
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].0, "Step 1");
        assert_eq!(chunks[1].0, "Step 2");
    }

    #[test]
    fn chunk_markdown_mixed_levels() {
        let input = "## Section\nTop text.\n\n### Sub A\nSub A text.\n\n### Sub B\nSub B text.";
        let chunks = chunk_markdown(input);
        assert_eq!(chunks.len(), 3);
        assert_eq!(chunks[0].0, "Section");
        assert_eq!(chunks[1].0, "Sub A");
        assert_eq!(chunks[2].0, "Sub B");
    }

    #[test]
    fn chunk_markdown_empty_body() {
        let input = "";
        let chunks = chunk_markdown(input);
        assert!(chunks.is_empty());
    }

    // ------------------------------------------------------------------
    // compute_checksum
    // ------------------------------------------------------------------

    #[test]
    fn compute_checksum_deterministic() {
        let a = compute_checksum("hello");
        let b = compute_checksum("hello");
        assert_eq!(a, b);
    }

    #[test]
    fn compute_checksum_different() {
        let a = compute_checksum("hello");
        let b = compute_checksum("world");
        assert_ne!(a, b);
    }

    // ------------------------------------------------------------------
    // relative_vault_section
    // ------------------------------------------------------------------

    #[test]
    fn relative_vault_section_extracts_top_dir() {
        let vault = Path::new("/vault");
        let file = Path::new("/vault/notes/rust.md");
        assert_eq!(
            relative_vault_section(file, vault),
            Some("notes".to_string())
        );
    }

    #[test]
    fn relative_vault_section_root_file() {
        let vault = Path::new("/vault");
        let file = Path::new("/vault/README.md");
        assert_eq!(
            relative_vault_section(file, vault),
            Some("README.md".to_string())
        );
    }

    // ------------------------------------------------------------------
    // relative_path_str
    // ------------------------------------------------------------------

    #[test]
    fn relative_path_str_strips_prefix() {
        let vault = Path::new("/vault");
        let file = Path::new("/vault/notes/rust.md");
        assert_eq!(relative_path_str(file, vault), "notes/rust.md");
    }

    #[test]
    fn relative_path_str_outside_vault() {
        let vault = Path::new("/vault");
        let file = Path::new("/other/file.md");
        assert_eq!(relative_path_str(file, vault), "/other/file.md");
    }
}

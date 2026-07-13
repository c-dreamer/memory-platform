//! Obsidian vault ingestion.
//!
//! This module is the shared implementation used by both:
//! - `vault-sync` for standalone vault syncs
//! - `ingest all` / `ingest vault` for scheduled ingestion

use crate::ingest::{IngestEngine, IngestReport};
use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use sha2::{Digest, Sha256};
use sqlx::PgPool;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};
use walkdir::WalkDir;

/// A discovered vault file with metadata.
#[derive(Debug, Clone)]
struct VaultFile {
    path: PathBuf,
    modified_at: DateTime<Utc>,
    checksum: String,
    content: String,
}

/// Sync summary reported at the end.
#[derive(Debug, Default, Clone)]
pub struct VaultSyncSummary {
    pub scanned: usize,
    pub imported: usize,
    pub skipped_duplicate: usize,
    pub renamed: usize,
    pub errors: Vec<String>,
}

/// Scan a directory recursively for `.md` files, sorted by modification time.
fn scan_vault(root: &Path, max_files: usize) -> Result<Vec<VaultFile>> {
    let mut files: Vec<VaultFile> = Vec::new();

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        if path.extension().and_then(|ext| ext.to_str()) != Some("md") {
            continue;
        }

        if path
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        {
            continue;
        }

        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read {:?}: {e}", path);
                continue;
            }
        };

        let checksum = hex::encode(Sha256::digest(content.as_bytes()));

        let metadata = match fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                warn!("Failed to get metadata for {:?}: {e}", path);
                continue;
            }
        };

        let modified_at = metadata
            .modified()
            .ok()
            .map(DateTime::<Utc>::from)
            .unwrap_or_else(Utc::now);

        files.push(VaultFile {
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

    if max_files > 0 {
        files.truncate(max_files);
    }

    Ok(files)
}

/// Ingest the vault into Postgres using the shared document upsert path.
pub async fn sync_vault(
    pool: &PgPool,
    vault_path: &Path,
    limit: usize,
    dry_run: bool,
) -> Result<VaultSyncSummary> {
    let files = scan_vault(vault_path, limit)?;
    info!(
        "Scanned {} .md files from {}",
        files.len(),
        vault_path.display()
    );

    let mut summary = VaultSyncSummary {
        scanned: files.len(),
        ..Default::default()
    };

    for file in files {
        let file_path = normalize_path(&file.path, vault_path);
        debug!("Processing vault file: {file_path}");

        let existing: Option<(uuid::Uuid, String)> = sqlx::query_as(
            "SELECT id, path FROM documents \
             WHERE checksum = $1 AND (path LIKE 'vault://%' OR path LIKE '%/obsidian-vault/%') \
             LIMIT 1",
        )
        .bind(&file.checksum)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("Failed to query checksum for {}", file.path.display()))?;

        if let Some((doc_id, db_path)) = existing {
            if db_path != file_path {
                if dry_run {
                    info!("[DRY-RUN] Would rename: {db_path} -> {file_path}");
                } else {
                    sqlx::query(
                        "UPDATE documents SET path = $1, file_modified_at = $2, updated_at = now() WHERE id = $3",
                    )
                    .bind(&file_path)
                    .bind(file.modified_at)
                    .bind(doc_id)
                    .execute(pool)
                    .await
                    .context("Failed to update document path")?;
                    info!("Renamed: {db_path} -> {file_path}");
                }
                summary.renamed += 1;
            } else {
                debug!("Skipped (duplicate): {file_path}");
                summary.skipped_duplicate += 1;
            }
            continue;
        }

        if dry_run {
            info!(
                "[DRY-RUN] Would import: {file_path} ({} bytes, checksum={})",
                file.content.len(),
                &file.checksum[..12]
            );
            summary.imported += 1;
            continue;
        }

        let title = file
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        let vault_section = file
            .path
            .strip_prefix(vault_path)
            .ok()
            .and_then(|rel| rel.components().next())
            .and_then(|c| c.as_os_str().to_str())
            .map(|s| s.to_string());

        let word_count = file.content.split_whitespace().count();
        let token_count = (word_count as f64 / 0.75).round() as i32;
        let file_modified_at = file.modified_at;
        let imported_at = Utc::now();
        let age_seconds = imported_at
            .signed_duration_since(file_modified_at)
            .num_seconds()
            .max(0);
        let meta = serde_json::json!({
            "source": "obsidian-vault",
            "source_path": file.path.to_string_lossy(),
            "source_last_modified_at": file_modified_at.to_rfc3339(),
            "source_age_seconds": age_seconds,
            "imported_at": imported_at.to_rfc3339(),
        });

        let mut tx = pool.begin().await?;
        sqlx::query("SELECT pg_advisory_xact_lock(hashtext($1))")
            .bind(&file_path)
            .execute(&mut *tx)
            .await?;

        let existing: Option<uuid::Uuid> =
            sqlx::query_scalar("SELECT id FROM documents WHERE path = $1 LIMIT 1")
                .bind(&file_path)
                .fetch_optional(&mut *tx)
                .await?;

        let result = if let Some(doc_id) = existing {
            sqlx::query(
                "UPDATE documents SET \
                 vault_section = $1, \
                 title = $2, \
                 content = $3, \
                 checksum = $4, \
                 token_count = $5, \
                 file_size_bytes = $6, \
                 file_modified_at = $7, \
                 frontmatter = $8, \
                 updated_at = now() \
                 WHERE id = $9",
            )
            .bind(&vault_section)
            .bind(&title)
            .bind(&file.content)
            .bind(&file.checksum)
            .bind(token_count)
            .bind(file.content.len() as i32)
            .bind(file_modified_at)
            .bind(&meta)
            .bind(doc_id)
            .execute(&mut *tx)
            .await
        } else {
            sqlx::query(
                "INSERT INTO documents (path, vault_section, title, content, checksum, \
                 token_count, file_size_bytes, file_modified_at, frontmatter) \
                 VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9)",
            )
            .bind(&file_path)
            .bind(&vault_section)
            .bind(&title)
            .bind(&file.content)
            .bind(&file.checksum)
            .bind(token_count)
            .bind(file.content.len() as i32)
            .bind(file_modified_at)
            .bind(&meta)
            .execute(&mut *tx)
            .await
        };

        match result {
            Ok(_) => {
                tx.commit().await?;
                info!("Imported: {file_path}");
                summary.imported += 1;
            }
            Err(e) => {
                tx.rollback().await.ok();
                let err_msg = format!("Failed to import {}: {e}", file.path.display());
                warn!("{err_msg}");
                summary.errors.push(err_msg);
            }
        }
    }

    Ok(summary)
}

/// Run the vault ingest through the shared engine for CLI callers.
pub async fn ingest_vault(
    engine: &IngestEngine,
    vault_path: &Path,
    limit: usize,
    dry_run: bool,
    report: &mut IngestReport,
) -> Result<VaultSyncSummary> {
    let summary = sync_vault(engine.pool(), vault_path, limit, dry_run).await?;
    report
        .sources_processed
        .insert("obsidian-vault".to_string(), summary.scanned as u64);
    report.documents_created += summary.imported as u64;
    report.errors += summary.errors.len() as u64;
    Ok(summary)
}

fn normalize_path(path: &Path, root: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .components()
        .map(|component| component.as_os_str().to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
        .trim_start_matches("./")
        .to_string()
        .pipe(|rel| format!("vault://{rel}"))
}

trait Pipe: Sized {
    fn pipe<T>(self, f: impl FnOnce(Self) -> T) -> T {
        f(self)
    }
}

impl<T> Pipe for T {}

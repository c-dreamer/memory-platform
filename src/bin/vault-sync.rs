//! Vault sync — unidirectional import of Obsidian vault .md files
//! into the memory platform's `documents` table.
//!
//! Design:
//! - Scans a vault directory recursively for `*.md` files
//! - Sorts by modification time (newest first), processes up to `--limit`
//! - Computes SHA-256 checksum for dedup and rename detection
//! - Skips files whose checksum already exists in the DB (C7 dedup)
//! - Detects renames: same checksum, different file path → updates path
//! - Inserts new documents with metadata and generates embeddings
//!
//! Usage:
//!   cargo run --bin vault-sync -- --vault /path/to/obsidian --limit 100
//!   cargo run --bin vault-sync -- --dry-run  # preview without writing
//!
//! Environment:
//!   DATABASE_URL       — PostgreSQL connection string
//!   VAULT_PATH         — fallback vault directory path
//!   VAULT_LIMIT        — max files to process per run (default: 100)

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::Parser;
use sha2::{Digest, Sha256};
use sqlx::postgres::PgPoolOptions;
use std::fs;
use std::path::{Path, PathBuf};
use tracing::{debug, info, warn};
use walkdir::WalkDir;

/// Vault sync CLI arguments.
#[derive(Parser, Debug)]
#[command(name = "vault-sync", about = "Sync Obsidian vault .md files into the memory platform")]
struct Args {
    /// Path to the Obsidian vault directory.
    #[arg(short = 'v', long = "vault", env = "VAULT_PATH")]
    vault_path: PathBuf,

    /// Maximum number of files to process per run.
    #[arg(short = 'l', long = "limit", default_value = "100", env = "VAULT_LIMIT")]
    limit: usize,

    /// Database URL (defaults to DATABASE_URL env).
    #[arg(short = 'd', long = "db-url", env = "DATABASE_URL")]
    db_url: String,

    /// Dry-run: scan and report without modifying the database.
    #[arg(short = 'n', long = "dry-run")]
    dry_run: bool,

    /// Verbose output.
    #[arg(short = 'V', long = "verbose")]
    verbose: bool,
}

/// A discovered vault file with metadata.
#[derive(Debug, Clone)]
struct VaultFile {
    path: PathBuf,
    modified_at: DateTime<Utc>,
    checksum: String,
    content: String,
}

/// Sync summary reported at the end.
#[derive(Debug, Default)]
struct SyncSummary {
    scanned: usize,
    imported: usize,
    skipped_duplicate: usize,
    renamed: usize,
    errors: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    // Initialise logging
    tracing_subscriber::fmt()
        .with_env_filter(
            if args.verbose { "debug" } else { "info" },
        )
        .init();

    info!("Vault sync starting — vault={:?}, limit={}, dry_run={}",
        args.vault_path, args.limit, args.dry_run);

    // Validate vault path
    if !args.vault_path.exists() {
        anyhow::bail!("Vault path does not exist: {:?}", args.vault_path);
    }
    if !args.vault_path.is_dir() {
        anyhow::bail!("Vault path is not a directory: {:?}", args.vault_path);
    }

    // Scan vault for .md files
    let files = scan_vault(&args.vault_path, args.limit)?;
    info!("Scanned {} .md files (limit={})", files.len(), args.limit);

    if files.is_empty() {
        info!("No .md files found — nothing to sync");
        return Ok(());
    }

    // Connect to database
    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&args.db_url)
        .await
        .context("Failed to connect to database")?;
    info!("Database connected");

    // Run the sync
    let summary = sync_files(&pool, &files, args.dry_run).await?;

    // Report summary
    println!();
    println!("=== Sync Summary ===");
    println!("  Scanned:           {}", summary.scanned);
    println!("  Imported (new):    {}", summary.imported);
    println!("  Skipped (dup):     {}", summary.skipped_duplicate);
    println!("  Renamed:           {}", summary.renamed);
    if !summary.errors.is_empty() {
        println!("  Errors:            {}", summary.errors.len());
        for err in &summary.errors {
            warn!("  Error: {err}");
        }
    }

    info!("Vault sync complete");
    Ok(())
}

/// Scan a directory recursively for .md files, sorted by modification time
/// (newest first), limited to `max_files`.
fn scan_vault(root: &Path, max_files: usize) -> Result<Vec<VaultFile>> {
    let mut files: Vec<VaultFile> = Vec::new();

    for entry in WalkDir::new(root)
        .follow_links(false)
        .into_iter()
        .filter_map(|e| e.ok())
    {
        let path = entry.path();

        // Only process .md files
        if path.extension().map_or(true, |ext| ext != "md") {
            continue;
        }

        // Skip hidden files/dirs
        if path
            .components()
            .any(|c| c.as_os_str().to_string_lossy().starts_with('.'))
        {
            continue;
        }

        // Read file content
        let content = match fs::read_to_string(path) {
            Ok(c) => c,
            Err(e) => {
                warn!("Failed to read {:?}: {e}", path);
                continue;
            }
        };

        // Compute SHA-256 checksum
        let checksum = hex::encode(Sha256::digest(content.as_bytes()));

        // Get modification time
        let metadata = match fs::metadata(path) {
            Ok(m) => m,
            Err(e) => {
                warn!("Failed to get metadata for {:?}: {e}", path);
                continue;
            }
        };

        let modified_at: DateTime<Utc> = metadata
            .modified()
            .ok()
            .map(|t| t.into())
            .unwrap_or_else(Utc::now);

        files.push(VaultFile {
            path: path.to_path_buf(),
            modified_at,
            checksum,
            content,
        });
    }

    // Sort by modification time (newest first), then by path
    files.sort_by(|a, b| {
        b.modified_at
            .cmp(&a.modified_at)
            .then_with(|| a.path.cmp(&b.path))
    });

    // Apply limit
    files.truncate(max_files);

    Ok(files)
}

/// Process vault files against the database:
/// - Skip duplicates (checksum already present)
/// - Detect renames (same checksum, different path)
/// - Insert new documents
async fn sync_files(pool: &sqlx::PgPool, files: &[VaultFile], dry_run: bool) -> Result<SyncSummary> {
    let mut summary = SyncSummary {
        scanned: files.len(),
        ..Default::default()
    };

    for file in files {
        let file_path_str = file.path.to_string_lossy().to_string();
        debug!("Processing: {:?}", file.path);

        // Check if checksum already exists in the documents table
        let existing: Option<(uuid::Uuid, String)> = sqlx::query_as(
            "SELECT id, path FROM documents WHERE checksum = $1 LIMIT 1",
        )
        .bind(&file.checksum)
        .fetch_optional(pool)
        .await
        .with_context(|| format!("Failed to query checksum for {:?}", file.path))?;

        if let Some((doc_id, db_path)) = existing {
            if db_path != file_path_str {
                // Rename detected — same content, different path
                if dry_run {
                    info!("[DRY-RUN] Would rename: {db_path} → {file_path_str}");
                } else {
                    sqlx::query(
                        "UPDATE documents SET path = $1, file_modified_at = $2, updated_at = now() WHERE id = $3",
                    )
                    .bind(&file_path_str)
                    .bind(file.modified_at)
                    .bind(doc_id)
                    .execute(pool)
                    .await
                    .context("Failed to update document path")?;
                    info!("Renamed: {db_path} → {file_path_str}");
                }
                summary.renamed += 1;
            } else {
                // Exact duplicate — skip
                debug!("Skipped (duplicate): {file_path_str}");
                summary.skipped_duplicate += 1;
            }
            // Even if renamed, we skip the embedding step — content didn't change
            continue;
        }

        // New file — insert into documents table
        if dry_run {
            info!("[DRY-RUN] Would import: {file_path_str} ({} bytes, checksum={})",
                file.content.len(), &file.checksum[..12]);
            summary.imported += 1;
            continue;
        }

        // Determine title from filename (strip .md extension)
        let title = file
            .path
            .file_stem()
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        // Determine vault section from parent directory name
        let vault_section = file
            .path
            .parent()
            .and_then(|p| p.file_stem())
            .map(|s| s.to_string_lossy().to_string())
            .unwrap_or_default();

        // Token count: rough estimate (words / 0.75 ≈ tokens)
        let word_count = file.content.split_whitespace().count();
        let token_count = (word_count as f64 / 0.75).round() as i32;

        // Insert the document (embedding will be null initially — can be backfilled)
        let result = sqlx::query(
            "INSERT INTO documents (path, vault_section, title, content, checksum, \
             token_count, file_size_bytes, file_modified_at) \
             VALUES ($1, $2, $3, $4, $5, $6, $7, $8) \
             ON CONFLICT (path) DO UPDATE SET \
             content = EXCLUDED.content, \
             checksum = EXCLUDED.checksum, \
             token_count = EXCLUDED.token_count, \
             file_size_bytes = EXCLUDED.file_size_bytes, \
             file_modified_at = EXCLUDED.file_modified_at, \
             updated_at = now()",
        )
        .bind(&file_path_str)
        .bind(&vault_section)
        .bind(&title)
        .bind(&file.content)
        .bind(&file.checksum)
        .bind(token_count)
        .bind(file.content.len() as i32)
        .bind(file.modified_at)
        .execute(pool)
        .await;

        match result {
            Ok(_) => {
                info!("Imported: {file_path_str}");
                summary.imported += 1;
            }
            Err(e) => {
                let err_msg = format!("Failed to import {:?}: {e}", file.path);
                warn!("{err_msg}");
                summary.errors.push(err_msg);
            }
        }
    }

    Ok(summary)
}

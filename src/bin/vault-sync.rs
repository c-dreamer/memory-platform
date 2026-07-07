//! Vault sync — unidirectional import of Obsidian vault `.md` files
//! into the memory platform's `documents` table.

use anyhow::{Context, Result};
use clap::Parser;
use sqlx::postgres::PgPoolOptions;
use std::path::PathBuf;
use tracing::info;

use memory_platform::ingest::vault;

/// Vault sync CLI arguments.
#[derive(Parser, Debug)]
#[command(
    name = "vault-sync",
    about = "Sync Obsidian vault .md files into the memory platform"
)]
struct Args {
    /// Path to the Obsidian vault directory.
    #[arg(short = 'v', long = "vault", env = "VAULT_PATH")]
    vault_path: PathBuf,

    /// Maximum number of files to process per run.
    #[arg(
        short = 'l',
        long = "limit",
        default_value = "100",
        env = "VAULT_LIMIT"
    )]
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

#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();

    tracing_subscriber::fmt()
        .with_env_filter(if args.verbose { "debug" } else { "info" })
        .init();

    info!(
        "Vault sync starting — vault={:?}, limit={}, dry_run={}",
        args.vault_path, args.limit, args.dry_run
    );

    if !args.vault_path.exists() {
        anyhow::bail!("Vault path does not exist: {:?}", args.vault_path);
    }
    if !args.vault_path.is_dir() {
        anyhow::bail!("Vault path is not a directory: {:?}", args.vault_path);
    }

    let pool = PgPoolOptions::new()
        .max_connections(2)
        .connect(&args.db_url)
        .await
        .context("Failed to connect to database")?;
    info!("Database connected");

    let summary = vault::sync_vault(&pool, &args.vault_path, args.limit, args.dry_run).await?;

    println!();
    println!("=== Sync Summary ===");
    println!("  Scanned:           {}", summary.scanned);
    println!("  Imported (new):    {}", summary.imported);
    println!("  Skipped (dup):     {}", summary.skipped_duplicate);
    println!("  Renamed:           {}", summary.renamed);
    if !summary.errors.is_empty() {
        println!("  Errors:            {}", summary.errors.len());
        for err in &summary.errors {
            println!("  {err}");
        }
    }

    info!("Vault sync complete");
    Ok(())
}

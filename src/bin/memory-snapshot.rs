//! Memory snapshot CLI — portable full-database export/import.
//!
//! Bundles `memories`, `sessions`, `experiences`, and `procedures` into one
//! JSON file for backup, migration, or human inspection/diffing — distinct
//! from `neon-sync` (live outbox replication) and `archive-documents.sh`
//! (moves individual rows to cold storage). Export only; there is no
//! `import` subcommand — every table's insert path here generates a new
//! UUID rather than preserving the original, so a faithful restore needs
//! dedicated upsert-by-id methods that don't exist yet. Flagging that as a
//! follow-up rather than shipping a restore that silently duplicates rows.
//!
//! Environment:
//!   DATABASE_URL — PostgreSQL connection string (local or Neon)

use anyhow::{Context, Result};
use chrono::{DateTime, Utc};
use clap::{Parser, Subcommand};
use memory_platform::db::postgres::PostgresDb;
use serde::Serialize;
use sqlx::postgres::PgPoolOptions;
use std::path::PathBuf;
use tracing::info;
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(
    name = "memory-snapshot",
    about = "Export a full memory-platform snapshot to JSON"
)]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// PostgreSQL connection string
    #[arg(short = 'd', long = "db-url", env = "DATABASE_URL")]
    db_url: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Export every memory, session, experience, and procedure to one JSON file
    Export {
        /// Output file path
        #[arg(short = 'o', long = "out", default_value = "memory-snapshot.json")]
        out: PathBuf,
    },
}

#[derive(Serialize)]
struct Snapshot {
    exported_at: DateTime<Utc>,
    memory_count: usize,
    session_count: usize,
    experience_count: usize,
    procedure_count: usize,
    memories: Vec<memory_platform::models::memory::Memory>,
    sessions: Vec<memory_platform::models::session::Session>,
    experiences: Vec<memory_platform::models::experience::Experience>,
    procedures: Vec<memory_platform::models::procedure::Procedure>,
}

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    info!("Connecting to database...");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&cli.db_url)
        .await
        .context("Failed to connect to PostgreSQL")?;
    let db = PostgresDb::with_pool(pool);

    let Commands::Export { out } = &cli.command;

    let memories = db
        .list_all_memories()
        .await
        .context("Failed to list memories")?;
    let sessions = db
        .list_all_sessions()
        .await
        .context("Failed to list sessions")?;
    let experiences = db
        .list_experiences(i64::MAX)
        .await
        .context("Failed to list experiences")?;
    let procedures = db
        .list_procedures()
        .await
        .context("Failed to list procedures")?;

    let snapshot = Snapshot {
        exported_at: Utc::now(),
        memory_count: memories.len(),
        session_count: sessions.len(),
        experience_count: experiences.len(),
        procedure_count: procedures.len(),
        memories,
        sessions,
        experiences,
        procedures,
    };

    let json = serde_json::to_string_pretty(&snapshot).context("Failed to serialize snapshot")?;
    std::fs::write(out, json).with_context(|| format!("Failed to write {}", out.display()))?;

    info!(
        "Snapshot written to {}: {} memories, {} sessions, {} experiences, {} procedures",
        out.display(),
        snapshot.memory_count,
        snapshot.session_count,
        snapshot.experience_count,
        snapshot.procedure_count
    );

    Ok(())
}

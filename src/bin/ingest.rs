//! Ingest CLI — batch-imports external data sources into the memory platform.
//!
//! Subcommands:
//! - `sessions` — Ingest OpenCode sessions from `opencode.db` SQLite
//! - `vault` — Ingest Obsidian vault .md files (delegates to vault-sync)
//! - `config` — Ingest OpenCode config, rules, skills
//! - `logs` — Ingest OpenCode log file
//! - `all` — Run all ingest steps sequentially
//!
//! Environment:
//!   DATABASE_URL — PostgreSQL connection string (local or Neon)

use anyhow::{Context, Result};
use clap::{Parser, Subcommand};
use memory_platform::ingest::{
    codex_sessions as ingest_codex_sessions, config as ingest_config, logs as ingest_logs,
    sessions as ingest_sessions, vault as ingest_vault,
};
use memory_platform::ingest::{IngestEngine, IngestReport};
use sqlx::postgres::PgPoolOptions;
use std::path::PathBuf;
use tracing::{info, warn};
use tracing_subscriber::EnvFilter;

#[derive(Parser)]
#[command(name = "ingest", about = "Batch-import data into the memory platform")]
struct Cli {
    #[command(subcommand)]
    command: Commands,

    /// PostgreSQL connection string
    #[arg(short = 'd', long = "db-url", env = "DATABASE_URL")]
    db_url: String,
}

#[derive(Subcommand)]
enum Commands {
    /// Ingest OpenCode sessions from SQLite DB
    Sessions {
        /// Path to opencode.db (SQLite)
        #[arg(
            short = 's',
            long = "source",
            default_value = "~/.local/share/opencode/opencode.db"
        )]
        source: PathBuf,
    },
    /// Ingest Obsidian vault .md files
    Vault {
        /// Path to Obsidian vault directory
        #[arg(short = 'p', long = "path", env = "VAULT_PATH")]
        vault_path: PathBuf,

        /// Max files to process (default: all)
        #[arg(short = 'l', long = "limit", default_value = "0")]
        limit: usize,
    },
    /// Ingest OpenCode config, rules, and skills
    Config {
        /// Path to opencode config directory
        #[arg(short = 'd', long = "dir", default_value = "~/.config/opencode")]
        config_dir: PathBuf,
    },
    /// Ingest OpenCode runtime log
    Logs {
        /// Path to opencode.log
        #[arg(
            short = 'l',
            long = "log",
            default_value = "~/.local/share/opencode/log/opencode.log"
        )]
        log_path: PathBuf,
    },
    /// Ingest Codex session archives from ~/.codex/sessions
    Codex {
        /// Path to Codex session archive root
        #[arg(short = 'p', long = "path", default_value = "~/.codex/sessions")]
        sessions_path: PathBuf,
    },
    /// Run all ingest steps
    All {
        /// Path to opencode.db (SQLite)
        #[arg(
            long = "sessions-db",
            default_value = "~/.local/share/opencode/opencode.db"
        )]
        sessions_db: PathBuf,

        /// Path to Obsidian vault
        #[arg(
            short = 'p',
            long = "vault-path",
            env = "VAULT_PATH",
            default_value = "~/obsidian-vault"
        )]
        vault_path: PathBuf,

        /// Path to opencode config directory
        #[arg(long = "config-dir", default_value = "~/.config/opencode")]
        config_dir: PathBuf,

        /// Path to opencode.log
        #[arg(
            long = "log-path",
            default_value = "~/.local/share/opencode/log/opencode.log"
        )]
        log_path: PathBuf,

        /// Path to Codex session archive root
        #[arg(long = "codex-path", default_value = "~/.codex/sessions")]
        codex_path: PathBuf,
    },
}

fn expand_path(path: &PathBuf) -> PathBuf {
    let s = path.to_string_lossy().to_string();
    if s.starts_with("~/") {
        if let Ok(home) = std::env::var("HOME") {
            return PathBuf::from(s.replacen("~/", &format!("{home}/"), 1));
        }
    }
    path.clone()
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::from_default_env().add_directive(tracing::Level::INFO.into()))
        .with_target(false)
        .with_writer(std::io::stderr)
        .init();

    let cli = Cli::parse();

    // Connect to Postgres
    info!("Connecting to database...");
    let pool = PgPoolOptions::new()
        .max_connections(5)
        .connect(&cli.db_url)
        .await
        .context("Failed to connect to PostgreSQL")?;

    let engine = IngestEngine::new(pool);
    let mut report = IngestReport::default();

    match &cli.command {
        Commands::Sessions { source } => {
            let path = expand_path(source);
            if !path.exists() {
                anyhow::bail!("Session DB not found at: {}", path.display());
            }
            ingest_sessions::ingest_sessions(&engine, &path, &mut report).await?;
        }
        Commands::Vault { vault_path, limit } => {
            let path = expand_path(vault_path);
            if !path.is_dir() {
                anyhow::bail!("Vault directory not found: {}", path.display());
            }
            ingest_vault::ingest_vault(&engine, &path, *limit, false, &mut report).await?;
        }
        Commands::Config { config_dir } => {
            let path = expand_path(config_dir);
            ingest_config::ingest_config(&engine, &path, &mut report).await?;
        }
        Commands::Logs { log_path } => {
            let path = expand_path(log_path);
            ingest_logs::ingest_logs(&engine, &path, &mut report).await?;
        }
        Commands::Codex { sessions_path } => {
            let path = expand_path(sessions_path);
            ingest_codex_sessions::ingest_codex_sessions(&engine, &path, &mut report).await?;
        }
        Commands::All {
            sessions_db,
            vault_path,
            config_dir,
            log_path,
            codex_path,
        } => {
            println!("\n═══════════════════════════════════════");
            println!("  INGESTING ALL SOURCES");
            println!("═══════════════════════════════════════\n");

            // 1. Sessions
            let sessions_path = expand_path(sessions_db);
            if sessions_path.exists() {
                println!("[1/4] Ingesting OpenCode sessions...");
                ingest_sessions::ingest_sessions(&engine, &sessions_path, &mut report).await?;
                println!("  ✓ Sessions complete");
            } else {
                warn!(
                    "Sessions DB not found at {}, skipping",
                    sessions_path.display()
                );
            }

            // 2. Vault
            let vault = expand_path(vault_path);
            if vault.is_dir() {
                println!("\n[2/4] Ingesting Obsidian vault...");
                ingest_vault::ingest_vault(&engine, &vault, 0, false, &mut report).await?;
                println!("  ✓ Vault complete");
            } else {
                warn!("Vault not found at {}, skipping", vault.display());
            }

            // 3. Config
            let cfg_dir = expand_path(config_dir);
            if cfg_dir.is_dir() {
                println!("\n[3/4] Ingesting OpenCode config, rules, skills...");
                ingest_config::ingest_config(&engine, &cfg_dir, &mut report).await?;
                println!("  ✓ Config complete");
            } else {
                warn!("Config dir not found at {}, skipping", cfg_dir.display());
            }

            // 4. Logs
            let log = expand_path(log_path);
            if log.exists() {
                println!("\n[4/4] Ingesting OpenCode logs...");
                ingest_logs::ingest_logs(&engine, &log, &mut report).await?;
                println!("  ✓ Logs complete");
            } else {
                warn!("Log not found at {}, skipping", log.display());
            }

            // 5. Codex sessions
            let codex_root = expand_path(codex_path);
            if codex_root.is_dir() {
                println!("\n[5/5] Ingesting Codex session archives...");
                ingest_codex_sessions::ingest_codex_sessions(&engine, &codex_root, &mut report)
                    .await?;
                println!("  ✓ Codex sessions complete");
            } else {
                warn!(
                    "Codex sessions not found at {}, skipping",
                    codex_root.display()
                );
            }
        }
    }

    report.print();
    Ok(())
}

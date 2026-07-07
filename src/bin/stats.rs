//! Stats CLI — prints database size and table counts for the memory platform.

use anyhow::Result;
use clap::Parser;
use memory_platform::Config;
use memory_platform::db::postgres::PostgresDb;
use serde_json::json;
use sqlx::query_scalar;

#[derive(Parser)]
#[command(name = "stats", about = "Show memory platform database size and counts")]
struct Cli {
    /// PostgreSQL connection string
    #[arg(short = 'd', long = "db-url", env = "DATABASE_URL")]
    db_url: Option<String>,

    /// Emit JSON instead of human-readable output
    #[arg(long)]
    json: bool,

    /// Compare two database URLs and print a side-by-side summary
    #[arg(long = "compare", num_args = 2, value_names = ["LEFT", "RIGHT"])]
    compare: Vec<String>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let cli = Cli::parse();

    if cli.compare.len() == 2 {
        let left = summarize(&cli.compare[0]).await?;
        let right = summarize(&cli.compare[1]).await?;

        if cli.json {
            println!(
                "{}",
                serde_json::to_string_pretty(&json!({
                    "left": left,
                    "right": right,
                }))?
            );
            return Ok(());
        }

        print_summary("left", &left);
        print_summary("right", &right);
        print_delta(&left, &right);
        return Ok(());
    }

    let mut config = Config::from_env()?;
    if let Some(db_url) = cli.db_url {
        config.database_url = db_url;
    }

    let summary = summarize(&config.database_url).await?;

    if cli.json {
        println!(
            "{}",
            serde_json::to_string_pretty(&summary)?
        );
        return Ok(());
    }

    print_summary("database", &summary);

    Ok(())
}

async fn summarize(db_url: &str) -> Result<serde_json::Value> {
    let mut config = Config::from_env()?;
    config.database_url = db_url.to_string();
    let db = PostgresDb::connect(&config).await?;
    let size_bytes: i64 = query_scalar("SELECT pg_database_size(current_database())")
        .fetch_one(&db.pool)
        .await?;
    let counts = db.get_stats().await;
    let size_mb = size_bytes as f64 / 1024.0 / 1024.0;
    Ok(json!({
        "database_url": db_url,
        "size_bytes": size_bytes,
        "size_mb": size_mb,
        "counts": counts,
    }))
}

fn print_summary(label: &str, summary: &serde_json::Value) {
    println!("{}: {}", label, summary["database_url"].as_str().unwrap_or_default());
    println!(
        "  size: {:.2} MB ({} bytes)",
        summary["size_mb"].as_f64().unwrap_or_default(),
        summary["size_bytes"].as_i64().unwrap_or_default()
    );
    println!("  counts:");
    if let Some(counts) = summary["counts"].as_object() {
        for key in [
            "documents",
            "memories",
            "experiences",
            "sessions",
            "agents",
            "procedures",
            "trading_results",
            "relationships",
        ] {
            println!(
                "    {key}: {}",
                counts.get(key).and_then(|v| v.as_i64()).unwrap_or_default()
            );
        }
    }
}

fn print_delta(left: &serde_json::Value, right: &serde_json::Value) {
    let left_mb = left["size_mb"].as_f64().unwrap_or_default();
    let right_mb = right["size_mb"].as_f64().unwrap_or_default();
    println!("delta:");
    println!("  size_mb: {:.2}", right_mb - left_mb);
    if let (Some(l), Some(r)) = (left["counts"].as_object(), right["counts"].as_object()) {
        for key in [
            "documents",
            "memories",
            "experiences",
            "sessions",
            "agents",
            "procedures",
            "trading_results",
            "relationships",
        ] {
            let lv = l.get(key).and_then(|v| v.as_i64()).unwrap_or_default();
            let rv = r.get(key).and_then(|v| v.as_i64()).unwrap_or_default();
            println!("  {key}: {}", rv - lv);
        }
    }
}

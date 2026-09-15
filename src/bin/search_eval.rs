//! Search evaluation harness.
//!
//! Measures recall@K, p50/p95 latency, and approximate context-token cost of
//! `hybrid_search` against a small, checked-in fixture query set
//! (`benches/fixtures/search_eval.json`). Runs in `"keyword"` mode, so it
//! needs no embedding service — only a reachable Postgres (a throwaway Neon
//! branch works, same as `tests/integration.rs`).
//!
//! Seeds its own fixture memories, evaluates against them, then deletes
//! them — safe to re-run against a shared database.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use anyhow::{Context, Result};
use memory_platform::config::Config;
use memory_platform::db::postgres::PostgresDb;
use memory_platform::migrations::Migrator;
use memory_platform::search::SearchEngine;
use serde::Deserialize;
use uuid::Uuid;

const FIXTURE_JSON: &str = include_str!("../../benches/fixtures/search_eval.json");
const TOP_K: i64 = 5;

#[derive(Deserialize)]
struct FixtureMemory {
    key: String,
    content: String,
    tags: Vec<String>,
}

#[derive(Deserialize)]
struct FixtureQuery {
    query: String,
    expect: Vec<String>,
}

#[derive(Deserialize)]
struct Fixtures {
    memories: Vec<FixtureMemory>,
    queries: Vec<FixtureQuery>,
}

#[tokio::main]
async fn main() -> Result<()> {
    let fixtures: Fixtures = serde_json::from_str(FIXTURE_JSON)
        .context("failed to parse benches/fixtures/search_eval.json")?;

    let config = Config::from_env()?;
    let db = Arc::new(PostgresDb::connect(&config).await?);
    Migrator::run(&db.pool).await?;
    let search = SearchEngine::new(db.clone(), Arc::new(config));

    let mut key_to_id: HashMap<String, Uuid> = HashMap::new();
    for m in &fixtures.memories {
        let row = db
            .store_memory(
                &m.content,
                "note",
                0.5,
                &m.tags,
                &serde_json::json!({}),
                None,
                None,
                None,
                None,
                None,
            )
            .await
            .context("failed to seed fixture memory")?;
        key_to_id.insert(m.key.clone(), row.id);
    }

    let mut latencies_ms = Vec::with_capacity(fixtures.queries.len());
    let mut hits = 0usize;
    let mut context_chars = 0usize;

    for q in &fixtures.queries {
        let start = Instant::now();
        let results = search
            .hybrid_search("memories", &q.query, &[], "keyword", TOP_K)
            .await
            .context("search failed")?;
        latencies_ms.push(start.elapsed().as_secs_f64() * 1000.0);

        let expected_ids: Vec<Uuid> = q
            .expect
            .iter()
            .filter_map(|k| key_to_id.get(k).copied())
            .collect();
        if results.iter().any(|r| expected_ids.contains(&r.id)) {
            hits += 1;
        } else {
            eprintln!("MISS: query {:?}, expected {:?}", q.query, q.expect);
        }
        context_chars += results.iter().map(|r| r.content.len()).sum::<usize>();
    }

    let ids: Vec<Uuid> = key_to_id.values().copied().collect();
    sqlx::query("DELETE FROM embeddings WHERE source_table = 'memories' AND source_id = ANY($1)")
        .bind(&ids)
        .execute(&db.pool)
        .await
        .ok();
    sqlx::query("DELETE FROM memories WHERE id = ANY($1)")
        .bind(&ids)
        .execute(&db.pool)
        .await
        .context("failed to clean up fixture memories")?;

    latencies_ms.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let recall = hits as f64 / fixtures.queries.len() as f64;
    let avg_context_tokens = (context_chars as f64 / fixtures.queries.len() as f64) / 4.0; // ~4 chars/token heuristic

    println!(
        "search_eval: {} queries against {} seeded memories",
        fixtures.queries.len(),
        fixtures.memories.len()
    );
    println!(
        "  recall@{TOP_K}: {:.0}% ({hits}/{})",
        recall * 100.0,
        fixtures.queries.len()
    );
    println!(
        "  latency p50: {:.2} ms, p95: {:.2} ms",
        percentile(&latencies_ms, 50.0),
        percentile(&latencies_ms, 95.0)
    );
    println!("  avg context tokens per query (top {TOP_K}): {avg_context_tokens:.0}");

    Ok(())
}

fn percentile(sorted: &[f64], pct: f64) -> f64 {
    if sorted.is_empty() {
        return 0.0;
    }
    let idx = ((pct / 100.0) * (sorted.len() as f64 - 1.0)).round() as usize;
    sorted[idx.min(sorted.len() - 1)]
}

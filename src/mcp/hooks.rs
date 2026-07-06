//! MCP session lifecycle hooks.
//!
//! - on_session_start: Load recent context, return structured memory package
//! - on_session_end: Trigger compaction on session memories
//! - pre_compact: Deduplicate and compact memories for a session

use anyhow::{Context, Result};
use tracing::{debug, info};
use uuid::Uuid;

use crate::db::postgres::PostgresDb;
use crate::db::postgres::ContextPackage;

/// Called when a new session starts.
///
/// Loads recent sessions and relevant context to prime the agent
/// with what the system already knows about similar work.
pub async fn on_session_start(db: &PostgresDb, session_id: Uuid) -> Result<ContextPackage> {
    info!("Session start hook: {session_id}");

    // Load the session to get its goal
    let session = db
        .get_session(session_id)
        .await
        .context("Failed to load session")?
        .ok_or_else(|| anyhow::anyhow!("Session not found: {session_id}"))?;

    let goal = session.goal.as_deref().unwrap_or("current work");

    // Assemble context using a zero embedding (keyword-only search)
    // since we may not have an embedding service wired in the hook layer
    let dummy_embedding = vec![0.0_f32; 384];
    let context = db
        .get_context_for_query(goal, &dummy_embedding, 10)
        .await
        .context("Failed to assemble context for session start")?;

    debug!(
        "Context loaded for session {session_id}: {} memories, {} docs, {} experiences",
        context.memories.len(),
        context.documents.len(),
        context.experiences.len()
    );

    Ok(context)
}

/// Called when a session ends.
///
/// Triggers compaction on memories created during this session.
pub async fn on_session_end(db: &PostgresDb, session_id: Uuid) -> Result<()> {
    info!("Session end hook: {session_id}");

    // Trigger pre-compaction for this session's memories
    pre_compact(db, session_id).await?;

    Ok(())
}

/// Pre-compaction hook: deduplicate and compact memories for a session.
///
/// Finds memories with very similar content within the same session
/// and merges them, keeping the most recent version.
pub async fn pre_compact(db: &PostgresDb, session_id: Uuid) -> Result<()> {
    debug!("Pre-compaction hook for session: {session_id}");

    // Find memories belonging to this session
    let memories = sqlx::query_as::<_, crate::models::Memory>(
        "SELECT id, agent_id, session_id, content, content_type, embedding, \
         importance, tags, metadata, last_accessed_at, access_count, \
         decay_score, created_at, updated_at \
         FROM memories WHERE session_id = $1 ORDER BY created_at DESC",
    )
    .bind(session_id)
    .fetch_all(&db.pool)
    .await
    .context("Failed to fetch session memories for compaction")?;

    if memories.len() < 2 {
        debug!("Session {session_id} has fewer than 2 memories — nothing to compact");
        return Ok(());
    }

    // Simple dedup: if two memories have >90% content similarity,
    // keep the newer one and soft-delete the older one
    let mut compacted = 0usize;
    for i in 0..memories.len() {
        for j in (i + 1)..memories.len() {
            let similarity = jaccard_similarity(&memories[i].content, &memories[j].content);
            if similarity > 0.9 {
                // Soft-delete the older memory (j is older since we ordered DESC)
                sqlx::query(
                    "UPDATE memories SET importance = 0.0, \
                     tags = array_append(tags, 'compacted'), \
                     updated_at = now() \
                     WHERE id = $1",
                )
                .bind(memories[j].id)
                .execute(&db.pool)
                .await
                .context("Failed to compact memory")?;
                compacted += 1;
            }
        }
    }

    if compacted > 0 {
        info!("Compacted {compacted} duplicate memories in session {session_id}");
    }

    Ok(())
}

/// Format a context package into a readable banner string.
///
/// Matches the Python MCP banner format: box-drawing header,
/// categorized sections with counts, and a footer line.
#[must_use]
pub fn format_context_banner(context: &ContextPackage) -> String {
    let mut lines = Vec::new();
    lines.push("\u{2501}\u{2501}\u{2501} Memory Context \u{2501}\u{2501}\u{2501}".to_string());

    let has_content = !context.memories.is_empty()
        || !context.documents.is_empty()
        || !context.experiences.is_empty()
        || !context.recent_sessions.is_empty()
        || !context.procedures.is_empty()
        || !context.trading_results.is_empty();

    if !has_content {
        lines.push(
            "  (no context found \u{2014} work will be stored for future reference)".to_string(),
        );
    } else {
        if !context.memories.is_empty() {
            lines.push(format!("\nMemories ({}):", context.memories.len()));
            for m in context.memories.iter().take(5) {
                lines.push(format!(
                    "  - [score:{:.2}] {}",
                    m.score,
                    truncate_str(&m.content, 120)
                ));
            }
        }

        if !context.documents.is_empty() {
            lines.push(format!("\nDocuments ({}):", context.documents.len()));
            for d in context.documents.iter().take(5) {
                lines.push(format!(
                    "  - [score:{:.2}] {} ({})",
                    d.score,
                    truncate_str(&d.content, 120),
                    d.source_info
                ));
            }
        }

        if !context.experiences.is_empty() {
            lines.push(format!(
                "\nExperiences ({}):",
                context.experiences.len()
            ));
            for e in context.experiences.iter().take(3) {
                lines.push(format!(
                    "  - [score:{:.2}] {}",
                    e.score,
                    truncate_str(&e.content, 120)
                ));
            }
        }

        if !context.recent_sessions.is_empty() {
            lines.push(format!(
                "\nRecent sessions ({}):",
                context.recent_sessions.len()
            ));
            for s in &context.recent_sessions {
                lines.push(format!(
                    "  - {} ({})",
                    s.goal.as_deref().unwrap_or("(no goal)"),
                    s.status
                ));
            }
        }

        if !context.procedures.is_empty() {
            lines.push(format!(
                "\nProcedures ({}):",
                context.procedures.len()
            ));
            for p in &context.procedures {
                lines.push(format!("  - [used:{}] {}", p.times_used, p.name));
            }
        }

        if !context.trading_results.is_empty() {
            lines.push(format!(
                "\nTrading results ({}):",
                context.trading_results.len()
            ));
            for t in &context.trading_results {
                lines.push(format!(
                    "  - [pf:{:.2}] {}",
                    t.profit_factor.unwrap_or(0.0),
                    t.ea_version.as_deref().unwrap_or("unknown")
                ));
            }
        }
    }

    lines.push(
        "\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}\u{2501}"
            .to_string(),
    );
    lines.join("\n")
}

/// Compute Jaccard similarity between two strings based on word overlap.
///
/// Returns a value in [0.0, 1.0] where 1.0 means identical word sets.
fn jaccard_similarity(a: &str, b: &str) -> f64 {
    let words_a: std::collections::HashSet<&str> =
        a.split_whitespace().collect();
    let words_b: std::collections::HashSet<&str> =
        b.split_whitespace().collect();

    if words_a.is_empty() && words_b.is_empty() {
        return 1.0;
    }

    let intersection = words_a.intersection(&words_b).count();
    let union = words_a.union(&words_b).count();

    if union == 0 {
        return 0.0;
    }

    intersection as f64 / union as f64
}

/// Truncate a string to `max_len` characters, appending "..." if truncated.
fn truncate_str(s: &str, max_len: usize) -> String {
    if s.chars().count() <= max_len {
        s.to_string()
    } else {
        format!("{}...", s.chars().take(max_len).collect::<String>())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::postgres::ContextPackage;
    use chrono::Utc;
    use uuid::Uuid;

    fn empty_context() -> ContextPackage {
        ContextPackage {
            memories: vec![],
            documents: vec![],
            experiences: vec![],
            recent_sessions: vec![],
            procedures: vec![],
            trading_results: vec![],
        }
    }

    #[test]
    fn format_context_banner_empty() {
        let banner = format_context_banner(&empty_context());
        assert!(banner.contains("no context found"));
        assert!(banner.starts_with('\u{2501}'));
    }

    #[test]
    fn format_context_banner_with_memories() {
        use crate::db::postgres::SearchResult;

        let mut ctx = empty_context();
        ctx.memories.push(SearchResult {
            id: Uuid::new_v4(),
            content: "Rust is memory-safe without a garbage collector".into(),
            source_info: "rust,memory".into(),
            score: 0.95,
        });

        let banner = format_context_banner(&ctx);
        assert!(banner.contains("Memories (1)"));
        assert!(banner.contains("Rust is memory-safe"));
    }

    #[test]
    fn jaccard_identical() {
        assert!((jaccard_similarity("a b c", "a b c") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn jaccard_disjoint() {
        assert!((jaccard_similarity("a b c", "d e f") - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn jaccard_partial() {
        let sim = jaccard_similarity("a b c", "a b d");
        // intersection: {a, b} = 2, union: {a, b, c, d} = 4 → 0.5
        assert!((sim - 0.5).abs() < f64::EPSILON);
    }

    #[test]
    fn jaccard_empty_both() {
        assert!((jaccard_similarity("", "") - 1.0).abs() < f64::EPSILON);
    }

    #[test]
    fn jaccard_one_empty() {
        assert!((jaccard_similarity("a b c", "") - 0.0).abs() < f64::EPSILON);
    }

    #[test]
    fn truncate_str_short() {
        assert_eq!(truncate_str("hello", 10), "hello");
    }

    #[test]
    fn truncate_str_long() {
        let long = "a".repeat(200);
        let result = truncate_str(&long, 10);
        assert_eq!(result.len(), 13);
        assert!(result.ends_with("..."));
    }
}
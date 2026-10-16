//! Derived, source-checked hierarchy. Rebuilding never changes authoritative records.

use std::collections::HashMap;

use anyhow::{ensure, Result};
use chrono::{DateTime, Utc};
use pulldown_cmark::{Event, Parser, Tag, TagEnd};
use serde::Serialize;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use sqlx::{PgPool, Postgres, Row, Transaction};
use uuid::Uuid;
use crate::{db::postgres::SearchFilters, search::SearchResult};

#[derive(Debug, Clone)]
struct ParsedNode {
    node_key: String,
    parent_key: Option<String>,
    ordinal: i32,
    title: String,
    section_path: String,
    content: String,
    content_start: i32,
    content_end: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct StructuralEvidence {
    pub node_id: Uuid,
    pub section_path: String,
    pub root_scope: String,
    pub score_boost: f64,
    pub excerpt: String,
    pub source_checksum: String,
    pub observed_at: Option<DateTime<Utc>>,
}

pub struct StructuralRetrieval {
    pub documents: Vec<SearchResult>,
    pub evidence: HashMap<Uuid, StructuralEvidence>,
    pub refinement_used: bool,
}

fn checksum(content: &str) -> String {
    hex::encode(Sha256::digest(content.as_bytes()))
}

fn bounded_excerpt(content: &str) -> String {
    match content.char_indices().nth(1_200) {
        Some((end, _)) => format!("{}...", &content[..end]),
        None => content.to_owned(),
    }
}

/// Ranges partition the source instead of copying an entire subtree into each
/// ancestor. Ancestor context can be selected separately at retrieval time.
fn parse_markdown(path: &str, title: &str, content: &str) -> Vec<ParsedNode> {
    let mut headings: Vec<(usize, usize, String)> = Vec::new();
    let mut heading: Option<(usize, usize, String)> = None;
    for (event, range) in Parser::new(content).into_offset_iter() {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                heading = Some((range.start, level as usize, String::new()));
            }
            Event::Text(text) | Event::Code(text) => {
                if let Some((_, _, title)) = &mut heading {
                    title.push_str(&text);
                }
            }
            Event::End(TagEnd::Heading(_)) => {
                if let Some(h) = heading.take() {
                    headings.push(h);
                }
            }
            _ => {}
        }
    }
    let first = headings.first().map_or(content.len(), |h| h.0);
    let mut nodes = vec![ParsedNode {
        node_key: "root".into(), parent_key: None, ordinal: 0,
        title: title.into(), section_path: path.into(),
        content: content[..first].into(), content_start: 0, content_end: first as i32,
    }];
    let mut stack: Vec<(usize, String, String)> = Vec::new();
    let mut occurrences: HashMap<(String, String), usize> = HashMap::new();
    for (index, (start, level, heading)) in headings.iter().enumerate() {
        while stack.last().is_some_and(|(prior, _, _)| prior >= level) {
            stack.pop();
        }
        let parent = stack.last().map_or("root", |(_, key, _)| key.as_str());
        let prefix = stack.last().map_or(path, |(_, _, path)| path.as_str());
        let count = occurrences.entry((parent.into(), heading.clone())).or_default();
        *count += 1;
        let key = checksum(&json!([parent, heading, count]).to_string());
        let section_path = format!("{prefix} > {heading}");
        let end = headings.get(index + 1).map_or(content.len(), |h| h.0);
        nodes.push(ParsedNode {
            node_key: key.clone(), parent_key: Some(parent.into()), ordinal: (index + 1) as i32,
            title: heading.clone(), section_path: section_path.clone(),
            content: content[*start..end].into(), content_start: *start as i32, content_end: end as i32,
        });
        stack.push((*level, key, section_path));
    }
    nodes
}

/// Classification requires explicit provenance; a folder name alone is not a vault.
pub fn explicit_scope(metadata: &Value, key: &str) -> String {
    metadata.get(key).and_then(Value::as_str).map(str::trim)
        .filter(|s| !s.is_empty()).unwrap_or("unclassified").to_string()
}

fn stable_id(table: &str, source: Uuid, key: &str) -> Uuid {
    let digest = Sha256::digest(json!(["memory-content-node-v1", table, source, key]).to_string().as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x80;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;
    Uuid::from_bytes(bytes)
}

#[allow(clippy::too_many_arguments)]
async fn write_nodes(
    tx: &mut Transaction<'_, Postgres>, table: &str, source_id: Uuid,
    nodes: &[ParsedNode], scope: &str, tier: &str, source_checksum: &str,
    observed_at: Option<DateTime<Utc>>, metadata: &Value,
) -> Result<()> {
    let keys: Vec<&str> = nodes.iter().map(|n| n.node_key.as_str()).collect();
    for n in nodes {
        sqlx::query(
            "INSERT INTO content_nodes (id,source_table,source_id,node_key,parent_key,root_scope,node_type,ordinal,title,section_path,content,content_start,content_end,checksum,source_checksum,storage_tier,observed_at,metadata) \
             VALUES ($1,$2,$3,$4,$5,$6,$7,$8,$9,$10,$11,$12,$13,$14,$15,$16,$17,$18) \
             ON CONFLICT (source_table,source_id,node_key) DO UPDATE SET \
                parent_key=EXCLUDED.parent_key,root_scope=EXCLUDED.root_scope,node_type=EXCLUDED.node_type,ordinal=EXCLUDED.ordinal, \
                title=EXCLUDED.title,section_path=EXCLUDED.section_path,content=EXCLUDED.content,content_start=EXCLUDED.content_start,content_end=EXCLUDED.content_end, \
                checksum=EXCLUDED.checksum,source_checksum=EXCLUDED.source_checksum,storage_tier=EXCLUDED.storage_tier,observed_at=EXCLUDED.observed_at,metadata=EXCLUDED.metadata,updated_at=now() \
             WHERE (to_jsonb(content_nodes) - ARRAY['id','created_at','updated_at','fts']) \
                IS DISTINCT FROM (to_jsonb(EXCLUDED) - ARRAY['id','created_at','updated_at','fts'])",
        )
        .bind(stable_id(table, source_id, &n.node_key)).bind(table).bind(source_id)
        .bind(&n.node_key).bind(&n.parent_key).bind(scope)
        .bind(if n.parent_key.is_some() { "section" } else { "root" }).bind(n.ordinal)
        .bind(&n.title).bind(&n.section_path).bind(&n.content).bind(n.content_start).bind(n.content_end)
        .bind(checksum(&n.content)).bind(source_checksum).bind(tier).bind(observed_at).bind(metadata)
        .execute(&mut **tx).await?;
    }
    sqlx::query("DELETE FROM content_nodes WHERE source_table=$1 AND source_id=$2 AND NOT (node_key=ANY($3))")
        .bind(table).bind(source_id).bind(keys).execute(&mut **tx).await?;
    Ok(())
}

#[allow(clippy::too_many_arguments)]
pub async fn rebuild_document_nodes(
    pool: &PgPool, document_id: Uuid, path: &str, title: &str, content: &str,
    _source_checksum: Option<&str>, _observed_at: Option<DateTime<Utc>>, scope: &str, _storage_tier: &str,
) -> Result<usize> {
    ensure!(content.len() <= i32::MAX as usize, "document exceeds supported range");
    let mut tx = pool.begin().await?;
    // Serialize rebuilds and hold the source stable until the derived write commits.
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("structure:documents:{document_id}")).execute(&mut *tx).await?;
    let row = sqlx::query("SELECT path,coalesce(title,'') AS title,content,storage_tier,frontmatter,file_modified_at FROM documents WHERE id=$1 FOR SHARE")
        .bind(document_id).fetch_one(&mut *tx).await?;
    ensure!(row.get::<String,_>("content") == content && row.get::<String,_>("path") == path
        && row.get::<String,_>("title") == title, "source changed; retry hierarchy indexing");
    let metadata: Value = row.get("frontmatter");
    let verified_scope = explicit_scope(&metadata, "vault_scope");
    ensure!(scope == verified_scope, "scope is not supported by source provenance");
    let nodes = parse_markdown(path, title, content);
    write_nodes(&mut tx, "documents", document_id, &nodes, &verified_scope,
        &row.get::<String,_>("storage_tier"), &checksum(content), row.get("file_modified_at"),
        &json!({"derived":true,"parser":"markdown-v2"})).await?;
    tx.commit().await?;
    Ok(nodes.len())
}

#[allow(clippy::too_many_arguments)]
pub async fn rebuild_flat_node(
    pool: &PgPool, source_table: &str, source_id: Uuid, node_key: &str, _scope: &str,
    title: &str, content: &str, _storage_tier: &str, _observed_at: Option<DateTime<Utc>>, _metadata: Value,
) -> Result<()> {
    ensure!(matches!(source_table, "memories" | "sessions"), "unsupported flat source");
    ensure!(content.len() <= i32::MAX as usize, "record exceeds supported range");
    let mut tx = pool.begin().await?;
    sqlx::query("SELECT pg_advisory_xact_lock(hashtextextended($1, 0))")
        .bind(format!("structure:{source_table}:{source_id}")).execute(&mut *tx).await?;
    let sql = if source_table == "memories" {
        "SELECT content,coalesce(content_type,'note') AS title,storage_tier,metadata,updated_at FROM memories WHERE id=$1 FOR SHARE"
    } else {
        "SELECT coalesce(summary,goal,'') AS content,coalesce(goal,'Session') AS title,storage_tier,'{}'::jsonb AS metadata,updated_at FROM sessions WHERE id=$1 FOR SHARE"
    };
    let row = sqlx::query(sql).bind(source_id).fetch_one(&mut *tx).await?;
    ensure!(row.get::<String,_>("content") == content && row.get::<String,_>("title") == title,
        "source changed; retry hierarchy indexing");
    let metadata: Value = row.get("metadata");
    let scope = explicit_scope(&metadata, "scope");
    let nodes = vec![ParsedNode { node_key:node_key.into(),parent_key:None,ordinal:0,title:title.into(),
        section_path:format!("{source_table}/{source_id}"), content:content.into(),content_start:0,content_end:content.len() as i32 }];
    write_nodes(&mut tx, source_table, source_id, &nodes, &scope, &row.get::<String,_>("storage_tier"),
        &checksum(content), row.get("updated_at"), &json!({"derived":true,"parser":"flat-v2"})).await?;
    tx.commit().await?;
    Ok(())
}

/// Evidence remains bound to the current authoritative source and its scope.
pub async fn document_evidence(pool: &PgPool, document_ids: &[Uuid], query: &str, scope: Option<&str>) -> Result<HashMap<Uuid, StructuralEvidence>> {
    if document_ids.is_empty() { return Ok(HashMap::new()); }
    let rows = sqlx::query(
        "SELECT DISTINCT ON (n.source_id) n.id,n.source_id,n.section_path,n.root_scope,n.content,n.source_checksum,n.observed_at, \
                ts_rank_cd(n.fts,websearch_to_tsquery('english',$2)) AS rank \
         FROM content_nodes n JOIN documents d ON n.source_table='documents' AND d.id=n.source_id \
         WHERE n.storage_tier='active' AND d.storage_tier='active' AND n.source_id=ANY($1) \
           AND n.source_checksum=encode(sha256(convert_to(d.content,'UTF8')),'hex') \
           AND n.root_scope=coalesce(nullif(btrim(d.frontmatter->>'vault_scope'),''),'unclassified') \
           AND n.content <> '' AND n.fts @@ websearch_to_tsquery('english',$2) AND ($3::text IS NULL OR n.root_scope=$3) \
         ORDER BY n.source_id,rank DESC,n.ordinal DESC",
    ).bind(document_ids).bind(query).bind(scope).fetch_all(pool).await?;
    let mut evidence = HashMap::new();
    for row in rows {
        evidence.insert(row.try_get("source_id")?, StructuralEvidence {
            node_id:row.try_get("id")?,section_path:row.try_get("section_path")?,root_scope:row.try_get("root_scope")?,
            score_boost:f64::from(row.try_get::<f32,_>("rank")?).min(0.25),excerpt:bounded_excerpt(&row.try_get::<String,_>("content")?),
            source_checksum:row.try_get("source_checksum")?,observed_at:row.try_get("observed_at")?,
        });
    }
    Ok(evidence)
}

fn allows_refinement(query: &str) -> bool {
    query.len() <= 4096 && !query.contains(['"', '\''])
        && !query.split_whitespace().any(|word| word.starts_with('-') || word.eq_ignore_ascii_case("or"))
}

/// One lexical retry inside an explicit scope. The candidate scan, returned
/// nodes, excerpts and query terms are bounded; source checks are never relaxed.
async fn refined_documents(
    pool: &PgPool, query: &str, filters: &SearchFilters,
) -> Result<(Vec<SearchResult>, HashMap<Uuid, StructuralEvidence>)> {
    let rows = sqlx::query(r#"
        WITH terms AS MATERIALIZED (
            SELECT unnest(tsvector_to_array(to_tsvector('english', $1))) AS term
        ), q AS (
            SELECT to_tsquery('simple', string_agg(quote_literal(term), ' | ')) AS search,
                   count(*) AS term_count FROM terms
        ), candidates AS MATERIALIZED (
            SELECT n.*, d.path AS source_path, q.term_count,
                   ts_rank_cd(n.fts, q.search, 32) AS lexical_rank
            FROM content_nodes n
            JOIN documents d ON n.source_table='documents' AND d.id=n.source_id
            CROSS JOIN q
            WHERE q.term_count BETWEEN 1 AND 24 AND n.fts @@ q.search
              AND n.content <> '' AND n.storage_tier='active' AND d.storage_tier='active'
              AND n.root_scope=$2
              AND n.root_scope=coalesce(nullif(btrim(d.frontmatter->>'vault_scope'),''),'unclassified')
              AND ($3::timestamptz IS NULL OR d.created_at >= $3)
              AND ($4::timestamptz IS NULL OR d.created_at <= $4)
            ORDER BY lexical_rank DESC, n.id LIMIT 120
        ), checked AS (
            SELECT n.*, matches.coverage
            FROM candidates n JOIN documents d ON d.id=n.source_id
            CROSS JOIN LATERAL (
                SELECT count(*) AS coverage FROM terms
                WHERE to_tsvector('english', left(n.content,200000))
                    @@ to_tsquery('simple', quote_literal(term))
            ) matches
            WHERE n.source_checksum=encode(sha256(convert_to(d.content,'UTF8')),'hex')
              AND matches.coverage >= greatest(1, (n.term_count + 1) / 2)
        ), ranked AS (
            SELECT *, (0.5 * coverage::float8 / term_count
                + least(lexical_rank::float8, 0.15)
                + 0.01 * least(cardinality(string_to_array(section_path, ' > ')), 6)
                + CASE WHEN lower(title)=lower($1) THEN 0.15 ELSE 0 END) AS score
            FROM checked
        ), best AS (
            SELECT DISTINCT ON (source_id) * FROM ranked
            ORDER BY source_id, score DESC, ordinal DESC, id
        )
        SELECT * FROM best ORDER BY score DESC, source_id LIMIT 30
    "#)
        .bind(query).bind(filters.scope.as_deref()).bind(filters.start_date).bind(filters.end_date)
        .fetch_all(pool).await?;
    let mut documents = Vec::new();
    let mut evidence = HashMap::new();
    for row in rows {
        let id: Uuid = row.try_get("source_id")?;
        let score: f64 = row.try_get("score")?;
        let excerpt = bounded_excerpt(&row.try_get::<String, _>("content")?);
        documents.push(SearchResult {id, content:excerpt.clone(), source_info:row.try_get("source_path")?, score,
            vec_rank:None,kw_rank:None,decay_factor:None});
        evidence.insert(id, StructuralEvidence {
            node_id:row.try_get("id")?,section_path:row.try_get("section_path")?,root_scope:row.try_get("root_scope")?,
            score_boost:score,excerpt,source_checksum:row.try_get("source_checksum")?,observed_at:row.try_get("observed_at")?,
        });
    }
    Ok((documents,evidence))
}

pub async fn rerank_documents(
    pool: &PgPool, mut documents: Vec<SearchResult>, query: &str,
    filters: Option<&SearchFilters>, limit: usize,
) -> Result<StructuralRetrieval> {
    documents.truncate(30);
    let scope = filters.and_then(|f| f.scope.as_deref());
    let ids: Vec<_> = documents.iter().map(|d| d.id).collect();
    let mut evidence = document_evidence(pool, &ids, query, scope).await?;
    let refinement_used = evidence.is_empty() && scope.is_some() && allows_refinement(query);
    if refinement_used {
        (documents, evidence) = refined_documents(pool, query, filters.expect("scope requires filters")).await?;
    } else {
        documents.retain(|d| evidence.contains_key(&d.id));
        for d in &mut documents {
            let e = &evidence[&d.id];
            d.score += e.score_boost;
            d.content = e.excerpt.clone();
        }
    }
    documents.sort_by(|a,b| b.score.total_cmp(&a.score).then(a.id.cmp(&b.id)));
    documents.truncate(limit.min(30));
    evidence.retain(|id,_| documents.iter().any(|d| d.id==*id));
    Ok(StructuralRetrieval {documents,evidence,refinement_used})
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn parser_partitions_crlf_unicode_and_ignores_code_fences() {
        let input = "Intro é\r\n# One\r\n```md\r\n# Not a heading\r\n```\r\n## Two\r\nText";
        let nodes = parse_markdown("note.md", "Note", input);
        assert_eq!(nodes.len(),3);
        assert_eq!(nodes[2].parent_key.as_ref(),Some(&nodes[1].node_key));
        assert_eq!(nodes.iter().map(|n| n.content.as_str()).collect::<String>(),input);
        for n in nodes { assert_eq!(&input[n.content_start as usize..n.content_end as usize],n.content); }
    }
    #[test]
    fn unrelated_heading_insert_preserves_identity() {
        let a=parse_markdown("a","A","# Existing\nText\n## Child\nValue");
        let b=parse_markdown("a","A","# New\nPrelude\n# Existing\nText\n## Child\nValue");
        assert_eq!(a[1].node_key,b[2].node_key);
        assert_eq!(a[2].node_key,b[3].node_key);
        let source=Uuid::new_v4();
        assert_eq!(stable_id("documents",source,&a[2].node_key),stable_id("documents",source,&b[3].node_key));
    }
    #[test]
    fn duplicate_headings_have_unique_keys() {
        let n=parse_markdown("a","A","# Duplicate\nA\n# Duplicate\nB");
        assert_ne!(n[1].node_key,n[2].node_key);
    }
    #[test]
    fn scope_requires_explicit_provenance() {
        assert_eq!(explicit_scope(&json!({"source_path":"/Main Vault/Law Modules Vault/note.md"}),"vault_scope"),"unclassified");
    }
    #[test]
    fn excerpts_are_utf8_safe_and_bounded() {
        assert_eq!(bounded_excerpt(&"é".repeat(1200)),"é".repeat(1200));
        assert_eq!(bounded_excerpt(&"é".repeat(1201)),"é".repeat(1200)+"...");
    }
    #[test]
    fn refinement_preserves_explicit_query_constraints() {
        assert!(allows_refinement("When can raw records be compacted?"));
        for query in ["scope -private", "\"exact phrase\"", "scope OR archive"] {
            assert!(!allows_refinement(query));
        }
        assert!(!allows_refinement(&"x".repeat(4097)));
    }
}

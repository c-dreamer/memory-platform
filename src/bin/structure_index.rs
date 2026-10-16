//! Bounded hierarchy maintenance. Credentials come only from the environment.
use anyhow::{ensure, Context, Result};
use clap::{Parser, ValueEnum};
use memory_platform::{migrations::Migrator, services::structure};
use serde_json::{json, Value};
use sqlx::{postgres::PgPoolOptions, Row};
use uuid::Uuid;

#[derive(Parser)]
#[command(name="structure-index",about="Inspect or rebuild a bounded page of derived hierarchy nodes")]
struct Args {
    #[arg(long)] scope: Option<String>,
    /// Maximum source rows per source in this invocation (1..500).
    #[arg(long,default_value_t=100)] limit: i64,
    /// Resume after this source ID, reported by the previous page.
    #[arg(long)] after: Option<Uuid>,
    #[arg(long)] apply: bool,
    /// Explicitly apply ordered local migrations before indexing.
    #[arg(long,conflicts_with_all=["list_scopes","list_vault_roots"])] migrate: bool,
    #[arg(long,conflicts_with="apply")] list_scopes: bool,
    #[arg(long,conflicts_with="apply")] list_vault_roots: bool,
    #[arg(long,value_enum,default_value="documents")] source: Vec<Source>,
}
#[derive(Clone,ValueEnum)]
enum Source { Documents, Memories, Sessions }

#[tokio::main]
async fn main() -> Result<()> {
    dotenvy::dotenv().ok();
    let args=Args::parse();
    ensure!((1..=500).contains(&args.limit),"limit must be between 1 and 500");
    let url=std::env::var("DATABASE_URL").context("DATABASE_URL must be set in the protected environment file")?;
    let pool=PgPoolOptions::new().max_connections(2).acquire_timeout(std::time::Duration::from_secs(5)).connect(&url).await?;
    if args.list_scopes || args.list_vault_roots {
        let rows=sqlx::query("SELECT coalesce(nullif(btrim(frontmatter->>'vault_scope'),''),'unclassified') AS scope,count(*)::bigint AS records FROM documents GROUP BY 1 ORDER BY 1").fetch_all(&pool).await?;
        for row in rows { println!("{}",json!({"scope":row.get::<String,_>("scope"),"records":row.get::<i64,_>("records")})); }
        return Ok(());
    }
    if args.migrate { Migrator::run(&pool).await?; }
    for source in &args.source {
        let (table, scope_sql)=match source {
            Source::Documents=>("documents","coalesce(nullif(btrim(frontmatter->>'vault_scope'),''),'unclassified')"),
            Source::Memories=>("memories","coalesce(nullif(btrim(metadata->>'scope'),''),'unclassified')"),
            Source::Sessions=>("sessions","'unclassified'::text"),
        };
        // A dry run transfers only identities and estimated bytes, never source bodies.
        let sql=format!("SELECT id,pg_column_size(t)::bigint AS bytes FROM {table} t WHERE ($1::uuid IS NULL OR id>$1) AND ($2::text IS NULL OR {scope_sql}=$2) ORDER BY id LIMIT $3");
        let rows=sqlx::query(&sql).bind(args.after).bind(&args.scope).bind(args.limit).fetch_all(&pool).await?;
        let selected=rows.len();
        let bytes: i64=rows.iter().map(|r| r.get::<i64,_>("bytes")).sum();
        let mut cursor=args.after;
        let mut node_count=0;
        for row in rows {
            let id: Uuid=row.get("id");
            if args.apply {
                let sql=format!("SELECT to_jsonb(t)-'embedding'-'fts' AS source FROM {table} t WHERE id=$1");
                let value: Value=sqlx::query_scalar(&sql).bind(id).fetch_one(&pool).await?;
                let tier=value["storage_tier"].as_str().unwrap_or("active");
                match source {
                    Source::Documents=>{
                        let scope=structure::explicit_scope(&value["frontmatter"],"vault_scope");
                        node_count+=structure::rebuild_document_nodes(&pool,id,value["path"].as_str().unwrap_or(""),value["title"].as_str().unwrap_or(""),value["content"].as_str().unwrap_or(""),None,None,&scope,tier).await?;
                    }
                    _=>{
                        let (key,title,content)=match source {
                            Source::Memories=>("memory",value["content_type"].as_str().unwrap_or("note"),value["content"].as_str().unwrap_or("")),
                            _=>("session",value["goal"].as_str().unwrap_or("Session"),value["summary"].as_str().or(value["goal"].as_str()).unwrap_or("")),
                        };
                        structure::rebuild_flat_node(&pool,table,id,key,"unclassified",title,content,tier,None,json!({})).await?;
                        node_count+=1;
                    }
                }
            }
            cursor=Some(id);
        }
        println!("{}",json!({"source":table,"selected":selected,"estimated_source_bytes":bytes,"apply":args.apply,"nodes":node_count,"after":cursor,"page_full":selected==args.limit as usize,"source_records_modified":false,"migrations_requested":args.migrate}));
    }
    Ok(())
}

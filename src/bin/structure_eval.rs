//! Synthetic retrieval smoke evaluation. Gold IDs never constrain retrieval.
//! Uses only STRUCTURE_TEST_DATABASE_URL, never the production .env file.
use anyhow::{ensure, Result};
use clap::Parser;
use memory_platform::{config::Config, db::postgres::{PostgresDb,SearchFilters}, migrations::Migrator, services::structure};
use serde::Deserialize;
use serde_json::json;
use std::time::Instant;
use uuid::Uuid;

#[derive(Parser)]
struct Args { #[arg(long)] confirm_test_database:bool }
#[derive(Deserialize)]
struct Fixtures { documents:Vec<Fixture> }
#[derive(Deserialize)]
struct Fixture { title:String,content:String,scope:String,query:String,expected_section:String }

#[tokio::main]
async fn main()->Result<()> {
    let args=Args::parse();
    ensure!(args.confirm_test_database,"requires --confirm-test-database");
    let url=std::env::var("STRUCTURE_TEST_DATABASE_URL")?;
    let options:sqlx::postgres::PgConnectOptions=url.parse()?;
    ensure!(matches!(options.get_host(),"127.0.0.1"|"localhost"|"::1"),"evaluation database must be local");
    ensure!(options.get_database().is_some_and(|n|n.starts_with("memory_structure_test")),"evaluation requires a dedicated test database");
    let fixtures:Fixtures=serde_json::from_str(include_str!("../../benches/fixtures/structural_eval.json"))?;
    let config=std::sync::Arc::new(Config {database_url:url,..Config::default()});
    let db=std::sync::Arc::new(PostgresDb::connect(&config).await?);
    let search=memory_platform::search::SearchEngine::new(db.clone(),config);
    Migrator::run(&db.pool).await?;
    let mut ids=Vec::new();
    let outcome:Result<()>=async {
        for f in &fixtures.documents {
            let id=Uuid::new_v4();
            let path=format!("fixture://structure-eval/{id}.md");
            sqlx::query("INSERT INTO documents(id,path,title,content,frontmatter) VALUES($1,$2,$3,$4,$5)")
                .bind(id).bind(&path).bind(&f.title).bind(&f.content).bind(json!({"vault_scope":f.scope})).execute(&db.pool).await?;
            ids.push(id);
            structure::rebuild_document_nodes(&db.pool,id,&path,&f.title,&f.content,None,None,&f.scope,"active").await?;
        }
        let mut leaf_hits=0;
        let mut recall=[0usize;3];
        let mut reciprocal=0.0;
        let mut timings=Vec::new();
        for (f,gold) in fixtures.documents.iter().zip(&ids) {
            let started=Instant::now();
            let filter=SearchFilters {scope:Some(f.scope.clone()),..Default::default()};
            let candidates=search.hybrid_search_filtered("documents",&f.query,&[],"keyword",30,Some(&filter)).await?;
            let result=structure::rerank_documents(&db.pool,candidates,&f.query,Some(&filter),30).await?;
            let ranked=result.documents;
            let evidence=result.evidence;
            let rank=ranked.iter().position(|r|r.id==*gold).map(|r|r+1);
            for (i,k) in [1,5,10].iter().enumerate() {recall[i]+=usize::from(rank.is_some_and(|r|r<=*k));}
            reciprocal+=rank.map_or(0.0,|r|1.0/r as f64);
            let leaf=evidence.get(gold).is_some_and(|e|e.section_path.ends_with(&f.expected_section));
            leaf_hits+=usize::from(leaf);
            timings.push(started.elapsed().as_secs_f64()*1000.0);
            println!("{}",json!({"case":f.title,"document_rank":rank,"expected_leaf_found":leaf,"candidate_count":ranked.len(),"refinement_used":result.refinement_used}));
        }
        timings.sort_by(f64::total_cmp);
        let n=ids.len();
        println!("{}",json!({"suite":"synthetic-smoke-v2","queries":n,"recall_at_1":recall[0] as f64/n as f64,"recall_at_5":recall[1] as f64/n as f64,"recall_at_10":recall[2] as f64/n as f64,"mrr":reciprocal/n as f64,"correct_leaves":leaf_hits,"p50_ms":timings[n/2],"p95_ms":timings[n-1],"release_evaluation":false}));
        ensure!(leaf_hits==n,"retrieval smoke gate failed; leave structural ranking opt-in");
        Ok(())
    }.await;
    // The finally-style cleanup also runs after a failed query or assertion.
    sqlx::query("DELETE FROM documents WHERE id=ANY($1)").bind(&ids).execute(&db.pool).await?;
    db.pool.close().await;
    outcome
}

//! Real PostgreSQL checks. Deliberately ignores DATABASE_URL and project .env.
use anyhow::{ensure, Result};
use memory_platform::{config::Config, db::postgres::{PostgresDb, SearchFilters}, migrations::Migrator, services::structure};
use serde_json::{json, Value};
use sqlx::Row;
use uuid::Uuid;

async fn seed_doc(db: &PostgresDb, scope: Option<&str>, content: &str) -> Result<(Uuid,String)> {
    let id=Uuid::new_v4();
    let path=format!("fixture://{id}.md");
    sqlx::query("INSERT INTO documents(id,path,title,content,frontmatter) VALUES($1,$2,'Fixture',$3,$4)")
        .bind(id).bind(&path).bind(content).bind(json!({"vault_scope":scope})).execute(&db.pool).await?;
    Ok((id,path))
}

async fn index(db: &PostgresDb,id:Uuid,path:&str,body:&str,scope:&str)->Result<usize> {
    structure::rebuild_document_nodes(&db.pool,id,path,"Fixture",body,None,None,scope,"active").await
}

#[tokio::test]
#[ignore = "requires isolated localhost STRUCTURE_TEST_DATABASE_URL"]
async fn hierarchy_preserves_data_and_scope_in_postgres() -> Result<()> {
    let url=std::env::var("STRUCTURE_TEST_DATABASE_URL")?;
    let options: sqlx::postgres::PgConnectOptions=url.parse()?;
    ensure!(matches!(options.get_host(),"localhost"|"127.0.0.1"|"::1"),"test database must be local");
    ensure!(options.get_database().is_some_and(|s| s.starts_with("memory_structure_test")),"test database name must start with memory_structure_test");
    let config=Config {database_url:url, ..Config::default()};
    let db=PostgresDb::connect(&config).await?;
    Migrator::run(&db.pool).await?;

    let body="Intro é\r\n# Isolation\r\nsynthetic isolation marker\r\n## Detail\r\nExact evidence\r\n";
    let (main,path)=seed_doc(&db,Some("Main Vault"),body).await?;
    let (law,law_path)=seed_doc(&db,Some("Law Modules Vault"),body).await?;
    let (unknown,unknown_path)=seed_doc(&db,None,body).await?;
    let before:Value=sqlx::query_scalar("SELECT to_jsonb(d) FROM documents d WHERE id=$1").bind(main).fetch_one(&db.pool).await?;
    index(&db,main,&path,body,"Main Vault").await?;
    index(&db,law,&law_path,body,"Law Modules Vault").await?;
    index(&db,unknown,&unknown_path,body,"unclassified").await?;
    let snapshot:Value=sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(n) ORDER BY node_key) FROM content_nodes n WHERE source_id=$1").bind(main).fetch_one(&db.pool).await?;
    index(&db,main,&path,body,"Main Vault").await?;
    let repeat:Value=sqlx::query_scalar("SELECT jsonb_agg(to_jsonb(n) ORDER BY node_key) FROM content_nodes n WHERE source_id=$1").bind(main).fetch_one(&db.pool).await?;
    ensure!(snapshot==repeat,"unchanged rebuild changed IDs/timestamps or generated work");
    let after:Value=sqlx::query_scalar("SELECT to_jsonb(d) FROM documents d WHERE id=$1").bind(main).fetch_one(&db.pool).await?;
    ensure!(before==after,"rebuild changed the authoritative document");

    let filter=SearchFilters {scope:Some("Main Vault".into()), ..Default::default()};
    let keywords=db.bm25_search("documents","synthetic isolation marker",50,Some(&filter)).await?;
    ensure!(keywords.iter().any(|r|r.id==main),"keyword retrieval missed active scope");
    ensure!(!keywords.iter().any(|r|r.id==law||r.id==unknown),"keyword scope leak");
    for id in [main,law,unknown] {
        sqlx::query("INSERT INTO embeddings(source_table,source_id,embedding,model,dimension) VALUES('documents',$1,'[1,0]'::vector,'test-2d',2)")
            .bind(id).execute(&db.pool).await?;
    }
    let vectors=db.vector_search("documents",&[1.0,0.0],"test-2d",50,0.0,Some(&filter)).await?;
    ensure!(vectors.iter().any(|r|r.id==main),"vector filter/bind failed");
    ensure!(!vectors.iter().any(|r|r.id==law||r.id==unknown),"vector scope leak");
    let e=structure::document_evidence(&db.pool,&[main,law,unknown],"synthetic isolation marker",Some("Main Vault")).await?;
    ensure!(e.len()==1&&e.contains_key(&main),"section evidence scope leak");
    ensure!(e[&main].section_path.ends_with("Isolation"),"wrong section selected");

    let amended=body.replace("# Isolation","# Newly inserted\r\nPreamble\r\n# Isolation");
    let old_node=e[&main].node_id;
    sqlx::query("UPDATE documents SET content=$2 WHERE id=$1").bind(main).bind(&amended).execute(&db.pool).await?;
    ensure!(structure::document_evidence(&db.pool,&[main],"synthetic isolation marker",Some("Main Vault")).await?.is_empty(),"stale hierarchy returned");
    ensure!(index(&db,main,&path,body,"Main Vault").await.is_err(),"stale index writer accepted");
    index(&db,main,&path,&amended,"Main Vault").await?;
    ensure!(structure::document_evidence(&db.pool,&[main],"synthetic isolation marker",Some("Main Vault")).await?[&main].node_id==old_node,"unrelated heading insertion changed node identity");

    sqlx::query("UPDATE documents SET storage_tier='archived' WHERE id=$1").bind(main).execute(&db.pool).await?;
    index(&db,main,&path,&amended,"Main Vault").await?;
    let active:i64=sqlx::query_scalar("SELECT count(*) FROM content_nodes WHERE source_id=$1 AND storage_tier='active'").bind(main).fetch_one(&db.pool).await?;
    ensure!(active==0,"reindex reactivated archived source");
    ensure!(structure::document_evidence(&db.pool,&[main],"synthetic isolation marker",Some("Main Vault")).await?.is_empty(),"archived evidence returned");
    ensure!(!db.bm25_search("documents","synthetic isolation marker",50,Some(&filter)).await?.iter().any(|r|r.id==main),"archived source returned");

    let mem=db.store_memory("retained exact fact","decision",0.5,&[],&json!({"scope":"Main Vault"}),None,None,None,None,None).await?;
    let node:Value=sqlx::query_scalar("SELECT to_jsonb(n) FROM content_nodes n WHERE source_table='memories' AND source_id=$1").bind(mem.id).fetch_one(&db.pool).await?;
    ensure!(node["content"]=="retained exact fact","memory flat insert failed");
    structure::rebuild_flat_node(&db.pool,"memories",mem.id,"memory","ignored","decision","retained exact fact","active",None,json!({})).await?;
    let next:Value=sqlx::query_scalar("SELECT to_jsonb(n) FROM content_nodes n WHERE source_table='memories' AND source_id=$1").bind(mem.id).fetch_one(&db.pool).await?;
    ensure!(node==next,"flat reindex is not a no-op");
    let session=db.create_session(None,Some("Session goal"),None).await?;
    db.end_session(session.id,"Retained outcome").await?;
    let text:String=sqlx::query_scalar("SELECT content FROM content_nodes WHERE source_table='sessions' AND source_id=$1").bind(session.id).fetch_one(&db.pool).await?;
    ensure!(text=="Retained outcome","session source write did not refresh hierarchy");

    // Distinct files and vaults with the same body retain separate source IDs.
    let root=std::env::temp_dir().join(format!("hierarchy-vault-test-{}",Uuid::new_v4()));
    for vault in ["Main Vault","Law Modules Vault"] {
        std::fs::create_dir_all(root.join(vault))?;
        std::fs::write(root.join(vault).join("same.md"),"same exact content")?;
    }
    ensure!(memory_platform::ingest::vault::sync_vault(&db.pool,&root,1,false,Some("Main Vault")).await.is_err(),"parent scope bypass");
    for vault in ["Main Vault","Law Modules Vault"] {
        let result=memory_platform::ingest::vault::sync_vault(&db.pool,&root.join(vault),1,false,Some(vault)).await?;
        ensure!(result.errors.is_empty(),"vault hierarchy indexing failed");
    }
    let paths=sqlx::query("SELECT id,path FROM documents WHERE path IN ('vault://Main%20Vault/same.md','vault://Law%20Modules%20Vault/same.md')").fetch_all(&db.pool).await?;
    ensure!(paths.len()==2,"same-path cross-vault source collision");
    ensure!(paths[0].get::<Uuid,_>("id")!=paths[1].get::<Uuid,_>("id"),"checksum dedup conflated sources");
    std::fs::remove_dir_all(root)?;
    let db=std::sync::Arc::new(db);
    let state=memory_platform::AppState {
        config:config.clone(), db:db.clone(),
        search:std::sync::Arc::new(memory_platform::search::SearchEngine::new(db.clone(),std::sync::Arc::new(config))),
        neo4j_client:None,redis_cache:None,context_service:None,contradiction_detector:None,decay_engine:None,
        embedding_service:None,experience_service:None,ingestion_service:None,procedure_service:None,
        pending_writes:std::sync::Arc::new(memory_platform::queue::PendingWriteQueue::new_empty()),
    };
    let saved=memory_platform::mcp::tools::call_tool(&state,"memory_store",json!({"content":"mcpcontract uniquefact","scope":"Main Vault"})).await?;
    let saved:Value=serde_json::from_str(&saved)?;
    ensure!(saved["id"].is_string(),"MCP scoped write failed");
    let own=memory_platform::mcp::tools::call_tool(&state,"memory_search",json!({"query":"mcpcontract uniquefact","scope":"Main Vault"})).await?;
    let other=memory_platform::mcp::tools::call_tool(&state,"memory_search",json!({"query":"mcpcontract uniquefact","scope":"Law Modules Vault"})).await?;
    let own:Value=serde_json::from_str(&own)?; let other:Value=serde_json::from_str(&other)?;
    ensure!(own["memories"].as_array().is_some_and(|r|!r.is_empty()),"MCP cannot recall scoped write");
    ensure!(other["memories"].as_array().is_some_and(|r|r.is_empty()),"MCP scoped write leaked");
    ensure!(own["retrieval"]["mode"]=="baseline","unapproved ranking changed default");
    for value in [json!(""),json!("  "),json!(42)] {
        ensure!(memory_platform::mcp::tools::call_tool(&state,"memory_search",json!({"query":"mcpcontract","scope":value})).await.is_err(),"invalid scope broadened retrieval");
    }

    let refinement_body="# Archive Protocol\nArchives retain raw records locally.\n\n## Verification\nDo not compact any record before a verified archive bundle exists.";
    let (fixture,fixture_path)=seed_doc(&db,Some("Retrieval Test Scope"),refinement_body).await?;
    let (foreign,foreign_path)=seed_doc(&db,Some("Law Modules Vault"),refinement_body).await?;
    index(&db,fixture,&fixture_path,refinement_body,"Retrieval Test Scope").await?;
    index(&db,foreign,&foreign_path,refinement_body,"Law Modules Vault").await?;
    let request=json!({"query":"When can raw records be compacted?","scope":"Retrieval Test Scope","structural":true,"limit":1});
    let found:Value=serde_json::from_str(&memory_platform::mcp::tools::call_tool(&state,"memory_search",request.clone()).await?)?;
    ensure!(found["retrieval"]["refinement_used"]==true,"structural retry was not exercised");
    ensure!(found["documents"].as_array().is_some_and(|d|d.len()==1&&d[0]["id"]==fixture.to_string()),"retry failed or returned foreign document");
    ensure!(found["structural_evidence"][0]["section_path"].as_str().is_some_and(|s|s.ends_with("Verification")),"wrong refined section");
    let mut dated=request.clone();
    dated["start_date"]=json!((chrono::Utc::now()+chrono::Duration::days(1)).to_rfc3339());
    let future:Value=serde_json::from_str(&memory_platform::mcp::tools::call_tool(&state,"memory_search",dated).await?)?;
    ensure!(future["documents"].as_array().is_some_and(Vec::is_empty),"retry bypassed date filter");
    let absent:Value=serde_json::from_str(&memory_platform::mcp::tools::call_tool(&state,"memory_search",json!({"query":"xylophone quasar nonexistentfact","scope":"Retrieval Test Scope","structural":true})).await?)?;
    ensure!(absent["retrieval"]["insufficient_evidence"]==true,"unanswerable query did not abstain");
    sqlx::query("UPDATE documents SET content='changed source without old evidence' WHERE id=$1").bind(fixture).execute(&db.pool).await?;
    let stale:Value=serde_json::from_str(&memory_platform::mcp::tools::call_tool(&state,"memory_search",request.clone()).await?)?;
    ensure!(stale["documents"].as_array().is_some_and(Vec::is_empty),"retry returned stale source");
    sqlx::query("UPDATE documents SET content=$2,storage_tier='archived' WHERE id=$1").bind(fixture).bind(refinement_body).execute(&db.pool).await?;
    let archived:Value=serde_json::from_str(&memory_platform::mcp::tools::call_tool(&state,"memory_search",request).await?)?;
    ensure!(archived["documents"].as_array().is_some_and(Vec::is_empty),"retry returned archived source");
    sqlx::query("DELETE FROM documents WHERE id=ANY($1)").bind(vec![fixture,foreign]).execute(&db.pool).await?;
    println!("Verified: source preservation, stable identities/no-op rebuilds, scoped keyword/vector retrieval, stale-index rejection, archive tiers, flat memory/session SQL, vault source identity, MCP scoped store/recall, bounded refinement, and abstention.");
    db.pool.close().await;
    Ok(())
}

//! Local-only operational dashboard for the memory platform.

use std::{env, net::SocketAddr, path::PathBuf, process::Stdio, sync::Arc};

use anyhow::{Context, Result};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    response::Html,
    routing::{get, post},
    Json, Router,
};
use serde::Serialize;
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use tokio::process::Command;

#[derive(Clone)]
struct DashboardState {
    local: PgPool,
    neon: Option<PgPool>,
    root: PathBuf,
    state_dir: PathBuf,
}

#[derive(Serialize)]
struct DashboardStatus {
    queue_depth: i64,
    events_total: i64,
    events_published: i64,
    events_unpublished: i64,
    documents_active: i64,
    documents_archived: i64,
    last_success: Option<serde_json::Value>,
    paused: bool,
    retry_pending: bool,
    scheduler: &'static str,
    neon_reachable: bool,
    orbstack_containers: Vec<String>,
    automation_paths: Vec<String>,
    local_database_mb: f64,
    neon_database_mb: Option<f64>,
    pending_upload_bytes: i64,
    neon_documents: Option<i64>,
}

fn root() -> Result<PathBuf> {
    env::var("MEMORY_PLATFORM_ROOT")
        .map(PathBuf::from)
        .or_else(|_| {
            env::current_exe()?
                .parent()
                .and_then(|p| p.parent())
                .map(PathBuf::from)
                .context("cannot derive memory platform root")
        })
        .context("MEMORY_PLATFORM_ROOT is required")
}

fn state_dir() -> PathBuf {
    env::var("XDG_STATE_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".into())).join(".local/state"))
        .join("memory-platform")
}

async fn status(State(state): State<Arc<DashboardState>>) -> Result<Json<DashboardStatus>, StatusCode> {
    let queue_depth = sqlx::query_scalar("SELECT count(*) FROM sync_meta.outbox")
        .fetch_one(&state.local).await.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let (events_total, events_published): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE pushed_at IS NOT NULL) FROM sync_meta.events",
    ).fetch_one(&state.local).await.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let (documents_active, documents_archived): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE storage_tier='active'), count(*) FILTER (WHERE storage_tier='archived') FROM documents",
    ).fetch_one(&state.local).await.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let last_success = sqlx::query_scalar("SELECT state_value FROM sync_meta.state WHERE state_key='last_success'")
        .fetch_optional(&state.local).await.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let pause = state.state_dir.join("neon-maintenance.paused");
    let retry = state.state_dir.join("neon-maintenance.retry");
    let neon_reachable = match &state.neon {
        Some(pool) => tokio::time::timeout(std::time::Duration::from_secs(3), sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(pool)).await.is_ok(),
        None => false,
    };
    let local_database_mb: f64 = sqlx::query_scalar("SELECT pg_database_size(current_database())::float8 / 1024 / 1024")
        .fetch_one(&state.local).await.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let pending_upload_bytes: i64 = sqlx::query_scalar("SELECT coalesce(sum(pg_column_size(payload)),0)::bigint FROM sync_meta.events WHERE pushed_at IS NULL")
        .fetch_one(&state.local).await.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let (neon_database_mb, neon_documents) = match &state.neon {
        Some(pool) if neon_reachable => {
            let size = sqlx::query_scalar("SELECT pg_database_size(current_database())::float8 / 1024 / 1024").fetch_one(pool).await.ok();
            let documents = sqlx::query_scalar("SELECT count(*) FROM documents").fetch_one(pool).await.ok();
            (size, documents)
        }
        _ => (None, None),
    };
    let orbstack_containers = Command::new("docker").args(["ps", "-a", "--format", "{{.Names}} | {{.Image}} | {{.Status}} | {{.Ports}}"])
        .output().await.ok().and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.lines().map(str::to_string).collect()).unwrap_or_default();
    let automation_paths = if cfg!(target_os = "macos") {
        vec!["~/Library/LaunchAgents/com.memory-platform.neon-sync.plist".into(), "~/Library/LaunchAgents/com.memory-platform.neon-retry.plist".into(), "~/Library/LaunchAgents/com.memory-platform.dashboard.plist".into()]
    } else { vec!["~/.config/systemd/user/memory-platform-dashboard.service".into(), "~/.config/systemd/user/memory-platform-maintenance.service".into()] };
    Ok(Json(DashboardStatus {
        queue_depth,
        events_total,
        events_published,
        events_unpublished: events_total - events_published,
        documents_active,
        documents_archived,
        last_success,
        paused: pause.exists(),
        retry_pending: retry.exists(),
        scheduler: if cfg!(target_os = "macos") { "launchd" } else { "systemd user service" },
        neon_reachable,
        orbstack_containers,
        automation_paths,
        local_database_mb,
        neon_database_mb,
        pending_upload_bytes,
        neon_documents,
    }))
}

async fn control(State(state): State<Arc<DashboardState>>, Path(action): Path<String>) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    let pause = state.state_dir.join("neon-maintenance.paused");
    std::fs::create_dir_all(&state.state_dir).map_err(internal)?;
    match action.as_str() {
        "pause" => std::fs::write(&pause, "paused by dashboard\n").map_err(internal)?,
        "resume" | "start" => {
            let _ = std::fs::remove_file(&pause);
            scheduler(&state.root, "start").await.map_err(internal)?;
        }
        "stop" => {
            std::fs::write(&pause, "stopped by dashboard\n").map_err(internal)?;
            scheduler(&state.root, "stop").await.map_err(internal)?;
        }
        _ => return Err((StatusCode::BAD_REQUEST, "unknown action".into())),
    }
    Ok(Json(json!({"ok": true, "action": action})))
}

async fn scheduler(_root: &PathBuf, action: &str) -> Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        let uid = Command::new("id").arg("-u").output().await?.stdout;
        let uid = String::from_utf8(uid)?.trim().to_string();
        let mut c = Command::new("launchctl");
        if action == "stop" { c.args(["kill", "SIGTERM", &format!("gui/{uid}/com.memory-platform.neon-sync")]); }
        else { c.args(["kickstart", "-k", &format!("gui/{uid}/com.memory-platform.neon-sync")]); }
        c
    } else {
        let mut c = Command::new("systemctl");
        c.args(["--user", action, "memory-platform-maintenance.service"]);
        c
    };
    command.stdout(Stdio::null()).stderr(Stdio::null());
    let status = command.status().await?;
    anyhow::ensure!(status.success(), "scheduler action failed");
    Ok(())
}

fn internal(error: impl std::fmt::Display) -> (StatusCode, String) {
    (StatusCode::INTERNAL_SERVER_ERROR, error.to_string())
}

const PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Memory Platform</title><style>
:root{--ink:#17312c;--paper:#f5f0e5;--lime:#d2e84d;--coral:#f26b4c;--line:#b9b09c}*{box-sizing:border-box}body{margin:0;background:radial-gradient(circle at 85% 5%,#d2e84d 0 12%,transparent 32%),var(--paper);color:var(--ink);font:16px Georgia,serif}.wrap{max-width:1000px;margin:auto;padding:52px 22px}h1{font:700 clamp(2.4rem,7vw,5rem)/.9 Georgia;margin:0;letter-spacing:-.06em}.lead{max-width:650px;font-size:1.2rem}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(190px,1fr));gap:12px;margin:32px 0}.card{border:1px solid var(--line);padding:18px;background:#fffaf0;min-height:110px}.num{display:block;font:700 2.3rem/.9 ui-monospace,monospace;margin-top:12px}.actions{display:flex;flex-wrap:wrap;gap:10px}.actions button{font:700 1rem Georgia;padding:13px 18px;border:1px solid var(--ink);background:var(--lime);cursor:pointer}.actions button.stop{background:var(--coral)}#note{margin-top:20px;font-family:ui-monospace,monospace}small{font-family:ui-monospace,monospace}</style></head><body><main class="wrap"><small>LOCAL MEMORY OPERATIONS</small><h1>Memory, at a glance.</h1><p class="lead">This control panel stays on this device. It shows operational counts only, never credentials or raw session text.</p><section class="grid" id="cards"></section><section class="actions"><button onclick="act('start')">Start now</button><button onclick="act('pause')">Pause schedule</button><button onclick="act('resume')">Resume</button><button class="stop" onclick="act('stop')">Stop current run</button></section><p id="note">Loading status...</p></main><script>async function load(){let s=await fetch('/api/status').then(r=>r.json());let x=[['Queue',s.queue_depth],['Events waiting',s.events_unpublished],['Events published',s.events_published],['Active documents',s.documents_active],['Archived documents',s.documents_archived],['Schedule',s.paused?'PAUSED':s.retry_pending?'RETRY PENDING':'READY']];cards.innerHTML=x.map(([a,b])=>`<article class=card><small>${a}</small><strong class=num>${b}</strong></article>`).join('');note.textContent=`Scheduler: ${s.scheduler}. Last successful transfer: ${s.last_success?.at||'not recorded'}. Refreshes every 10 seconds.`}async function act(a){note.textContent='Applying '+a+'...';await fetch('/api/action/'+a,{method:'POST'});load()}load();setInterval(load,10000)</script></body></html>"#;

const OPERATIONS_PAGE: &str = r#"<!doctype html><html><head><meta charset="utf-8"><title>Memory Operations</title><style>body{margin:0;padding:32px;background:#101b1a;color:#f3f0e6;font:16px Georgia,serif}h1{font-size:42px}.grid{display:grid;grid-template-columns:repeat(auto-fit,minmax(190px,1fr));gap:12px;margin:28px 0}.card{background:#1d2c2a;border:1px solid #49645c;padding:16px}.label{color:#a9c6aa;font:12px ui-monospace}.value{display:block;font:28px ui-monospace;margin-top:10px}button{padding:11px 15px;margin:3px;background:#d4e958;color:#10201e;border:0;font-weight:bold}button.stop{background:#ff8064}pre{white-space:pre-wrap;background:#152321;padding:16px}</style></head><body><h1>Memory Operations</h1><p>Live local, Neon, OrbStack, and automation state.</p><div id="cards" class="grid"></div><div><button onclick="act('start')">Start</button><button onclick="act('pause')">Pause</button><button onclick="act('resume')">Resume</button><button class="stop" onclick="act('stop')">Stop</button></div><pre id="detail">Loading...</pre><script>async function load(){let s=await fetch('/api/status').then(r=>r.json());let v=[['Queue',s.queue_depth],['Unpublished events',s.events_unpublished],['Neon',s.neon_reachable?'Connected':'Unavailable'],['OrbStack containers',s.orbstack_containers.length],['Active documents',s.documents_active],['Scheduler',s.paused?'Paused':s.retry_pending?'Retry pending':'Ready']];cards.innerHTML=v.map(x=>'<article class=card><span class=label>'+x[0]+'</span><strong class=value>'+x[1]+'</strong></article>').join('');detail.textContent='Automation paths:\n'+s.automation_paths.join('\n')+'\n\nOrbStack:\n'+(s.orbstack_containers.join('\n')||'No containers reported')+'\n\nLast sync:\n'+JSON.stringify(s.last_success)}async function act(a){await fetch('/api/action/'+a,{method:'POST'});load()}load();setInterval(load,10000)</script></body></html>"#;

async fn operations_page() -> Html<String> {
    Html(format!(r#"{OPERATIONS_PAGE}<script>async function enrich(){{let s=await fetch('/api/status').then(r=>r.json());detail.textContent=`Database sizes:\nLocal: ${{s.local_database_mb.toFixed(2)}} MB\nNeon: ${{s.neon_database_mb===null?'unavailable':s.neon_database_mb.toFixed(2)+' MB'}}\n\nPending upload estimate: ${{s.pending_upload_bytes}} bytes\nDocument parity: local ${{s.documents_active}}, Neon ${{s.neon_documents??'unavailable'}}\n\nAutomation paths:\n${{s.automation_paths.join('\n')}}\n\nOrbStack containers:\n${{s.orbstack_containers.join('\n')||'No containers reported'}}`;}}enrich();setInterval(enrich,10000)</script>"#))
}

#[tokio::main]
async fn main() -> Result<()> {
    let database_url = env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let port = env::var("MEMORY_DASHBOARD_PORT").unwrap_or_else(|_| "8765".into()).parse::<u16>()?;
    let neon = env::var("NEON_SYNC_URL").ok().and_then(|url| PgPoolOptions::new().max_connections(1).connect_lazy(&url).ok());
    let state = Arc::new(DashboardState { local: PgPoolOptions::new().max_connections(2).connect(&database_url).await?, neon, root: root()?, state_dir: state_dir() });
    let app = Router::new().route("/", get(|| async { Html(PAGE) })).route("/operations", get(operations_page)).route("/api/status", get(status)).route("/api/action/{action}", post(control)).with_state(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    println!("Memory dashboard listening on http://{addr}");
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
}

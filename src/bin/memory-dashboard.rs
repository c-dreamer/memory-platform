//! Local-only operational dashboard for the memory platform.

use std::{collections::BTreeMap, env, net::SocketAddr, path::PathBuf, process::Stdio, sync::Arc};

use anyhow::{Context, Result};
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::Html,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use sqlx::{postgres::PgPoolOptions, PgPool};
use tokio::process::Command;

#[derive(Clone)]
struct DashboardState {
    local: PgPool,
    neon: Option<PgPool>,
    root: PathBuf,
    state_dir: PathBuf,
    token: String,
}

#[derive(Deserialize)]
struct AccessQuery {
    token: Option<String>,
}

#[derive(Serialize)]
struct ParityRow {
    label: &'static str,
    local: i64,
    neon: Option<i64>,
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
    source_parity: Vec<ParityRow>,
    next_action: String,
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
        .unwrap_or_else(|_| {
            PathBuf::from(env::var("HOME").unwrap_or_else(|_| ".".into())).join(".local/state")
        })
        .join("memory-platform")
}

fn authorized(
    headers: &HeaderMap,
    query: &AccessQuery,
    state: &DashboardState,
) -> Result<(), StatusCode> {
    let supplied = query.token.as_deref().or_else(|| {
        headers
            .get("x-memory-dashboard-token")
            .and_then(|value| value.to_str().ok())
    });
    if supplied == Some(state.token.as_str()) {
        Ok(())
    } else {
        Err(StatusCode::UNAUTHORIZED)
    }
}

async fn document_source_counts(pool: &PgPool) -> Result<BTreeMap<String, i64>, sqlx::Error> {
    let rows: Vec<(String, i64)> = sqlx::query_as(
        "SELECT CASE \
          WHEN path LIKE 'codex://%' OR coalesce(frontmatter->>'source','') LIKE 'codex%' THEN 'Codex' \
          WHEN path LIKE 'config://opencode%' OR path LIKE 'log://opencode%' OR coalesce(frontmatter->>'source','') LIKE 'opencode%' THEN 'OpenCode' \
          WHEN path LIKE 'vault://%' OR path LIKE 'obsidian://%' OR coalesce(frontmatter->>'source','') LIKE 'vault%' THEN 'Vault' \
          ELSE 'Other' END, count(*) \
         FROM documents WHERE storage_tier='active' GROUP BY 1",
    )
    .fetch_all(pool)
    .await?;
    Ok(rows.into_iter().collect())
}

async fn record_counts(pool: &PgPool) -> Result<(i64, i64, i64), sqlx::Error> {
    sqlx::query_as(
        "SELECT \
          count(*) FILTER (WHERE storage_tier='active'), \
          (SELECT count(*) FROM memories WHERE storage_tier='active'), \
          (SELECT count(*) FROM memories WHERE storage_tier='active' AND (importance >= 0.9 OR 'critical'=ANY(tags) OR metadata @> '{\"critical\":true}'::jsonb)) \
         FROM sessions",
    )
    .fetch_one(pool)
    .await
}

async fn source_parity(
    local: &PgPool,
    neon: Option<&PgPool>,
) -> Result<Vec<ParityRow>, sqlx::Error> {
    let local_documents = document_source_counts(local).await?;
    let (local_sessions, local_memories, local_critical) = record_counts(local).await?;
    let remote = match neon {
        Some(pool) => {
            let documents = document_source_counts(pool).await.ok();
            let records = record_counts(pool).await.ok();
            (documents, records)
        }
        None => (None, None),
    };
    let remote_document = |name: &str| {
        remote
            .0
            .as_ref()
            .and_then(|counts| counts.get(name).copied())
    };
    Ok(vec![
        ParityRow {
            label: "Vault documents",
            local: *local_documents.get("Vault").unwrap_or(&0),
            neon: remote_document("Vault"),
        },
        ParityRow {
            label: "Codex documents",
            local: *local_documents.get("Codex").unwrap_or(&0),
            neon: remote_document("Codex"),
        },
        ParityRow {
            label: "OpenCode documents",
            local: *local_documents.get("OpenCode").unwrap_or(&0),
            neon: remote_document("OpenCode"),
        },
        ParityRow {
            label: "Sessions",
            local: local_sessions,
            neon: remote.1.map(|counts| counts.0),
        },
        ParityRow {
            label: "Memories",
            local: local_memories,
            neon: remote.1.map(|counts| counts.1),
        },
        ParityRow {
            label: "Critical records",
            local: local_critical,
            neon: remote.1.map(|counts| counts.2),
        },
    ])
}

async fn status(
    State(state): State<Arc<DashboardState>>,
    headers: HeaderMap,
    Query(query): Query<AccessQuery>,
) -> Result<Json<DashboardStatus>, StatusCode> {
    authorized(&headers, &query, &state)?;
    let queue_depth = sqlx::query_scalar("SELECT count(*) FROM sync_meta.outbox")
        .fetch_one(&state.local)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let (events_total, events_published): (i64, i64) = sqlx::query_as(
        "SELECT count(*), count(*) FILTER (WHERE pushed_at IS NOT NULL) FROM sync_meta.events",
    )
    .fetch_one(&state.local)
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let (documents_active, documents_archived): (i64, i64) = sqlx::query_as(
        "SELECT count(*) FILTER (WHERE storage_tier='active'), count(*) FILTER (WHERE storage_tier='archived') FROM documents",
    ).fetch_one(&state.local).await.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let last_success = sqlx::query_scalar(
        "SELECT state_value FROM sync_meta.state WHERE state_key='last_success'",
    )
    .fetch_optional(&state.local)
    .await
    .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let pause = state.state_dir.join("neon-maintenance.paused");
    let retry = state.state_dir.join("neon-maintenance.retry");
    let neon_reachable = match &state.neon {
        Some(pool) => tokio::time::timeout(
            std::time::Duration::from_secs(3),
            sqlx::query_scalar::<_, i32>("SELECT 1").fetch_one(pool),
        )
        .await
        .is_ok(),
        None => false,
    };
    let local_database_mb: f64 =
        sqlx::query_scalar("SELECT pg_database_size(current_database())::float8 / 1024 / 1024")
            .fetch_one(&state.local)
            .await
            .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let pending_upload_bytes: i64 = sqlx::query_scalar("SELECT coalesce(sum(pg_column_size(payload)),0)::bigint FROM sync_meta.events WHERE pushed_at IS NULL")
        .fetch_one(&state.local).await.map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let (neon_database_mb, usable_neon) = match &state.neon {
        Some(pool) if neon_reachable => {
            let size = sqlx::query_scalar(
                "SELECT pg_database_size(current_database())::float8 / 1024 / 1024",
            )
            .fetch_one(pool)
            .await
            .ok();
            (size, Some(pool))
        }
        _ => (None, None),
    };
    let source_parity = source_parity(&state.local, usable_neon)
        .await
        .map_err(|_| StatusCode::SERVICE_UNAVAILABLE)?;
    let orbstack_containers = Command::new("docker")
        .args([
            "ps",
            "-a",
            "--format",
            "{{.Names}} | {{.Image}} | {{.Status}} | {{.Ports}}",
        ])
        .output()
        .await
        .ok()
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.lines().map(str::to_string).collect())
        .unwrap_or_default();
    let automation_paths = if cfg!(target_os = "macos") {
        vec![
            "~/Library/LaunchAgents/com.memory-platform.neon-sync.plist".into(),
            "~/Library/LaunchAgents/com.memory-platform.neon-retry.plist".into(),
            "~/Library/LaunchAgents/com.memory-platform.dashboard.plist".into(),
        ]
    } else {
        vec![
            "~/.config/systemd/user/memory-platform-dashboard.service".into(),
            "~/.config/systemd/user/memory-platform-maintenance.service".into(),
        ]
    };
    let next_action = if pause.exists() {
        "Sync is paused. Resume only when you want background work to continue.".into()
    } else if !neon_reachable {
        "Neon is unavailable. The queue is safe locally; retry when the network recovers.".into()
    } else if queue_depth > 0 || pending_upload_bytes > 0 {
        format!("{} durable changes are waiting. Start a bounded sync; it will stop cleanly and resume later.", queue_depth)
    } else if retry.exists() {
        "A previous run requested a retry. Start a bounded sync when convenient.".into()
    } else {
        "No transfer is waiting. Run an audit only when you want to check source parity.".into()
    };
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
        scheduler: if cfg!(target_os = "macos") {
            "launchd"
        } else {
            "systemd user service"
        },
        neon_reachable,
        orbstack_containers,
        automation_paths,
        local_database_mb,
        neon_database_mb,
        pending_upload_bytes,
        source_parity,
        next_action,
    }))
}

async fn control(
    State(state): State<Arc<DashboardState>>,
    Path(action): Path<String>,
    headers: HeaderMap,
    Query(query): Query<AccessQuery>,
) -> Result<Json<serde_json::Value>, (StatusCode, String)> {
    authorized(&headers, &query, &state)
        .map_err(|_| (StatusCode::UNAUTHORIZED, "dashboard token required".into()))?;
    let pause = state.state_dir.join("neon-maintenance.paused");
    std::fs::create_dir_all(&state.state_dir).map_err(internal)?;
    match action.as_str() {
        "pause" => std::fs::write(&pause, "paused by dashboard\n").map_err(internal)?,
        "resume" | "start" | "retry" | "push_critical" => {
            let _ = std::fs::remove_file(&pause);
            scheduler(&state.root, "start").await.map_err(internal)?;
        }
        "stop" => {
            std::fs::write(&pause, "stopped by dashboard\n").map_err(internal)?;
            scheduler(&state.root, "stop").await.map_err(internal)?;
        }
        _ => return Err((StatusCode::BAD_REQUEST, "unknown action".into())),
    }
    Ok(Json(
        json!({"ok": true, "action": action, "note": "Only active, sync-eligible records are transferred. Archived raw content stays local."}),
    ))
}

async fn scheduler(_root: &PathBuf, action: &str) -> Result<()> {
    let mut command = if cfg!(target_os = "macos") {
        let uid = Command::new("id").arg("-u").output().await?.stdout;
        let uid = String::from_utf8(uid)?.trim().to_string();
        let mut c = Command::new("launchctl");
        if action == "stop" {
            c.args([
                "kill",
                "SIGTERM",
                &format!("gui/{uid}/com.memory-platform.neon-sync"),
            ]);
        } else {
            c.args([
                "kickstart",
                "-k",
                &format!("gui/{uid}/com.memory-platform.neon-sync"),
            ]);
        }
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

const OPERATIONS_PAGE: &str = r#"<!doctype html>
<html><head><meta charset="utf-8"><meta name="viewport" content="width=device-width,initial-scale=1">
<title>Memory Platform</title><style>
:root{--ink:#e8efe5;--muted:#9eb2a8;--paper:#11201d;--panel:#172b26;--panel2:#1e3730;--line:#35564a;--lime:#d5ed63;--coral:#ff9277;--amber:#ffc86a}*{box-sizing:border-box}body{margin:0;background:radial-gradient(ellipse at 88% 0,#52734d 0,transparent 37%),radial-gradient(ellipse at 4% 96%,#203f35 0,transparent 44%),#0d1916;color:var(--ink);font:16px Georgia,serif}.shell{max-width:1280px;margin:auto;padding:32px 24px 56px}.eyebrow,.kicker,.muted{font:12px ui-monospace,SFMono-Regular,monospace;letter-spacing:.08em;color:var(--muted);text-transform:uppercase}.head{display:flex;justify-content:space-between;align-items:end;gap:20px;border-bottom:1px solid var(--line);padding-bottom:25px}.head h1{font-size:clamp(2.7rem,7vw,5.5rem);line-height:.82;letter-spacing:-.065em;margin:8px 0 0}.state{min-width:235px;padding:14px 16px;border:1px solid var(--line);background:#13251f}.state b{display:block;color:var(--lime);font:700 13px ui-monospace,monospace;margin-top:6px}.next{margin:24px 0;padding:20px 22px;background:linear-gradient(110deg,#d5ed63,#dff08d);color:#17271f;border-radius:3px;display:flex;gap:18px;align-items:start}.next b{font:700 12px ui-monospace,monospace;letter-spacing:.08em}.next span{font-size:18px;line-height:1.35}.metrics{display:grid;grid-template-columns:repeat(4,minmax(0,1fr));gap:12px;margin:22px 0}.metric,.panel{background:linear-gradient(150deg,var(--panel2),var(--panel));border:1px solid var(--line);border-radius:4px}.metric{padding:18px;min-height:118px}.metric strong{display:block;margin-top:14px;font:700 clamp(1.7rem,3vw,2.5rem)/.9 ui-monospace,monospace}.metric .good{color:var(--lime)}.metric .warn{color:var(--amber)}.split{display:grid;grid-template-columns:1.45fr .85fr;gap:14px}.panel{padding:20px}.panel h2{font-size:24px;letter-spacing:-.03em;margin:4px 0 18px}.panel p{color:var(--muted);line-height:1.45}.table{width:100%;border-collapse:collapse;font:14px ui-monospace,monospace}.table th{text-align:left;color:var(--muted);font-weight:400}.table th,.table td{padding:11px 7px;border-bottom:1px solid #2c493f}.table td:last-child,.table th:last-child{text-align:right}.match{color:var(--lime)}.drift{color:var(--amber)}.actions{display:flex;flex-wrap:wrap;gap:8px}.actions button{appearance:none;border:1px solid #587565;background:#203a31;color:var(--ink);font:700 13px Georgia,serif;padding:11px 13px;cursor:pointer}.actions button.primary{background:var(--lime);color:#14241c;border-color:var(--lime)}.actions button.stop{background:transparent;color:var(--coral);border-color:var(--coral)}.actions button:hover{filter:brightness(1.12)}#notice{min-height:22px;color:var(--muted);font:13px ui-monospace,monospace}.compact{font:13px ui-monospace,monospace;white-space:pre-wrap;line-height:1.55;color:#c4d2c8}.foot{margin-top:16px;color:var(--muted);font:12px ui-monospace,monospace}@media(max-width:760px){.shell{padding:22px 15px}.head{align-items:start;flex-direction:column}.metrics{grid-template-columns:repeat(2,minmax(0,1fr))}.split{grid-template-columns:1fr}.next{flex-direction:column;gap:8px}.table{font-size:12px}}
</style></head><body><main class="shell"><header class="head"><div><div class="eyebrow">Private local control room</div><h1>Memory<br>Platform</h1></div><div class="state"><span class="kicker">Live state</span><b id="state">Checking local services</b></div></header><section class="next"><b>NEXT SAFE ACTION</b><span id="next">Reading the durable queue...</span></section><section class="metrics" id="metrics"></section><section class="split"><section class="panel"><span class="kicker">Projection coverage</span><h2>Local vs Neon</h2><p>Only active, sync-eligible records are expected in Neon. Archived raw content stays local by design.</p><table class="table"><thead><tr><th>Record group</th><th>Local</th><th>Neon</th></tr></thead><tbody id="parity"></tbody></table></section><aside class="panel"><span class="kicker">Controls</span><h2>Bounded work</h2><p>Start and retry use the existing resumable queue. Closing the app pauses background maintenance safely; every committed batch is preserved.</p><div class="actions"><button class="primary" onclick="act('start')">Start sync</button><button onclick="act('retry')">Retry now</button><button onclick="act('push_critical')">Push critical</button><button onclick="act('pause')">Pause</button><button onclick="act('resume')">Resume</button><button class="stop" onclick="act('stop')">Stop run</button></div><p id="notice">No action in progress.</p></aside></section><section class="split" style="margin-top:14px"><section class="panel"><span class="kicker">Transfer plan</span><h2>Data and freshness</h2><div class="compact" id="transfer">Loading...</div></section><aside class="panel"><span class="kicker">Local runtime</span><h2>OrbStack and automation</h2><div class="compact" id="runtime">Loading...</div></aside></section><div class="foot">Operational data only. No credentials or raw session content are shown. Refreshes every 10 seconds.</div></main><script>
const token=new URLSearchParams(location.search).get('token')||'';const auth={'X-Memory-Dashboard-Token':token};const fmt=n=>new Intl.NumberFormat().format(n||0);const mb=n=>n==null?'Unavailable':n.toFixed(2)+' MB';
async function load(){try{let r=await fetch('/api/status',{headers:auth});if(!r.ok)throw new Error(r.status===401?'Dashboard token rejected':'Local service unavailable');let s=await r.json();state.textContent=s.paused?'Paused':s.neon_reachable?'Ready for bounded work':'Neon unavailable';next.textContent=s.next_action;metrics.innerHTML=[['Projection queue',fmt(s.queue_depth),s.queue_depth?'warn':'good'],['Unpublished events',fmt(s.events_unpublished),s.events_unpublished?'warn':'good'],['Neon',''+(s.neon_reachable?'Connected':'Offline'),s.neon_reachable?'good':'warn'],['OrbStack',fmt(s.orbstack_containers.length)+' containers','good']].map(x=>`<article class="metric"><span class="kicker">${x[0]}</span><strong class="${x[2]}">${x[1]}</strong></article>`).join('');parity.innerHTML=s.source_parity.map(r=>{let d=r.neon===null?'unavailable':r.local-r.neon;return `<tr><td>${r.label}</td><td>${fmt(r.local)}</td><td class="${d===0?'match':'drift'}">${r.neon===null?'Unavailable':fmt(r.neon)+(d===0?'':' ('+(d>0?'+':'')+d+')')}</td></tr>`}).join('');transfer.textContent=`Local database: ${mb(s.local_database_mb)}\nNeon database: ${mb(s.neon_database_mb)}\nUnpublished event payload: ${fmt(s.pending_upload_bytes)} bytes\nLast successful transfer: ${s.last_success?.at||'Not recorded'}\nActive documents: ${fmt(s.documents_active)}\nArchived local documents: ${fmt(s.documents_archived)}`;runtime.textContent=`Scheduler: ${s.scheduler}\n\nOrbStack\n${s.orbstack_containers.join('\n')||'No containers reported'}\n\nAutomation definitions\n${s.automation_paths.join('\n')}`;}catch(e){state.textContent='Needs attention';notice.textContent=e.message}}
async function act(a){notice.textContent='Applying '+a+' safely...';try{let r=await fetch('/api/action/'+a,{method:'POST',headers:auth});let x=await r.json();if(!r.ok)throw new Error(x||'Action failed');notice.textContent=x.note||'Action accepted.';await load()}catch(e){notice.textContent='Action was not applied: '+e.message}}load();setInterval(load,10000);
</script></body></html>"#;

async fn page(
    State(state): State<Arc<DashboardState>>,
    headers: HeaderMap,
    Query(query): Query<AccessQuery>,
) -> Result<Html<&'static str>, StatusCode> {
    authorized(&headers, &query, &state)?;
    Ok(Html(OPERATIONS_PAGE))
}

#[tokio::main]
async fn main() -> Result<()> {
    let database_url = env::var("DATABASE_URL").context("DATABASE_URL is required")?;
    let port = env::var("MEMORY_DASHBOARD_PORT")
        .unwrap_or_else(|_| "8765".into())
        .parse::<u16>()?;
    let token = env::var("MEMORY_DASHBOARD_TOKEN").context("MEMORY_DASHBOARD_TOKEN is required")?;
    anyhow::ensure!(token.len() >= 32, "MEMORY_DASHBOARD_TOKEN is too short");
    let neon = env::var("NEON_SYNC_URL").ok().and_then(|url| {
        PgPoolOptions::new()
            .max_connections(1)
            .connect_lazy(&url)
            .ok()
    });
    let state = Arc::new(DashboardState {
        local: PgPoolOptions::new()
            .max_connections(2)
            .connect(&database_url)
            .await?,
        neon,
        root: root()?,
        state_dir: state_dir(),
        token,
    });
    let app = Router::new()
        .route("/", get(page))
        .route("/operations", get(page))
        .route("/api/status", get(status))
        .route("/api/action/{action}", post(control))
        .with_state(state);
    let addr = SocketAddr::from(([127, 0, 0, 1], port));
    println!("Memory dashboard listening on http://{addr}");
    axum::serve(tokio::net::TcpListener::bind(addr).await?, app).await?;
    Ok(())
}

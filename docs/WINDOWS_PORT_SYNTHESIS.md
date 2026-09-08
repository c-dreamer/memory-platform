# Memory Platform — Windows Deployment: Synthesized Architecture and Plan

Synthesized from 10 researched-and-verified dimensions plus direct re-verification against the repo and live CI (2026-09-08). Builds on and supersedes `docs/WINDOWS_PORT.md` (committed 2026-09-05, "planning and audit complete, no code changed yet") — that doc's 4 blockers, manual-task list, and work order are the skeleton this fleshes out. Two findings below were confirmed by reading the repo and CI directly during this synthesis, not sourced from any of the 10 dimensions: **CI is red today for two independent, already-existing reasons** (not a future Windows problem), and **`memory-dashboard.rs` is a third, ungated egress path to Neon**. Both are load-bearing for Sections 2 and 3.

---

## 1. Decisions

| # | Decision | Choice | Reason |
|---|---|---|---|
| 1 | Local embedding runtime | `llama.cpp` (`llama-server`, loopback HTTP) serving a **Qwen3-Embedding-4B** GGUF (Apache-2.0), MRL-truncated to 2048 dims and **renormalized in Rust** | Only Qwen3-Embedding-4B/8B are ≥2048-dim with sanctioned Matryoshka truncation ([huggingface.co/Qwen/Qwen3-Embedding-4B](https://huggingface.co/Qwen/Qwen3-Embedding-4B)); llama.cpp avoids Ollama's unresolved Windows RCE CVE pair (below) and shares one runtime with any future generation use |
| 2 | Local generation (chat) model | **Deferred, not built** | Confirmed by direct grep: the only `reqwest::Client` in the crate is `NvidiaNimEmbedding` (`src/services/embedding.rs:173`) — nothing in `src/` calls an LLM for text generation today. Building a generation pipeline nothing calls is unrequested scope (see Q2, §8) |
| 3 | Ollama | **Rejected** for both embedding and generation | CVE-2026-42248 (unsigned-update trust) + CVE-2026-42249 (path traversal → Startup-folder RCE) chain into zero-click persistent Windows RCE, versions 0.12.10–0.17.5, patch-release status unconfirmed as of this research; daemon/auto-updater model; and once Rust must defensively renormalize the MRL truncation anyway (see #1), Ollama's "convenience" over llama.cpp disappears |
| 4 | `fastembed` feature for embeddings | **Do not use** | Every one of its 46 built-in ONNX models tops out at ≤1024 dims — structurally cannot reach 2048 (`docs.rs/fastembed`); confirmed its own test doesn't compile: `embedding.rs:505` calls `LocalEmbedding::new(1000)` (1 arg) against the real `new(cache_size, expected_dimension)` signature at `embedding.rs:75` |
| 5 | Multi-model vector storage | Discriminator column (`model`, `dimension`) on the derived `public.embeddings` cache only, unconstrained `vector` type; **source tables keep fixed `VECTOR(2048)`**, left `NULL` for locally-embedded rows | pgvector's own documented pattern for mixed dims; touching all 7 fixed-width source-table columns is unnecessary — only the cache needs cross-model rows to coexist |
| 6 | Vector index (HNSW/IVFFlat) | **None yet** | pgvector caps both at 2000 dims regardless (`src/hnsw.h:19-20`, `src/ivfflat.h:37`, needs `halfvec` past that); actual row count is unmeasured — the oft-cited "3,022" is the **outbox backlog** at a paused checkpoint (`docs/OPERATIONS_STATUS.md:65-66`), not table size. Run `SELECT count(*) FROM memories` before deciding |
| 7 | Sync ordering (3rd device) | `neon_seq BIGSERIAL` on Neon's `sync_meta.events`, assigned inside the existing lease-serialized push transaction | `logical_time` is `nextval()` inside an `AFTER` trigger (`neon-sync.rs:358`) — non-transactional. `push_event_batch` orders by `created_at,event_id`, not `logical_time` (`neon-sync.rs:514`), so under the app's own 10-connection pool (`postgres.rs:161-162`) a fast-committing high-`logical_time` write can push before a slow-committing lower one — reproducing the exact permanent-skip bug this fix exists to close, just on a different column |
| 8 | Windows CI | Add a `windows-latest` leg, no `services:` block, DB-free tests only — **and delete the now-dead `services: postgres:` block from the existing matrix too** | **Directly confirmed live** (`gh run view 33951950565`): `macos-latest` fails today at "Set up job" — `docker: command not found` — because GitHub-hosted macOS runners have no Docker daemon and `services:` requires one. The 2 DB-gated tests are `#[ignore]` and CI never passes `--include-ignored`, so the container is provably unused on every OS already |
| 9 | Ubuntu CI unused-symbol errors | Delete `use chrono::Utc;` (`hooks.rs:250`); inline `config.expected_dimension.max(1)` at its two use sites instead of a shared `let` (`embedding.rs:428`) | **Directly confirmed live**: `cargo build --all-targets` under `RUSTFLAGS=-D warnings` fails on exactly these two, exit 101, blocking every push to `main` today, unrelated to Windows |
| 10 | Local Postgres on Windows | Native PostgreSQL 17 + pgvector, wrapped as a Windows service (WinSW — .NET already present) with SCM `depend=` ordering | Podman needs WSL2 (not installed) or Hyper-V, both with documented startup-ordering races; a native OS service dependency is the platform-native mechanism, and it sidesteps the Huntress/DefensX EDR-alert risk WSL2 was already flagged to trigger (`docs/WINDOWS_PORT.md`'s own manual-task list) |
| 11 | Redis / Neo4j on Windows | **Skip entirely** | Already optional — `main.rs:59-87` degrades gracefully on either being unreachable; skipping avoids Docker/Podman/WSL altogether and favors requirement 1 (fast/efficient on 15.4GB, no discrete GPU) |
| 12 | Degraded-startup fallback | Replace `PostgresDb::new_empty()` (`main.rs:45-57`) with bounded retry then hard exit | Its own doc comment says test-only; today the daemon reports started, binds its port, and fails every write silently |
| 13 | Offline write durability | Local SQLite (sqlx `sqlite` feature) WAL-mode pending-writes queue ahead of `POST /events` | Only option that prevents loss rather than reporting it faster; confirmed clean to add — `libsqlite3-sys` sits in `Cargo.lock` but `cargo tree -e normal -i libsqlite3-sys` prints nothing, i.e. it isn't in today's active build graph |
| 14 | `/health` status code | Return 503 when `db.health()` is false, not always 200 | A status-code-only supervisor (Task Scheduler, `curl -f`) cannot see "degraded" today (`root.rs:16-28`) |
| 15 | Full-offline / no-egress gate | Single `MEMORY_LOCAL_ONLY` bool, enforced at **three** independent points: `Config::load()` (hard exit), `EmbeddingServiceFactory::new` (delete the `Fallback`-wrapping branch structurally), and explicitly inside both `neon-sync.rs::main()` **and** `memory-dashboard.rs::main()` | Three real, independent egress call sites exist today — confirmed by direct read: `FallbackEmbedding` silently retries NVIDIA on any local error (`embedding.rs:382-397`), `neon-sync.rs` dials Neon directly, and **`memory-dashboard.rs:500-505` independently reads `NEON_SYNC_URL` and opens its own `PgPool::connect_lazy`**, bypassing both other gates entirely |
| 16 | Neon = optional cloud backup | `MEMORY_LOCAL_ONLY=1` is also the "Neon off" switch | One flag satisfies requirement 3 (offline) and requirement 4 (Neon optional) instead of two overlapping switches |
| 17 | Personal/work separation | `MEMORY_SCOPE=work\|personal` + category include/exclude, **orthogonal** to `MEMORY_LOCAL_ONLY`, both default-off | Local-only answers "does anything leave"; scope answers "which rows, to which project" — conflating them risks a forgotten category silently leaking client data |
| 18 | Archive encryption | `age` (Rust crate 0.12.1, MIT/Apache-2.0, confirmed against `str4d/rage`'s own `Cargo.toml`) compiled into a small new binary; keys in Keeper Secrets Manager | Only format here with default AEAD (ChaCha20-Poly1305) that's genuinely portable across Windows/macOS/Linux; gpg's AEAD is non-interoperable, `openssl enc` has none by the maintainers' own design, 7-Zip's is CRC32-only, DPAPI/EFS are Windows-only |
| 19 | Archive rollout | Gate behind `ARCHIVE_ENCRYPT=1` (default off); restore supports both legacy plaintext and new `.age` bundles | Requirement 5 (opt-in on running deployments); **confirmed `restore-archive-documents.sh` today does zero file I/O at all** — pure SQL state-flip (`storage_tier` → `active`) — the whole decrypt+verify path is net-new, not an extension |
| 20 | MCP transport | Keep stdio; do not build out `transport-http` | Spec guidance: stdio implementations should pull creds from env, not run OAuth — matches this repo's wrapper-sources-the-env-file pattern already; `transport-http` is an empty scaffold (`transport.rs`, no session ID/SSE/version header) and Zed has no remote MCP support at all, so HTTP could never fully replace stdio anyway |
| 21 | MCP protocol/tool bugs | Fix the hardcoded `protocolVersion: "2025-03-26"` echo (`mod.rs:194`) and the stale `tools.len()==17` assertions (`mod.rs:320`, `tools.rs:1387`) against the real count of 18, in the same pass as the Windows work | Confirmed real precedent (thingsboard-mcp#35) of a stale echoed protocolVersion silently breaking `tools/call` after a successful `initialize` on a Claude client family — not cosmetic |
| 22 | Scheduling | Checked-in Task Scheduler XML templates + PowerShell installer, mirroring `launchd/`/`systemd/`; `StartWhenAvailable=true`, battery gates off, `MultipleInstances=IgnoreNew`, dashboard job gets `ExecutionTimeLimit=PT0S` | Task Scheduler's real defaults (battery-gated, no catch-up, 72h execution cap) silently break laptop parity with `launchd` otherwise |
| 23 | Scheduled jobs under local-only | Do not install the 4 Neon-related tasks (`neon-sync`, `neon-retry`, `neon-reconcile`, `neon-count-audit`) when `MEMORY_LOCAL_ONLY=1` | They have nothing to do; installing them anyway either no-ops forever (confusing) or, if the gate has a bug, is one more place it could be bypassed |
| 24 | Manual installs dropped from `docs/WINDOWS_PORT.md` | **Do not** `wsl --install` or install Podman Desktop | Superseded by #10; also removes the WSL2/Huntress/DefensX EDR-alert risk Caleb himself flagged |

---

## 2. Blocker resolutions

### A. Local/offline embeddings — 2048-dim mismatch

**Fix:** Replace (not patch) the fastembed path with an `llama.cpp`-backed `EmbeddingServiceFactory` variant, structurally cloned from `NvidiaNimEmbedding`.

**Diff sketch:**
- No new Cargo dependency — `llama-server` is an external process; reuse the existing `reqwest`+`rustls` client pattern.
- New variant in `src/services/embedding.rs`, cloning the shape of `NvidiaNimEmbedding` (lines 152–284: retry/backoff, `parse_embedding_response`, chunk-and-average): POST to `http://127.0.0.1:<port>/v1/embeddings` (or `/embedding` — confirm llama-server's exact route by hitting a running instance before committing), truncate the returned 2560-dim vector to `expected_dimension` if needed, then **L2-renormalize in Rust regardless of server behavior** — the same ~5-line pattern already at `embedding.rs:206-210`. Don't trust an unverified server-side truncate/renormalize on a correctness-sensitive path (this generalizes the "don't trust Ollama's undocumented `dimensions` renorm" finding to whichever server is used).
- `EmbeddingConfig.model` gains a third value alongside `"local"`/`"nvidia"` in the match at `embedding.rs:429`.
- New migration (`013_embeddings_multi_model.sql`): relax `public.embeddings.embedding` from `VECTOR(2048)` to unconstrained `vector`; add `model TEXT NOT NULL`, backfilled to the existing `nvidia/llama-nemotron-embed-1b-v2` default for pre-existing rows (same pattern migration 007 already uses for a rejection-not-discard backfill); add `CHECK (vector_dims(embedding) = dimension)`; widen the unique index from `(source_table, source_id)` to `(source_table, source_id, model)`.
- `store_embedding` (`postgres.rs:1131`) must bind the real `model` value instead of relying on the column default; `vector_search` and the pairwise-similarity query (`postgres.rs:623-626,741-747`) each gain `AND e.model = $n`.
- **Required companion, not optional:** `rebuild_derived` (`neon-sync.rs:1052-1080`) reconstructs `documents`/`memories`/`experiences` cache rows from those tables' own `VECTOR(2048)` columns — which are `NULL` for locally-embedded rows by design (decision #5). Its existing "future source types" carry-through loop (`:1064-1071`) explicitly skips those three tables. Extend that skip to also preserve any cache row whose `model` isn't the cloud default, or every local-model embedding for those three tables is silently deleted on the next scheduled rebuild-derived run.
- Delete the dead `#[cfg(feature="fastembed")] LocalEmbedding` struct and its non-compiling test outright — it can never produce a usable vector under this schema (ladder rung 1: it doesn't need to exist).
- Known gap to disclose, not silently swallow: `find_similar_experiences` (`postgres.rs:616-635`) self-joins `experiences.embedding` directly (the source column, not the cache) — a locally-embedded experience never participates in that comparison until/unless that query is migrated to read the cache instead. Flag as follow-up.

**Touches running macOS/Linux?** No behavior change for machines that never set `EMBEDDING_MODEL=llama-cpp` — new config value, additive migration with backfill, existing `nvidia` path untouched.

### B. Windows CI

**Fix — two independent parts, both live today:**

1. **Ubuntu leg (build error):**
```
- src/mcp/hooks.rs:250   delete `use chrono::Utc;`
- src/services/embedding.rs:428
    - let expected_dimension = config.expected_dimension.max(1);
    + // inline at each of the two use sites (line ~433, ~447) instead of a shared binding
```
2. **macOS leg + adding Windows:**
```yaml
# .github/workflows/ci.yml
jobs:
  build-and-test:
    strategy:
      matrix:
        os: [ubuntu-latest, macos-latest, windows-latest]
    # DELETE the job-level `services: postgres:` block entirely —
    # tests/integration.rs's 2 #[ignore] tests and src/migrations/mod.rs's
    # 1 #[ignore] test never run without --include-ignored, so nothing
    # in this workflow has ever used the container.
    steps:
      - name: Secret scan
        shell: bash          # NEW — Windows' default step shell is pwsh
        run: ./scripts/check_secrets.sh
      # rest unchanged; cargo build/test/clippy/fmt need no DATABASE_URL
```
Plus a new `.gitattributes`: `*.sh text eol=lf` (Windows checkout otherwise hands `.sh` files CRLF, breaking `set -euo pipefail`).

**Touches running macOS/Linux?** Yes — and it's a pure regression fix: removes a container dependency neither job's actual test run exercises, and unblocks a build that's failing on every push today (confirmed via `gh run view 33951950565`: macOS dies in 3s at "Set up job", `docker: command not found`; Ubuntu dies at "Build (all targets)", exit 101, on exactly the two errors above).

### C. Sync cursor clock-skew

**Fix:** `neon_seq BIGSERIAL` on Neon's `sync_meta.events`, assigned inside the already lease-serialized push transaction (`acquire_target_lease`/`renew_target_lease`, `neon-sync.rs:427-466`) — not per-device `logical_time`, which is provably non-transactional under this app's own connection pool.

**Diff sketch:**
```sql
-- migrations/014_neon_event_seq.sql
ALTER TABLE sync_meta.events ADD COLUMN IF NOT EXISTS neon_seq BIGSERIAL;
```
```rust
// neon-sync.rs pull_events() (~line 604-641)
// WHERE (created_at,event_id) > (...)  ORDER BY created_at,event_id
// becomes:
// WHERE neon_seq > $1 ORDER BY neon_seq LIMIT $2
// cursor storage: single BIGINT instead of (timestamptz, uuid) pair
```
`push_event_batch`'s INSERT (`neon-sync.rs:526-528`) needs no change — it doesn't list `neon_seq`, so Postgres's own `DEFAULT nextval(...)` fires automatically, and because the whole push is one lease-serialized transaction, assignment order is genuinely commit order.

One-time diagnostic before shipping (not part of the fix itself): `SELECT device_id, count(*), max(logical_time) FROM sync_meta.events GROUP BY device_id;` against local and Neon, to rule out any historical non-monotonicity before trusting `logical_time` for its remaining role (the `UNIQUE(device_id, logical_time)` dedup key only).

**Touches running macOS/Linux?** Yes, **required** — both existing devices must run this migration and the new `pull_events()` before Windows starts pushing as a third clock. This is the one blocker fix that is not simply opt-in; per AGENTS.md ("keep `main` deployable") and `docs/WINDOWS_PORT.md`'s own instinct ("show the diff before it lands near `main`"), review this diff specifically before merging since it changes live sync topology.

### D. Degraded-mode data durability

**Fix — three layers:**
```rust
// main.rs:45-57 — replace degrade-to-new_empty with bounded retry + hard exit
let db = retry_connect(&config, Duration::from_secs(2), Duration::from_secs(120)).await?;
// (delete PostgresDb::new_empty()'s production call site; its own doc
//  comment already says "use only for unit tests")
```
```rust
// api/handlers/root.rs:16-28
if !db.health().await { return (StatusCode::SERVICE_UNAVAILABLE, Json(body)); }
```
```rust
// NEW: local SQLite pending-writes queue (Cargo.toml:36 — add "sqlite" to
// sqlx's feature list; confirmed clean, libsqlite3-sys isn't in the
// currently-active build graph despite sitting in Cargo.lock)
// WAL mode, synchronous=FULL, file at dirs::data_local_dir()/memory-platform/pending-writes.db
// ingest_event() (events.rs) writes here first, then attempts Postgres;
// success deletes the row, failure leaves it for a background drain task.
// ponytail: row-count/age-bounded queue, 503 when full — raise the cap or
// add backpressure if this is ever actually hit.
```
Windows deployment layer (no shared code touched): native PostgreSQL as an Automatic-start Windows service; `memory-platform.exe` wrapped with WinSW as a second service, `sc config memory-platform depend= postgresql-x64-17`, `sc failure memory-platform reset= 86400 actions= restart/30000/restart/60000/restart/120000` — SCM handles boot ordering and crash-restart natively.

**Touches running macOS/Linux?** The fail-fast/503/queue changes: yes, and they're a straight bugfix (silent data loss today) — ship everywhere, not gated. The WinSW/native-service layer is Windows-only deployment config.

---

## 3. Fully-offline architecture

**Scope correction first:** the only AI network call anywhere in `src/` is embeddings. There is no LLM text-generation call site in the Rust core. "Fully offline" for the *codebase as it exists* means solving the embedding backend; a local chat model is available at zero extra install cost later (same `llama-server` binary) but isn't required today — see open question 2 (§8).

**Runtime:** `llama.cpp`'s `llama-server`, prebuilt Windows Vulkan release (no MSVC/cmake needed for the binary itself — MSVC is still required separately to `cargo build` the Rust crate), bound to `127.0.0.1` only, `--embedding` mode, serving a Qwen3-Embedding-4B GGUF (Apache-2.0). Truncate 2560→2048, renormalize in Rust (§2.A).

**Fails closed via `MEMORY_LOCAL_ONLY=1`,** three enforcement layers (not one — three real call sites exist):

| Layer | Mechanism | Why it must be here specifically |
|---|---|---|
| `Config::load()` | Hard `bail!` if `local_only=true` but `embedding_model != "local"` or `nvidia_api_key`/`openai_api_key` non-empty | Propagates via `?` at `main.rs:39` — the only layer that actually **stops the process** |
| `EmbeddingServiceFactory::new` | Delete the `Fallback`-wrapping branch (`embedding.rs:435-454`) entirely under `local_only`, don't skip it | `main.rs:109-119` already catches any factory error and downgrades to `embedding_service: None` with a `tracing::warn!` — correct behavior for "model file missing," but this must not be the *only* thing standing between local text and NVIDIA |
| `neon-sync.rs::main()` + `memory-dashboard.rs::main()` | Explicit check before any Neon connection attempt, refuse-to-run (exit nonzero), not silent no-op | **Confirmed by direct read**: `memory-dashboard.rs:500-505` independently reads `NEON_SYNC_URL` and calls `PgPoolOptions::connect_lazy` — this is not covered by a gate placed only in `neon-sync.rs` or the embedding factory |

**Exhaustive egress inventory** (every network-capable call site in `src/`, confirmed by direct read of all 9 real binaries — `Cargo.toml` declares 5 `[[bin]]` targets, but `cargo metadata` confirms `autobins` picks up 4 more from `src/bin/*.rs`: `memory-dashboard`, `reindex`, `repair_embeddings`, `stats`):

1. `NvidiaNimEmbedding` (`embedding.rs:173`) — the one `reqwest::Client` in the crate. Gated via the factory.
2. `neon-sync.rs` — Postgres wire connections to `NEON_SYNC_URL`/`NEON_DIRECT`. Gated at `main()`.
3. `memory-dashboard.rs:500-505` — independent `NEON_SYNC_URL` read + `connect_lazy`. **Not covered by #1 or #2's gates** — needs its own.
4. `openai_api_key` (`config.rs:46,149`) — parsed, redacted in `Debug`, never read elsewhere (confirmed dead). Zero egress today, but a landmine: add to the same startup check so a future wire-up can't bypass the gate.
5. `obsidian_api_url`/`obsidian_api_key` (`config.rs:50-51,153-155`) — same dead-field status, same preemptive check.
6. `reindex.rs`, `repair_embeddings.rs` — both call `EmbeddingServiceFactory::new` directly (confirmed by grep), covered once the factory is gated. `vault-sync.rs`, `ingest.rs` — no embedding or Neon calls found at all.
7. Redis, Neo4j, local Postgres — loopback/localhost by construction, not egress; per decision #11, not even installed here.
8. Ollama — explicitly not used (decision #3); stated here so nobody reintroduces it casually given its unresolved CVE pair.
9. **Out of this application's control surface, state this boundary explicitly to the user:** Windows Update, Defender cloud lookups, the OneDrive sync client's own traffic. "Prove no egress" below scopes to this app's binaries only.

**How the user can prove no egress:**
1. **Live socket proof**, run continuously across a real session (search, store, embed, archive), not a single snapshot:
   ```powershell
   Get-NetTCPConnection -OwningProcess (Get-Process memory-platform,mcp-server,neon-sync,memory-dashboard -EA SilentlyContinue).Id |
     Where-Object { $_.RemoteAddress -notin @('127.0.0.1','::1') }
   ```
   Zero rows for the whole session = zero non-loopback connections from those processes.
2. **Belt-and-suspenders OS firewall**, scoped to binary paths (not ports — a port rule would also block legitimate loopback Postgres/llama-server traffic):
   ```powershell
   New-NetFirewallRule -DisplayName "memory-platform-no-egress" -Direction Outbound `
     -Program "C:\...\target\release\memory-platform.exe" -Action Block -RemoteAddress Internet
   # repeat for neon-sync.exe, memory-dashboard.exe, mcp-server.exe
   ```
   Even a bug in the Rust-side gate cannot then open a non-loopback socket.

---

## 4. Any-AI interop

**Transport decision:** keep stdio. `transport-http` (`src/mcp/transport.rs`) is an empty scaffold today — no session ID, no SSE, no protocol-version header handling — and Zed has no remote MCP support at all, so building it out now is speculative scope; revisit only if connection-pool pressure from multiple simultaneously-open agents is actually observed. The tool surface is already agent-agnostic (zero `Claude`/`Anthropic` string literals anywhere in `src/mcp/`).

**Two live bugs to fix in the same pass** (confirmed by direct read):
- `mod.rs:194` hardcodes `"protocolVersion": "2025-03-26"`, echoed regardless of what the client requests; `mod.rs:416`'s test asserts the same literal. Bump to `"2025-06-18"`. Not cosmetic — `thingsboard-mcp#35` documents exactly this failure mode (stale/mismatched protocolVersion → client silently stops issuing `tools/call` after a successful `initialize`) on a Claude client family; verify empirically against the actual installed Claude Code build before treating this as low-priority.
- `mod.rs:320` and `tools.rs:1387` assert `tools.len() == 17`; direct enumeration of `list_tools()`'s json literal in `tools.rs` counts **18** top-level tools (not the "12" the module doc-comment claims either). Fix both assertions and the comment together.

**Per-agent config** (all point at the same native `mcp-server.exe` — no `npx`/`.cmd` shim problem since it's a real binary):

| Agent | Config path | Note |
|---|---|---|
| Claude Code | project `.mcp.json` | `{"mcpServers":{"memory-platform":{"command":"...\\mcp-server.exe"}}}` — secrets never in this file, sourced by the wrapper from the protected `.env` per AGENTS.md |
| Codex CLI | `~/.codex/config.toml` | `[mcp_servers.memory-platform]`, `command`/`args` |
| OpenCode | its own `mcpServers` block | same shape as Claude Code |
| Cursor | `%USERPROFILE%\.cursor\mcp.json` | open, unresolved Windows-11-specific community report of project-level config not working (forum.cursor.com/t/.../62182) — smoke-test before documenting as supported |
| Windsurf | `%USERPROFILE%\.codeium\windsurf\mcp_config.json` | — |
| Zed | `%APPDATA%\Zed\settings.json`, `context_servers`, `"source":"custom"` | stdio only, no remote HTTP at all |
| VS Code / Copilot | `.vscode/mcp.json` or `~/.copilot/mcp-config.json` | confirm which surface is actually used before documenting both |

**Windows launcher:** port `scripts/mcp-transport-guard.sh` → `scripts/mcp-transport-guard.ps1` (`Get-Process -Id $pid -EA SilentlyContinue` for the `kill -0` staleness check, `$env:LOCALAPPDATA\memory-platform\mcp-transports` for the PID registry, `icacls` to restrict it). In the same pass, fix `run-mcp-server.sh`'s hardcoded `cd /home/humanoracle26/memory-platform-rust` to be `$ROOT`-relative like `mcp-entrypoint.sh` already is — this script is already non-portable to a *second Mac/Linux machine* today, not just to Windows; don't carry the anti-pattern into a new hardcoded Windows path.

**Preserve the env-file pattern exactly:** MCP clients forward only a small allowlist to stdio subprocesses (Windows: `APPDATA`/`HOMEDRIVE`/`HOMEPATH`/`LOCALAPPDATA`/`PATH`/`PATHEXT`/`PROCESSOR_ARCHITECTURE`/`SYSTEMDRIVE`/`SYSTEMROOT`/`TEMP`/`USERNAME`/`USERPROFILE`) — never `DATABASE_URL` etc. The wrapper must keep sourcing the protected `.env` itself, or the Windows `mcp-server` silently starts DB-less with only a warn-level log line.

**Carried forward, not solved here:** all ~18 tools including `dashboard_control`'s write actions are available to every configured agent with no per-tool/per-agent scoping. The only real lever today is which agents a developer chooses to wire up. Worth a decision before adding a third or fourth agent to a box holding client data.

---

## 5. Storage tiers

| Tier | Required when `MEMORY_LOCAL_ONLY=0` | Required when `MEMORY_LOCAL_ONLY=1` | Notes |
|---|---|---|---|
| **Local PostgreSQL** (hot) | Always | Always | Native Windows service, port **5432** — `.env.example`'s existing `postgresql://memory:password@127.0.0.1:5432/memory` default already matches a native install with **zero deviation** (the 5433-remap in `docs/WINDOWS_PORT.md`'s earlier plan was specifically for a Podman container port conflict, which no longer applies under decision #10) |
| **OneDrive** (cold, encrypted) | Sanctioned exception per requirement 3 itself | Same — the one deliberate egress this design allows | Microtech tenant path (`C:\Users\CalebArumugam\OneDrive - MICROTECH SYSTEMS`) only; personal OneDrive stays signed out (confirm at §8 Q7). Gated behind `ARCHIVE_ENCRYPT=1`. Files On-Demand hydration required before hashing or verification reports a false "corrupted" alarm |
| **Neon** (cloud backup) | Required only if configured — genuinely optional now | Not used at all; connection attempts refused at startup | Needs a **separate Neon project** from the personal deployments — the actual personal/work boundary is which project, not a label |
| **llama-server model weights** (new tier) | N/A | Local disk, a few GB, no cloud tier | Re-downloadable, not user data — needs no backup |

---

## 6. Personal/work separation

Two orthogonal axes, not three overlapping switches:

- **`MEMORY_LOCAL_ONLY`** (bool, default off) — absolute kill switch for *all* network egress (embeddings and Neon both). Answers "does anything leave this machine at all."
- **`MEMORY_SCOPE=work|personal`** + category include/exclude (`MEMORY_SYNC_INCLUDE`/`MEMORY_SYNC_EXCLUDE`) — a row-level filter that only matters once Neon is reachable at all (i.e., only relevant when `MEMORY_LOCAL_ONLY=0`). Already correctly scoped in `docs/WINDOWS_PORT.md`'s work order #2: a `sync_meta.scope` marker row per database, `neon-sync` refuses to run when config and target disagree, keyed off `documents.vault_section` (already exists, `migrations/001_initial.sql:79`) — other synced tables have no equivalent column yet (open question, §8 Q5).

**Composition table:**

| `MEMORY_LOCAL_ONLY` | `MEMORY_SCOPE` | Behavior |
|---|---|---|
| 1 | (ignored) | Nothing leaves the machine, ever. Neon/NVIDIA refused at startup. OneDrive cold-storage is the sole exception, still `ARCHIVE_ENCRYPT`-gated |
| 0 | unset | Today's macOS/Linux behavior, byte-for-byte unchanged: sync everything to whatever `DATABASE_URL`/`NEON_SYNC_URL` point at |
| 0 | `work` | Only category-allowed rows sync, to the **work** Neon project |
| 0 | `personal` | Only category-allowed rows sync, to the **personal** Neon project |

Both default off/no-op on macOS and Linux — no code path changes unless these env vars are explicitly set, satisfying requirement 5 literally.

---

## 7. Sequenced work order

| # | Change | Files | Effort | Risk to running deployments | Human decision first |
|---|---|---|---|---|---|
| 0 | Install MSVC Build Tools + Windows SDK | (machine, no repo files) | Manual/elevation | None (Windows-only) | None — just execute |
| 1 | Fix 2 live CI compile errors | `src/mcp/hooks.rs`, `src/services/embedding.rs` | Trivial | None — pure bugfix | None |
| 2 | Delete dead `services:` block; add `windows-latest`; `.gitattributes` | `.github/workflows/ci.yml`, `.gitattributes` (new) | Small | None — container was unused; fixes today's macOS failure too | None |
| 3 | `neon_seq` migration + `pull_events()` rewrite | `migrations/014_*.sql`, `src/bin/neon-sync.rs` | Small (~30 lines) | **Medium** — must land on all 3 devices before/as Windows joins | Review the diff before merge (live-sync-topology change) |
| 4 | Fail-fast + 503 + SQLite pending-writes queue | `main.rs`, `root.rs`, `events.rs`, new queue module, `Cargo.toml` | Medium | Low — Postgres-down is already a failure state; this only removes silent loss | None (queue bound is a marked `ponytail:` simplification, not a decision) |
| 5 | `llama.cpp` embedding backend + migration 013 + `rebuild_derived` carry-through fix | `src/services/embedding.rs`, `migrations/013_*.sql`, `src/db/postgres.rs`, `src/bin/neon-sync.rs` | Medium-large | Low if additive as scoped — **must ship with the `rebuild_derived` fix together**, or local-model cache rows get silently deleted on the next scheduled run | Confirm Qwen3-Embedding-4B vs -8B (§8 Q1) |
| 6 | `MEMORY_LOCAL_ONLY` gate at all 3 real call sites + dead-field landmine checks | `config.rs`, `embedding.rs`, `neon-sync.rs`, `memory-dashboard.rs` | Small | None — bool defaults false = today's behavior exactly | None — direct implementation of a stated requirement |
| 7 | `MEMORY_SCOPE` + category lists | `config.rs`, `neon-sync.rs`, new migration | Medium | **Medium** — changes live sync topology, needs one-time Neon-side `set-scope` init | Build now for `documents` only, or wait for a shared `scope_tag` column? (§8 Q5) |
| 8 | Archive encryption: `age` binary, Keeper KSM custody, `ARCHIVE_ENCRYPT=1` gate, real restore logic | new `src/bin/archive-crypt.rs`, `archive-documents.sh`, `restore-archive-documents.sh` (net-new decrypt path), `verify-memory-archive.sh` | Medium-large | Low if gated + dual-format restore | Is KSM actually provisioned? Where does the MSP-recovery identity live? (§8 Q3, Q4) |
| 9 | Windows scheduling: XML templates, 5 script fixes, log rotation, dashboard runtime-probe rename | new `taskscheduler/*.xml.template`, `install-*.ps1`, 5 existing scripts, `memory-dashboard.rs` | Medium | Low — all 5 script fixes are single-implementation, cross-platform | Verify the `StartInterval=3600` → `Repetition`+`Duration=P1D` mapping actually repeats indefinitely on a live box before trusting it for `neon-retry` |
| 10 | Any-AI polish: protocolVersion, tool-count tests, PowerShell launcher, path fix, per-agent docs | `mod.rs`, `tools.rs`, new `mcp-transport-guard.ps1`, `run-mcp-server.sh`, `AGENTS.md` | Small | None — additive docs + 2 trivial fixes | None |

---

## 8. Open questions for Caleb

1. **Model size** — Qwen3-Embedding-4B (~2.5–3GB resident at Q4) vs. -8B (~5GB+) for local embeddings. Recommend 4B given 15.4GB total RAM and no discrete GPU. Confirm.
2. **Local generation model** — is a local chat/completion model (Qwen2.5-7B via the same `llama-server`) wanted now, given nothing in the current codebase calls an LLM for generation? Recommend: skip for now, install only the embedding server.
3. **Keeper Secrets Manager** — is KSM actually licensed/provisioned on the Microtech Keeper tenant, separate from the interactive vault? Determines the exact shape of the archive key-retrieval script.
4. **MSP-recovery key custody** — does the second `age` identity live in the *same* Keeper account, or a genuinely separate one, so a single Keeper compromise can't unlock the archive alone?
5. **`MEMORY_SCOPE` category scope** — build now for `documents` only (the one table with an existing `vault_section` column), or add a shared `scope_tag` column to every synced table first?
6. **Subprocessor approvals** — confirm the two already-listed Microtech sign-offs (Neon, NVIDIA NIM as data subprocessors) before `MEMORY_LOCAL_ONLY=0` is ever used on this box; this plan assumes NVIDIA is reachable only after that approval.
7. **OneDrive** — personal account stays signed out, Microtech tenant only, per `docs/WINDOWS_PORT.md`'s existing note. Still the intent?

---

## 9. Manual tasks

- [ ] Install MSVC Build Tools + Windows SDK: `winget install --id Microsoft.VisualStudio.2022.BuildTools --override "--wait --passive --add Microsoft.VisualStudio.Workload.VCTools --includeRecommended"` (~4-6GB) — nothing compiles on Windows without this
- [ ] Install native PostgreSQL 17 for Windows + pgvector (confirm a prebuilt Windows pgvector binary exists before committing to a from-source `nmake` build)
- [ ] ~~`wsl --install`~~ / ~~install Podman Desktop~~ — **dropped**, superseded by native Postgres (decision #10); tell Daniel this is no longer happening
- [ ] Download the llama.cpp Windows Vulkan release zip and a Qwen3-Embedding-4B GGUF (source pending §8 Q1)
- [ ] Set a git identity for this machine (currently unset — `git commit` fails outright)
- [ ] Create `.env` (gitignored) — `DATABASE_URL` now needs no port change from `.env.example`'s existing default; still needs a Windows `VAULT_PATH` and `MEMORY_ARCHIVE_ROOT`
- [ ] Set a stable `MEMORY_DEVICE_ID` — do not rely on the Git-Bash-broken `hostname -s` fallback
- [ ] Save `NVIDIA_API_KEY` (only if the subprocessor approval lands) and `NEON_*` URLs directly into the env file yourself — never paste into a session
- [ ] Create a separate Neon project for work
- [ ] Provision/confirm KSM on the Microtech Keeper tenant; create identity-key records once §7 item 8 is approved
- [ ] Decide + execute the OneDrive personal-sign-in question (recommend: stay signed out)
- [ ] Get Microtech sign-off: Neon as data subprocessor, NVIDIA NIM as data subprocessor (only if `MEMORY_LOCAL_ONLY=0` is ever used here)
- [ ] Get Microtech sign-off before archives move from local folder to the OneDrive tenant
- [ ] Review/approve the `neon_seq` migration diff before or immediately as this machine starts pushing events
- [ ] `gh auth login` (2.100.0 present, unauthenticated) if CI/PR workflows from this machine are wanted

---

## 10. Rejected options

- **Ollama** (embedding and/or generation) — unresolved Windows RCE CVE pair (2026-42248/-42249), daemon/auto-updater model, no functional gain over llama.cpp once Rust must renormalize the MRL truncation itself regardless
- **`fastembed`** (any of its 46 built-in models) — none reach 2048 dims; dead end regardless of which is picked; its bundled test doesn't compile
- **`fastembed`'s `qwen3`/candle feature** — not in `Cargo.lock` today; duplicates what an external llama.cpp server gives for free; unverified on this machine's toolchain
- **Microsoft Foundry Local** — best NPU story on paper, but its default catalog model caps at 1024 dims, and Intel NPU+OpenVINO has open, named issues (`microsoft/Foundry-Local#332`); revisit once its catalog documents a ≥2048-dim MRL path
- **vLLM** — no official Windows target at all (Linux/macOS/CPU/ROCm/XPU only); WSL is deliberately absent here
- **NV-Embed-v2, e5-mistral-7b-instruct** (local embedding candidates) — no official MRL support; NV-Embed-v2 is additionally CC-BY-NC-4.0 (non-commercial), a hard blocker for MSP client work regardless
- **jina-embeddings-v4** — inherits a non-commercial Qwen Research license via its base model
- **Zero-padding a short local vector to 2048** — mathematically preserves cosine similarity but makes cross-model comparison silently syntactically legal, deleting the dimension-mismatch error that's the actual safety feature
- **Per-source-table embedding column per model** — touches all 7 synced source tables' schema and the sync fingerprint path for every model ever tried
- **Separate table per model / LIST partitioning** — strictly more DDL/code than a discriminator column for no benefit at today's (unmeasured, but small) scale; partitioning specifically only past ~5M rows in one space
- **HNSW/IVFFlat index now** — pgvector caps both at 2000 dims; not needed yet at unmeasured-but-small row count; don't build speculatively
- **A generic "egress firewall" wrapper type** — there's exactly one `reqwest::Client` in the crate; check the flag at the 3-4 real call sites directly instead of an interface-with-one-implementation
- **Streamable HTTP MCP transport, built now** — `transport-http` is an empty scaffold; every current agent works fine over stdio (and Zed has no remote MCP support at all); revisit only if pool pressure is actually observed
- **PowerShell rewrite of all 25 bash scripts** — doubles maintenance surface forever for breakages that each have single-implementation, cross-platform fixes; Git Bash is already present and required
- **Folding the 25 scripts into a Rust `memory-ops` binary now** — cleanest long-term answer, but a materially larger separate piece of work than porting the scheduling layer; good opportunistic follow-up, not a prerequisite
- **GnuPG/gpg** for archive encryption — its only AEAD mode (OCB) is non-interoperable with other OpenPGP tools; its portable mode falls back to a pre-AEAD SHA-1 MDC check
- **`openssl enc`** — deliberately excludes AEAD ciphers by the maintainers' own stated design; using it correctly means hand-rolling Encrypt-then-MAC
- **7-Zip AES-256** — password verification is post-decompression CRC32, not cryptographic
- **Windows DPAPI / EFS** — single-machine/single-user-bound by design, not portable to macOS/Linux at all
- **Docker/Podman for local Postgres on Windows** — needs WSL2 (not installed) or Hyper-V with documented startup-ordering races; doubles the exact boot-ordering race Blocker D exists to fix
- **A pluggable container-runtime registry** for the dashboard's status panel — exactly two binary names exist fleet-wide (docker, podman); a widened 2-item probe is enough
- **`MEMORY_LOCAL_ONLY` as a degenerate case of `MEMORY_SCOPE`** — conflates "which rows sync" with "does anything leave at all"; kept as two orthogonal axes instead

---

**Remaining uncertainty, stated plainly rather than papered over:** llama-server's exact `/v1/embeddings` (vs `/embedding`) response shape and whether a ready GGUF of Qwen3-Embedding-4B exists or needs local conversion are both unverified pending a hands-on smoke test on the target machine; the Task Scheduler `Repetition`+`Duration=P1D` mapping for an indefinitely-repeating hourly job is unverified against a live Windows 11 box; and Ollama's CVE-2026-42248/-42249 patch-release status was not confirmed as shipped in a tagged release as of the source dates available — all three should be closed with a direct check before the corresponding work item ships, not assumed.

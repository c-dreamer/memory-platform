# Memory Platform Operations

## Authority and Secrets

- Each device's local PostgreSQL is its offline write cache. Neon is the shared
  active event exchange and projection; Google Drive is encrypted cold storage.
- Never print, commit, or pass database URLs, API keys, or tokens in command
  arguments, logs, state tables, LaunchAgent files, or commits. Use a protected
  per-device environment file only; client JSON names the MCP wrapper, never secrets.
- Preserve unrelated worktree changes and stage named task files only.

## Resumable Neon Sync

- `./sync-to-neon.sh run` is the normal operation. It uses `neon-sync`, a Rust
  outbox synchronizer, not `pg_dump`, Docker, or OrbStack.
- The sync target is any Postgres-compatible database with pgvector, not only
  Neon — Supabase and a self-hosted VPS Postgres both work. Set
  `SYNC_TARGET_URL` (provider-neutral name; `NEON_SYNC_URL`/`NEON_DIRECT`
  still work for existing Neon setups) to its direct, non-pooled endpoint.
  `neon-sync` rejects known pooler hostnames/ports for Neon, Supabase, and
  PgBouncer at startup — see `.env.example`.
- `reconcile` fetches complete local and Neon inventories before changing live
  data. It archives stale/conflicting Neon rows in `sync_meta.archive`, removes
  them from the live mirror, and queues only missing or mismatched local rows.
- `run` drains independently committed transactions of at most 25 rows or 2 MiB
  for up to ten minutes. It halves batch size after a transport failure and only
  removes a local queue row after the corresponding Neon commit succeeds.
- The target upsert is idempotent. An interruption after target commit but before
  local acknowledgement must replay safely on the next run.
- `status` is read-only. `rebuild-derived` reconstructs FTS and the universal
  `embeddings` cache inside Neon from source vectors and local cache metadata;
  it must never call NVIDIA.
- The local-only capture trigger must never be installed on Neon. The binary
  verifies this at startup.
- Full dump uploads are retired. `reset-target --confirm-neon-reset` is a
  last-resort destructive recovery command and is never automatic or routine.

## Migrations and Validation

- Apply migrations only through `src/migrations/mod.rs`, in order, to both local
  and target databases. Migration `005_embeddings_2048` must preserve a correct
  2048-dimensional cache. Migration `007` must reject non-null legacy 384-dim
  `code_changes` embeddings rather than silently discard them.
- The canonical embedding model is `nvidia/nemotron-3-embed-1b` (2048-dim). It
  replaced `nvidia/llama-nemotron-embed-1b-v2`, which NVIDIA retired 2026-08-25
  (`410 Gone`). The dimension is unchanged, so no vector migration is needed;
  never re-point the config at a model with a different dimension.
- Before a recovery is accepted: run a small-table/document canary, two
  consecutive no-op runs, count and fingerprint parity, embedding dimensions and
  null counts, FTS coverage, migration ledger checks, and queue depth zero.
- Use `NEON_SYNC_FAIL_AFTER_TARGET_COMMIT=1` only in a test environment to prove
  replay safety after a committed target batch.

## Automation and Git

- `scripts/install-neon-sync-launchd.sh` installs a user LaunchAgent that runs
  daily at 03:00, exits quickly when idle, and records a retry if Neon is
  unreachable. A retry checker runs hourly only after a failure, and
  posts a macOS notification before retrying. Use `./sync-to-neon.sh run` for
  an explicit manual sync.
- The daily count audit and weekly fingerprint reconciliation are scheduled by
  dedicated LaunchAgents created by the installer.
- Keep `main` deployable. Fetch before publishing; commit and stage only verified
  task-owned files. Run a staged secret scan before commit. Do not publish until
  local and remote `main` are verified to match.

## Cold Archive

- `scripts/archive-documents.sh` defaults to a dry run. It writes a
  checksum-verified bundle to the mounted Google Drive archive root only with
  `--mark-archived`, then changes records to `archived` so Neon removes them.
- Do not compact local raw data until a restore drill succeeds on both Mac and VPS.
- `scripts/restore-archive-documents.sh ARCHIVE_ID [LIMIT]` restores only
  verified archived documents and relies on the outbox to reintroduce them to Neon.
- `scripts/verify-memory-archive.sh` is safe for scheduled use: it checks the
  mounted Drive bundle files, checksums, archive ledger, tiers, and queue depth,
  but never creates, archives, restores, or compacts records.

## Windows MCP Client Configuration

Every agent below points its "command" at the wrapper, never at `mcp-server.exe`
directly and never at a bare secret — same rule as macOS/Linux. On Windows the
wrapper is `scripts/mcp-transport-guard.ps1` (ported from
`scripts/mcp-transport-guard.sh`; same PID registry/log-then-launch behavior,
`Get-Process -Id` instead of `kill -0`, `icacls` instead of `chmod`). Since `.ps1`
isn't directly executable from a JSON `command` field, every config invokes it
through `powershell.exe -File`.

| Agent | Config path | Command / notes |
|---|---|---|
| Claude Code | project `.mcp.json` | `{"mcpServers":{"memory-platform":{"command":"powershell.exe","args":["-NoProfile","-ExecutionPolicy","Bypass","-File","<repo>\\scripts\\mcp-transport-guard.ps1"]}}}` |
| Codex CLI | `~/.codex/config.toml` | `[mcp_servers.memory-platform]` with the same `command`/`args` shape |
| OpenCode | its own `mcpServers` block | same shape as Claude Code |
| Cursor | `%USERPROFILE%\.cursor\mcp.json` | there is an open, unresolved Windows-11-specific community report of project-level config not working (forum.cursor.com/t/.../62182) — smoke-test before documenting as supported on a given box |
| Windsurf | `%USERPROFILE%\.codeium\windsurf\mcp_config.json` | same `command`/`args` shape |
| Zed | `%APPDATA%\Zed\settings.json`, `context_servers`, `"source":"custom"` | stdio only, no remote HTTP support at all |
| VS Code / Copilot | `.vscode/mcp.json` or `~/.copilot/mcp-config.json` | confirm which surface is actually active in the installed VS Code build before documenting both |

The MCP client itself forwards only a small allowlist of environment variables
to the stdio subprocess — on Windows: `APPDATA`, `HOMEDRIVE`, `HOMEPATH`,
`LOCALAPPDATA`, `PATH`, `PATHEXT`, `PROCESSOR_ARCHITECTURE`, `SYSTEMDRIVE`,
`SYSTEMROOT`, `TEMP`, `USERNAME`, `USERPROFILE` — never `DATABASE_URL` or any
other secret. `mcp-transport-guard.ps1` must keep sourcing the protected
per-device environment file itself; if that file goes missing, the server must
fail loudly (`missing_environment`, exit 78), not start silently DB-less.

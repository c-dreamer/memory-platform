# Memory Platform Operations Status

## Scope

This document is the operational handoff for the Rust memory MCP, the local
dashboard, and the resumable Neon projection. It contains only aggregate state
and safe procedures; credentials, raw transcripts, and database URLs remain in
the protected per-device environment file.

## Current Architecture

- `mcp-server` is the only memory server used by Codex and OpenCode. It runs
  over stdio through `scripts/mcp-entrypoint.sh`.
- Local PostgreSQL is authoritative. `sync_meta.outbox` is durable and records
  are acknowledged only after the corresponding Neon transaction commits.
- Neon holds the active searchable projection and immutable event metadata.
- The dashboard binds to `127.0.0.1:8765`, requires a per-device token, and
  reports only aggregate operational data.
- The dashboard runtime is installed under `~/Library/Application Support/Memory
  Platform/runtime/<git-revision>/`. It must never rely on `/private/tmp`.
- `mcp-entrypoint.sh` starts `mcp-transport-guard.sh`. The guard records each
  stdio launch, clears only dead transport records, and distinguishes a normal
  peer closure from a server error without writing secrets to logs.

## MCP Transport Recovery

MCP stdio is scoped to one Codex/OpenCode client session. A server process
cannot reattach after its client closes the pipe, so a `Transport closed` error
requires that client task to be restarted once. The next launch runs through the
transport guard and starts a fresh server process automatically.

The guard must never terminate a live MCP child to "repair" another session.
It is safe because memory records are in PostgreSQL; the MCP process is
stateless and does not own the outbox or archive lifecycle.

## Embedding Model

- Canonical model: `nvidia/nemotron-3-embed-1b`, 2048 dimensions.
- The previous model `nvidia/llama-nemotron-embed-1b-v2` reached end-of-life on
  2026-08-25 and now returns `410 Gone`. When it did, the dimension probe failed
  closed to keyword-only search; it did not corrupt or mix vectors.
- Because the replacement is also 2048-dimensional, there is no schema change
  and no re-embedding migration. Vectors produced by the prior model remain
  valid under the same `nvidia-2048-v1` generation.
- Verify after a model change: `status` must report
  `embedding_service: true` and `search_mode: semantic`.

## Connection Pool Resilience

- `PostgresDb::connect` tunes `PgPoolOptions` for a long-lived stdio server that
  outlives a Docker Postgres restart:
  - `idle_timeout(60s)` recycles idle connections well before the default 10 min,
    shrinking the window in which a stale TCP connection can be handed out.
  - `max_lifetime(None)` avoids a synchronized expiry storm with
    `min_connections(2)`.
  - `acquire_timeout(5s)` fails fast so the client sees a retryable error instead
    of hanging behind a dead socket.
  - `test_before_acquire(false)` plus a selective `before_acquire` ping only when
    `idle_for > 60s`, so healthy hot-path acquisitions do not pay a ping.
- The MCP client `memory` timeout is 10s to sit above the 5s acquire timeout.
- Expected behavior: after `docker restart memory-postgres`, the next
  `memory_search` recovers automatically without restarting the MCP process.

## Storage Audit

The dashboard endpoint `GET /storage/catalog` and the MCP `storage_catalog`
tool expose the same redacted catalog:

- Local database size and pending event payload bytes.
- Active versus archived document, memory, and session counts and sizes.
- Document source categories: Vault, Codex, OpenCode, generated browser output,
  and other.
- Archive bundle and verification totals.

These views never return raw archive contents, transcript bodies, credentials,
or database URLs. The catalog supports review; it never archives, compacts, or
deletes data.

## Recovery Safety

- `neon-sync full --confirm-full-push` is manual-only and resumable.
- The `neon-maintenance.paused` marker is stored under `$XDG_STATE_HOME` or
  `$HOME/.local/state`, in the `memory-platform` subdirectory. `status` and `health`
  remain available while paused. The source wrapper rejects unsupported
  commands before loading the protected environment file. The maintenance runner
  checks before loading it and between ingestion/sync steps; the full-recovery
  runner rechecks before each attempt, before its final pull, and before
  reporting that its queues are drained. It propagates `full`, `pull`, `status`,
  and database-query failures instead of reporting success. These shell guards
  fail closed if they cannot inspect the marker.
- The marker blocks Neon sync through the guarded wrappers and the combined
  `scripts/run-memory-maintenance.sh` ingestion/sync runner. The separate direct
  `systemd/user/memory-ingest.service` -> `ingest.sh all` local ingestion path
  does not check this marker: local capture may continue during paused Neon
  sync and may create more local work. This ingestion path does not drain the
  sync queue or publish queued events to Neon.
- The source `neon-sync` binary checks before connecting and at bounded event,
  bootstrap, and projection batch boundaries. These binary checks require a
  rebuild and install to become active. The currently installed release predates
  them, so a running `full` invocation can continue until that attempt finishes.
  Among installed full-recovery snapshots, only the active Application Support
  script has the shell guard; older snapshots and direct binary invocations are
  not covered by it. The source compatibility and maintenance wrappers are guarded.
- `rebuild-derived` checks the marker while rebuilding inside a transaction; a
  pause before commit aborts and rolls that work back. Other single-step commands
  check at entry, so an already-running database statement can finish.
- Dashboard and MCP status treat an unreadable or dangling pause marker as
  paused. If scheduler start fails after a resume removed an existing marker,
  the control restores the marker and keeps sync paused.
- Use the dashboard or MCP maintenance controls to resume. Their `start`,
  `resume`, `retry`, and `push_critical` actions all remove the pause marker and
  start the scheduler, so retry is not a pause-preserving operation.
- The active full-recovery runtime uses the shared Postgres environment helper
  so `psql` receives a password-free URL in its arguments. The runtime installer
  copies that helper alongside the runner.
- The full-recovery runner reports that its local sync queues are drained; the
  parity, retrieval, migration, archive, and health checks below still determine
  whether recovery is complete.
- Event publication and projection rows are interleaved, so metadata backlog
  cannot consume an entire recovery run before active records advance.
- Every Neon connection has bounded connection, statement, and idle transaction
  timeouts. Target writes, archives, deletes, vectors, and commits are
  idempotent and client-bounded.
- On any interruption, leave the local outbox untouched and resume. Do not run
  `reset-target` as part of normal recovery.

## Paused Checkpoint: 2026-07-24

This is a historical snapshot, not the current live status. Recheck the live
pause marker, local queue, and target before following the recovery steps.

- Full recovery was intentionally paused to avoid overnight CPU/network use.
- The durable local outbox contains 3,022 projection records: 907 documents and
  2,115 memories.
- Event publication has completed; only active projection rows remain queued.
- The dashboard is healthy from the stable Application Support runtime.

To resume deliberately on macOS, first build and install a `neon-sync` release
that contains the source pause guards, then install the recovery runtime. Use
the dashboard or MCP `resume` control to remove the pause marker; installing
the runtime alone leaves recovery paused. Only after that deliberate resume,
bootstrap `com.memory-platform.full-recovery`. It resumes from the outbox and
does not retransmit or delete confirmed rows. The currently installed release
predates the source guards, so do not treat an install of that binary as
pause-safe.

## Before Declaring Recovery Complete

1. Outbox depth and the count of local events where `pushed_at IS NULL` are
   both zero.
2. Two consecutive `neon-sync push` runs are no-ops.
3. Active authoritative-table counts and fingerprints match policy.
4. Both clients report 2048-dimensional semantic retrieval and pass a real
   search/recall smoke test.
5. Migration ledgers, FTS coverage, archive verification, and dashboard health
   are clean.

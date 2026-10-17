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
- Because the replacement is also 2048-dimensional, there is no schema change.
  A one-off re-embed **is** required, for two independent reasons:
  - vectors produced by the retired model live in a different vector space and
    do not compare meaningfully with `nemotron-3-embed-1b` query vectors;
  - before `e48983f` every NVIDIA call sent `input_type=query` and truncated
    input to 512 characters, so even stored vectors made by the new model are
    query-type embeddings of the first ~512 characters of each chunk.
  Every NVIDIA vector written before `e48983f` is affected. The model tag in the
  `embeddings` cache cannot tell them apart, so re-embed all of them.
- Re-embed procedure (operator-run on each device's local database; needs the
  NVIDIA key from the protected env file, never automatic):
  1. Take a local snapshot first (`memory-snapshot`).
  2. Clear source vectors and the NVIDIA cache rows in one transaction:
     `UPDATE <table> SET embedding = NULL WHERE embedding IS NOT NULL` for
     `documents`, `memories`, `experiences`, `sessions`, `summaries`,
     `code_changes`, `trading_results`, then
     `DELETE FROM embeddings WHERE model LIKE 'nvidia/%'`.
     Vector search is degraded until step 3 finishes.
  3. `cargo run --release --bin repair_embeddings -- run` (or
     `scripts/run_embedding_repair_detached.sh` for a long, resumable run);
     it fails unless `verify` passes.
  4. `./sync-to-neon.sh run` until queue depth is zero, then
     `./sync-to-neon.sh rebuild-derived` so the target cache is rebuilt from
     the new source vectors.
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
- Event publication and projection rows are interleaved, so metadata backlog
  cannot consume an entire recovery run before active records advance.
- Every Neon connection has bounded connection, statement, and idle transaction
  timeouts. Target writes, archives, deletes, vectors, and commits are
  idempotent and client-bounded.
- On any interruption, leave the local outbox untouched and resume. Do not run
  `reset-target` as part of normal recovery.

## Paused Checkpoint: 2026-07-24

- Full recovery was intentionally paused to avoid overnight CPU/network use.
- The durable local outbox contains 3,022 projection records: 907 documents and
  2,115 memories.
- Event publication has completed; only active projection rows remain queued.
- The dashboard is healthy from the stable Application Support runtime.

To resume deliberately on macOS, install the current recovery runtime and
bootstrap `com.memory-platform.full-recovery`. It resumes from the outbox and
does not retransmit or delete confirmed rows.

## Before Declaring Recovery Complete

1. Outbox depth is zero.
2. Two consecutive `neon-sync push` runs are no-ops.
3. Active authoritative-table counts and fingerprints match policy.
4. Both clients report 2048-dimensional semantic retrieval and pass a real
   search/recall smoke test.
5. Migration ledgers, FTS coverage, archive verification, and dashboard health
   are clean.

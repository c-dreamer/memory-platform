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

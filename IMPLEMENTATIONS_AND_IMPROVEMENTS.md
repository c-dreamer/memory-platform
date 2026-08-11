# Memory Platform Implementations and Improvements

## Purpose

This document is the implementation handoff for OpenCode. It follows the
platform review completed on 2026-08-10 and is specific to this repository's
requirements:

- Local PostgreSQL remains authoritative.
- Neon is an active searchable projection and event exchange, not a destructive
  replacement for local data.
- Google Drive archives are encrypted recovery copies, not the only backup.
- Codex and OpenCode use the same memory authority.
- Obsidian, Codex sessions, OpenCode sessions, project state, job-hunt data,
  and selected mail data require provenance and safe restoration.
- No operation may delete or compact data until an independently verified
  recovery copy exists.

Do not reset Neon, delete Docker volumes, compact local records, or rewrite
unrelated worktree changes while implementing this document.

## Final Architecture

```text
Codex / OpenCode
        |
        +--> protected loopback HTTP MCP/API (primary shared runtime)
        |
        +--> stdio compatibility adapter (only when a client requires stdio)
                         |
                         v
                 Rust memory authority
                         |
                 local PostgreSQL event ledger
                    |              |
                    v              v
          Neon active projection   encrypted archive bundles
```

The HTTP service and the stdio adapter must call the same Rust MCP handlers.
There must be one writer and one schema, not separate Mem0, Tencent, Cognee,
Graphiti, SQLite, or legacy Bun memory writers.

## Release Blockers

### 1. Eliminate embedding-dimension drift

Current findings:

- `DEFAULT_EMBEDDING_DIM` is 2048.
- `src/config.rs` still defaults `EMBEDDING_DIM` to 384.
- `.env.example` still advertises `EMBEDDING_DIM=384`.
- Some model tests still construct 384-dimensional fixtures.

Implement:

1. Make the active configuration default 2048 everywhere.
2. Set the canonical model and generation explicitly to
   `nvidia/llama-nemotron-embed-1b-v2` and `2048`.
3. Add a startup guard that compares configured dimension, database vector
   columns, source-vector metadata, and retrieval query dimension.
4. Fail closed for semantic search on mismatch. Continue keyword-only search
   with an explicit `keyword-only` or `degraded` health mode.
5. Never silently pad, truncate, or replace a vector with a zero vector.
6. Keep old 384-dimensional records quarantined or marked for explicit
   re-embedding; never compare 384 and 2048 vectors.
7. Add model generation metadata to every source vector and retrieval result.

Acceptance tests:

- A clean environment starts in semantic 2048 mode.
- A 384/2048 mismatch refuses semantic search without data mutation.
- A missing embedding service returns keyword-only status.
- No database vector column or fixture remains unintentionally at 384.

### 2. Make the runtime release-safe

Implement a versioned release contract containing:

- binary revision;
- schema migration revision;
- embedding model and dimension;
- supported MCP protocol revision;
- build timestamp;
- environment profile, without secrets.

The launch wrapper must refuse to run when the binary, release marker, and
migration ledger disagree. It must load credentials only from the protected
environment file and must never print them.

Add a read-only `memory doctor` command or equivalent API endpoint that checks:

- executable and release marker;
- database identity and connectivity;
- migration ledger;
- vector dimensions and model generation;
- MCP handshake and tool discovery;
- outbox/event cursor state;
- Neon freshness and lease state;
- archive verification state;
- recent redacted transport errors.

### 3. Fix the MCP transport architecture

The current `mcp-transport-guard.sh` is useful for lifecycle logging, but it
cannot reconnect a closed stdio pipe. Once Codex closes the client-owned stdio
transport, that session must be replaced by a new client connection.

Implement:

1. A persistent loopback HTTP MCP endpoint using Streamable HTTP semantics.
2. Authentication with a per-device token from the protected environment file.
3. Loopback binding by default: `127.0.0.1` only.
4. Explicit session handling and clean session termination.
5. A health endpoint separate from MCP that never performs migrations or writes.
6. A thin stdio adapter that starts the same release binary for clients that do
   not support HTTP.
7. Bounded request, connection, and shutdown timeouts.
8. Redacted structured transport logs with rotation.
9. A startup self-test that performs `initialize`, `tools/list`, and
   `memory_health` before advertising the server as ready.

Update documentation so it does not claim that `/mcp` is available unless the
binary was built and launched with the required feature. Document the exact
Codex and OpenCode configuration separately.

Transport failure tests:

- client closes the stdio pipe normally;
- MCP child exits unexpectedly;
- database unavailable during startup;
- request timeout;
- client reconnects after HTTP session expiry;
- two clients connect concurrently;
- stale binary is rejected;
- direct handshake works independently of the Codex UI.

The expected behavior after `Transport closed` is a visible degraded status and
a fresh connection attempt, never killing an unrelated live MCP process and
never pretending that the old session was reattached.

### 4. Harden Neon synchronization

Keep the resumable `neon-sync` design, but verify it against the current schema.

Required behavior:

- local mutations and outbox/event append occur in one transaction;
- event IDs and idempotency keys are immutable;
- pull and push use durable cursors;
- batches are limited to 25 rows or 2 MiB;
- each batch commits independently;
- acknowledgement occurs only after the target commit;
- replay after acknowledgement failure is idempotent;
- leases use expiry and fencing generation;
- audits are keyset-paginated and resumable;
- no routine `pg_dump`, full reset, Docker dependency, or OrbStack dependency;
- full transfer is manual and explicitly confirmed only.

Before any live repair:

1. Read-only inventory local PostgreSQL and Neon.
2. Abort if either inventory is incomplete.
3. Archive Neon-only and conflicting rows in the forensic namespace.
4. Queue active missing or changed records only.
5. Canary one small table and one document batch.
6. Run two no-op syncs.
7. Verify counts, BLAKE3/content fingerprints, vector metadata, FTS coverage,
   migration ledger, and zero pending eligible events.

Never delete a local row because it is absent on Neon. Never delete a Neon row
without archive-then-remove recording.

### 5. Make archives recoverable

Keep archive creation separate from archive publication and local compaction.
Each bundle must contain:

- archive ID;
- source table/key and provenance;
- raw content where policy allows;
- compact summary;
- source checksum;
- manifest checksum;
- schema and embedding generation;
- creation device and timestamp;
- restore instructions.

Required state machine:

`active -> archive_pending -> archived`

and

`archived -> restore_pending -> active`

Failures leave the original data intact and retryable. Local compaction is
blocked until two independently verified recovery copies and one restore drill
pass. Google Drive is encrypted storage, not immutable WORM backup; retain a
second recovery copy for important data.

## Memory Quality Improvements

Borrow patterns from Tencent Agent Memory without importing it as a parallel
database:

1. `staging`: raw session or ingestion material.
2. `active`: searchable facts, decisions, procedures, project state, and
   approved summaries.
3. `compressed`: compact promoted summaries with source references.
4. `archive`: raw material retained for explicit restoration.

Add provenance to retrieval results:

- source type and path;
- source checksum;
- created and observed time;
- freshness and confidence;
- embedding model and generation;
- supersession state;
- archive availability;
- local/Neon availability.

Add controlled tools or API operations:

- `remember`;
- `update_memory` with supersession/version;
- `archive_status`;
- `restore_archive`;
- `memory_health`;
- `storage_catalog`;
- explicit `push_critical` with confirmation.

Critical information must be promoted only after source and checksum are
recorded. Raw Codex/OpenCode transcripts should not automatically become
permanent facts without promotion or confidence metadata.

## Candidate Platform Evaluation

Do not install a second writer. Build read-only adapters or isolated test
datasets for comparison.

Evaluate:

- TencentDB Agent Memory for tiered compression and portable local memory.
- Cognee for graph-plus-vector retrieval and shared HTTP MCP.
- Mem0/OpenMemory for API, authentication, dashboard, and audit patterns.
- Graphiti for temporal facts and relationship provenance.

Use one representative evaluation set containing:

- Obsidian vault questions;
- law-material isolation checks;
- job-hunt and mail provenance questions;
- Codex/OpenCode session continuation questions;
- project-state and Neon-sync questions;
- archive discovery and restore questions;
- contradictory or superseded facts.

Measure:

- precision@5 and recall@10;
- currentness and supersession correctness;
- provenance completeness;
- archive discovery and restore success;
- p50/p95 latency;
- CPU, RAM, disk, embedding calls, and network bytes;
- restart recovery and offline convergence;
- setup and upgrade complexity.

Switch only if a candidate wins materially on retrieval and operations while
supporting local Mac/VPS deployment, direct Codex/OpenCode MCP compatibility,
privacy boundaries, and lossless export. Until then, retain the Rust system as
the sole authority and adopt only proven patterns.

## Codex and OpenCode Rollout

For both clients:

1. Use the same verified release revision.
2. Use environment references, never plaintext credentials in JSON or command
   arguments.
3. Run the read-only doctor check before enabling memory tools.
4. Confirm `initialize`, `tools/list`, `memory_health`, `memory_search`, and
   `recall`.
5. Start a test session and store one non-sensitive canary memory.
6. Recall it from a fresh client session.
7. Confirm the transport log records the new connection and no error.
8. Remove stale client configurations only after the new configuration passes.

The client configuration must clearly identify whether it uses:

- persistent loopback HTTP MCP; or
- stdio compatibility mode.

Never run both paths under the same client name, because duplicate MCP
processes can create confusing health and session results.

## Required Verification Before Commit

- `cargo fmt --check`;
- focused unit tests;
- MCP handshake and tool discovery test;
- transport failure/reconnect tests;
- embedding dimension guard tests;
- Neon idempotent replay test;
- archive corruption and restore tests;
- nested Main/Law vault exclusion tests;
- staged secret scan;
- review of only task-owned files;
- confirm no destructive reset command is referenced by automation;
- confirm local and remote `main` revisions only after an explicit release.

## Implementation Order

1. Fix dimension defaults, `.env.example`, model metadata, and fail-closed
   startup checks.
2. Add `memory doctor` and transport diagnostics.
3. Complete and test persistent loopback HTTP MCP.
4. Keep and test the stdio compatibility adapter.
5. Harden event cursors, leases, replay, and paginated audits.
6. Harden archive manifests, independent verification, and restore drills.
7. Add memory promotion, supersession, provenance, and storage catalog views.
8. Run the isolated candidate-platform evaluation.
9. Deploy to Mac first, then reproduce the verified revision on the VPS.
10. Only then enable scheduled sync/archive services and publish to `main`.

## Data-Loss Rules

- No reset is automatic.
- No local raw data is deleted after a failed upload.
- No archive is considered valid without checksum and manifest verification.
- No queue entry is acknowledged before target commit.
- No event is overwritten; newer events supersede older events.
- No conflicting records are silently discarded.
- No embedding is compared across model generations or dimensions.
- No client is told memory is healthy unless the live health check proves it.


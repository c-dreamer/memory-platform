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

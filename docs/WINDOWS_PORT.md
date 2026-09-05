# Windows Port — Working Plan

Scope: bring the memory platform to Windows as the **work** deployment, kept strictly separate
from the personal macOS and Linux deployments. Microtech client data is in scope, so security
decisions are made at the stricter bar throughout.

Status: planning and audit complete. No code changed yet.

---

## Manual tasks (Caleb)

These need a human — elevation, a UAC prompt, a credential, an account, or an approval. Nothing
here can or should be done by an agent.

### Approvals in flight

- [ ] Microtech sign-off on repo ownership for `c-dreamer/memory-platform` holding work code
- [ ] Microtech sign-off on **Neon** as a data subprocessor for client-derived content
- [ ] Microtech sign-off on **NVIDIA NIM** as a data subprocessor, if `EMBEDDING_MODEL=nvidia`
      stays (document content is sent to their API on every embed)
- [ ] Microtech sign-off before archives move from the local folder to the OneDrive tenant
- [ ] Tell Daniel that WSL2 / `VirtualMachinePlatform` is being enabled on this Intune-managed
      laptop, and that local listening ports appear — Huntress and DefensX may alert on both

### Installs (need elevation)

- [ ] `winget install --id Kitware.CMake -e` — **hold.** Its only consumer is the `fastembed`
      feature, which cannot currently produce a storable vector. See Blocker 1.
- [ ] `wsl --install --no-distribution`
- [ ] `winget install --id RedHat.Podman-Desktop -e`
- [ ] Decide on a Defender exclusion for `target\` — real build speedup, real reduction in
      scanning coverage on a machine that handles client data

### Credentials and config

- [ ] Set a git identity (global, or repo-local if you'd rather not make it global). Currently
      unset, so `git commit` fails outright.
- [ ] Create `.env` (gitignored). Windows values differ from `.env.example` in five places:
      `DATABASE_URL` port **5433**, `REDIS_URL` on `127.0.0.1`, `NEO4J_URI` on `127.0.0.1`,
      a Windows `VAULT_PATH`, and a `MEMORY_ARCHIVE_ROOT` pointing at a local folder.
- [ ] Set a stable `MEMORY_DEVICE_ID` for this box — do not rely on the `hostname -s` fallback,
      which does not work in Git Bash.
- [ ] Save `NVIDIA_API_KEY` and the `NEON_*` URLs yourself. Never paste them into a session.
- [ ] Create a **separate Neon project** for work. This is the actual personal/work boundary;
      everything else is labelling.
- [ ] Generate the archive encryption key and store it in Keeper. If this key is lost, cold
      storage is unrecoverable — there is no recovery path by design.
- [ ] Decide whether to sign in personal OneDrive at all, or keep archives local until the
      Microtech tenant is approved.

---

## Blockers found during audit

### 1. There is no working offline embedding path

`LocalEmbedding` hardcodes `EmbeddingModel::AllMiniLML6V2`, which produces **384** dimensions
(`src/services/embedding.rs:83`). `expected_dimension` comes from `EMBEDDING_DIM`, which is 2048
(`src/main.rs`, `.env.example`). `src/services/embedding.rs:133` errors on any mismatch, and every
embedding column in the schema is `VECTOR(2048)`.

So `--features fastembed` compiles and then fails at runtime on every embed call. The only backend
that works today is `nvidia`, which is a network call — meaning the platform cannot create new
embeddings while offline, on any platform. This is not Windows-specific; the Windows port simply
surfaced it.

Needs a decision: pick a local model whose dimension matches, add a separate lower-dimension
column and treat local vectors as a distinct space, or accept that offline capture stores content
now and embeds later when Neon and NVIDIA are reachable.

Note that mixing 384-dimension local vectors and 2048-dimension NVIDIA vectors in one column is
not merely a storage problem — similarity scores across two different embedding spaces are
meaningless even when the dimensions are forced to agree.

### 2. `windows-latest` cannot join the existing CI matrix

`.github/workflows/ci.yml` uses a `services:` block for `pgvector/pgvector:pg17`. GitHub Actions
service containers run **only on Linux runners**. Adding `windows-latest` to that matrix gives a
job whose database never starts.

Windows needs its own job with no `services:` block, running the checks that need no database
(`cargo build`, `cargo fmt --check`, `cargo clippy`, `cargo test --lib`). Integration tests stay
on Linux. Also note the job-level `RUSTFLAGS: "-D warnings"` — any Windows-only warning becomes a
build failure, so expect the first Windows run to fail on warnings alone.

### 3. Event sync orders by wall clock, not by logical time

`src/bin/neon-sync.rs:514` pushes with `ORDER BY created_at,event_id`, and the pull cursor at
`:606` compares `(created_at,event_id) > (cursor_created_at, cursor_event_id)`. The cursor only
moves forward.

`sync_meta.events` carries a `logical_time` column, and the pull path ignores it.

Consequence: an event written on a device whose clock lags behind another device's already-advanced
cursor is **never pulled**. It stays `pushed_at`-clean locally and silently absent everywhere else.
Two devices in different timezones already carry this risk; adding a third — Windows, SAST, on
home fibre and a phone hotspot with no NTP guarantee — increases it. Timezone offset itself is not
the problem (`TIMESTAMPTZ` normalises), but unsynchronised clocks are.

### 4. Production degraded mode uses a test-only constructor

`src/main.rs:56` falls back to `PostgresDb::new_empty()` when Postgres is unreachable.
`src/db/postgres.rs:144` documents that constructor as "Use only for unit tests that exercise
handler structure, not database access", and it connects lazily to
`postgresql://localhost:5432/nonexistent`.

So when the database is down the daemon logs a warning, reports itself started, binds its port, and
fails every single request. On Windows this is likely rather than theoretical: a Task Scheduler job
that fires before WSL and the Podman machine are up hits exactly this path. For an offline write
cache, "started successfully but cannot store anything" is the worst of the available behaviours.

---

## Foreseeable pitfalls

### Containers and WSL

- Podman's `restart: unless-stopped` needs `podman-restart.service` enabled **inside** the machine,
  or containers do not come back after a reboot. Easy to miss, and it looks like data loss.
- The Podman machine does not start at login by default. Any scheduled job that assumes a database
  is a job that fails until it is configured to wait.
- `docker-compose.yml` publishes to all interfaces (`"5433:5432"`, `"6379:6379"`, `"7474:7474"`,
  `"7687:7687"`). Bind them to `127.0.0.1:` explicitly. Redis has no authentication at all, and the
  Postgres and Neo4j passwords are committed as `password`.
- WSL2 memory is uncapped by default and will happily take most of the 15.4 GB on this machine.
  Neo4j's JVM is the heaviest tenant. A `.wslconfig` memory ceiling avoids a laptop that swaps
  during a Teams call.

### OneDrive as archive storage

- **Files On-Demand turns synced files into placeholders.** `verify-memory-archive.sh` runs
  `shasum -a 256` over `documents.ndjson`, which forces a hydration download per bundle. Offline,
  that fails and reports as a missing or mismatched archive — a false alarm that looks exactly like
  corruption.
- The verification checksum in `archive_meta.bundles` is computed over the **plaintext**. Once
  encryption is added, decide explicitly whether the manifest records the plaintext or ciphertext
  hash, or verification breaks the day encryption lands.
- OneDrive is sync, not backup. A local deletion propagates. Cold storage that follows local
  deletions is not cold storage.
- `restore-archive-documents.sh` honours no `MEMORY_ARCHIVE_ROOT` — it reads `remote_path` from
  `archive_meta.bundles`. A bundle recorded on macOS stores a macOS path, so cross-device restore
  from Windows resolves to a path that does not exist.

### Scheduling

- The seven `launchd` jobs become Task Scheduler tasks. Task Scheduler has no `StartInterval`
  equivalent that behaves identically — repetition intervals are configured differently and behave
  differently on wake from sleep.
- `flock` does not exist in Git Bash, so `scripts/run_embedding_repair_detached.sh:53` has no lock
  on Windows. Two overlapping repair runs are possible.
- Laptops sleep. A missed calendar-interval job on a machine that was asleep at 03:00 needs
  "run task as soon as possible after a scheduled start is missed" set deliberately.
- Scheduled tasks run in a session without the interactive PATH. Every script needs absolute paths
  to `bash`, `psql` and the built binaries.

### Security and secrets

- `psql "$DATABASE_URL"` appears in 10 places. On Windows the full command line of a process is
  readable by any process running as the same user, and by administrators through Task Manager or
  WMI. `AGENTS.md` explicitly forbids passing database URLs in command arguments. `PGPASSFILE` or
  `PGSERVICEFILE` fixes it.
- `scripts/check_secrets.sh` matches vendor prefixes only. A `postgres://user:password@host`
  connection string, a PEM private key block, an Azure key and an AWS secret all pass clean.
- `src/api/auth.rs:53` compares the API key with `!=`, which is not constant-time.
- `.env` is gitignored, but `set -a; . "$ROOT/.env"; set +a` in the scripts means any log or trace
  that dumps the environment carries every secret in it.

### Windows specifics

- Git Bash lacks `uuidgen` and its `hostname` rejects `-s`. Both are used by
  `scripts/archive-documents.sh`.
- `python3` on PATH is the Windows Store stub: it prints "Python was not found", exits 49, and
  under `set -euo pipefail` surfaces as a confusing unrelated failure. Two callers. Use `py`.
- `/usr/bin/link.exe` is Git Bash's coreutils `link` and shadows the MSVC linker that `rustc`
  spawns by that exact name. Run `cargo` from PowerShell.
- The dashboard reports OrbStack container state (`s.orbstack_containers` in
  `src/bin/memory-dashboard.rs`). On Windows that panel is either empty or wrong until it learns
  about Podman.
- `scripts/rehydrate_local.sh:29` shells out to `docker run` and pulls `postgres:18`, which has no
  pgvector, unlike compose and CI on `pgvector/pgvector:pg17`.

### Compliance and data boundary

- Neon and NVIDIA both become data subprocessors for client-derived content the moment work data
  flows through them. That is an approval question, not a technical one.
- A personal Neon account holding Microtech client data is the same question wearing different
  clothes.
- Archives in the Microtech OneDrive tenant are subject to company retention and eDiscovery.
  That is not automatically wrong — it is the correct place for company data — but it must be a
  decision rather than a side effect.
- `humanoracle26@gmail.com` is hardcoded as the default archive path in
  `scripts/archive-documents.sh:9` and `scripts/verify-memory-archive.sh:9`. On a work deployment
  that default should fail loudly rather than silently point at personal storage.

---

## Work order

1. **API hardening** — bind `127.0.0.1` by default via a configurable `API_BIND`, refuse to start
   on an empty `API_KEY`, CORS from an explicit allowlist defaulting to empty, constant-time key
   compare, request body cap. All platforms, not Windows-only. Self-contained, no schema change,
   no effect on live sync. Costs one `API_KEY=` line per existing deployment.
2. **Scope guard, optional like Redis/Neo4j** — same shape as those two: absent config means
   today's behaviour (sync everything, no gate), so existing Mac/Linux deployments are untouched
   until they opt in. Two independent levels, both off by default:
   - **Device scope** — `MEMORY_SCOPE=work|personal` in config, a `sync_meta.scope` marker row per
     database, `neon-sync` refuses to run when config and target disagree. Needs a one-time
     `set-scope` init on the existing Neon project.
   - **Category scope-in/out** — a table_name/vault_section allow-or-deny list
     (`MEMORY_SYNC_INCLUDE` / `MEMORY_SYNC_EXCLUDE`), checked in `neon-sync`'s push and pull
     queries. `documents.vault_section` already exists (`migrations/001_initial.sql:79`) and
     covers documents; other synced tables (`memories`, `experiences`, `code_changes`, …) have no
     equivalent column today, so category scoping needs either a shared `scope_tag` column added
     to each, or scoping restricted to `documents` until that's decided.
   Show the diff before either lands near `main` — this changes your live sync topology.
3. **Client-side encryption** in `archive-documents.sh`, with the manifest-checksum decision from
   above settled explicitly. Unblocks archives ahead of the OneDrive approval.
4. **`PGPASSFILE`** across the 10 `psql` call sites.
5. **Windows script fixes** — `flock`, `python3`, `uuidgen`, `hostname -s`, `docker run`.
6. **Windows CI job**, separate from the Linux matrix, no `services:` block.

Blockers 1, 3 and 4 above each need a decision before their fix can be written. 2 is a code fix
that can land any time.

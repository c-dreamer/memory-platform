# Memory Platform

A Rust-based memory and knowledge management platform with hybrid search, embedding-based retrieval, contradiction detection, and decay-aware scoring.

## Features

<<<<<<< HEAD
- **MCP Server** — JSON-RPC 2.0 stdio server (`src/mcp/`) exposing 22 tools plus a read-only `memory://` resources surface, for Claude Code, Codex, OpenCode, and other MCP clients
- **Hybrid Search** — Combines vector (pgvector), BM25, and full-text search with Reciprocal Rank Fusion (RRF)
- **Embedding Service** — NVIDIA NIM embeddings (`nvidia/nemotron-3-embed-1b`, 2048-dim) with LRU caching and a fail-closed dimension guard
- **Vault Ingestion** — Walks Obsidian vault directories, parses frontmatter, chunks by markdown headers, embeds, and upserts
- **Contradiction Detection** — Finds semantically similar memories with opposing signals using negation word pairs
- **Experience Tracking** — Records interactions, updates confidence scores, and applies Ebbinghaus-inspired decay
- **Procedure System** — Detects, executes, and records reusable procedures
- **Context Service** — Builds enriched context packages from recent memories, sessions, experiences, and procedures
- **Memory Decay** — Ebbinghaus half-life formula (recency + frequency + importance) applied at query time with configurable weights
- **Durable Event Queue** — Local SQLite WAL-mode pending-writes queue ahead of Postgres so a momentary outage never silently drops an event

## Architecture

```
┌──────────────┐     ┌──────────────┐     ┌──────────────┐
│   Axum API   │────▶│              │────▶│   Database   │
│  (REST/JSON) │     │   Services   │     │  (Postgres)  │
├──────────────┤     │  (Business   │     │  + Redis*    │
│  MCP Server  │────▶│   Logic)     │────▶│  + Neo4j*    │
│ (JSON-RPC 2.0│     │              │     │ (*optional)  │
│  over stdio) │     └──────┬───────┘     └──────────────┘
└──────────────┘            │
                             ▼
                      ┌──────────────┐
                      │    Search    │
                      │    Engine    │
                      │   (Hybrid)   │
                      └──────────────┘
```

Both the Axum HTTP API and the MCP stdio server sit in front of the same `AppState`
(db, search, and business-logic services) — they are two independent entry points
into one core, not layered on top of each other. See `src/mcp/mod.rs` for the MCP
JSON-RPC dispatcher and `src/mcp/tools.rs` for the tool implementations.

### Layers

| Layer | Crate | Description |
|---|---|---|
| **API** | `axum` | REST endpoints (23 routes, see `src/api/mod.rs`) with auth, CORS, and DTO validation |
| **MCP** | custom (JSON-RPC 2.0 / stdio) | 22 tools + a read-only `memory://` resources surface (`src/mcp/`) |
| **Services** | Custom | 7 business logic services (ingestion, embedding, decay, contradiction, experience, procedure, context) |
| **Search** | Custom | Hybrid search with vector (pgvector), BM25, full-text, and RRF or RSF fusion |
| **DB** | `sqlx` | PostgreSQL with 16 tables, pgvector extension; plus a local SQLite pending-writes queue (`src/queue/`) |
| **Cache** | `redis` | Optional at runtime — degrades gracefully if unreachable (not cargo-feature-gated) |
| **Graph** | `neo4rs` | Optional Neo4j for relationship queries — cargo feature `neo4j` (on by default), degrades gracefully if unreachable |

## Quick Start

### Prerequisites

- Rust 1.83+ (MSRV pinned in `Cargo.toml`; `rust-toolchain.toml` tracks the `stable` channel)
- PostgreSQL 15+ with pgvector extension
- Optional: Redis, Neo4j
- On Windows: MSVC Build Tools + Windows SDK (for the linker), and run `cargo` from PowerShell, not Git Bash — see `CLAUDE.md`

### Setup

```bash
# Clone and build
git clone https://github.com/c-dreamer/memory-platform.git
cd memory-platform
cp .env.example .env   # Edit for your environment

# Run database migrations
cargo run --bin memory-platform  # Runs migrations on startup

# Run tests
cargo test
cargo test --test integration

# Run with live database
DATABASE_URL=postgresql://... cargo test --test integration -- --include-ignored
```

### Bootstrap

For a full local recovery after cloning the repo:

```bash
./scripts/bootstrap.sh
```

This will:

1. Build the release binaries used by the memory MCP and ingest workflow.
2. Rehydrate the local Postgres store from Neon.
3. Re-ingest the live vault, OpenCode sessions, Codex sessions, config, and logs.
4. Verify backup coverage, including the Numerai model backup under `gdrive:backups/numerai/models`.

### Persistent MCP

OpenCode should point to the HTTP MCP endpoint served by the main daemon:

- Build with `transport-http` enabled.
- Run `memory-platform` as a persistent user service.
- Point OpenCode at `http://127.0.0.1:8000/mcp`.

Recommended service control (macOS/Linux, via the systemd user unit in `systemd/user/`):

```bash
systemctl --user enable --now memory-platform.service
```

**Windows** has no systemd. Two things stand in for it:

- MCP clients (Claude Code, Codex CLI, OpenCode, etc.) launch `mcp-server.exe` over
  stdio through `scripts/mcp-transport-guard.ps1`, invoked as
  `powershell.exe -File scripts\mcp-transport-guard.ps1` — see AGENTS.md's
  "Windows MCP Client Configuration" table for the exact config per client.
- Scheduled jobs (sync, maintenance, archive verification) install as Windows Task
  Scheduler tasks via `scripts/install-taskscheduler.ps1`, from the templates in
  `taskscheduler/*.xml.template` (the Task Scheduler counterpart to `launchd/` on
  macOS and `systemd/user/` on Linux).

### Configuration

All configuration is via environment variables (see `.env.example`):

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | `postgresql://memory:password@memory-postgres:5432/memory` | PostgreSQL connection string |
| `REDIS_URL` | `redis://memory-redis:6379/0` | Redis connection string (optional at runtime) |
| `NEO4J_URI` | `bolt://memory-neo4j:7687` | Neo4j bolt URI (optional at runtime) |
| `API_KEY` | (empty = dev mode) | API key for auth |
| `API_PORT` | `8000` | HTTP server port |
<<<<<<< HEAD
| `EMBEDDING_MODEL` | `local` | Embedding backend. `nvidia` is the working backend (`.env.example` sets it). `local` pairs with `MEMORY_LOCAL_ONLY` and has no working backend yet, so it fails fast rather than silently reaching NVIDIA |
| `NVIDIA_EMBEDDING_MODEL` | `nvidia/nemotron-3-embed-1b` | NVIDIA embedding model (2048-dim). Supersedes the EOL `llama-nemotron-embed-1b-v2` |
| `VAULT_PATH` | `/vault` | Path to Obsidian vault |
| `MEMORY_LOCAL_ONLY` | (empty = off) | When set truthy, refuses to start unless `EMBEDDING_MODEL=llama-cpp` and no cloud credential (NVIDIA/OpenAI/Obsidian API key) is set; also blocks `neon-sync`/`memory-dashboard` from dialing Neon |
| `SYNC_TARGET_URL` | (empty) | Direct, non-pooled Postgres+pgvector endpoint for `neon-sync` (Neon, Supabase, or self-hosted) — see AGENTS.md |

See `.env.example` for the full list (search, decay, chunking, archive-encryption, and other tunables).

### Stats

Use the stats CLI to inspect a single database or compare two URLs:

```bash
cargo run --quiet --bin stats -- --db-url "$DATABASE_URL"
cargo run --quiet --bin stats -- --compare "$LOCAL_URL" "$NEON_URL"
```

The compare mode prints both database URLs, their sizes in MB, and the delta for the core tables.

## Development

```bash
# Build
cargo build

# Check warnings
cargo check

# Run all unit tests
cargo test --lib

# Format code
cargo fmt

# Lint
cargo clippy
```

## Project Structure

```
src/
├── api/
│   ├── auth.rs          # API key auth extractor
│   ├── dto.rs           # Request/response DTOs
│   ├── handlers/        # 10 handler modules (agents, context, events, experiences,
│   │                     #   ingestion, memories, procedures, root, search, sessions)
│   └── mod.rs           # Router definition — 23 routes
├── bin/                  # 10 binaries: mcp-server, memory-dashboard, neon-sync,
│                         #   ingest, vault-sync, stats, search_eval, reindex,
│                         #   repair_embeddings, archive-crypt
├── config.rs             # Environment config
├── db/
│   ├── postgres.rs       # PostgreSQL — 42 pub methods on PostgresDb
│   ├── redis.rs          # Redis cache (optional at runtime)
│   └── neo4j.rs          # Neo4j graph client (optional at runtime)
├── ingest/                # Batch ingestion of external sources (OpenCode/Codex
│                          #   sessions, OpenCode config/rules, OpenCode logs)
├── lib.rs                # AppState definition + module declarations
├── main.rs               # HTTP server entry point
├── mcp/
│   ├── mod.rs            # JSON-RPC 2.0 stdio dispatcher
│   ├── tools.rs          # 22 MCP tool implementations
│   ├── resources.rs      # Read-only `memory://` resources surface
│   ├── hooks.rs          # Session lifecycle hooks (start/end/pre-compact)
│   └── transport.rs      # Feature-gated (`transport-http`) HTTP transport
├── migrations/
│   └── mod.rs            # SQL migration runner (embedded via include_str!)
├── models/                # 15 data model modules (one per table, plus events)
├── queue/                 # Local SQLite WAL-mode pending-writes queue
├── search/
│   ├── bm25.rs           # BM25 keyword scoring
│   ├── mod.rs            # SearchEngine orchestrator
│   ├── rrf.rs            # Reciprocal Rank Fusion
│   ├── rsf.rs            # Relative Score Fusion
│   └── vector.rs         # Vector (pgvector) search
└── services/
    ├── context.rs        # Context assembly
    ├── contradiction.rs  # Contradiction detection
    ├── decay.rs          # Ebbinghaus decay engine
    ├── embedding.rs      # Embedding service (nvidia / llama-cpp backends)
    ├── experience.rs     # Experience tracking
    ├── ingestion.rs      # Vault ingestion
    └── procedure.rs      # Procedure execution
migrations/               # 21 SQL migration files
tests/
├── integration.rs        # Integration tests (DB-gated tests are #[ignore])
└── mcp.rs                # MCP protocol tests
```

## Database Schema

16 tables in the `public` schema, with vector, full-text, and decay-aware indexes
(plus `archive_meta.*` and `sync_meta.*` tables in their own schemas for the
cold-archive and Neon-sync outbox — see AGENTS.md):

- `agents`, `sessions`, `memories`, `documents`
- `experiences`, `procedures`, `trading_results`
- `contradictions`, `relationships`, `projects`
- `code_changes`, `summaries`, `embeddings`, `config`
- `session_documents`, `session_memories`

Migrations live in `migrations/*.sql` and are embedded into the binary at compile
time; `src/migrations/mod.rs` is the authoritative, ordered list. Note the
duplicate `010_` version prefix: both `010_shared_event_sync` and
`010_session_source_keys` are real, independently-tracked migrations — the
prefix collision is a naming quirk, not a bug, since each is recorded by its
full version string.

## License

_(No `LICENSE` file is present in this repository — verify licensing before
publishing or relying on this project. `Cargo.toml` declares `license =
"Apache-2.0"`, but that alone is not a substitute for a `LICENSE` file.)_

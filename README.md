# Memory Platform

A Rust-based memory and knowledge management platform with hybrid search, embedding-based retrieval, contradiction detection, and decay-aware scoring.

## Features

- **Hybrid Search** — Combines vector (pgvector), BM25, and full-text search with Reciprocal Rank Fusion (RRF)
- **Embedding Service** — NVIDIA NIM embeddings (`nvidia/nemotron-3-embed-1b`, 2048-dim) with LRU caching and a fail-closed dimension guard
- **Vault Ingestion** — Walks Obsidian vault directories, parses frontmatter, chunks by markdown headers, embeds, and upserts
- **Contradiction Detection** — Finds semantically similar memories with opposing signals using 30 negation word pairs
- **Experience Tracking** — Records interactions, updates confidence scores, and applies Ebbinghaus-inspired decay
- **Procedure System** — Detects, executes, and records reusable procedures
- **Context Service** — Builds enriched context packages from recent memories, sessions, experiences, and procedures
- **Memory Decay** — Ebbinghaus half-life formula applied at query time with configurable decay parameters
- **Decay Engine** — Configurable half-life decay to prioritize recent and frequently accessed information

## Architecture

```
┌─────────────┐     ┌──────────────┐     ┌─────────────┐
│   Axum API   │────▶│  Services    │────▶│  Database   │
│  (REST/JSON) │     │  (Business   │     │  (Postgres) │
└─────────────┘     │   Logic)     │     │  + Redis    │
                    └──────────────┘     │  + Neo4j    │
                          │              └─────────────┘
                          ▼
                    ┌──────────────┐
                    │  Search      │
                    │  Engine      │
                    │  (Hybrid)    │
                    └──────────────┘
```

### Layers

| Layer | Crate | Description |
|---|---|---|
| **API** | `axum` | REST endpoints with auth, CORS, and DTO validation |
| **Services** | Custom | 7 business logic services (ingestion, embedding, decay, contradiction, experience, procedure, context) |
| **Search** | Custom | Hybrid search with vector (pgvector), BM25, full-text, and RRF fusion |
| **DB** | `sqlx` | PostgreSQL with 15 tables, pgvector extension |
| **Cache** | `redis` | Optional Redis for TTL-based caching |
| **Graph** | `neo4rs` | Optional Neo4j for relationship queries |

## Quick Start

### Prerequisites

- Rust 1.85+ (see `rust-toolchain.toml`)
- PostgreSQL 15+ with pgvector extension
- Optional: Redis, Neo4j

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

Recommended service control:

```bash
systemctl --user enable --now memory-platform.service
```

### Configuration

All configuration is via environment variables (see `.env.example`):

| Variable | Default | Description |
|---|---|---|
| `DATABASE_URL` | `postgresql://memory:password@memory-postgres:5432/memory` | PostgreSQL connection string |
| `REDIS_URL` | `redis://memory-redis:6379/0` | Redis connection string |
| `NEO4J_URI` | `bolt://memory-neo4j:7687` | Neo4j bolt URI |
| `API_KEY` | (empty = dev mode) | API key for auth |
| `API_PORT` | `8000` | HTTP server port |
| `EMBEDDING_MODEL` | `local` | Embedding backend. `nvidia` is the working backend (`.env.example` sets it). `local` pairs with `MEMORY_LOCAL_ONLY` and has no working backend yet, so it fails fast rather than silently reaching NVIDIA |
| `NVIDIA_EMBEDDING_MODEL` | `nvidia/nemotron-3-embed-1b` | NVIDIA embedding model (2048-dim). Supersedes the EOL `llama-nemotron-embed-1b-v2` |
| `VAULT_PATH` | `/vault` | Path to Obsidian vault |

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
│   ├── handlers/        # 11 endpoint handlers
│   ├── mod.rs           # Router definition
├── bin/
│   └── mcp-server.rs    # MCP stdio binary
├── config.rs            # Environment config
├── db/
│   ├── postgres.rs      # PostgreSQL with 37 methods
│   ├── redis.rs         # Redis cache
│   └── neo4j.rs         # Neo4j graph client
├── lib.rs               # AppState module declarations
├── main.rs              # HTTP server entry point
├── mcp/
│   └── mod.rs           # MCP server placeholder
├── migrations/
│   └── mod.rs           # SQL migration runner
├── models/              # 15 data models
├── search/
│   ├── bm25.rs          # BM25 keyword scoring
│   ├── mod.rs           # Search engine
│   ├── rrf.rs           # RRF fusion
│   └── vector.rs        # Vector search
└── services/
    ├── context.rs       # Context assembly
    ├── contradiction.rs # Contradiction detection
    ├── decay.rs         # Ebbinghaus decay engine
    ├── embedding.rs     # Embedding service
    ├── experience.rs    # Experience tracking
    ├── ingestion.rs     # Vault ingestion
    └── procedure.rs     # Procedure execution
migrations/              # SQL migration files
tests/
└── integration.rs       # Integration tests
```

## Database Schema

15 tables with vector, full-text, and decay-aware indexes:

- `agents`, `sessions`, `memories`, `documents`
- `experiences`, `procedures`, `trading_results`
- `contradictions`, `relationships`, `projects`
- `code_changes`, `summaries`, `events`
- `config_entries`, `embeddings`

## License

MIT

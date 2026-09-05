# CLAUDE.md

@AGENTS.md

`AGENTS.md` is the canonical, tool-agnostic operations contract for this repo — it must keep
working for any AI assistant. Do not rewrite it into Claude-specific instructions. Anything that
is only true for Claude Code, or only true for one machine, belongs in this file instead.

## Current work: extend to Windows

macOS and Linux are already configured and running. Windows support is the open track, alongside
general improvements. What that actually involves, as of 2026-09-04:

- The Rust core has **zero** `cfg(target_os)` / `cfg(unix)` / `cfg(windows)` blocks. Treat it as
  portable until proven otherwise, and keep it that way — prefer `std::path::PathBuf`,
  `dirs`-style resolution, and env vars over hardcoded POSIX paths.
- Platform coupling lives outside `src/`: 25 bash scripts, `launchd/` (macOS), `systemd/` (Linux),
  `macos-app/` (Swift), and `docker-compose.yml`. Windows equivalents are scheduled tasks and
  PowerShell, not new `#[cfg]` branches in the core.
- `.env.example` defaults are macOS-shaped (`MEMORY_RUNTIME=homebrew`, an iCloud `VAULT_PATH`,
  `host.docker.internal`). Windows needs its own documented defaults, not edits to the macOS ones.
- CI (`.github/workflows/ci.yml`) runs `ubuntu-latest` and `macos-latest` only. Adding
  `windows-latest` to that matrix is the acceptance test for "Windows is supported" — don't claim
  Windows support before it's green there.

## This Windows machine

Verified 2026-09-05:

- `cargo` / `rustc` **1.98.1**, host `x86_64-pc-windows-msvc`. `gh` **2.100.0** (not yet
  authenticated — `gh auth login`).
- **No MSVC C++ toolchain and no Windows SDK.** `cargo check --all-targets` fails on every build
  script at link time. Visual Studio Build Tools with the VC workload is the missing piece;
  nothing else native is required (see below).
- **Do not run `cargo` from Git Bash.** `/usr/bin/link.exe` is Git Bash's coreutils `link`, and it
  shadows the MSVC linker that `rustc` spawns by that exact name. The resulting error is
  `link: extra operand ...  Try 'link --help' for more information.`, which looks like a Rust bug
  and is not one. Run `cargo` from PowerShell.
- Native dependency audit (from `Cargo.lock` and `cargo tree -e normal`): `ring` 0.17.14 ships
  pregenerated assembly, so **no nasm**. `openssl-sys` is in the lock file but not in the default
  build graph, so **no OpenSSL and no perl**. `cmake` and `protoc` are unnecessary. The only
  crates that would need `cmake` plus an ONNX Runtime download are `ort` / `tokenizers` /
  `esaxx-rs`, and those come in solely through the **optional, non-default** `fastembed` feature.
  Enabling `--features fastembed` on Windows is therefore its own separate piece of work.
- **No Docker and no container runtime**, deliberately, and WSL is not installed either
  (`wsl -l -v` reports "The Windows Subsystem for Linux is not installed"). Never propose
  `docker-compose up` as the local path here. Postgres, Redis and Neo4j are not running locally.
- Good news for local checks: the repo uses only runtime `sqlx::query(` and no `sqlx::query!`
  macros, so `cargo check`, `cargo clippy` and `cargo fmt` need no `DATABASE_URL` and no live
  database once the linker works. `tests/integration.rs` does need Postgres with pgvector — point
  it at a throwaway Neon branch rather than standing up a local server.

## Working style

- Shortest change that actually fixes the root cause. No speculative abstractions, no interface
  with one implementation, no config for a value that never changes.
- Verify against the code before asserting. Grep every caller before changing a shared function.
- Never commit, push, or branch without being asked. Hand over the exact command instead.
- Never accept or write a pasted secret. Env file only, per `AGENTS.md`.

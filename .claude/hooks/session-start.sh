#!/bin/bash
# SessionStart hook for Claude Code cloud sessions.
# Warms the Rust toolchain and cargo caches so `cargo test`, `cargo clippy`
# and `cargo fmt` are fast in the session. Runs synchronously and idempotently.
set -euo pipefail

if [ "${CLAUDE_CODE_REMOTE:-}" != "true" ]; then
  exit 0
fi

PROJECT_DIR="${CLAUDE_PROJECT_DIR:-$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)}"
cd "$PROJECT_DIR"

export DEBIAN_FRONTEND=noninteractive
export CARGO_TERM_COLOR=never

log() { echo "[session-start] $*"; }

# --- Native build prerequisites -------------------------------------------
# Default features need only a C compiler (ring, bundled libsqlite3-sys) and
# pkg-config. libssl/libpq are not in the default dependency graph (rustls,
# sqlx without openssl), and the optional `fastembed` feature (ORT + cmake)
# is deliberately NOT enabled.
missing_pkgs=()
command -v cc >/dev/null 2>&1 || missing_pkgs+=(build-essential)
command -v pkg-config >/dev/null 2>&1 || missing_pkgs+=(pkg-config)
if [ "${#missing_pkgs[@]}" -gt 0 ]; then
  log "installing apt packages: ${missing_pkgs[*]}"
  SUDO=""
  if [ "$(id -u)" -ne 0 ] && command -v sudo >/dev/null 2>&1; then
    SUDO="sudo"
  fi
  $SUDO apt-get update -qq
  $SUDO apt-get install -y -qq --no-install-recommends "${missing_pkgs[@]}"
else
  log "native build prerequisites already present"
fi

# --- Rust toolchain -------------------------------------------------------
# rust-toolchain.toml pins the `stable` channel with rustfmt/clippy/rust-src;
# `rustup show` installs or syncs it non-interactively if it is not present.
if command -v rustup >/dev/null 2>&1; then
  rustup show >/dev/null 2>&1 || rustup toolchain install stable --profile default
fi
log "using $(rustc --version) / $(cargo --version)"

# --- Cargo caches (default feature set only) ------------------------------
# Container state is cached after this hook, so a full debug build of every
# target (lib, bins, tests, benches, examples — same as CI's
# `cargo build --all-targets`) makes later `cargo test` / `cargo clippy`
# incremental. Cargo itself is the idempotence check: an up-to-date tree
# finishes in seconds.
log "cargo fetch"
cargo fetch --locked
log "cargo build --all-targets"
cargo build --all-targets --locked
# Clippy is run warn-only here so the crate's check artifacts are cached even
# though the pedantic/nursery/cargo lint groups currently fire (CI runs clippy
# with continue-on-error). Output goes to a log to keep session start quiet.
log "cargo clippy --all-targets (warm clippy cache; log: target/session-start-clippy.log)"
cargo clippy --all-targets --locked > target/session-start-clippy.log 2>&1 \
  || log "clippy exited non-zero; see target/session-start-clippy.log"

# --- Session environment --------------------------------------------------
if [ -n "${CLAUDE_ENV_FILE:-}" ]; then
  {
    echo 'export CARGO_TERM_COLOR=never'
    echo 'export RUST_BACKTRACE=1'
  } >> "$CLAUDE_ENV_FILE"
fi

log "done"

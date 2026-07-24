# Memory Dashboard

`memory-dashboard` is a standalone, local-only operations application. It is
separate from the MCP process and can run on either the Mac or the VPS.

## What it shows

- Pending local projection rows, published and unpublished sync events.
- Active and archived document totals.
- Last successful transfer, pause state, retry state, and scheduler type.
- Local database size, pending transfer bytes, active/archive tiers, source
  categories, and archive-bundle verification status through the storage audit.

It never returns credentials, database URLs, raw session text, or archive data.

## What it controls

- Start a bounded maintenance run now.
- Pause future maintenance and retries.
- Resume maintenance.
- Stop the current scheduled run and keep future maintenance paused.

## Closing the app safely

On macOS, Memory Platform owns the dashboard and maintenance LaunchAgents for
the duration of the app session. Closing the app stops those background tasks.
This does not discard sync work: the local event ledger and outbox are durable,
and an outbox entry is acknowledged only after Neon commits its transaction.
The next app launch replays an interrupted batch safely or resumes the remaining
queue. Session ingestion is source-idempotent, so a partially observed session
is updated rather than duplicated on the next run.

The dashboard binds only to `127.0.0.1:8765`. On the VPS, use an SSH tunnel:

```sh
ssh -L 8765:127.0.0.1:8765 user@vps
```

Then open `http://127.0.0.1:8765` locally. Do not expose this port publicly.

## Installation

Build the verified release, then install the dashboard service:

```sh
scripts/install-memory-release.sh
scripts/install-dashboard-runtime.sh
```

macOS installs a versioned runtime under `~/Library/Application Support/Memory
Platform/runtime/` and a user LaunchAgent pointing there. This intentionally
avoids using a temporary checkout path. Linux installs a systemd user service
and a matching maintenance service used by the dashboard controls.

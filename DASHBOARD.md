# Memory Dashboard

`memory-dashboard` is a standalone, local-only operations application. It is
separate from the MCP process and can run on either the Mac or the VPS.

## What it shows

- Pending local projection rows, published and unpublished sync events.
- Active and archived document totals.
- Last successful transfer, pause state, retry state, and scheduler type.

It never returns credentials, database URLs, raw session text, or archive data.

## What it controls

- Start a bounded maintenance run now.
- Pause future maintenance and retries.
- Resume maintenance.
- Stop the current scheduled run and keep future maintenance paused.

The dashboard binds only to `127.0.0.1:8765`. On the VPS, use an SSH tunnel:

```sh
ssh -L 8765:127.0.0.1:8765 user@vps
```

Then open `http://127.0.0.1:8765` locally. Do not expose this port publicly.

## Installation

Build the verified release, then install the dashboard service:

```sh
scripts/install-memory-release.sh
scripts/install-memory-dashboard.sh
```

macOS installs a user LaunchAgent. Linux installs a systemd user service and a
matching maintenance service used by the dashboard controls.

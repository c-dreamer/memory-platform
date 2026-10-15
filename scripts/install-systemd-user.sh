#!/usr/bin/env bash
set -euo pipefail

# Render systemd/user/* for this checkout into ~/.config/systemd/user (the Linux
# counterpart of install-neon-sync-launchd.sh). Units are copied, not linked, so
# re-run this after moving the checkout. Nothing is enabled; that stays your call.
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
UNITS="${XDG_CONFIG_HOME:-$HOME/.config}/systemd/user"

mkdir -p "$UNITS"
for unit in "$ROOT"/systemd/user/*.service "$ROOT"/systemd/user/*.timer; do
  sed "s|__ROOT__|$ROOT|g" "$unit" > "$UNITS/$(basename "$unit")"
done
systemctl --user daemon-reload

echo "Installed memory-platform user units into $UNITS for $ROOT. Enable what you need, e.g.:"
echo "  systemctl --user enable --now memory-platform.service"
echo "  systemctl --user enable --now memory-neon-sync.timer memory-ingest.timer memory-backup-verify.timer memory-archive-verify.timer"

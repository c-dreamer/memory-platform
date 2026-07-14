# VPS and OpenCode Rollout

Run these steps from the VPS clone after pulling the released `main` commit.

1. Confirm `rclone` can access the same encrypted archive remote and set
   `MEMORY_ARCHIVE_ROOT` to its mounted or rclone-backed archive location.
2. Keep credentials in `.env`; do not add them to Git or systemd units.
3. Run `./sync-to-neon.sh status`. Do not reset Neon or run a full dump.
4. Run `scripts/verify-memory-archive.sh` after the archive location is
   available. It is read-only and must pass before enabling timers.
5. Run one `scripts/restore-archive-documents.sh ARCHIVE_ID 1` drill only
   against a verified bundle, then `./sync-to-neon.sh run` and confirm queue
   depth reaches zero.
6. Enable `memory-archive-verify.timer` only after the status, verification,
   restore drill, and two no-op sync runs pass.

OpenCode must use the Rust `mcp-server` only. It may call `archive_status` to
inspect archive health, but must never access Google Drive directly, reset
Neon, or archive/compact records without a verified manifest.

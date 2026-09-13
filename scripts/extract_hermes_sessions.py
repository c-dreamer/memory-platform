#!/usr/bin/env python3
"""Extract Hermes session data from ~/.hermes/state.db and output as JSON lines.

Usage: python3 extract_hermes_sessions.py <path-to-state.db>

Output: One JSON object per line, compatible with the memory-platform
Rust `ingest sessions` consumer:
  {"type": "session", "data": {...}}
  {"type": "part", "session_id": "...", "data": {...}}
  {"type": "metadata", "total_sessions": N, "total_parts": M, "total_todos": 0}
"""

import json
import sqlite3
import sys
from datetime import datetime, timezone


def iso(ts):
    """Convert unix epoch float (Hermes) to ISO8601 UTC, or None."""
    if ts is None:
        return None
    try:
        return datetime.fromtimestamp(float(ts), tz=timezone.utc).isoformat()
    except (OSError, OverflowError, ValueError, TypeError):
        return None


def main():
    if len(sys.argv) < 2:
        print("Usage: extract_hermes_sessions.py <path-to-state.db>", file=sys.stderr)
        sys.exit(1)

    db_path = sys.argv[1]
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row

    required = {"sessions", "messages"}
    existing = set()
    for row in conn.execute("SELECT name FROM sqlite_master WHERE type='table'"):
        existing.add(row["name"])
    missing = required - existing
    if missing:
        print(f"Missing tables: {missing}", file=sys.stderr)
        sys.exit(1)

    session_rows = conn.execute("""
        SELECT id, source, model, title, started_at, ended_at,
               input_tokens, output_tokens, cache_read_tokens, cache_write_tokens,
               reasoning_tokens, message_count, tool_call_count, cwd, end_reason
        FROM sessions
        WHERE archived = 0
        ORDER BY started_at ASC
    """).fetchall()

    total_parts = 0

    for ses in session_rows:
        d = dict(ses)
        sid = d["id"]
        title = (d.get("title") or "").strip() or sid

        ses_dict = {
            "id": sid,
            "title": title,
            "agent_name": d.get("source") or "hermes",
            "model_name": d.get("model") or "unknown",
            "tokens_input": d.get("input_tokens") or 0,
            "tokens_output": d.get("output_tokens") or 0,
            "cost": 0.0,
            "time_created_iso": iso(d.get("started_at")),
            "time_updated_iso": iso(d.get("ended_at") or d.get("started_at")),
            "metadata": {
                "source": "hermes-session",
                "source_system": d.get("source"),
                "cwd": d.get("cwd"),
                "end_reason": d.get("end_reason"),
                "message_count": d.get("message_count") or 0,
                "tool_call_count": d.get("tool_call_count") or 0,
                "cache_read_tokens": d.get("cache_read_tokens") or 0,
                "cache_write_tokens": d.get("cache_write_tokens") or 0,
                "reasoning_tokens": d.get("reasoning_tokens") or 0,
            },
        }
        print(json.dumps({"type": "session", "data": ses_dict}))

        part_rows = conn.execute("""
            SELECT role, content, tool_name, timestamp, token_count
            FROM messages
            WHERE session_id = ? AND active = 1 AND compacted = 0
            ORDER BY timestamp ASC
        """, (sid,)).fetchall()

        for prt in part_rows:
            text = prt["content"]
            if not text:
                continue
            part_dict = {
                "session_id": sid,
                "role": prt["role"] or "unknown",
                "parsed_data": {"text": text},
                "tool_name": prt["tool_name"],
                "timestamp_iso": iso(prt["timestamp"]),
                "token_count": prt["token_count"] or 0,
            }
            print(json.dumps({"type": "part", "session_id": sid, "data": part_dict}))
            total_parts += 1

    print(json.dumps({
        "type": "metadata",
        "total_sessions": len(session_rows),
        "total_parts": total_parts,
        "total_todos": 0,
    }))
    conn.close()


if __name__ == "__main__":
    main()
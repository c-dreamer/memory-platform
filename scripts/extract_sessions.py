#!/usr/bin/env python3
"""Extract OpenCode session data from opencode.db and output as JSON lines.

Usage: python3 extract_sessions.py <path-to-opencode.db>

Output: One JSON object per line, containing all sessions with their message parts.
The output is designed to be consumed by `ingest sessions` via stdin pipe.

Format:
  {"type": "session", "data": {...}}
  {"type": "part", "session_id": "...", "data": {...}}
  {"type": "metadata", "total_sessions": 100, "total_parts": 5000}
"""

import json
import re
import sqlite3
import sys
from datetime import datetime, timezone


def _sanitise(obj):
    """Recursively replace lone surrogates (U+D800–U+DFFF) with U+FFFD."""
    if isinstance(obj, str):
        return re.sub(r'[\ud800-\udfff]', '\uFFFD', obj)
    if isinstance(obj, dict):
        return {k: _sanitise(v) for k, v in obj.items()}
    if isinstance(obj, list):
        return [_sanitise(v) for v in obj]
    return obj


def main():
    if len(sys.argv) < 2:
        print("Usage: extract_sessions.py <path-to-opencode.db>", file=sys.stderr)
        sys.exit(1)

    db_path = sys.argv[1]
    conn = sqlite3.connect(db_path)
    conn.row_factory = sqlite3.Row

    # Validate tables exist
    required = {'session', 'part', 'message'}
    existing = set()
    for row in conn.execute("SELECT name FROM sqlite_master WHERE type='table'"):
        existing.add(row['name'])

    missing = required - existing
    if missing:
        print(f"Missing tables: {missing}", file=sys.stderr)
        sys.exit(1)

    # --- Extract sessions ---
    session_rows = conn.execute("""
        SELECT id, project_id, slug, title, time_created, time_updated,
               tokens_input, tokens_output, tokens_reasoning, cost,
               agent, model, metadata
        FROM session
        ORDER BY time_created ASC
    """).fetchall()

    total_sessions = len(session_rows)
    total_parts = 0

    for ses in session_rows:
        ses_dict = dict(ses)

        # Parse agent JSON field
        agent_raw = ses_dict.get('agent')
        if agent_raw:
            try:
                agent_obj = json.loads(agent_raw)
                ses_dict['agent_name'] = agent_obj.get('name', agent_raw)
            except (json.JSONDecodeError, TypeError):
                ses_dict['agent_name'] = agent_raw if len(agent_raw) < 40 else 'agent'
        else:
            ses_dict['agent_name'] = 'unknown'

        # Parse model JSON field (could be a JSON object with 'modelID' or a simple string)
        model_raw = ses_dict.get('model')
        if model_raw:
            try:
                model_obj = json.loads(model_raw)
                # Try different key names used in OpenCode
                ses_dict['model_name'] = (model_obj.get('modelID') or
                                           model_obj.get('id') or
                                           model_obj.get('name') or
                                           str(model_raw))
                if len(ses_dict['model_name']) > 60:
                    ses_dict['model_name'] = 'unknown'
            except (json.JSONDecodeError, TypeError):
                # It's a plain string
                ses_dict['model_name'] = model_raw if len(model_raw) < 60 else 'unknown'
        else:
            ses_dict['model_name'] = 'unknown'

        # Normalise the display title so every session can be ingested.
        title = (ses_dict.get('title') or '').strip()
        if not title:
            title = (ses_dict.get('slug') or '').strip()
        if not title:
            title = ses_dict['id']
        ses_dict['title'] = title

        # Parse metadata
        meta_raw = ses_dict.get('metadata')
        if meta_raw:
            try:
                ses_dict['metadata_parsed'] = json.loads(meta_raw)
            except (json.JSONDecodeError, TypeError):
                ses_dict['metadata_parsed'] = {}
        else:
            ses_dict['metadata_parsed'] = {}

        # Clean up raw fields
        del ses_dict['agent']
        del ses_dict['model']
        del ses_dict['metadata']

        # Convert timestamps
        for ts_field in ['time_created', 'time_updated']:
            ts = ses_dict.get(ts_field)
            if ts:
                try:
                    dt = datetime.fromtimestamp(ts / 1000, tz=timezone.utc)
                    ses_dict[ts_field + '_iso'] = dt.isoformat()
                except (OSError, OverflowError):
                    ses_dict[ts_field + '_iso'] = None

        # Output session
        print(json.dumps({"type": "session", "data": ses_dict}))

        # --- Extract parts for this session ---
        # role is stored inside message.data as JSON, extract via json_extract
        part_rows = conn.execute("""
            SELECT p.id, p.message_id, p.time_created, p.data,
                   json_extract(m.data, '$.role') as role,
                   json_extract(m.data, '$.agent') as msg_agent,
                   json_extract(m.data, '$.model') as msg_model
            FROM part p
            JOIN message m ON m.id = p.message_id
            WHERE p.session_id = ?
            ORDER BY p.time_created ASC
        """, (ses['id'],)).fetchall()

        for prt in part_rows:
            prt_dict = dict(prt)
            try:
                prt_dict['parsed_data'] = json.loads(prt['data'])
            except (json.JSONDecodeError, TypeError):
                prt_dict['parsed_data'] = {}

            del prt_dict['data']
            total_parts += 1

            print(json.dumps({"type": "part", "session_id": ses['id'], "data": _sanitise(prt_dict)}))

    # --- Extract all user todos ---
    todo_rows = conn.execute("""
        SELECT session_id, content, status, priority, position, time_created, time_updated
        FROM todo
        ORDER BY session_id, position
    """).fetchall()

    for td in todo_rows:
        td_dict = dict(td)
        print(json.dumps({"type": "todo", "data": td_dict}))

    conn.close()

    # --- Final metadata line ---
    meta = {
        "type": "metadata",
        "data": {
            "total_sessions": total_sessions,
            "total_parts": total_parts,
            "total_todos": len(todo_rows),
            "db_path": db_path,
        }
    }
    print(json.dumps(meta))


if __name__ == '__main__':
    main()

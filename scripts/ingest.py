#!/usr/bin/env python3
"""Unified ingestion script for the memory platform.

Ingests all data sources into the memory platform Postgres database:
  - sessions: OpenCode sessions from opencode.db
  - vault: Obsidian vault .md files
  - config: OpenCode config, rules, skills
  - logs: OpenCode runtime log
  - all: everything above

Usage:
  python3 scripts/ingest.py --db-url <DATABASE_URL> sessions [--source <opencode.db>]
  python3 scripts/ingest.py --db-url <DATABASE_URL> vault --path <vault-dir>
  python3 scripts/ingest.py --db-url <DATABASE_URL> config [--dir <config-dir>]
  python3 scripts/ingest.py --db-url <DATABASE_URL> logs [--log <opencode.log>]
  python3 scripts/ingest.py --db-url <DATABASE_URL> all
"""

import argparse
import json
import os
import subprocess
import sys
import time
from datetime import datetime, timezone
from pathlib import Path


def expand_path(p: str) -> str:
    """Expand ~ to home directory."""
    return str(Path(p).expanduser())


def run_psql(db_url: str, sql: str) -> list[dict]:
    """Execute SQL via psql and return results as list of dicts."""
    result = subprocess.run(
        ["psql", db_url, "-t", "-A", "-F", "|", "-c", sql],
        capture_output=True, text=True, timeout=30,
    )
    if result.returncode != 0:
        print(f"  ⚠ psql error: {result.stderr.strip()}", file=sys.stderr)
        return []
    
    lines = [l.strip() for l in result.stdout.split("\n") if l.strip()]
    if not lines:
        return []
    return [{"value": l} for l in lines]


def check_ingested(db_url: str, opencode_session_id: str) -> bool:
    """Check if a session has already been ingested."""
    rows = run_psql(
        db_url,
        f"SELECT 1 FROM memories WHERE metadata @> '{{\"opencode_session_id\": \"{opencode_session_id}\"}}'::jsonb LIMIT 1"
    )
    return len(rows) > 0


def insert_session(db_url: str, goal: str, status: str, started_at: str, ended_at: str) -> str | None:
    """Insert a session record and return its UUID."""
    if ended_at is None:
        ended_at_sql = "NULL"
    else:
        ended_at_sql = f"'{ended_at}'"
    started_at_esc = started_at if started_at else datetime.now(timezone.utc).isoformat()
    
    sql = f"""
    INSERT INTO sessions (goal, status, started_at, ended_at)
    VALUES ({_esc(goal)}, '{status}', '{started_at_esc}', {ended_at_sql})
    RETURNING id;
    """
    result = subprocess.run(
        ["psql", db_url, "-t", "-A", "-c", sql],
        capture_output=True, text=True, timeout=10,
    )
    if result.returncode != 0:
        print(f"  ⚠ insert_session error: {result.stderr.strip()}", file=sys.stderr)
        return None
    sid = result.stdout.strip().split("\n")[0].strip()
    return sid if sid else None


def insert_experience(db_url: str, session_id: str | None, goal: str, result: str, tags: list[str],
                      duration_secs: int | None, related_project: str) -> str | None:
    """Insert an experience record."""
    sid = f"'{session_id}'" if session_id else "NULL"
    dur = str(duration_secs) if duration_secs is not None else "NULL"
    tags_sql = _pg_array(tags)
    related_esc = _esc(related_project) if related_project else "NULL"
    goal_esc = _esc(goal)
    result_esc = _esc(result)
    
    sql = f"""
    INSERT INTO experiences (session_id, goal, result, tags, duration_seconds, related_project)
    VALUES ({sid}, {goal_esc}, {result_esc}, {tags_sql}, {dur}, {related_esc})
    RETURNING id;
    """
    result = subprocess.run(
        ["psql", db_url, "-t", "-A", "-c", sql],
        capture_output=True, text=True, timeout=10,
    )
    if result.returncode != 0:
        print(f"  ⚠ insert_experience error: {result.stderr.strip()[:200]}", file=sys.stderr)
        return None
    return result.stdout.strip()


def insert_memory(db_url: str, session_id: str | None, content: str, content_type: str,
                  importance: float, tags: list[str], metadata: dict) -> str | None:
    """Insert a memory record and return its UUID."""
    sid = f"'{session_id}'" if session_id else "NULL"
    tags_sql = _pg_array(tags)
    meta_json = json.dumps(metadata)
    content_esc = _esc(content)
    
    sql = f"""
    INSERT INTO memories (session_id, content, content_type, importance, tags, metadata)
    VALUES ({sid}, {content_esc}, '{content_type}', {importance}, {tags_sql}, '{meta_json}')
    RETURNING id;
    """
    result = subprocess.run(
        ["psql", db_url, "-t", "-A", "-c", sql],
        capture_output=True, text=True, timeout=10,
    )
    if result.returncode != 0:
        print(f"  ⚠ insert_memory error: {result.stderr.strip()[:200]}", file=sys.stderr)
        return None
    return result.stdout.strip()


def insert_document(db_url: str, path: str, section: str | None, title: str | None,
                    content: str, checksum: str | None, frontmatter_data: dict) -> str | None:
    """Insert a document, updating if path exists. Returns UUID."""
    
    # Check if path exists
    path_escaped = path.replace("'", "''")
    rows = run_psql(db_url, f"SELECT id FROM documents WHERE path = '{path_escaped}'")
    if rows:
        doc_id = rows[0]['value']
        content_esc = _esc(content)
        fm_json = json.dumps(frontmatter_data)
        checksum_val = f"'{checksum}'" if checksum else "NULL"
        sql = f"""
        UPDATE documents SET content = {content_esc}, frontmatter = '{fm_json}'::jsonb,
            checksum = COALESCE({checksum_val}, checksum), updated_at = now()
        WHERE id = '{doc_id}';
        """
        subprocess.run(["psql", db_url, "-c", sql], capture_output=True, timeout=30)
        return doc_id
    
    section_val = _esc(section) if section else "NULL"
    if title:
        title_val = _esc(title)
    else:
        title_val = _esc(path.split("/")[-1].replace(".md", "").replace(".jsonc", ""))
    content_esc = _esc(content)
    checksum_val = f"'{checksum}'" if checksum else "NULL"
    fm_json = json.dumps(frontmatter_data)

    sql = f"""
    INSERT INTO documents (path, vault_section, title, content, checksum, frontmatter)
    VALUES ('{path}', {section_val}, {title_val}, {content_esc}, {checksum_val}, '{fm_json}')
    RETURNING id;
    """

    if len(sql) > 100000:
        import tempfile
        with tempfile.NamedTemporaryFile(mode='w', suffix='.sql', delete=False, dir='/tmp') as f:
            f.write(sql)
            tmp_path = f.name
        result = subprocess.run(
            ["psql", db_url, "-t", "-A", "-f", tmp_path],
            capture_output=True, text=True, timeout=60,
        )
        os.unlink(tmp_path)
        if result.returncode != 0:
            print(f"  ⚠ insert_document error: {result.stderr.strip()[:200]}", file=sys.stderr)
            return None
        return result.stdout.strip()
    
    result = subprocess.run(
        ["psql", db_url, "-t", "-A", "-c", sql],
        capture_output=True, text=True, timeout=30,
    )
    if result.returncode != 0:
        print(f"  ⚠ insert_document error: {result.stderr.strip()[:200]}", file=sys.stderr)
        return None
    return result.stdout.strip()


def ensure_agent(db_url: str, name: str) -> str:
    """Ensure an agent exists. Returns its UUID."""
    rows = run_psql(db_url, f"SELECT id FROM agents WHERE name = '{name}'")
    if rows:
        return rows[0]['value']
    
    meta = json.dumps({"source": "ingest", "imported_at": datetime.now(timezone.utc).isoformat()})
    sql = f"""
    INSERT INTO agents (name, capabilities, metadata)
    VALUES ('{name}', '["ingested"]', '{meta}')
    RETURNING id;
    """
    result = subprocess.run(
        ["psql", db_url, "-t", "-A", "-c", sql],
        capture_output=True, text=True, timeout=10,
    )
    if result.returncode != 0:
        return None
    return result.stdout.strip()


def _esc(s: str) -> str:
    """Escape a string for safe SQL insertion."""
    return "'" + s.replace("'", "''") + "'"


def _pg_array(tags: list[str]) -> str:
    """Convert Python list to PostgreSQL array literal format."""
    if not tags:
        return "'{}'::text[]"
    escaped = []
    for t in tags:
        # Escape backslashes and quotes for PostgreSQL array elements
        e = t.replace("\\", "\\\\").replace('"', '\\"').replace("'", "''")
        # Quote elements that contain special chars
        if any(c in t for c in (',', '"', "'", '\\', ' ', '{', '}', '(', ')', '[', ']')):
            escaped.append(f'"{e}"')
        else:
            escaped.append(e)
    return "'{" + ",".join(escaped) + "}'::text[]"


# ============================================================
# SESSION INGESTION
# ============================================================

def ingest_sessions(db_url: str, source: str, report: dict):
    """Ingest OpenCode sessions from SQLite DB."""
    source_path = expand_path(source)
    print(f"\n📂 Ingesting sessions from: {source_path}")
    
    if not os.path.exists(source_path):
        print(f"  ❌ File not found: {source_path}")
        return
    
    # Run extraction script
    script_dir = Path(__file__).parent
    extract_script = script_dir / "extract_sessions.py"
    
    if not extract_script.exists():
        print(f"  ❌ Extraction script not found: {extract_script}")
        return
    
    start = time.time()
    proc = subprocess.Popen(
        ["python3", str(extract_script), source_path],
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
    )
    
    total = 0
    new = 0
    skipped = 0
    current_session = None
    current_parts = []
    current_todos = []
    
    def flush_session():
        nonlocal total, new, skipped, current_session, current_parts, current_todos
        if current_session is None:
            return
        total += 1
        process_one_session(db_url, current_session, current_parts, current_todos, report)
        # Count only non-skipped as new
        current_session = None
        current_parts = []
        current_todos = []
    
    for line in proc.stdout:
        line = line.strip()
        if not line:
            continue
        
        try:
            obj = json.loads(line)
        except json.JSONDecodeError:
            continue
        
        typ = obj.get("type")
        data = obj.get("data", {})
        
        if typ == "session":
            flush_session()
            current_session = data
        elif typ == "part":
            if current_session:
                current_parts.append(data)
        elif typ == "todo":
            if current_session:
                current_todos.append(data)
        elif typ == "metadata":
            report["sources"]["opencode-sessions"] = data.get("total_sessions", 0)
    
    flush_session()
    
    # Count what's actually new (not skipped)
    new = report["sessions_created"]
    elapsed = time.time() - start
    print(f"  ✓ {total} sessions ({new} new, {total - new - (report.get('session_errors', 0))} skipped, {report.get('session_errors', 0)} errors) in {elapsed:.1f}s")


def process_one_session(db_url: str, ses_data: dict, parts: list, todos: list, report: dict):
    """Process a single session."""
    ses_id = ses_data.get("id", "unknown")
    
    # Dedup check
    if check_ingested(db_url, ses_id):
        return
    
    title = ses_data.get("title", "Untitled")
    agent_name = ses_data.get("agent_name", "unknown")
    model_name = ses_data.get("model_name", "unknown")
    tokens_in = ses_data.get("tokens_input", 0) or 0
    tokens_out = ses_data.get("tokens_output", 0) or 0
    cost = ses_data.get("cost", 0.0) or 0.0
    
    created_iso = ses_data.get("time_created_iso")
    updated_iso = ses_data.get("time_updated_iso")
    created_dt = created_iso or datetime.now(timezone.utc).isoformat()
    updated_dt = updated_iso or datetime.now(timezone.utc).isoformat()
    
    # Duration
    try:
        duration = (datetime.fromisoformat(updated_dt) - datetime.fromisoformat(created_dt)).total_seconds()
    except (ValueError, TypeError):
        duration = 0
    
    # Extract user messages
    user_texts = []
    for p in parts:
        role = p.get("role", "")
        parsed = p.get("parsed_data", {})
        text = parsed.get("text", "")
        if role == "user" and len(text) > 50:
            user_texts.append(text)
    
    # Ensure agent
    ensure_agent(db_url, agent_name)
    
    # Insert session
    mem_session_id = insert_session(db_url, title, "completed", created_dt, updated_dt)
    
    meta = {
        "opencode_session_id": ses_id,
        "source": "opencode-session",
        "agent": agent_name,
        "model": model_name,
        "tokens_input": tokens_in,
        "tokens_output": tokens_out,
        "cost": cost,
        "imported_at": datetime.now(timezone.utc).isoformat(),
    }
    
    tags = [agent_name, model_name, "opencode-session"]
    
    # Insert experience
    result_summary = (
        f"Session: {title} | Agent: {agent_name} | Model: {model_name} | "
        f"Tokens: {tokens_in} in / {tokens_out} out | Cost: ${cost:.4f} | "
        f"Messages: {len(parts)} | User prompts: {len(user_texts)}"
    )
    insert_experience(db_url, mem_session_id, title, result_summary, tags, int(duration), "opencode")
    
    # Insert session summary memory
    summary = (
        f"OpenCode session: {title}\nAgent: {agent_name}\nModel: {model_name}\n"
        f"Messages: {len(parts)}\nUser prompts: {len(user_texts)}\n"
        f"Tokens: {tokens_in} in / {tokens_out} out\nCost: ${cost:.4f}"
    )
    insert_memory(db_url, mem_session_id, summary, "session", 0.6, 
                  [agent_name, model_name, "session-summary"], meta)
    
    # Insert user messages as memories
    for text in user_texts:
        insert_memory(db_url, mem_session_id, text, "conversation", 0.4,
                      [agent_name, model_name, "user-prompt"], meta)
    
    report["sessions_created"] += 1
    report["memories_created"] += 1 + len(user_texts)  # summary + user messages
    report["experiences_created"] += 1


# ============================================================
# VAULT INGESTION
# ============================================================

def ingest_vault(db_url: str, vault_path: str, limit: int, report: dict):
    """Ingest Obsidian vault .md files."""
    vault = expand_path(vault_path)
    print(f"\n📁 Ingesting vault from: {vault}")
    
    if not os.path.isdir(vault):
        print(f"  ❌ Directory not found: {vault}")
        return
    
    from hashlib import sha256
    import time
    
    # Collect all .md files
    files = []
    for root, dirs, fnames in os.walk(vault):
        for f in fnames:
            if f.endswith(".md"):
                files.append(os.path.join(root, f))
    
    # Sort by mtime (newest first)
    files.sort(key=lambda p: os.path.getmtime(p) if os.path.exists(p) else 0, reverse=True)
    
    total = len(files)
    if limit > 0 and limit < total:
        files = files[:limit]
    
    print(f"  Found {total} .md files, processing {len(files)}")
    
    created = 0
    skipped = 0
    errors = 0
    start = time.time()
    
    for i, path in enumerate(files):
        try:
            with open(path, "r", encoding="utf-8", errors="replace") as f:
                content = f.read()
        except Exception as e:
            print(f"  ⚠ Error reading {os.path.basename(path)}: {e}")
            errors += 1
            continue
        
        if not content.strip():
            skipped += 1
            continue
        
        checksum = sha256(content.encode()).hexdigest()
        
        # Relative path within vault
        rel_path = os.path.relpath(path, vault)
        vault_key = f"vault://{rel_path}"
        
        # Extract title
        title = "Untitled"
        for line in content.split("\n"):
            line = line.strip()
            if line.startswith("# "):
                title = line[2:].strip()
                break
        if title == "Untitled":
            title = os.path.splitext(os.path.basename(path))[0]
        
        # Extract section
        parts_list = rel_path.split(os.sep)
        section = parts_list[0] if len(parts_list) > 1 else None
        
        meta = {
            "source": "obsidian-vault",
            "file_path": path,
            "imported_at": datetime.now(timezone.utc).isoformat(),
        }
        
        # Check if already exists
        existing = run_psql(db_url, f"SELECT id FROM documents WHERE path = '{vault_key}'")
        if existing:
            skipped += 1
            continue
        
        insert_document(db_url, vault_key, section, title, content, checksum, meta)
        created += 1
        
        if (i + 1) % 100 == 0 or i == len(files) - 1:
            elapsed = time.time() - start
            print(f"  Progress: {i+1}/{len(files)} ({created} new, {skipped} skipped, {errors} errors) {elapsed:.0f}s")
    
    report["sources"]["obsidian-vault"] = total
    report["documents_created"] += created


# ============================================================
# CONFIG INGESTION
# ============================================================

def ingest_config(db_url: str, config_dir: str, report: dict):
    """Ingest OpenCode configuration, rules, and skills."""
    cfg = expand_path(config_dir)
    print(f"\n⚙️  Ingesting config from: {cfg}")
    
    if not os.path.isdir(cfg):
        print(f"  ❌ Directory not found: {cfg}")
        return
    
    from hashlib import sha256
    
    # Files to ingest
    config_files = [
        "opencode.jsonc",
        "oh-my-openagent.jsonc",
    ]
    
    for cf in config_files:
        cf_path = os.path.join(cfg, cf)
        if not os.path.exists(cf_path):
            continue
        
        try:
            with open(cf_path, "r") as f:
                content = f.read()
        except Exception as e:
            print(f"  ⚠ Error reading {cf}: {e}")
            continue
        
        checksum = sha256(content.encode()).hexdigest()
        meta = {
            "source": "opencode-config",
            "path": cf_path,
            "file_type": "jsonc",
            "imported_at": datetime.now(timezone.utc).isoformat(),
        }
        
        insert_document(db_url, f"config://{cf}", "config", cf, content, checksum, meta)
        report["documents_created"] += 1
        
        # Store as memory
        summary = f"OpenCode configuration file: {cf} ({len(content)} bytes)"
        insert_memory(db_url, None, summary, "config", 0.7,
                      ["opencode-config", "configuration"], meta)
        report["memories_created"] += 1
        print(f"  ✓ {cf} ({len(content)} bytes)")
    
    # Rules
    rules_dir = os.path.join(cfg, "rules")
    if os.path.isdir(rules_dir):
        print(f"\n  📋 Ingesting rules from {rules_dir}")
        for root, dirs, fnames in os.walk(rules_dir):
            for f in fnames:
                fpath = os.path.join(root, f)
                try:
                    with open(fpath, "r") as fh:
                        content = fh.read()
                except Exception as e:
                    print(f"    ⚠ Error reading {f}: {e}")
                    continue
                
                rel = os.path.relpath(fpath, rules_dir)
                key = f"rules://{rel}"
                checksum = sha256(content.encode()).hexdigest()
                parent_dir = os.path.dirname(rel).split(os.sep)[0] if os.path.dirname(rel) else "common"
                
                meta = {
                    "source": "opencode-rule",
                    "path": fpath,
                    "file_type": "md",
                    "category": parent_dir,
                    "imported_at": datetime.now(timezone.utc).isoformat(),
                }
                
                insert_document(db_url, key, "rules", f, content, checksum, meta)
                report["documents_created"] += 1
                print(f"    ✓ {rel}")
    
    # Skills
    skills_dir = os.path.join(cfg, "skills")
    if os.path.isdir(skills_dir):
        print(f"\n  🛠️  Ingesting skills from {skills_dir}")
        for root, dirs, fnames in os.walk(skills_dir):
            for f in fnames:
                fpath = os.path.join(root, f)
                try:
                    with open(fpath, "r") as fh:
                        content = fh.read()
                except Exception as e:
                    print(f"    ⚠ Error reading {f}: {e}")
                    continue
                
                key = f"skill://{os.path.relpath(fpath, '/')}"
                checksum = sha256(content.encode()).hexdigest()
                
                meta = {
                    "source": "opencode-skill",
                    "path": fpath,
                    "file_type": "md",
                    "imported_at": datetime.now(timezone.utc).isoformat(),
                }
                
                insert_document(db_url, key, "skills", f, content, checksum, meta)
                report["documents_created"] += 1
                print(f"    ✓ {os.path.relpath(fpath, skills_dir)}")


# ============================================================
# LOG INGESTION
# ============================================================

def ingest_logs(db_url: str, log_path: str, report: dict):
    """Ingest OpenCode runtime log."""
    log = expand_path(log_path)
    print(f"\n📄 Ingesting logs from: {log}")
    
    if not os.path.exists(log):
        print(f"  ❌ File not found: {log}")
        return
    
    from hashlib import sha256
    import re
    
    with open(log, "r", errors="replace") as f:
        content = f.read()
    
    lines = content.split("\n")
    
    # Extract patterns
    session_ids = set(re.findall(r"session\.id=(ses_\w+)", content))
    model_ids = set(re.findall(r"modelID=([\w.-]+)", content))
    agents = set(re.findall(r'agent="([^"]+)"', content))
    errors = content.count("level=ERROR")
    
    print(f"  {len(lines)} lines, {len(session_ids)} sessions, {len(model_ids)} models, {len(agents)} agents, {errors} errors")
    
    checksum = sha256(content.encode()).hexdigest()
    meta = {
        "source": "opencode-log",
        "path": log,
        "file_type": "log",
        "line_count": len(lines),
        "session_count": len(session_ids),
        "error_count": errors,
        "imported_at": datetime.now(timezone.utc).isoformat(),
    }
    
    insert_document(db_url, "log://opencode.log", "logs", "OpenCode Runtime Log", content, checksum, meta)
    report["documents_created"] += 1
    
    # Experience
    analysis = (
        f"OpenCode Log Analysis\n"
        f"Total log lines: {len(lines)}\n"
        f"Unique sessions: {len(session_ids)}\n"
        f"Models used: {len(model_ids)}\n"
        f"Agents used: {len(agents)}\n"
        f"Errors found: {errors}\n\n"
        f"Models: {', '.join(sorted(model_ids))}\n"
        f"Agents: {', '.join(sorted(agents))}"
    )
    result = f"{len(lines)} lines analyzed, {len(session_ids)} sessions, {len(model_ids)} models, {errors} errors"
    insert_experience(db_url, None, "OpenCode Log Analysis", result,
                      ["opencode-log", "analysis"], None, "opencode")
    report["experiences_created"] += 1
    
    print(f"  ✓ {len(lines)} lines ingested")


# ============================================================
# MAIN
# ============================================================

def print_report(report: dict, start: float):
    elapsed = time.time() - start
    print("\n" + "═" * 50)
    print("  INGESTION REPORT")
    print("═" * 50)
    for source, count in sorted(report["sources"].items()):
        if count > 0:
            print(f"  • {source}: {count}")
    print("─" * 30)
    print(f"  Memories:     {report['memories_created']}")
    print(f"  Experiences:  {report['experiences_created']}")
    print(f"  Documents:    {report['documents_created']}")
    print(f"  Sessions:     {report['sessions_created']}")
    if report.get("errors", 0) > 0:
        print(f"  ❌ Errors:      {report['errors']}")
    print(f"\n  ⏱️  {elapsed:.1f}s")
    print("═" * 50 + "\n")


def main():
    parser = argparse.ArgumentParser(description="Memory Platform Ingestion Tool")
    parser.add_argument("--db-url", required=True, help="PostgreSQL connection string")
    
    subparsers = parser.add_subparsers(dest="command", required=True)
    
    # sessions
    sp = subparsers.add_parser("sessions", help="Ingest OpenCode sessions")
    sp.add_argument("--source", default="~/.local/share/opencode/opencode.db",
                    help="Path to opencode.db SQLite file")
    
    # vault
    sp = subparsers.add_parser("vault", help="Ingest Obsidian vault")
    sp.add_argument("--path", default="~/obsidian-vault", help="Path to vault directory")
    sp.add_argument("--limit", type=int, default=0, help="Max files to process")
    
    # config
    sp = subparsers.add_parser("config", help="Ingest OpenCode config/rules/skills")
    sp.add_argument("--dir", default="~/.config/opencode", help="Path to opencode config directory")
    
    # logs
    sp = subparsers.add_parser("logs", help="Ingest OpenCode log")
    sp.add_argument("--log", default="~/.local/share/opencode/log/opencode.log",
                    help="Path to opencode.log")
    
    # all
    sp = subparsers.add_parser("all", help="Ingest everything")
    sp.add_argument("--sessions-db", default="~/.local/share/opencode/opencode.db")
    sp.add_argument("--vault-path", default="~/obsidian-vault")
    sp.add_argument("--config-dir", default="~/.config/opencode")
    sp.add_argument("--log-path", default="~/.local/share/opencode/log/opencode.log")
    
    args = parser.parse_args()
    db_url = args.db_url
    
    # Verify psql is available
    if subprocess.run(["which", "psql"], capture_output=True).returncode != 0:
        print("❌ psql not found. Install PostgreSQL client.", file=sys.stderr)
        sys.exit(1)
    
    report = {
        "sources": {},
        "memories_created": 0,
        "experiences_created": 0,
        "documents_created": 0,
        "sessions_created": 0,
        "errors": 0,
    }
    
    start = time.time()
    
    if args.command == "sessions":
        ingest_sessions(db_url, args.source, report)
    elif args.command == "vault":
        ingest_vault(db_url, args.path, args.limit, report)
    elif args.command == "config":
        ingest_config(db_url, args.dir, report)
    elif args.command == "logs":
        ingest_logs(db_url, args.log, report)
    elif args.command == "all":
        ingest_sessions(db_url, args.sessions_db, report)
        ingest_vault(db_url, args.vault_path, 0, report)
        ingest_config(db_url, args.config_dir, report)
        ingest_logs(db_url, args.log_path, report)
    
    print_report(report, start)


if __name__ == "__main__":
    main()

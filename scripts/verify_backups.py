#!/usr/bin/env python3
"""Verify backup parity and retention for the memory/vault backup targets."""

from __future__ import annotations

import argparse
import os
import subprocess
import sys
from collections.abc import Iterable
from dataclasses import dataclass
from datetime import datetime, timedelta, timezone
from pathlib import Path


def is_remote(path: str) -> bool:
    return ":" in path and not Path(path).exists()


def run(cmd: list[str], timeout: int = 120) -> str:
    try:
        proc = subprocess.run(
            cmd, capture_output=True, text=True, check=False, timeout=timeout
        )
    except subprocess.TimeoutExpired as exc:  # pragma: no cover - operational guard
        raise RuntimeError(f"Command timed out after {timeout}s: {' '.join(cmd)}") from exc
    if proc.returncode != 0:
        stderr = proc.stderr.strip() or proc.stdout.strip()
        raise RuntimeError(f"Command failed: {' '.join(cmd)}\n{stderr}")
    return proc.stdout


def list_files(path: str) -> set[str]:
    if is_remote(path):
        output = run(["rclone", "lsf", "--recursive", "--files-only", path])
        return {line.strip() for line in output.splitlines() if line.strip()}

    root = Path(path).expanduser()
    if not root.exists():
        raise FileNotFoundError(root)

    files: set[str] = set()
    for current_root, _, filenames in os.walk(root):
        for filename in filenames:
            full = Path(current_root) / filename
            files.add(str(full.relative_to(root)).replace(os.sep, "/"))
    return files


def list_dirs(path: str) -> set[str]:
    if is_remote(path):
        output = run(["rclone", "lsf", "--recursive", "--dirs-only", path])
        return {line.strip().rstrip("/") for line in output.splitlines() if line.strip()}

    root = Path(path).expanduser()
    if not root.exists():
        raise FileNotFoundError(root)

    dirs: set[str] = set()
    for current_root, dirnames, _ in os.walk(root):
        for dirname in dirnames:
            full = Path(current_root) / dirname
            dirs.add(str(full.relative_to(root)).replace(os.sep, "/"))
    return dirs


def check_parity(source: str, mirror: str) -> tuple[list[str], list[str]]:
    source_files = list_files(source)
    mirror_files = list_files(mirror)
    missing = sorted(source_files - mirror_files)
    extra = sorted(mirror_files - source_files)
    return missing, extra


def check_nonempty(path: str) -> bool:
    try:
        if is_remote(path):
            output = run(["rclone", "lsf", "--max-depth", "1", path], timeout=120)
            return any(line.strip() for line in output.splitlines())
        return bool(list_files(path) or list_dirs(path))
    except FileNotFoundError:
        return False


def parse_yyyymmdd(name: str) -> datetime | None:
    try:
        return datetime.strptime(name, "%Y%m%d").replace(tzinfo=timezone.utc)
    except ValueError:
        return None


def archive_retention_violations(path: str, retention_days: int) -> list[str]:
    cutoff = datetime.now(timezone.utc) - timedelta(days=retention_days)
    violations: list[str] = []
    for entry in list_dirs(path):
        date = parse_yyyymmdd(Path(entry).name)
        if date and date < cutoff:
            violations.append(entry)
    return sorted(violations)


@dataclass(frozen=True)
class CoverageTarget:
    label: str
    path: str


def parse_targets(values: Iterable[str]) -> list[CoverageTarget]:
    targets: list[CoverageTarget] = []
    for value in values:
        if "=" in value:
            label, path = value.split("=", 1)
        else:
            label = path = value
        targets.append(CoverageTarget(label=label, path=path))
    return targets


def main() -> int:
    parser = argparse.ArgumentParser(description="Verify backup parity and retention.")
    parser.add_argument(
        "--vault-source",
        default=str(Path.home() / "obsidian-vault"),
        help="Source vault directory or remote path",
    )
    parser.add_argument(
        "--vault-mirror",
        default="gdrive:obsidian-vault",
        help="Vault mirror directory or remote path",
    )
    parser.add_argument(
        "--archive-root",
        default="gdrive:obsidian-vault-archive",
        help="Vault archive root used for retention checks",
    )
    parser.add_argument(
        "--retention-days",
        type=int,
        default=15,
        help="Maximum archive age to keep before it should be purged",
    )
    parser.add_argument(
        "--coverage-target",
        action="append",
        default=None,
        help="Expected backup target (label=path). Can be repeated.",
    )
    args = parser.parse_args()

    failures: list[str] = []

    try:
        missing, extra = check_parity(args.vault_source, args.vault_mirror)
        print(f"[verify-backups] vault source: {args.vault_source}")
        print(f"[verify-backups] vault mirror: {args.vault_mirror}")
        print(f"[verify-backups] vault parity: {len(missing)} missing, {len(extra)} extra")
        if missing:
            print("[verify-backups] missing files:")
            for item in missing[:20]:
                print(f"  - {item}")
            failures.append("vault mirror is missing source files")
        if extra:
            print("[verify-backups] extra files:")
            for item in extra[:20]:
                print(f"  - {item}")
            failures.append("vault mirror has unexpected files")
    except Exception as exc:  # pragma: no cover - operational guard
        failures.append(f"vault parity check failed: {exc}")

    try:
        violations = archive_retention_violations(args.archive_root, args.retention_days)
        print(
            f"[verify-backups] archive retention: {len(violations)} folders older than "
            f"{args.retention_days} days"
        )
        for item in violations[:20]:
            print(f"  - {item}")
        if violations:
            failures.append("vault archive retention violation")
    except Exception as exc:  # pragma: no cover - operational guard
        failures.append(f"archive retention check failed: {exc}")

    targets = parse_targets(
        args.coverage_target
        or [
            "vault=gdrive:obsidian-vault",
            "mt5=gdrive:Docker_Nodes_Backup/mt5_node",
            "btc=gdrive:Docker_Nodes_Backup/btc_node",
            "numerai_models=gdrive:backups/numerai/models",
            "supabase=gdrive-crypt:supabase",
            "neo4j=gdrive-crypt:neo4j",
        ]
    )
    for target in targets:
        try:
            ok = check_nonempty(target.path)
        except Exception as exc:  # pragma: no cover - operational guard
            ok = False
            failures.append(f"coverage check failed for {target.label}: {exc}")
        status = "ok" if ok else "missing"
        print(f"[verify-backups] coverage {target.label}: {status} ({target.path})")
        if not ok:
            failures.append(f"missing backup coverage for {target.label}")

    if failures:
        print("[verify-backups] failures:")
        for failure in failures:
            print(f"  - {failure}")
        return 1

    print("[verify-backups] all checks passed")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

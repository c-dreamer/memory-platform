#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/.." && pwd)"

command -v python3 >/dev/null || { echo "Python 3 is required" >&2; exit 78; }
python3 - <<'PY'
try:
    import gi
    gi.require_version("Gtk", "3.0")
    gi.require_version("WebKit2", "4.0")
    from gi.repository import Gtk, WebKit2  # noqa: F401
except (ImportError, ValueError) as exc:
    raise SystemExit("Install python3-gi, gir1.2-gtk-3.0, and gir1.2-webkit2-4.0") from exc
PY

python3 - "$ROOT/linux-app/dashboard.py" "$ROOT/linux-app/dashboard_core.py" <<'PY'
import ast
import pathlib
import sys

for source in sys.argv[1:]:
    ast.parse(pathlib.Path(source).read_text(encoding="utf-8"), filename=source)
PY
echo "Linux dashboard app sources are ready at: $ROOT/linux-app"
echo "No files were installed or services changed."

"""Token and URL helpers for the Linux dashboard window."""

from pathlib import Path
from urllib.parse import urlencode


DEFAULT_TOKEN_FILE = Path.home() / ".config/memory-platform/dashboard.token"
BASE_URL = "http://127.0.0.1:8765/operations"


def read_token(path: Path = DEFAULT_TOKEN_FILE) -> str:
    """Read the existing dashboard token, without logging or modifying it."""
    token = path.read_text(encoding="utf-8").strip()
    if len(token) < 32:
        raise ValueError("Dashboard token is missing or too short")
    return token


def dashboard_url(token: str) -> str:
    """Construct the same authenticated local URL used by the macOS app."""
    if len(token) < 32:
        raise ValueError("Dashboard token is missing or too short")
    return f"{BASE_URL}?{urlencode({'token': token})}"

#!/usr/bin/env python3
"""GTK/WebKit window for the systemd-owned local dashboard service."""

import sys

import gi

gi.require_version("Gtk", "3.0")
gi.require_version("WebKit2", "4.0")
from gi.repository import Gtk, WebKit2  # noqa: E402

from dashboard_core import DEFAULT_TOKEN_FILE, dashboard_url, read_token


class DashboardWindow(Gtk.Window):
    def __init__(self):
        super().__init__(title="Memory Platform")
        self.set_default_size(960, 680)
        self.connect("destroy", Gtk.main_quit)
        self.web = WebKit2.WebView()
        self.add(self.web)
        self.web.connect("load-failed", self.on_load_failed)
        self.web.connect("web-process-terminated", self.on_process_terminated)

        try:
            url = dashboard_url(read_token(DEFAULT_TOKEN_FILE))
        except (OSError, UnicodeError, ValueError):
            self.show_message(
                "Dashboard unavailable",
                f"The dashboard token could not be read from {DEFAULT_TOKEN_FILE}.\n"
                "Check the token file and that the dashboard service is running.",
            )
            return

        self.web.load_uri(url)

    def on_load_failed(self, _view, _event, _uri, _error):
        self.show_message(
            "Dashboard unavailable",
            "The local dashboard service is not responding at 127.0.0.1:8765.\n"
            "Check its systemd user service, then reopen this window.",
        )
        return True

    def on_process_terminated(self, _view, _reason):
        self.show_message("Dashboard unavailable", "The embedded web view stopped unexpectedly.")

    def show_message(self, title, detail):
        label = Gtk.Label(label=f"{title}\n\n{detail}")
        label.set_justify(Gtk.Justification.CENTER)
        label.set_line_wrap(True)
        label.set_margin_start(32)
        label.set_margin_end(32)
        label.set_margin_top(32)
        label.set_margin_bottom(32)
        self.remove(self.get_child())
        self.add(label)
        self.show_all()


def main():
    try:
        DashboardWindow().show_all()
        Gtk.main()
    except Exception as exc:
        # Do not print exception details that might include the tokenized URL.
        print(f"Memory Platform dashboard could not start ({type(exc).__name__}).", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main())

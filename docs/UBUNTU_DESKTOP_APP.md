# Ubuntu dashboard window

The GTK/WebKit window embeds the existing authenticated dashboard at
`http://127.0.0.1:8765/operations`. It reads
`~/.config/memory-platform/dashboard.token` and sends that token only to the
embedded local page. It does not launch an external browser or manage services.
The dashboard remains owned by the existing systemd user service. If the token
is unreadable or the service is unavailable, the window shows an explanation.

## Build and install

On Ubuntu, install the distribution GTK 3 and WebKitGTK introspection packages:

```sh
sudo apt install python3-gi gir1.2-gtk-3.0 gir1.2-webkit2-4.0
```

Check the app source and dependencies:

```sh
scripts/build-linux-dashboard-app.sh
```

Install the app files and desktop entry for the current user:

```sh
install -d "$HOME/.local/lib/memory-platform-dashboard" "$HOME/.local/share/applications"
install -m 644 linux-app/dashboard.py linux-app/dashboard_core.py "$HOME/.local/lib/memory-platform-dashboard/"
cat > "$HOME/.local/share/applications/memory-platform-dashboard.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Memory Platform
Comment=Open the local Memory Platform dashboard
Exec=python3 $HOME/.local/lib/memory-platform-dashboard/dashboard.py
Terminal=false
Categories=Utility;
EOF
chmod 644 "$HOME/.local/share/applications/memory-platform-dashboard.desktop"
```

The dashboard service must already be installed and running through systemd.
The window does not start, stop, enable, or disable it. Ubuntu build and GUI
runtime behavior must be verified on an Ubuntu desktop; a macOS check does not
establish Linux GUI support.

## Focused source tests

The token helper tests use only temporary files and Python's standard library:

```sh
python3 -m unittest discover -s linux-app -p 'test_*.py'
```

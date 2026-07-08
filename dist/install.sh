#!/usr/bin/env bash
# wattea install script.
#   - release build
#   - installs binaries to ~/.cargo/bin
#   - installs the systemd user unit + enable-linger + starts it
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UNIT="$HOME/.config/systemd/user/wattea-daemon.service"

echo "==> 1/4  release build"
(cd "$ROOT" && cargo build --release)

echo "==> 2/4  installing binaries (~/.cargo/bin)"
install -Dm755 "$ROOT/target/release/wattea" "$HOME/.cargo/bin/wattea"
install -Dm755 "$ROOT/target/release/wattea-daemon" "$HOME/.cargo/bin/wattea-daemon"

echo "==> 3/4  installing systemd user unit"
install -Dm644 "$ROOT/dist/wattea-daemon.service" "$UNIT"
systemctl --user daemon-reload

# Keep running even when the session is closed.
if ! loginctl show-user "$USER" 2>/dev/null | grep -q '^Linger=yes'; then
	echo "    (loginctl enable-linger — may require root)"
	sudo loginctl enable-linger "$USER" ||
		echo "    WARNING: could not enable linger. The service runs only while the session is open."
fi

echo "==> 4/4  starting service"
systemctl --user enable --now wattea-daemon.service

echo
echo "✅ Wattea installed."
echo "   TUI:      wattea"
echo "   Service:  systemctl --user status wattea-daemon"
echo "   Logs:     journalctl --user -u wattea-daemon -f"
echo "   Data:     ~/.local/share/wattea/db.sqlite"

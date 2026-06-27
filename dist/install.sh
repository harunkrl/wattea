#!/usr/bin/env bash
# wattea kurulum scripti.
#   - release build
#   - binary'leri ~/.cargo/bin'e kurar (cargo install)
#   - systemd user unit'ini kurar + enable-linger + başlatır
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
UNIT="$HOME/.config/systemd/user/wattea-daemon.service"

echo "==> 1/4  release build"
(cd "$ROOT" && cargo build --release)

echo "==> 2/4  binary kurulumu (~/.cargo/bin)"
install -Dm755 "$ROOT/target/release/wattea" "$HOME/.cargo/bin/wattea"
install -Dm755 "$ROOT/target/release/wattea-daemon" "$HOME/.cargo/bin/wattea-daemon"

echo "==> 3/4  systemd user unit kurulumu"
install -Dm644 "$ROOT/dist/wattea-daemon.service" "$UNIT"
systemctl --user daemon-reload

# Oturum kapalıyken de çalışsın.
if ! loginctl show-user "$USER" 2>/dev/null | grep -q '^Linger=yes'; then
	echo "    (loginctl enable-linger — root gerekebilir)"
	sudo loginctl enable-linger "$USER" ||
		echo "    UYARI: linger etkinleştirilemedi. Servis yalnızca oturum açıkken çalışır."
fi

echo "==> 4/4  servis başlatılıyor"
systemctl --user enable --now wattea-daemon.service

echo
echo "✅ Wattea kuruldu."
echo "   TUI:      wattea"
echo "   Servis:   systemctl --user status wattea-daemon"
echo "   Loglar:   journalctl --user -u wattea-daemon -f"
echo "   Veri:     ~/.local/share/wattea/db.sqlite"

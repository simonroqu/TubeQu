#!/bin/sh
# Baut Tube und installiert es für den aktuellen Nutzer (ohne root) samt Startmenü-Eintrag.
# Entfernen: die drei unten genannten Dateien löschen.
set -e
cd "$(dirname "$0")"

cargo build --release

bin="$HOME/.local/bin/tube"
icon="$HOME/.local/share/icons/hicolor/scalable/apps/dev.tube.Tube.svg"
desktop="$HOME/.local/share/applications/dev.tube.Tube.desktop"

install -Dm755 target/release/tube "$bin"
install -Dm644 data/dev.tube.Tube.svg "$icon"
mkdir -p "$(dirname "$desktop")"
sed "s|@BIN@|$bin|" data/dev.tube.Tube.desktop > "$desktop"

# Startmenü neu einlesen (falls das Werkzeug da ist).
command -v update-desktop-database >/dev/null && update-desktop-database -q "$(dirname "$desktop")" || true

echo "Installiert: $bin"
echo "Startmenü:   $desktop"

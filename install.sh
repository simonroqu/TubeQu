#!/bin/sh
# Baut Tube und installiert es für den aktuellen Nutzer (ohne root) samt Startmenü-Eintrag.
# Entfernen: die unten genannten Dateien löschen; für die Link-Weiche in
# ~/.config/hyprland-mimeapps.list die https-Zeile wieder auf omarchy-browser.desktop setzen.
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

# Links aus anderen Apps (Discord …): nur unter Omarchy, das Links über omarchy-browser.desktop leitet.
# Die Weiche bekommt nur https (Omarchy erkennt den Browser am http-Eintrag, der bleibt).
links="$HOME/.local/bin/tube-links"
links_desktop="$HOME/.local/share/applications/dev.tube.Links.desktop"
list="$HOME/.config/hyprland-mimeapps.list"
if command -v omarchy-launch-browser >/dev/null && grep -qx 'x-scheme-handler/https=omarchy-browser.desktop' "$list" 2>/dev/null; then
  cp "$list" "$list.bak-tube"
  sed -i 's|^x-scheme-handler/https=omarchy-browser.desktop$|x-scheme-handler/https=dev.tube.Links.desktop;omarchy-browser.desktop;|' "$list"
  echo "Links:       YouTube-Links aus anderen Apps öffnen jetzt Tube (Sicherung: $list.bak-tube)"
fi
if grep -q 'dev.tube.Links.desktop' "$list" 2>/dev/null; then
  sed "s|@BIN@|$bin|" data/tube-links > "$links"
  chmod 755 "$links"
  sed "s|@LINKS@|$links|" data/dev.tube.Links.desktop > "$links_desktop"
fi

# Startmenü neu einlesen (falls das Werkzeug da ist).
command -v update-desktop-database >/dev/null && update-desktop-database -q "$(dirname "$desktop")" || true

echo "Installiert: $bin"
echo "Startmenü:   $desktop"

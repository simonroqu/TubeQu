#!/bin/sh
# Baut Tubequ und installiert es für den aktuellen Nutzer (ohne root) samt Startmenü-Eintrag.
# Entfernen: die unten genannten Dateien löschen; für die Link-Weiche in
# ~/.config/hyprland-mimeapps.list die https-Zeile wieder auf omarchy-browser.desktop setzen.
set -e
cd "$(dirname "$0")"

cargo build --release

bin="$HOME/.local/bin/tubequ"
icon="$HOME/.local/share/icons/hicolor/scalable/apps/dev.tubequ.Tubequ.svg"
desktop="$HOME/.local/share/applications/dev.tubequ.Tubequ.desktop"

install -Dm755 target/release/tubequ "$bin"
install -Dm644 data/dev.tubequ.Tubequ.svg "$icon"
mkdir -p "$(dirname "$desktop")"
sed "s|@BIN@|$bin|" data/dev.tubequ.Tubequ.desktop > "$desktop"

# Links aus anderen Apps (Discord …): nur unter Omarchy, das Links über omarchy-browser.desktop leitet.
# Die Weiche bekommt nur https (Omarchy erkennt den Browser am http-Eintrag, der bleibt).
links="$HOME/.local/bin/tubequ-links"
links_desktop="$HOME/.local/share/applications/dev.tubequ.Links.desktop"
list="$HOME/.config/hyprland-mimeapps.list"
if command -v omarchy-launch-browser >/dev/null && grep -qx 'x-scheme-handler/https=omarchy-browser.desktop' "$list" 2>/dev/null; then
  cp "$list" "$list.bak-tubequ"
  sed -i 's|^x-scheme-handler/https=omarchy-browser.desktop$|x-scheme-handler/https=dev.tubequ.Links.desktop;omarchy-browser.desktop;|' "$list"
  echo "Links:       YouTube-Links aus anderen Apps öffnen jetzt Tubequ (Sicherung: $list.bak-tubequ)"
fi
if grep -q 'dev.tubequ.Links.desktop' "$list" 2>/dev/null; then
  sed "s|@BIN@|$bin|" data/tubequ-links > "$links"
  chmod 755 "$links"
  sed "s|@LINKS@|$links|" data/dev.tubequ.Links.desktop > "$links_desktop"
fi

# Startmenü neu einlesen (falls das Werkzeug da ist).
command -v update-desktop-database >/dev/null && update-desktop-database -q "$(dirname "$desktop")" || true

echo "Installiert: $bin"
echo "Startmenü:   $desktop"

#!/bin/sh
# Baut Tubequ und installiert es für den aktuellen Nutzer (ohne root) samt Startmenü-Eintrag.
# Entfernen: die Dateien löschen, die `--install` ausgibt, und die Link-Weiche; für sie in
# ~/.config/hyprland-mimeapps.list die https-Zeile wieder auf omarchy-browser.desktop setzen.
set -e
cd "$(dirname "$0")"

cargo build --release
# Programm, Icon und Startmenü-Eintrag (Texte aus qu.toml) legt die App selbst an.
./target/release/tubequ --install
bin="$HOME/.local/bin/tubequ"

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

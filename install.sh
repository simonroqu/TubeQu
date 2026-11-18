#!/bin/sh
# Installiert die App für den aktuellen Nutzer: fehlende Pakete (GTK ≥ 4.20, libmpv), Rust, bauen,
# Programm + Startmenü-Eintrag. Linux (Arch, Debian/Ubuntu, Fedora, openSUSE) und macOS (Homebrew).
# Windows: install.ps1. Entfernen: die Dateien löschen, die `--install` ausgibt, und die Link-Weiche; für sie in
# ~/.config/hyprland-mimeapps.list die https-Zeile wieder auf omarchy-browser.desktop setzen.
set -e
cd "$(dirname "$0")"
name=$(sed -n 's/^name = "\(.*\)"/\1/p' qu.toml | head -n1)

# Pakete nur, wenn etwas fehlt (dann ohne sudo-Abfrage, wenn alles da ist).
if ! pkg-config --atleast-version=4.20 gtk4 2>/dev/null || ! pkg-config --exists mpv 2>/dev/null; then
  if [ "$(uname)" = Darwin ]; then
    command -v brew >/dev/null || { echo "Homebrew fehlt: https://brew.sh"; exit 1; }
    brew install gtk4 mpv pkgconf
  elif command -v pacman >/dev/null; then sudo pacman -S --needed --noconfirm base-devel gtk4 mpv
  elif command -v apt-get >/dev/null; then sudo apt-get install -y build-essential curl pkg-config libgtk-4-dev libmpv-dev
  elif command -v dnf >/dev/null; then sudo dnf install -y gcc curl pkgconf-pkg-config gtk4-devel mpv-devel
  elif command -v zypper >/dev/null; then sudo zypper install -y gcc curl pkgconf gtk4-devel mpv-devel
  else echo "Bitte installieren: GTK ≥ 4.20 (Entwicklerpaket), libmpv (Entwicklerpaket), pkg-config"; exit 1
  fi
  pkg-config --atleast-version=4.20 gtk4 ||
    { echo "GTK $(pkg-config --modversion gtk4) ist zu alt, gebraucht wird ≥ 4.20 (neuere Distribution nötig)."; exit 1; }
fi

if [ -f extra-deps.sh ]; then . ./extra-deps.sh; fi

# Rust über rustup, falls cargo fehlt.
if ! command -v cargo >/dev/null; then
  [ -x "$HOME/.cargo/bin/cargo" ] || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
  PATH="$HOME/.cargo/bin:$PATH"
fi

cargo update -p qu-core   # neuester quCore-Stand (Cargo.lock hält sonst den alten fest)
cargo build --release
# Programm, Icon und Startmenü-Eintrag (Texte aus qu.toml) legt die App selbst an.
"./target/release/$name" --install

bin="$HOME/.local/bin/tubequ"

# Links aus anderen Apps (Discord …): nur unter Omarchy, das Links über omarchy-browser.desktop leitet.
# Die Weiche bekommt nur https (Omarchy erkennt den Browser am http-Eintrag, der bleibt).
links="$HOME/.local/bin/tubequ-links"
links_desktop="$HOME/.local/share/applications/dev.tubequ.Links.desktop"
list="$HOME/.config/hyprland-mimeapps.list"
if command -v omarchy-launch-browser >/dev/null && grep -qx 'x-scheme-handler/https=omarchy-browser.desktop' "$list" 2>/dev/null; then
  cp "$list" "$list.bak-tubequ"
  sed -i 's|^x-scheme-handler/https=omarchy-browser.desktop$|x-scheme-handler/https=dev.tubequ.Links.desktop;omarchy-browser.desktop;|' "$list"
  echo "Links:       YouTube-Links aus anderen Apps öffnen jetzt TubeQu (Sicherung: $list.bak-tubequ)"
fi
if grep -q 'dev.tubequ.Links.desktop' "$list" 2>/dev/null; then
  sed "s|@BIN@|$bin|" data/tubequ-links > "$links"
  chmod 755 "$links"
  sed "s|@LINKS@|$links|" data/dev.tubequ.Links.desktop > "$links_desktop"
fi

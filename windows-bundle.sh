#!/bin/sh
# Windows: in MSYS2 (UCRT64) Pakete holen, bauen und mit allen DLLs und GTK-Daten nach target/<Titel>/ legen
# (bin/, lib/, share/ wie unter MSYS2, dann findet GTK seine Daten relativ zur DLL).
# Aufgerufen von install.ps1.
set -e
cd "$(dirname "$0")"
name=$(sed -n 's/^name = "\(.*\)"/\1/p' qu.toml | head -n1)
title=$(sed -n 's/^title = "\(.*\)"/\1/p' qu.toml | head -n1)

# ponytail: -Sy ohne -u (Teil-Update); reicht für frische MSYS2-Installationen, sonst vorher `pacman -Syu`.
pacman -Sy --needed --noconfirm zip git \
  mingw-w64-ucrt-x86_64-rust mingw-w64-ucrt-x86_64-pkgconf mingw-w64-ucrt-x86_64-gtk4 mingw-w64-ucrt-x86_64-mpv \
  mingw-w64-ucrt-x86_64-librsvg mingw-w64-ucrt-x86_64-webp-pixbuf-loader mingw-w64-ucrt-x86_64-adwaita-icon-theme

# neuester quCore-Stand (Cargo.lock hält sonst den alten fest); schlägt fehl, wenn Cargo.lock qu-core noch nicht
# als Git-Quelle kennt (z. B. alter Pfad-Eintrag) – dann löst cargo build es ohnehin frisch auf.
cargo update -p qu-core 2>/dev/null || true
cargo build --release

out="target/$title"
rm -rf "$out"
mkdir -p "$out/bin" "$out/lib" "$out/share/glib-2.0/schemas" "$out/share/icons"
cp "target/release/$name.exe" "$out/bin/"
# App-eigene Zusatzprogramme nach "$out/bin" (ihre DLLs sammelt die Schleife unten mit ein).
if [ -f extra-bundle.sh ]; then . ./extra-bundle.sh; fi
cp -r /ucrt64/lib/gdk-pixbuf-2.0 "$out/lib/"
rm -f "$out"/lib/gdk-pixbuf-2.0/2.10.0/loaders/*.a
cp /ucrt64/share/glib-2.0/schemas/gschemas.compiled "$out/share/glib-2.0/schemas/"
cp -r /ucrt64/share/icons/Adwaita /ucrt64/share/icons/hicolor "$out/share/icons/"
# Bildformat-Lader: Pfade relativ machen (gdk-pixbuf setzt sie unter Windows vor den Programmordner).
gdk-pixbuf-query-loaders | sed -E 's#^"[^"]*(lib[\\/]+gdk-pixbuf-2\.0)#"\1#' > "$out/lib/gdk-pixbuf-2.0/2.10.0/loaders.cache"
# Alle DLLs aus MSYS2, die das Programm und die Lader brauchen (Windows-eigene bleiben draußen).
for f in "$out"/bin/*.exe "$out"/lib/gdk-pixbuf-2.0/2.10.0/loaders/*.dll; do
  ldd "$f" | grep -o '/ucrt64/bin/[^ ]*\.dll' || true
done | sort -u | xargs -I{} cp {} "$out/bin/"
echo "Fertig: $out"

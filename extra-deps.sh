# Von install.sh eingebunden: TubeQu spielt über yt-dlp, das für YouTube eine JS-Laufzeit (deno) braucht.
# JS-Laufzeit: deno oder node (Omarchy-yt-dlp nutzt node über /etc/yt-dlp.conf).
if ! command -v yt-dlp >/dev/null || ! { command -v deno >/dev/null || command -v node >/dev/null; }; then
  if [ "$(uname)" = Darwin ]; then brew install yt-dlp deno
  elif command -v pacman >/dev/null; then sudo pacman -S --needed --noconfirm yt-dlp deno
  else echo "Hinweis: TubeQu braucht noch yt-dlp (https://github.com/yt-dlp/yt-dlp) und deno (https://deno.com)."
  fi
fi

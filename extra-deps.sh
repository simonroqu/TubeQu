# Von install.sh eingebunden: TubeQu spielt über yt-dlp, das für YouTube eine JS-Laufzeit (deno) braucht.
if ! command -v yt-dlp >/dev/null || ! command -v deno >/dev/null; then
  if [ "$(uname)" = Darwin ]; then brew install yt-dlp deno
  elif command -v pacman >/dev/null; then sudo pacman -S --needed --noconfirm yt-dlp deno
  else echo "Hinweis: TubeQu braucht noch yt-dlp (https://github.com/yt-dlp/yt-dlp) und deno (https://deno.com)."
  fi
fi

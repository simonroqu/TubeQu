# Von install.sh eingebunden: TubeQu spielt über yt-dlp, das für YouTube eine JS-Laufzeit (deno) braucht.
# JS-Laufzeit: deno oder node (Omarchy-yt-dlp nutzt node über /etc/yt-dlp.conf).
if ! command -v yt-dlp >/dev/null || ! { command -v deno >/dev/null || command -v node >/dev/null; }; then
  if [ "$(uname)" = Darwin ]; then brew install yt-dlp deno
  elif command -v pacman >/dev/null; then sudo pacman -S --needed --noconfirm yt-dlp deno
  else echo "Hinweis: TubeQu braucht noch yt-dlp (https://github.com/yt-dlp/yt-dlp) und deno (https://deno.com)."
  fi
fi

# PO-Token für das Abspielen mit Login (gegen die Bot-Sperre): rustypipe-botguard erzeugt ihn lokal, das yt-dlp-Plugin
# reicht ihn an yt-dlp weiter. Fehlt eins, spielt TubeQu nach dem Login-Fehlversuch anonym ab.
plugin=yt_dlp_get_pot_rustypipe-0.2.0-py3-none-any.whl
plugins="${XDG_CONFIG_HOME:-$HOME/.config}/yt-dlp/plugins"
if [ ! -f "$plugins/$plugin" ]; then
  mkdir -p "$plugins"
  curl -fsSL -o "$plugins/$plugin" "https://files.pythonhosted.org/packages/py3/y/yt-dlp-get-pot-rustypipe/$plugin" ||
    echo "Hinweis: yt-dlp-Plugin nicht geladen (Abspielen dann ohne Login)."
fi

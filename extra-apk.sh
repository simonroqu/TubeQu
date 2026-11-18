# Von install.sh --apk eingebunden: TubeQus YouTube-Bibliothek (rquickjs) erzeugt beim Bauen Bindings, braucht libclang.
if ! command -v clang >/dev/null; then
  if [ "$(uname)" = Darwin ]; then xcode-select --install 2>/dev/null || true
  elif command -v pacman >/dev/null; then sudo pacman -S --needed --noconfirm clang
  elif command -v apt-get >/dev/null; then sudo apt-get install -y clang libclang-dev
  elif command -v dnf >/dev/null; then sudo dnf install -y clang-devel
  elif command -v zypper >/dev/null; then sudo zypper install -y clang-devel
  else echo "Hinweis: TubeQu braucht für die APK noch libclang (clang)."
  fi
fi

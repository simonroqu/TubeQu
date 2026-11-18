#!/bin/sh
# Baut die Android-App (APK) für Handy und Android TV: ./build-apk.sh  (Linux, macOS; Windows: build-apk.ps1 über WSL).
# Richtet fehlende Werkzeuge ein, ohne root außer für Systempakete (JDK, unzip, C-Compiler):
# Android SDK + NDK nach ~/Android/Sdk, eigenes Rust mit Android-Zielen nach ~/Android/rust (System-Rust bleibt
# unberührt), cargo-ndk. Ergebnis: target/<Titel>.apk. Ist genau ein Gerät per adb verbunden, wird sie gleich
# installiert (Daten bleiben). Erster Lauf lädt einige GB und dauert, danach geht es schnell.
set -e
cd "$(dirname "$0")"
title=$(sed -n 's/^title = "\(.*\)"/\1/p' qu.toml | head -n1)
export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
case "$(uname)" in Darwin) os=mac ;; *) os=linux ;; esac

# Systempakete: JDK ≥ 17 (Gradle), unzip (SDK), C-Compiler (Rust-Bauskripte), curl.
if [ "$os" = mac ]; then
  command -v cc >/dev/null || { xcode-select --install; echo "Nach der Installation der Xcode-Werkzeuge erneut starten."; exit 1; }
  if ! command -v javac >/dev/null && [ -z "$JAVA_HOME" ]; then
    brew install openjdk
    export JAVA_HOME="$(brew --prefix openjdk)"
  fi
elif ! { command -v javac && command -v unzip && command -v cc && command -v curl; } >/dev/null; then
  if command -v pacman >/dev/null; then sudo pacman -S --needed --noconfirm jdk-openjdk unzip base-devel curl
  elif command -v apt-get >/dev/null; then sudo apt-get update && sudo apt-get install -y default-jdk-headless unzip build-essential curl
  elif command -v dnf >/dev/null; then sudo dnf install -y java-latest-openjdk-devel unzip gcc curl
  elif command -v zypper >/dev/null; then sudo zypper install -y java-21-openjdk-devel unzip gcc curl
  else echo "Bitte installieren: JDK ≥ 17, unzip, gcc, curl"; exit 1
  fi
fi

# Android SDK: Kommandozeilen-Werkzeuge (neueste laut Googles Verzeichnis), Lizenzen, Plattform, NDK.
sdkmanager="$ANDROID_HOME/cmdline-tools/latest/bin/sdkmanager"
if [ ! -x "$sdkmanager" ]; then
  zip=$(curl -fsSL https://dl.google.com/android/repository/repository2-3.xml |
    grep -oE "commandlinetools-$os-[0-9]+_latest\.zip" | sort -uV | tail -n1)
  [ -n "$zip" ] || { echo "Android-Kommandozeilenwerkzeuge nicht gefunden (Netz?)."; exit 1; }
  mkdir -p "$ANDROID_HOME/cmdline-tools"
  curl -fsSL -o "$ANDROID_HOME/tools.zip" "https://dl.google.com/android/repository/$zip"
  rm -rf "$ANDROID_HOME/cmdline-tools/latest" "$ANDROID_HOME/cmdline-tools/cmdline-tools"
  (cd "$ANDROID_HOME/cmdline-tools" && unzip -q ../tools.zip && mv cmdline-tools latest)
  rm "$ANDROID_HOME/tools.zip"
fi
if [ ! -d "$ANDROID_HOME/ndk" ] || [ ! -d "$ANDROID_HOME/platform-tools" ]; then
  yes | "$sdkmanager" --licenses >/dev/null || true
  # ponytail: feste Versionen (wie entwickelt); neuere: Plattform passend zu compileSdk in android/app/build.gradle.kts.
  "$sdkmanager" "platform-tools" "platforms;android-37.0" "ndk;30.0.16248370"
fi

# Rust für Android (Pfade wie qu-cores android/build-rust.sh).
export RUSTUP_HOME="${RUSTUP_HOME:-$HOME/Android/rust/rustup}"
export PATH="$HOME/Android/rust/cargo/bin:$PATH"
if [ ! -x "$HOME/Android/rust/cargo/bin/rustup" ] && [ "$RUSTUP_HOME" = "$HOME/Android/rust/rustup" ]; then
  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs |
    CARGO_HOME="$HOME/Android/rust/cargo" sh -s -- -y --no-modify-path --profile minimal
fi
rustup target add aarch64-linux-android x86_64-linux-android
command -v cargo-ndk >/dev/null || [ -x "$HOME/.cargo/bin/cargo-ndk" ] || cargo install cargo-ndk

# App-eigene Zusätze (TubeQu: libclang).
if [ -f extra-apk.sh ]; then . ./extra-apk.sh; fi

(cd android && ./gradlew assembleDebug)
mkdir -p target
cp android/app/build/outputs/apk/debug/app-debug.apk "target/$title.apk"
echo "APK: $PWD/target/$title.apk"

# Unter Windows (WSL) installiert build-apk.ps1 mit dem adb von Windows (NO_ADB=1), WSL sieht keine USB-Geräte.
[ -n "$NO_ADB" ] && exit 0
adb="${ADB:-$ANDROID_HOME/platform-tools/adb}"
devices=$("$adb" devices 2>/dev/null | grep -c '	device$' || true)
if [ "$devices" = 1 ]; then
  "$adb" install -r "target/$title.apk" && echo "Installiert auf dem verbundenen Gerät."
else
  echo "Installieren: $adb install -r target/$title.apk (Gerät mit USB-/WLAN-Debugging) oder die Datei aufs Gerät kopieren."
fi

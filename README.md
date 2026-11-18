# TubeQu

A lightweight YouTube desktop client for Linux (and, untested, Windows and macOS), written in Rust with GTK4 and libmpv.

- Home page with your personal recommendations (when signed in via a browser)
- Search with suggestions, channel pages
- Embedded mpv player with its own controls, chapters, storyboard previews and SponsorBlock
- Continue watching, watch later, history, per-channel playback speed
- Follows your [Omarchy](https://omarchy.org) theme live, ships its own default theme otherwise

## Requirements

- GTK ≥ 4.20
- mpv / libmpv
- [yt-dlp](https://github.com/yt-dlp/yt-dlp) and [deno](https://deno.com) (yt-dlp needs a JS runtime for YouTube)
- Optional: a browser signed in to YouTube (Firefox, Zen, LibreWolf, Chromium, Brave, Chrome) for personal recommendations and search
- Optional: `mpv-mpris` for media keys

## Install

One script per platform: it installs whatever is missing (GTK, mpv, Rust, yt-dlp, deno), builds TubeQu and adds it to
your app launcher / start menu. The first run takes a while (downloading and compiling), later runs are quick.

### Linux

```sh
git clone https://github.com/simonroqu/tubequ
cd tubequ
./install.sh
```

Supports Arch, Debian/Ubuntu, Fedora and openSUSE (asks for your sudo password if packages are missing).
Needs a distribution with GTK ≥ 4.20 (e.g. Arch, Fedora 43, Ubuntu 25.10).
Installs to `~/.local/bin/tubequ` plus a launcher entry and icon.

### macOS (untested)

Needs [Homebrew](https://brew.sh). Then in Terminal:

```sh
git clone https://github.com/simonroqu/tubequ
cd tubequ
./install.sh
```

Installs `~/Applications/TubeQu.app` (shows up in Launchpad and Spotlight, no icon yet) and `~/.local/bin/tubequ`.

### Windows (untested)

In PowerShell:

```powershell
winget install -e --id Git.Git      # once, if git is missing (then open a new PowerShell)
git clone https://github.com/simonroqu/tubequ
cd tubequ
powershell -ExecutionPolicy Bypass -File install.ps1
```

The script installs [MSYS2](https://www.msys2.org) (if missing) and builds TubeQu with GTK and mpv from there.
Installs to `%LOCALAPPDATA%\Programs\TubeQu` plus a start menu entry.

### Update and uninstall

- Update: `git pull` in the folder, then run the install script again. It always builds against the latest
  [qu-core](https://github.com/simonroqu/quCore).
- Uninstall: delete the files the install script lists at the end.
- Building fetches qu-core from GitHub, so it has to be reachable from your machine.

## Usage

Run directly:

```sh
tubequ               # home page
tubequ lofi          # search
tubequ <video-url>   # play
tubequ <channel-url> # open channel
```

## Keyboard shortcuts

| Key | Action |
|---|---|
| `/`, `Ctrl+K` | Search |
| `F5`, `Ctrl+R` | Reload |
| `Ctrl+V` | Open link from clipboard |
| `0`–`9` | Seek to 0–90 % |
| `,` / `.` | Previous / next frame |
| `c` | Toggle subtitles |

Double-click left/right on the player seeks −/+10 s, the middle toggles fullscreen. The mouse wheel changes the volume.

## Data

- `~/.local/share/tubequ/`: progress, watch later, recent searches, speeds
- `~/.config/tubequ/`: quality, volume
- `~/.cache/tubequ/`: thumbnails, cached home page

## Credits

- [RustyPipe](https://codeberg.org/ThetaDev/rustypipe) for YouTube data
- [yt-dlp](https://github.com/yt-dlp/yt-dlp) and [mpv](https://mpv.io) for playback
- [SponsorBlock](https://sponsor.ajay.app) for segment data
- [FSRCNNX](https://github.com/igv/FSRCNN-TensorFlow) upscaling shader by igv (GPL-3.0/LGPL-3.0, via qu-core)
- The default theme uses the colors of the "Last Horizon" theme from [Omarchy](https://github.com/basecamp/omarchy) (MIT)

## Disclaimer

TubeQu is not affiliated with, endorsed by or sponsored by YouTube or Google. "YouTube" is a trademark of Google LLC.
TubeQu does not host any content. Use it in accordance with the laws of your country and the terms of the services you access.

## License

[GPL-3.0-or-later](LICENSE)

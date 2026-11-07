# Tubequ

A lightweight YouTube desktop client for Linux, written in Rust with GTK4 and libmpv.

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

## Build & install

```sh
cargo build --release
./install.sh        # installs to ~/.local/bin/tubequ and adds a launcher entry
```

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
- The default theme uses the colors of the "Last Horizon" theme from [Omarchy](https://github.com/basecamp/omarchy) (MIT)

## Disclaimer

Tubequ is not affiliated with, endorsed by or sponsored by YouTube or Google. "YouTube" is a trademark of Google LLC.
Use it in accordance with the terms of the services you access.

## License

[GPL-3.0-or-later](LICENSE)

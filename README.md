# TubeQu

A lightweight YouTube desktop client for Linux (and, untested, Windows and macOS), written in Rust with GTK4 and libmpv.

- Home page with your personal YouTube recommendations (when signed in, see [YouTube account](#youtube-account))
- Uses your YouTube account from your browser: history, subscriptions, several Google accounts, no extra login
- Search with suggestions, channel pages
- Embedded mpv player with its own controls, chapters, storyboard previews and SponsorBlock
- Continue watching, watch later, history, per-channel playback speed
- Follows your [Omarchy](https://omarchy.org) theme live, ships its own default theme otherwise
- Android app for phones and Android TV (build it yourself, see [Android](#android-phone-and-android-tv))

## Requirements

- GTK ≥ 4.20
- mpv / libmpv
- [yt-dlp](https://github.com/yt-dlp/yt-dlp) and [deno](https://deno.com) (yt-dlp needs a JS runtime for YouTube)
- Optional: a browser signed in to YouTube (Zen, Firefox, LibreWolf, Chromium, Brave, Chrome) for your account, see
  [YouTube account](#youtube-account)
- For playing with your account (installed by the install scripts): [rustypipe-botguard](https://codeberg.org/ThetaDev/rustypipe-botguard)
  and the yt-dlp plugin [yt-dlp-get-pot-rustypipe](https://pypi.org/project/yt-dlp-get-pot-rustypipe/)
- Optional: `mpv-mpris` for media keys

## Install

One script per platform: it installs whatever is missing (GTK, mpv, Rust, yt-dlp, deno, the PO-token tools), builds TubeQu and adds it to
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

### Android (phone and Android TV)

There is no ready-made APK to download: one script builds it from the same folder. Home page (your YouTube home page when signed in, see [Sign in on a phone or TV](#sign-in-on-a-phone-or-tv)), search, playback including live streams, SponsorBlock, watch later, history and continue watching. YouTube links open the app. Playback on Android is always anonymous (via NewPipeExtractor). Not there yet: channel pages.

Works on phones and on Android TV (shows up in the TV launcher, fully usable with the remote). Needs Android 8.0 or newer.

**Linux and macOS (macOS untested):**

```sh
./build-apk.sh
```

**Windows (untested):** in PowerShell, in the cloned folder:

```powershell
powershell -ExecutionPolicy Bypass -File build-apk.ps1
```

Windows builds through [WSL](https://learn.microsoft.com/windows/wsl/) (Linux inside Windows) with the same script.
If WSL isn't set up yet, the first run sets it up with Ubuntu: it asks for administrator rights, then **restart Windows**,
open "Ubuntu" from the start menu once (pick a user name and password) and run the script again. Needs Windows 10
(version 2004 or newer) or Windows 11 with virtualization enabled; company PCs sometimes block WSL.

**What the script does:** it sets up everything that's missing (a JDK, the Android SDK and NDK in `~/Android/Sdk`,
Rust with the Android targets in `~/Android/rust` — your normal Rust stays untouched — and cargo-ndk), then builds
`target/TubeQu.apk`. The first run downloads a few GB and takes a while, later runs are quick. It asks for your password
only to install system packages (JDK, unzip, a C compiler).

The APK is **not** stored in the repository and does **not** update by itself: after every `git pull`, run the script again.

**Install on the device:**

- If exactly one device is connected with USB or wireless debugging (developer options), the script installs the APK
  right away. On Windows this needs adb: `winget install -e --id Google.PlatformTools`.
- Otherwise copy `target/TubeQu.apk` to the device (USB, cloud, a USB stick for TVs), open it and allow installing apps
  from that file manager.
- Android TV with adb: enable developer options and network debugging on the TV, then
  `adb connect <tv-ip>:5555` and `adb install -r target/TubeQu.apk`.

**Updates:** install the new APK over the old one, your data stays. The APK is signed with a debug key that is
created per computer: an APK built on a different computer can't update the installed one, uninstall first
(this deletes progress, watch later and the login).

### Update and uninstall

- Update: `git pull` in the folder, then run the install script (and `build-apk` for Android) again. It always builds against the latest
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

## YouTube account

TubeQu has no login screen of its own (Google blocks sign-ins from embedded browsers). It uses the browser you
are already signed in to YouTube with, nothing to set up:

- At startup TubeQu looks for a browser profile: Firefox-like browsers first (Zen, Firefox, LibreWolf; the most
  recently used profile), otherwise Chromium, Brave or Chrome. The usual folders on Linux, macOS and Windows are checked.
- yt-dlp reads the cookies locally. They are only sent to youtube.com (and, if you ask for it, to your TV, see below).
- To sign out, sign out of YouTube in the browser. TubeQu always follows the browser.
- Chrome on Windows locks its cookies so that they can't be read. Use Firefox or Zen there, otherwise TubeQu runs anonymously.

**With an account you get:**

- Home page = your YouTube recommendations
- Search results personalized like on youtube.com
- Watched videos land in your YouTube history, including the position (continue on other devices)
- Subscribe / unsubscribe on channel pages
- Videos still play when YouTube shows its bot check, and age-restricted videos play (see below)

**Without an account** everything works anonymously. The home page is then built from videos related to the last
three you watched, so search and watch a few videos first.

**Several Google accounts:** open the menu (top left), your profile picture is at the bottom. Click it to see your
other accounts and pick one. Switching reloads the home page. Account 0 is the one you signed in to first in the
browser, not necessarily the one selected on youtube.com.

### Sign in on a phone or TV

The Android app can't read browser cookies. It gets your login from TubeQu on your computer instead:

1. Phone/TV: select the account icon in the bubble at the top left. It shows a code like `23-12345678` and waits.
2. Computer (TubeQu signed in, same home network): menu → **Sign in on TV** → enter the code.
3. The computer finds the device in your network and sends the login. The device checks the code, stores the login
   inside the app and stops listening. After 3 wrong codes, or when you leave the page, it stops as well.

Signed in, the Android home page shows your YouTube home page and watched videos go into your history.
Playback on Android stays anonymous. To sign out on the device, select your profile picture in the bubble → **Sign out** (deletes the stored login).

> The login is sent **unencrypted** within your home network, and the cookies give full access to your Google
> account. Only do this in a network you trust.

## Bot check ("Sign in to confirm you're not a bot")

YouTube blocks an IP address for a while if it fetches videos anonymously too often. What TubeQu does about it:

- **Request limit:** at most 200 requests per hour and at least 2 s between them; results are cached for 10 minutes.
  This also applies across restarts (`~/.cache/tubequ/requests.txt`) and on Android.
- **Fast first, account as backup:** videos load anonymously first, which is the fastest way. If that fails (bot check,
  age restriction), TubeQu tries once more with your account plus a *PO token*, a proof that youtube.com normally
  creates in the browser. `rustypipe-botguard` creates it locally and the yt-dlp plugin `yt-dlp-get-pot-rustypipe`
  hands it to yt-dlp; the install scripts set up both. That second try takes a few seconds longer.

If you still see the message: it goes away by itself, usually within hours. Avoid restarting or reloading
over and over in the meantime. To check the setup:

```sh
rustypipe-botguard --version     # the tool (Windows: next to tubequ.exe)
ls ~/.config/yt-dlp/plugins      # the plugin (Windows: %APPDATA%\yt-dlp\plugins)
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

Paths for Linux and macOS; on Windows the same files are in `%APPDATA%\tubequ` and `%LOCALAPPDATA%\tubequ` (cache).
On Android everything stays inside the app (uninstalling deletes it).

- `~/.local/share/tubequ/`: progress, watch later, recent searches, speeds
- `~/.config/tubequ/`: quality, volume, selected Google account (`account`)
- `~/.cache/tubequ/`: thumbnails, cached home page, known accounts (`accounts.tsv`), requests of the last hour (`requests.txt`)
- Android: the login from your computer (`login.json`) stays inside the app

## Credits

- [RustyPipe](https://codeberg.org/ThetaDev/rustypipe) for YouTube data
- [yt-dlp](https://github.com/yt-dlp/yt-dlp) and [mpv](https://mpv.io) for playback,
  [NewPipeExtractor](https://github.com/TeamNewPipe/NewPipeExtractor) on Android
- [rustypipe-botguard](https://codeberg.org/ThetaDev/rustypipe-botguard) for PO tokens
- [SponsorBlock](https://sponsor.ajay.app) for segment data
- [FSRCNNX](https://github.com/igv/FSRCNN-TensorFlow) upscaling shader by igv (GPL-3.0/LGPL-3.0, via qu-core)
- The default theme uses the colors of the "Last Horizon" theme from [Omarchy](https://github.com/basecamp/omarchy) (MIT)

## Disclaimer

TubeQu is not affiliated with, endorsed by or sponsored by YouTube or Google. "YouTube" is a trademark of Google LLC.
TubeQu does not host any content. Use it in accordance with the laws of your country and the terms of the services you access.

## License

[GPL-3.0-or-later](LICENSE)

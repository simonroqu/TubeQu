# TubeQu – YouTube-Client für Linux (Rust + GTK4 + libmpv)

Eigene YouTube-App ohne Werbung, entwickelt auf Omarchy/Hyprland, läuft aber auch ohne Omarchy
(eingebautes Standard-Theme). Startseite = persönliche YouTube-Empfehlungen, Suche mit Vorschlägen,
Kanalseiten, eingebetteter Player mit eigener Bedienleiste. Schwester-Apps: AniQu (`~/Work/aniqu`), BoxQu
(`~/Work/boxqu`); gemeinsamer Unterbau über **qu-core** (`~/Work/quCore`).

**Code-Kommentare: Deutsch. Alle Texte in der Oberfläche: Englisch** (auch Fehlermeldungen,
Tooltips, „3 days ago“ usw.). Keine deutschen UI-Strings einführen.

**Diese Datei bei jeder Änderung am Projekt mitpflegen** (gleicher Schritt wie die Code-Änderung), damit eine
neue Session sofort Bescheid weiß. Nur Entwicklungsrelevantes, keine persönlichen Bemerkungen.

**Zwei Oberflächen, eine Logik:** Desktop (GTK, `src/`) und Android/TV (Compose, `android/`). Jede Änderung am Aussehen
oder an der Bedienung **immer in beiden** umsetzen (gleicher Schritt), Android danach per Fernbedienung prüfen. Logik
gehört nach `core/` und gilt dann für beide; neue Funktionen für Android in `ffi/` exportieren. Geht etwas nur auf einer
Seite (z. B. Miniplayer, Konto), kurz beim Nutzer nachfragen. **Auch auf Android gilt das Anfragen-Limit** (unten).

## Bauen, starten, installieren

```bash
cargo build && cargo test            # Unit-Tests, ohne Netzwerk
TUBEQU_DEV=1 cargo run               # eigene Instanz neben der installierten (siehe Tests)
cargo run -- lofi                    # direkt suchen
cargo run -- <video-link>            # direkt abspielen
cargo run -- <kanal-link>            # .../channel/UC... öffnet die Kanalseite
./install.sh                         # Release-Build nach ~/.local/bin/tubequ + Startmenü-Eintrag + Icon
```

Install-Skripte für Linux/macOS/Windows (`install.sh`, `install.ps1`, `windows-bundle.sh`) sind in allen Apps gleich, siehe
`~/Work/aniqu/CLAUDE.md` Abschnitt Windows.

Eine laufende installierte Instanz bekommt das neue Binary erst nach einem Neustart.

Installiert: `~/.local/bin/tubequ`, `~/.local/share/applications/dev.tubequ.Tubequ.desktop`,
`~/.local/share/icons/hicolor/scalable/apps/dev.tubequ.Tubequ.svg`. App-ID `dev.tubequ.Tubequ`.

Link-Weiche (nur Omarchy): `install.sh` setzt in `~/.config/hyprland-mimeapps.list` **nur https** auf
`dev.tubequ.Links.desktop` → `~/.local/bin/tubequ-links` (YouTube-Video/-Kanal → TubeQu, sonst
`omarchy-launch-browser`). http bleibt bei `omarchy-browser.desktop`, weil `omarchy-cmd-default-browser`
den Browser am http-Eintrag erkennt (sonst Schleife + Super+B kaputt). Test: `TUBEQU_LINKS_DRYRUN=1 tubequ-links <url>`.
Rückgängig: https-Zeile wieder auf `omarchy-browser.desktop` (Sicherung `.bak-tubequ`).

Systemvoraussetzungen: `gtk4` (≥ 4.20), `mpv`/libmpv, `yt-dlp` + JS-Laufzeit (Omarchy-yt-dlp ab 2026.10.26:
`node` über `/etc/yt-dlp.conf` `--js-runtimes node`, Challenge-Löser ejs ist eingebaut; sonst `deno`). Browser-Login (Zen/Firefox/LibreWolf/Chromium/Brave/Chrome) optional.

## Android (`android/` + `ffi/`, Stand 2026-11-18)

Wie AniQu (Werkzeuge, Bauen, Waydroid-Test, Fernbedienungs-Regeln: `~/Work/aniqu/CLAUDE.md` Abschnitt Android; Bausteine
aus qu-core `android/`). `ffi/` = `tubequ-ffi` (UniFFI über `tubequ-core`; Karten mit fertigen Texten aus `core/fmt.rs`,
Seiten-Fortsetzung bleibt in Rust: `Page.next` → `more(next)`), `android/app/.../MainActivity.kt` = Seiten.
- **Anonym** (kein Browser-Login, kein yt-dlp): Startseite = eigene Empfehlungen aus ähnlichen Videos (`own_recommendations`),
  Suche über rustypipe, „Watch later“/„History“ als Knöpfe in der Bubble, „Continue watching“ nach 4 Reihen (höchstens 3).
- **Abspielen über NewPipeExtractor** (Kotlin, JitPack `TeamNewPipe:NewPipeExtractor`, `Http`-Downloader mit OkHttp):
  rustypipe kann nicht abspielen (siehe Lücken). Vorher **immer `throttle()`** aus Rust (eine Extraktion = 1 Anfrage, wie
  yt-dlp am Desktop). Bild + Ton getrennt (H.264 bis 1080p bevorzugt, AAC), je als **DASH-Manifest**
  (`YoutubeProgressiveDashManifestCreator`, als `data:`-Adresse): die ganze Datei auf einmal angefordert bricht YouTube nach
  wenigen Sekunden ab (wie mpvs 10-MiB-Stücke am Desktop). Livestreams per HLS. SponsorBlock automatisch.
  YouTube-Links (youtube.com, youtu.be) öffnen die App und spielen direkt.
- **rustypipe nur über HTTP/1.1** (`RP` in `youtube.rs`): mit rustls/HTTP/2 bricht YouTube ab („stream error … protocol
  error“, 2026-11-18). Am Desktop war es durch native-tls ohnehin HTTP/1.1.
- rustypipes JS-Engine `rquickjs` hat keine Android-Bindings → Feature `bindgen` nur für Android (`core/Cargo.toml`), braucht
  libclang; das API-Level dafür setzt qu-cores `build-rust.sh`.
- **Anmelden auf Fernseher/Handy per Code** (`core/pair.rs`, so gewollt): Android → Bubble-Knopf Konto → Seite zeigt
  „<IP-Endung>-<8 Ziffern>“ und wartet auf TCP `pair::PORT` (47821). Desktop → Burger-Spalte „Sign in on TV“ (TV-Icon,
  `SearchModal::input`, nichts im Suchverlauf) → `youtube::login_for_tv` (Browser-Cookies lokal per yt-dlp + Konto aus
  `accounts.tsv`, **keine** YouTube-Anfrage) → `pair::send` sucht den Fernseher in allen eigenen IPv4-Netzen (erste drei
  Stellen + IP-Endung, ausgehende Verbindung, darum kein Firewall-Problem) und schickt eine JSON-Zeile. Fernseher prüft die
  8 Ziffern (Vergleich ohne frühen Abbruch), speichert `login.json` (App-privat), setzt das Konto, schließt den Port; nach
  3 falschen Codes oder Verlassen der Seite (`AbortOnDrop` in `ffi`) ebenfalls zu.
  **Verschlüsselt** (seit 2026-11-18, die Cookies sind der ganze Google-Login): aus den 8 Ziffern per SPAKE2 (`spake2`)
  ein 256-Bit-Schlüssel (Mitlesen erlaubt kein Offline-Durchprobieren), Login mit ChaCha20-Poly1305 (`chacha20poly1305`),
  Zufall über `getrandom`; alles reines Rust, läuft auf Linux/macOS/Windows/Android. Ablauf in `pair.rs` oben.
  **Feste Kopplung**: der ausgehandelte Schlüssel bleibt gespeichert (TV `pair.key`, bleibt beim Abmelden; Desktop
  `tvs.tsv` = IP + Schlüssel, Einträge der unverschlüsselten Vorversion gelten nicht). Danach schickt „Sign in on TV“ am Desktop direkt (`send_paired`, frische
  Cookies, kein Code); klappt das nicht (TV-App zu, neue IP), öffnet sich die Code-Eingabe. Die TV-App wartet dafür,
  solange sie läuft, außer auf der Konto-Seite (`receive(None)` in `App`, nur mit dem Schlüssel Verschlüsseltes; `bind` wartet
  kurz, bis der Port der anderen Seite frei ist).
  Angemeldet: `logged_in()`/`browser_cookies()` nehmen den übertragenen Login → YouTube-Verlauf beim Abspielen
  (`mark_watched`, 1 Anfrage) und Position beim Verlassen (Ping). „Sign out“ löscht `login.json`. Abspielen bleibt anonym.
  Getestet 2026-11-18: Code in Waydroid, Dummy-Login vom Desktop → angemeldet, zweiter Versuch findet den Port zu,
  Abmelden; echte Browser-Cookies am Desktop gefunden (nur gezählt, nicht gesendet).
  **Startseite angemeldet** = YouTube-Startseite wie auf youtube.com: innertube `browse` `FEwhat_to_watch` mit dem
  übertragenen Login (`feed_page`/`parse_feed`, 1 Anfrage pro Seite, Nachladen über Fortsetzungs-Token `More::Feed`);
  Werbung und Shorts-Regal fallen weg, bei Fehler eigene Empfehlungen. Testdatei `core/testdata/home_feed.json`
  (echte Antwort 2026-11-18, gekürzt) – ändert YouTube das Format, dort neu abgleichen.
  **Abos** (Bubble-Knopf, wie Desktop) = innertube `browse` `FEsubscriptions` über denselben `feed_page`/`parse_feed`
  (geprüft 2026-11-18: gleiche Struktur, `lockupViewModel` + Shelves, 100 Videos/Seite). Ohne Login: Fehlertext.
  Lehnt YouTube den übertragenen Login ab (abgelaufene Cookies), antwortet es trotzdem mit `loggedOut: true` und
  leerem Abo-Feed → `logged_out` macht daraus einen Fehler („Sign in on TV again“), Startseite fällt auf eigene zurück.
  `innertube()` meldet dann sofort ab (`pair::forget`, bei jeder Login-Anfrage geprüft). Ursache laut yt-dlp-Doku: Google
  erneuert die Konto-Cookies in offenen YouTube-Tabs regelmäßig, kopierte Cookies verfallen dann (nicht an PC-Neustart
  gebunden). Privates Fenster hilft hier nicht: `login_for_tv` liest die Cookies aus dem Browserprofil.
  Noch nicht: YouTube-Verlauf/„Watch later“ von YouTube anzeigen.
- Startseite neu laden (wie Desktop): Home-Knopf auf der Startseite und nach An-/Abmelden (`Nav.reloadHome`, einmalig über
  `takeReload`, leert vorher den Cache = bis 3 Anfragen). Zurückkommen lädt nicht neu.
- Achtung Anfragen: Start der App lädt die Startseite neu, wenn `home.jsonl` älter als 10 min ist (eigene Empfehlungen =
  bis 3 Anfragen) – bei Tests mit mehreren Neustarts mitzählen.
- Kanal auf Android über rustypipe (`channel_rp`, ohne Dauer/Aufrufe/Datum), aber noch **keine Kanalseite** in der App.
  Ebenfalls noch nicht: Konto/Abo, Suchvorschläge (nur Verlauf), „Watch later“ setzen, Video-Infos.
- **Getestet (Waydroid, 2026-11-18, 4 Anfragen, kein Bot-Check):** Start, Bubble, Hinweis ohne Empfehlungen (ohne Anfrage),
  Suche (Karten mit LIVE/Dauer, Aufrufe, Datum), Abspielen über NewPipeExtractor mit DASH-Manifest (läuft durch, > 1 min geprüft; 6 Anfragen). Noch nicht live
  geprüft: Livestream (HLS), Startseiten-Empfehlungen nach geschauten Videos, Nachladen.
`cd android && ./gradlew assembleDebug`. Für Nutzer: `./build-apk.sh` / `build-apk.ps1` (siehe `~/Work/aniqu/CLAUDE.md`
Abschnitt Android), TubeQu-Zusatz `extra-apk.sh` (libclang). Windows-Bündel: `extra-bundle.sh` legt rustypipe-botguard.exe dazu.

## Aufbau (`src/` = GTK-App, `core/` = Crate `tubequ-core` ohne Oberfläche: `youtube`, `fmt`, `progress`, `lists`, `sponsorblock`)

| Datei | Aufgabe |
|---|---|
| `main.rs` | Fenster, Navigation (`Nav`: Seiten `browse`/`channel`/`watch`, Verlauf zurück/vor), schwebende Nav-Bubble inkl. Kontowahl, Tastenkürzel, Maus-Seitentasten, Startargumente, Single-Instance (`HANDLES_COMMAND_LINE`), tokio-Runtime + `bg()` |
| `core/youtube.rs` | **Einzige Stelle, die mit YouTube spricht.** Rate-Limit, Cache, Suche, Vorschläge, Kanal, Details, Startseite (yt-dlp), Verlauf-Eintrag, Konto/Abonnieren (innertube), Thumbnails/Avatare |
| `grid.rs` | Video-Raster (2 FlowBoxen + „Continue watching“ nach 4 Reihen), Karten, Nachladen beim Scrollen |
| `core/fmt.rs` | Texte für Karten/Infos: `fmt_views`, `fmt_ago`, `fmt_duration`, `meta` („45K views · 3 days ago“), `badge` (Dauer/„● LIVE“) – Desktop und Android |
| `channel.rs` | Kanalseite: Avatar, Name, Abonnentenzahl, Abo-Knopf, eigenes Raster (nur Videos, kein Live-Tab) |
| `player.rs` | Player-Anbindung an qu-core: `Hooks` (yt-dlp-Optionen, Fortschritt + „Watch later“ aufräumen, Position an YouTube, SponsorBlock, Tempo pro Kanal, zweiter Versuch mit Login), `play`/`reload`, yt-dlp-Qualität (`format_for`) |
| `watch.rs` | Infos unter dem Video (Titel, Kanal, Beschreibung, Platzhalter bis Details da sind) im `WatchFrame` aus qu-core |
| `qu.toml` | App-Konfiguration für qu-core: Name/Titel/ID/Startmenü-Texte, **Menü oben links** (`[nav]`), Suche, Player, Fortschrittsregeln, Raster. Format in `~/Work/quCore/CLAUDE.md`; Test `qu_toml` in `main.rs` prüft sie |
| **qu-core** (`~/Work/quCore`) | Gemeinsamer Unterbau, **dort ändern**: App-Start (`start`, `--install`), Dateien/Tempo (`store`), Fortschrittsregeln (`progress`), Raster-Bausteine (`cards`), Fenster/Seitenverlauf/Miniplayer (`nav`), Player (`player`, `controls`, Rahmen `watch`), Menü (`navbar`), Ladeanzeige (`Busy`), `bg`, CSS-Schichten (`style`), `app` (Pfade), `theme`, gemeinsames CSS (`qu_core::CSS`), `icons` (`qu-*-symbolic`), `nav_shape`, `fly`, `smooth`, `search` (Suchfenster + Suchverlauf), `upscale` (Shader-Set hier: `UPSCALER` in `controls.rs`). Beschreibung in `~/Work/quCore/CLAUDE.md` |
| `core/progress.rs` | Fortschritt pro Video in `~/.local/share/tubequ/progress.tsv` (+ Titel/Kanal/Datum für „Continue watching“) |
| `core/lists.rs` | Watch later, letzte Suchen, Tempo pro Kanal |
| `core/sponsorblock.rs` | SponsorBlock-Abschnitte (sponsor.ajay.app, Hash-Präfix, kein YouTube-Request) |

Muster: Netzwerk nie im GTK-Thread. `glib::spawn_future_local(async { bg(fut).await })` –
`bg()` führt auf tokio aus, Ergebnis kommt im GTK-Thread an. Verspätete Antworten werden per
`generation`/aktueller ID verworfen.

## Datenquellen – was funktioniert (Stand 2026-11)

| Was | Quelle | Bemerkung |
|---|---|---|
| Video-Details, Vorschläge | rustypipe 0.11.4, `Language::En` | |
| Kanalseite (Name, Avatar, Abos, Videos) | `yt-dlp -J --flat-playlist …/channel/<id>/videos` + `youtubetab:approximate_date` (mit Login: Cookies + Konto-Header) | 1 Anfrage, Nachladen über `ytdlp_page`; Datum ungefähr, Aufrufe + Dauer vorhanden; Avatar = `avatar_uncropped` mit `=s176`; Abonnentenzahl = `channel_follower_count` |
| Suche | mit Login `yt-dlp --cookies-from-browser … ytsearchall:<q>` (personalisiert), sonst/bei Fehler rustypipe | gemeinsamer Code mit Startseite: `ytdlp_page` |
| Startseite (persönliche Empfehlungen) | `yt-dlp --cookies-from-browser … :ytrec`; Android angemeldet: innertube `browse` (`feed_page`) | Datum nur ungefähr (`youtubetab:approximate_date`), Aufrufe fehlen oft; ohne Login eigene Empfehlungen aus ähnlichen Videos |
| Abspielen (Android) | NewPipeExtractor (Kotlin), anonym, vorher `throttle()` | siehe Abschnitt Android |
| Abspielen | mpv → ytdl_hook → yt-dlp; zuerst anonym `player_client=visionos` (`ANON_YTDL_OPTIONS`, schnell); scheitert das (Bot-Sperre, Alter, 403), Zweitversuch mit Browser-Login **Cookies + Konto + PO-Token**, `player_client=web_safari` (HLS; `login_ytdl_options`) | Format über `ytdl-format`, Qualität in `~/.config/tubequ/quality`. Standard-Clients von yt-dlp 2026.10.26 (`tv_samsung`, `visionos`, `web`): `tv_samsung`-Streams (`c=TVHTML5`) bekommen seit 2026-11-16 unregelmäßig 403 → „Couldn't load video“ |
| Als gesehen melden | `youtubei/v1/player` per `innertube` (Cookies, Konto) → `playbackTracking`; Ping an `videostatsPlaybackUrl` (Verlauf), bei Pause/Videowechsel/Stopp Ping an `videostatsWatchtimeUrl` mit echter Position (`cmt`, `st=0`, `et=pos`) | 1 Anfrage über `throttle()`; Pings an s.youtube.com ungedrosselt. **Nicht** `yt-dlp --mark-watched`: meldet immer `len-1` → Video bei YouTube ganz gesehen. Abspielen selbst bleibt ohne Login |
| Abos (Burger-Spalte „Subscriptions“) | `yt-dlp --cookies-from-browser … :ytsubs` (`youtube::subscriptions`); Android angemeldet: innertube `FEsubscriptions` | Neueste zuerst = Reihenfolge von YouTube, nicht nachsortiert; 1 Anfrage pro 30 Videos, ohne Login nur Fehlertext |
| Thumbnails/Avatare | `i.ytimg.com` / yt3 per reqwest, Cache `~/.cache/tubequ/thumbs` | zählt nicht zum Limit, max. 6 parallel |
| SponsorBlock | sponsor.ajay.app | zählt nicht zum Limit |
| Abonnieren/Deabonnieren | `youtubei/v1/subscription/(un)subscribe` per reqwest (`youtube::innertube`), Cookies via `yt-dlp --cookies-from-browser … --cookies <tmp>` (ohne URL: bricht ab, schreibt die Datei aber, kein Netz), Header `SAPISIDHASH`/`SAPISID1PHASH`/`SAPISID3PHASH` | 1 Anfrage über `throttle()`. Erfolg nur, wenn die Antwort `"subscribed":true/false` enthält (HTTP 200 allein heißt nichts). Abo-Zustand wird **nicht** abgefragt (wäre 1 Anfrage pro Kanalbesuch), nur in der Sitzung pro Konto gemerkt |
| Konto (mehrere Google-Konten im Browser) | `X-Goog-AuthUser: <n>`; bei yt-dlp per `--add-header` (YouTube-Seite liefert dann `SESSION_INDEX` n, yt-dlp übernimmt es für alle API-Aufrufe; getestet 2026-11-07). Kontoliste über `account/accounts_list` | YouTube nennt pro `accounts_list`-Anfrage nur das Konto der mitgeschickten Nummer → 0, 1, 2 … abfragen bis Fehler/Duplikat. Konto 0 = zuerst im Browser angemeldetes, **nicht** das auf youtube.com gewählte |

Bekannte Lücken (nicht nochmal „neu entdecken“):
- rustypipe-Kanal-Videos: keine Dauer, keine Aufrufe, kein Datum (Layout-Änderung bei YouTube) →
  darum läuft die Kanalseite über yt-dlp. yt-dlp meldet nicht, ob ein Kanal Livestreams hat.
- rustypipe `subscriber_count` auf der Kanalseite falsch (Lofi Girl = 481); in Video-Details korrekt.
- YouTube-Kanal-RSS (`/feeds/videos.xml`) liefert **404** – abgeschaltet.
- rustypipe `video_details` scheitert seit 2026-11-06 bei manchen Videos („could not find
  secondary_info“: YouTube schickt den Kanal als `avatarStack`/`attributedTitle`, z. B. bei
  Gemeinschafts-Videos). Ersatz: yt-dlp-JSON, das mpv beim Abspielen ohnehin holt
  (`user-data/mpv/ytdl/json-subprocess-result/stdout`, doppelt JSON-kodiert) → `details_from_ytdlp`;
  Kanal-Logo dann über die (zwischengespeicherte) Kanal-Anfrage. Fehlerberichte von rustypipe liegen
  in `~/.cache/tubequ/rustypipe_reports/` (mit kompletter YouTube-Antwort) – erste Anlaufstelle.
- rustypipe `player()` scheitert („could not get deobf data“) → rustypipe kann **nicht** abspielen.
- **Abspielen mit Login als Zweitversuch (seit 2026-11-18, gegen die Bot-Sperre der IP):** Erst anonym (visionos), weil der
  Login-Weg spürbar langsamer lädt (Webseite, JS-Aufgabe über node, PO-Token; vom Nutzer bemerkt, darum umgedreht).
  Login-Weg: Cookies + Konto-Header +
  PO-Token. Den Token erzeugt `rustypipe-botguard` (Programm, lokal), das yt-dlp-Plugin `yt-dlp-get-pot-rustypipe`
  (.whl in `~/.config/yt-dlp/plugins` bzw. `%APPDATA%\yt-dlp\plugins`) reicht ihn weiter. Installiert von
  `install.sh`/`extra-deps.sh`/`extra-deps.ps1`/`extra-bundle.sh` (Windows: exe neben tubequ.exe). Client muss
  `web_safari` sein: yt-dlps Standard mit Login ist `web_embedded`, dafür fordert yt-dlp keinen Token an → 403 (getestet).
  Der eine Zweitversuch (`retry_opts`, `with_raw`, mpv-Schreibweise `%n%wert`) nimmt den Login, wenn der anonyme Abruf
  scheitert (auch altersbeschränkte Videos); kostet dann 1 Anfrage mehr.
  `main.rs` hängt Programmordner, `~/.local/bin`, `~/.cargo/bin`, Homebrew an PATH (Finder/Startmenü kennen sie nicht).
  Browser-Login wird auch in macOS-/Windows-Profilordnern gesucht (`COOKIES`).
- Liken ist bewusst nicht eingebaut.
- Eigener Google-Login ohne Browser geht nicht: eingebettete Browser blockiert Google beim Login,
  TV-/OAuth-Login für YouTube ist gesperrt (yt-dlp hat ihn 2024 entfernt). Darum Browser-Cookies.
- Shorts und angekündigte Premieren werden überall herausgefiltert (`Page::from`, `video_from_json`).
  Mitglieder-/Premium-Videos ebenfalls (`availability` von yt-dlp).
- Ein Wechsel zu `Language::En` liefert Originaltitel; mit `De` übersetzt YouTube Titel automatisch.

## Anfragen-Limit und Bot-Sperre (wichtig!)

`youtube::throttle()` ist das selbstgebaute Limit (kein offizielles YouTube-Limit):
`MIN_GAP` 2 s zwischen Anfragen, `MAX_PER_HOUR` 200, `CACHE_TTL` 10 min. Anfragen der letzten
Stunde stehen in `~/.cache/tubequ/requests.txt`, Limit + Mindestabstand gelten also über Neustarts.
Pro Video-Klick: Stream (1, in `main.rs` vor `player.play`) + Details (1) + Verlauf (1) = 3.
Reihenfolge wichtig: Stream zuerst, erst danach `watch.load_details` – sonst wartet das Video 2 s.
Reload (`F5`/`Strg+R`/`R`, erneuter Klick auf den Knopf der aktuellen Seite; Reload-Icon unter Video-Fehlern) = 1 Anfrage über
`throttle()`; Seiten-Reload leert vorher den Seiten-Cache (`youtube::forget_cache`).
Startseite/Kanal: 1 pro 30 Videos; Nachladen Seite k zählt k Anfragen (`ytdlp_page`, yt-dlp blättert
intern ab Anfang). Start lädt die Startseite nicht neu, wenn `home.jsonl` < `CACHE_TTL` alt
(`saved_home_fresh`). Suchvorschläge laufen bewusst **ohne** Limit (Debounce + Cache).

Bot-Sperre („Sign in to confirm you're not a bot“, IP-bezogen, läuft von selbst aus) wurde zweimal
ausgelöst: durch Testskripte, die mpv/yt-dlp am Limit vorbei starteten (~15 Extraktionen in Minuten),
und durch ~12 App-Neustarts in einer Stunde, als Limit/Cache noch nur im Speicher lagen.
Fix: Abspielen mit Login + PO-Token (oben, seit 2026-11-18 wieder drin, auf Wunsch des Nutzers).

Diagnose-Hinweis: `curl` auf Stream-URLs kann bei eingeloggten Abrufen fälschlich 403 zeigen
(anderer User-Agent). Aussagekräftig ist nur echtes Abspielen mit mpv.

## Tests – Regeln

- `cargo test` (offline): Rate-Limit (pausierte tokio-Zeit), Formatierungen, Fehlertext aus yt-dlp,
  Fortschritt-Datei, Theme-Parsing/Rundung, SponsorBlock-Logik, Beschreibungs-Markup, Video-ID aus
  Links, Player-Höhe, Nav-Form, Kanal-/Konto-JSON, Cookie-Datei + Login-Header.
- `cargo test -- --ignored --nocapture`: **Live-Test gegen YouTube** (Suche, Nachladen, Thumbnail,
  Kanal, Details, Startseite) – macht mehrere echte Anfragen. Nur gezielt und selten ausführen.
- **Live-Anfragen nur nach Rückfrage beim Nutzer** (mit Angabe, wie viele). Standard-Prüfung
  = `cargo build && cargo test`. Neue Funktionen dürfen keine zusätzlichen Abfragen an YouTube
  erzeugen (kein Vorladen, keine Wiederhol-Schleifen, immer über `throttle()`).
- Live-Anfragen knapp halten, mehrere Messungen in **einer** mpv/yt-dlp-Sitzung bündeln. Direkte
  Testanfragen am Limit vorbei in `~/.cache/tubequ/requests.txt` nachtragen. Bei „not a bot“ sofort
  aufhören und Bescheid geben. Antworten zum Entwickeln einmal speichern und offline parsen.
- Der Nutzer hat die installierte App meist offen. Sichtprüfung per Screenshot daher nur mit
  `TUBEQU_DEV=1` (sonst landet der Aufruf in der offenen Instanz), Fenster über `hyprctl clients -j`
  per **PID** des selbst gestarteten Prozesses finden, `grim -g` aufnehmen; beenden nur mit `kill <pid>`.
  Ohne YouTube-Anfragen: Fake-`HOME` im Scratchpad (leere `cookies.sqlite` in einem Zen-Profilordner
  aktiviert den Login-Zweig, frische `home.jsonl` verhindert das Neuladen, `accounts.tsv` das Konto).
  Offline-Videos: lokaler Range-Server im Scratchpad; `.mp4`-Adressen öffnet mpv direkt.
  `inhibitingIdle` in `hyprctl clients` zeigt die Bildschirmschoner-Sperre.
- **Nie** `wtype`/Eingaben simulieren (landet in fremden Fenstern), **nie** `pkill mpv` oder breite
  `pkill -f`-Muster. Ohne Maus/Tastatur: Startargumente (`tubequ lofi`, Links) oder ein temporärer Hack.
- Temporäre Test-Hacks (z. B. Menü per Timer öffnen, `HIDE_AFTER` auf 60 s, `ROWS_BEFORE_SHELF` auf 1,
  Testeinträge in `progress.tsv`) danach **nachweislich** zurücksetzen (grep).
- Theme-Tests mit Fake-`HOME` im Scratchpad, nie das echte Theme ändern.

## Design-Regeln

- Keine Kopfzeile. Oben links schwebende **Bubble** (Burger/Zurück/Home/Suche). Auf Übersicht/Kanal
  immer sichtbar; auf der Ansehen-Seite wandert sie **in den Player** und blendet sich mit dessen
  Leiste aus (`Nav::show`).
- Unter Tiling-WMs (`XDG_CURRENT_DESKTOP` enthält hyprland/sway/niri/river/i3) keine Fenster-
  Knöpfe; sonst `WindowControls` in Bubble oben rechts + `WindowHandle` zum Verschieben.
- Player-Leiste: Bubble-Design, alle Bubbles gleich hoch (32 px), Buttons 28 px, Timeline ohne
  Hintergrund (6 px), weiß statt Signalfarben. Erscheint nur bei Mausbewegung **über dem
  Player**, weg nach 2 s oder sofort beim Verlassen; pausiert/Menü offen bleibt sie.
  Ruht die Maus 2 s, verschwindet auch die Hover-Hervorhebung (Klasse `idle` am Player).
  Bewegung zählt nur bei geänderter Position (qu-core `player.rs`): GTK schickt nach Layout-Änderungen (Ein-/Ausblenden,
  Zeit) Bewegungs-Ereignisse an derselben Stelle nach – sonst blendet sich die Leiste nie aus (wie AniQu).
- Buttons: **immer Icons, keine Beschriftung** (z. B. Reload = `tubequ-reload-symbolic`, nicht „Reload“);
  Erklärung nur als Tooltip. Kein Rahmen/Hintergrund, nur Hover-/Klick-Effekt. Eigene minimalistische
  Icons (`tubequ-*-symbolic`, 16×16, `fill="#000"`, GTK färbt sie in der Schriftfarbe), keine Emojis.
- Laden von Inhalten = graue Platzhalter in der Form des Inhalts mit Glanzstreifen (qu-core-CSS `.skeleton`): Raster/Nachladen `grid.rs` `ghost_card` (Breiten über `part`, keine festen Pixel), Ansehen-Seite `watch.rs` `skeleton`, Kanalkopf `channel.rs` `ghost`. Sonst (Bubble, Player) Spinner, kein Text. Spinner = `icons::spinner()` (gedrehtes Icon), **nie `gtk::Spinner`**:
  der friert in GTK 4.24 nach einmal Ausblenden (auch Seitenwechsel) für immer ein.
- Keine knalligen Farben (Sponsor-Bereiche dezent in Schriftfarbe, Abo-Symbol ohne YouTube-Rot).
- Popover (z. B. Player-Einstellungen) in Theme-Farben, Auswahlpunkte in Akzentfarbe (`theme.rs`).
- Runde Bilder: Klasse `.avatar` + `overflow(Hidden)` + `halign/valign(Center)` – sonst ist das
  Image-Widget größer als das Bild und die Rundung schneidet nur dessen Ecken.
- CSS-Priorität: App-CSS (`qu_core::CSS`, TubeQu hat kein eigenes mehr) = `APPLICATION + 1`, `theme.rs` = `APPLICATION`. GTK entscheidet
  bei gleicher Priorität nach Ladereihenfolge, nicht nach Spezifität. GTK-Default-Theme gibt
  `scale highlight` einen blauen Rahmen + `margin: -1px` → bei eigenen Reglern `border: none;
  margin: 0;` und eine `min-height` setzen.
- Rundungen über `--radius-s/-m/-l/-pill` (von `theme.rs`); nur ein Omarchy-Theme („Solitude“)
  gibt `rounding` vor, sonst die Standardwerte.

## Produktentscheidungen (so gewollt – nicht ohne Rückfrage ändern)

- Startseite = YouTube-Empfehlungen (Discovery ist wichtig, ein eigener Algorithmus reicht nicht).
- Raster: Spalten nach Breite (Karten ≥ 320 px), **höchstens 4 pro Reihe**, Karten füllen die
  Reihe, Vorschaubild-Höhe folgt der Spaltenbreite (16:9) – `grid.rs` `MAX_COLUMNS`.
  Strg+Mausrad im Raster: hoch = eine Spalte mehr, runter = eine weniger, 3–6 (`WANT`, gilt für alle Raster,
  gespeichert in `~/.config/tubequ/columns`); nur so viele, wie Karten ≥ 200 px passen. Touchpad wird dabei ignoriert.
  **Keine festen Pixelbreiten** in Raster/Karten/„Continue watching“ (gleich breite Plätze statt
  `set_size_request`), Stack nicht homogen: sonst hält eine Mindestbreite das Fenster breit, beim
  Verkleinern rutscht der Inhalt rechts raus und die Messfläche (Overlay über der Seite) sieht die
  neue Breite nie.
- Nach 4 Reihen „Continue watching“ mit **höchstens 3** Videos, ohne Scrollen.
- Fortschritt: < 30 s nicht speichern, ≥ 95 % = fertig, beim Fortsetzen 3 s zurück.
- Springen zum Schlüsselbild (`hr-seek=no`), Timeline springt erst nach 150 ms Stillstand –
  YouTube drosselt Downloads, bildgenaue Sprünge dauerten bis 6 s.
- Player: 5 px Rand (Hintergrundfarbe), Ecken `--radius-window` (= Hyprland-`decoration:rounding` per hyprctl, sonst Theme, sonst 8 px), 85 % der Fensterbreite und mittig,
  unter 1600 px volle Breite (`watch.rs`, `PLAYER_WIDTH`, `FULL_WIDTH_BELOW`); im Vollbild randlos und eckig.
  Strg+Mausrad auf der Ansehen-Seite stellt eine feste Player-Breite ein (`want`, 80 px pro Raste, ab 640 px,
  gespeichert in `~/.config/tubequ/player-width`): kleineres Fenster nimmt nur den Rand weg; ab Fensterbreite randlos und eckig.
- Puffer: mpv lädt bis 1 GiB voraus und hält 512 MiB zurück (`player.rs`, im RAM), damit
  Sprünge im Geladenen sofort laufen. `stream-lavf-o=request_size=10485760`: ffmpeg lädt in
  10-MiB-Stücken (wie yt-dlp), sonst bremst YouTube auf Abspielgeschwindigkeit (mpv ≥ 0.42 macht
  das im ytdl_hook selbst, mpv 0.41 nicht). Vorgeladener Bereich hell in der Timeline
  (`demuxer-cache-time`); Spinner auch beim Puffern/Springen (nach 300 ms, kein Flackern).
- Lautstärke-Angleich: `af=lavfi=[dynaudnorm=f=250:g=15:p=0.95]` (kein Startverzug, getestet). `TUBEQU_NO_NORM=1` schaltet ihn ab (Gegenprobe).
- Kapitel (mpv `chapter-list`, ab 2 Stück): Timeline in Abschnitte mit 1-px-Lücken (`CHAPTER_GAP`); rund nur an Anfang und Ende der ganzen Timeline, an Kapitelgrenzen eckig (`rounded` mit Radius links/rechts). Die Schiene zeichnet dann `segment_layer` selbst (Klasse `chapters` am Regler macht trough/highlight per CSS unsichtbar; `chapter_spans`), der Abschnitt unter der Maus wird dicker; beim Überfahren steht der Kapiteltitel in der Vorschau.
- Timeline-Vorschau: Storyboard aus dem yt-dlp-JSON (`storyboard_from_ytdlp`, größte Stufe),
  Bögen von i.ytimg.com nur beim Überfahren geladen (im RAM, pro Video), nicht über das Limit.
  Die Vorschau liegt als eigene Ebene im Player (nicht in der Leiste) und sitzt immer `PREVIEW_GAP` über der Schiene (`place_preview`).
- Startseite: letzte Liste in `~/.cache/tubequ/home.jsonl`, beim Start sofort gezeigt
  (`Grid::show_saved`), wird ersetzt, sobald die neue da ist; scheitert die, bleibt sie stehen.
- **Im GTK-Thread kein `mpv.get_property` in Ereignissen:** wartet, solange mpv beschäftigt ist
  (gemessen 200 ms) → Oberfläche/Spinner hängen. Werte aus `PropertyChange` mitschreiben.
- Video-Fläche wird beim App-Start eingerichtet (`Player::prepare`, ~120 ms); `play` wartet, bis
  sie bereit ist (sonst „No video output“ + automatischer Zweitversuch = Extra-Anfrage).
- Beim Laden zeigt der Player das Vorschaubild des Videos (`poster`, `grid::load_thumb`, Thumbnail-Cache, kein YouTube-Limit) unter Spinner/Fehlermeldung; weg bei `FileLoaded`. Bei Fehlern bleibt es stehen.
- SponsorBlock überspringt sponsor/self-promotion/interaction automatisch, keine Einstellungen.
- Links in Beschreibung: Zeitstempel springen, YouTube-Videos/Kanäle in der App, Rest im Browser.
- Kanalseite ohne Live-Tab. Abo-Knopf = YouTube-Symbol (Kontur = nicht abonniert, gefüllt = abonniert),
  nur mit Browser-Login sichtbar.
- Kontowahl: Profilbild des aktiven Kontos unten in der Burger-Spalte (Daten aus
  `~/.cache/tubequ/accounts.tsv`, nur beim allerersten Mal 1 Anfrage); Klick klappt die übrigen Konten
  mit Profilbild nach rechts auf (Bubble = liegendes U). Die Liste wird erst beim Aufklappen geladen
  (1 Anfrage pro Konto +1, einmal pro Sitzung). Wechsel leert den Cache und lädt die Startseite neu.
- Abgelehnt bzw. wieder entfernt: eigene Filter, „ähnliche Videos“, Likes.

## Bedienung (Komfort)

- Timeline-Sprung: Die Timeline hält das Ziel (`seek_target`, `seek_state` in qu-core `controls.rs`), bis das Video 1 s darüber hinaus ist. mpv meldet nach dem Sprung erst das Ziel, dann das Schlüsselbild davor (`hr-seek=no`), vorher noch die alte Position – sonst springt der Zeiger Ziel → alt → Ziel bzw. kurz zurück.

- Player: Doppelklick links/rechts = −/+10 s, Mitte = Vollbild; `+`/`-` = Lautstärke (Mausrad bewusst nicht); Einblendung
  mitten im Bild bei Sprung/Lautstärke/Ton/Tempo/Untertitel (nicht in den ersten 1,5 s nach `play`).
  Tasten zusätzlich: `0`–`9` (0–90 %), `Strg+←/→` voriges/nächstes Kapitel (`chapter_target`; ← in den ersten 3 s ins vorige, sonst an den Kapitelanfang; Titel wird eingeblendet; mehrfach schnell drücken springt durch, weil das Ziel des letzten Sprungs als Position gilt, solange mpv am Schlüsselbild davor steht: `chapter_pos`), `,`/`.` Bild für Bild, `c` bzw. Linksklick auf den Untertitel-Knopf = an/aus
  (gewählte Sprache, Standard Englisch; `Controls::toggle_subs`/`pick_sub`), Rechtsklick = Menü mit Sprache (Spuren des
  Videos, beim Öffnen gelesen), Größe und Hintergrund (Umriss/Kasten). Nur de/en, auch automatische; erst beim Einschalten geladen, `F5`/`Strg+R`/`R` Reload, `Strg+V` Link aus Zwischenablage.
- Maus-Seitentasten (GTK-Knopf 8/9) = Zurück/Vor, überall im Fenster (`Nav::forward`, merkt nur die
  Seite, nicht deren Inhalt; neuer Weg oder Home leert „Vor“).
- Geschwindigkeit pro Kanal gemerkt (`speeds.tsv`), sonst normal. Endbildschirm (Nochmal/Startseite).
- Karten: Rechtsklick-Menü (Watch later, Copy link, Open channel, Open in browser – über **http**, damit
  die Link-Weiche es nicht zurück in TubeQu schickt), Mittelklick = Watch later. Fertig Geschautes
  abgedunkelt. Scroll-Position bleibt beim Zurückkommen erhalten.
- Menü per Tastatur: Esc klappt es auf – auf der Startseite (kein Zurück-Verlauf) und beim Video (im Vollbild verlässt Esc zuerst das Vollbild); sonst bleibt Esc „Zurück“. Pfeil hoch/runter in der Spalte (Watch later, History, Konto; Umlauf), rechts in die obere Reihe (Zurück, Home, Suche), dort links/rechts, links am Anfang bzw. runter zurück in die Spalte; Enter/Leertaste drückt, Esc klappt zu (`menu_key` in `main.rs`). Knöpfe nur dann fokussierbar; `set_focus_visible(true)` zeigt die Markierung (style.css `.bubble button:focus-visible`). Offenes Menü hält die Navigation im Player sichtbar (auch wenn die Leiste weg ist).
- Nav-Bubble: Burger, Zurück, Home, Suche, Lade-Kreis; Burger klappt die Bubble nach unten auf (Abos, Watch later, Verlauf, Sign in on TV, Konto), Auswahl oder Klick daneben klappt zu. Kein Reload-Knopf. Klick auf den Knopf der Seite, auf der man ist, lädt sie neu.
- Lade-Kreis in der Bubble: `crate::Busy::new()` in jeden Ladevorgang mitnehmen (Raster, Nachladen, Video-Details, Konten, Abo); Reload lässt das Alte stehen, bis das Neue da ist.
- Ansehen-Seite: Infos unter dem Video erst sichtbar, wenn die Details da sind; bis dahin graue Platzhalter in derselben Form (Titel, Profilbild-Kreis, Kanal, Beschreibung) mit durchlaufendem Glanzstreifen (`watch.rs` `skeleton`, style.css `.skeleton`), nicht bei direkt abgespielten Adressen. Kanal-Profilbilder haben bis zum Laden einen grauen Kreis (`.avatar.placeholder`). Suchfeld zeigt beim Öffnen die letzte Suche (markiert); leer oder unverändert = Liste der letzten Suchen, keine Vorschlags-Anfrage.
- Weiches Scrollen mit dem Mausrad (`smooth.rs`, an jedem ScrolledWindow): Raststufen gleiten statt zu springen; Touchpad bleibt bei GTK.
- Feinschliff (qu-core-CSS, Ende): Vorschaubilder mit grauem Platzhalter (`.thumb-box`), Einblenden beim Laden (`.fade` → `.loaded`), Hover-Zoom 4 %; Ansehen-Seite blendet ein (`page-in-a/-b` im Wechsel, sonst startet die Animation nicht neu), Karte → Player und Player ↔ Miniplayer fliegen (`fly.rs`).
- Ton: Stummschalten blendet über den Filter `@fade` (lavfi `volume`, per `af-command`) in 160 ms aus/ein, `+`/`-` ändern die Lautstärke in kleinen Schritten (`Controls::toggle_mute`/`step_volume`). `af-command` meldet trotzdem einen Fehler (Ziel „all“ trifft auch Filter ohne den Befehl), wirkt aber.
- Tastatur im Raster: Pfeiltasten wählen die erste Karte, danach navigiert GTK; die Liste gleitet zur gewählten Karte (`smooth.rs`, nur bei Tastatur-Fokus). Timeline braucht nichts: mpv meldet `time-pos` jedes Bild.
- Nachladen eine Bildschirmhöhe vor dem Ende (`grid.rs`, `connect_value_changed`), nicht früher (jede Seite = Anfrage).
- Miniplayer: Verlässt man die Ansehen-Seite, während ein Video läuft, spielt es unten rechts klein weiter (400 px; Knöpfe als Bubbles in den Ecken: oben links groß machen, oben rechts schließen, unten links Play + Ton). Tasten im Miniplayer: Leertaste/`k` Pause, `m` Ton, `f` zurück zum großen Video (kein Vollbild), `q` schließen; Pfeile/Ziffern bleiben fürs Raster. Die Ansehen-Seite liegt dafür als Ebene über dem Stack (im Stack nur ein Platzhalter „watch“) und wird nur verkleinert (`WatchFrame::set_mini`, `Nav::set_mini` in qu-core): der Player wird **nicht umgehängt**, sonst baut GTK die GLArea neu und mpv schaltet das Video ab. Am Ende/bei Fehler stoppt das Video (`Player::active`). Die Mini-Knöpfe erscheinen erst nach der Schrumpf-Animation (`Zoom::after`), sonst würden sie mit hochskaliert gezeichnet. Der Miniplayer lässt sich ziehen (`GestureDrag` an der Ansehen-Seite, Capture, ab 8 px; gezeichnet über `Zoom::set_offset`) und gleitet beim Loslassen (Ziehen in qu-core `nav.rs`) mit der Zoom-Animation in die nächste Ecke (`WatchFrame::set_corner`, gilt für die Sitzung). Klick ins Miniplayer-Video pausiert deshalb nicht (qu-core qu-core `player.rs` `in_mini`), Pause über den Knopf.
- MPRIS über `/usr/lib/mpv-mpris/mpris.so` (Paket mpv-mpris, falls vorhanden); Bildschirmschoner-
  Sperre während der Wiedergabe über `gtk::Application::inhibit`. Fenstertitel = Videotitel.

## Gespeicherte Dateien

- `~/.local/share/tubequ/progress.tsv` – Fortschritt (id, pos, dauer, zeit, titel, kanal, kanal-id, veröffentlicht)
- `~/.local/share/tubequ/later.jsonl` – Watch later (neueste zuerst; fertig Geschautes fliegt raus)
- `~/.local/share/tubequ/searches.txt` – letzte 8 Suchen; `speeds.tsv` – Tempo pro Kanal
- `~/.config/tubequ/quality` – gewählte Qualität (Standard 1080)
- `~/.config/tubequ/subtitles` – Untertitel: Sprache, Größe (`sub-font-size`), `box`/`outline` (je eine Zeile)
- `~/.config/tubequ/volume` – Lautstärke 0–200 (Standard 100 = Regler in der Mitte, `MAX_VOLUME`, mpv `volume-max`), bei jeder Änderung gespeichert
- `~/.config/tubequ/player-width` / `columns` – mit Strg+Mausrad gewählte Player-Breite / Spaltenzahl
- `~/.config/tubequ/account` – Nummer des Google-Kontos im Browser (Standard 0)
- `~/.local/share/tubequ/tvs.tsv` – fest gekoppelte Fernseher (IP, Schlüssel); Android: `files/tubequ/pair.key`
- `~/.cache/tubequ/home.jsonl` – letzte Startseite (sofort beim Start)
- `~/.cache/tubequ/accounts.tsv` – bekannte Konten (Nummer, Name, E-Mail, Profilbild-URL)
- Android: `files/tubequ/login.json` – vom Desktop übertragener Login (Cookies, Konto), nur in der App lesbar
- `~/.cache/tubequ/requests.txt` – Zeitpunkte der YouTube-Anfragen der letzten Stunde (Limit)
- `~/.cache/tubequ/crash.log` – Panik-Meldung + Backtrace (Hook in `main`), erste Anlaufstelle bei Abstürzen
- `~/.cache/tubequ/cookies.tmp` – nur kurz während einer innertube-Anfrage, danach gelöscht
- `~/.cache/tubequ/` – rustypipe-Cache, `thumbs/` (auch Avatare/Profilbilder), `icons/`, `rustypipe_reports/`
- Omarchy-Theme gelesen aus `~/.local/state/omarchy/current/theme/` (`colors.toml`, `hyprland.lua`)

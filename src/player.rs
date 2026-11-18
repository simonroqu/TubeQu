//! Player-Anbindung: der Player aus qu-core mit TubeQu-Daten – yt-dlp, Fortschritt pro Video (fertig → aus
//! „Watch later“), Position an YouTube melden, SponsorBlock (automatisch übersprungen), Tempo pro Kanal,
//! zweiter Versuch mit Browser-Login bei Altersbeschränkung.

use crate::progress;
use crate::youtube::Video;
use qu_core::player::{Hooks, Media};
use qu_core::segments::Segment;
use std::future::Future;
use std::pin::Pin;
use std::rc::Rc;
use std::time::Duration;

pub use qu_core::player::toast;
pub type Player = qu_core::player::Player<Video>;

/// yt-dlp-Formatauswahl für eine maximale Bildhöhe.
pub fn format_for(height: u32) -> String {
    format!("bestvideo[height<={height}]+bestaudio/best[height<={height}]/best")
}

/// Untertitel-Optionen für yt-dlp (Teil von `ytdl-raw-options`): auch automatische, aber nur Deutsch/Englisch
/// (sonst ~100 Übersetzungen). Geladen wird eine Spur erst beim Einschalten – keine Extra-Anfrage beim Start.
const SUBS_RAW: &str = "write-auto-subs=,sub-langs=(de|en).*";

/// `loadfile`-Optionen für den zweiten Versuch mit Browser-Login. Der Wert enthält Kommas, darum
/// mpvs Längen-Schreibweise `%n%wert` (sonst trennt mpv die Optionen dort).
fn with_login(opts: &str, login: &str) -> String {
    let raw = format!("{SUBS_RAW},{login}");
    let raw = format!("ytdl-raw-options=%{}%{raw}", raw.len());
    if opts.is_empty() { raw } else { format!("{opts},{raw}") }
}

struct TubeHooks;

impl Hooks<Video> for TubeHooks {
    fn init(&self, i: &libmpv2::MpvInitializer) -> libmpv2::Result<()> {
        i.set_property("ytdl", true)?;
        i.set_property("ytdl-format", format_for(qu_core::controls::saved_quality()))?;
        // YouTube bremst große Bereichs-Anfragen auf etwa Abspielgeschwindigkeit; viele kleine Anfragen
        // (10 MiB, wie yt-dlp selbst lädt) laufen mit voller Geschwindigkeit. Erst dadurch füllt sich
        // der Puffer schneller als abgespielt wird. ffmpeg ≥ 9 (`request_size`); neueres mpv setzt das
        // im ytdl_hook selbst (mpv-PR #17471), mpv 0.41 noch nicht. Unbekannt = still ignoriert.
        i.set_property("stream-lavf-o", "request_size=10485760")?;
        // Lautstärke angleichen (YouTube normalisiert, mpv nicht): sanft, großes Zeitfenster gegen
        // „Pumpen“. Kein spürbarer Startverzug (getestet).
        // ponytail: TUBEQU_NO_NORM=1 schaltet den Angleich ab (Gegenprobe bei Lautstärke-Spitzen).
        let norm = if std::env::var_os("TUBEQU_NO_NORM").is_some() { "" } else { "lavfi=[dynaudnorm=f=250:g=15:p=0.95]," };
        i.set_property("af", format!("{norm}@fade:lavfi=[volume=1]"))?;
        // Wert ohne Komma: mpv trennt die Optionen an Kommas.
        // ponytail: player_client=visionos – seit yt-dlp 2026.10.26 liefern die Standard-Clients
        // (tv_samsung) Stream-URLs, die mpv mit 403 abgewiesen werden. visionos braucht keinen
        // PO-Token und liefert HLS. Wenn auch das kippt: Client-Liste in yt-dlp `_DEFAULT_CLIENTS` prüfen.
        i.set_property("ytdl-raw-options", format!("{SUBS_RAW},extractor-args=youtube:player_client=visionos"))
    }

    fn save(&self, v: &Video, pos: f64, dur: f64) {
        progress::record(v, pos, dur);
        // Fertig geschaut: aus „Später ansehen“ nehmen.
        if progress::get(&v.id).is_some_and(|e| e.finished()) {
            crate::lists::remove_later(&v.id);
        }
    }

    /// Position an YouTube melden (Pause, Videowechsel, Stopp) – nicht alle 5 s wie das lokale Speichern.
    /// `wait`: blockiert bis zu 3 s, damit die Meldung beim Schließen der App noch rausgeht.
    fn report(&self, v: &Video, pos: f64, wait: bool) {
        let id = v.id.clone();
        let fut = async move {
            if let Err(e) = crate::youtube::report_position(id, pos).await {
                eprintln!("Position an YouTube melden fehlgeschlagen: {e}");
            }
        };
        if wait {
            qu_core::player::wait_at_most(Duration::from_secs(3), fut);
        } else {
            crate::RT.spawn(fut);
        }
    }

    /// SponsorBlock-Abschnitte (fragt sponsor.ajay.app, nicht YouTube).
    fn segments(&self, v: &Video) -> Option<Pin<Box<dyn Future<Output = Vec<Segment>> + Send>>> {
        Some(Box::pin(crate::sponsorblock::segments(v.id.clone())))
    }

    fn speed_changed(&self, v: &Video, speed: f64) {
        if let Some(c) = &v.channel_id {
            crate::lists::set_channel_speed(c, speed);
        }
    }

    /// Altersbeschränkt: der eine zweite Versuch läuft mit Browser-Login.
    fn retry_opts(&self, log: &[String], opts: &str) -> Option<String> {
        let login = log.iter().any(|l| l.contains("confirm your age")).then(crate::youtube::login_ytdl_options).flatten()?;
        Some(with_login(opts, &login))
    }
}

pub fn new() -> Player {
    let p = Player::new(TubeHooks);
    // Qualität geändert: yt-dlp wählt das Format, gilt erst nach neuem Laden (an derselben Stelle weiter).
    let mpv = p.mpv;
    p.controls.connect_quality(move |height| {
        let _ = mpv.set_property("ytdl-format", format_for(height));
        if let Ok(path) = mpv.get_property::<String>("path") {
            let pos = mpv.get_property::<f64>("time-pos").unwrap_or(0.0);
            let live = mpv.get_property::<f64>("duration").is_err();
            let opts = if live { String::new() } else { format!("start={pos:.1}") };
            let _ = mpv.command("loadfile", &[&path, "replace", "-1", &opts]);
        }
    });
    p
}

/// Video abspielen (über yt-dlp): ist es bekannt, wird dort fortgesetzt, wo man aufgehört hat; Tempo des
/// Kanals; bis es lädt, das Vorschaubild aus dem Thumbnail-Cache (keine YouTube-Anfrage).
pub fn play(player: &Player, video: Option<Video>, url: &str, live: bool) {
    let start = video.as_ref().filter(|_| !live).and_then(|v| progress::resume_at(&v.id));
    let speed = video.as_ref().and_then(|v| v.channel_id.as_deref()).map_or(1.0, crate::lists::channel_speed);
    let id = video.as_ref().map(|v| v.id.clone());
    player.play(Media { start, speed, live, poster: video.is_some(), ..Media::link(video, url) });
    // ponytail: kein ID-Abgleich – kommt das Bild eines vorigen Videos verspätet an, steht es kurz da.
    // Unkritisch, die Bilder liegen meist schon im Cache (Raster).
    if let Some(id) = id {
        crate::grid::load_thumb(&player.poster, id);
    }
}

/// Aktuelles Video neu laden (Reload-Icon unter Fehlern oder in der Navigation). Holt frische Stream-Adressen
/// und zählt daher als Anfrage; setzt an der zuletzt gespeicherten Stelle fort.
pub fn reload(player: &Rc<Player>) {
    let Some(m) = player.current() else { return };
    let Some(url) = m.variants.first().map(|v| v.1.clone()) else { return };
    player.loading();
    let player = player.clone();
    gtk4::glib::spawn_future_local(async move {
        let res = crate::bg(crate::youtube::throttle()).await;
        // Inzwischen ein anderes Video gestartet: nichts tun.
        if player.current().is_some() {
            return;
        }
        match res {
            Ok(()) => play(&player, m.item, &url, m.live),
            Err(e) => player.show_error(&e),
        }
    });
}

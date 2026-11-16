//! Eingebetteter Player: libmpv zeichnet per OpenGL in eine GtkGLArea.

use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use crate::controls::Controls;
use crate::progress;
use libmpv2::events::PropertyData;
use std::time::{Duration, Instant};
use gtk::gdk;
use libmpv2::events::Event;
use libmpv2::{Format, Mpv};
use libmpv2::render::{OpenGLInitParams, RenderContext, RenderParam, RenderParamApiType};
use std::cell::{Cell, RefCell};
use std::ffi::{CString, c_char, c_void};
use std::rc::Rc;
use std::sync::LazyLock;

pub struct Player {
    pub mpv: &'static Mpv,
    /// Video plus Meldungsfläche darüber; das kommt ins Layout.
    pub widget: gtk::Overlay,
    pub controls: Rc<Controls>,
    area: gtk::GLArea,
    msg: gtk::Label,
    /// Reload-Icon unter einer Fehlermeldung.
    pub retry: gtk::Button,
    /// Endbildschirm: „Zur Startseite“ (verbunden in main.rs).
    pub end_home: gtk::Button,
    /// Kurze Einblendung mitten im Bild („+10 s“, „65 %“, „1.5×“ …).
    flash: gtk::Label,
    /// Wird mit `true` aufgerufen, solange ein Video läuft (Bildschirmschoner aus), sonst `false`.
    on_playing: Rc<RefCell<Option<Box<dyn Fn(bool)>>>>,
    spinner: gtk::Image,
    /// Vorschaubild des Videos, solange es lädt (aus dem Thumbnail-Cache, keine YouTube-Anfrage).
    poster: gtk::Picture,
    /// Wird mit dem Seitenverhältnis (Breite/Höhe) aufgerufen, sobald das Video es kennt.
    on_aspect: Rc<RefCell<Option<Box<dyn Fn(f64)>>>>,
    /// Bekommt das yt-dlp-JSON des Videos (holt mpv beim Laden ohnehin), z. B. als Ersatz-Details.
    on_info: Rc<RefCell<Option<Box<dyn Fn(&str)>>>>,
    track: Rc<RefCell<Track>>,
    /// Video-Fläche bereit (mpv-Render-Kontext angelegt)? Vorher scheitert `loadfile` mit „No video
    /// output“; das Video wartet dann in `pending`, bis die Fläche da ist.
    gl_ready: Rc<Cell<bool>>,
    pending: Rc<RefCell<Option<(String, String)>>>,
}

/// Was gerade läuft, um den Fortschritt zu speichern.
#[derive(Default)]
struct Track {
    video: Option<crate::youtube::Video>,
    live: bool,
    last_save: Option<Instant>,
    /// SponsorBlock-Abschnitte und welche davon schon übersprungen wurden (nur einmal springen).
    segments: Vec<crate::sponsorblock::Segment>,
    skipped: Vec<usize>,
    /// Was zuletzt geladen wurde (URL, Optionen), für einen zweiten Versuch bei Fehlern.
    load: Option<(String, String)>,
    retried: bool,
    /// Wann `play` lief: kurz danach keine Einblendungen (Geschwindigkeit/Lautstärke setzt `play` selbst).
    started: Option<Instant>,
}

/// Untertitel-Optionen für yt-dlp (Teil von `ytdl-raw-options`).
const SUBS_RAW: &str = "write-auto-subs=,sub-langs=(de|en).*";

/// `loadfile`-Optionen für den zweiten Versuch mit Browser-Login. Der Wert enthält Kommas, darum
/// mpvs Längen-Schreibweise `%n%wert` (sonst trennt mpv die Optionen dort).
fn with_login(opts: &str, login: &str) -> String {
    let raw = format!("{SUBS_RAW},{login}");
    let raw = format!("ytdl-raw-options=%{}%{raw}", raw.len());
    if opts.is_empty() { raw } else { format!("{opts},{raw}") }
}

/// Kurzer Hinweis oben links über dem Video, verschwindet nach ein paar Sekunden von selbst.
pub fn toast(label: &gtk::Label, text: &str) {
    label.set_text(text);
    label.set_visible(true);
    let (label, text) = (label.downgrade(), text.to_string());
    glib::timeout_add_local_once(Duration::from_millis(2500), move || {
        // Nur ausblenden, wenn inzwischen kein neuer Hinweis kam.
        if let Some(l) = label.upgrade().filter(|l| l.text() == text) {
            l.set_visible(false);
        }
    });
}

/// Wie oft während des Abspielens gespeichert wird (zusätzlich bei Pause, Stopp, Schließen).
const SAVE_EVERY: Duration = Duration::from_secs(5);

fn save(mpv: &Mpv, track: &RefCell<Track>) {
    let mut t = track.borrow_mut();
    let Some(v) = t.video.clone().filter(|_| !t.live) else { return };
    if let (Ok(pos), Ok(dur)) = (mpv.get_property::<f64>("time-pos"), mpv.get_property::<f64>("duration")) {
        progress::record(&v, pos, dur);
        // Fertig geschaut: aus „Später ansehen“ nehmen.
        if progress::get(&v.id).is_some_and(|e| e.finished()) {
            crate::lists::remove_later(&v.id);
        }
        t.last_save = Some(Instant::now());
    }
}

/// Position an YouTube melden (Pause, Videowechsel, Stopp) – nicht alle 5 s wie das lokale Speichern.
/// `wait`: blockiert bis zu 3 s, damit die Meldung beim Schließen der App noch rausgeht.
fn report(mpv: &Mpv, track: &RefCell<Track>, wait: bool) {
    let t = track.borrow();
    let Some(id) = t.video.as_ref().filter(|_| !t.live).map(|v| v.id.clone()) else { return };
    let Ok(pos) = mpv.get_property::<f64>("time-pos") else { return };
    let fut = async move {
        if let Err(e) = crate::youtube::report_position(id, pos).await {
            eprintln!("Position an YouTube melden fehlgeschlagen: {e}");
        }
    };
    if wait {
        wait_at_most(Duration::from_secs(3), fut);
    } else {
        crate::RT.spawn(fut);
    }
}

/// Blockiert, bis `fut` fertig ist, höchstens `limit`. Der Timer muss *im* `block_on` entstehen:
/// `timeout()` braucht schon beim Anlegen die Tokio-Runtime, im GTK-Thread gibt es keine (Panik
/// „no reactor running“ → Absturz beim Schließen/Zurück).
fn wait_at_most(limit: Duration, fut: impl std::future::Future<Output = ()>) {
    let _ = crate::RT.block_on(async move { tokio::time::timeout(limit, fut).await });
}

/// Zeigt eine Meldung über dem Video (rot, wenn `error`); `retry` = darunter Reload-Icon anbieten.
fn show(msg: &gtk::Label, retry: &gtk::Button, text: &str, error: bool) {
    msg.set_text(text);
    if error { msg.add_css_class("error") } else { msg.remove_css_class("error") }
    msg.set_visible(true);
    retry.set_visible(error);
}

/// Macht aus den Fehlerzeilen von mpv/yt-dlp eine lesbare Ursache.
/// Aus "ERROR: [youtube] <id>: This video is unavailable" wird "This video is unavailable".
fn reason(log: &[String], fallback: &str) -> String {
    let Some(line) = log.iter().find(|l| l.contains("ERROR:")).or(log.first()) else {
        return fallback.to_string();
    };
    let s = line.split_once("ERROR: ").map_or(line.as_str(), |(_, r)| r);
    if s.starts_with('[')
        && let Some((_, rest)) = s.split_once("] ")
    {
        if let Some((id, msg)) = rest.split_once(": ")
            && !id.contains(' ')
        {
            return msg.to_string();
        }
        return rest.to_string();
    }
    s.to_string()
}

/// MPRIS-Plugin aus dem Paket mpv-mpris (Medientasten, Anzeige in Waybar & Co.).
const MPRIS_PLUGIN: &str = "/usr/lib/mpv-mpris/mpris.so";

// GTK4 nutzt unter Wayland (und standardmäßig auch unter X11) EGL.
// ponytail: nur eglGetProcAddress; Mesa und NVIDIA liefern darüber auch Core-GL-Funktionen.
static EGL: LazyLock<libloading::Library> =
    LazyLock::new(|| unsafe { libloading::Library::new("libEGL.so.1").expect("libEGL.so.1 fehlt") });

fn get_proc_address(_: &(), name: &str) -> *mut c_void {
    let name = CString::new(name).unwrap();
    unsafe {
        let f: libloading::Symbol<unsafe extern "C" fn(*const c_char) -> *mut c_void> =
            EGL.get(b"eglGetProcAddress\0").expect("eglGetProcAddress");
        f(name.as_ptr())
    }
}

fn current_fbo() -> i32 {
    const GL_FRAMEBUFFER_BINDING: u32 = 0x8CA6;
    let ptr = get_proc_address(&(), "glGetIntegerv");
    let get: unsafe extern "C" fn(u32, *mut i32) = unsafe { std::mem::transmute(ptr) };
    let mut fbo = 0;
    unsafe { get(GL_FRAMEBUFFER_BINDING, &mut fbo) };
    fbo
}

impl Player {
    pub fn new() -> Self {
        // ponytail: ein Mpv für die ganze Laufzeit, bewusst geleakt, damit der RenderContext ihn 'static leihen kann.
        let mut mpv = Mpv::with_initializer(|i| {
                i.set_property("vo", "libmpv")?;
                i.set_property("ytdl", true)?;
                i.set_property("ytdl-format", crate::controls::format_for(crate::controls::saved_quality()))?;
                i.set_property("hwdec", "auto-safe")?;
                // Lauter als normal erlaubt (bis 200 %, mpv-Standard 130 %).
                i.set_property("volume-max", crate::controls::MAX_VOLUME)?;
                i.set_property("volume", crate::controls::saved_volume())?;
                i.set_property("keep-open", true)?;
                // Sprünge zum nächsten Schlüsselbild statt bildgenau: YouTube drosselt Downloads auf etwa
                // Abspielgeschwindigkeit, bildgenaue Sprünge müssten erst Sekunden davor laden (gemessen:
                // bis 6 s statt meist unter 1 s). Ziel liegt dadurch meist 1–2 s neben der Stelle.
                i.set_property("hr-seek", "no")?;
                // Puffer: so weit vorladen wie möglich, damit Sprünge nach vorn sofort laufen; Zurückspringen
                // bleibt im schon Geladenen. mpv-Standard wäre 150 MiB vor / 50 MiB zurück.
                // ponytail: Puffer im RAM, 1080p ≈ 2 GB/h – bei langen Videos `cache-on-disk` erwägen.
                i.set_property("cache", "yes")?;
                i.set_property("cache-secs", 36000)?;
                i.set_property("demuxer-max-bytes", "1GiB")?;
                i.set_property("demuxer-max-back-bytes", "512MiB")?;
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
                // Untertitel: auch automatische, aber nur Deutsch/Englisch (sonst ~100 Übersetzungen).
                // Geladen wird eine Spur erst beim Einschalten (Taste c) – keine Extra-Anfrage beim Start.
                // Wert ohne Komma: mpv trennt die Optionen an Kommas.
                // ponytail: player_client=visionos – seit yt-dlp 2026.10.26 liefern die Standard-Clients
                // (tv_samsung) Stream-URLs, die mpv mit 403 abgewiesen werden. visionos braucht keinen
                // PO-Token und liefert HLS. Wenn auch das kippt: Client-Liste in yt-dlp `_DEFAULT_CLIENTS` prüfen.
                i.set_property("ytdl-raw-options", format!("{SUBS_RAW},extractor-args=youtube:player_client=visionos"))?;
                // Medientasten und Anzeige in der Leiste (MPRIS), falls das mpv-mpris-Plugin installiert ist.
                if std::path::Path::new(MPRIS_PLUGIN).exists() {
                    i.set_property("scripts", MPRIS_PLUGIN)?;
                }
                Ok(())
            })
            .expect("mpv failed to start");
        // mpv meldet neue Ereignisse aus einem eigenen Thread; ausgewertet wird im GTK-Thread.
        let (ev_tx, ev_rx) = async_channel::unbounded::<()>();
        mpv.set_wakeup_callback(move || {
            let _ = ev_tx.try_send(());
        });
        // Fehlerzeilen (z. B. von yt-dlp) als Ereignisse bekommen, um die Ursache anzeigen zu können.
        unsafe { libmpv2_sys::mpv_request_log_messages(mpv.ctx.as_ptr(), c"error".as_ptr()) };
        let mpv: &'static Mpv = Box::leak(Box::new(mpv));
        for (name, format) in [
            ("time-pos", Format::Double),
            ("duration", Format::Double),
            ("pause", Format::Flag),
            ("volume", Format::Double),
            ("mute", Format::Flag),
            ("speed", Format::Double),
            ("eof-reached", Format::Flag),
            ("current-tracks/sub/lang", Format::String),
            ("dwidth", Format::Int64),
            ("dheight", Format::Int64),
            ("demuxer-cache-time", Format::Double),
            ("paused-for-cache", Format::Flag),
            ("seeking", Format::Flag),
            // Vom ytdl_hook gesetzt, solange das Video läuft (mpv ≥ 0.37).
            ("user-data/mpv/ytdl/json-subprocess-result/stdout", Format::String),
        ] {
            let _ = mpv.observe_property(name, format, 0);
        }

        let area = gtk::GLArea::new();
        area.set_hexpand(true);
        // Höhe bestimmt die Ansehen-Seite passend zum Video; nicht von selbst wachsen.
        area.set_focusable(true);

        let ctx: Rc<RefCell<Option<RenderContext<'static>>>> = Rc::default();
        let gl_ready: Rc<Cell<bool>> = Rc::default();
        let pending: Rc<RefCell<Option<(String, String)>>> = Rc::default();

        area.connect_realize({
            let (ctx, gl_ready, pending) = (ctx.clone(), gl_ready.clone(), pending.clone());
            move |a| {
                a.make_current();
                if let Some(e) = a.error() {
                    eprintln!("OpenGL nicht verfügbar: {e}");
                    return;
                }
                let mut rc = mpv
                    .create_render_context(vec![
                        RenderParam::ApiType(RenderParamApiType::OpenGl),
                        RenderParam::InitParams(OpenGLInitParams { get_proc_address, ctx: () }),
                    ])
                    .expect("mpv render context");
                // Der Callback kommt aus einem mpv-Thread; neu gezeichnet wird im GTK-Thread.
                let (tx, rx) = async_channel::unbounded::<()>();
                rc.set_update_callback(move || {
                    let _ = tx.try_send(());
                });
                let weak = a.downgrade();
                glib::spawn_future_local(async move {
                    while rx.recv().await.is_ok() {
                        match weak.upgrade() {
                            Some(a) => a.queue_render(),
                            None => break,
                        }
                    }
                });
                *ctx.borrow_mut() = Some(rc);
                gl_ready.set(true);
                if let Some((url, opts)) = pending.take() {
                    let _ = mpv.command("loadfile", &[&url, "replace", "-1", &opts]);
                }
            }
        });

        area.connect_unrealize({
            let (ctx, gl_ready) = (ctx.clone(), gl_ready.clone());
            move |a| {
                a.make_current();
                gl_ready.set(false);
                ctx.borrow_mut().take();
            }
        });

        area.connect_render(move |a, _| {
            if let Some(rc) = ctx.borrow().as_ref() {
                let s = a.scale_factor();
                let _ = rc.render::<()>(current_fbo(), a.width() * s, a.height() * s, true);
            }
            glib::Propagation::Stop
        });

        let widget = gtk::Overlay::new();
        // Abrundung (CSS-Klasse `rounded`) schneidet auch das Video ab.
        widget.set_overflow(gtk::Overflow::Hidden);
        let controls = Controls::new(mpv, &widget);
        let track: Rc<RefCell<Track>> = Rc::default();

        // Klick aufs Video: Pause umschalten. Doppelklick: Vollbild
        // (der erste Klick des Doppelklicks pausiert, der zweite hebt das wieder auf).
        // Kurze Einblendung mitten im Bild.
        let flash = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .visible(false)
            .can_target(false)
            .css_classes(["osd", "pill", "flash"])
            .build();

        // Doppelklick: im linken/rechten Drittel 10 s zurück/vor (wie in der YouTube-App), in der Mitte Vollbild.
        let click = gtk::GestureClick::new();
        click.connect_pressed({
            let (controls, flash) = (controls.clone(), flash.clone());
            move |g, n, x, _| {
                // Im Miniplayer: Ziehen statt Pause (Pause über den Knopf im Miniplayer).
                if g.widget().is_some_and(|w| in_mini(&w)) {
                    return;
                }
                let _ = mpv.command("cycle", &["pause"]);
                if n == 2 {
                    let w = g.widget().map_or(1, |w| w.width()) as f64;
                    let third = if x < w / 3.0 { -1 } else if x > w * 2.0 / 3.0 { 1 } else { 0 };
                    if third == 0 {
                        controls.fullscreen.emit_clicked();
                    } else {
                        let _ = mpv.command("seek", &[&(10 * third).to_string()]);
                        toast(&flash, if third < 0 { "−10 s" } else { "+10 s" });
                    }
                }
            }
        });
        area.add_controller(click);
        controls.subtitles.connect_clicked({
            let (controls, flash) = (controls.clone(), flash.clone());
            move |_| {
                if !controls.toggle_subs() {
                    toast(&flash, "No subtitles");
                }
            }
        });

        // Mausbewegung über dem Video blendet die Leiste ein.
        let motion = gtk::EventControllerMotion::new();
        // Nur echte Bewegung zählt: GTK schickt nach Änderungen im Layout (Zeit, Timeline, Ein-/Ausblenden)
        // Bewegungs-Ereignisse an derselben Stelle nach, die Leiste bliebe sonst für immer sichtbar.
        let last = std::cell::Cell::new((f64::NAN, f64::NAN));
        motion.connect_motion({
            let controls = controls.clone();
            move |_, x, y| {
                let (lx, ly) = last.replace((x, y));
                if (x - lx).abs() >= 1.0 || (y - ly).abs() >= 1.0 || lx.is_nan() {
                    controls.poke();
                }
            }
        });
        // Maus verlässt den Player: Leiste sofort weg.
        motion.connect_leave({
            let controls = controls.clone();
            move |_| controls.hide()
        });
        widget.add_controller(motion);

        let msg = gtk::Label::builder()
            .wrap(true)
            .justify(gtk::Justification::Center)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .margin_start(24)
            .margin_end(24)
            .visible(false)
            .css_classes(["osd"])
            .build();
        let retry = gtk::Button::builder()
            .icon_name("tubequ-reload-symbolic")
            .tooltip_text("Reload")
            .halign(gtk::Align::Center)
            .focusable(false)
            .visible(false)
            .css_classes(["flat", "osd"])
            .cursor(&gdk::Cursor::from_name("pointer", None).unwrap())
            .build();
        let msg_box = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .build();
        msg_box.append(&msg);
        msg_box.append(&retry);
        widget.set_child(Some(&area));
        // Vorschaubild unter Meldung/Spinner, bis das Video geladen ist (`play` zeigt es, `FileLoaded` blendet es aus).
        let poster = gtk::Picture::builder().content_fit(gtk::ContentFit::Cover).can_target(false).visible(false).build();
        widget.add_overlay(&poster);
        widget.add_overlay(&msg_box);
        // Laden: nur ein Spinner, kein Text.
        let spinner = crate::icons::spinner(32).halign(gtk::Align::Center).valign(gtk::Align::Center).visible(false).build();
        widget.add_overlay(&spinner);
        let on_aspect: Rc<RefCell<Option<Box<dyn Fn(f64)>>>> = Rc::default();
        let on_info: Rc<RefCell<Option<Box<dyn Fn(&str)>>>> = Rc::default();
        let note = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .margin_start(16)
            .margin_top(16)
            .visible(false)
            .css_classes(["osd", "pill"])
            .build();
        widget.add_overlay(&note);
        widget.add_overlay(&flash);

        // Endbildschirm: Nochmal / Zur Startseite (nur Icons).
        let end_again = gtk::Button::builder().icon_name("tubequ-reload-symbolic").tooltip_text("Watch again").focusable(false).build();
        let end_home = gtk::Button::builder().icon_name("tubequ-home-symbolic").tooltip_text("Home").focusable(false).build();
        let end_box = gtk::Box::builder()
            .spacing(2)
            .halign(gtk::Align::Center)
            .valign(gtk::Align::Center)
            .visible(false)
            .css_classes(["bubble", "end-screen"])
            .build();
        end_box.append(&end_again);
        end_box.append(&end_home);
        end_again.connect_clicked(move |_| {
            let _ = mpv.command("seek", &["0", "absolute"]);
            let _ = mpv.set_property("pause", false);
        });
        widget.add_overlay(&end_box);
        widget.add_overlay(&controls.widget);
        widget.add_overlay(&controls.preview);
        let on_playing: Rc<RefCell<Option<Box<dyn Fn(bool)>>>> = Rc::default();

        glib::spawn_future_local({
            let msg = msg.clone();
            let retry = retry.clone();
            let spinner = spinner.clone();
            let poster = poster.clone();
            let on_aspect = on_aspect.clone();
            let on_info = on_info.clone();
            let on_playing = on_playing.clone();
            let (flash, end_box) = (flash.clone(), end_box.clone());
            let controls = controls.clone();
            let track = track.clone();
            async move {
                let mut log: Vec<String> = Vec::new();
                // Erst nach dem ersten Laden zeigt der Spinner Puffern/Springen; davor regeln Start/Geladen ihn.
                let mut loaded = false;
                // Zustände aus den Ereignissen mitschreiben statt mpv zu fragen: `get_property` wartet, solange
                // mpv beschäftigt ist (gemessen bis 200 ms), und hält so die Oberfläche samt Spinner an.
                let waiting: Rc<Cell<(bool, bool)>> = Rc::default(); // (Puffer leer, springt)
                let (mut dw, mut dh) = (0i64, 0i64);
                let mut sub_lang: Option<String> = None;
                while ev_rx.recv().await.is_ok() {
                    while let Some(ev) = mpv.wait_event(0.0) {
                        match ev {
                            Ok(Event::StartFile) => {
                                log.clear();
                                loaded = false;
                                end_box.set_visible(false);
                                msg.set_visible(false);
                                retry.set_visible(false);
                                spinner.set_visible(true);
                            }
                            Ok(Event::FileLoaded) => {
                                loaded = true;
                                spinner.set_visible(false);
                                poster.set_visible(false);
                                controls.load_chapters();
                            }
                            Ok(Event::LogMessage { text, .. }) => log.push(text.trim().to_string()),
                            Ok(Event::PropertyChange { name, change, .. }) => {
                                // Puffer leer oder Sprung: Spinner zeigen, aber erst nach kurzer Zeit, damit
                                // Sprünge innerhalb des Vorgeladenen nicht flackern.
                                if let ("paused-for-cache" | "seeking", PropertyData::Flag(on)) = (name, &change) {
                                    let (mut buffering, mut seeking) = waiting.get();
                                    if name == "seeking" { seeking = *on } else { buffering = *on }
                                    waiting.set((buffering, seeking));
                                }
                                if loaded && (name == "paused-for-cache" || name == "seeking") {
                                    let is_waiting = |w: &Cell<(bool, bool)>| w.get().0 || w.get().1;
                                    if is_waiting(&waiting) {
                                        let (sp, w) = (spinner.downgrade(), waiting.clone());
                                        glib::timeout_add_local_once(Duration::from_millis(300), move || {
                                            if let Some(sp) = sp.upgrade().filter(|_| is_waiting(&w)) {
                                                sp.set_visible(true);
                                            }
                                        });
                                    } else {
                                        spinner.set_visible(false);
                                    }
                                }
                                match (name, &change) {
                                    (n, PropertyData::Str(json)) if n.ends_with("json-subprocess-result/stdout") => {
                                        controls.set_storyboard(crate::youtube::storyboard_from_ytdlp(json));
                                        if let Some(f) = on_info.borrow().as_ref() {
                                            f(json);
                                        }
                                    }
                                    ("dwidth", PropertyData::Int64(v)) => dw = *v,
                                    ("dheight", PropertyData::Int64(v)) => dh = *v,
                                    _ => {}
                                }
                                if (name == "dwidth" || name == "dheight")
                                    && dw > 0
                                    && dh > 0
                                    && let Some(f) = on_aspect.borrow().as_ref()
                                {
                                    f(dw as f64 / dh as f64);
                                }
                                let due = track.borrow().last_save.is_none_or(|t| t.elapsed() >= SAVE_EVERY);
                                let paused = name == "pause" && matches!(change, PropertyData::Flag(true));
                                if (name == "time-pos" && due) || paused {
                                    save(mpv, &track);
                                }
                                if paused {
                                    report(mpv, &track, false);
                                }
                                // SponsorBlock: in einem markierten Abschnitt ans Ende springen.
                                if let (true, PropertyData::Double(pos)) = (name == "time-pos", &change) {
                                    let mut t = track.borrow_mut();
                                    if let Some(i) = crate::sponsorblock::to_skip(&t.segments, &t.skipped, *pos) {
                                        t.skipped.push(i);
                                        let s = t.segments[i].clone();
                                        let _ = mpv.command("seek", &[&s.end.to_string(), "absolute"]);
                                        toast(&note, &format!("Skipped {}", s.label));
                                    }
                                }
                                // Einblendungen bei Änderungen durch den Nutzer (nicht direkt nach dem Start).
                                let quiet = track.borrow().started.is_none_or(|t| t.elapsed() < Duration::from_millis(1500));
                                match (name, &change) {
                                    ("volume", PropertyData::Double(v)) if loaded && !quiet => toast(&flash, &format!("{v:.0} %")),
                                    ("mute", PropertyData::Flag(m)) if loaded && !quiet => toast(&flash, if *m { "Muted" } else { "Sound on" }),
                                    ("speed", PropertyData::Double(s)) => {
                                        if loaded && !quiet {
                                            toast(&flash, &crate::controls::speed_label(*s));
                                        }
                                        // Geschwindigkeit pro Kanal merken (setzt `play` beim nächsten Video wieder).
                                        if let Some(c) = track.borrow().video.as_ref().and_then(|v| v.channel_id.clone()).filter(|_| !quiet) {
                                            crate::lists::set_channel_speed(&c, *s);
                                        }
                                    }
                                    ("pause", PropertyData::Flag(p)) => {
                                        if let Some(f) = on_playing.borrow().as_ref() {
                                            f(!p);
                                        }
                                    }
                                    ("eof-reached", PropertyData::Flag(e)) => end_box.set_visible(*e && loaded),
                                    ("current-tracks/sub/lang", PropertyData::Str(l)) => {
                                        if loaded {
                                            toast(&flash, &format!("Subtitles: {l}"));
                                        }
                                        sub_lang = Some(l.to_string());
                                    }
                                    ("current-tracks/sub/lang", PropertyData::None) => {
                                        if loaded && sub_lang.take().is_some() {
                                            toast(&flash, "Subtitles off");
                                        }
                                    }
                                    _ => {}
                                }
                                controls.update(name, change);
                            }
                            // Ein Fehler-Ereignis ist hier das Ende einer Datei mit Fehler.
                            Err(e) => {
                                eprintln!("Video-Fehler: {log:?}");
                                // YouTube verweigert gelegentlich einzelne Stream-Server kurzzeitig.
                                // Einmal neu laden holt frische Adressen; erst dann den Fehler zeigen.
                                // Altersbeschränkt: der eine Versuch läuft stattdessen mit Browser-Login.
                                let login = log
                                    .iter()
                                    .any(|l| l.contains("confirm your age"))
                                    .then(crate::youtube::login_ytdl_options)
                                    .flatten();
                                let again = {
                                    let mut t = track.borrow_mut();
                                    let again = (!t.retried).then(|| t.load.clone()).flatten().map(|(url, opts)| match &login {
                                        Some(l) => (url, with_login(&opts, l)),
                                        None => (url, opts),
                                    });
                                    t.retried = true;
                                    again
                                };
                                if let Some((url, opts)) = again {
                                    let _ = mpv.command("loadfile", &[&url, "replace", "-1", &opts]);
                                } else {
                                    let why = reason(&log, &e.to_string());
                                    spinner.set_visible(false);
                                    show(&msg, &retry, &format!("Couldn't load video:\n{why}"), true);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        Player { mpv, widget, controls, area, msg, retry, end_home, flash, on_playing, spinner, poster, on_aspect, on_info, track, gl_ready, pending }
    }

    /// Video-Fläche schon beim App-Start einrichten (OpenGL + mpv-Render-Kontext, gemessen ~120 ms im
    /// GTK-Thread), nicht erst beim ersten Video-Klick. Geht erst, wenn das Fenster da ist.
    pub fn prepare(&self) {
        if !self.area.is_realized() {
            self.area.realize();
        }
    }

    pub fn connect_playing(&self, f: impl Fn(bool) + 'static) {
        self.on_playing.replace(Some(Box::new(f)));
    }

    pub fn connect_info(&self, f: impl Fn(&str) + 'static) {
        self.on_info.replace(Some(Box::new(f)));
    }

    pub fn connect_aspect(&self, f: impl Fn(f64) + 'static) {
        self.on_aspect.replace(Some(Box::new(f)));
    }

    /// Fehler ohne geladenes Video (z. B. Anfragen-Limit): ohne Reload-Icon, das ginge ja auch nicht.
    pub fn show_error(&self, text: &str) {
        self.spinner.set_visible(false);
        show(&self.msg, &self.retry, text, true);
        self.retry.set_visible(false);
    }

    /// Aktuelles Video neu laden (Reload-Icon unter Fehlern oder in der Navigation). Holt frische Stream-Adressen
    /// und zählt daher als Anfrage; setzt an der zuletzt gespeicherten Stelle fort.
    pub fn reload(self: &Rc<Self>) {
        let (video, live, url) = {
            let t = self.track.borrow();
            let Some((url, _)) = t.load.clone() else { return };
            (t.video.clone(), t.live, url)
        };
        self.msg.set_visible(false);
        self.retry.set_visible(false);
        self.spinner.set_visible(true);
        let me = self.clone();
        glib::spawn_future_local(async move {
            let res = crate::bg(crate::youtube::throttle()).await;
            // Inzwischen ein anderes Video gestartet: nichts tun.
            if me.track.borrow().load.as_ref().is_none_or(|(u, _)| *u != url) {
                return;
            }
            match res {
                Ok(()) => me.play(video, &url, live),
                Err(e) => me.show_error(&e),
            }
        });
    }

    /// Video abspielen; ist es bekannt, wird dort fortgesetzt, wo man aufgehört hat.
    pub fn play(&self, video: Option<crate::youtube::Video>, url: &str, live: bool) {
        save(self.mpv, &self.track); // vorheriges Video sichern
        report(self.mpv, &self.track, false);
        let start = video.as_ref().filter(|_| !live).and_then(|v| progress::resume_at(&v.id));
        // SponsorBlock-Abschnitte im Hintergrund holen (fragt sponsor.ajay.app, nicht YouTube).
        if let Some(id) = video.as_ref().filter(|_| !live).map(|v| v.id.clone()) {
            let (track, controls) = (self.track.clone(), self.controls.clone());
            glib::spawn_future_local(async move {
                let segs = crate::bg(crate::sponsorblock::segments(id.clone())).await;
                let mut t = track.borrow_mut();
                if t.video.as_ref().is_some_and(|v| v.id == id) {
                    controls.set_segments(segs.clone());
                    t.segments = segs;
                }
            });
        }
        // Geschwindigkeit, die zuletzt für diesen Kanal gewählt wurde (sonst normal).
        let speed = video.as_ref().and_then(|v| v.channel_id.as_deref()).map_or(1.0, crate::lists::channel_speed);
        let _ = self.mpv.set_property("speed", speed);
        // Vorschaubild, bis das Video lädt; das alte erst entfernen, sonst blitzt es kurz auf.
        // ponytail: kein ID-Abgleich – kommt das Bild eines vorigen Videos verspätet an, steht es kurz da.
        // Unkritisch, die Bilder liegen meist schon im Cache (Raster).
        self.poster.set_paintable(None::<&gdk::Paintable>);
        self.poster.remove_css_class("loaded");
        self.poster.set_visible(video.is_some());
        if let Some(v) = &video {
            crate::grid::load_thumb(&self.poster, v.id.clone());
        }
        self.track.replace(Track { video, live, started: Some(Instant::now()), ..Default::default() });
        self.controls.reset(live);
        self.area.grab_focus();
        // loadfile <url> <flags> <index> <optionen>; index -1 = ohne Playlist-Position.
        let opts = start.map(|s| format!("start={s:.1}")).unwrap_or_default();
        self.track.borrow_mut().load = Some((url.to_string(), opts.clone()));
        if !self.gl_ready.get() {
            // Video-Fläche noch nicht bereit (erstes Video der Sitzung): laden, sobald sie da ist.
            self.pending.replace(Some((url.to_string(), opts)));
            return;
        }
        if let Err(e) = self.mpv.command("loadfile", &[url, "replace", "-1", &opts]) {
            eprintln!("mpv loadfile: {e}");
        }
    }

    /// Läuft ein Video (geladen, kein Fehler, nicht am Ende)? Dann geht es beim Verlassen in den Miniplayer.
    pub fn active(&self) -> bool {
        self.track.borrow().load.is_some()
            && !self.msg.is_visible()
            && !self.end_home.parent().is_some_and(|b| b.is_visible())
    }

    /// Wiedergabe beenden (Miniplayer schließen, App schließen). Speichert vorher die Position.
    pub fn stop(&self) {
        save(self.mpv, &self.track);
        // ponytail: wartet auch beim Schließen des Miniplayers (kurz); nur beim App-Ende wäre es nötig.
        report(self.mpv, &self.track, true);
        self.track.replace(Track::default());
        self.pending.take();
        let _ = self.mpv.command("stop", &[]);
        self.poster.set_visible(false);
        self.msg.set_visible(false);
        self.retry.set_visible(false);
        self.spinner.set_visible(false);
        if let Some(f) = self.on_playing.borrow().as_ref() {
            f(false);
        }
    }

    fn speed(&self, dir: i32) -> bool {
        self.controls.step_speed(dir);
        true
    }

    /// Strg+→/←: nächstes/voriges Kapitel, mit Einblendung des Titels.
    pub fn chapter(&self, dir: i32) {
        match self.controls.chapter_jump(dir) {
            Some(t) if !t.is_empty() => toast(&self.flash, &t),
            Some(_) => {}
            None if self.controls.has_chapters() => {}
            None => toast(&self.flash, "No chapters"),
        }
        self.controls.poke();
    }

    /// Tastenkürzel wie bei YouTube. Gibt `true` zurück, wenn die Taste verwendet wurde.
    pub fn key(&self, key: gdk::Key) -> bool {
        // Sprünge mit Einblendung; Lautstärke/Ton/Tempo blenden über ihre Ereignisse ein.
        let jump = |s: i32| {
            self.controls.forget_chapter_jump();
            let _ = self.mpv.command("seek", &[&s.to_string()]);
            toast(&self.flash, &if s < 0 { format!("−{} s", -s) } else { format!("+{s} s") });
            self.controls.poke();
            true
        };
        // 0–9: an 0–90 % springen (wie bei YouTube).
        if let Some(d) = key.to_unicode().and_then(|c| c.to_digit(10)) {
            self.controls.forget_chapter_jump();
            let _ = self.mpv.command("seek", &[&(d * 10).to_string(), "absolute-percent"]);
            self.controls.poke();
            return true;
        }
        // Lautstärke und Ton weich (controls.rs), nicht als harter Sprung.
        match key {
            gdk::Key::Up | gdk::Key::plus | gdk::Key::KP_Add => {
                self.controls.step_volume(5.0);
                return true;
            }
            gdk::Key::Down | gdk::Key::minus | gdk::Key::KP_Subtract => {
                self.controls.step_volume(-5.0);
                return true;
            }
            gdk::Key::m => {
                self.controls.toggle_mute();
                return true;
            }
            _ => {}
        }
        let cmd: &[&str] = match key {
            gdk::Key::space | gdk::Key::k => &["cycle", "pause"],
            gdk::Key::Left => return jump(-5),
            gdk::Key::Right => return jump(5),
            gdk::Key::j => return jump(-10),
            gdk::Key::l => return jump(10),
            // Bild für Bild (bei Pause), wie bei YouTube.
            gdk::Key::comma => &["frame-back-step"],
            gdk::Key::period => &["frame-step"],
            gdk::Key::c => {
                if !self.controls.toggle_subs() {
                    toast(&self.flash, "No subtitles");
                }
                self.controls.poke();
                return true;
            }
            // Lautstärke: + / - (auch Ziffernblock) und Pfeile hoch/runter.
            gdk::Key::greater => return self.speed(1),
            gdk::Key::less => return self.speed(-1),
            _ => return false,
        };
        let _ = self.mpv.command(cmd[0], &cmd[1..]);
        self.controls.poke();
        true
    }
}

/// Liegt `w` im Miniplayer (Ansehen-Seite mit Klasse `mini`)?
fn in_mini(w: &gtk::Widget) -> bool {
    std::iter::successors(Some(w.clone()), |w| w.parent()).any(|w| w.has_css_class("mini"))
}

#[cfg(test)]
mod tests {
    use super::reason;

    #[test]
    fn wait_outside_runtime() {
        // Normaler Test-Thread ohne Tokio-Kontext, wie der GTK-Thread.
        let t = std::time::Instant::now();
        super::wait_at_most(std::time::Duration::from_millis(50), std::future::pending());
        assert!(t.elapsed() >= std::time::Duration::from_millis(50));
        super::wait_at_most(std::time::Duration::from_secs(5), async {});
    }

    fn l(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn reason_extracts_cause() {
        let log = l(&[
            "ERROR: [youtube] jNQXAC9eVRc: This video is unavailable",
            "youtube-dl failed: unexpected error occurred",
            "Failed to recognize file format.",
        ]);
        assert_eq!(reason(&log, "x"), "This video is unavailable");
        let bot = l(&["ERROR: [youtube] abc: Sign in to confirm you're not a bot. Use --cookies"]);
        assert_eq!(reason(&bot, "x"), "Sign in to confirm you're not a bot. Use --cookies");
        assert_eq!(reason(&l(&["Failed to recognize file format."]), "x"), "Failed to recognize file format.");
        assert_eq!(reason(&[], "fallback"), "fallback");
    }

    #[test]
    fn login_opts_escaped() {
        let raw = format!("{},cookies-from-browser=x", super::SUBS_RAW);
        let want = format!("ytdl-raw-options=%{}%{raw}", raw.len());
        assert_eq!(super::with_login("", "cookies-from-browser=x"), want);
        assert_eq!(super::with_login("start=12.0", "cookies-from-browser=x"), format!("start=12.0,{want}"));
    }
}

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
use std::cell::RefCell;
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
    spinner: gtk::Spinner,
    /// Wird mit dem Seitenverhältnis (Breite/Höhe) aufgerufen, sobald das Video es kennt.
    on_aspect: Rc<RefCell<Option<Box<dyn Fn(f64)>>>>,
    track: Rc<RefCell<Track>>,
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
}

/// Kurzer Hinweis oben links über dem Video, verschwindet nach ein paar Sekunden von selbst.
fn toast(label: &gtk::Label, text: &str) {
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
        t.last_save = Some(Instant::now());
    }
}

/// Zeigt eine Meldung über dem Video (rot, wenn `error`).
fn show(msg: &gtk::Label, text: &str, error: bool) {
    msg.set_text(text);
    if error { msg.add_css_class("error") } else { msg.remove_css_class("error") }
    msg.set_visible(true);
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
                i.set_property("keep-open", true)?;
                // Sprünge zum nächsten Schlüsselbild statt bildgenau: YouTube drosselt Downloads auf etwa
                // Abspielgeschwindigkeit, bildgenaue Sprünge müssten erst Sekunden davor laden (gemessen:
                // bis 6 s statt meist unter 1 s). Ziel liegt dadurch meist 1–2 s neben der Stelle.
                i.set_property("hr-seek", "no")?;
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
            ("dwidth", Format::Int64),
            ("dheight", Format::Int64),
        ] {
            let _ = mpv.observe_property(name, format, 0);
        }

        let area = gtk::GLArea::new();
        area.set_hexpand(true);
        // Höhe bestimmt die Ansehen-Seite passend zum Video; nicht von selbst wachsen.
        area.set_focusable(true);

        let ctx: Rc<RefCell<Option<RenderContext<'static>>>> = Rc::default();

        area.connect_realize({
            let ctx = ctx.clone();
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
            }
        });

        area.connect_unrealize({
            let ctx = ctx.clone();
            move |a| {
                a.make_current();
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
        let controls = Controls::new(mpv, &widget);
        let track: Rc<RefCell<Track>> = Rc::default();

        // Klick aufs Video: Pause umschalten. Doppelklick: Vollbild
        // (der erste Klick des Doppelklicks pausiert, der zweite hebt das wieder auf).
        let click = gtk::GestureClick::new();
        click.connect_pressed({
            let controls = controls.clone();
            move |_, n, _, _| {
                let _ = mpv.command("cycle", &["pause"]);
                if n == 2 {
                    controls.fullscreen.emit_clicked();
                }
            }
        });
        area.add_controller(click);

        // Mausbewegung über dem Video blendet die Leiste ein.
        let motion = gtk::EventControllerMotion::new();
        motion.connect_motion({
            let controls = controls.clone();
            move |_, _, _| controls.poke()
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
        widget.set_child(Some(&area));
        widget.add_overlay(&msg);
        // Laden: nur ein Spinner, kein Text.
        let spinner = gtk::Spinner::builder().width_request(32).height_request(32).halign(gtk::Align::Center).valign(gtk::Align::Center).visible(false).build();
        widget.add_overlay(&spinner);
        let on_aspect: Rc<RefCell<Option<Box<dyn Fn(f64)>>>> = Rc::default();
        let note = gtk::Label::builder()
            .halign(gtk::Align::Start)
            .valign(gtk::Align::Start)
            .margin_start(16)
            .margin_top(16)
            .visible(false)
            .css_classes(["osd", "pill"])
            .build();
        widget.add_overlay(&note);
        widget.add_overlay(&controls.widget);

        glib::spawn_future_local({
            let msg = msg.clone();
            let spinner = spinner.clone();
            let on_aspect = on_aspect.clone();
            let controls = controls.clone();
            let track = track.clone();
            async move {
                let mut log: Vec<String> = Vec::new();
                while ev_rx.recv().await.is_ok() {
                    while let Some(ev) = mpv.wait_event(0.0) {
                        match ev {
                            Ok(Event::StartFile) => {
                                log.clear();
                                msg.set_visible(false);
                                spinner.set_visible(true);
                                spinner.start();
                            }
                            Ok(Event::FileLoaded) => {
                                spinner.stop();
                                spinner.set_visible(false);
                                controls.load_chapters();
                            }
                            Ok(Event::LogMessage { text, .. }) => log.push(text.trim().to_string()),
                            Ok(Event::PropertyChange { name, change, .. }) => {
                                if name == "dwidth" || name == "dheight" {
                                    let w = mpv.get_property::<i64>("dwidth").unwrap_or(0);
                                    let h = mpv.get_property::<i64>("dheight").unwrap_or(0);
                                    if let (true, Some(f)) = (w > 0 && h > 0, on_aspect.borrow().as_ref()) {
                                        f(w as f64 / h as f64);
                                    }
                                }
                                let due = track.borrow().last_save.is_none_or(|t| t.elapsed() >= SAVE_EVERY);
                                if (name == "time-pos" && due) || (name == "pause" && matches!(change, PropertyData::Flag(true))) {
                                    save(mpv, &track);
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
                                controls.update(name, change);
                            }
                            // Ein Fehler-Ereignis ist hier das Ende einer Datei mit Fehler.
                            Err(e) => {
                                eprintln!("Video-Fehler: {log:?}");
                                // YouTube verweigert gelegentlich einzelne Stream-Server kurzzeitig.
                                // Einmal neu laden holt frische Adressen; erst dann den Fehler zeigen.
                                let retry = {
                                    let mut t = track.borrow_mut();
                                    let retry = (!t.retried).then(|| t.load.clone()).flatten();
                                    t.retried = true;
                                    retry
                                };
                                if let Some((url, opts)) = retry {
                                    let _ = mpv.command("loadfile", &[&url, "replace", "-1", &opts]);
                                } else {
                                    let why = reason(&log, &e.to_string());
                                    spinner.stop();
                                    spinner.set_visible(false);
                                    show(&msg, &format!("Couldn't load video:\n{why}"), true);
                                }
                            }
                            _ => {}
                        }
                    }
                }
            }
        });

        Player { mpv, widget, controls, area, msg, spinner, on_aspect, track }
    }

    pub fn connect_aspect(&self, f: impl Fn(f64) + 'static) {
        self.on_aspect.replace(Some(Box::new(f)));
    }

    pub fn show_error(&self, text: &str) {
        self.spinner.set_visible(false);
        show(&self.msg, text, true);
    }

    /// Video abspielen; ist es bekannt, wird dort fortgesetzt, wo man aufgehört hat.
    pub fn play(&self, video: Option<crate::youtube::Video>, url: &str, live: bool) {
        save(self.mpv, &self.track); // vorheriges Video sichern
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
        self.track.replace(Track { video, live, ..Default::default() });
        self.controls.reset(live);
        self.area.grab_focus();
        // loadfile <url> <flags> <index> <optionen>; index -1 = ohne Playlist-Position.
        let opts = start.map(|s| format!("start={s:.1}")).unwrap_or_default();
        self.track.borrow_mut().load = Some((url.to_string(), opts.clone()));
        if let Err(e) = self.mpv.command("loadfile", &[url, "replace", "-1", &opts]) {
            eprintln!("mpv loadfile: {e}");
        }
    }

    /// Wiedergabe beenden (Ansehen-Seite verlassen, App schließen). Speichert vorher die Position.
    pub fn stop(&self) {
        save(self.mpv, &self.track);
        self.track.replace(Track::default());
        let _ = self.mpv.command("stop", &[]);
        self.msg.set_visible(false);
        self.spinner.set_visible(false);
    }

    fn speed(&self, dir: i32) -> bool {
        self.controls.step_speed(dir);
        true
    }

    /// Tastenkürzel wie bei YouTube. Gibt `true` zurück, wenn die Taste verwendet wurde.
    pub fn key(&self, key: gdk::Key) -> bool {
        let cmd: &[&str] = match key {
            gdk::Key::space | gdk::Key::k => &["cycle", "pause"],
            gdk::Key::Left => &["seek", "-5"],
            gdk::Key::Right => &["seek", "5"],
            gdk::Key::j => &["seek", "-10"],
            gdk::Key::l => &["seek", "10"],
            gdk::Key::Up => &["add", "volume", "5"],
            gdk::Key::Down => &["add", "volume", "-5"],
            gdk::Key::m => &["cycle", "mute"],
            gdk::Key::greater => return self.speed(1),
            gdk::Key::less => return self.speed(-1),
            _ => return false,
        };
        let _ = self.mpv.command(cmd[0], &cmd[1..]);
        self.controls.poke();
        true
    }
}

#[cfg(test)]
mod tests {
    use super::reason;

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
}

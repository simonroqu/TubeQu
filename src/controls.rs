//! Eigene Bedienleiste über dem Video: Fortschritt (mit Kapiteln), Play/Pause, Lautstärke,
//! Einstellungen (Qualität, Geschwindigkeit), Vollbild.
//! Bubble-Design: Timeline ohne Hintergrund, jede Knopfgruppe in einer eigenen Glas-Pille.
//! Erscheint nur bei Mausbewegung über dem Player und blendet sich nach kurzer Zeit wieder aus.

use crate::grid::fmt_duration;
use crate::youtube::Storyboard;
use gtk::gdk;
use gtk::gdk_pixbuf::Pixbuf;
use std::collections::HashMap;
use gtk4 as gtk;
use gtk::glib;
use gtk::prelude::*;
use libmpv2::Mpv;
use crate::sponsorblock::Segment;
use libmpv2::events::PropertyData;
use std::cell::{Cell, RefCell};
use std::path::PathBuf;
use std::rc::Rc;
use std::time::Duration;

const HIDE_AFTER: Duration = Duration::from_millis(2000);
/// Beim Ziehen an der Timeline: so lange still halten, bevor gesprungen wird.
const SEEK_DELAY: Duration = Duration::from_millis(150);

/// Wählbare Qualitäten (maximale Bildhöhe). Gibt es die gewählte nicht, nimmt mpv die nächstkleinere.
const QUALITIES: [(u32, &str); 6] =
    [(2160, "4K (2160p)"), (1440, "1440p"), (1080, "1080p"), (720, "720p"), (480, "480p"), (360, "360p")];
const DEFAULT_QUALITY: u32 = 1080;
pub const SPEEDS: [f64; 8] = [0.25, 0.5, 0.75, 1.0, 1.25, 1.5, 1.75, 2.0];

/// Datei in `~/.config/tubequ/` (Qualität, Lautstärke).
pub fn config_file(name: &str) -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("tubequ").join(name)
}

fn quality_file() -> PathBuf {
    config_file("quality")
}

/// Ein-/Ausblenden des Tons und Lautstärke-Rampen: so viele Schritte in diesem Abstand (zusammen 160 ms).
const FADE_STEPS: u32 = 8;
const FADE_STEP: Duration = Duration::from_millis(20);

/// Höchste Lautstärke in % (Regler-Ende). 100 = normal, liegt so in der Mitte des Reglers.
pub const MAX_VOLUME: f64 = 200.0;

/// Zuletzt eingestellte Lautstärke (0–`MAX_VOLUME`), bleibt über Neustarts erhalten.
/// Standard 100 = normale Lautstärke, der Regler steht dann in der Mitte.
pub fn saved_volume() -> f64 {
    std::fs::read_to_string(config_file("volume"))
        .ok()
        .and_then(|s| s.trim().parse::<f64>().ok())
        .map_or(100.0, |v| v.clamp(0.0, MAX_VOLUME))
}

fn save_volume(v: f64) {
    let f = config_file("volume");
    let _ = std::fs::create_dir_all(f.parent().unwrap());
    let _ = std::fs::write(f, format!("{v:.0}"));
}

/// Zuletzt gewählte Qualität (bleibt über Neustarts erhalten).
pub fn saved_quality() -> u32 {
    std::fs::read_to_string(quality_file())
        .ok()
        .and_then(|s| s.trim().parse().ok())
        .filter(|q| QUALITIES.iter().any(|(h, _)| h == q))
        .unwrap_or(DEFAULT_QUALITY)
}

/// yt-dlp-Formatauswahl für eine maximale Bildhöhe.
pub fn format_for(height: u32) -> String {
    format!("bestvideo[height<={height}]+bestaudio/best[height<={height}]/best")
}

pub fn speed_label(s: f64) -> String {
    if s == 1.0 { "Normal".into() } else { format!("{s}×") }
}

/// Kapitel, in dem `pos` liegt (Kapitel sind nach Startzeit sortiert).
fn chapter_at(chapters: &[(f64, String)], pos: f64) -> Option<&str> {
    chapters.iter().rev().find(|(start, _)| *start <= pos).map(|(_, t)| t.as_str())
}

pub struct Controls {
    pub widget: gtk::Revealer,
    /// Vorschau über der Timeline; liegt als eigene Ebene im Player (player.rs), nicht in der Leiste.
    pub preview: gtk::Box,
    pub fullscreen: gtk::Button,
    mpv: &'static Mpv,
    pub play: gtk::Button,
    seek: gtk::Scale,
    time: gtk::Label,
    vol: gtk::Scale,
    pub mute: gtk::Button,
    settings: gtk::MenuButton,
    speed_buttons: Vec<(f64, gtk::CheckButton)>,
    /// Bekommt beim Ausblenden einen unsichtbaren Mauszeiger.
    cursor_target: gtk::Widget,
    hide_timer: RefCell<Option<glib::SourceId>>,
    chapters: Rc<RefCell<Vec<(f64, String)>>>,
    /// SponsorBlock-Abschnitte, als farbige Bereiche über dem Fortschrittsbalken gezeichnet.
    segments: Rc<RefCell<Vec<Segment>>>,
    /// Bis hierhin (Sekunden) ist das Video vorgeladen; wird hell in der Timeline gezeigt.
    cached_until: Rc<Cell<f64>>,
    /// Vorschau beim Überfahren der Timeline: Bild aus dem Storyboard plus Zeit/Kapitel.
    storyboard: Rc<RefCell<Option<Storyboard>>>,
    /// Geladene Storyboard-Bögen (None = lädt gerade).
    sheets: Rc<RefCell<HashMap<usize, Option<Pixbuf>>>>,
    segment_layer: gtk::DrawingArea,
    pos: Cell<f64>,
    dur: Cell<f64>,
    speed: Cell<f64>,
    /// Ton aus (von mpv gemeldet) und laufende Ein-/Ausblendung bzw. Lautstärke-Rampe (Nummer, damit eine
    /// neuere die ältere abbricht).
    muted: Cell<bool>,
    fade_gen: Cell<u32>,
    vol_gen: Cell<u32>,
    vol_target: Cell<Option<f64>>,
    live: Cell<bool>,
    paused: Cell<bool>,
}

fn icon_button(icon: &str, tip: &str) -> gtk::Button {
    gtk::Button::builder()
        .icon_name(icon)
        .tooltip_text(tip)
        .css_classes(["flat"])
        // Kein Fokus, sonst würde die Leertaste den zuletzt geklickten Button auslösen.
        .focusable(false)
        .build()
}

/// Abstand der Vorschau über der Timeline-Schiene.
const PREVIEW_GAP: f32 = 24.0;

/// Vorschau mittig über der Maus (nicht über die Enden der Timeline hinaus) und immer gleich hoch über
/// der Schiene. Gerechnet in der Ebene, in der sie liegt: so verschieben sie weder Kapitel-Striche
/// unter der Schiene noch eine höhere Vorschau.
fn place_preview(preview: &gtk::Box, seek: &gtk::Scale, trough: &gtk::Widget, x: f64) {
    let Some(layer) = preview.parent() else { return };
    let (Some(tr), Some(sk)) = (trough.compute_bounds(&layer), seek.compute_bounds(&layer)) else { return };
    let w = preview.width().max(preview.first_child().map_or(0, |c| c.width_request())).max(60) as f64;
    let lo = sk.x() as f64;
    let hi = (lo + sk.width() as f64 - w).max(lo);
    preview.set_margin_start((lo + x - w / 2.0).clamp(lo, hi) as i32);
    preview.set_margin_bottom((layer.height() as f32 - tr.y() + PREVIEW_GAP).max(0.0) as i32);
}

fn slider(max: f64) -> gtk::Scale {
    let s = gtk::Scale::with_range(gtk::Orientation::Horizontal, 0.0, max, 1.0);
    s.set_draw_value(false);
    s.set_focusable(false);
    s
}

/// Spalte im Einstellungsmenü: Überschrift und Auswahlknöpfe (nur einer aktiv).
fn radio_column(title: &str, labels: &[String]) -> (gtk::Box, Vec<gtk::CheckButton>) {
    let col = gtk::Box::new(gtk::Orientation::Vertical, 2);
    col.append(&gtk::Label::builder().label(title).xalign(0.0).margin_bottom(4).css_classes(["heading"]).build());
    let mut buttons: Vec<gtk::CheckButton> = Vec::new();
    for l in labels {
        let b = gtk::CheckButton::builder().label(l).focusable(false).build();
        if let Some(first) = buttons.first() {
            b.set_group(Some(first));
        }
        col.append(&b);
        buttons.push(b);
    }
    (col, buttons)
}

impl Controls {
    pub fn new(mpv: &'static Mpv, cursor_target: &impl IsA<gtk::Widget>) -> Rc<Self> {
        let play = icon_button("tubequ-pause-symbolic", "Pause (space)");
        play.connect_clicked(move |_| {
            let _ = mpv.command("cycle", &["pause"]);
        });

        let seek = slider(1.0);
        seek.set_hexpand(true);

        let mute = icon_button("tubequ-volume-symbolic", "Mute (m)");
        let vol = slider(MAX_VOLUME);
        vol.set_width_request(90);
        vol.set_margin_end(8);
        vol.connect_change_value(move |_, _, v| {
            let _ = mpv.set_property("volume", v.clamp(0.0, MAX_VOLUME));
            glib::Propagation::Proceed
        });

        let time = gtk::Label::builder()
            .css_classes(["numeric"])
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let fullscreen = icon_button("tubequ-fullscreen-symbolic", "Full screen (f)");

        // Timeline: change-value kommt nur von der Maus, nicht von set_value (keine Rückkopplung).
        // Beim Ziehen nicht bei jeder Bewegung springen (jeder Sprung lädt neu), sondern erst,
        // wenn die Maus kurz still steht oder losgelassen wird. Die Zeit folgt sofort.
        let pending_seek: Rc<RefCell<Option<glib::SourceId>>> = Rc::default();
        seek.connect_change_value({
            let time = time.downgrade();
            move |s, _, v| {
                if let Some(t) = time.upgrade() {
                    t.set_text(&format!("{} / {}", fmt_duration(v.max(0.0) as u32), fmt_duration(s.adjustment().upper() as u32)));
                }
                if let Some(id) = pending_seek.take() {
                    id.remove();
                }
                let pending = pending_seek.clone();
                let id = glib::timeout_add_local_once(SEEK_DELAY, move || {
                    pending.take();
                    let _ = mpv.command("seek", &[&v.to_string(), "absolute"]);
                });
                pending_seek.replace(Some(id));
                glib::Propagation::Proceed
            }
        });

        // Einstellungen: Qualität und Geschwindigkeit nebeneinander.
        let (quality_col, quality_buttons) =
            radio_column("Quality", &QUALITIES.iter().map(|(_, l)| l.to_string()).collect::<Vec<_>>());
        let (speed_col, speed_buttons) = radio_column("Playback speed", &SPEEDS.map(speed_label));
        let menu = gtk::Box::builder().spacing(24).margin_top(8).margin_bottom(8).margin_start(8).margin_end(8).build();
        menu.append(&quality_col);
        menu.append(&speed_col);
        let settings = gtk::MenuButton::builder()
            .icon_name("tubequ-settings-symbolic")
            .tooltip_text("Settings")
            .direction(gtk::ArrowType::Up)
            .popover(&gtk::Popover::builder().child(&menu).build())
            .focusable(false)
            .css_classes(["flat"])
            .build();

        let saved = saved_quality();
        for ((height, _), b) in QUALITIES.iter().zip(&quality_buttons) {
            b.set_active(*height == saved);
            let height = *height;
            b.connect_toggled(move |b| {
                if !b.is_active() {
                    return;
                }
                let _ = std::fs::create_dir_all(quality_file().parent().unwrap());
                let _ = std::fs::write(quality_file(), height.to_string());
                let _ = mpv.set_property("ytdl-format", format_for(height));
                // Neue Qualität gilt erst nach neuem Laden: an derselben Stelle weiter.
                if let Ok(path) = mpv.get_property::<String>("path") {
                    let pos = mpv.get_property::<f64>("time-pos").unwrap_or(0.0);
                    let live = mpv.get_property::<f64>("duration").is_err();
                    let opts = if live { String::new() } else { format!("start={pos:.1}") };
                    let _ = mpv.command("loadfile", &[&path, "replace", "-1", &opts]);
                }
            });
        }
        let speed_buttons: Vec<(f64, gtk::CheckButton)> = SPEEDS.into_iter().zip(speed_buttons).collect();
        for (speed, b) in &speed_buttons {
            b.set_active(*speed == 1.0);
            let speed = *speed;
            b.connect_toggled(move |b| {
                if b.is_active() {
                    let _ = mpv.set_property("speed", speed);
                }
            });
        }

        // Jede Gruppe in einer eigenen Bubble.
        let bubble = |widgets: &[&gtk::Widget]| {
            let b = gtk::Box::builder().spacing(2).valign(gtk::Align::Center).css_classes(["bubble"]).build();
            for w in widgets {
                b.append(*w);
            }
            b
        };
        let buttons = gtk::Box::new(gtk::Orientation::Horizontal, 8);
        buttons.append(&bubble(&[play.upcast_ref()]));
        buttons.append(&bubble(&[mute.upcast_ref(), vol.upcast_ref()]));
        buttons.append(&bubble(&[time.upcast_ref()]));
        buttons.append(&gtk::Box::builder().hexpand(true).build());
        // Untertitel durchschalten (wie Taste c); die Sprache blendet der Player ein.
        let subtitles = icon_button("tubequ-subtitles-symbolic", "Subtitles (c)");
        subtitles.connect_clicked(move |_| {
            let _ = mpv.command("cycle", &["sub"]);
        });
        buttons.append(&bubble(&[subtitles.upcast_ref(), settings.upcast_ref(), fullscreen.upcast_ref()]));

        // Vorschau über der Timeline (Bild + Zeit), folgt der Maus. Liegt in der Leiste über dem Balken;
        // unsichtbar nimmt sie keinen Platz ein, sichtbar wächst die (unten verankerte) Leiste nach oben.
        let preview_pic = gtk::Picture::builder().overflow(gtk::Overflow::Hidden).can_shrink(true).content_fit(gtk::ContentFit::Fill).css_classes(["preview"]).visible(false).build();
        let preview_time = gtk::Label::builder().css_classes(["osd", "pill", "numeric"]).halign(gtk::Align::Center).max_width_chars(40).ellipsize(gtk::pango::EllipsizeMode::End).build();
        let preview = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .halign(gtk::Align::Start)
            .valign(gtk::Align::End)
            .can_target(false)
            .visible(false)
            .build();
        preview.append(&preview_pic);
        preview.append(&preview_time);
        let storyboard: Rc<RefCell<Option<Storyboard>>> = Rc::default();
        let chapters: Rc<RefCell<Vec<(f64, String)>>> = Rc::default();
        let sheets: Rc<RefCell<HashMap<usize, Option<Pixbuf>>>> = Rc::default();

        let bar = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(6)
            .margin_start(16)
            .margin_end(16)
            .margin_bottom(14)
            .css_classes(["player-controls"])
            .build();
        seek.add_css_class("timeline");

        // Farbige Sponsor-Bereiche: eigene Zeichenfläche über dem Balken, genau über dessen Schiene.
        let segments: Rc<RefCell<Vec<Segment>>> = Rc::default();
        let cached_until: Rc<Cell<f64>> = Rc::default();
        let segment_layer = gtk::DrawingArea::builder().can_target(false).build();
        segment_layer.set_draw_func({
            let (segments, cached_until, seek) = (segments.clone(), cached_until.clone(), seek.downgrade());
            move |area, cr, _, _| {
                let Some(seek) = seek.upgrade() else { return };
                let segs = segments.borrow();
                let dur = seek.adjustment().upper();
                // Die Schiene ("trough") ist ein internes Kind des Reglers; ihre Lage bestimmt, wo gezeichnet wird.
                let mut child = seek.first_child();
                while let Some(c) = child.as_ref().filter(|c| c.css_name() != "trough") {
                    child = c.next_sibling();
                }
                let (Some(trough), true) = (child, dur > 1.0) else { return };
                let Some(r) = trough.compute_bounds(area) else { return };
                let (h, y) = (r.height() as f64, r.y() as f64);
                let x_at = |t: f64| r.x() as f64 + r.width() as f64 * (t / dur).clamp(0.0, 1.0);
                // Vorgeladener Bereich (aktuelle Stelle bis Pufferende) heller, wie bei YouTube:
                // dorthin springen geht ohne Warten.
                let (pos, until) = (seek.value(), cached_until.get());
                if until > pos {
                    cr.set_source_rgba(1.0, 1.0, 1.0, 0.3);
                    cr.rectangle(x_at(pos), y, x_at(until) - x_at(pos), h);
                    let _ = cr.fill();
                }
                // Dezent in der Schriftfarbe des Themes, keine Signalfarben.
                let c = seek.color();
                for s in segs.iter() {
                    let x0 = r.x() as f64 + r.width() as f64 * (s.start / dur).clamp(0.0, 1.0);
                    let x1 = r.x() as f64 + r.width() as f64 * (s.end / dur).clamp(0.0, 1.0);
                    cr.set_source_rgba(c.red().into(), c.green().into(), c.blue().into(), 0.45);
                    cr.rectangle(x0, y, (x1 - x0).max(2.0), h);
                    let _ = cr.fill();
                }
            }
        });
        let seek_row = gtk::Overlay::builder().child(&seek).build();
        seek_row.add_overlay(&segment_layer);
        bar.append(&seek_row);

        let hover = gtk::EventControllerMotion::new();
        hover.connect_motion({
            let (preview, pic, label, seek, storyboard, sheets, chapters) = (
                preview.clone(),
                preview_pic.clone(),
                preview_time.clone(),
                seek.downgrade(),
                storyboard.clone(),
                sheets.clone(),
                chapters.clone(),
            );
            move |_, x, _| {
                let Some(seek) = seek.upgrade() else { return };
                let dur = seek.adjustment().upper();
                // Zeit unter der Maus: Lage der Schiene ("trough") im Regler.
                let mut child = seek.first_child();
                while let Some(c) = child.as_ref().filter(|c| c.css_name() != "trough") {
                    child = c.next_sibling();
                }
                let (Some(trough), true) = (child, dur > 1.0) else { return };
                let Some(r) = trough.compute_bounds(&seek) else { return };
                let t = ((x - r.x() as f64) / r.width() as f64).clamp(0.0, 1.0) * dur;
                // Zeit und – falls vorhanden – Kapitel an dieser Stelle.
                match chapter_at(&chapters.borrow(), t).filter(|c| !c.is_empty()) {
                    Some(c) => label.set_text(&format!("{} · {c}", fmt_duration(t as u32))),
                    None => label.set_text(&fmt_duration(t as u32)),
                }
                show_frame(&pic, &storyboard, &sheets, t);
                place_preview(&preview, &seek, &trough, x);
                preview.set_visible(true);
            }
        });
        hover.connect_leave({
            let preview = preview.clone();
            move |_| preview.set_visible(false)
        });
        seek.add_controller(hover);
        bar.append(&buttons);

        let widget = gtk::Revealer::builder()
            .child(&bar)
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .reveal_child(true)
            .valign(gtk::Align::End)
            .build();

        let c = Rc::new(Controls {
            widget,
            preview: preview.clone(),
            fullscreen,
            mpv,
            play,
            seek,
            time,
            vol,
            mute,
            settings,
            speed_buttons,
            cursor_target: cursor_target.clone().upcast(),
            hide_timer: RefCell::default(),
            chapters,
            segments,
            segment_layer,
            cached_until,
            storyboard,
            sheets,
            pos: Cell::new(0.0),
            dur: Cell::new(0.0),
            speed: Cell::new(1.0),
            muted: Cell::new(false),
            fade_gen: Cell::new(0),
            vol_gen: Cell::new(0),
            vol_target: Cell::new(None),
            live: Cell::new(false),
            paused: Cell::new(false),
        });
        c.mute.connect_clicked({
            let c = Rc::downgrade(&c);
            move |_| {
                if let Some(c) = c.upgrade() {
                    c.toggle_mute();
                }
            }
        });
        c
    }

    /// Leiste zeigen und das Ausblenden neu starten (bei Mausbewegung, Taste, Pause …).
    pub fn poke(self: &Rc<Self>) {
        self.widget.set_reveal_child(true);
        self.cursor_target.set_cursor(None);
        self.cursor_target.remove_css_class("idle");
        if let Some(t) = self.hide_timer.take() {
            t.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(HIDE_AFTER, move || {
            let Some(c) = weak.upgrade() else { return };
            c.hide_timer.take(); // Timer ist abgelaufen, nicht mehr entfernen.
            // Maus ruht: Hover-Hervorhebung aus (style.css `.idle`), auch wenn die Leiste sichtbar bleibt.
            c.cursor_target.add_css_class("idle");
            // Pausiert oder bei offenem Einstellungsmenü sichtbar bleiben.
            if !c.paused.get() && !c.settings.is_active() {
                c.widget.set_reveal_child(false);
                c.cursor_target.set_cursor_from_name(Some("none"));
            }
        });
        self.hide_timer.replace(Some(id));
    }

    /// Sofort ausblenden (Maus hat den Player verlassen). Bei offenem Einstellungsmenü nicht.
    pub fn hide(&self) {
        if self.settings.is_active() {
            return;
        }
        if let Some(t) = self.hide_timer.take() {
            t.remove();
        }
        self.widget.set_reveal_child(false);
    }

    /// Neues Video: Anzeige zurücksetzen. Bei Livestreams gibt es keinen Fortschrittsbalken.
    pub fn reset(self: &Rc<Self>, live: bool) {
        self.live.set(live);
        self.pos.set(0.0);
        self.dur.set(0.0);
        self.chapters.borrow_mut().clear();
        self.cached_until.set(0.0);
        self.storyboard.replace(None);
        self.sheets.borrow_mut().clear();
        self.set_segments(vec![]);
        self.seek.clear_marks();
        self.seek.set_visible(!live);
        self.seek.set_value(0.0);
        self.update_time();
        self.poke();
    }

    /// Storyboard des laufenden Videos (aus den yt-dlp-Infos); ohne zeigt die Vorschau nur die Zeit.
    pub fn set_storyboard(&self, sb: Option<Storyboard>) {
        if *self.storyboard.borrow() != sb {
            self.sheets.borrow_mut().clear();
            self.storyboard.replace(sb);
        }
    }

    pub fn set_segments(&self, segments: Vec<Segment>) {
        self.segments.replace(segments);
        self.segment_layer.queue_draw();
    }

    /// Kapitel aus mpv übernehmen (yt-dlp liefert sie beim Laden mit) und im Balken markieren.
    pub fn load_chapters(&self) {
        let count = self.mpv.get_property::<i64>("chapter-list/count").unwrap_or(0);
        let chapters: Vec<(f64, String)> = (0..count)
            .filter_map(|i| {
                let start = self.mpv.get_property::<f64>(&format!("chapter-list/{i}/time")).ok()?;
                let title = self.mpv.get_property::<String>(&format!("chapter-list/{i}/title")).unwrap_or_default();
                Some((start, title))
            })
            .collect();
        self.seek.clear_marks();
        for (start, _) in chapters.iter().filter(|(s, _)| *s > 0.0) {
            self.seek.add_mark(*start, gtk::PositionType::Bottom, None);
        }
        self.chapters.replace(chapters);
        self.update_time();
    }

    /// Geschwindigkeit eine Stufe schneller (+1) oder langsamer (-1), wie `>` / `<` bei YouTube.
    pub fn step_speed(&self, dir: i32) {
        let i = SPEEDS.iter().position(|s| *s == self.speed.get()).unwrap_or(3) as i32;
        let next = SPEEDS[(i + dir).clamp(0, SPEEDS.len() as i32 - 1) as usize];
        let _ = self.mpv.set_property("speed", next);
    }

    /// Ton weich aus- bzw. einblenden statt hart umzuschalten (Filter `@fade` in player.rs; die
    /// Lautstärke selbst bleibt dabei unberührt). Knopf, Taste `m` und Miniplayer.
    pub fn toggle_mute(self: &Rc<Self>) {
        let muting = !self.muted.get();
        let generation = self.fade_gen.get() + 1;
        self.fade_gen.set(generation);
        if !muting {
            let _ = self.mpv.command("af-command", &["fade", "volume", "0"]);
            let _ = self.mpv.set_property("mute", false);
        }
        for i in 1..=FADE_STEPS {
            let me = self.clone();
            glib::timeout_add_local_once(FADE_STEP * i, move || {
                if me.fade_gen.get() != generation {
                    return;
                }
                let f = i as f64 / FADE_STEPS as f64;
                let level = if muting { 1.0 - f } else { f };
                let _ = me.mpv.command("af-command", &["fade", "volume", &format!("{level:.3}")]);
                if muting && i == FADE_STEPS {
                    let _ = me.mpv.set_property("mute", true);
                    let _ = me.mpv.command("af-command", &["fade", "volume", "1"]);
                }
            });
        }
    }

    /// Lautstärke um `delta` ändern, in kleinen Schritten statt eines Sprungs (Tasten `+`/`-`).
    pub fn step_volume(self: &Rc<Self>, delta: f64) {
        let from = self.vol_target.get().unwrap_or(self.vol.value());
        let to = (from + delta).clamp(0.0, MAX_VOLUME);
        let start = self.vol.value();
        let generation = self.vol_gen.get() + 1;
        self.vol_gen.set(generation);
        self.vol_target.set(Some(to));
        for i in 1..=FADE_STEPS {
            let me = self.clone();
            glib::timeout_add_local_once(FADE_STEP * i, move || {
                if me.vol_gen.get() != generation {
                    return;
                }
                let v = start + (to - start) * i as f64 / FADE_STEPS as f64;
                let _ = me.mpv.set_property("volume", v.round());
                if i == FADE_STEPS {
                    me.vol_target.set(None);
                }
            });
        }
    }

    /// Wird für jede beobachtete mpv-Eigenschaft aufgerufen, wenn sie sich ändert.
    pub fn update(self: &Rc<Self>, name: &str, data: PropertyData) {
        match (name, data) {
            ("time-pos", PropertyData::Double(t)) => {
                self.pos.set(t);
                // Während der Nutzer den Regler zieht, nicht dazwischenfunken.
                if !self.seek.state_flags().contains(gtk::StateFlags::ACTIVE) {
                    self.seek.set_value(t);
                }
                self.update_time();
            }
            ("demuxer-cache-time", PropertyData::Double(t)) => {
                self.cached_until.set(t);
                self.segment_layer.queue_draw();
            }
            ("duration", PropertyData::Double(d)) => {
                self.dur.set(d);
                self.seek.set_range(0.0, d.max(1.0));
                self.segment_layer.queue_draw();
                self.update_time();
            }
            ("pause", PropertyData::Flag(p)) => {
                self.paused.set(p);
                let (icon, tip) = if p {
                    ("tubequ-play-symbolic", "Play (space)")
                } else {
                    ("tubequ-pause-symbolic", "Pause (space)")
                };
                self.play.set_icon_name(icon);
                self.play.set_tooltip_text(Some(tip));
                self.poke();
            }
            ("speed", PropertyData::Double(s)) => {
                self.speed.set(s);
                if let Some((_, b)) = self.speed_buttons.iter().find(|(v, _)| *v == s) {
                    b.set_active(true);
                }
                self.update_time();
                self.poke();
            }
            // Jede Änderung (Regler, Pfeiltasten) merken.
            ("volume", PropertyData::Double(v)) => {
                self.vol.set_value(v);
                save_volume(v);
            }
            ("mute", PropertyData::Flag(m)) => {
                self.muted.set(m);
                self.mute.set_icon_name(if m { "tubequ-mute-symbolic" } else { "tubequ-volume-symbolic" });
            }
            _ => {}
        }
    }

    fn update_time(&self) {
        // Während an der Timeline gezogen wird, zeigt die Zeit das Ziel (siehe change-value).
        if self.seek.state_flags().contains(gtk::StateFlags::ACTIVE) {
            return;
        }
        let mut parts = vec![if self.live.get() {
            "● LIVE".to_string()
        } else {
            format!("{} / {}", fmt_duration(self.pos.get() as u32), fmt_duration(self.dur.get() as u32))
        }];
        if let Some(c) = chapter_at(&self.chapters.borrow(), self.pos.get()).filter(|c| !c.is_empty()) {
            parts.push(c.to_string());
        }
        if self.speed.get() != 1.0 {
            parts.push(speed_label(self.speed.get()));
        }
        let t = parts.join(" · ");
        if self.time.text() != t {
            self.time.set_text(&t);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chapters_speed_quality() {
        let ch = vec![(0.0, "Intro".to_string()), (60.0, "Teil 1".to_string()), (300.0, "Ende".to_string())];
        assert_eq!(chapter_at(&ch, 10.0), Some("Intro"));
        assert_eq!(chapter_at(&ch, 60.0), Some("Teil 1"));
        assert_eq!(chapter_at(&ch, 999.0), Some("Ende"));
        assert_eq!(chapter_at(&[], 5.0), None);
        assert_eq!(speed_label(1.5), "1.5×");
        assert_eq!(speed_label(1.0), "Normal");
        assert!(format_for(720).contains("height<=720"));
    }
}

/// Vorschaubild zur Zeit `t` zeigen. Fehlt der Bogen noch, wird er im Hintergrund geladen und
/// danach gezeigt (solange die Maus noch dort ist); bis dahin nur die Zeit.
fn show_frame(pic: &gtk::Picture, sb: &Rc<RefCell<Option<Storyboard>>>, sheets: &Rc<RefCell<HashMap<usize, Option<Pixbuf>>>>, t: f64) {
    let Some(board) = sb.borrow().clone() else {
        pic.set_visible(false);
        return;
    };
    // Anzeigegröße: mindestens 192 px breit, Seitenverhältnis wie im Storyboard.
    let w = board.width.max(192);
    pic.set_size_request(w, w * board.height / board.width.max(1));
    let (sheet, x, y) = board.locate(t);
    let loaded = sheets.borrow().get(&sheet).cloned();
    match loaded {
        Some(Some(pixbuf)) => {
            if x + board.width <= pixbuf.width() && y + board.height <= pixbuf.height() {
                let frame = pixbuf.new_subpixbuf(x, y, board.width, board.height);
                #[allow(deprecated)] // ponytail: Texture::for_pixbuf reicht für kleine Vorschaubilder
                pic.set_paintable(Some(&gdk::Texture::for_pixbuf(&frame)));
                pic.set_visible(true);
            }
        }
        Some(None) => {} // lädt gerade
        None => {
            sheets.borrow_mut().insert(sheet, None);
            let url = board.sheets[sheet].clone();
            let (pic, sb, sheets) = (pic.downgrade(), sb.clone(), sheets.clone());
            glib::spawn_future_local(async move {
                let bytes = crate::bg(crate::youtube::fetch_image(url)).await;
                let pixbuf = bytes.ok().and_then(|b| {
                    let loader = gtk::gdk_pixbuf::PixbufLoader::new();
                    loader.write(&b).ok()?;
                    loader.close().ok()?;
                    loader.pixbuf()
                });
                // Inzwischen anderes Video: verwerfen (die Tabelle wurde dann geleert).
                if !sheets.borrow().contains_key(&sheet) {
                    return;
                }
                match pixbuf {
                    Some(p) => {
                        sheets.borrow_mut().insert(sheet, Some(p));
                        if let Some(pic) = pic.upgrade().filter(|p| p.parent().is_some_and(|b| b.is_visible())) {
                            show_frame(&pic, &sb, &sheets, t);
                        }
                    }
                    None => {
                        sheets.borrow_mut().remove(&sheet);
                    }
                }
            });
        }
    }
}

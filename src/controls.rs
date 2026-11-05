//! Eigene Bedienleiste über dem Video: Fortschritt (mit Kapiteln), Play/Pause, Lautstärke,
//! Einstellungen (Qualität, Geschwindigkeit), Vollbild.
//! Bubble-Design: Timeline ohne Hintergrund, jede Knopfgruppe in einer eigenen Glas-Pille.
//! Erscheint nur bei Mausbewegung über dem Player und blendet sich nach kurzer Zeit wieder aus.

use crate::grid::fmt_duration;
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

fn quality_file() -> PathBuf {
    let base = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|_| PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config"));
    base.join("tube/quality")
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

fn speed_label(s: f64) -> String {
    if s == 1.0 { "Normal".into() } else { format!("{s}×") }
}

/// Kapitel, in dem `pos` liegt (Kapitel sind nach Startzeit sortiert).
fn chapter_at(chapters: &[(f64, String)], pos: f64) -> Option<&str> {
    chapters.iter().rev().find(|(start, _)| *start <= pos).map(|(_, t)| t.as_str())
}

pub struct Controls {
    pub widget: gtk::Revealer,
    pub fullscreen: gtk::Button,
    mpv: &'static Mpv,
    play: gtk::Button,
    seek: gtk::Scale,
    time: gtk::Label,
    vol: gtk::Scale,
    mute: gtk::Button,
    settings: gtk::MenuButton,
    speed_buttons: Vec<(f64, gtk::CheckButton)>,
    /// Bekommt beim Ausblenden einen unsichtbaren Mauszeiger.
    cursor_target: gtk::Widget,
    hide_timer: RefCell<Option<glib::SourceId>>,
    chapters: RefCell<Vec<(f64, String)>>,
    /// SponsorBlock-Abschnitte, als farbige Bereiche über dem Fortschrittsbalken gezeichnet.
    segments: Rc<RefCell<Vec<Segment>>>,
    segment_layer: gtk::DrawingArea,
    pos: Cell<f64>,
    dur: Cell<f64>,
    speed: Cell<f64>,
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
        let play = icon_button("tube-pause-symbolic", "Pause (space)");
        play.connect_clicked(move |_| {
            let _ = mpv.command("cycle", &["pause"]);
        });

        let seek = slider(1.0);
        seek.set_hexpand(true);

        let mute = icon_button("tube-volume-symbolic", "Mute (m)");
        mute.connect_clicked(move |_| {
            let _ = mpv.command("cycle", &["mute"]);
        });
        let vol = slider(100.0);
        vol.set_width_request(90);
        vol.set_margin_end(8);
        vol.connect_change_value(move |_, _, v| {
            let _ = mpv.set_property("volume", v.clamp(0.0, 100.0));
            glib::Propagation::Proceed
        });

        let time = gtk::Label::builder()
            .css_classes(["numeric"])
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .build();
        let fullscreen = icon_button("tube-fullscreen-symbolic", "Full screen (f)");

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
            .icon_name("tube-settings-symbolic")
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
        buttons.append(&bubble(&[settings.upcast_ref(), fullscreen.upcast_ref()]));

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
        let segment_layer = gtk::DrawingArea::builder().can_target(false).build();
        segment_layer.set_draw_func({
            let (segments, seek) = (segments.clone(), seek.downgrade());
            move |area, cr, _, _| {
                let Some(seek) = seek.upgrade() else { return };
                let segs = segments.borrow();
                let dur = seek.adjustment().upper();
                // Die Schiene ("trough") ist ein internes Kind des Reglers; ihre Lage bestimmt, wo gezeichnet wird.
                let mut child = seek.first_child();
                while let Some(c) = child.as_ref().filter(|c| c.css_name() != "trough") {
                    child = c.next_sibling();
                }
                let (Some(trough), true) = (child, dur > 1.0 && !segs.is_empty()) else { return };
                let Some(r) = trough.compute_bounds(area) else { return };
                let (h, y) = (r.height() as f64, r.y() as f64);
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
        bar.append(&buttons);

        let widget = gtk::Revealer::builder()
            .child(&bar)
            .transition_type(gtk::RevealerTransitionType::Crossfade)
            .reveal_child(true)
            .valign(gtk::Align::End)
            .build();

        Rc::new(Controls {
            widget,
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
            chapters: RefCell::default(),
            segments,
            segment_layer,
            pos: Cell::new(0.0),
            dur: Cell::new(0.0),
            speed: Cell::new(1.0),
            live: Cell::new(false),
            paused: Cell::new(false),
        })
    }

    /// Leiste zeigen und das Ausblenden neu starten (bei Mausbewegung, Taste, Pause …).
    pub fn poke(self: &Rc<Self>) {
        self.widget.set_reveal_child(true);
        self.cursor_target.set_cursor(None);
        if let Some(t) = self.hide_timer.take() {
            t.remove();
        }
        let weak = Rc::downgrade(self);
        let id = glib::timeout_add_local_once(HIDE_AFTER, move || {
            let Some(c) = weak.upgrade() else { return };
            c.hide_timer.take(); // Timer ist abgelaufen, nicht mehr entfernen.
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
        self.set_segments(vec![]);
        self.seek.clear_marks();
        self.seek.set_visible(!live);
        self.seek.set_value(0.0);
        self.update_time();
        self.poke();
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
            ("duration", PropertyData::Double(d)) => {
                self.dur.set(d);
                self.seek.set_range(0.0, d.max(1.0));
                self.segment_layer.queue_draw();
                self.update_time();
            }
            ("pause", PropertyData::Flag(p)) => {
                self.paused.set(p);
                let (icon, tip) = if p {
                    ("tube-play-symbolic", "Play (space)")
                } else {
                    ("tube-pause-symbolic", "Pause (space)")
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
            ("volume", PropertyData::Double(v)) => self.vol.set_value(v),
            ("mute", PropertyData::Flag(m)) => self.mute.set_icon_name(if m { "tube-mute-symbolic" } else { "tube-volume-symbolic" }),
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

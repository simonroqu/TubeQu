//! Ansehen-Seite wie bei YouTube: Video, darunter Titel, Kanalzeile (Avatar, Name, Abonnenten)
//! und ein Kasten mit Aufrufen, Datum und ausklappbarer Beschreibung. Die ganze Seite scrollt.
//! Der Player ist genau so hoch wie das Video (keine schwarzen Balken oben/unten).

use crate::bg;
use crate::grid::fmt_views;
use crate::youtube::{self, Details, Video};
use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

/// So viele Zeilen Beschreibung sind eingeklappt sichtbar.
const DESCRIPTION_LINES: usize = 3;
const SHOW_MORE: &str = "Show more";
const SHOW_LESS: &str = "Show less";

/// Eingeklappte Beschreibung: die ersten Zeilen. (GTK begrenzt `lines` nur pro Absatz,
/// bei Beschreibungen mit vielen Zeilenumbrüchen würde sonst alles angezeigt.)
fn collapsed(desc: &str) -> String {
    desc.lines().take(DESCRIPTION_LINES).collect::<Vec<_>>().join("\n")
}

/// Player-Höhe: volle Breite im Seitenverhältnis, höchstens so hoch wie die Seite.
/// Im Vollbild immer die ganze Höhe.
fn player_height(width: i32, page_height: i32, aspect: f64, fullscreen: bool) -> i32 {
    if fullscreen || aspect <= 0.0 {
        return page_height;
    }
    ((width as f64 / aspect).round() as i32).min(page_height)
}

/// Rand um den Player (Hintergrundfarbe), im Vollbild keiner.
const PLAYER_MARGIN: i32 = 5;
/// Anteil der Fensterbreite, den der Player höchstens einnimmt; links/rechts bleibt Hintergrund.
const PLAYER_WIDTH: f64 = 0.85;
/// Unter dieser Fensterbreite nimmt der Player die volle Breite ein.
const FULL_WIDTH_BELOW: i32 = 1600;
/// Miniplayer unten rechts (Seite verlassen, Video läuft weiter): Breite und Abstand zum Fensterrand.
const MINI_WIDTH: i32 = 400;
const MINI_MARGIN: i32 = 16;

/// Kleinste Player-Breite, die man mit Strg+Mausrad einstellen kann.
const MIN_PLAYER_WIDTH: i32 = 640;
/// So viele Pixel breiter/schmaler pro Mausrad-Raste.
const ZOOM_STEP: f64 = 80.0;

/// Seitenrand des Players. `want` = mit Strg+Mausrad gewählte Player-Breite: Sie bleibt fest, ein
/// schmaleres Fenster nimmt nur den Rand weg; reicht das Fenster nicht, randlos über die ganze Breite.
/// Ohne Wahl: mittig `PLAYER_WIDTH` der Breite, unter `FULL_WIDTH_BELOW` volle Breite. Im Vollbild kein Rand.
fn player_side(width: i32, fullscreen: bool, want: Option<i32>) -> i32 {
    if fullscreen {
        return 0;
    }
    match want {
        Some(p) if p >= width => 0,
        Some(p) => PLAYER_MARGIN.max((width - p) / 2),
        // Schmales Fenster: volle Breite (nur der kleine Rand).
        None if width < FULL_WIDTH_BELOW => PLAYER_MARGIN,
        None => PLAYER_MARGIN.max((width as f64 * (1.0 - PLAYER_WIDTH) / 2.0).round() as i32),
    }
}

/// Neue Wunschbreite nach `dy` Mausrad-Rasten (hoch = negativ = breiter). Ab Fensterbreite = randlos.
fn zoom(current: i32, dy: f64, window: i32) -> i32 {
    ((current as f64 - dy * ZOOM_STEP).round() as i32).clamp(MIN_PLAYER_WIDTH, window.max(MIN_PLAYER_WIDTH))
}

fn saved_width() -> Option<i32> {
    std::fs::read_to_string(crate::controls::config_file("player-width")).ok()?.trim().parse().ok()
}

fn save_width(w: i32) {
    let f = crate::controls::config_file("player-width");
    let _ = std::fs::create_dir_all(f.parent().unwrap());
    let _ = std::fs::write(f, w.to_string());
}

pub struct WatchPage {
    pub widget: gtk::Overlay,
    /// Alles unter dem Video; wird im Vollbild ausgeblendet.
    pub info: gtk::Box,
    /// Graue Platzhalter in Form von `info` mit Glanz-Animation, solange die Details laden.
    skeleton: gtk::Box,
    scroll: gtk::ScrolledWindow,
    player: gtk::Widget,
    title: gtk::Label,
    avatar: gtk::Image,
    channel: gtk::Button,
    channel_name: gtk::Label,
    subscribers: gtk::Label,
    meta: gtk::Label,
    /// Details geladen: erst dann erscheint die Info unter dem Video (vorher nur der Player).
    ready: Cell<bool>,
    description: gtk::Label,
    more: gtk::Button,
    full_description: RefCell<String>,
    /// Seitengröße, Seitenverhältnis des Videos und Vollbild: daraus folgt die Player-Höhe.
    size: Cell<(i32, i32)>,
    aspect: Cell<f64>,
    fullscreen: Cell<bool>,
    /// Mit Strg+Mausrad gewählte Player-Breite (bleibt über Neustarts erhalten).
    want: Cell<Option<i32>>,
    /// Miniplayer: nur das Video, klein unten rechts über der aktuellen Seite.
    mini: Cell<bool>,
    /// Ecke des Miniplayers (rechts?, unten?); per Ziehen wählbar, gilt für die Sitzung.
    corner: Cell<(bool, bool)>,
    /// ID des angezeigten Videos und seines Kanals; verspätete Antworten anderer Videos werden verworfen.
    current: RefCell<String>,
    /// Ersatz-Details aus yt-dlp und ob rustypipe für das aktuelle Video gescheitert ist.
    fallback: RefCell<Option<Details>>,
    details_failed: Cell<bool>,
    channel_id: RefCell<Option<String>>,
}

/// Platzhalter für Titel, Kanalzeile und Beschreibung (style.css `.skeleton`), gleicher Aufbau wie `info`.
/// Breite -1 = ganze Zeile. ponytail: feste Breiten nur klein (≤ 360 px), damit das Fenster schmal bleiben kann.
fn skeleton() -> gtk::Box {
    let bar = |w: i32, h: i32| {
        gtk::Box::builder()
            .width_request(w)
            .height_request(h)
            .halign(if w < 0 { gtk::Align::Fill } else { gtk::Align::Start })
            .css_classes(["skeleton"])
            .build()
    };
    let name_col = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).valign(gtk::Align::Center).build();
    name_col.append(&bar(160, 16));
    name_col.append(&bar(100, 12));
    let channel_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
    let circle = bar(40, 40);
    circle.add_css_class("round");
    channel_row.append(&circle);
    channel_row.append(&name_col);
    let desc = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).css_classes(["description-box"]).build();
    for (w, h) in [(140, 16), (-1, 14), (-1, 14), (240, 14)] {
        desc.append(&bar(w, h));
    }
    let b = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .spacing(12)
        .margin_top(12)
        .margin_bottom(16)
        .margin_start(16)
        .margin_end(16)
        .can_target(false)
        .visible(false)
        .build();
    b.append(&bar(360, 26));
    b.append(&channel_row);
    b.append(&desc);
    b
}

fn text(classes: &[&str]) -> gtk::Label {
    gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).focusable(false).css_classes(classes.to_vec()).build()
}

impl WatchPage {
    /// `on_link` bekommt das Ziel eines angeklickten Links in der Beschreibung
    /// (`seek:<sek>`, `channel:<id>` oder eine URL, siehe `youtube::description_markup`).
    pub fn new(player: &impl IsA<gtk::Widget>, on_channel: Rc<dyn Fn(String)>, on_link: Rc<dyn Fn(String)>) -> Rc<Self> {
        let title = text(&["title-3"]);

        let avatar = gtk::Image::builder().pixel_size(40).overflow(gtk::Overflow::Hidden).halign(gtk::Align::Center).valign(gtk::Align::Center).css_classes(["avatar", "placeholder"]).build();
        let channel_name = gtk::Label::builder().xalign(0.0).css_classes(["heading"]).build();
        let channel = gtk::Button::builder()
            .child(&channel_name)
            .css_classes(["flat", "channel-link"])
            .halign(gtk::Align::Start)
            .focusable(false)
            .cursor(&gdk::Cursor::from_name("pointer", None).unwrap())
            .build();
        let subscribers = gtk::Label::builder().xalign(0.0).css_classes(["dim-label", "caption"]).build();
        let name_col = gtk::Box::new(gtk::Orientation::Vertical, 0);
        name_col.append(&channel);
        name_col.append(&subscribers);
        let channel_row = gtk::Box::new(gtk::Orientation::Horizontal, 12);
        channel_row.append(&avatar);
        channel_row.append(&name_col);

        let meta = text(&["heading"]);
        let description = text(&[]);
        description.set_use_markup(true);
        description.connect_activate_link(move |_, uri| {
            on_link(uri.to_string());
            glib::Propagation::Stop
        });
        let more = gtk::Button::builder().label(SHOW_MORE).halign(gtk::Align::Start).focusable(false).css_classes(["flat"]).build();
        let desc_box = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(6).css_classes(["description-box"]).build();
        desc_box.append(&meta);
        desc_box.append(&description);
        desc_box.append(&more);

        let info = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(12)
            .margin_bottom(16)
            .margin_start(16)
            .margin_end(16)
            .build();
        info.append(&title);
        info.append(&channel_row);
        info.append(&desc_box);

        let content = gtk::Box::new(gtk::Orientation::Vertical, 0);
        content.append(player);
        content.append(&info);
        let skeleton = skeleton();
        content.append(&skeleton);
        let scroll = gtk::ScrolledWindow::builder().child(&content).hscrollbar_policy(gtk::PolicyType::Never).build();
        crate::smooth::attach(&scroll);

        // Unsichtbare Messfläche über der ganzen Seite: liefert Breite und Höhe für die Player-Höhe.
        let sizer = gtk::DrawingArea::builder().can_target(false).build();
        // Liegt als Ebene über den Seiten (main.rs), daher eigener Hintergrund (außer als Miniplayer).
        let widget = gtk::Overlay::builder().child(&scroll).css_classes(["background"]).visible(false).build();
        widget.add_overlay(&sizer);

        let page = Rc::new(WatchPage {
            widget,
            info,
            skeleton,
            scroll,
            player: player.clone().upcast(),
            title,
            avatar,
            channel,
            channel_name,
            subscribers,
            meta,
            ready: Cell::new(false),
            description,
            more,
            full_description: RefCell::default(),
            fallback: RefCell::default(),
            details_failed: Cell::new(false),
            size: Cell::new((0, 0)),
            aspect: Cell::new(16.0 / 9.0),
            fullscreen: Cell::new(false),
            want: Cell::new(saved_width()),
            mini: Cell::new(false),
            corner: Cell::new((true, true)),
            current: RefCell::default(),
            channel_id: RefCell::default(),
        });

        sizer.connect_resize({
            let page = Rc::downgrade(&page);
            move |_, w, h| {
                if let Some(p) = page.upgrade() {
                    p.size.set((w, h));
                    // Nicht mitten in der Größenberechnung umbauen, sondern direkt danach.
                    glib::idle_add_local_once(move || p.fit_player());
                }
            }
        });
        // Strg+Mausrad: Player-Breite einstellen (vor dem Scrollen der Seite).
        let zoom_ctl = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        zoom_ctl.set_propagation_phase(gtk::PropagationPhase::Capture);
        zoom_ctl.connect_scroll({
            let page = Rc::downgrade(&page);
            move |c, _, dy| {
                let Some(p) = page.upgrade() else { return glib::Propagation::Proceed };
                if !c.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) || p.mini.get() || p.fullscreen.get() {
                    return glib::Propagation::Proceed;
                }
                let w = p.size.get().0;
                // Touchpad liefert Pixel statt Rasten.
                let dy = if c.unit() == gdk::ScrollUnit::Wheel { dy } else { dy / ZOOM_STEP };
                let new = zoom(w - 2 * player_side(w, false, p.want.get()), dy, w);
                p.want.set(Some(new));
                save_width(new);
                p.fit_player();
                glib::Propagation::Stop
            }
        });
        page.widget.add_controller(zoom_ctl);
        page.channel.connect_clicked({
            let page = Rc::downgrade(&page);
            move |_| {
                if let Some(id) = page.upgrade().and_then(|p| p.channel_id.borrow().clone()) {
                    on_channel(id);
                }
            }
        });
        page.more.connect_clicked({
            let page = Rc::downgrade(&page);
            move |b| {
                let Some(p) = page.upgrade() else { return };
                let full = p.full_description.borrow();
                let expand = b.label().as_deref() == Some(SHOW_MORE);
                let shown = if expand { full.clone() } else { collapsed(&full) };
                p.description.set_markup(&shown);
                b.set_label(if expand { SHOW_LESS } else { SHOW_MORE });
            }
        });
        page
    }

    fn fit_player(&self) {
        if self.mini.get() {
            let a = if self.aspect.get() > 0.0 { self.aspect.get() } else { 16.0 / 9.0 };
            let h = (MINI_WIDTH as f64 / a).round() as i32;
            self.player.set_margin_top(0);
            self.player.set_margin_start(0);
            self.player.set_margin_bottom(0);
            self.player.set_margin_end(0);
            self.player.add_css_class("rounded");
            self.player.set_size_request(-1, h);
            self.widget.set_size_request(MINI_WIDTH, h);
            return;
        }
        self.widget.set_size_request(-1, -1);
        let (w, h) = self.size.get();
        let fs = self.fullscreen.get();
        let side = player_side(w, fs, self.want.get());
        // Randlos (Vollbild oder ganz breit gezoomt): auch oben/unten kein Rand, eckig.
        let edge = side == 0;
        let m = if edge { 0 } else { PLAYER_MARGIN };
        self.player.set_margin_top(m);
        self.player.set_margin_bottom(m);
        self.player.set_margin_start(side);
        self.player.set_margin_end(side);
        // Text darunter bündig zum Player, wie bisher 16 px eingerückt (bzw. 11 px mehr als der Player).
        self.info.set_margin_start(side.max(PLAYER_MARGIN) - PLAYER_MARGIN + 16);
        self.info.set_margin_end(side.max(PLAYER_MARGIN) - PLAYER_MARGIN + 16);
        // Abgerundete Ecken wie die Fensterecken (`--radius-window`), im Vollbild eckig.
        if edge { self.player.remove_css_class("rounded") } else { self.player.add_css_class("rounded") }
        if w > 0 && h > 0 {
            self.player.set_size_request(-1, player_height(w - 2 * side, h - 2 * m, self.aspect.get(), fs));
        }
    }

    /// Seitenverhältnis des laufenden Videos (vom Player gemeldet).
    pub fn set_aspect(&self, aspect: f64) {
        self.aspect.set(aspect);
        self.fit_player();
    }

    fn update_info(&self) {
        let below = !self.fullscreen.get() && !self.mini.get();
        self.info.set_visible(self.ready.get() && below);
        // Platzhalter nur, wenn Details kommen (YouTube-Video), nicht bei direkt abgespielten Adressen.
        self.skeleton.set_visible(!self.ready.get() && below && !self.current.borrow().is_empty());
    }

    pub fn is_mini(&self) -> bool {
        self.mini.get()
    }

    /// Miniplayer an/aus: dieselbe Seite samt Player, nur klein unten rechts und ohne Infos
    /// (der Player wird nicht umgehängt, sonst müsste mpv seine Video-Fläche neu aufbauen).
    pub fn set_mini(&self, on: bool) {
        self.mini.set(on);
        self.place_mini();
        if on {
            self.widget.remove_css_class("background");
            self.widget.add_css_class("mini");
            self.scroll.vadjustment().set_value(0.0);
        } else {
            self.widget.add_css_class("background");
            self.widget.remove_css_class("mini");
        }
        let v = if on { gtk::PolicyType::Never } else { gtk::PolicyType::Automatic };
        self.scroll.set_policy(gtk::PolicyType::Never, v);
        self.update_info();
        self.fit_player();
    }

    /// Miniplayer in die Ecke `right`/`bottom` legen (nach dem Ziehen).
    pub fn set_corner(&self, right: bool, bottom: bool) {
        self.corner.set((right, bottom));
        self.place_mini();
    }

    /// Ausrichtung/Ränder: im Miniplayer in die gewählte Ecke, sonst ganze Fläche.
    fn place_mini(&self) {
        let on = self.mini.get();
        let (right, bottom) = self.corner.get();
        let side = |end: bool| if !on { gtk::Align::Fill } else if end { gtk::Align::End } else { gtk::Align::Start };
        self.widget.set_halign(side(right));
        self.widget.set_valign(side(bottom));
        let m = if on { MINI_MARGIN } else { 0 };
        self.widget.set_margin_start(m);
        self.widget.set_margin_end(m);
        self.widget.set_margin_top(m);
        self.widget.set_margin_bottom(m);
    }

    pub fn set_fullscreen(&self, fullscreen: bool) {
        self.fullscreen.set(fullscreen);
        self.update_info();
        self.scroll.vadjustment().set_value(0.0);
        self.fit_player();
    }

    /// Sofort zeigen, was schon bekannt ist; Details im Hintergrund laden.
    pub fn show(self: &Rc<Self>, v: &Video) {
        self.current.replace(v.id.clone());
        self.fallback.replace(None);
        self.details_failed.set(false);
        self.channel_id.replace(v.channel_id.clone());
        self.title.set_text(&v.title);
        self.channel_name.set_text(&v.channel);
        self.channel.set_sensitive(v.channel_id.is_some());
        self.avatar.clear();
        for l in [&self.subscribers, &self.meta, &self.description] {
            l.set_text("");
        }
        self.full_description.replace(String::new());
        self.more.set_label(SHOW_MORE);
        self.more.set_visible(false);
        self.ready.set(false);
        self.update_info();
        // Bis das neue Video sein Format meldet: 16:9 annehmen.
        self.aspect.set(16.0 / 9.0);
        self.fit_player();
        self.scroll.vadjustment().set_value(0.0);
    }

    /// Details (Beschreibung, Aufrufe, Avatar) holen. Getrennt von `show`, damit der Stream zuerst
    /// durchs Anfragen-Limit kommt: sonst wartet das Video 2 s (`MIN_GAP`) auf die Details.
    pub fn load_details(self: &Rc<Self>, id: String) {
        let page = self.clone();
        let busy = crate::Busy::new();
        glib::spawn_future_local(async move {
            let res = bg(youtube::details(id.clone())).await;
            drop(busy);
            if *page.current.borrow() != id {
                return;
            }
            match res {
                Ok(d) => page.set_details(d),
                // rustypipe scheitert: Ersatz aus yt-dlp, sobald der Player ihn hat (oder schon hat).
                Err(e) => {
                    eprintln!("Details über rustypipe fehlgeschlagen, nutze yt-dlp: {e}");
                    page.details_failed.set(true);
                    // Sonst kommt der Ersatz, sobald der Player die yt-dlp-Infos hat (`ytdlp_info`).
                    if let Some(d) = page.fallback.take() {
                        page.apply_fallback(d);
                    }
                }
            }
        });
    }

    /// yt-dlp-Infos des laufenden Videos (vom Player). Ersatz, falls rustypipe die Details nicht liefert.
    pub fn ytdlp_info(self: &Rc<Self>, json: &str) {
        let Some(d) = youtube::details_from_ytdlp(json).filter(|d| *self.current.borrow() == d.id) else { return };
        if self.details_failed.get() {
            self.apply_fallback(d);
        } else {
            self.fallback.replace(Some(d));
        }
    }

    /// Ersatz-Details zeigen und das fehlende Kanal-Logo nachholen (1 Anfrage, danach zwischengespeichert).
    fn apply_fallback(self: &Rc<Self>, d: Details) {
        let (id, channel) = (d.id.clone(), d.channel_id.clone());
        self.set_details(d);
        if channel.is_empty() {
            return;
        }
        let page = self.clone();
        glib::spawn_future_local(async move {
            let Some(url) = bg(youtube::channel_avatar(channel)).await else { return };
            if *page.current.borrow() == id
                && let Ok(bytes) = bg(youtube::avatar(url)).await
                && let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes))
            {
                page.avatar.set_paintable(Some(&tex));
            }
        });
    }

    fn set_details(self: &Rc<Self>, d: Details) {
        crate::progress::remember_meta(Video {
            id: d.id.clone(),
            title: d.title.clone(),
            channel: d.channel_name.clone(),
            channel_id: Some(d.channel_id.clone()),
            duration: None,
            is_live: d.is_live,
            views: Some(d.views),
            published: d.published.clone(),
            published_at: d.published_at,
        });
        self.title.set_text(&d.title);
        self.channel_name.set_text(&d.channel_name);
        self.channel.set_sensitive(true);
        self.channel_id.replace(Some(d.channel_id.clone()));
        self.subscribers.set_text(&d.subscribers.map(|n| format!("{} subscribers", fmt_views(n))).unwrap_or_default());
        let views = if d.is_live { format!("{} watching", fmt_views(d.views)) } else { format!("{} views", fmt_views(d.views)) };
        self.meta.set_text(&[Some(views), d.published].into_iter().flatten().collect::<Vec<_>>().join(" · "));
        let full = d.description.trim().to_string();
        let short = collapsed(&full);
        self.description.set_markup(&short);
        self.more.set_visible(short != full);
        self.full_description.replace(full);
        self.ready.set(true);
        self.update_info();

        if let Some(url) = d.channel_avatar {
            let img = self.avatar.downgrade();
            glib::spawn_future_local(async move {
                let Ok(bytes) = bg(youtube::avatar(url)).await else { return };
                let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)) else { return };
                if let Some(img) = img.upgrade() {
                    img.set_paintable(Some(&tex));
                }
            });
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collapses_to_first_lines() {
        assert_eq!(collapsed("a\nb\nc\nd\ne"), "a\nb\nc");
        assert_eq!(collapsed("nur eine Zeile"), "nur eine Zeile");
    }

    #[test]
    fn player_fits_video() {
        // 16:9 bei 1280 Breite: 720 hoch, Seite ist höher -> passt.
        assert_eq!(player_height(1280, 1400, 16.0 / 9.0, false), 720);
        // Breites Fenster, niedrige Seite: durch die Seitenhöhe begrenzt.
        assert_eq!(player_height(1920, 900, 16.0 / 9.0, false), 900);
        // Hochkant-Video (9:16): wäre riesig, also Seitenhöhe.
        assert_eq!(player_height(1280, 1400, 9.0 / 16.0, false), 1400);
        assert_eq!(player_height(1280, 1400, 16.0 / 9.0, true), 1400);
        // Seitenrand: unter 1600 px volle Breite (5 px), sonst 85 % mittig, Vollbild = 0.
        assert_eq!(player_side(1343, false, None), 5);
        assert_eq!(player_side(2560, false, None), 192);
        assert_eq!(player_side(2560, true, None), 0);
        // Gewählte Breite bleibt, schmaleres Fenster nimmt den Rand weg, zu schmal = randlos.
        assert_eq!(player_side(2560, false, Some(1600)), 480);
        assert_eq!(player_side(1800, false, Some(1600)), 100);
        assert_eq!(player_side(1500, false, Some(1600)), 0);
    }

    #[test]
    fn zoom_steps_and_limits() {
        assert_eq!(zoom(1600, -1.0, 2560), 1680);
        assert_eq!(zoom(1600, 1.0, 2560), 1520);
        assert_eq!(zoom(2500, -3.0, 2560), 2560);
        assert_eq!(zoom(700, 5.0, 2560), MIN_PLAYER_WIDTH);
    }
}

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

/// Seitenrand des Players: so viel, dass er mittig `PLAYER_WIDTH` der Breite einnimmt, mindestens
/// `PLAYER_MARGIN`. Im Vollbild kein Rand.
fn player_side(width: i32, fullscreen: bool) -> i32 {
    if fullscreen {
        return 0;
    }
    // Schmales Fenster: volle Breite (nur der kleine Rand).
    if width < FULL_WIDTH_BELOW {
        return PLAYER_MARGIN;
    }
    PLAYER_MARGIN.max((width as f64 * (1.0 - PLAYER_WIDTH) / 2.0).round() as i32)
}

pub struct WatchPage {
    pub widget: gtk::Overlay,
    /// Alles unter dem Video; wird im Vollbild ausgeblendet.
    pub info: gtk::Box,
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
    /// ID des angezeigten Videos und seines Kanals; verspätete Antworten anderer Videos werden verworfen.
    current: RefCell<String>,
    /// Ersatz-Details aus yt-dlp und ob rustypipe für das aktuelle Video gescheitert ist.
    fallback: RefCell<Option<Details>>,
    details_failed: Cell<bool>,
    channel_id: RefCell<Option<String>>,
}

fn text(classes: &[&str]) -> gtk::Label {
    gtk::Label::builder().xalign(0.0).wrap(true).selectable(true).focusable(false).css_classes(classes.to_vec()).build()
}

impl WatchPage {
    /// `on_link` bekommt das Ziel eines angeklickten Links in der Beschreibung
    /// (`seek:<sek>`, `channel:<id>` oder eine URL, siehe `youtube::description_markup`).
    pub fn new(player: &impl IsA<gtk::Widget>, on_channel: Rc<dyn Fn(String)>, on_link: Rc<dyn Fn(String)>) -> Rc<Self> {
        let title = text(&["title-3"]);

        let avatar = gtk::Image::builder().pixel_size(40).overflow(gtk::Overflow::Hidden).css_classes(["avatar"]).build();
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
        let scroll = gtk::ScrolledWindow::builder().child(&content).hscrollbar_policy(gtk::PolicyType::Never).build();

        // Unsichtbare Messfläche über der ganzen Seite: liefert Breite und Höhe für die Player-Höhe.
        let sizer = gtk::DrawingArea::builder().can_target(false).build();
        let widget = gtk::Overlay::builder().child(&scroll).build();
        widget.add_overlay(&sizer);

        let page = Rc::new(WatchPage {
            widget,
            info,
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
        let (w, h) = self.size.get();
        let fs = self.fullscreen.get();
        let m = if fs { 0 } else { PLAYER_MARGIN };
        let side = player_side(w, fs);
        self.player.set_margin_top(m);
        self.player.set_margin_bottom(m);
        self.player.set_margin_start(side);
        self.player.set_margin_end(side);
        // Text darunter bündig zum Player, wie bisher 16 px eingerückt (bzw. 11 px mehr als der Player).
        self.info.set_margin_start(side - PLAYER_MARGIN + 16);
        self.info.set_margin_end(side - PLAYER_MARGIN + 16);
        // Abgerundete Ecken wie die Fensterecken (`--radius-window`), im Vollbild eckig.
        if fs { self.player.remove_css_class("rounded") } else { self.player.add_css_class("rounded") }
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
        self.info.set_visible(self.ready.get() && !self.fullscreen.get());
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
        assert_eq!(player_side(1343, false), 5);
        assert_eq!(player_side(2560, false), 192);
        assert_eq!(player_side(2560, true), 0);
    }
}

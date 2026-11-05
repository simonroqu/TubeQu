//! Raster aus Video-Karten. Suche, Feed und Kanal nutzen alle dieses eine Raster.

use crate::bg;
use crate::progress;
use crate::youtube::{self, More, Page, Video};
use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub fn fmt_duration(s: u32) -> String {
    let (h, m, s) = (s / 3600, s / 60 % 60, s % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

/// Kurz wie bei YouTube (Englisch): 999 → "999", 4500 → "4.5K", 45300 → "45K", 1234567 → "1.2M".
pub fn fmt_views(n: u64) -> String {
    let short = |v: f64, unit: &str| {
        // Unter 10 eine Nachkommastelle (4.5K), darüber keine (45K).
        let num = if v < 10.0 { format!("{v:.1}") } else { format!("{v:.0}") };
        format!("{}{unit}", num.trim_end_matches(".0"))
    };
    match n {
        1_000_000_000.. => short(n as f64 / 1e9, "B"),
        1_000_000.. => short(n as f64 / 1e6, "M"),
        1_000.. => short(n as f64 / 1e3, "K"),
        _ => n.to_string(),
    }
}

/// Zeit seit Veröffentlichung wie bei YouTube: "just now", "3 minutes ago", "1 day ago", "2 years ago".
pub fn fmt_ago(secs: i64) -> String {
    const UNITS: [(i64, &str); 6] =
        [(365 * 86400, "year"), (30 * 86400, "month"), (7 * 86400, "week"), (86400, "day"), (3600, "hour"), (60, "minute")];
    UNITS
        .iter()
        .find(|(len, _)| secs >= *len)
        .map(|(len, name)| {
            let n = secs / len;
            format!("{n} {name}{} ago", if n == 1 { "" } else { "s" })
        })
        .unwrap_or_else(|| "just now".into())
}

fn label(text: &str, classes: &[&str]) -> gtk::Label {
    gtk::Label::builder()
        .label(text)
        .xalign(0.0)
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .lines(2)
        .ellipsize(gtk::pango::EllipsizeMode::End)
        .max_width_chars(1) // Breite kommt von der Karte, nicht vom Text
        .hexpand(true)
        .css_classes(classes.to_vec())
        .build()
}

/// Kanalname als flacher Button; Klick öffnet die Kanalseite.
pub fn channel_button(name: &str, id: Option<String>, on_channel: Rc<dyn Fn(String)>) -> gtk::Button {
    let b = gtk::Button::builder()
        .child(&label(name, &["dim-label"]))
        .css_classes(["flat", "channel-link"])
        .halign(gtk::Align::Start)
        .focusable(false)
        .build();
    match id {
        Some(id) => {
            b.set_cursor_from_name(Some("pointer"));
            b.connect_clicked(move |_| on_channel(id.clone()));
        }
        None => b.set_sensitive(false),
    }
    b
}

/// Fortschrittsbalken unten im Thumbnail; unsichtbar, solange nichts geschaut wurde.
fn set_progress(bar: &gtk::ProgressBar, id: &str) {
    let fraction = progress::get(id).map(|e| e.fraction());
    bar.set_fraction(fraction.unwrap_or(0.0));
    bar.set_visible(fraction.is_some());
}

fn card(v: &Video, on_channel: Rc<dyn Fn(String)>) -> (gtk::Box, gtk::ProgressBar) {
    let pic = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .can_shrink(true)
        .width_request(320)
        .height_request(180)
        .css_classes(["thumb"])
        .build();
    load_thumb(&pic, v.id.clone());

    let thumb = gtk::Overlay::builder().child(&pic).build();
    let badge = if v.is_live { Some("● LIVE".to_string()) } else { v.duration.map(fmt_duration) };
    if let Some(b) = badge {
        let l = gtk::Label::builder()
            .label(b)
            .halign(gtk::Align::End)
            .valign(gtk::Align::End)
            .margin_end(6)
            .margin_bottom(6)
            .css_classes(if v.is_live { vec!["osd", "error"] } else { vec!["osd"] })
            .build();
        thumb.add_overlay(&l);
    }
    let bar = gtk::ProgressBar::builder().valign(gtk::Align::End).css_classes(["watched"]).build();
    set_progress(&bar, &v.id);
    thumb.add_overlay(&bar);

    let unit = if v.is_live { "watching" } else { "views" };
    // Veröffentlichung: aus dem Zeitpunkt berechnet ("3 minutes ago"), sonst YouTubes Text.
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let published = v.published_at.map(|t| fmt_ago(now - t)).or_else(|| v.published.clone()).filter(|_| !v.is_live);
    let meta: Vec<String> = [v.views.map(|n| format!("{} {unit}", fmt_views(n))), published]
        .into_iter()
        .flatten()
        .collect();

    let b = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).width_request(320).build();
    b.append(&thumb);
    b.append(&label(&v.title, &["heading"]));
    b.append(&channel_button(&v.channel, v.channel_id.clone(), on_channel));
    if !meta.is_empty() {
        b.append(&label(&meta.join(" · "), &["dim-label", "caption"]));
    }
    (b, bar)
}

fn load_thumb(pic: &gtk::Picture, id: String) {
    let pic = pic.downgrade();
    glib::spawn_future_local(async move {
        let Ok(bytes) = bg(youtube::thumbnail(id)).await else { return };
        let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)) else { return };
        if let Some(pic) = pic.upgrade() {
            pic.set_paintable(Some(&tex));
        }
    });
}

/// Kartenbreite plus Abstand, um aus der verfügbaren Breite die Spaltenzahl zu berechnen.
const CARD_WIDTH: i32 = 320;
const COLUMN_SPACING: i32 = 16;
/// Nach so vielen Reihen Vorschlägen kommt auf der Startseite „Weiterschauen“.
const ROWS_BEFORE_SHELF: usize = 4;
/// Höchstens so viele Videos in „Weiterschauen“.
const SHELF_MAX: usize = 3;

fn flowbox() -> gtk::FlowBox {
    gtk::FlowBox::builder()
        .homogeneous(true)
        .selection_mode(gtk::SelectionMode::None)
        .activate_on_single_click(true)
        .row_spacing(24)
        .column_spacing(COLUMN_SPACING as u32)
        .valign(gtk::Align::Start)
        .build()
}

pub struct Grid {
    pub widget: gtk::ScrolledWindow,
    /// Die ersten Reihen (auf der Startseite vor „Weiterschauen“) und der Rest.
    top: gtk::FlowBox,
    rest: gtk::FlowBox,
    shelf: gtk::Box,
    shelf_cards: gtk::Box,
    status: gtk::Label,
    spinner: gtk::Spinner,
    footer: gtk::Label,
    footer_spinner: gtk::Spinner,
    on_play: Rc<dyn Fn(Video)>,
    on_channel: Rc<dyn Fn(String)>,
    videos: RefCell<Vec<Video>>,
    children: RefCell<Vec<gtk::FlowBoxChild>>,
    bars: RefCell<Vec<gtk::ProgressBar>>,
    next: RefCell<Option<More>>,
    loading: Cell<bool>,
    /// Startseite: mit „Weiterschauen“-Reihe.
    home: Cell<bool>,
    columns: Cell<usize>,
    /// Breite einer Spalte, damit „Weiterschauen“-Karten so breit sind wie die im Raster.
    column_width: Cell<i32>,
    /// Wird bei jedem neuen Inhalt erhöht, damit verspätete Antworten alter Suchen verworfen werden.
    generation: Cell<u64>,
}

impl Grid {
    pub fn new(on_play: impl Fn(Video) + 'static, on_channel: impl Fn(String) + 'static) -> Rc<Self> {
        // ponytail: FlowBox baut alle Karten; bei mehreren hundert Videos auf GridView umstellen.
        let top = flowbox();
        let rest = flowbox();
        // Laden: nur ein Spinner. Die Labels zeigen nur Fehler oder „keine Ergebnisse“.
        let status = gtk::Label::builder().visible(false).margin_top(48).build();
        let spinner = gtk::Spinner::builder().width_request(32).height_request(32).margin_top(48).visible(false).build();
        let footer = gtk::Label::builder().visible(false).margin_top(12).margin_bottom(24).css_classes(["dim-label"]).build();
        let footer_spinner = gtk::Spinner::builder().margin_top(12).margin_bottom(24).visible(false).build();

        // „Weiterschauen“: Überschrift und eine waagerecht scrollbare Reihe.
        let shelf_cards = gtk::Box::new(gtk::Orientation::Horizontal, COLUMN_SPACING);
        let shelf = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(12)
            .margin_top(32)
            .margin_bottom(32)
            .visible(false)
            .build();
        shelf.append(&gtk::Label::builder().label("Continue watching").xalign(0.0).css_classes(["title-3"]).build());
        // Feste Reihe (höchstens SHELF_MAX Videos), ohne Scrollen.
        shelf.append(&shelf_cards);

        // Unsichtbarer Messstreifen: meldet die verfügbare Breite, daraus folgt die Spaltenzahl.
        let sizer = gtk::DrawingArea::builder().hexpand(true).height_request(1).build();

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            // Platz für die schwebende Navigation oben links.
            .margin_top(68)
            .margin_start(16)
            .margin_end(16)
            .build();
        content.append(&sizer);
        content.append(&spinner);
        content.append(&status);
        content.append(&top);
        content.append(&shelf);
        content.append(&rest);
        content.append(&footer_spinner);
        content.append(&footer);
        let widget = gtk::ScrolledWindow::builder()
            .child(&content)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();

        let grid = Rc::new(Grid {
            widget,
            top,
            rest,
            shelf,
            shelf_cards,
            status,
            spinner,
            footer,
            footer_spinner,
            on_play: Rc::new(on_play),
            on_channel: Rc::new(on_channel),
            videos: RefCell::default(),
            children: RefCell::default(),
            bars: RefCell::default(),
            next: RefCell::default(),
            loading: Cell::new(false),
            home: Cell::new(false),
            columns: Cell::new(3),
            column_width: Cell::new(CARD_WIDTH),
            generation: Cell::new(0),
        });

        for flow in [&grid.top, &grid.rest] {
            let grid = Rc::downgrade(&grid);
            flow.connect_child_activated(move |_, child| {
                let Some(grid) = grid.upgrade() else { return };
                let i = grid.children.borrow().iter().position(|c| c == child);
                let v = i.and_then(|i| grid.videos.borrow().get(i).cloned());
                if let Some(v) = v {
                    (grid.on_play)(v);
                }
            });
        }

        sizer.connect_resize({
            let grid = Rc::downgrade(&grid);
            move |_, width, _| {
                let cols = ((width + COLUMN_SPACING) / (CARD_WIDTH + COLUMN_SPACING + 8)).max(1) as usize;
                let Some(grid) = grid.upgrade() else { return };
                let col_width = (width - (cols as i32 - 1) * COLUMN_SPACING) / cols as i32;
                if (col_width - grid.column_width.get()).abs() > 4 {
                    grid.column_width.set(col_width);
                    let mut c = grid.shelf_cards.first_child();
                    while let Some(w) = c {
                        w.set_size_request(col_width, -1);
                        c = w.next_sibling();
                    }
                }
                if cols != grid.columns.get() {
                    grid.columns.set(cols);
                    // Nicht mitten in der Größenberechnung umbauen, sondern direkt danach.
                    glib::idle_add_local_once(move || grid.layout());
                }
            }
        });

        // Am unteren Ende angekommen: nächste Seite holen.
        grid.widget.connect_edge_reached({
            let grid = Rc::downgrade(&grid);
            move |_, pos| {
                if pos == gtk::PositionType::Bottom
                    && let Some(grid) = grid.upgrade()
                {
                    grid.load_more();
                }
            }
        });

        // Zurück im Raster (z. B. nach einem Video): Fortschritt und „Weiterschauen“ auffrischen.
        grid.widget.connect_map({
            let grid = Rc::downgrade(&grid);
            move |_| {
                if let Some(grid) = grid.upgrade() {
                    for (v, bar) in grid.videos.borrow().iter().zip(grid.bars.borrow().iter()) {
                        set_progress(bar, &v.id);
                    }
                    grid.fill_shelf();
                }
            }
        });

        grid
    }

    /// Startseite (mit „Weiterschauen“) oder normale Liste (Suche, Kanal).
    pub fn set_home(&self, home: bool) {
        self.home.set(home);
        self.fill_shelf();
    }

    fn fill_shelf(&self) {
        while let Some(c) = self.shelf_cards.first_child() {
            self.shelf_cards.remove(&c);
        }
        if self.home.get() {
            for v in crate::progress::continue_watching(SHELF_MAX) {
                let (card, _) = card(&v, self.on_channel.clone());
                card.set_cursor_from_name(Some("pointer"));
                // Feste Breite wie eine Rasterspalte, statt sich in der Reihe breitzumachen.
                card.set_size_request(self.column_width.get(), -1);
                card.set_hexpand(false);
                let click = gtk::GestureClick::new();
                let on_play = self.on_play.clone();
                click.connect_released(move |_, _, _, _| on_play(v.clone()));
                card.add_controller(click);
                self.shelf_cards.append(&card);
            }
        }
        self.layout();
    }

    /// Verteilt die Karten: die ersten Reihen nach oben, den Rest unter „Weiterschauen“.
    fn layout(&self) {
        let cols = self.columns.get();
        for flow in [&self.top, &self.rest] {
            flow.set_min_children_per_line(cols as u32);
            flow.set_max_children_per_line(cols as u32);
            flow.remove_all();
        }
        let children = self.children.borrow();
        let has_shelf = self.home.get() && self.shelf_cards.first_child().is_some() && !children.is_empty();
        let split = if has_shelf { ROWS_BEFORE_SHELF * cols } else { usize::MAX };
        for (i, c) in children.iter().enumerate() {
            if i < split { self.top.append(c) } else { self.rest.append(c) }
        }
        self.shelf.set_visible(has_shelf);
    }

    /// Ersetzt den Inhalt durch das Ergebnis von `fut` (z. B. eine Suche).
    pub fn show(self: &Rc<Self>, fut: impl Future<Output = Result<Page, String>> + Send + 'static) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.videos.borrow_mut().clear();
        self.children.borrow_mut().clear();
        self.bars.borrow_mut().clear();
        self.layout();
        self.next.replace(None);
        self.footer.set_visible(false);
        self.status.set_visible(false);
        self.spinner.set_visible(true);
        self.spinner.start();
        self.widget.vadjustment().set_value(0.0);
        self.loading.set(true);

        let grid = self.clone();
        glib::spawn_future_local(async move {
            let res = bg(fut).await;
            if grid.generation.get() != generation {
                return;
            }
            grid.loading.set(false);
            grid.spinner.stop();
            grid.spinner.set_visible(false);
            match res {
                Ok(p) if p.videos.is_empty() => grid.message("No results"),
                Ok(p) => grid.append(p),
                Err(e) => grid.message(&format!("Error: {e}")),
            }
        });
    }

    fn message(&self, text: &str) {
        self.status.set_text(text);
        self.status.set_visible(true);
    }

    fn append(&self, p: Page) {
        for v in &p.videos {
            let (card, bar) = card(v, self.on_channel.clone());
            let child = gtk::FlowBoxChild::builder().child(&card).cursor(&gdk::Cursor::from_name("pointer", None).unwrap()).build();
            self.children.borrow_mut().push(child);
            self.bars.borrow_mut().push(bar);
        }
        self.videos.borrow_mut().extend(p.videos);
        self.next.replace(p.next);
        self.layout();
    }

    fn load_more(self: &Rc<Self>) {
        if self.loading.get() {
            return;
        }
        let Some(m) = self.next.take() else { return };
        self.loading.set(true);
        self.footer.set_visible(false);
        self.footer_spinner.set_visible(true);
        self.footer_spinner.start();

        let generation = self.generation.get();
        let grid = self.clone();
        glib::spawn_future_local(async move {
            let res = bg(youtube::more(m.clone())).await;
            if grid.generation.get() != generation {
                return;
            }
            grid.loading.set(false);
            grid.footer_spinner.stop();
            grid.footer_spinner.set_visible(false);
            match res {
                Ok(p) => grid.append(p),
                Err(e) => {
                    // Merker zurücklegen, damit erneutes Scrollen es nochmal versucht.
                    grid.next.replace(Some(m));
                    grid.footer.set_text(&format!("Error: {e}"));
                    grid.footer.set_visible(true);
                }
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_ago(30), "just now");
        assert_eq!(fmt_ago(60), "1 minute ago");
        assert_eq!(fmt_ago(3 * 60 + 59), "3 minutes ago");
        assert_eq!(fmt_ago(2 * 3600), "2 hours ago");
        assert_eq!(fmt_ago(86400), "1 day ago");
        assert_eq!(fmt_ago(15 * 86400), "2 weeks ago");
        assert_eq!(fmt_ago(400 * 86400), "1 year ago");
        assert_eq!(fmt_duration(59), "0:59");
        assert_eq!(fmt_duration(3725), "1:02:05");
        assert_eq!(fmt_views(999), "999");
        assert_eq!(fmt_views(4_500), "4.5K");
        assert_eq!(fmt_views(45_300), "45K");
        assert_eq!(fmt_views(1_234_567), "1.2M");
        assert_eq!(fmt_views(3_000_000), "3M");
        assert_eq!(fmt_views(2_500_000_000), "2.5B");
    }
}

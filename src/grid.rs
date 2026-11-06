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
    let entry = progress::get(id);
    let fraction = entry.as_ref().map(|e| e.fraction());
    bar.set_fraction(fraction.unwrap_or(0.0));
    bar.set_visible(fraction.is_some());
    // Fertig geschaut: Vorschaubild abdunkeln, damit Neues auffällt.
    if let Some(thumb) = bar.parent() {
        thumb.set_opacity(if entry.is_some_and(|e| e.finished()) { 0.45 } else { 1.0 });
    }
}

/// Rechtsklick: Menü mit „Watch later“, Link kopieren, Kanal, Browser. Mittelklick: „Watch later“.
/// Die Aktionen (`card.*`) hängen am Raster, siehe `Grid::new`.
fn card_menu(card: &gtk::Box, v: &Video) {
    let click = gtk::GestureClick::builder().button(0).build();
    let (id, channel) = (v.id.clone(), v.channel_id.clone());
    click.connect_pressed(move |g, _, x, y| {
        let Some(card) = g.widget() else { return };
        match g.current_button() {
            2 => {
                g.set_state(gtk::EventSequenceState::Claimed);
                let _ = card.activate_action("card.later", Some(&id.to_variant()));
            }
            3 => {
                g.set_state(gtk::EventSequenceState::Claimed);
                let menu = gtk::gio::Menu::new();
                let later = if crate::lists::is_later(&id) { "Remove from Watch later" } else { "Watch later" };
                menu.append(Some(later), Some(&format!("card.later('{id}')")));
                menu.append(Some("Copy link"), Some(&format!("card.copy('{id}')")));
                if let Some(c) = &channel {
                    menu.append(Some("Open channel"), Some(&format!("card.channel('{c}')")));
                }
                menu.append(Some("Open in browser"), Some(&format!("card.browser('{id}')")));
                let pop = gtk::PopoverMenu::from_model(Some(&menu));
                pop.set_parent(&card);
                pop.set_has_arrow(false);
                pop.set_pointing_to(Some(&gdk::Rectangle::new(x as i32, y as i32, 1, 1)));
                // Nach dem Schließen wieder vom Karten-Widget lösen.
                pop.connect_closed(|p| {
                    let p = p.clone();
                    glib::idle_add_local_once(move || p.unparent());
                });
                pop.popup();
            }
            _ => {}
        }
    });
    card.add_controller(click);
}

/// Vorschaubild-Höhe im 16:9-Format zur Kartenbreite.
fn thumb_height(width: i32) -> i32 {
    width * 9 / 16
}

fn card(v: &Video, width: i32, on_channel: Rc<dyn Fn(String)>) -> (gtk::Box, gtk::ProgressBar, gtk::Picture) {
    let pic = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .can_shrink(true)
        .height_request(thumb_height(width))
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

    // Keine Mindestbreite: die Spaltenzahl folgt aus `columns`, die Karte füllt ihre Spalte. Eine feste
    // Mindestbreite machte die Seite bei knapper Breite ein paar Pixel breiter als das Fenster.
    let b = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(4).build();
    card_menu(&b, v);
    b.append(&thumb);
    b.append(&label(&v.title, &["heading"]));
    b.append(&channel_button(&v.channel, v.channel_id.clone(), on_channel));
    if !meta.is_empty() {
        b.append(&label(&meta.join(" · "), &["dim-label", "caption"]));
    }
    (b, bar, pic)
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

/// Kleinste Kartenbreite plus Abstand, um aus der verfügbaren Breite die Spaltenzahl zu berechnen.
/// Die Karten wachsen dann mit, bis die Reihe gefüllt ist.
const CARD_WIDTH: i32 = 320;
/// Höchstens so viele Karten pro Reihe (auch im Vollbild), damit sie groß bleiben.
const MAX_COLUMNS: i32 = 4;
const COLUMN_SPACING: i32 = 16;
/// Nach so vielen Reihen Vorschlägen kommt auf der Startseite „Weiterschauen“.
const ROWS_BEFORE_SHELF: usize = 4;
/// Höchstens so viele Videos in „Weiterschauen“.
const SHELF_MAX: usize = 3;

/// Spalten für eine Breite: so viele 320-px-Karten wie passen, höchstens `MAX_COLUMNS`.
fn columns(width: i32) -> usize {
    ((width + COLUMN_SPACING) / (CARD_WIDTH + COLUMN_SPACING + 8)).clamp(1, MAX_COLUMNS) as usize
}

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
    pub widget: gtk::Overlay,
    scroll: gtk::ScrolledWindow,
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
    /// Alle angezeigten Videos (Raster + „Weiterschauen“) nach ID, für das Karten-Menü.
    known: RefCell<std::collections::HashMap<String, Video>>,
    children: RefCell<Vec<gtk::FlowBoxChild>>,
    bars: RefCell<Vec<gtk::ProgressBar>>,
    /// Vorschaubilder im Raster und in „Weiterschauen“, deren Höhe der Spaltenbreite folgt.
    pics: RefCell<Vec<gtk::Picture>>,
    shelf_pics: RefCell<Vec<gtk::Picture>>,
    /// Karten für „Weiterschauen“; `layout` zeigt davon so viele, wie es Spalten gibt.
    shelf_items: RefCell<Vec<gtk::Box>>,
    next: RefCell<Option<More>>,
    loading: Cell<bool>,
    /// Startseite: mit „Weiterschauen“-Reihe.
    home: Cell<bool>,
    columns: Cell<usize>,
    /// Breite einer Spalte, damit „Weiterschauen“-Karten so breit sind wie die im Raster.
    column_width: Cell<i32>,
    /// Wird bei jedem neuen Inhalt erhöht, damit verspätete Antworten alter Suchen verworfen werden.
    generation: Cell<u64>,
    /// Startet die zuletzt gezeigte Anfrage erneut (für Reload).
    last: RefCell<Option<Rc<dyn Fn(&Rc<Grid>)>>>,
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
        // Gleich breite Plätze, einer pro Rasterspalte (leere Plätze füllt `layout` auf): so sind die Karten
        // so breit wie im Raster, ohne feste Pixelbreite, die das Fenster beim Verkleinern breit hielte.
        // ponytail: 3 px = Innenabstand von `flowboxchild` im GTK-Standard-Theme, damit die Karten bündig
        // unter den Rasterkarten stehen; ändert sich der, hier anpassen.
        let shelf_cards = gtk::Box::builder()
            .orientation(gtk::Orientation::Horizontal)
            .spacing(COLUMN_SPACING + 6)
            .margin_start(3)
            .margin_end(3)
            .homogeneous(true)
            .build();
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

        // Unsichtbare Messfläche über der ganzen Seite: meldet die Fensterbreite, daraus folgt die Spaltenzahl.
        // Liegt bewusst nicht im Inhalt: der kann wegen fester Kartenbreiten beim Verkleinern zu breit
        // bleiben, dann sähe die Messfläche die neue Breite nie (Inhalt rutschte rechts raus).
        let sizer = gtk::DrawingArea::builder().can_target(false).build();

        let content = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            // Platz für die schwebende Navigation oben links.
            .margin_top(68)
            .margin_start(16)
            .margin_end(16)
            .build();
        content.append(&spinner);
        content.append(&status);
        content.append(&top);
        content.append(&shelf);
        content.append(&rest);
        content.append(&footer_spinner);
        content.append(&footer);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&content)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        let widget = gtk::Overlay::builder().child(&scroll).build();
        // Kurze Hinweise unten („Saved to Watch later“ …).
        let note = gtk::Label::builder()
            .halign(gtk::Align::Center)
            .valign(gtk::Align::End)
            .margin_bottom(24)
            .visible(false)
            .css_classes(["osd", "pill"])
            .build();
        widget.add_overlay(&note);
        widget.add_overlay(&sizer);

        let grid = Rc::new(Grid {
            widget,
            scroll,
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
            known: RefCell::default(),
            children: RefCell::default(),
            bars: RefCell::default(),
            pics: RefCell::default(),
            shelf_pics: RefCell::default(),
            shelf_items: RefCell::default(),
            next: RefCell::default(),
            loading: Cell::new(false),
            home: Cell::new(false),
            columns: Cell::new(3),
            column_width: Cell::new(CARD_WIDTH),
            generation: Cell::new(0),
            last: RefCell::default(),
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
            move |_, page_width, _| {
                // Abzüglich der Seitenränder des Inhalts (je 16 px).
                let width = page_width - 32;
                let cols = columns(width);
                let Some(grid) = grid.upgrade() else { return };
                let col_width = (width - (cols as i32 - 1) * COLUMN_SPACING) / cols as i32;
                if (col_width - grid.column_width.get()).abs() > 4 {
                    grid.column_width.set(col_width);
                    for pic in grid.pics.borrow().iter().chain(grid.shelf_pics.borrow().iter()) {
                        pic.set_height_request(thumb_height(col_width));
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
        grid.scroll.connect_edge_reached({
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
                    // Neu Aufbauen setzt die Liste nach oben: Stelle merken und danach wiederherstellen.
                    let adj = grid.scroll.vadjustment();
                    let pos = adj.value();
                    grid.fill_shelf();
                    glib::idle_add_local_once(move || adj.set_value(pos));
                }
            }
        });

        // Aktionen für das Karten-Menü (`card_menu`); Parameter = Video- bzw. Kanal-ID.
        let actions = gtk::gio::SimpleActionGroup::new();
        let add = |name: &str, f: Box<dyn Fn(&Rc<Grid>, String)>| {
            let a = gtk::gio::SimpleAction::new(name, Some(glib::VariantTy::STRING));
            let grid = Rc::downgrade(&grid);
            a.connect_activate(move |_, p| {
                if let (Some(g), Some(id)) = (grid.upgrade(), p.and_then(|p| p.get::<String>())) {
                    f(&g, id);
                }
            });
            actions.add_action(&a);
        };
        add("later", Box::new({
            let note = note.clone();
            move |g, id| {
                let Some(v) = g.known.borrow().get(&id).cloned() else { return };
                let saved = crate::lists::toggle_later(&v);
                crate::player::toast(&note, if saved { "Saved to Watch later" } else { "Removed from Watch later" });
            }
        }));
        add("copy", Box::new({
            let note = note.clone();
            move |g, id| {
                g.widget.clipboard().set_text(&format!("https://www.youtube.com/watch?v={id}"));
                crate::player::toast(&note, "Link copied");
            }
        }));
        add("channel", Box::new(|g, id| (g.on_channel)(id)));
        // http statt https: geht direkt an den Browser, nicht über die Tube-Link-Weiche zurück in die App.
        add("browser", Box::new(|_, id| {
            let _ = gtk::gio::AppInfo::launch_default_for_uri(
                &format!("http://www.youtube.com/watch?v={id}"),
                None::<&gtk::gio::AppLaunchContext>,
            );
        }));
        grid.widget.insert_action_group("card", Some(&actions));

        grid
    }

    /// Startseite (mit „Weiterschauen“) oder normale Liste (Suche, Kanal).
    pub fn set_home(&self, home: bool) {
        self.home.set(home);
        self.fill_shelf();
    }

    fn fill_shelf(&self) {
        self.shelf_items.borrow_mut().clear();
        self.shelf_pics.borrow_mut().clear();
        if self.home.get() {
            for v in crate::progress::continue_watching(SHELF_MAX) {
                self.known.borrow_mut().insert(v.id.clone(), v.clone());
                let (card, _, pic) = card(&v, self.column_width.get(), self.on_channel.clone());
                self.shelf_pics.borrow_mut().push(pic);
                card.set_cursor_from_name(Some("pointer"));
                let click = gtk::GestureClick::new();
                let on_play = self.on_play.clone();
                click.connect_released(move |_, _, _, _| on_play(v.clone()));
                card.add_controller(click);
                self.shelf_items.borrow_mut().push(card);
            }
        }
        self.layout();
    }

    /// Verteilt die Karten: die ersten Reihen nach oben, den Rest unter „Weiterschauen“.
    fn layout(&self) {
        let cols = self.columns.get();
        for flow in [&self.top, &self.rest] {
            // Feste Spaltenzahl (FlowBox würde sonst nach der natürlichen Bildbreite umbrechen). Die Karten
            // haben keine Mindestbreite, daher zwingt das dem Fenster keine Mindestbreite auf.
            flow.set_min_children_per_line(cols as u32);
            flow.set_max_children_per_line(cols as u32);
            flow.remove_all();
        }
        let children = self.children.borrow();
        // „Weiterschauen“: höchstens so viele Karten, wie das Raster Spalten hat, Rest mit leeren Plätzen.
        while let Some(c) = self.shelf_cards.first_child() {
            self.shelf_cards.remove(&c);
        }
        let items = self.shelf_items.borrow();
        for i in 0..cols {
            match items.get(i) {
                Some(card) => self.shelf_cards.append(card),
                None => self.shelf_cards.append(&gtk::Box::new(gtk::Orientation::Vertical, 0)),
            }
        }
        let has_shelf = self.home.get() && !items.is_empty() && !children.is_empty();
        let split = if has_shelf { ROWS_BEFORE_SHELF * cols } else { usize::MAX };
        for (i, c) in children.iter().enumerate() {
            if i < split { self.top.append(c) } else { self.rest.append(c) }
        }
        self.shelf.set_visible(has_shelf);
    }

    /// Ersetzt den Inhalt durch das Ergebnis von `make()` (z. B. eine Suche). `make` wird für Reload
    /// gemerkt und dann erneut aufgerufen.
    pub fn show<F>(self: &Rc<Self>, make: impl Fn() -> F + 'static)
    where
        F: Future<Output = Result<Page, String>> + Send + 'static,
    {
        let run: Rc<dyn Fn(&Rc<Grid>)> = Rc::new(move |g: &Rc<Grid>| g.run(make(), None));
        self.last.replace(Some(run.clone()));
        run(self);
    }

    /// Wie `show`, zeigt aber `saved` (z. B. die letzte Startseite) sofort und tauscht sie gegen das
    /// neue Ergebnis aus, sobald es da ist. Scheitert die Anfrage, bleibt das Gespeicherte stehen.
    pub fn show_saved<F>(self: &Rc<Self>, saved: Option<Page>, make: impl Fn() -> F + 'static)
    where
        F: Future<Output = Result<Page, String>> + Send + 'static,
    {
        let make = Rc::new(make);
        let again = make.clone();
        self.last.replace(Some(Rc::new(move |g: &Rc<Grid>| g.run(again(), None))));
        self.run(make(), saved);
    }

    /// Letzte Anfrage neu ausführen, am Zwischenspeicher vorbei (zählt als Anfrage).
    pub fn reload(self: &Rc<Self>) {
        let last = self.last.borrow().clone();
        if let Some(run) = last {
            youtube::forget_cache();
            run(self);
        }
    }

    fn clear(&self) {
        self.videos.borrow_mut().clear();
        self.children.borrow_mut().clear();
        self.bars.borrow_mut().clear();
        self.pics.borrow_mut().clear();
        self.layout();
        self.next.replace(None);
    }

    fn run(self: &Rc<Self>, fut: impl Future<Output = Result<Page, String>> + Send + 'static, saved: Option<Page>) {
        let generation = self.generation.get() + 1;
        self.generation.set(generation);
        self.clear();
        self.footer.set_visible(false);
        self.status.set_visible(false);
        self.scroll.vadjustment().set_value(0.0);
        self.loading.set(true);
        // Gespeichertes sofort zeigen (kein Spinner); sonst Spinner bis zur Antwort.
        let has_saved = saved.is_some();
        match saved {
            Some(p) => self.append(p),
            None => {
                self.spinner.set_visible(true);
                self.spinner.start();
            }
        }

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
                Ok(p) if p.videos.is_empty() && !has_saved => grid.message("No results"),
                Ok(p) if p.videos.is_empty() => {}
                Ok(p) => {
                    grid.clear();
                    grid.append(p);
                }
                Err(e) if has_saved => eprintln!("Neu laden fehlgeschlagen, zeige gespeicherte Seite: {e}"),
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
            let (card, bar, pic) = card(v, self.column_width.get(), self.on_channel.clone());
            self.pics.borrow_mut().push(pic);
            let child = gtk::FlowBoxChild::builder().child(&card).cursor(&gdk::Cursor::from_name("pointer", None).unwrap()).build();
            self.children.borrow_mut().push(child);
            self.bars.borrow_mut().push(bar);
        }
        self.known.borrow_mut().extend(p.videos.iter().map(|v| (v.id.clone(), v.clone())));
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
        // Spalten: schmal 1, mittel 2–3, breit/Vollbild höchstens 4.
        assert_eq!(columns(300), 1);
        assert_eq!(columns(1000), 2);
        assert_eq!(columns(1100), 3);
        assert_eq!(columns(2528), 4);
        assert_eq!(thumb_height(640), 360);
    }
}

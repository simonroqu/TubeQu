//! Raster aus Video-Karten. Suche, Feed und Kanal nutzen alle dieses eine Raster.

use qu_core::cards::{COLUMN_SPACING, TOP_MARGIN, columns, ghost_card, image_height, label, step_columns};
use crate::bg;
use crate::progress;
use crate::youtube::{self, More, Page, Video};
use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

pub use qu_core::controls::fmt_duration;

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

fn card(v: &Video, width: i32, on_channel: Rc<dyn Fn(String)>) -> (gtk::Box, gtk::ProgressBar, gtk::Picture) {
    let pic = gtk::Picture::builder()
        .content_fit(gtk::ContentFit::Cover)
        .can_shrink(true)
        .height_request(image_height(width))
        .css_classes(["thumb"])
        .build();
    load_thumb(&pic, v.id.clone());
    qu_core::fly::register(&v.id, &pic);

    // Grauer Platzhalter in Bildgröße, bis das Bild da ist; schneidet auch den Hover-Zoom ab.
    let thumb = gtk::Overlay::builder().child(&pic).overflow(gtk::Overflow::Hidden).css_classes(["thumb-box"]).build();
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

pub fn load_thumb(pic: &gtk::Picture, id: String) {
    // Unsichtbar bis zum Laden, dann einblenden (style.css `.fade`).
    pic.add_css_class("fade");
    let pic = pic.downgrade();
    glib::spawn_future_local(async move {
        let Ok(bytes) = bg(youtube::thumbnail(id)).await else { return };
        let Some(tex) = without_bars(&bytes).or_else(|| gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)).ok())
        else {
            return;
        };
        if let Some(pic) = pic.upgrade() {
            pic.set_paintable(Some(&tex));
            pic.add_css_class("loaded");
        }
    });
}

/// Vorschaubild ohne schwarze Balken oben/unten. YouTube liefert 4:3 mit eingebrannten Balken,
/// bei Kinoformat oder Balken im Video selbst dicker als der 16:9-Zuschnitt (`ContentFit::Cover`)
/// wegschneidet. Der Rest wird dann auf 16:9 gezoomt.
fn without_bars(bytes: &[u8]) -> Option<gdk::Texture> {
    let loader = gtk::gdk_pixbuf::PixbufLoader::new();
    loader.write(bytes).ok()?;
    loader.close().ok()?;
    let pb = loader.pixbuf()?;
    let (w, h, n, stride) = (pb.width(), pb.height(), pb.n_channels() as usize, pb.rowstride() as usize);
    let px = pb.read_pixel_bytes();
    // Zeile gilt als Balken, wenn jedes 8. Pixel fast schwarz ist (JPEG-Rauschen erlaubt).
    let dark = |y: i32| (0..w).step_by(8).all(|x| px[y as usize * stride + x as usize * n..][..3].iter().all(|&c| c <= 24));
    let (top, bottom) = bars(h, dark);
    if top == 0 && bottom == 0 {
        return None;
    }
    #[allow(deprecated)] // ponytail: Texture::for_pixbuf reicht für kleine Vorschaubilder (wie controls.rs)
    Some(gdk::Texture::for_pixbuf(&pb.new_subpixbuf(0, top, w, h - top - bottom)))
}

/// Dicke der dunklen Balken oben und unten, je höchstens 30 % (sonst ist das Bild einfach dunkel).
/// Ein Pixel mehr, weil die Kante im JPEG verschmiert.
fn bars(h: i32, dark: impl Fn(i32) -> bool) -> (i32, i32) {
    let max = h * 3 / 10;
    let count = |rows: &mut dyn Iterator<Item = i32>| match rows.take_while(|&y| dark(y)).count() as i32 {
        0 => 0,
        n if n >= max => 0,
        n => n + 1,
    };
    (count(&mut (0..max)), count(&mut (0..max).map(|y| h - 1 - y)))
}

/// Nach so vielen Reihen Vorschlägen kommt auf der Startseite „Weiterschauen“.
const ROWS_BEFORE_SHELF: usize = 4;
/// Höchstens so viele Videos in „Weiterschauen“.
const SHELF_MAX: usize = 3;

thread_local! {
    /// Gewählte Spaltenzahl, gilt für alle Raster (Startseite, Kanal); gespeichert in `~/.config/tubequ/columns`.
    static WANT: Cell<Option<usize>> = Cell::new(
        std::fs::read_to_string(qu_core::app::config_file("columns")).ok().and_then(|s| s.trim().parse().ok()),
    );
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
    /// Graue Platzhalter-Karten beim Laden (oben) und beim Nachladen (unten).
    ghost: gtk::FlowBox,
    footer: gtk::Label,
    footer_ghost: gtk::FlowBox,
    ghost_thumbs: RefCell<Vec<gtk::Box>>,
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
    /// Zuletzt gemessene Breite (ohne Seitenränder), um bei geänderter Spaltenwahl neu aufzuteilen.
    width: Cell<i32>,
    /// Breite einer Spalte, damit „Weiterschauen“-Karten so breit sind wie die im Raster.
    column_width: Cell<i32>,
    /// Wird bei jedem neuen Inhalt erhöht, damit verspätete Antworten alter Suchen verworfen werden.
    generation: Cell<u64>,
    /// Startet die zuletzt gezeigte Anfrage erneut (für Reload).
    last: RefCell<Option<Rc<dyn Fn(&Rc<Grid>, Option<Page>)>>>,
}

impl Grid {
    /// Fester Abstand über dem Raster (statt Platz für die Navigation), für Seiten mit eigenem Kopf.
    /// Außerhalb des Scrollbereichs, sonst scrollt er mit weg und die Videos kleben am Kopf.
    pub fn set_top_margin(&self, px: i32) {
        if let Some(content) = self.top.parent() {
            content.set_margin_top(0);
        }
        self.widget.set_margin_top(px);
    }

    pub fn new(on_play: impl Fn(Video) + 'static, on_channel: impl Fn(String) + 'static) -> Rc<Self> {
        // ponytail: FlowBox baut alle Karten; bei mehreren hundert Videos auf GridView umstellen.
        let top = flowbox();
        let rest = flowbox();
        // Laden: graue Platzhalter-Karten. Die Labels zeigen nur Fehler oder „keine Ergebnisse“.
        let status = gtk::Label::builder().visible(false).margin_top(48).build();
        let ghost = flowbox();
        ghost.set_can_target(false);
        ghost.set_visible(false);
        let footer = gtk::Label::builder().visible(false).margin_top(12).margin_bottom(24).css_classes(["dim-label"]).build();
        let footer_ghost = flowbox();
        footer_ghost.set_can_target(false);
        footer_ghost.set_visible(false);
        footer_ghost.set_margin_top(24);
        footer_ghost.set_margin_bottom(24);

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
            // Platz für die schwebende Navigation oben links (12 Rand + 38 Bubble + 6).
            .margin_top(TOP_MARGIN)
            .margin_start(16)
            .margin_end(16)
            .build();
        content.append(&ghost);
        content.append(&status);
        content.append(&top);
        content.append(&shelf);
        content.append(&rest);
        content.append(&footer_ghost);
        content.append(&footer);
        let scroll = gtk::ScrolledWindow::builder()
            .child(&content)
            .hscrollbar_policy(gtk::PolicyType::Never)
            .vexpand(true)
            .build();
        qu_core::smooth::attach(&scroll);
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
            ghost,
            footer,
            footer_ghost,
            ghost_thumbs: RefCell::default(),
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
            width: Cell::new(0),
            column_width: Cell::new(qu_core::config().grid.card_width),
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
                if let Some(grid) = grid.upgrade() {
                    // Abzüglich der Seitenränder des Inhalts (je 16 px).
                    grid.width.set(page_width - 32);
                    grid.fit();
                }
            }
        });

        // Strg+Mausrad: eine Spalte mehr/weniger (vor dem weichen Scrollen der Seite).
        let zoom_ctl = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        zoom_ctl.set_propagation_phase(gtk::PropagationPhase::Capture);
        zoom_ctl.connect_scroll({
            let grid = Rc::downgrade(&grid);
            move |c, _, dy| {
                let Some(grid) = grid.upgrade() else { return glib::Propagation::Proceed };
                if !c.current_event_state().contains(gdk::ModifierType::CONTROL_MASK) {
                    return glib::Propagation::Proceed;
                }
                // ponytail: Touchpad (Pixel statt Rasten) wird nur geschluckt; aufsummieren, falls gewünscht.
                if c.unit() == gdk::ScrollUnit::Wheel && dy != 0.0 {
                    let n = step_columns(grid.columns.get(), dy);
                    WANT.set(Some(n));
                    let f = qu_core::app::config_file("columns");
                    let _ = std::fs::create_dir_all(f.parent().unwrap());
                    let _ = std::fs::write(f, n.to_string());
                    grid.fit();
                }
                glib::Propagation::Stop
            }
        });
        grid.widget.add_controller(zoom_ctl);

        // Eine Bildschirmhöhe vor dem Ende: nächste Seite holen, damit man beim Scrollen nicht am Ende
        // hängen bleibt. Weiter vorher nicht: jede Seite ist eine Anfrage an YouTube.
        grid.scroll.vadjustment().connect_value_changed({
            let grid = Rc::downgrade(&grid);
            move |adj| {
                if adj.value() > 0.0
                    && adj.value() + 2.0 * adj.page_size() >= adj.upper()
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
                    // Spaltenwahl kann sich auf einer anderen Seite (anderes Raster) geändert haben.
                    grid.fit();
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
        // http statt https: geht direkt an den Browser, nicht über die Tubequ-Link-Weiche zurück in die App.
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

    /// Spaltenzahl und Bildhöhen aus der gemessenen Breite und der Spaltenwahl.
    fn fit(self: &Rc<Self>) {
        let width = self.width.get();
        if width <= 0 {
            return;
        }
        let cols = columns(width, WANT.get());
        let col_width = (width - (cols as i32 - 1) * COLUMN_SPACING) / cols as i32;
        if (col_width - self.column_width.get()).abs() > 4 {
            self.column_width.set(col_width);
            for pic in self.pics.borrow().iter().chain(self.shelf_pics.borrow().iter()) {
                pic.set_height_request(image_height(col_width));
            }
            for t in self.ghost_thumbs.borrow().iter() {
                t.set_height_request(image_height(col_width));
            }
        }
        if cols != self.columns.get() {
            self.columns.set(cols);
            // Nicht mitten in der Größenberechnung umbauen, sondern direkt danach.
            let grid = self.clone();
            glib::idle_add_local_once(move || grid.layout());
        }
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
        let run: Rc<dyn Fn(&Rc<Grid>, Option<Page>)> = Rc::new(move |g: &Rc<Grid>, saved| g.run(make(), saved));
        self.last.replace(Some(run.clone()));
        run(self, None);
    }

    /// Wie `show`, zeigt aber `saved` (z. B. die letzte Startseite) sofort und tauscht sie gegen das
    /// neue Ergebnis aus, sobald es da ist. Scheitert die Anfrage, bleibt das Gespeicherte stehen.
    pub fn show_saved<F>(self: &Rc<Self>, saved: Option<Page>, make: impl Fn() -> F + 'static)
    where
        F: Future<Output = Result<Page, String>> + Send + 'static,
    {
        let make = Rc::new(make);
        let again = make.clone();
        self.last.replace(Some(Rc::new(move |g: &Rc<Grid>, saved| g.run(again(), saved))));
        self.run(make(), saved);
    }

    /// Letzte Anfrage neu ausführen, am Zwischenspeicher vorbei (zählt als Anfrage).
    pub fn reload(self: &Rc<Self>) {
        let last = self.last.borrow().clone();
        if let Some(run) = last {
            youtube::forget_cache();
            // Bisheriges bleibt stehen, bis das Neue da ist (Lade-Kreis in der Bubble).
            let videos = self.videos.borrow().clone();
            let current = (!videos.is_empty()).then(|| Page { videos, next: self.next.borrow().clone() });
            run(self, current);
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
        // Platzhalter einer abgelösten Anfrage weg (deren Antwort wird verworfen und räumt nicht mehr auf).
        self.hide_ghost(&self.ghost);
        self.hide_ghost(&self.footer_ghost);
        self.scroll.vadjustment().set_value(0.0);
        self.loading.set(true);
        // Gespeichertes sofort zeigen; sonst Platzhalter-Karten bis zur Antwort.
        let has_saved = saved.is_some();
        match saved {
            Some(p) => self.append(p),
            None => self.show_ghost(&self.ghost, None),
        }

        let grid = self.clone();
        let busy = crate::Busy::new();
        glib::spawn_future_local(async move {
            let _busy = busy;
            let res = bg(fut).await;
            if grid.generation.get() != generation {
                return;
            }
            grid.loading.set(false);
            grid.hide_ghost(&grid.ghost);
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

    /// Platzhalter-Karten in `flow` (Spaltenzahl und -breite wie das Raster). `rows` = None: so viele
    /// Reihen, dass die sichtbare Höhe gefüllt ist.
    fn show_ghost(&self, flow: &gtk::FlowBox, rows: Option<usize>) {
        let (cols, width) = (self.columns.get(), self.column_width.get());
        // ponytail: ~90 px Text unter dem Bild + Reihenabstand geschätzt.
        // Beim App-Start hat das Fenster noch keine Höhe: dann 4 Reihen.
        let h = self.scroll.height();
        let rows = rows.unwrap_or(if h > 0 { (h / (image_height(width) + 90).max(1)) as usize + 1 } else { 4 }).max(2);
        flow.set_min_children_per_line(cols as u32);
        flow.set_max_children_per_line(cols as u32);
        flow.remove_all();
        for _ in 0..rows * cols {
            let (card, thumb) = ghost_card(width);
            flow.append(&card);
            self.ghost_thumbs.borrow_mut().push(thumb);
        }
        flow.set_visible(true);
    }

    fn hide_ghost(&self, flow: &gtk::FlowBox) {
        flow.remove_all();
        flow.set_visible(false);
        // Nur leeren, wenn beide weg sind (sonst gehen die Bildhöhen des anderen verloren).
        if !self.ghost.is_visible() && !self.footer_ghost.is_visible() {
            self.ghost_thumbs.borrow_mut().clear();
        }
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
        self.show_ghost(&self.footer_ghost, Some(1));

        let generation = self.generation.get();
        let grid = self.clone();
        let busy = crate::Busy::new();
        glib::spawn_future_local(async move {
            let _busy = busy;
            let res = bg(youtube::more(m.clone())).await;
            if grid.generation.get() != generation {
                return;
            }
            grid.loading.set(false);
            grid.hide_ghost(&grid.footer_ghost);
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

    fn cfg() {
        qu_core::config::load(include_str!("../qu.toml"));
    }

    #[test]
    fn bars_found() {
        cfg();
        // 360 hoch, oben 45 und unten 60 dunkle Zeilen.
        assert_eq!(bars(360, |y| y < 45 || y >= 300), (46, 61));
        assert_eq!(bars(360, |_| false), (0, 0));
        // Ganz dunkles Bild: nichts abschneiden.
        assert_eq!(bars(360, |_| true), (0, 0));
    }

    #[test]
    fn formats() {
        cfg();
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
        assert_eq!(columns(300, None), 1);
        assert_eq!(columns(1000, None), 2);
        assert_eq!(columns(1100, None), 3);
        assert_eq!(columns(2528, None), 4);
        // Gewählt: so viele, wie 200-px-Karten passen.
        assert_eq!(columns(2528, Some(6)), 6);
        assert_eq!(columns(1100, Some(6)), 4);
        assert_eq!(step_columns(4, -1.0), 5);
        assert_eq!(step_columns(6, -1.0), 6);
        assert_eq!(step_columns(3, 1.0), 3);
        assert_eq!(step_columns(1, -1.0), 3);
        assert_eq!(image_height(640), 360);
    }
}

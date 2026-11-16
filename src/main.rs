mod channel;
mod controls;
mod fly;
mod grid;
mod icons;
mod lists;
mod nav_shape;
mod player;
mod progress;
mod search;
mod smooth;
mod sponsorblock;
mod theme;
mod watch;
mod youtube;

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::LazyLock;
use youtube::Video;

pub static RT: LazyLock<tokio::runtime::Runtime> =
    LazyLock::new(|| tokio::runtime::Runtime::new().expect("tokio"));

/// Führt `fut` im Hintergrund (tokio) aus; das Ergebnis kommt im GTK-Thread an.
pub async fn bg<T: Send + 'static>(fut: impl Future<Output = T> + Send + 'static) -> T {
    RT.spawn(fut).await.expect("Hintergrund-Task abgestürzt")
}

thread_local! {
    static BUSY_COUNT: std::cell::Cell<u32> = const { std::cell::Cell::new(0) };
    /// Lade-Kreis in der Nav-Bubble (neben der Suche).
    static BUSY_ICON: std::cell::OnceCell<gtk::Image> = const { std::cell::OnceCell::new() };
}

/// Solange ein `Busy` lebt, dreht sich der Lade-Kreis in der Nav-Bubble. Für jedes Laden
/// (Seiten, Nachladen, Video-Details) eins anlegen und in den Ladevorgang mitnehmen.
pub struct Busy(());

impl Busy {
    pub fn new() -> Self {
        BUSY_COUNT.with(|c| c.set(c.get() + 1));
        busy_update();
        Busy(())
    }
}

impl Drop for Busy {
    fn drop(&mut self) {
        BUSY_COUNT.with(|c| c.set(c.get().saturating_sub(1)));
        busy_update();
    }
}

fn busy_update() {
    let on = BUSY_COUNT.with(|c| c.get()) > 0;
    BUSY_ICON.with(|i| {
        if let Some(i) = i.get() {
            i.set_visible(on);
        }
    });
}

/// Seitenwechsel mit Verlauf für den Zurück-Button: "browse", "channel", "watch".
struct Nav {
    stack: gtk::Stack,
    back: gtk::Button,
    window: gtk::ApplicationWindow,
    player: Rc<player::Player>,
    history: RefCell<Vec<String>>,
    /// Seiten, von denen man mit „Zurück“ gekommen ist (Maustaste 5 = vor). Neue Wege leeren sie.
    forward: RefCell<Vec<String>>,
    /// Schwebende Knöpfe oben links. Auf Übersicht/Kanal immer sichtbar über der Seite;
    /// beim Video sitzen sie im Player und blenden sich mit dessen Leiste aus und ein.
    nav_bar: gtk::Revealer,
    page_layer: gtk::Overlay,
    in_player: std::cell::Cell<bool>,
    watch: Rc<watch::WatchPage>,
    /// Umgibt die Ansehen-Seite für die Öffnen-/Miniplayer-Animation.
    zoom: fly::Zoom,
    /// Knöpfe im Miniplayer (Pause, Ton, Zurück zum Video, Schließen).
    mini_bar: gtk::Box,
}

impl Nav {
    fn current(&self) -> String {
        self.stack.visible_child_name().map(|s| s.to_string()).unwrap_or_default()
    }

    fn show(&self, page: &str) {
        let watch = page == "watch";
        // Ansehen-Seite erscheint (neu oder aus dem Miniplayer): kurz einblenden.
        let appear = watch && (self.current() != "watch" || self.watch.is_mini());
        // Verlässt man die Ansehen-Seite, läuft ein laufendes Video im Miniplayer weiter, sonst stoppt es.
        if self.current() == "watch" && !watch {
            self.window.unfullscreen();
            if self.player.active() {
                self.set_mini(true);
            } else {
                self.close_mini();
            }
        }
        if watch {
            self.set_mini(false);
        } else if self.watch.is_mini() && !self.player.active() {
            // Im Miniplayer zu Ende gelaufen (z. B. „Home“ im Endbildschirm): weg damit.
            self.close_mini();
        }
        self.watch.widget.set_visible(watch || self.watch.is_mini());
        if appear && !self.zoom.animating() {
            // Die CSS-Animation startet nur neu, wenn sich ihr Name ändert: zwischen a und b wechseln.
            let w = &self.watch.widget;
            let a = w.has_css_class("page-in-a");
            w.remove_css_class(if a { "page-in-a" } else { "page-in-b" });
            w.add_css_class(if a { "page-in-b" } else { "page-in-a" });
        }
        // Von/zur Ansehen-Seite ohne Überblendung: die liegt als eigene Ebene darüber (Zoom-Animation),
        // im Stack steht nur ein leerer Platzhalter. Mit Überblendung wäre es beim Schrumpfen dahinter schwarz.
        let fade = !watch && self.current() != "watch";
        self.stack.set_transition_type(if fade { gtk::StackTransitionType::Crossfade } else { gtk::StackTransitionType::None });
        self.stack.set_visible_child_name(page);
        self.back.set_visible(!self.history.borrow().is_empty());

        if watch != self.in_player.get() {
            let (from, to) = if watch { (&self.page_layer, &self.player.widget) } else { (&self.player.widget, &self.page_layer) };
            from.remove_overlay(&self.nav_bar);
            to.add_overlay(&self.nav_bar);
            self.in_player.set(watch);
        }
        self.nav_bar.set_reveal_child(!watch || self.player.controls.widget.reveals_child());
    }

    fn set_mini(&self, on: bool) {
        // Wechsel groß ↔ klein bei laufendem Video: die Seite schrumpft/wächst vom alten Platz aus.
        let from = (self.watch.is_mini() != on && self.player.active()).then(|| self.zoom.anchor_bounds()).flatten();
        // Beim Großwerden bleibt die Seite darunter (Raster) stehen, bis die Animation fertig ist.
        let backdrop = from.filter(|_| !on).and_then(|_| self.zoom.backdrop(&self.stack));
        self.watch.set_mini(on);
        if let Some(from) = from {
            self.zoom.animate_from(from, backdrop);
        }
        self.mini_bar.set_visible(on);
        // Die große Leiste passt nicht in den kleinen Player.
        self.player.controls.widget.set_visible(!on);
    }

    /// Miniplayer schließen bzw. Video beenden (speichert die Position).
    fn close_mini(&self) {
        // Das Video hat den Tastatur-Fokus. Verschwindet es damit, gibt GTK den Fokus der ersten Karte,
        // und die Liste springt nach oben. Darum vorher den Fokus abgeben.
        gtk::prelude::GtkWindowExt::set_focus(&self.window, None::<&gtk::Widget>);
        self.player.stop();
        self.set_mini(false);
        self.watch.widget.set_visible(self.current() == "watch");
        self.window.set_title(Some("Tubequ"));
    }

    fn go(&self, page: &str) {
        let cur = self.current();
        if cur != page {
            self.history.borrow_mut().push(cur);
            self.forward.borrow_mut().clear();
        }
        self.show(page);
    }

    fn back(&self) {
        let prev = self.history.borrow_mut().pop();
        if let Some(p) = prev {
            self.forward.borrow_mut().push(self.current());
            self.show(&p);
        }
    }

    /// Gegenstück zu `back`. ponytail: merkt nur die Seite, nicht ihren Inhalt – die Seiten gibt es je
    /// einmal und sie zeigen noch, was zuletzt drin war (Kanal, Video).
    fn forward(&self) {
        let next = self.forward.borrow_mut().pop();
        if let Some(p) = next {
            self.history.borrow_mut().push(self.current());
            self.show(&p);
        }
    }

    fn home(&self) {
        self.history.borrow_mut().clear();
        self.forward.borrow_mut().clear();
        self.show("browse");
    }
}

/// Video-ID aus einem YouTube-Link (watch?v=…, youtu.be/…, shorts/…).
fn video_id(url: &str) -> Option<String> {
    let rest = url.split_once("v=").or_else(|| url.split_once("youtu.be/")).or_else(|| url.split_once("/shorts/"))?.1;
    let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    (id.len() == 11).then_some(id)
}

/// Läuft die App unter einem Tiling-Window-Manager?
/// ponytail: kurze Liste bekannter Tiling-WMs; erweitern, wenn jemand einen anderen nutzt.
fn tiling_wm() -> bool {
    let desktop = std::env::var("XDG_CURRENT_DESKTOP").unwrap_or_default().to_lowercase();
    ["hyprland", "sway", "niri", "river", "i3"].iter().any(|wm| desktop.contains(wm))
}

/// Profilbild (über `youtube::avatar`, auf der Platte zwischengespeichert) und Tooltip eines Kontos.
fn show_account(pic: &gtk::Image, button: &gtk::Button, a: &youtube::Account) {
    button.set_tooltip_text(Some(&format!("{}\n{}", a.name, a.email)));
    pic.set_icon_name(Some("tubequ-account-symbolic"));
    let Some(url) = a.photo.clone() else { return };
    let pic = pic.downgrade();
    glib::spawn_future_local(async move {
        let Ok(bytes) = bg(youtube::avatar(url)).await else { return };
        let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)) else { return };
        if let Some(pic) = pic.upgrade() {
            pic.set_paintable(Some(&tex));
        }
    });
}

fn build(app: &gtk::Application) -> Rc<dyn Fn(Option<String>)> {
    let display = gdk::Display::default().expect("Display");
    let css = gtk::CssProvider::new();
    css.load_from_string(include_str!("style.css"));
    // Eine Stufe über dem Theme (theme.rs): gezielte Regeln wie die Player-Bubbles sollen gewinnen,
    // GTK entscheidet bei gleicher Priorität nach Ladereihenfolge, nicht nach Genauigkeit.
    gtk::style_context_add_provider_for_display(&display, &css, gtk::STYLE_PROVIDER_PRIORITY_APPLICATION + 1);
    theme::install(&display);
    icons::install(&display);

    let player = Rc::new(player::Player::new());

    // Keine Kopfzeile: Zurück, Home und Suche schweben oben links in einer Bubble.
    let nav_button = |icon: &str, tip: &str| {
        gtk::Button::builder().icon_name(icon).tooltip_text(tip).focusable(false).css_classes(["flat"]).build()
    };
    let back = nav_button("tubequ-back-symbolic", "Back (Esc)");
    back.set_visible(false);
    let home = nav_button("tubequ-home-symbolic", "Home");
    let search_button = nav_button("tubequ-search-symbolic", "Search (/)");
    let history_button = nav_button("tubequ-history-symbolic", "History");
    let later_button = nav_button("tubequ-later-symbolic", "Watch later");
    // Oben: Burger, Zurück, Home, Suche. Der Burger klappt die Bubble nach unten auf (übrige Seiten).
    let menu_button = nav_button("tubequ-menu-symbolic", "More");
    let top = gtk::Box::builder().spacing(2).height_request(32).build();
    top.append(&menu_button);
    top.append(&back);
    top.append(&home);
    top.append(&search_button);
    let busy_icon = icons::spinner(16).visible(false).margin_start(6).margin_end(6).build();
    BUSY_ICON.with(|i| i.set(busy_icon.clone()).ok());
    top.append(&busy_icon);
    let extra = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).halign(gtk::Align::Start).build();
    // Links bündig: sonst wachsen die Knöpfe mit, wenn die Kontoliste die Spalte breiter macht.
    for b in [&later_button, &history_button] {
        b.set_halign(gtk::Align::Start);
        extra.append(b);
    }
    // Konto (nur mit Browser-Login): Profilbild des aktiven Kontos unten in der Spalte. Klick klappt
    // daneben die übrigen Konten nach rechts auf (Bubble wird zum liegenden U).
    let account_pic = gtk::Image::builder()
        .icon_name("tubequ-account-symbolic")
        .pixel_size(22)
        // Genau so groß wie das Bild, sonst schneidet die Rundung nur die Ecken eines größeren Felds.
        .halign(gtk::Align::Center)
        .valign(gtk::Align::Center)
        .overflow(gtk::Overflow::Hidden)
        .css_classes(["avatar"])
        .build();
    let account_button = gtk::Button::builder().child(&account_pic).tooltip_text("Account").focusable(false).css_classes(["flat"]).build();
    let other_accounts = gtk::Box::builder().spacing(2).margin_start(2).build();
    let accounts_menu = gtk::Revealer::builder().child(&other_accounts).transition_type(gtk::RevealerTransitionType::SlideRight).build();
    let account_row = gtk::Box::builder().halign(gtk::Align::Start).visible(youtube::COOKIES.is_some()).build();
    account_row.append(&account_button);
    account_row.append(&accounts_menu);
    extra.append(&account_row);
    // Links ausgerichtet: nav_shape.rs misst an der Revealer-Breite, wie breit die Spalte ist.
    let menu = gtk::Revealer::builder().child(&extra).halign(gtk::Align::Start).transition_type(gtk::RevealerTransitionType::SlideDown).build();
    extra.set_margin_top(2);
    let nav_box = nav_shape::NavShape::new();
    nav_box.set_css_classes(&["bubble", "nav-shape"]);
    nav_box.append(&top);
    nav_box.append(&menu);
    nav_box.set_parts(&later_button, &account_row);
    // Oben nur so breit wie die Knöpfe: sonst zieht eine breite Kontoliste die obere Reihe mit.
    top.set_halign(gtk::Align::Start);
    let set_menu: Rc<dyn Fn(bool)> = Rc::new({
        let (menu, accounts_menu) = (menu.clone(), accounts_menu.clone());
        move |open| {
            menu.set_reveal_child(open);
            if !open {
                accounts_menu.set_reveal_child(false);
            }
        }
    });
    menu_button.connect_clicked({
        let (menu, set_menu) = (menu.clone(), set_menu.clone());
        move |_| set_menu(!menu.reveals_child())
    });
    // Nach der Auswahl wieder zuklappen.
    for b in [&later_button, &history_button, &home, &search_button] {
        let set_menu = set_menu.clone();
        b.connect_clicked(move |_| set_menu(false));
    }
    let nav_bar = gtk::Revealer::builder()
        .child(&nav_box)
        .transition_type(gtk::RevealerTransitionType::Crossfade)
        .reveal_child(true)
        .halign(gtk::Align::Start)
        .valign(gtk::Align::Start)
        .css_classes(["nav-bubble"])
        .build();

    // Nicht homogen: sonst bestimmt die breiteste (auch unsichtbare) Seite die Mindestbreite aller Seiten.
    let stack = gtk::Stack::builder()
        .transition_type(gtk::StackTransitionType::Crossfade)
        .hhomogeneous(false)
        .vhomogeneous(false)
        .build();
    // Ebene 1: Seiten plus Navigation. Ebene 2 (darüber): Suchfenster und Fenster-Knöpfe.
    let page_layer = gtk::Overlay::builder().child(&stack).build();
    page_layer.add_overlay(&nav_bar);
    let root = gtk::Overlay::builder().child(&page_layer).build();
    // Unter Tiling-Window-Managern schließt man Fenster per Tastenkürzel (z. B. Super+W): keine
    // Fenster-Knöpfe. Sonst (GNOME, KDE, Windows …) oben rechts in einer eigenen Bubble.
    if !tiling_wm() {
        let controls = gtk::WindowControls::builder().side(gtk::PackType::End).css_classes(["bubble"]).build();
        let wc = gtk::Box::builder().halign(gtk::Align::End).valign(gtk::Align::Start).css_classes(["nav-bubble"]).build();
        wc.append(&controls);
        root.add_overlay(&wc);
    }

    let window = gtk::ApplicationWindow::builder()
        .application(app)
        .title("Tubequ")
        .default_width(1280)
        .default_height(860)
        .decorated(false)
        .build();
    // Klick neben das offene Menü klappt es zu. Der Klick geht trotzdem weiter an sein Ziel.
    let outside = gtk::GestureClick::builder().button(0).propagation_phase(gtk::PropagationPhase::Capture).build();
    outside.connect_pressed({
        let (window, nav_box, menu, set_menu) = (window.clone(), nav_box.clone(), menu.clone(), set_menu.clone());
        move |_, _, x, y| {
            let inside = window.pick(x, y, gtk::PickFlags::DEFAULT).is_some_and(|w| w.is_ancestor(&nav_box));
            if menu.reveals_child() && !inside {
                set_menu(false);
            }
        }
    });
    window.add_controller(outside);
    if tiling_wm() {
        window.set_child(Some(&root));
    } else {
        // Ohne Titelleiste: an freien Flächen ziehen verschiebt das Fenster.
        window.set_child(Some(&gtk::WindowHandle::builder().child(&root).build()));
    }

    // Kanalseite öffnen. Wird erst unten befüllt, weil Kanalseite und Ansehen-Seite sich gegenseitig brauchen.
    let open_channel_cell: Rc<std::cell::OnceCell<Rc<dyn Fn(String)>>> = Rc::default();
    let open_channel_late: Rc<dyn Fn(String)> = Rc::new({
        let cell = open_channel_cell.clone();
        move |id| {
            if let Some(f) = cell.get() {
                f(id)
            }
        }
    });

    // Ansehen-Seite: Video oben, darunter Titel, Kanal, Beschreibung.
    // Links in der Beschreibung: Zeitstempel springen, Kanäle und Videos öffnen in der App,
    // alles andere im Browser. `play_late` wird unten befüllt (braucht die Ansehen-Seite selbst).
    let play_cell: Rc<std::cell::OnceCell<Rc<dyn Fn(Video)>>> = Rc::default();
    let on_link: Rc<dyn Fn(String)> = Rc::new({
        let (player, open_channel, play_cell, window) =
            (player.clone(), open_channel_late.clone(), play_cell.clone(), window.clone());
        move |uri: String| {
            if let Some(sec) = uri.strip_prefix("seek:") {
                let _ = player.mpv.command("seek", &[sec, "absolute"]);
                player.controls.poke();
            } else if let Some(id) = uri.strip_prefix("channel:") {
                open_channel(id.to_string());
            } else if let (Some(id), Some(play)) = (video_id(&uri).filter(|_| uri.contains("youtube.com/watch")), play_cell.get()) {
                play(Video { id, ..Default::default() });
            } else {
                gtk::UriLauncher::new(&uri).launch(Some(&window), gio::Cancellable::NONE, |r| {
                    if let Err(e) = r {
                        eprintln!("Link nicht geöffnet: {e}");
                    }
                });
            }
        }
    });
    let watch = watch::WatchPage::new(&player.widget, open_channel_late.clone(), on_link);

    // Miniplayer-Knöpfe in den Ecken, je eine Bubble: oben links groß machen, oben rechts schließen,
    // unten links Play und Ton. Die Symbole folgen den Knöpfen der großen Leiste.
    let mini_bar = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .margin_start(8)
        .margin_end(8)
        .margin_top(8)
        .margin_bottom(8)
        .visible(false)
        .build();
    let bubble = || gtk::Box::builder().spacing(2).height_request(32).css_classes(["bubble"]).build();
    let (mini_top_left, mini_top_right, mini_bottom) = (bubble(), bubble(), bubble());
    let mini_play = nav_button("tubequ-pause-symbolic", "Play/Pause");
    player.controls.play.bind_property("icon-name", &mini_play, "icon-name").sync_create().build();
    let mini_mute = nav_button("tubequ-volume-symbolic", "Mute");
    player.controls.mute.bind_property("icon-name", &mini_mute, "icon-name").sync_create().build();
    let mini_open = nav_button("tubequ-fullscreen-symbolic", "Back to video");
    let mini_close = nav_button("tubequ-close-symbolic", "Close");
    mini_top_left.append(&mini_open);
    mini_top_right.append(&mini_close);
    mini_bottom.append(&mini_play);
    mini_bottom.append(&mini_mute);
    let top = gtk::CenterBox::builder().start_widget(&mini_top_left).end_widget(&mini_top_right).valign(gtk::Align::Start).vexpand(true).build();
    mini_bottom.set_halign(gtk::Align::Start);
    mini_bottom.set_valign(gtk::Align::End);
    mini_bar.append(&top);
    mini_bar.append(&mini_bottom);
    mini_play.connect_clicked({
        let mpv = player.mpv;
        move |_| {
            let _ = mpv.command("cycle", &["pause"]);
        }
    });
    mini_mute.connect_clicked({
        let controls = player.controls.clone();
        move |_| controls.toggle_mute()
    });
    watch.widget.add_overlay(&mini_bar);
    // Über den Seiten, unter Navigation (wird beim Seitenwechsel neu obenauf gelegt) und Suche.
    // In `Zoom` verpackt: geht beim Öffnen aus der Karte auf, Miniplayer ebenso (fly.rs).
    let zoom = fly::Zoom::new(&watch.widget, &player.widget);
    page_layer.add_overlay(&zoom);
    // Miniplayer ziehen; beim Loslassen gleitet er in die nächste Ecke.
    let drag = gtk::GestureDrag::new();
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    let dragging = Rc::new(Cell::new(false));
    drag.connect_drag_update({
        let (watch, zoom, dragging) = (watch.clone(), zoom.clone(), dragging.clone());
        move |g, dx, dy| {
            if !watch.is_mini() {
                return;
            }
            // Erst ab ein paar Pixeln ist es Ziehen; sonst bleibt es ein Klick (Knöpfe im Miniplayer).
            if !dragging.get() && dx.hypot(dy) < 8.0 {
                return;
            }
            if !dragging.get() {
                dragging.set(true);
                g.set_state(gtk::EventSequenceState::Claimed);
            }
            zoom.set_offset(dx as f32, dy as f32);
        }
    });
    // Loslassen nach Ziehen um (dx, dy): in die nächste Ecke gleiten.
    let snap: Rc<dyn Fn(f32, f32)> = Rc::new({
        let (watch, zoom) = (watch.clone(), zoom.clone());
        move |dx, dy| {
            zoom.set_offset(0.0, 0.0);
            let (Some(player), Some(page)) = (zoom.anchor_bounds(), watch.widget.compute_bounds(&zoom)) else { return };
            // Nächste Ecke = Seite, auf der die Mitte des Miniplayers jetzt liegt.
            let right = page.x() + dx + page.width() / 2.0 > zoom.width() as f32 / 2.0;
            let bottom = page.y() + dy + page.height() / 2.0 > zoom.height() as f32 / 2.0;
            watch.set_corner(right, bottom);
            let from = gtk::graphene::Rect::new(player.x() + dx, player.y() + dy, player.width(), player.height());
            zoom.animate_from(from, None);
        }
    });
    drag.connect_drag_end({
        let snap = snap.clone();
        move |_, dx, dy| {
            if dragging.replace(false) {
                snap(dx as f32, dy as f32);
            }
        }
    });
    watch.widget.add_controller(drag);

    let nav = Rc::new(Nav {
        stack: stack.clone(),
        back: back.clone(),
        window: window.clone(),
        player: player.clone(),
        history: RefCell::default(),
        forward: RefCell::default(),
        nav_bar: nav_bar.clone(),
        page_layer: page_layer.clone(),
        in_player: std::cell::Cell::new(false),
        watch: watch.clone(),
        zoom: zoom.clone(),
        mini_bar: mini_bar.clone(),
    });
    mini_open.connect_clicked({
        let nav = nav.clone();
        move |_| nav.go("watch")
    });
    mini_close.connect_clicked({
        let nav = nav.clone();
        move |_| nav.close_mini()
    });
    // Beim Video: Navigation blendet sich zusammen mit der Player-Leiste ein und aus.
    // Offenes Menü hält die Navigation sichtbar, auch wenn sich die Leiste ausblendet.
    player.controls.widget.connect_reveal_child_notify({
        let (nav, menu) = (nav.clone(), menu.clone());
        move |r| {
            if nav.in_player.get() {
                nav.nav_bar.set_reveal_child(r.reveals_child() || menu.reveals_child());
            }
        }
    });
    menu.connect_reveal_child_notify({
        let (nav, player) = (nav.clone(), player.clone());
        move |m| {
            if nav.in_player.get() {
                nav.nav_bar.set_reveal_child(m.reveals_child() || player.controls.widget.reveals_child());
            }
        }
    });


    let play: Rc<dyn Fn(Video)> = Rc::new({
        let (nav, player, watch) = (nav.clone(), player.clone(), watch.clone());
        move |v: Video| {
            // Seite geht aus dem Vorschaubild der angeklickten Karte auf (vor dem Seitenwechsel messen).
            let from = fly::thumb_bounds(&v.id, &nav.zoom);
            let backdrop = from.and_then(|_| nav.zoom.backdrop(&nav.stack));
            watch.show(&v);
            nav.go("watch");
            if let Some(from) = from {
                // Nach `go`: übernimmt auch einen Miniplayer-Übergang; die Einblendung von `show` braucht es dann nicht.
                nav.zoom.animate_from(from, backdrop);
                nav.watch.widget.remove_css_class("page-in-a");
                nav.watch.widget.remove_css_class("page-in-b");
            }
            // Fenstertitel = Videotitel (Alt-Tab, Fensterliste); „Tubequ“ bleibt hinten dran.
            if !v.title.is_empty() {
                nav.window.set_title(Some(&format!("{} – Tubequ", v.title)));
            }
            let (player, watch) = (player.clone(), watch.clone());
            glib::spawn_future_local(async move {
                // Das Laden des Streams (yt-dlp in mpv) zählt auch als Anfrage – und kommt als Erstes dran.
                // Details erst danach anstellen, sonst müsste der Stream 2 s auf sie warten.
                match bg(youtube::throttle()).await {
                    Ok(()) => player.play(Some(v.clone()), &v.url(), v.is_live),
                    Err(e) => return player.show_error(&e),
                }
                watch.load_details(v.id.clone());
                // Im Hintergrund im YouTube-Verlauf eintragen; Fehler sind hier nicht schlimm.
                if let Err(e) = bg(youtube::mark_watched(v.id)).await {
                    eprintln!("Als gesehen markieren fehlgeschlagen: {e}");
                }
            });
        }
    });

    let _ = play_cell.set(play.clone());

    let channel_page = channel::ChannelPage::new({
        let play = play.clone();
        move |v| play(v)
    });
    let open_channel: Rc<dyn Fn(String)> = Rc::new({
        let (nav, channel_page) = (nav.clone(), channel_page.clone());
        move |id| {
            channel_page.open(id);
            nav.go("channel");
        }
    });
    let _ = open_channel_cell.set(open_channel.clone());

    let grid = grid::Grid::new(
        {
            let play = play.clone();
            move |v| play(v)
        },
        {
            let open_channel = open_channel.clone();
            move |id| open_channel(id)
        },
    );
    stack.add_named(&grid.widget, Some("browse"));
    stack.add_named(&channel_page.widget, Some("channel"));
    // Platzhalter: die Ansehen-Seite selbst liegt als Ebene darüber (damit sie auch als Miniplayer geht).
    stack.add_named(&gtk::Box::new(gtk::Orientation::Horizontal, 0), Some("watch"));

    // Reload: aktuelle Seite neu laden (Startseite/Suche, Kanal oder Video).
    let reload_page: Rc<dyn Fn()> = Rc::new({
        let (nav, grid, channel_page, player) = (nav.clone(), grid.clone(), channel_page.clone(), player.clone());
        move || match nav.current().as_str() {
            "browse" => grid.reload(),
            "channel" => channel_page.reload(),
            "watch" => player.reload(),
            _ => {}
        }
    });
    player.retry.connect_clicked({
        let player = Rc::downgrade(&player);
        move |_| {
            if let Some(p) = player.upgrade() {
                p.reload();
            }
        }
    });

    back.connect_clicked({
        let nav = nav.clone();
        move |_| nav.back()
    });
    // Seitentasten der Maus (GTK: 8 = zurück, 9 = vor), überall im Fenster, auch über dem Player.
    let side_buttons = gtk::GestureClick::builder().button(0).propagation_phase(gtk::PropagationPhase::Capture).build();
    side_buttons.connect_pressed({
        let nav = nav.clone();
        move |g, _, _, _| match g.current_button() {
            8 => {
                g.set_state(gtk::EventSequenceState::Claimed);
                nav.back();
            }
            9 => {
                g.set_state(gtk::EventSequenceState::Claimed);
                nav.forward();
            }
            _ => {}
        }
    });
    window.add_controller(side_buttons);

    // Welche Liste das Raster zeigt ("home", "later", "history", sonst ""). Klick auf den Knopf der
    // Seite, auf der man schon ist, lädt sie neu (gilt für jeden Seiten-Knopf in der Bubble).
    let section: Rc<std::cell::Cell<&'static str>> = Rc::default();
    let nav_to = {
        let (nav, section, reload_page) = (nav.clone(), section.clone(), reload_page.clone());
        move |name: &'static str, open: Box<dyn Fn()>| {
            let (nav, section, reload_page) = (nav.clone(), section.clone(), reload_page.clone());
            move |_: &gtk::Button| {
                if nav.current() == "browse" && section.get() == name {
                    reload_page();
                } else {
                    nav.home();
                    section.set(name);
                    open();
                }
            }
        }
    };
    home.connect_clicked(nav_to("home", {
        let grid = grid.clone();
        // Gespeicherte Startseite sofort, die neue ersetzt sie (Lade-Kreis in der Bubble).
        Box::new(move || {
            grid.set_home(true);
            grid.show_saved(youtube::saved_home(), youtube::home);
        })
    }));

    // Verlauf und „Später ansehen“: lokale Listen im Raster (keine Anfragen an YouTube).
    history_button.connect_clicked(nav_to("history", {
        let grid = grid.clone();
        Box::new(move || {
            grid.set_home(false);
            grid.show(|| async { Ok(youtube::Page { videos: progress::history(), next: None }) });
        })
    }));
    later_button.connect_clicked(nav_to("later", {
        let grid = grid.clone();
        Box::new(move || {
            grid.set_home(false);
            grid.show(|| async { Ok(youtube::Page { videos: lists::later(), next: None }) });
        })
    }));

    // Aktives Konto beim Start: aus ~/.cache/tubequ/accounts.tsv, nur beim allerersten Mal eine Anfrage.
    if youtube::COOKIES.is_some() {
        let (pic, button) = (account_pic.downgrade(), account_button.downgrade());
        glib::spawn_future_local(async move {
            if let Ok(a) = bg(youtube::current_account()).await
                && let (Some(pic), Some(button)) = (pic.upgrade(), button.upgrade())
            {
                show_account(&pic, &button, &a);
            }
        });
    }
    // Klick aufs Profilbild: übrige Konten nach rechts aufklappen (erst dann geladen, einmal pro Sitzung).
    account_button.connect_clicked({
        let (accounts_menu, other_accounts, account_pic, home, set_menu) =
            (accounts_menu.clone(), other_accounts.clone(), account_pic.clone(), home.clone(), set_menu.clone());
        move |account_button| {
            let open = !accounts_menu.reveals_child();
            accounts_menu.set_reveal_child(open);
            if !open {
                return;
            }
            while let Some(c) = other_accounts.first_child() {
                other_accounts.remove(&c);
            }
            other_accounts.append(&icons::spinner(16).margin_start(6).margin_end(6).build());
            let (other_accounts, account_pic, account_button, home, set_menu) =
                (other_accounts.downgrade(), account_pic.downgrade(), account_button.downgrade(), home.downgrade(), set_menu.clone());
            glib::spawn_future_local(async move {
                let _busy = Busy::new();
                let res = bg(youtube::accounts()).await;
                let Some(list) = other_accounts.upgrade() else { return };
                while let Some(c) = list.first_child() {
                    list.remove(&c);
                }
                let accounts = match res {
                    Ok(a) => a,
                    Err(e) => {
                        list.append(&gtk::Label::new(Some(&e)));
                        return;
                    }
                };
                let others: Vec<_> = accounts.iter().filter(|a| a.index != youtube::account()).cloned().collect();
                if others.is_empty() {
                    list.append(&gtk::Label::new(Some("No other accounts")));
                }
                for a in others {
                    let pic = gtk::Image::builder()
                        .icon_name("tubequ-account-symbolic")
                        .pixel_size(22)
                        // Genau so groß wie das Bild, sonst schneidet die Rundung nur die Ecken eines größeren Felds.
                        .halign(gtk::Align::Center)
                        .valign(gtk::Align::Center)
                        .overflow(gtk::Overflow::Hidden)
                        .css_classes(["avatar"])
                        .build();
                    let b = gtk::Button::builder().child(&pic).focusable(false).css_classes(["flat"]).build();
                    show_account(&pic, &b, &a);
                    let (account_pic, account_button, home, set_menu) = (account_pic.clone(), account_button.clone(), home.clone(), set_menu.clone());
                    b.connect_clicked(move |_| {
                        youtube::set_account(a.index);
                        if let (Some(pic), Some(button)) = (account_pic.upgrade(), account_button.upgrade()) {
                            show_account(&pic, &button, &a);
                        }
                        set_menu(false);
                        if let Some(home) = home.upgrade() {
                            home.emit_clicked();
                        }
                    });
                    list.append(&b);
                }
            });
        }
    });

    // Schwebendes Suchfenster über allem.
    let search = search::SearchModal::new({
        let (grid, nav, section) = (grid.clone(), nav.clone(), section.clone());
        move |q| {
            nav.home();
            section.set("");
            grid.set_home(false);
            grid.show(move || youtube::search(q.clone()));
        }
    });
    root.add_overlay(&search.widget);
    search_button.connect_clicked({
        let search = search.clone();
        move |_| search.open()
    });
    // Home (auch der Endbildschirm-Knopf) verlässt die Suche: Feld beim nächsten Öffnen leer.
    home.connect_clicked({
        let search = search.clone();
        move |_| search.reset()
    });

    // Vollbild: Kopfzeile und Infos ausblenden, nur noch Video.
    player.controls.fullscreen.connect_clicked({
        let window = window.clone();
        move |_| {
            if window.is_fullscreen() { window.unfullscreen() } else { window.fullscreen() }
        }
    });
    window.connect_fullscreened_notify({
        let (watch, fs) = (watch.clone(), player.controls.fullscreen.clone());
        move |w| {
            let full = w.is_fullscreen();
            watch.set_fullscreen(full);
            fs.set_icon_name(if full { "tubequ-restore-symbolic" } else { "tubequ-fullscreen-symbolic" });
        }
    });
    // Player-Höhe an das Seitenverhältnis des Videos anpassen.
    // Endbildschirm „Zur Startseite“.
    player.end_home.connect_clicked({
        let home = home.clone();
        move |_| home.emit_clicked()
    });
    // Während ein Video läuft: kein Bildschirmschoner / keine Sperre (über GTK, also systemunabhängig).
    player.connect_playing({
        let (app, window) = (app.clone(), window.clone());
        let cookie = std::cell::Cell::new(0u32);
        move |playing| {
            if playing && cookie.get() == 0 {
                cookie.set(app.inhibit(Some(&window), gtk::ApplicationInhibitFlags::IDLE, Some("Playing a video")));
            } else if !playing && cookie.get() != 0 {
                app.uninhibit(cookie.replace(0));
            }
        }
    });

    // yt-dlp-Infos vom Player: Ersatz, falls die Details über rustypipe scheitern.
    player.connect_info({
        let watch = watch.clone();
        move |json| watch.ytdlp_info(json)
    });
    player.connect_aspect({
        let watch = watch.clone();
        move |a| watch.set_aspect(a)
    });

    // Tastenkürzel. Tippt man im Suchfeld, kommen die Tasten hier nicht an.
    let keys = gtk::EventControllerKey::new();
    // Menü per Tastatur (Esc auf der Startseite): Pfeile hoch/runter wählen, Enter/Leertaste drückt,
    // Esc klappt zu. Die Knöpfe sind sonst nicht fokussierbar (Maus-Bedienung), nur solange so geöffnet.
    let col_all = [later_button.clone(), history_button.clone(), account_button.clone()];
    let top_all = [back.clone(), home.clone(), search_button.clone()];
    let menu_key: Rc<dyn Fn(gdk::Key) -> bool> = Rc::new({
        let (window, menu, set_menu, account) = (window.clone(), menu.clone(), set_menu.clone(), account_button.clone());
        move |key| {
            let shown = |b: &&gtk::Button| b.is_visible() && b.parent().is_some_and(|p| p.is_visible());
            // Spalte (Watch later, History, Konto) und obere Reihe (Zurück, Home, Suche).
            let col: Vec<&gtk::Button> = col_all.iter().filter(shown).collect();
            let top: Vec<&gtk::Button> = top_all.iter().filter(shown).collect();
            let close = || {
                set_menu(false);
                for b in col_all.iter().chain(&top_all) {
                    b.set_focusable(false);
                }
                gtk::prelude::GtkWindowExt::set_focus(&window, None::<&gtk::Widget>);
            };
            let go = |b: Option<&&gtk::Button>| {
                if let Some(b) = b {
                    b.grab_focus();
                }
            };
            if !menu.reveals_child() {
                // Esc: aufklappen, erster Eintrag gewählt.
                if key != gdk::Key::Escape {
                    return false;
                }
                set_menu(true);
                for b in col_all.iter().chain(&top_all) {
                    b.set_focusable(true);
                }
                // Markierung des gewählten Knopfs zeigen (`:focus-visible`).
                window.set_focus_visible(true);
                go(col.first());
                return true;
            }
            let focus = gtk::prelude::GtkWindowExt::focus(&window);
            let is = |b: &&gtk::Button| focus.as_ref().is_some_and(|f| f == b.upcast_ref::<gtk::Widget>());
            let (in_col, in_top) = (col.iter().position(is), top.iter().position(is));
            let down = key == gdk::Key::Down;
            match key {
                gdk::Key::Escape => close(),
                // Hoch/runter: in der Spalte mit Umlauf; aus der oberen Reihe runter in die Spalte.
                gdk::Key::Up | gdk::Key::Down => match (in_col, in_top) {
                    (Some(i), _) => {
                        let n = col.len();
                        go(col.get(if down { (i + 1) % n } else { (i + n - 1) % n }));
                    }
                    (None, Some(_)) if !down => {}
                    _ => go(col.first()),
                },
                // Rechts: aus der Spalte in die obere Reihe, dort weiter; links zurück (am Anfang in die Spalte).
                gdk::Key::Right => match (in_col, in_top) {
                    (_, Some(i)) => go(top.get(i + 1)),
                    (Some(_), _) => go(top.first()),
                    _ => go(col.first()),
                },
                gdk::Key::Left => match in_top {
                    Some(0) => go(col.first()),
                    Some(i) => go(top.get(i - 1)),
                    None => {}
                },
                gdk::Key::Return | gdk::Key::KP_Enter | gdk::Key::space => {
                    let Some(b) = col.iter().chain(&top).find(|b| is(b)).map(|b| (*b).clone()) else { return false };
                    // Konto klappt die Kontoliste auf (Menü bleibt offen), sonst schließt der Klick das Menü.
                    if b != account {
                        close();
                    }
                    b.emit_clicked();
                }
                _ => return false,
            }
            true
        }
    });
    keys.connect_key_pressed({
        let (nav, window, player, search) = (nav.clone(), window.clone(), player.clone(), search.clone());
        let (menu_key, menu) = (menu_key.clone(), menu.clone());
        let (play, open_channel) = (play.clone(), open_channel.clone());
        let reload_page = reload_page.clone();
        move |_, key, _, mods| {
            // Suchfenster offen: nur Esc schließt, alles andere gehört der Suche.
            if search.is_open() {
                if key == gdk::Key::Escape {
                    search.close();
                    return glib::Propagation::Stop;
                }
                return glib::Propagation::Proceed;
            }
            // Strg+V: YouTube-Link aus der Zwischenablage öffnen (Video oder Kanal).
            if key == gdk::Key::v && mods.contains(gdk::ModifierType::CONTROL_MASK) {
                let (play, open_channel) = (play.clone(), open_channel.clone());
                let clipboard = window.clipboard();
                glib::spawn_future_local(async move {
                    let Ok(Some(text)) = clipboard.read_text_future().await else { return };
                    let text = text.trim().to_string();
                    if let Some(id) = video_id(&text) {
                        play(Video { id, ..Default::default() });
                    } else if let Some((_, rest)) = text.split_once("/channel/") {
                        open_channel(rest.split(['/', '?']).next().unwrap_or(rest).to_string());
                    }
                });
                return glib::Propagation::Stop;
            }
            // Suche öffnen: "/" (wie bei YouTube) oder Strg+K.
            if key == gdk::Key::slash || (key == gdk::Key::k && mods.contains(gdk::ModifierType::CONTROL_MASK)) {
                search.open();
                return glib::Propagation::Stop;
            }
            // Neu laden: F5, Strg+R oder R.
            if key == gdk::Key::F5 || (key == gdk::Key::r && mods.contains(gdk::ModifierType::CONTROL_MASK)) {
                reload_page();
                return glib::Propagation::Stop;
            }
            // Strg+←/→: voriges/nächstes Kapitel (wie YouTube).
            if mods == gdk::ModifierType::CONTROL_MASK && matches!(key, gdk::Key::Left | gdk::Key::Right) && nav.current() == "watch" {
                player.chapter(if key == gdk::Key::Right { 1 } else { -1 });
                return glib::Propagation::Stop;
            }
            // Shift ist erlaubt (">" braucht es auf deutschen Tastaturen), andere Modifier nicht.
            if !(mods - gdk::ModifierType::SHIFT_MASK).is_empty() {
                return glib::Propagation::Proceed;
            }
            let watching = nav.current() == "watch";
            // Menü offen: Pfeile/Enter/Esc gehören ihm. Esc öffnet es beim Video (außer im Vollbild: das
            // verlässt Esc zuerst) und auf der Startseite (kein Zurück); sonst bleibt Esc „Zurück“.
            let home = nav.current() == "browse" && nav.history.borrow().is_empty();
            let menu_esc = key == gdk::Key::Escape && !window.is_fullscreen() && (watching || home);
            if (menu.reveals_child() || menu_esc) && mods.is_empty() && menu_key(key) {
                return glib::Propagation::Stop;
            }
            // Miniplayer: nur die Video-Tasten; Pfeile/Ziffern bleiben fürs Raster.
            let mini = !watching && nav.watch.is_mini();
            let handled = match key {
                gdk::Key::Escape if window.is_fullscreen() => {
                    window.unfullscreen();
                    true
                }
                gdk::Key::Escape => {
                    nav.back();
                    true
                }
                gdk::Key::r | gdk::Key::R => {
                    reload_page();
                    true
                }
                gdk::Key::f if watching => {
                    player.controls.fullscreen.emit_clicked();
                    true
                }
                // f: zurück zum großen Video (Ansehen-Seite, kein Vollbild); q: Miniplayer schließen (wie in mpv).
                gdk::Key::f if mini => {
                    nav.go("watch");
                    true
                }
                gdk::Key::q if mini => {
                    nav.close_mini();
                    true
                }
                gdk::Key::space | gdk::Key::k | gdk::Key::m if mini => player.key(key),
                k if watching => player.key(k),
                // Pfeiltasten, solange nichts auf der Seite gewählt ist (nach dem Start, mit Miniplayer):
                // erste Karte wählen, danach navigiert GTK selbst im Raster weiter (Enter öffnet).
                gdk::Key::Up | gdk::Key::Down | gdk::Key::Left | gdk::Key::Right => {
                    let page = nav.stack.visible_child();
                    match page {
                        Some(p) if gtk::prelude::GtkWindowExt::focus(&window).is_none_or(|f| !f.is_ancestor(&p)) => p.child_focus(gtk::DirectionType::TabForward),
                        _ => false,
                    }
                }
                _ => false,
            };
            if handled { glib::Propagation::Stop } else { glib::Propagation::Proceed }
        }
    });
    window.add_controller(keys);

    // Beim Schließen die Position des laufenden Videos sichern.
    window.connect_close_request({
        let player = player.clone();
        move |_| {
            player.stop();
            glib::Propagation::Proceed
        }
    });

    // Öffnet, was beim Start übergeben wurde, auch wenn die App schon läuft:
    // `tubequ <video-link>` spielt ab, `tubequ <kanal-link>` öffnet den Kanal, `tubequ <suchbegriff>` sucht,
    // `tubequ` allein zeigt beim ersten Mal die Startseite und holt sonst nur das Fenster nach vorne.
    let started = std::cell::Cell::new(false);
    Rc::new(move |arg: Option<String>| {
        match arg {
            Some(arg) if arg.contains("/channel/") => {
                let id = arg.split_once("/channel/").map_or("", |(_, id)| id);
                open_channel(id.split(['/', '?']).next().unwrap_or(id).to_string());
            }
            Some(arg) if arg.starts_with("http") => {
                // Per Link (auch aus anderen Apps über tubequ-links): wie ein Klick, also über das Anfragen-Limit;
                // Titel usw. kommen über die Details nach. Andere Adressen spielt mpv direkt.
                match video_id(&arg) {
                    Some(id) => play(Video { id, ..Default::default() }),
                    None => {
                        nav.go("watch");
                        player.play(None, &arg, false);
                    }
                }
            }
            Some(arg) => {
                nav.home();
                section.set("");
                grid.set_home(false);
                grid.show(move || youtube::search(arg.clone()));
            }
            None if !started.get() => {
                section.set("home");
                grid.set_home(true);
                // Frisch gespeicherte Startseite: beim Start nicht neu holen (nur beim ersten Lauf;
                // Reload/Home fragen danach normal).
                let saved = youtube::saved_home();
                let skip = std::cell::Cell::new(saved.is_some() && youtube::saved_home_fresh());
                grid.show_saved(saved, move || {
                    let skip = skip.replace(false);
                    async move { if skip { Ok(youtube::Page { videos: Vec::new(), next: None }) } else { youtube::home().await } }
                });
            }
            None => {}
        }
        started.set(true);
        window.present();
        player.prepare();
    })
}

thread_local! {
    /// Das Fenster gibt es nur einmal; weitere Starts geben ihre Argumente hierher weiter.
    static OPEN: std::cell::OnceCell<Rc<dyn Fn(Option<String>)>> = const { std::cell::OnceCell::new() };
}

fn main() -> glib::ExitCode {
    // libmpv verlangt LC_NUMERIC=C; ohne setlocale durch GTK bleibt alles auf C.
    gtk::disable_setlocale();
    // Panik-Meldung zusätzlich in ~/.cache/tubequ/crash.log: aus dem Startmenü gestartet geht stderr
    // verloren, und Paniken in GTK-Callbacks beenden die App sofort (abort).
    let default_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        use std::io::Write;
        let path = std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache/tubequ/crash.log");
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(path) {
            let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs());
            let _ = writeln!(f, "--- {now}\n{info}\n{}", std::backtrace::Backtrace::force_capture());
        }
        default_hook(info);
    }));
    let app = gtk::Application::builder()
        .application_id("dev.tubequ.Tubequ")
        .flags(if std::env::var_os("TUBEQU_DEV").is_some() {
            // Entwicklung: eigene Instanz neben der installierten, statt Aufrufe an sie weiterzugeben.
            gio::ApplicationFlags::HANDLES_COMMAND_LINE | gio::ApplicationFlags::NON_UNIQUE
        } else {
            gio::ApplicationFlags::HANDLES_COMMAND_LINE
        })
        .build();
    // Läuft die App schon, landet ein weiterer Start hier im laufenden Prozess.
    app.connect_command_line(|app, cmdline| {
        let arg = cmdline.arguments().get(1).map(|a| a.to_string_lossy().into_owned());
        let open = OPEN.with(|o| o.get_or_init(|| build(app)).clone());
        open(arg);
        glib::ExitCode::SUCCESS
    });
    app.run()
}

#[cfg(test)]
mod tests {
    #[test]
    fn video_id() {
        assert_eq!(super::video_id("https://www.youtube.com/watch?v=sF80I-TQiW0&t=3").as_deref(), Some("sF80I-TQiW0"));
        assert_eq!(super::video_id("https://youtu.be/sF80I-TQiW0").as_deref(), Some("sF80I-TQiW0"));
        assert_eq!(super::video_id("https://www.youtube.com/channel/UCxyz"), None);
    }
}

mod channel;
mod controls;
mod grid;
mod icons;
mod lists;
mod player;
mod progress;
mod search;
mod sponsorblock;
mod theme;
mod watch;
mod youtube;

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::LazyLock;
use youtube::Video;

static RT: LazyLock<tokio::runtime::Runtime> =
    LazyLock::new(|| tokio::runtime::Runtime::new().expect("tokio"));

/// Führt `fut` im Hintergrund (tokio) aus; das Ergebnis kommt im GTK-Thread an.
pub async fn bg<T: Send + 'static>(fut: impl Future<Output = T> + Send + 'static) -> T {
    RT.spawn(fut).await.expect("Hintergrund-Task abgestürzt")
}

/// Seitenwechsel mit Verlauf für den Zurück-Button: "browse", "channel", "watch".
struct Nav {
    stack: gtk::Stack,
    back: gtk::Button,
    window: gtk::ApplicationWindow,
    player: Rc<player::Player>,
    history: RefCell<Vec<String>>,
    /// Schwebende Knöpfe oben links. Auf Übersicht/Kanal immer sichtbar über der Seite;
    /// beim Video sitzen sie im Player und blenden sich mit dessen Leiste aus und ein.
    nav_bar: gtk::Revealer,
    page_layer: gtk::Overlay,
    in_player: std::cell::Cell<bool>,
}

impl Nav {
    fn current(&self) -> String {
        self.stack.visible_child_name().map(|s| s.to_string()).unwrap_or_default()
    }

    fn show(&self, page: &str) {
        // Verlässt man die Ansehen-Seite, stoppt das Video: Player gibt es nur beim Abspielen.
        if self.current() == "watch" && page != "watch" {
            self.player.stop();
            self.window.unfullscreen();
            self.window.set_title(Some("Tube"));
        }
        self.stack.set_visible_child_name(page);
        self.back.set_visible(!self.history.borrow().is_empty());

        let watch = page == "watch";
        if watch != self.in_player.get() {
            let (from, to) = if watch { (&self.page_layer, &self.player.widget) } else { (&self.player.widget, &self.page_layer) };
            from.remove_overlay(&self.nav_bar);
            to.add_overlay(&self.nav_bar);
            self.in_player.set(watch);
        }
        self.nav_bar.set_reveal_child(!watch || self.player.controls.widget.reveals_child());
    }

    fn go(&self, page: &str) {
        let cur = self.current();
        if cur != page {
            self.history.borrow_mut().push(cur);
        }
        self.show(page);
    }

    fn back(&self) {
        let prev = self.history.borrow_mut().pop();
        if let Some(p) = prev {
            self.show(&p);
        }
    }

    fn home(&self) {
        self.history.borrow_mut().clear();
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
    let back = nav_button("tube-back-symbolic", "Back (Esc)");
    back.set_visible(false);
    let home = nav_button("tube-home-symbolic", "Home");
    let search_button = nav_button("tube-search-symbolic", "Search (/)");
    let reload = nav_button("tube-reload-symbolic", "Reload (F5)");
    let history_button = nav_button("tube-history-symbolic", "History");
    let later_button = nav_button("tube-later-symbolic", "Watch later");
    let nav_box = gtk::Box::builder().spacing(2).css_classes(["bubble"]).build();
    nav_box.append(&back);
    nav_box.append(&home);
    nav_box.append(&search_button);
    nav_box.append(&later_button);
    nav_box.append(&history_button);
    nav_box.append(&reload);
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
        .title("Tube")
        .default_width(1280)
        .default_height(860)
        .decorated(false)
        .build();
    if tiling_wm() {
        window.set_child(Some(&root));
    } else {
        // Ohne Titelleiste: an freien Flächen ziehen verschiebt das Fenster.
        window.set_child(Some(&gtk::WindowHandle::builder().child(&root).build()));
    }

    let nav = Rc::new(Nav {
        stack: stack.clone(),
        back: back.clone(),
        window: window.clone(),
        player: player.clone(),
        history: RefCell::default(),
        nav_bar: nav_bar.clone(),
        page_layer: page_layer.clone(),
        in_player: std::cell::Cell::new(false),
    });
    // Beim Video: Navigation blendet sich zusammen mit der Player-Leiste ein und aus.
    player.controls.widget.connect_reveal_child_notify({
        let nav = nav.clone();
        move |r| {
            if nav.in_player.get() {
                nav.nav_bar.set_reveal_child(r.reveals_child());
            }
        }
    });

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

    let play: Rc<dyn Fn(Video)> = Rc::new({
        let (nav, player, watch) = (nav.clone(), player.clone(), watch.clone());
        move |v: Video| {
            watch.show(&v);
            nav.go("watch");
            // Fenstertitel = Videotitel (Alt-Tab, Fensterliste); „Tube“ bleibt hinten dran.
            if !v.title.is_empty() {
                nav.window.set_title(Some(&format!("{} – Tube", v.title)));
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
    stack.add_named(&watch.widget, Some("watch"));

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
    reload.connect_clicked({
        let reload_page = reload_page.clone();
        move |_| reload_page()
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

    home.connect_clicked({
        let (grid, nav) = (grid.clone(), nav.clone());
        move |_| {
            nav.home();
            grid.set_home(true);
            grid.show(youtube::home);
        }
    });

    // Verlauf und „Später ansehen“: lokale Listen im Raster (keine Anfragen an YouTube).
    history_button.connect_clicked({
        let (grid, nav) = (grid.clone(), nav.clone());
        move |_| {
            nav.home();
            grid.set_home(false);
            grid.show(|| async { Ok(youtube::Page { videos: progress::history(), next: None }) });
        }
    });
    later_button.connect_clicked({
        let (grid, nav) = (grid.clone(), nav.clone());
        move |_| {
            nav.home();
            grid.set_home(false);
            grid.show(|| async { Ok(youtube::Page { videos: lists::later(), next: None }) });
        }
    });

    // Schwebendes Suchfenster über allem.
    let search = search::SearchModal::new({
        let (grid, nav) = (grid.clone(), nav.clone());
        move |q| {
            nav.home();
            grid.set_home(false);
            grid.show(move || youtube::search(q.clone()));
        }
    });
    root.add_overlay(&search.widget);
    search_button.connect_clicked({
        let search = search.clone();
        move |_| search.open()
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
            fs.set_icon_name(if full { "tube-restore-symbolic" } else { "tube-fullscreen-symbolic" });
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
    keys.connect_key_pressed({
        let (nav, window, player, search) = (nav.clone(), window.clone(), player.clone(), search.clone());
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
            // Neu laden: F5 oder Strg+R.
            if key == gdk::Key::F5 || (key == gdk::Key::r && mods.contains(gdk::ModifierType::CONTROL_MASK)) {
                reload_page();
                return glib::Propagation::Stop;
            }
            // Shift ist erlaubt (">" braucht es auf deutschen Tastaturen), andere Modifier nicht.
            if !(mods - gdk::ModifierType::SHIFT_MASK).is_empty() {
                return glib::Propagation::Proceed;
            }
            let watching = nav.current() == "watch";
            let handled = match key {
                gdk::Key::Escape if window.is_fullscreen() => {
                    window.unfullscreen();
                    true
                }
                gdk::Key::Escape => {
                    nav.back();
                    true
                }
                gdk::Key::f if watching => {
                    player.controls.fullscreen.emit_clicked();
                    true
                }
                k if watching => player.key(k),
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
    // `tube <video-link>` spielt ab, `tube <kanal-link>` öffnet den Kanal, `tube <suchbegriff>` sucht,
    // `tube` allein zeigt beim ersten Mal die Startseite und holt sonst nur das Fenster nach vorne.
    let started = std::cell::Cell::new(false);
    Rc::new(move |arg: Option<String>| {
        match arg {
            Some(arg) if arg.contains("/channel/") => {
                let id = arg.split_once("/channel/").map_or("", |(_, id)| id);
                open_channel(id.split(['/', '?']).next().unwrap_or(id).to_string());
            }
            Some(arg) if arg.starts_with("http") => {
                // Per Link (auch aus anderen Apps über tube-links): wie ein Klick, also über das Anfragen-Limit;
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
                grid.set_home(false);
                grid.show(move || youtube::search(arg.clone()));
            }
            None if !started.get() => {
                grid.set_home(true);
                grid.show_saved(youtube::saved_home(), youtube::home);
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
    let app = gtk::Application::builder()
        .application_id("dev.tube.Tube")
        .flags(if std::env::var_os("TUBE_DEV").is_some() {
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

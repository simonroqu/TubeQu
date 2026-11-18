// Unter Windows ohne zusätzliches Konsolenfenster starten.
#![cfg_attr(windows, windows_subsystem = "windows")]

use tubequ_core::{lists, progress, sponsorblock, youtube};

mod channel;
mod grid;
mod player;
mod watch;

use qu_core::{fly, icons, search};
use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gdk, gio, glib};
use std::rc::Rc;
use youtube::{video_id, Video};

pub use qu_core::{bg, busy::Busy, RT};

/// Profilbild (über `youtube::avatar`, auf der Platte zwischengespeichert) und Tooltip eines Kontos.
fn show_account(pic: &gtk::Image, button: &gtk::Button, a: &youtube::Account) {
    button.set_tooltip_text(Some(&format!("{}\n{}", a.name, a.email)));
    pic.set_icon_name(Some("qu-account-symbolic"));
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
    // Theme, Icons und CSS in Schichten: Kern < App < ~/.config/tubequ/style.css (TubeQu hat kein eigenes CSS).
    qu_core::style::install(&display, "");

    let player = Rc::new(player::new());

    // Keine Kopfzeile: das Menü oben links baut qu-core aus `[nav]` in qu.toml (Burger mit Spalte).
    // Fenster, Seitenstapel und Menü oben links (aus qu.toml) baut qu-core.
    let shell = qu_core::nav::Shell::new(app);
    let navbar = shell.navbar.clone();
    let back = navbar.button("back").clone();
    let home = navbar.button("home").clone();
    let search_button = navbar.button("search").clone();
    let history_button = navbar.button("history").clone();
    let later_button = navbar.button("later").clone();
    // Konto (nur mit Browser-Login): Profilbild des aktiven Kontos unten in der Spalte. Klick klappt
    // daneben die übrigen Konten nach rechts auf (Bubble wird zum liegenden U).
    let account_pic = gtk::Image::builder()
        .icon_name("qu-account-symbolic")
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
    let account_row = navbar.slot("account").clone();
    account_row.set_visible(youtube::COOKIES.is_some());
    account_row.append(&account_button);
    account_row.append(&accounts_menu);
    // Menü zu: aufgeklappte Kontoliste mit zu.
    navbar.connect_menu({
        let accounts_menu = accounts_menu.clone();
        move |open| {
            if !open {
                accounts_menu.set_reveal_child(false);
            }
        }
    });
    let set_menu: Rc<dyn Fn(bool)> = Rc::new({
        let navbar = navbar.clone();
        move |open| navbar.set_menu(open)
    });

    let (stack, root, window) = (shell.stack.clone(), shell.root.clone(), shell.window.clone());

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

    // Seitenverlauf, Ansehen-Seite als Ebene darüber, Miniplayer (ziehbar, Zoom-Animation): qu-core.
    let nav = qu_core::nav::Nav::new(&shell, player.clone(), watch.frame());


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
            // Fenstertitel = Videotitel (Alt-Tab, Fensterliste); „TubeQu“ bleibt hinten dran.
            if !v.title.is_empty() {
                nav.window.set_title(Some(&format!("{} – {}", v.title, qu_core::config().app.title)));
            }
            let (player, watch) = (player.clone(), watch.clone());
            glib::spawn_future_local(async move {
                // Das Laden des Streams (yt-dlp in mpv) zählt auch als Anfrage – und kommt als Erstes dran.
                // Details erst danach anstellen, sonst müsste der Stream 2 s auf sie warten.
                match bg(youtube::throttle()).await {
                    Ok(()) => player::play(&player, Some(v.clone()), &v.url(), v.is_live),
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
            "watch" => player::reload(&player),
            _ => {}
        }
    });
    player.retry.connect_clicked({
        let player = Rc::downgrade(&player);
        move |_| {
            if let Some(p) = player.upgrade() {
                player::reload(&p);
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
                        .icon_name("qu-account-symbolic")
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
    let search = search::SearchModal::from_config(Some(std::rc::Rc::new(|q: String| -> std::pin::Pin<Box<dyn Future<Output = Vec<String>>>> { Box::pin(bg(youtube::suggestions(q))) })), {
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
            fs.set_icon_name(if full { "qu-restore-symbolic" } else { "qu-fullscreen-symbolic" });
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
    // yt-dlp-Infos: Storyboard für die Timeline-Vorschau, Ersatz-Details für die Ansehen-Seite.
    player.connect_info({
        let (watch, controls) = (watch.clone(), player.controls.clone());
        move |json| {
            controls.set_storyboard(youtube::storyboard_from_ytdlp(json));
            watch.ytdlp_info(json)
        }
    });
    player.connect_aspect({
        let watch = watch.clone();
        move |a| watch.set_aspect(a)
    });

    // Tastenkürzel. Tippt man im Suchfeld, kommen die Tasten hier nicht an.
    let keys = gtk::EventControllerKey::new();
    keys.connect_key_pressed({
        let (nav, window, player, search) = (nav.clone(), window.clone(), player.clone(), search.clone());
        let navbar = navbar.clone();
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
            if (navbar.menu_open() || menu_esc) && mods.is_empty() && navbar.key(&window, key) {
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
                        player::play(&player, None, &arg, false);
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

fn main() -> glib::ExitCode {
    qu_core::start(include_str!("../qu.toml"), include_str!("../data/dev.tubequ.Tubequ.svg"), build)
}

#[cfg(test)]
mod tests {
    /// qu.toml ist gültig (Tippfehler, unbekannte Menüeinträge fallen hier auf statt beim Start).
    #[test]
    fn qu_toml() {
        let c = qu_core::config::Config::parse(include_str!("../qu.toml")).unwrap();
        assert_eq!(c.app.name, env!("CARGO_PKG_NAME"));
    }
}

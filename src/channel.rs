//! Kanalseite: Avatar, Name und Abo-Knopf oben, darunter das Video-Raster (nur Videos, kein Live-Tab).

use crate::bg;
use crate::grid::Grid;
use crate::youtube::{self, ChannelInfo, Video};
use gtk4 as gtk;
use gtk::prelude::*;
use gtk::{gdk, glib};
use std::cell::RefCell;
use std::rc::Rc;

pub struct ChannelPage {
    pub widget: gtk::Box,
    grid: Rc<Grid>,
    avatar: gtk::Image,
    name: gtk::Label,
    subscribers: gtk::Label,
    /// Graue Balken statt Name/Abonnenten, bis die Kanal-Infos da sind (style.css `.skeleton`).
    ghost: gtk::Box,
    sub: gtk::Button,
    id: RefCell<String>,
}

thread_local! {
    /// Kanäle ("<konto>:<kanal-id>"), die in dieser Sitzung abonniert wurden. Den echten Abo-Zustand fragt die App nicht
    /// ab (wäre eine Anfrage pro Kanalbesuch); unbekannt = YouTube-Symbol leer.
    static SUBSCRIBED: RefCell<std::collections::HashSet<String>> = RefCell::default();
}

fn show_sub(sub: &gtk::Button, on: bool) {
    sub.set_icon_name(if on { "tubequ-subscribed-symbolic" } else { "tubequ-subscribe-symbolic" });
    sub.set_tooltip_text(Some(if on { "Unsubscribe" } else { "Subscribe" }));
}

impl ChannelPage {
    pub fn new(on_play: impl Fn(Video) + 'static) -> Rc<Self> {
        let avatar = gtk::Image::builder().pixel_size(64).overflow(gtk::Overflow::Hidden).halign(gtk::Align::Center).valign(gtk::Align::Center).css_classes(["avatar", "placeholder"]).build();
        let name = gtk::Label::builder().xalign(0.0).css_classes(["title-2"]).build();

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        top.append(&avatar);
        // Name und darunter die Abonnentenzahl (wie auf der Ansehen-Seite).
        let subscribers = gtk::Label::builder().xalign(0.0).css_classes(["dim-label", "caption"]).build();
        let name_col = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(2).valign(gtk::Align::Center).build();
        name_col.append(&name);
        name_col.append(&subscribers);
        let ghost = gtk::Box::builder().orientation(gtk::Orientation::Vertical).spacing(8).valign(gtk::Align::Center).visible(false).build();
        for (w, h) in [(220, 28), (120, 12)] {
            ghost.append(&gtk::Box::builder().width_request(w).height_request(h).halign(gtk::Align::Start).css_classes(["skeleton"]).build());
        }
        top.append(&ghost);
        top.append(&name_col);
        // Nur mit Browser-Login; ohne Login kann YouTube kein Abo zuordnen.
        let sub = gtk::Button::builder().focusable(false).valign(gtk::Align::Center).css_classes(["flat"]).visible(youtube::COOKIES.is_some()).build();
        show_sub(&sub, false);
        top.append(&sub);
        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            // Platz für die schwebende Navigation oben links.
            .margin_top(68)
            .margin_start(16)
            .margin_end(16)
            .build();
        header.append(&top);

        // Kanal-Links im Raster dieser Seite öffnen einfach den (gleichen) Kanal neu.
        let page: Rc<RefCell<Option<std::rc::Weak<ChannelPage>>>> = Rc::default();
        let grid = Grid::new(on_play, {
            let page = page.clone();
            move |id| {
                if let Some(p) = page.borrow().as_ref().and_then(|w| w.upgrade()) {
                    p.open(id);
                }
            }
        });

        // Der Kopf hält schon Abstand zur Navigation: Raster direkt darunter.
        grid.set_top_margin(12);
        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.append(&header);
        widget.append(&grid.widget);

        let me = Rc::new(ChannelPage { widget, grid, avatar, name, subscribers, ghost, sub, id: RefCell::default() });
        page.replace(Some(Rc::downgrade(&me)));

        let weak = Rc::downgrade(&me);
        me.sub.connect_clicked(move |b| {
            let Some(me) = weak.upgrade() else { return };
            let id = format!("{}:{}", youtube::account(), me.id.borrow());
            let on = !SUBSCRIBED.with(|s| s.borrow().contains(&id));
            let me_id = me.id.borrow().clone();
            b.set_sensitive(false);
            let b = b.downgrade();
            glib::spawn_future_local(async move {
                let _busy = crate::Busy::new();
                let res = bg(youtube::subscribe(me_id, on)).await;
                let Some(b) = b.upgrade() else { return };
                b.set_sensitive(true);
                match res {
                    Ok(()) => {
                        SUBSCRIBED.with(|s| if on { s.borrow_mut().insert(id) } else { s.borrow_mut().remove(&id) });
                        show_sub(&b, on);
                    }
                    Err(e) => b.set_tooltip_text(Some(&e)),
                }
            });
        });
        me
    }

    /// Kanal anzeigen.
    pub fn open(self: &Rc<Self>, id: String) {
        show_sub(&self.sub, SUBSCRIBED.with(|s| s.borrow().contains(&format!("{}:{id}", youtube::account()))));
        self.id.replace(id);
        self.name.set_text("");
        self.subscribers.set_text("");
        self.avatar.clear();
        self.ghost.set_visible(true);
        self.load();
    }

    fn load(self: &Rc<Self>) {
        let id = self.id.borrow().clone();
        let weak = Rc::downgrade(self);
        // Eine Anfrage liefert Kanal-Infos und Videos: das Raster bekommt die Videos,
        // die Infos kommen über diesen Kanal zum Kopfbereich. (Bei Reload läuft das hier erneut.)
        self.grid.show(move || {
            let (tx, rx) = async_channel::bounded::<ChannelInfo>(1);
            let weak = weak.clone();
            glib::spawn_future_local(async move {
                let info = rx.recv().await;
                let Some(me) = weak.upgrade() else { return };
                match info {
                    Ok(info) if *me.id.borrow() == info.id => me.set_info(info),
                    Ok(_) => {}
                    // Anfrage gescheitert (Sender weg): Platzhalter nicht ewig stehen lassen.
                    Err(_) => me.ghost.set_visible(false),
                }
            });
            let id = id.clone();
            async move {
                let (info, page) = youtube::channel(id).await?;
                let _ = tx.send(info).await;
                Ok(page)
            }
        });
    }

    /// Kanal neu laden (Reload-Knopf).
    pub fn reload(&self) {
        self.grid.reload();
    }

    fn set_info(&self, info: ChannelInfo) {
        self.ghost.set_visible(false);
        self.name.set_text(&info.name);
        self.subscribers.set_text(&info.subscribers.map(|n| format!("{} subscribers", crate::grid::fmt_views(n))).unwrap_or_default());
        if let Some(url) = info.avatar {
            let pic = self.avatar.downgrade();
            glib::spawn_future_local(async move {
                let Ok(bytes) = bg(youtube::avatar(url)).await else { return };
                let Ok(tex) = gdk::Texture::from_bytes(&glib::Bytes::from_owned(bytes)) else { return };
                if let Some(pic) = pic.upgrade() {
                    pic.set_paintable(Some(&tex));
                }
            });
        }
    }
}

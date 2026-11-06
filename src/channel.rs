//! Kanalseite: Avatar und Name oben, Tabs „Videos“ und „Live“, darunter das Video-Raster.

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
    videos_tab: gtk::ToggleButton,
    live_tab: gtk::ToggleButton,
    id: RefCell<String>,
}

impl ChannelPage {
    pub fn new(on_play: impl Fn(Video) + 'static) -> Rc<Self> {
        let avatar = gtk::Image::builder().pixel_size(64).overflow(gtk::Overflow::Hidden).css_classes(["avatar"]).build();
        let name = gtk::Label::builder().xalign(0.0).css_classes(["title-2"]).build();
        let videos_tab = gtk::ToggleButton::builder().label("Videos").active(true).focusable(false).build();
        let live_tab = gtk::ToggleButton::builder().label("Live").group(&videos_tab).focusable(false).build();

        let top = gtk::Box::new(gtk::Orientation::Horizontal, 16);
        top.append(&avatar);
        top.append(&name);
        let tabs = gtk::Box::builder().css_classes(["linked"]).build();
        tabs.append(&videos_tab);
        tabs.append(&live_tab);
        let header = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(16)
            // Platz für die schwebende Navigation oben links.
            .margin_top(68)
            .margin_start(16)
            .margin_end(16)
            .build();
        header.append(&top);
        header.append(&tabs);

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

        let widget = gtk::Box::new(gtk::Orientation::Vertical, 0);
        widget.append(&header);
        widget.append(&grid.widget);

        let me = Rc::new(ChannelPage { widget, grid, avatar, name, videos_tab, live_tab, id: RefCell::default() });
        page.replace(Some(Rc::downgrade(&me)));

        // `clicked` kommt nur vom Nutzer, nicht von set_active in `open`.
        for (tab, live) in [(&me.videos_tab, false), (&me.live_tab, true)] {
            let weak = Rc::downgrade(&me);
            tab.connect_clicked(move |t| {
                if t.is_active()
                    && let Some(me) = weak.upgrade()
                {
                    me.load(live);
                }
            });
        }
        me
    }

    /// Kanal anzeigen, beginnend mit dem Tab „Videos“.
    pub fn open(self: &Rc<Self>, id: String) {
        self.id.replace(id);
        self.name.set_text("");
        self.avatar.clear();
        self.live_tab.set_visible(false);
        self.videos_tab.set_active(true);
        self.load(false);
    }

    fn load(self: &Rc<Self>, live: bool) {
        let id = self.id.borrow().clone();
        let weak = Rc::downgrade(self);
        // Eine Anfrage liefert Kanal-Infos und Videos: das Raster bekommt die Videos,
        // die Infos kommen über diesen Kanal zum Kopfbereich. (Bei Reload läuft das hier erneut.)
        self.grid.show(move || {
            let (tx, rx) = async_channel::bounded::<ChannelInfo>(1);
            let weak = weak.clone();
            glib::spawn_future_local(async move {
                if let Ok(info) = rx.recv().await
                    && let Some(me) = weak.upgrade()
                    && *me.id.borrow() == info.id
                {
                    me.set_info(info);
                }
            });
            let id = id.clone();
            async move {
                let (info, page) = youtube::channel(id, live).await?;
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
        self.name.set_text(&info.name);
        self.live_tab.set_visible(info.has_live);
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

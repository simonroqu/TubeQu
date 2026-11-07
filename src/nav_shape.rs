//! Nav-Bubble als selbst gezeichnete Form: eingeklappt eine Pille, aufgeklappt ein umgedrehtes L
//! (Reihe oben, Spalte unter dem Burger). CSS kann nur Rechtecke, darum Pfad + Schatten von Hand.
//! Farben wie `.bubble` in style.css.

use gtk4 as gtk;
use gtk::prelude::*;
use gtk::subclass::prelude::*;
use gtk::{gdk, glib, graphene, gsk};

/// Innenabstand der Form (= Padding 2 px + Rand 1 px einer normalen Bubble).
pub const PAD: f32 = 3.0;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct NavShape;

    #[glib::object_subclass]
    impl ObjectSubclass for NavShape {
        const NAME: &'static str = "TubequNavShape";
        type Type = super::NavShape;
        type ParentType = gtk::Box;
    }

    impl ObjectImpl for NavShape {}
    impl BoxImpl for NavShape {}

    impl WidgetImpl for NavShape {
        fn snapshot(&self, s: &gtk::Snapshot) {
            let obj = self.obj();
            let (Some(top), Some(menu)) = (obj.first_child(), obj.last_child()) else { return };
            let (Some(t), Some(m)) = (top.compute_bounds(&*obj), menu.compute_bounds(&*obj)) else { return };
            // GTK misst ab der Innenfläche (ohne Padding): Form um PAD nach außen erweitern.
            s.save();
            s.translate(&graphene::Point::new(-PAD, -PAD));
            let (w, h) = (obj.width() as f32 + 2.0 * PAD, obj.height() as f32 + 2.0 * PAD);
            let row = t.y() + t.height() + 2.0 * PAD;
            let col = m.x() + m.width() + 2.0 * PAD;
            if let Ok(path) = gsk::Path::parse(&shape(w, row, col, h, crate::theme::pill())) {
                // Schatten wie `box-shadow: 0 6px 20px rgba(0,0,0,.35)`: nur außerhalb der Form.
                s.push_mask(gsk::MaskMode::InvertedAlpha);
                s.append_fill(&path, gsk::FillRule::Winding, &gdk::RGBA::BLACK);
                s.pop();
                s.push_blur(10.0);
                s.save();
                s.translate(&graphene::Point::new(0.0, 6.0));
                s.append_fill(&path, gsk::FillRule::Winding, &gdk::RGBA::new(0.0, 0.0, 0.0, 0.35));
                s.restore();
                s.pop();
                s.pop();
                s.append_fill(&path, gsk::FillRule::Winding, &gdk::RGBA::new(24.0 / 255.0, 24.0 / 255.0, 28.0 / 255.0, 0.55));
                s.append_stroke(&path, &gsk::Stroke::new(1.0), &gdk::RGBA::new(1.0, 1.0, 1.0, 0.12));
            }
            s.restore();
            self.parent_snapshot(s);
        }
    }
}

glib::wrapper! {
    pub struct NavShape(ObjectSubclass<imp::NavShape>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl NavShape {
    pub fn new() -> Self {
        let o: Self = glib::Object::new();
        o.set_orientation(gtk::Orientation::Vertical);
        o
    }
}

/// SVG-Pfad der Form. `w` × `row`: obere Reihe, `col`: Breite der Spalte, `h`: Gesamthöhe
/// (> `row`, solange das Menü aufgeht). `pill`: Bubble-Rundung aus dem Theme. Halber Pixel
/// Einzug, damit der 1-px-Rand scharf ist.
fn shape(w: f32, row: f32, col: f32, h: f32, pill: f32) -> String {
    let (l, t, r, b, cr, cb) = (0.5, 0.5, w - 0.5, row - 0.5, col - 0.5, h - 0.5);
    let rad = pill.min((b - t) / 2.0);
    let top = format!("M{l},{} A{rad},{rad} 0 0 1 {},{t} H{} A{rad},{rad} 0 0 1 {r},{} V{} A{rad},{rad} 0 0 1 {},{b}",
        t + rad, l + rad, r - rad, t + rad, b - rad, r - rad);
    let e = h - row;
    if e < 1.0 {
        return format!("{top} H{} A{rad},{rad} 0 0 1 {l},{} Z", l + rad, b - rad);
    }
    // Innere Ecke (nach innen gewölbt) und Spaltenende wachsen beim Aufklappen mit.
    let f = rad.min(e / 2.0);
    let rb = rad.min((cr - l) / 2.0).min(e - f);
    format!("{top} H{} A{f},{f} 0 0 0 {cr},{} V{} A{rb},{rb} 0 0 1 {},{cb} H{} A{rb},{rb} 0 0 1 {l},{} Z",
        cr + f, b + f, cb - rb, cr - rb, l + rb, cb - rb)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shape_pill_and_l() {
        // Eingeklappt: Pille ohne innere Ecke; aufgeklappt: mit nach innen gewölbter Ecke (Bogen mit sweep 0).
        let closed = shape(130.0, 36.0, 36.0, 36.0, 9999.0);
        assert!(!closed.contains(" 0 0 0 "));
        let open = shape(130.0, 36.0, 36.0, 130.0, 9999.0);
        assert!(open.contains(" 0 0 0 "));
        assert!(open.contains("V129.5") || open.contains(",129.5"), "Spalte reicht bis unten: {open}");
        assert!(gsk::Path::parse(&closed).is_ok() && gsk::Path::parse(&open).is_ok());
    }
}

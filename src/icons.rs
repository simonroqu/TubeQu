//! Eigener minimalistischer Icon-Satz (data/icons). Die SVGs stecken im Programm und werden beim
//! Start in den Cache geschrieben und GTK bekannt gemacht. Endung `-symbolic`: GTK färbt sie in
//! der aktuellen Schriftfarbe ein, sie passen sich also dem Theme an.

use gtk4 as gtk;
use std::path::PathBuf;

const ICONS: [(&str, &str); 17] = [
    ("tubequ-search-symbolic", include_str!("../data/icons/tubequ-search-symbolic.svg")),
    ("tubequ-home-symbolic", include_str!("../data/icons/tubequ-home-symbolic.svg")),
    ("tubequ-back-symbolic", include_str!("../data/icons/tubequ-back-symbolic.svg")),
    ("tubequ-play-symbolic", include_str!("../data/icons/tubequ-play-symbolic.svg")),
    ("tubequ-pause-symbolic", include_str!("../data/icons/tubequ-pause-symbolic.svg")),
    ("tubequ-volume-symbolic", include_str!("../data/icons/tubequ-volume-symbolic.svg")),
    ("tubequ-mute-symbolic", include_str!("../data/icons/tubequ-mute-symbolic.svg")),
    ("tubequ-settings-symbolic", include_str!("../data/icons/tubequ-settings-symbolic.svg")),
    ("tubequ-fullscreen-symbolic", include_str!("../data/icons/tubequ-fullscreen-symbolic.svg")),
    ("tubequ-restore-symbolic", include_str!("../data/icons/tubequ-restore-symbolic.svg")),
    ("tubequ-reload-symbolic", include_str!("../data/icons/tubequ-reload-symbolic.svg")),
    ("tubequ-history-symbolic", include_str!("../data/icons/tubequ-history-symbolic.svg")),
    ("tubequ-later-symbolic", include_str!("../data/icons/tubequ-later-symbolic.svg")),
    ("tubequ-subtitles-symbolic", include_str!("../data/icons/tubequ-subtitles-symbolic.svg")),
    ("tubequ-menu-symbolic", include_str!("../data/icons/tubequ-menu-symbolic.svg")),
    ("tubequ-spinner-symbolic", include_str!("../data/icons/tubequ-spinner-symbolic.svg")),
    ("tubequ-close-symbolic", include_str!("../data/icons/tubequ-close-symbolic.svg")),
];

/// Lade-Kreis: eigenes Icon, per CSS gedreht (`.tubequ-spinner` in style.css), sichtbar = läuft.
/// Statt `gtk::Spinner`: der friert in GTK 4.24 ein, sobald er einmal ausgeblendet war
/// (auch durch Seitenwechsel), und dreht sich nach dem Wiedereinblenden nicht mehr.
pub fn spinner(size: i32) -> gtk::builders::ImageBuilder {
    gtk::Image::builder().icon_name("tubequ-spinner-symbolic").pixel_size(size).css_classes(["tubequ-spinner"])
}

pub fn install(display: &gtk::gdk::Display) {
    // Lose Dateien direkt im Suchpfad findet GTK ohne Theme-Struktur.
    let dir = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache/tubequ/icons");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    for (name, svg) in ICONS {
        let _ = std::fs::write(dir.join(format!("{name}.svg")), svg);
    }
    gtk::IconTheme::for_display(display).add_search_path(dir);
}

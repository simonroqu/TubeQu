//! Eigener minimalistischer Icon-Satz (data/icons). Die SVGs stecken im Programm und werden beim
//! Start in den Cache geschrieben und GTK bekannt gemacht. Endung `-symbolic`: GTK färbt sie in
//! der aktuellen Schriftfarbe ein, sie passen sich also dem Theme an.

use gtk4 as gtk;
use std::path::PathBuf;

const ICONS: [(&str, &str); 10] = [
    ("tube-search-symbolic", include_str!("../data/icons/tube-search-symbolic.svg")),
    ("tube-home-symbolic", include_str!("../data/icons/tube-home-symbolic.svg")),
    ("tube-back-symbolic", include_str!("../data/icons/tube-back-symbolic.svg")),
    ("tube-play-symbolic", include_str!("../data/icons/tube-play-symbolic.svg")),
    ("tube-pause-symbolic", include_str!("../data/icons/tube-pause-symbolic.svg")),
    ("tube-volume-symbolic", include_str!("../data/icons/tube-volume-symbolic.svg")),
    ("tube-mute-symbolic", include_str!("../data/icons/tube-mute-symbolic.svg")),
    ("tube-settings-symbolic", include_str!("../data/icons/tube-settings-symbolic.svg")),
    ("tube-fullscreen-symbolic", include_str!("../data/icons/tube-fullscreen-symbolic.svg")),
    ("tube-restore-symbolic", include_str!("../data/icons/tube-restore-symbolic.svg")),
];

pub fn install(display: &gtk::gdk::Display) {
    // Lose Dateien direkt im Suchpfad findet GTK ohne Theme-Struktur.
    let dir = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache/tube/icons");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    for (name, svg) in ICONS {
        let _ = std::fs::write(dir.join(format!("{name}.svg")), svg);
    }
    gtk::IconTheme::for_display(display).add_search_path(dir);
}

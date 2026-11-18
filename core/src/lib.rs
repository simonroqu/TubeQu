//! TubeQu ohne Oberfläche: alles, was nicht an GTK hängt. Die GTK-App (`../src`) nutzt es, die Android-App über
//! `ffi/` (Ordner vorher mit `qu_core::app::set_dirs` setzen). Auf Android gibt es kein yt-dlp und kein Browser-Login:
//! dort läuft alles anonym über rustypipe (Kanal, Abspielen), siehe `youtube.rs`.

pub mod fmt;
pub mod lists;
pub mod progress;
pub mod sponsorblock;
pub mod youtube;

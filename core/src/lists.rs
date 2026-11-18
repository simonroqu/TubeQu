//! Kleine lokale Listen in `~/.local/share/tubequ/`: „Später ansehen“, Suchverlauf und die
//! Geschwindigkeit pro Kanal. Alles ohne Anfragen an YouTube.

use qu_core::store::{read, write};
use crate::youtube::{Video, video_from_json, video_to_json};

// ---------- Später ansehen (neueste zuerst) ----------

pub fn later() -> Vec<Video> {
    read("later.jsonl").lines().filter_map(|l| serde_json::from_str(l).ok()).filter_map(|j| video_from_json(&j)).collect()
}

fn save_later(list: &[Video]) {
    write("later.jsonl", &list.iter().map(|v| video_to_json(v).to_string()).collect::<Vec<_>>().join("\n"));
}

pub fn is_later(id: &str) -> bool {
    later().iter().any(|v| v.id == id)
}

/// Merken bzw. wieder entfernen. Gibt zurück, ob das Video jetzt gemerkt ist.
pub fn toggle_later(v: &Video) -> bool {
    let mut list = later();
    let had = list.iter().any(|x| x.id == v.id);
    list.retain(|x| x.id != v.id);
    if !had {
        list.insert(0, v.clone());
    }
    save_later(&list);
    !had
}

/// Fertig geschaut: aus „Später ansehen“ entfernen.
pub fn remove_later(id: &str) {
    let mut list = later();
    let n = list.len();
    list.retain(|x| x.id != id);
    if list.len() != n {
        save_later(&list);
    }
}

// ---------- Geschwindigkeit pro Kanal ----------

/// Zuletzt für diesen Kanal gewählte Geschwindigkeit (sonst normal).
pub fn channel_speed(channel_id: &str) -> f64 {
    qu_core::store::speed(channel_id)
}

pub fn set_channel_speed(channel_id: &str, speed: f64) {
    qu_core::store::set_speed(channel_id, speed);
}

//! Kleine lokale Listen in `~/.local/share/tube/`: „Später ansehen“, Suchverlauf und die
//! Geschwindigkeit pro Kanal. Alles ohne Anfragen an YouTube.

use crate::progress::data_file;
use crate::youtube::{Video, video_from_json, video_to_json};

fn read(name: &str) -> String {
    std::fs::read_to_string(data_file(name)).unwrap_or_default()
}

fn write(name: &str, text: &str) {
    let path = data_file(name);
    let _ = std::fs::create_dir_all(path.parent().unwrap());
    let _ = std::fs::write(path, text);
}

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

// ---------- Suchverlauf ----------

const SEARCHES_MAX: usize = 8;

pub fn searches() -> Vec<String> {
    read("searches.txt").lines().map(String::from).collect()
}

pub fn add_search(q: &str) {
    let mut list = searches();
    list.retain(|s| s != q);
    list.insert(0, q.replace('\n', " "));
    list.truncate(SEARCHES_MAX);
    write("searches.txt", &list.join("\n"));
}

// ---------- Geschwindigkeit pro Kanal ----------

fn parse_speeds(text: &str) -> Vec<(String, f64)> {
    text.lines().filter_map(|l| l.split_once('\t')).filter_map(|(c, s)| Some((c.to_string(), s.parse().ok()?))).collect()
}

/// Zuletzt für diesen Kanal gewählte Geschwindigkeit (sonst normal).
pub fn channel_speed(channel_id: &str) -> f64 {
    parse_speeds(&read("speeds.tsv")).into_iter().find(|(c, _)| c == channel_id).map_or(1.0, |(_, s)| s)
}

pub fn set_channel_speed(channel_id: &str, speed: f64) {
    let mut list = parse_speeds(&read("speeds.tsv"));
    list.retain(|(c, _)| c != channel_id);
    // Normal ist Standard und braucht keinen Eintrag.
    if speed != 1.0 {
        list.push((channel_id.to_string(), speed));
    }
    write("speeds.tsv", &list.iter().map(|(c, s)| format!("{c}\t{s}")).collect::<Vec<_>>().join("\n"));
}

#[cfg(test)]
mod tests {
    #[test]
    fn speeds_parse() {
        let l = super::parse_speeds("UC1\t1.5\nkaputt\nUC2\tx\nUC3\t2");
        assert_eq!(l, vec![("UC1".to_string(), 1.5), ("UC3".to_string(), 2.0)]);
    }
}

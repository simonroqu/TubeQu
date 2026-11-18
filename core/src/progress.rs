//! Merkt sich pro Video, wie weit es geschaut wurde (plus Titel und Kanal für „Weiterschauen“).
//! Datei: ~/.local/share/tubequ/progress.tsv, eine Zeile pro Video:
//! id, Position, Dauer, Zeitstempel, Titel, Kanal, Kanal-ID, Veröffentlichung (die letzten vier können fehlen).

use crate::youtube::Video;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Clone, Debug, PartialEq, Default)]
pub struct Entry {
    pub pos: f64,
    pub dur: f64,
    pub updated: u64,
    pub title: String,
    pub channel: String,
    pub channel_id: String,
    /// Veröffentlichung des Videos (Unix-Zeit), für „3 days ago“ in „Continue watching“.
    pub published_at: Option<i64>,
}

impl Entry {
    pub fn finished(&self) -> bool {
        qu_core::progress::finished(self.pos, self.dur)
    }

    /// Anteil für den Balken auf dem Thumbnail (0.0 – 1.0).
    pub fn fraction(&self) -> f64 {
        qu_core::progress::fraction(self.pos, self.dur)
    }
}

/// Datei in `~/.local/share/tubequ/`.
pub fn data_file(name: &str) -> PathBuf {
    qu_core::app::data_file(name)
}

fn path() -> PathBuf {
    data_file("progress.tsv")
}

/// Alle geschauten Videos (mit Titel), zuletzt geschaute zuerst – für die Verlauf-Seite.
pub fn history() -> Vec<Video> {
    let db = DB.lock().unwrap();
    let mut v: Vec<_> = db.iter().filter(|(_, e)| !e.title.is_empty()).collect();
    v.sort_unstable_by(|a, b| b.1.updated.cmp(&a.1.updated));
    v.into_iter().map(|(id, e)| to_video(id, e)).collect()
}

fn to_video(id: &str, e: &Entry) -> Video {
    Video {
        id: id.to_string(),
        title: e.title.clone(),
        channel: e.channel.clone(),
        channel_id: (!e.channel_id.is_empty()).then(|| e.channel_id.clone()),
        duration: Some(e.dur as u32),
        published_at: e.published_at,
        ..Default::default()
    }
}

fn parse(s: &str) -> HashMap<String, Entry> {
    s.lines()
        .filter_map(|l| {
            let mut f = l.split('\t');
            let id = f.next()?.to_string();
            let pos = f.next()?.parse().ok()?;
            let dur = f.next()?.parse().ok()?;
            let updated = f.next()?.parse().ok()?;
            let mut text = || f.next().unwrap_or_default().to_string();
            let (title, channel, channel_id) = (text(), text(), text());
            let published_at = text().parse().ok();
            Some((id, Entry { pos, dur, updated, title, channel, channel_id, published_at }))
        })
        .collect()
}

/// Tabs und Zeilenumbrüche würden das Dateiformat zerbrechen.
fn clean(s: &str) -> String {
    s.replace(['\t', '\n', '\r'], " ")
}

fn serialize(db: &HashMap<String, Entry>) -> String {
    db.iter()
        .map(|(id, e)| {
            format!(
                "{id}\t{:.1}\t{:.1}\t{}\t{}\t{}\t{}\t{}\n",
                e.pos,
                e.dur,
                e.updated,
                clean(&e.title),
                clean(&e.channel),
                clean(&e.channel_id),
                e.published_at.map(|t| t.to_string()).unwrap_or_default()
            )
        })
        .collect()
}

// ponytail: ganze Datei im Speicher und bei jedem Speichern komplett neu geschrieben.
// Reicht für zehntausende Videos; erst bei mehr auf SQLite umstellen.
static DB: LazyLock<Mutex<HashMap<String, Entry>>> =
    LazyLock::new(|| Mutex::new(parse(&std::fs::read_to_string(path()).unwrap_or_default())));

pub fn get(id: &str) -> Option<Entry> {
    DB.lock().unwrap().get(id).cloned()
}

/// Die zuletzt geschauten Videos, neuestes zuerst.
pub fn recent(n: usize) -> Vec<String> {
    let db = DB.lock().unwrap();
    let mut v: Vec<_> = db.iter().map(|(id, e)| (e.updated, id.clone())).collect();
    v.sort_unstable_by(|a, b| b.cmp(a));
    v.into_iter().take(n).map(|(_, id)| id).collect()
}

/// Angefangene, nicht fertige Videos für „Weiterschauen“, zuletzt geschaute zuerst.
pub fn continue_watching(n: usize) -> Vec<Video> {
    let db = DB.lock().unwrap();
    let mut v: Vec<_> = db.iter().filter(|(_, e)| !e.finished() && !e.title.is_empty()).collect();
    v.sort_unstable_by(|a, b| b.1.updated.cmp(&a.1.updated));
    v.into_iter().take(n).map(|(id, e)| to_video(id, e)).collect()
}

/// Stelle zum Fortsetzen, falls das Video angefangen und nicht fertig ist.
pub fn resume_at(id: &str) -> Option<f64> {
    let e = get(id)?;
    qu_core::progress::resume_from(e.pos, e.dur)
}

/// Titel/Kanal aus den Video-Details, für Videos, die ohne diese Infos gestartet wurden (per Link).
static META: LazyLock<Mutex<HashMap<String, Video>>> = LazyLock::new(Default::default);

pub fn remember_meta(v: Video) {
    META.lock().unwrap().insert(v.id.clone(), v);
}

/// Position speichern (unter 30 s geschaut: nichts tun).
/// Fehlen Titel/Kanal (z. B. per Link gestartet), bleiben die bisher gespeicherten erhalten.
pub fn record(v: &Video, pos: f64, dur: f64) {
    if !qu_core::progress::worth_saving(pos, dur) {
        return;
    }
    let updated = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let meta = META.lock().unwrap().get(&v.id).cloned();
    let v = meta.as_ref().filter(|_| v.title.is_empty()).unwrap_or(v);
    let mut db = DB.lock().unwrap();
    let old = db.remove(&v.id).unwrap_or_default();
    let pick = |new: &str, old: String| if new.is_empty() { old } else { new.to_string() };
    let entry = Entry {
        pos,
        dur,
        updated,
        title: pick(&v.title, old.title),
        channel: pick(&v.channel, old.channel),
        channel_id: pick(v.channel_id.as_deref().unwrap_or_default(), old.channel_id),
        published_at: v.published_at.or(old.published_at),
    };
    db.insert(v.id.clone(), entry);
    // Erst in eine Hilfsdatei schreiben, dann umbenennen: ein Absturz mittendrin zerstört nichts.
    let p = path();
    let tmp = p.with_extension("tsv.tmp");
    let ok = std::fs::create_dir_all(p.parent().unwrap())
        .and_then(|_| std::fs::write(&tmp, serialize(&db)))
        .and_then(|_| std::fs::rename(&tmp, &p));
    if let Err(e) = ok {
        eprintln!("Fortschritt nicht gespeichert: {e}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cfg() {
        qu_core::config::load(include_str!("../../qu.toml"));
    }

    #[test]
    fn rules_and_roundtrip() {
        cfg();
        let half = Entry { pos: 300.0, dur: 600.0, updated: 1, title: "T\tmit Tab".into(), published_at: Some(1_700_000_000), ..Default::default() };
        let done = Entry { pos: 590.0, dur: 600.0, updated: 2, ..Default::default() };
        assert!(!half.finished() && done.finished());
        assert_eq!(half.fraction(), 0.5);
        assert_eq!(done.fraction(), 1.0);

        let db: HashMap<_, _> = [("a".to_string(), half), ("b".to_string(), done)].into();
        let back = parse(&serialize(&db));
        assert_eq!(back["a"].title, "T mit Tab");
        assert_eq!(back["a"].published_at, Some(1_700_000_000));
        assert_eq!(back["b"], db["b"]);
        // Altes Format mit nur vier Spalten bleibt lesbar.
        assert_eq!(parse("x\t40.0\t100.0\t5\n")["x"].pos, 40.0);
        assert!(parse("kaputt\nx\t1\n").is_empty());
    }
}

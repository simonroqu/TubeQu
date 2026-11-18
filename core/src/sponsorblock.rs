//! SponsorBlock: von der Community markierte Abschnitte (Sponsoring, Eigenwerbung,
//! „Abonniert den Kanal“) automatisch überspringen. Fragt sponsor.ajay.app, nicht YouTube.

use sha2::{Digest, Sha256};
use std::sync::LazyLock;

/// Diese Arten werden übersprungen: Kategorie bei SponsorBlock und Anzeige-Name.
const CATEGORIES: [(&str, &str); 3] =
    [("sponsor", "sponsor"), ("selfpromo", "self-promotion"), ("interaction", "subscribe reminder")];

pub use qu_core::segments::Segment;

static HTTP: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

/// Abschnitte zum Überspringen. Leer, wenn es keine gibt oder SponsorBlock nicht erreichbar ist.
pub async fn segments(video_id: String) -> Vec<Segment> {
    // Datenschutz: nur die ersten 4 Zeichen des Hashes werden gesendet; SponsorBlock antwortet
    // mit allen Videos zu diesem Präfix, das richtige suchen wir uns selbst heraus.
    let hash = format!("{:x}", Sha256::digest(video_id.as_bytes()));
    let cats = format!("[{}]", CATEGORIES.map(|(c, _)| format!("\"{c}\"")).join(","));
    let url = format!("https://sponsor.ajay.app/api/skipSegments/{}", &hash[..4]);
    let Ok(res) = HTTP.get(url).query(&[("categories", cats)]).send().await else { return vec![] };
    let Ok(json) = res.json::<serde_json::Value>().await else { return vec![] };
    parse(&json, &video_id)
}

fn parse(json: &serde_json::Value, video_id: &str) -> Vec<Segment> {
    let Some(video) = json.as_array().and_then(|a| a.iter().find(|v| v["videoID"] == video_id)) else {
        return vec![];
    };
    let mut out: Vec<Segment> = video["segments"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|s| s["actionType"].as_str().is_none_or(|a| a == "skip"))
        .filter_map(|s| {
            let label = CATEGORIES.iter().find(|(c, _)| s["category"] == *c)?.1;
            let start = s["segment"][0].as_f64()?;
            let end = s["segment"][1].as_f64()?;
            (end > start).then_some(Segment { start, end, label })
        })
        .collect();
    out.sort_by(|a, b| a.start.total_cmp(&b.start));
    out
}


#[cfg(test)]
mod tests {
    use super::*;
    use qu_core::segments::to_skip;

    #[test]
    fn parses_and_decides() {
        let json = serde_json::json!([
            {"videoID": "anderes", "segments": [{"category": "sponsor", "segment": [1.0, 2.0]}]},
            {"videoID": "abc", "segments": [
                {"category": "selfpromo", "segment": [100.0, 110.0], "actionType": "skip"},
                {"category": "sponsor", "segment": [10.0, 40.0], "actionType": "skip"},
                {"category": "sponsor", "segment": [50.0, 60.0], "actionType": "mute"},
                {"category": "music_offtopic", "segment": [70.0, 80.0]}
            ]}
        ]);
        let s = parse(&json, "abc");
        assert_eq!(s.len(), 2, "nur skip-Abschnitte bekannter Kategorien");
        assert_eq!((s[0].start, s[0].label), (10.0, "sponsor"));
        assert_eq!(to_skip(&s, &[], 15.0), Some(0));
        assert_eq!(to_skip(&s, &[0], 15.0), None, "schon übersprungen: nicht nochmal");
        assert_eq!(to_skip(&s, &[], 39.8), None, "kurz vor Ende nicht mehr springen");
        assert_eq!(to_skip(&s, &[], 45.0), None);
        assert!(parse(&json, "fehlt").is_empty());
    }
}

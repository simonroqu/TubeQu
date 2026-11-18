//! Texte für Karten und Infos (wie bei YouTube), für Desktop und Android gleich.

use crate::youtube::Video;
pub use qu_core::progress::fmt_duration;

/// Aufrufe kurz: 999, 4.5K, 45K, 1.2M, 3B.
pub fn fmt_views(n: u64) -> String {
    let short = |v: f64, unit: &str| {
        // Unter 10 eine Nachkommastelle (4.5K), darüber keine (45K).
        let num = if v < 10.0 { format!("{v:.1}") } else { format!("{v:.0}") };
        format!("{}{unit}", num.trim_end_matches(".0"))
    };
    match n {
        1_000_000_000.. => short(n as f64 / 1e9, "B"),
        1_000_000.. => short(n as f64 / 1e6, "M"),
        1_000.. => short(n as f64 / 1e3, "K"),
        _ => n.to_string(),
    }
}

/// Zeit seit Veröffentlichung wie bei YouTube: "just now", "3 minutes ago", "1 day ago", "2 years ago".
pub fn fmt_ago(secs: i64) -> String {
    const UNITS: [(i64, &str); 6] =
        [(365 * 86400, "year"), (30 * 86400, "month"), (7 * 86400, "week"), (86400, "day"), (3600, "hour"), (60, "minute")];
    UNITS
        .iter()
        .find(|(len, _)| secs >= *len)
        .map(|(len, name)| {
            let n = secs / len;
            format!("{n} {name}{} ago", if n == 1 { "" } else { "s" })
        })
        .unwrap_or_else(|| "just now".into())
}

/// Zeile unter dem Titel: „45K views · 3 days ago“ (live: „1.2K watching“). Veröffentlichung aus dem Zeitpunkt,
/// sonst YouTubes Text.
pub fn meta(v: &Video) -> Vec<String> {
    let unit = if v.is_live { "watching" } else { "views" };
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs() as i64);
    let published = v.published_at.map(|t| fmt_ago(now - t)).or_else(|| v.published.clone()).filter(|_| !v.is_live);
    [v.views.map(|n| format!("{} {unit}", fmt_views(n))), published].into_iter().flatten().collect()
}

/// Ecke des Vorschaubilds: „● LIVE“ oder die Dauer.
pub fn badge(v: &Video) -> Option<String> {
    if v.is_live { Some("● LIVE".to_string()) } else { v.duration.map(fmt_duration) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn formats() {
        assert_eq!(fmt_ago(30), "just now");
        assert_eq!(fmt_ago(60), "1 minute ago");
        assert_eq!(fmt_ago(3 * 60 + 59), "3 minutes ago");
        assert_eq!(fmt_ago(2 * 3600), "2 hours ago");
        assert_eq!(fmt_ago(86400), "1 day ago");
        assert_eq!(fmt_ago(15 * 86400), "2 weeks ago");
        assert_eq!(fmt_ago(400 * 86400), "1 year ago");
        assert_eq!(fmt_duration(59), "0:59");
        assert_eq!(fmt_duration(3725), "1:02:05");
        assert_eq!(fmt_views(999), "999");
        assert_eq!(fmt_views(4_500), "4.5K");
        assert_eq!(fmt_views(45_300), "45K");
        assert_eq!(fmt_views(1_234_567), "1.2M");
        assert_eq!(fmt_views(3_000_000), "3M");
        assert_eq!(fmt_views(2_500_000_000), "2.5B");
    }
}

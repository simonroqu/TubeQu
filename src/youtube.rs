//! Einzige Stelle, die mit YouTube spricht. Ändert YouTube etwas, wird nur hier repariert.

use rustypipe::client::RustyPipe;
use rustypipe::model::VideoItem;
use rustypipe::model::paginator::Paginator;
use rustypipe::param::{ChannelVideoTab, Language};
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::{LazyLock, Mutex as StdMutex};
use std::time::Duration;
use tokio::sync::{Mutex, Semaphore};
use tokio::time::Instant;

/// Mindestabstand zwischen zwei Anfragen an YouTube.
const MIN_GAP: Duration = Duration::from_secs(2);
/// Harte Obergrenze pro Stunde. Darüber verweigert die App weitere Anfragen.
const MAX_PER_HOUR: usize = 200;
/// Wie lange Ergebnisse wiederverwendet werden, ohne YouTube erneut zu fragen.
const CACHE_TTL: Duration = Duration::from_secs(10 * 60);
/// Gleichzeitige Thumbnail-Downloads (Bilder-CDN, ohne Login, zählen nicht zum Limit oben).
const THUMB_PARALLEL: usize = 6;

#[derive(Clone, Debug, Default)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub channel_id: Option<String>,
    pub duration: Option<u32>,
    pub is_live: bool,
    pub views: Option<u64>,
    /// Text von YouTube, z. B. "3 days ago" (falls kein Zeitpunkt bekannt ist).
    pub published: Option<String>,
    /// Veröffentlichung als Unix-Zeit; daraus wird "3 minutes ago" usw. berechnet.
    pub published_at: Option<i64>,
}

impl Video {
    pub fn url(&self) -> String {
        format!("https://www.youtube.com/watch?v={}", self.id)
    }
}

impl From<VideoItem> for Video {
    fn from(v: VideoItem) -> Self {
        Video {
            id: v.id,
            title: v.name,
            channel: v.channel.as_ref().map(|c| c.name.clone()).unwrap_or_default(),
            channel_id: v.channel.map(|c| c.id),
            duration: v.duration,
            is_live: v.is_live,
            views: v.view_count,
            published_at: v.publish_date.map(|d| d.unix_timestamp()),
            published: v.publish_date_txt,
        }
    }
}

/// Eine Seite Ergebnisse; `next` ist gesetzt, wenn es weitere Seiten gibt.
#[derive(Clone)]
pub struct Page {
    pub videos: Vec<Video>,
    pub next: Option<More>,
}

/// Undurchsichtiger Merker für die nächste Seite.
#[derive(Clone)]
pub enum More {
    Yt(Paginator<VideoItem>),
    /// Startseite: ab diesem Eintrag weiter.
    Home(usize),
}

impl From<Paginator<VideoItem>> for Page {
    fn from(mut p: Paginator<VideoItem>) -> Self {
        // Nur normale Videos und Livestreams: keine Shorts, keine angekündigten Premieren/Streams.
        let videos = std::mem::take(&mut p.items)
            .into_iter()
            .filter(|v| !v.is_short && !v.is_upcoming)
            .map(Video::from)
            .collect();
        let next = (!p.is_exhausted()).then_some(More::Yt(p));
        Page { videos, next }
    }
}

fn cache_dir() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache/tube")
}

static RP: LazyLock<RustyPipe> = LazyLock::new(|| {
    let dir = cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    RustyPipe::builder().storage_dir(dir).lang(Language::En).build().expect("rustypipe init")
});

// Zeitpunkte der Anfragen der letzten Stunde. Der Mutex wird während des Wartens gehalten,
// dadurch laufen alle Anfragen strikt nacheinander.
// ponytail: ein globales Limit für alles; getrennte Budgets pro Endpunkt erst, wenn nötig.
static HISTORY: Mutex<VecDeque<Instant>> = Mutex::const_new(VecDeque::new());

/// Wartet, bis eine Anfrage erlaubt ist. Fehler, wenn das Stundenlimit erreicht ist.
pub async fn throttle() -> Result<(), String> {
    let mut hist = HISTORY.lock().await;
    let now = Instant::now();
    while hist.front().is_some_and(|t| now.duration_since(*t) > Duration::from_secs(3600)) {
        hist.pop_front();
    }
    if hist.len() >= MAX_PER_HOUR {
        return Err(format!("Request limit reached ({MAX_PER_HOUR} per hour). Please try again later."));
    }
    if let Some(last) = hist.back() {
        tokio::time::sleep((*last + MIN_GAP).saturating_duration_since(now)).await;
    }
    hist.push_back(Instant::now());
    Ok(())
}

static CACHE: LazyLock<StdMutex<HashMap<String, (Instant, Page)>>> = LazyLock::new(Default::default);

fn cached(key: &str) -> Option<Page> {
    let c = CACHE.lock().unwrap();
    c.get(key).filter(|(t, _)| t.elapsed() < CACHE_TTL).map(|(_, p)| p.clone())
}

fn store(key: String, p: &Page) {
    CACHE.lock().unwrap().insert(key, (Instant::now(), p.clone()));
}

pub async fn search(query: String) -> Result<Page, String> {
    let key = format!("search:{query}");
    if let Some(p) = cached(&key) {
        return Ok(p);
    }
    throttle().await?;
    let res = RP.query().search::<VideoItem, _>(&query).await.map_err(|e| e.to_string())?;
    let page = Page::from(res.items);
    store(key, &page);
    Ok(page)
}

/// Suchvorschläge beim Tippen (wie in der YouTube-Suchleiste).
/// Läuft über YouTubes Vorschlags-Server, nicht über das Anfragen-Limit: das Suchfenster wartet
/// ohnehin, bis man kurz nicht tippt, und Ergebnisse werden zwischengespeichert.
pub async fn suggestions(query: String) -> Vec<String> {
    static SUGGEST: LazyLock<StdMutex<HashMap<String, Vec<String>>>> = LazyLock::new(Default::default);
    if let Some(s) = SUGGEST.lock().unwrap().get(&query) {
        return s.clone();
    }
    let list = RP.query().search_suggestion(&query).await.unwrap_or_default();
    SUGGEST.lock().unwrap().insert(query, list.clone());
    list
}

/// Lädt die nächste Seite (Suche, Kanal, …).
pub async fn more(m: More) -> Result<Page, String> {
    match m {
        More::Home(start) => home_page(start).await,
        More::Yt(p) => {
            throttle().await?;
            let next = p.next(RP.query()).await.map_err(|e| e.to_string())?;
            Ok(next.map(Page::from).unwrap_or(Page { videos: vec![], next: None }))
        }
    }
}

// ---------- Startseite ----------

/// Wie viele Empfehlungen pro Seite geholt werden.
const HOME_BATCH: usize = 30;

/// Browser-Cookies für yt-dlp (`--cookies-from-browser`), einmal beim Start gesucht.
/// Firefox-artige Browser (Zen, Firefox, LibreWolf): das zuletzt benutzte Profil.
/// Sonst Chromium-artige, falls installiert. `None` = kein Browser, App läuft ohne Login.
pub static COOKIES: LazyLock<Option<String>> = LazyLock::new(|| {
    let home = PathBuf::from(std::env::var("HOME").unwrap_or_default());
    let firefox_like = [".config/zen", ".zen", ".mozilla/firefox", ".config/mozilla/firefox", ".librewolf"];
    let newest = firefox_like
        .iter()
        .filter_map(|d| std::fs::read_dir(home.join(d)).ok())
        .flatten()
        .flatten()
        .filter_map(|e| {
            let modified = std::fs::metadata(e.path().join("cookies.sqlite")).ok()?.modified().ok()?;
            Some((modified, e.path()))
        })
        .max_by_key(|(m, _)| *m);
    if let Some((_, profile)) = newest {
        return Some(format!("firefox:{}", profile.display()));
    }
    let chromium_like = [(".config/chromium", "chromium"), (".config/BraveSoftware/Brave-Browser", "brave"), (".config/google-chrome", "chrome")];
    chromium_like.iter().find(|(d, _)| home.join(d).exists()).map(|(_, name)| name.to_string())
});

// ---------- Ansehen-Seite: Details und Abo-Status ----------

/// Was YouTube unter einem Video anzeigt.
#[derive(Clone, Debug)]
pub struct Details {
    pub id: String,
    pub title: String,
    /// Beschreibung als Pango-Markup mit Links (siehe `description_markup`).
    pub description: String,
    pub views: u64,
    pub published: Option<String>,
    pub published_at: Option<i64>,
    pub is_live: bool,
    pub channel_id: String,
    pub channel_name: String,
    pub channel_avatar: Option<String>,
    pub subscribers: Option<u64>,
}

/// Beschreibung als Pango-Markup. Link-Ziele:
/// `seek:<sek>` = Zeitstempel in diesem Video, `channel:<id>` = Kanal in der App,
/// YouTube-Video-Links und sonstige Web-Links als normale URL.
/// Jede Zeile ist für sich gültiges Markup, damit man nach Zeilen kürzen kann.
fn description_markup(rt: &rustypipe::model::richtext::RichText, video_id: &str) -> String {
    use rustypipe::model::UrlTarget;
    use rustypipe::model::richtext::TextComponent;
    let esc = |t: &str| glib_escape(t);
    let mut out = String::new();
    for c in &rt.0 {
        let (text, href) = match c {
            TextComponent::Text { text, .. } => (text, None),
            TextComponent::Web { text, url } => (text, Some(url.clone())),
            TextComponent::YouTube { text, target } => (
                text,
                Some(match target {
                    UrlTarget::Video { id, start_time } if id == video_id => format!("seek:{start_time}"),
                    UrlTarget::Video { id, start_time } => format!("https://www.youtube.com/watch?v={id}&t={start_time}"),
                    UrlTarget::Channel { id } => format!("channel:{id}"),
                    UrlTarget::Playlist { id } | UrlTarget::Album { id } => format!("https://www.youtube.com/playlist?list={id}"),
                    #[allow(unreachable_patterns)]
                    _ => String::new(),
                }),
            ),
            #[allow(unreachable_patterns)]
            _ => continue,
        };
        // Pro Zeile einzeln verlinken, damit kein Tag über einen Zeilenumbruch reicht.
        let pieces: Vec<String> = text
            .split('\n')
            .map(|line| match href.as_deref().filter(|h| !h.is_empty() && !line.is_empty()) {
                Some(h) => format!("<a href=\"{}\">{}</a>", esc(h), esc(line)),
                None => esc(line),
            })
            .collect();
        out.push_str(&pieces.join("\n"));
    }
    out
}

/// Text für Pango-Markup unschädlich machen (ohne GTK-Abhängigkeit in diesem Modul).
fn glib_escape(t: &str) -> String {
    t.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;").replace('\'', "&#39;")
}

fn https(url: &str) -> String {
    if url.starts_with("//") { format!("https:{url}") } else { url.to_string() }
}

pub async fn details(id: String) -> Result<Details, String> {
    static DETAILS: LazyLock<StdMutex<HashMap<String, (Instant, Details)>>> = LazyLock::new(Default::default);
    if let Some((t, d)) = DETAILS.lock().unwrap().get(&id)
        && t.elapsed() < CACHE_TTL
    {
        return Ok(d.clone());
    }
    throttle().await?;
    let d = RP.query().video_details(&id).await.map_err(|e| e.to_string())?;
    let description = description_markup(&d.description, &d.id);
    let details = Details {
        id: d.id,
        title: d.name,
        description,
        views: d.view_count,
        published: d.publish_date_txt,
        published_at: d.publish_date.map(|t| t.unix_timestamp()),
        is_live: d.is_live,
        channel_id: d.channel.id,
        channel_name: d.channel.name,
        channel_avatar: d.channel.avatar.iter().find(|t| t.width >= 48).or(d.channel.avatar.last()).map(|t| https(&t.url)),
        subscribers: d.channel.subscriber_count,
    };
    DETAILS.lock().unwrap().insert(id, (Instant::now(), details.clone()));
    Ok(details)
}

/// Video in deinem YouTube-Verlauf als gesehen eintragen (nur mit Browser-Login),
/// damit YouTube deine Empfehlungen weiter verbessert. Abgespielt wird ohne Login:
/// mit Cookies liefert YouTube Stream-Adressen, die sich nicht öffnen lassen.
pub async fn mark_watched(id: String) -> Result<(), String> {
    let Some(cookies) = COOKIES.clone() else { return Ok(()) };
    throttle().await?;
    let out = tokio::process::Command::new("yt-dlp")
        .args(["--cookies-from-browser", &cookies, "--mark-watched", "--skip-download"])
        .args(["--ignore-no-formats-error", "--quiet", "--no-warnings"])
        .arg(format!("https://www.youtube.com/watch?v={id}"))
        .output()
        .await
        .map_err(|e| format!("Couldn't run yt-dlp: {e}"))?;
    if out.status.success() { Ok(()) } else { Err(String::from_utf8_lossy(&out.stderr).trim().to_string()) }
}

/// Ein Eintrag aus `yt-dlp -j --flat-playlist`.
/// Shorts und angekündigte (noch nicht laufende) Streams ergeben `None`, wie bei `Page::from`.
fn video_from_json(j: &serde_json::Value) -> Option<Video> {
    let s = |k: &str| j.get(k).and_then(|v| v.as_str()).map(String::from);
    if s("url").is_some_and(|u| u.contains("/shorts/")) || s("live_status").as_deref() == Some("is_upcoming") {
        return None;
    }
    Some(Video {
        id: s("id")?,
        title: s("title")?,
        channel: s("channel").or_else(|| s("uploader")).unwrap_or_default(),
        channel_id: s("channel_id"),
        duration: j.get("duration").and_then(|v| v.as_f64()).map(|d| d as u32),
        is_live: s("live_status").as_deref() == Some("is_live"),
        views: j.get("view_count").and_then(|v| v.as_u64()),
        published: None,
        published_at: j.get("timestamp").and_then(|v| v.as_i64()),
    })
}

/// Startseite: deine YouTube-Empfehlungen (mit Login), sonst eigene aus ähnlichen Videos.
pub async fn home() -> Result<Page, String> {
    if COOKIES.is_none() {
        return own_recommendations().await;
    }
    match home_page(0).await {
        Ok(p) if !p.videos.is_empty() => Ok(p),
        Ok(_) => own_recommendations().await,
        Err(e) => {
            eprintln!("YouTube-Startseite nicht verfügbar, nutze eigene Empfehlungen: {e}");
            own_recommendations().await
        }
    }
}

async fn home_page(start: usize) -> Result<Page, String> {
    let key = format!("home:{start}");
    if let Some(p) = cached(&key) {
        return Ok(p);
    }
    let cookies = COOKIES.clone().ok_or("No browser login found")?;
    throttle().await?;
    let out = tokio::process::Command::new("yt-dlp")
        .args(["--cookies-from-browser", &cookies, "--flat-playlist", "-j"])
        .args(["--playlist-start", &(start + 1).to_string(), "--playlist-end", &(start + HOME_BATCH).to_string()])
        .arg(":ytrec")
        .output()
        .await
        .map_err(|e| format!("Couldn't run yt-dlp: {e}"))?;
    let videos: Vec<Video> = String::from_utf8_lossy(&out.stdout)
        .lines()
        .filter_map(|l| serde_json::from_str(l).ok())
        .filter_map(|j| video_from_json(&j))
        .collect();
    if videos.is_empty() && !out.status.success() {
        let err = String::from_utf8_lossy(&out.stderr);
        return Err(err.lines().rfind(|l| l.contains("ERROR")).unwrap_or("yt-dlp failed").to_string());
    }
    // Weiter, solange yt-dlp eine volle Seite geliefert hat (gefilterte Einträge zählen mit).
    let lines = String::from_utf8_lossy(&out.stdout).lines().count();
    let next = (lines >= HOME_BATCH).then_some(More::Home(start + HOME_BATCH));
    let page = Page { videos, next };
    store(key, &page);
    Ok(page)
}

/// Ohne Login: „ähnliche Videos“ der zuletzt geschauten Videos, gemischt, ohne bereits fertig Geschautes.
async fn own_recommendations() -> Result<Page, String> {
    // ponytail: die letzten 3 Videos als Ausgangspunkt (= 3 Anfragen); mehr, wenn es zu eintönig wird.
    let seeds = crate::progress::recent(3);
    if seeds.is_empty() {
        return Err("No recommendations yet. Search for something and watch a few videos.".into());
    }
    let mut lists = Vec::new();
    for id in seeds {
        if let Some(p) = cached(&format!("related:{id}")) {
            lists.push(p.videos);
            continue;
        }
        throttle().await?;
        let Ok(d) = RP.query().video_details(&id).await else { continue };
        let page = Page::from(d.recommended);
        store(format!("related:{id}"), &page);
        lists.push(page.videos);
    }
    // Reißverschluss: abwechselnd aus jeder Liste, damit kein Ausgangsvideo dominiert.
    let mut seen = std::collections::HashSet::new();
    let mut videos = Vec::new();
    for i in 0..lists.iter().map(Vec::len).max().unwrap_or(0) {
        for l in &lists {
            if let Some(v) = l.get(i)
                && seen.insert(v.id.clone())
                && !crate::progress::get(&v.id).is_some_and(|e| e.finished())
            {
                videos.push(v.clone());
            }
        }
    }
    Ok(Page { videos, next: None })
}

#[derive(Clone, Debug)]
pub struct ChannelInfo {
    pub id: String,
    pub name: String,
    pub avatar: Option<String>,
    pub has_live: bool,
}

/// Kanal-Infos plus erste Seite des Tabs (`live` = Livestreams statt Videos).
pub async fn channel(id: String, live: bool) -> Result<(ChannelInfo, Page), String> {
    let key = format!("channel:{id}:{live}");
    static CHANNELS: LazyLock<StdMutex<HashMap<String, ChannelInfo>>> = LazyLock::new(Default::default);
    if let Some(p) = cached(&key)
        && let Some(info) = CHANNELS.lock().unwrap().get(&id).cloned()
    {
        return Ok((info, p));
    }
    throttle().await?;
    let tab = if live { ChannelVideoTab::Live } else { ChannelVideoTab::Videos };
    let c = RP.query().channel_videos_tab(&id, tab).await.map_err(|e| e.to_string())?;
    // Avatar in mittlerer Größe; URLs kommen teils ohne Schema ("//yt3…").
    let avatar = c.avatar.iter().find(|t| t.width >= 88).or(c.avatar.last()).map(|t| {
        if t.url.starts_with("//") { format!("https:{}", t.url) } else { t.url.clone() }
    });
    // ponytail: subscriber_count liefert rustypipe 0.11.4 falsch (Lofi Girl: 481), daher weggelassen.
    let info = ChannelInfo { id: c.id, name: c.name, avatar, has_live: c.has_live };
    let mut page = Page::from(c.content);
    // Auf der Kanalseite fehlt der Kanal an den einzelnen Videos.
    for v in &mut page.videos {
        if v.channel.is_empty() {
            v.channel = info.name.clone();
            v.channel_id = Some(info.id.clone());
        }
    }
    CHANNELS.lock().unwrap().insert(id, info.clone());
    store(key, &page);
    Ok((info, page))
}

static THUMB_SLOTS: Semaphore = Semaphore::const_new(THUMB_PARALLEL);
static HTTP: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

/// Thumbnail (320×180 JPEG) als Bytes; einmal geladen, kommt es von der Festplatte.
pub async fn thumbnail(id: String) -> Result<Vec<u8>, String> {
    image(format!("https://i.ytimg.com/vi/{id}/mqdefault.jpg"), format!("{id}.jpg")).await
}

/// Kanal-Avatar als Bytes, ebenfalls auf der Festplatte zwischengespeichert.
pub async fn avatar(url: String) -> Result<Vec<u8>, String> {
    use std::hash::{Hash, Hasher};
    let mut h = std::collections::hash_map::DefaultHasher::new();
    url.hash(&mut h);
    image(url, format!("avatar-{:x}", h.finish())).await
}

/// Bild von YouTubes Bilder-CDN laden (ohne Login, daher nicht im Anfragen-Limit).
async fn image(url: String, file: String) -> Result<Vec<u8>, String> {
    let path = cache_dir().join("thumbs").join(file);
    if let Ok(b) = tokio::fs::read(&path).await {
        return Ok(b);
    }
    let _slot = THUMB_SLOTS.acquire().await.map_err(|e| e.to_string())?;
    let res = HTTP.get(url).send().await.and_then(|r| r.error_for_status()).map_err(|e| e.to_string())?;
    let bytes = res.bytes().await.map_err(|e| e.to_string())?.to_vec();
    // ponytail: Cache wächst unbegrenzt (~15 KB pro Bild); Aufräumen nach Alter, falls es stört.
    let _ = tokio::fs::create_dir_all(path.parent().unwrap()).await;
    let _ = tokio::fs::write(&path, &bytes).await;
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn description_links() {
        use rustypipe::model::richtext::RichText;
        let rt: RichText = serde_json::from_value(serde_json::json!([
            {"Text": {"text": "Intro <1>\n"}},
            {"YouTube": {"text": "1:30", "target": {"Video": {"id": "me", "start_time": 90}}}},
            {"Text": {"text": " & "}},
            {"Web": {"text": "a\nb", "url": "https://x.y/?a=1&b=2"}},
            {"YouTube": {"text": "@Kanal", "target": {"Channel": {"id": "UC1"}}}}
        ]))
        .unwrap();
        let m = description_markup(&rt, "me");
        assert_eq!(
            m,
            "Intro &lt;1&gt;\n<a href=\"seek:90\">1:30</a> &amp; <a href=\"https://x.y/?a=1&amp;b=2\">a</a>\n<a href=\"https://x.y/?a=1&amp;b=2\">b</a><a href=\"channel:UC1\">@Kanal</a>"
        );
    }

    #[test]
    fn parses_yt_dlp_json() {
        let j: serde_json::Value = serde_json::from_str(r#"{"id":"abc","title":"T","channel":"C","channel_id":"UC1","duration":125.0,"live_status":"is_live","view_count":null}"#).unwrap();
        let v = video_from_json(&j).unwrap();
        assert_eq!((v.id.as_str(), v.duration, v.is_live, v.views), ("abc", Some(125), true, None));
        assert!(video_from_json(&serde_json::json!({"title": "ohne id"})).is_none());
        let short = serde_json::json!({"id": "s", "title": "S", "url": "https://www.youtube.com/shorts/s"});
        let upcoming = serde_json::json!({"id": "u", "title": "U", "live_status": "is_upcoming"});
        assert!(video_from_json(&short).is_none() && video_from_json(&upcoming).is_none());
    }

    #[tokio::test(start_paused = true)]
    async fn throttle_spaces_and_caps() {
        let start = Instant::now();
        throttle().await.unwrap();
        throttle().await.unwrap();
        assert!(start.elapsed() >= MIN_GAP, "zweite Anfrage muss warten");
        for _ in 2..MAX_PER_HOUR {
            throttle().await.unwrap();
        }
        assert!(throttle().await.is_err(), "Stundenlimit muss greifen");
    }

    /// Prüft gegen das echte YouTube, ob Suche, Nachladen und Thumbnails noch gehen:
    /// `cargo test -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn live_search() {
        let p = search("lofi".into()).await.unwrap();
        assert!(!p.videos.is_empty());
        println!("{:#?}", &p.videos[..2]);
        let p2 = more(p.next.expect("weitere Seite")).await.unwrap();
        assert!(!p2.videos.is_empty());
        let jpg = thumbnail(p.videos[0].id.clone()).await.unwrap();
        assert_eq!(&jpg[..2], &[0xFF, 0xD8], "JPEG erwartet");

        let cid = p.videos[0].channel_id.clone().expect("Kanal-ID");
        let (info, vids) = channel(cid.clone(), false).await.unwrap();
        println!("{info:#?}");
        assert!(!vids.videos.is_empty() && !vids.videos[0].channel.is_empty());
        assert!(!avatar(info.avatar.expect("Avatar")).await.unwrap().is_empty());
        if info.has_live {
            channel(cid, true).await.unwrap();
        }

        // Startseite mit Browser-Login (falls vorhanden)
        let d = details(p.videos[0].id.clone()).await.unwrap();
        println!("Details: {} · {} Aufrufe · {:?} Abos · {} Zeichen Beschreibung", d.channel_name, d.views, d.subscribers, d.description.len());
        assert!(!d.title.is_empty() && !d.channel_id.is_empty());

        if COOKIES.is_some() {
            let h = home_page(0).await.unwrap();
            println!("Startseite: {} Videos, erstes: {:?}", h.videos.len(), h.videos.first().map(|v| &v.title));
            assert!(!h.videos.is_empty());
        }
    }
}

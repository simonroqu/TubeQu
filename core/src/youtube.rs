//! Einzige Stelle, die mit YouTube spricht. Ändert YouTube etwas, wird nur hier repariert.

use rustypipe::client::RustyPipe;
use rustypipe::model::VideoItem;
use rustypipe::model::paginator::Paginator;
use rustypipe::param::Language;
use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
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
    /// yt-dlp mit Login (Startseite `:ytrec`, Suche `ytsearchall:…`): ab diesem Eintrag weiter.
    YtDlp(String, usize),
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
    qu_core::app::cache_dir()
}

static RP: LazyLock<RustyPipe> = LazyLock::new(|| {
    let dir = cache_dir();
    let _ = std::fs::create_dir_all(&dir);
    // Nur HTTP/1.1: über HTTP/2 bricht YouTube die Anfragen ab („stream error … protocol error“, gesehen 2026-11-18
    // mit rustls auf Android). Am Desktop (native-tls ohne ALPN) war es ohnehin schon HTTP/1.1 – dort ändert sich nichts.
    RustyPipe::builder()
        .storage_dir(dir)
        .lang(Language::En)
        .build_with_client(reqwest::ClientBuilder::new().http1_only())
        .expect("rustypipe init")
});

// Zeitpunkte der Anfragen der letzten Stunde. Der Mutex wird während des Wartens gehalten,
// dadurch laufen alle Anfragen strikt nacheinander.
// ponytail: ein globales Limit für alles; getrennte Budgets pro Endpunkt erst, wenn nötig.
static HISTORY: Mutex<VecDeque<Instant>> = Mutex::const_new(VecDeque::new());
static HISTORY_LOADED: std::sync::atomic::AtomicBool = std::sync::atomic::AtomicBool::new(false);

/// Anfragen der letzten Stunde in einer Datei (Unix-Sekunden je Zeile), damit Limit und
/// Mindestabstand auch über Neustarts gelten (Bot-Sperre nach vielen Neustarts, 2026-11-06).
fn requests_file() -> PathBuf {
    cache_dir().join("requests.txt")
}

fn unix_now() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_secs())
}

/// Zeitpunkte aus der Datei, die jünger als eine Stunde sind (als `Instant`), älteste zuerst.
fn recent_requests(text: &str, now_unix: u64, now: Instant) -> VecDeque<Instant> {
    let mut v: Vec<u64> = text.lines().filter_map(|l| l.trim().parse().ok()).filter(|&t| now_unix.saturating_sub(t) < 3600 && t <= now_unix).collect();
    v.sort_unstable();
    v.into_iter().map(|t| now.checked_sub(Duration::from_secs(now_unix - t)).unwrap_or(now)).collect()
}

/// Wartet, bis eine Anfrage erlaubt ist. Fehler, wenn das Stundenlimit erreicht ist.
pub async fn throttle() -> Result<(), String> {
    let mut hist = HISTORY.lock().await;
    let persist = !cfg!(test);
    if persist && !HISTORY_LOADED.swap(true, std::sync::atomic::Ordering::Relaxed) {
        let text = std::fs::read_to_string(requests_file()).unwrap_or_default();
        let now_unix = unix_now();
        *hist = recent_requests(&text, now_unix, Instant::now());
        // Datei auf die letzte Stunde kürzen.
        let keep: String = text.lines().filter(|l| l.trim().parse::<u64>().is_ok_and(|t| now_unix.saturating_sub(t) < 3600)).map(|l| format!("{}\n", l.trim())).collect();
        let _ = std::fs::create_dir_all(cache_dir());
        let _ = std::fs::write(requests_file(), keep);
    }
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
    if persist {
        use std::io::Write;
        if let Ok(mut f) = std::fs::OpenOptions::new().create(true).append(true).open(requests_file()) {
            let _ = writeln!(f, "{}", unix_now());
        }
    }
    Ok(())
}

static CACHE: LazyLock<StdMutex<HashMap<String, (Instant, Page)>>> = LazyLock::new(Default::default);

fn cached(key: &str) -> Option<Page> {
    let c = CACHE.lock().unwrap();
    c.get(key).filter(|(t, _)| t.elapsed() < CACHE_TTL).map(|(_, p)| p.clone())
}

/// Zwischengespeicherte Seiten verwerfen (Reload soll wirklich neu laden).
pub fn forget_cache() {
    CACHE.lock().unwrap().clear();
}

fn store(key: String, p: &Page) {
    CACHE.lock().unwrap().insert(key, (Instant::now(), p.clone()));
}

/// Suche. Mit Browser-Login über yt-dlp, damit YouTube die Ergebnisse wie auf der Website
/// personalisiert (Verlauf, Abos); ohne Login oder bei Fehler anonym über rustypipe.
pub async fn search(query: String) -> Result<Page, String> {
    if COOKIES.is_some() {
        match ytdlp_page(format!("ytsearchall:{query}"), 0).await {
            Ok(p) => return Ok(p),
            Err(e) => eprintln!("Suche mit Login fehlgeschlagen, suche anonym: {e}"),
        }
    }
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
        More::YtDlp(target, start) => ytdlp_page(target, start).await,
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
/// Mit Login: Browser-Cookies (Desktop) oder vom Desktop übertragener Login (Fernseher/Handy, `pair`).
/// Nur für die innertube-Aufrufe (Verlauf, Abo, Konto); yt-dlp-Wege hängen weiter an `COOKIES`.
pub fn logged_in() -> bool {
    COOKIES.is_some() || crate::pair::stored().is_some()
}

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

/// Ersatz-Details aus dem yt-dlp-JSON, das mpv beim Abspielen ohnehin holt (keine eigene Anfrage).
/// Für Videos, bei denen rustypipe scheitert (z. B. neues Kanal-Format bei Gemeinschafts-Videos).
/// Ohne Kanal-Logo; das holt `channel_avatar`.
pub fn details_from_ytdlp(json: &str) -> Option<Details> {
    let j = ytdlp_json(json)?;
    let s = |k: &str| j.get(k).and_then(|v| v.as_str()).map(String::from);
    let id = s("id")?;
    let description = plain_description_markup(&s("description").unwrap_or_default());
    Some(Details {
        title: s("title").unwrap_or_default(),
        description,
        views: j.get("view_count").and_then(|v| v.as_u64()).unwrap_or(0),
        published: None,
        published_at: j.get("timestamp").and_then(|v| v.as_i64()),
        is_live: j.get("is_live").and_then(|v| v.as_bool()).unwrap_or(false),
        channel_id: s("channel_id").unwrap_or_default(),
        channel_name: s("channel").or_else(|| s("uploader")).unwrap_or_default(),
        channel_avatar: None,
        subscribers: j.get("channel_follower_count").and_then(|v| v.as_u64()),
        id,
    })
}

/// yt-dlp-JSON lesen. mpv liefert es als JSON-Text in Anführungszeichen: dann noch einmal auspacken.
fn ytdlp_json(json: &str) -> Option<serde_json::Value> {
    let j: serde_json::Value = serde_json::from_str(json).ok()?;
    match j.as_str() {
        Some(inner) => serde_json::from_str(inner).ok(),
        None => Some(j),
    }
}

/// Vorschaubilder für die Timeline: YouTube-„Storyboard“, Bögen mit `cols`×`rows` kleinen Bildern.
pub use qu_core::storyboard::Storyboard;

/// Storyboard mit den größten Bildern aus dem yt-dlp-JSON (holt mpv beim Laden ohnehin; die Bögen
/// kommen vom Bilder-CDN i.ytimg.com wie die Thumbnails, nicht über das Anfragen-Limit).
pub fn storyboard_from_ytdlp(json: &str) -> Option<Storyboard> {
    let j = ytdlp_json(json)?;
    let f = j.get("formats")?.as_array()?.iter()
        .filter(|f| f.get("format_note").and_then(|v| v.as_str()) == Some("storyboard"))
        .max_by_key(|f| f.get("width").and_then(|v| v.as_i64()).unwrap_or(0))?;
    let num = |k: &str| f.get(k).and_then(|v| v.as_f64());
    let sheets: Vec<String> = f.get("fragments")?.as_array()?.iter()
        .filter_map(|x| x.get("url").and_then(|u| u.as_str()).map(String::from))
        .collect();
    Some(Storyboard {
        width: num("width")? as i32,
        height: num("height")? as i32,
        cols: num("columns")? as u32,
        rows: num("rows")? as u32,
        fps: num("fps")?,
        sheets,
    })
    .filter(|s| !s.sheets.is_empty() && s.cols > 0 && s.rows > 0)
}

/// Klartext-Beschreibung als Pango-Markup; Web-Adressen werden zu Links.
// ponytail: nur http(s)-Links; Zeitstempel/Kanal-Links gibt es nur über rustypipe.
fn plain_description_markup(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            line.split(' ')
                .map(|w| {
                    if w.starts_with("https://") || w.starts_with("http://") {
                        format!("<a href=\"{0}\">{0}</a>", glib_escape(w))
                    } else {
                        glib_escape(w)
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Kanal-Logo (URL) über die Kanal-Anfrage; die ist danach zwischengespeichert, ein Klick auf den
/// Kanal kostet dann keine weitere Anfrage.
pub async fn channel_avatar(id: String) -> Option<String> {
    channel(id).await.ok()?.0.avatar
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

/// Video in deinem YouTube-Verlauf eintragen (nur mit Browser-Login), damit YouTube deine
/// Empfehlungen weiter verbessert. Abgespielt wird ohne Login: mit Cookies liefert YouTube
/// Stream-Adressen, die sich nicht öffnen lassen. Eine `player`-Anfrage über `throttle()` liefert die
/// Tracking-Adressen; die Playback-Meldung trägt ins Verlauf ein, die echte Position kommt später über
/// `report_position` (nicht wie `yt-dlp --mark-watched`, das immer „bis zum Ende geschaut“ meldet).
pub async fn mark_watched(id: String) -> Result<(), String> {
    if !logged_in() {
        return Ok(());
    }
    let cookies = browser_cookies().await?;
    let account = account();
    let body = serde_json::json!({ "context": innertube_context(), "videoId": id });
    let text = innertube(&cookies, account, "player", &body).await?;
    let j: serde_json::Value = serde_json::from_str(&text).map_err(|e| e.to_string())?;
    let url = |k: &str| j["playbackTracking"][k]["baseUrl"].as_str().map(String::from);
    let (Some(playback), Some(watchtime)) = (url("videostatsPlaybackUrl"), url("videostatsWatchtimeUrl")) else {
        return Err("No tracking addresses in the player response".into());
    };
    // ponytail: Dummy-cpn statt des Algorithmus aus base.js – laut yt-dlp reicht das.
    let cpn: String = format!("{:016x}", std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_nanos())
        .chars()
        .rev()
        .take(16)
        .collect();
    let t = Tracking { id, cookies, account, watchtime, cpn };
    stats_ping(&t, &stats_url(&playback, &t.cpn, 0.0, false)).await?;
    *TRACKING.lock().unwrap() = Some(t);
    Ok(())
}

/// Tracking-Daten des zuletzt eingetragenen Videos, für `report_position`.
#[derive(Clone)]
struct Tracking {
    id: String,
    cookies: Vec<(String, String)>,
    account: u32,
    watchtime: String,
    cpn: String,
}

static TRACKING: StdMutex<Option<Tracking>> = StdMutex::new(None);

/// Wie weit du das Video geschaut hast, an YouTube melden (Fortschrittsbalken dort, Weiterschauen am TV).
/// Keine `player`-Anfrage, nur eine Meldung an `s.youtube.com` (geht darum nicht über `throttle()`).
/// Tut nichts, wenn das Video nicht (oder noch nicht) über `mark_watched` eingetragen wurde.
pub async fn report_position(id: String, pos: f64) -> Result<(), String> {
    let Some(t) = TRACKING.lock().unwrap().clone().filter(|t| t.id == id) else { return Ok(()) };
    stats_ping(&t, &stats_url(&t.watchtime, &t.cpn, pos, true)).await
}

/// Tracking-Adresse mit Position `pos` (Sekunden). `watchtime`: zusätzlich der geschaute Abschnitt 0…pos.
fn stats_url(base: &str, cpn: &str, pos: f64, watchtime: bool) -> String {
    let Ok(mut url) = reqwest::Url::parse(base) else { return base.to_string() };
    let keep: Vec<(String, String)> = url
        .query_pairs()
        .filter(|(k, _)| !matches!(k.as_ref(), "ver" | "cpn" | "cmt" | "el" | "st" | "et"))
        .map(|(k, v)| (k.into_owned(), v.into_owned()))
        .collect();
    let pos = format!("{pos:.3}");
    let mut q = url.query_pairs_mut();
    q.clear().extend_pairs(keep).extend_pairs([("ver", "2"), ("cpn", cpn), ("cmt", &pos), ("el", "detailpage")]);
    if watchtime {
        q.extend_pairs([("st", "0"), ("et", &pos)]);
    }
    drop(q);
    url.into()
}

async fn stats_ping(t: &Tracking, url: &str) -> Result<(), String> {
    let cookie = t.cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ");
    let res = HTTP
        .get(url)
        .header("Cookie", cookie)
        .header("X-Goog-AuthUser", t.account.to_string())
        .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0")
        .timeout(Duration::from_secs(5))
        .send()
        .await
        .map_err(|e| format!("Couldn't reach YouTube: {e}"))?;
    if res.status().is_success() { Ok(()) } else { Err(format!("YouTube refused ({})", res.status())) }
}

/// Ein Eintrag aus `yt-dlp -j --flat-playlist`.
/// Shorts, angekündigte (noch nicht laufende) Streams und Videos nur für Kanalmitglieder/Premium
/// ergeben `None`, wie bei `Page::from`.
pub fn video_from_json(j: &serde_json::Value) -> Option<Video> {
    let s = |k: &str| j.get(k).and_then(|v| v.as_str()).map(String::from);
    if s("url").is_some_and(|u| u.contains("/shorts/"))
        || s("live_status").as_deref() == Some("is_upcoming")
        || matches!(s("availability").as_deref(), Some("subscriber_only" | "premium_only"))
    {
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
    let page = if COOKIES.is_none() {
        own_recommendations().await
    } else {
        match ytdlp_page(":ytrec".into(), 0).await {
            Ok(p) if !p.videos.is_empty() => Ok(p),
            Ok(_) => own_recommendations().await,
            Err(e) => {
                eprintln!("YouTube-Startseite nicht verfügbar, nutze eigene Empfehlungen: {e}");
                own_recommendations().await
            }
        }
    };
    if let Ok(p) = &page {
        save_home(p);
    }
    page
}

/// Letzte Startseite auf der Festplatte: beim App-Start sofort zeigen, während die neue lädt.
fn home_file() -> PathBuf {
    cache_dir().join("home.jsonl")
}

pub fn video_to_json(v: &Video) -> serde_json::Value {
    serde_json::json!({
        "id": v.id, "title": v.title, "channel": v.channel, "channel_id": v.channel_id,
        "duration": v.duration, "live_status": if v.is_live { "is_live" } else { "not_live" },
        "view_count": v.views, "timestamp": v.published_at,
    })
}

fn save_home(p: &Page) {
    let lines: Vec<String> = p.videos.iter().map(|v| video_to_json(v).to_string()).collect();
    let _ = std::fs::write(home_file(), lines.join("\n"));
}

/// Zuletzt geladene Startseite (ohne Nachladen), `None` wenn es noch keine gibt.
/// Gespeicherte Startseite jünger als der Cache? Dann holt ein Neustart sie nicht neu: Limit und
/// Cache leben nur im Speicher, wiederholte Neustarts sollen nicht jedes Mal YouTube fragen.
pub fn saved_home_fresh() -> bool {
    std::fs::metadata(home_file()).and_then(|m| m.modified()).ok().and_then(|t| t.elapsed().ok()).is_some_and(|age| age < CACHE_TTL)
}

pub fn saved_home() -> Option<Page> {
    let text = std::fs::read_to_string(home_file()).ok()?;
    let videos: Vec<Video> =
        text.lines().filter_map(|l| serde_json::from_str(l).ok()).filter_map(|j| video_from_json(&j)).collect();
    (!videos.is_empty()).then_some(Page { videos, next: None })
}

/// Eine Seite aus einer yt-dlp-Liste mit Browser-Login (`:ytrec` = Startseite, `ytsearchall:…` = Suche).
/// yt-dlp blättert für Seite k intern ab Anfang (k Abrufe in einem Aufruf): darum zählen sie als k Anfragen.
/// ponytail: tiefes Nachladen wird dadurch langsamer (2 s je Seite); eigene Fortsetzungs-Token, falls das stört.
async fn ytdlp_page(target: String, start: usize) -> Result<Page, String> {
    let key = format!("ytdlp:{}:{target}:{start}", account());
    if let Some(p) = cached(&key) {
        return Ok(p);
    }
    for _ in 0..=start / HOME_BATCH {
        throttle().await?;
    }
    let mut cmd = tokio::process::Command::new("yt-dlp");
    // Kanäle gehen auch ohne Login; Startseite und Suche rufen das hier nur mit Login auf.
    if let Some(cookies) = COOKIES.as_deref() {
        cmd.args(["--cookies-from-browser", cookies, "--add-header", &account_header()]);
    }
    let out = cmd
        .args(["--flat-playlist", "-j"])
        .args(["--playlist-start", &(start + 1).to_string(), "--playlist-end", &(start + HOME_BATCH).to_string()])
        // Ungefähres Datum („3 days ago“) aus dem Text der Liste, ohne zusätzliche Anfrage.
        .args(["--extractor-args", "youtubetab:approximate_date"])
        .arg("--")
        .arg(&target)
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
    let next = (lines >= HOME_BATCH).then(|| More::YtDlp(target, start + HOME_BATCH));
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
    pub subscribers: Option<u64>,
}

/// Kanal-Infos plus erste Seite der Videos. Über yt-dlp statt rustypipe: rustypipe liefert bei
/// Kanal-Videos seit einer YouTube-Änderung weder Datum noch Aufrufe noch Dauer. yt-dlp hat
/// Aufrufe, Dauer und (mit `approximate_date`) das ungefähre Datum. Eine Anfrage, weitere Seiten
/// wie bei der Startseite über `ytdlp_page`. Mit Browser-Login über das gewählte Konto.
pub async fn channel(id: String) -> Result<(ChannelInfo, Page), String> {
    let key = format!("channel:{id}");
    static CHANNELS: LazyLock<StdMutex<HashMap<String, ChannelInfo>>> = LazyLock::new(Default::default);
    if let Some(p) = cached(&key)
        && let Some(info) = CHANNELS.lock().unwrap().get(&id).cloned()
    {
        return Ok((info, p));
    }
    throttle().await?;
    #[cfg(not(target_os = "android"))]
    let (info, mut page) = channel_ytdlp(&id).await?;
    // Android hat kein yt-dlp: rustypipe (Videos dann ohne Dauer, Aufrufe und Datum, siehe „Bekannte Lücken“).
    #[cfg(target_os = "android")]
    let (info, mut page) = channel_rp(&id).await?;
    // Auf der Kanalseite fehlt der Kanal an den einzelnen Videos.
    for v in &mut page.videos {
        if v.channel.is_empty() {
            v.channel = info.name.clone();
        }
        v.channel_id.get_or_insert_with(|| info.id.clone());
    }
    CHANNELS.lock().unwrap().insert(id, info.clone());
    store(key, &page);
    Ok((info, page))
}

/// Kanal über `yt-dlp -J --flat-playlist` (Desktop). Die Anfrage zählt der Aufrufer (`throttle`).
#[cfg(not(target_os = "android"))]
async fn channel_ytdlp(id: &str) -> Result<(ChannelInfo, Page), String> {
    let url = format!("https://www.youtube.com/channel/{id}/videos");
    let mut cmd = tokio::process::Command::new("yt-dlp");
    if let Some(cookies) = COOKIES.as_deref() {
        cmd.args(["--cookies-from-browser", cookies, "--add-header", &account_header()]);
    }
    let out = cmd
        .args(["-J", "--flat-playlist", "--playlist-end", &HOME_BATCH.to_string()])
        .args(["--extractor-args", "youtubetab:approximate_date"])
        .arg("--")
        .arg(&url)
        .output()
        .await
        .map_err(|e| format!("Couldn't run yt-dlp: {e}"))?;
    let json: serde_json::Value = serde_json::from_slice(&out.stdout).map_err(|_| {
        let err = String::from_utf8_lossy(&out.stderr);
        err.lines().rfind(|l| l.contains("ERROR")).unwrap_or("yt-dlp failed").to_string()
    })?;
    let (info, page) = channel_from_json(&json, id, url);
    Ok((info, page))
}

/// Kanal über rustypipe (Android, ohne yt-dlp). Die Anfrage zählt der Aufrufer (`throttle`).
#[cfg(target_os = "android")]
async fn channel_rp(id: &str) -> Result<(ChannelInfo, Page), String> {
    let c = RP.query().channel_videos(id).await.map_err(|e| e.to_string())?;
    let avatar = c.avatar.iter().find(|t| t.width >= 88).or(c.avatar.last()).map(|t| https(&t.url));
    let info = ChannelInfo { id: c.id, name: c.name, avatar, subscribers: c.subscriber_count };
    Ok((info, Page::from(c.content)))
}

/// Video-ID aus einem YouTube-Link (watch?v=…, youtu.be/…, shorts/…).
pub fn video_id(url: &str) -> Option<String> {
    let rest = url.split_once("v=").or_else(|| url.split_once("youtu.be/")).or_else(|| url.split_once("/shorts/"))?.1;
    let id: String = rest.chars().take_while(|c| c.is_ascii_alphanumeric() || *c == '-' || *c == '_').collect();
    (id.len() == 11).then_some(id)
}

/// Kanal-Infos und erste Seite aus `yt-dlp -J --flat-playlist`. `url` = Kanal-Tab fürs Nachladen.
fn channel_from_json(j: &serde_json::Value, id: &str, url: String) -> (ChannelInfo, Page) {
    let s = |k: &str| j.get(k).and_then(|v| v.as_str()).map(String::from);
    // Avatar: unbeschnittenes Bild (`=s0`, volle Größe) auf 176 px verkleinert anfordern.
    let avatar = j["thumbnails"]
        .as_array()
        .and_then(|t| t.iter().find(|t| t["id"] == "avatar_uncropped"))
        .and_then(|t| t["url"].as_str())
        .map(|u| https(&u.replace("=s0", "=s176")));
    let entries = j["entries"].as_array().map(Vec::as_slice).unwrap_or_default();
    let info = ChannelInfo {
        id: s("channel_id").unwrap_or_else(|| id.to_string()),
        name: s("channel").or_else(|| s("uploader")).unwrap_or_default(),
        avatar,
        subscribers: j["channel_follower_count"].as_u64(),
    };
    // Weiter, solange yt-dlp eine volle Seite geliefert hat (gefilterte Einträge zählen mit).
    let next = (entries.len() >= HOME_BATCH).then(|| More::YtDlp(url, HOME_BATCH));
    (info, Page { videos: entries.iter().filter_map(video_from_json).collect(), next })
}

// ---------- Konto und Abonnieren ----------

/// Welches der im Browser angemeldeten Google-Konten die App nutzt (0 = zuerst angemeldetes).
/// Gilt für alles mit Login: Startseite, Suche, als gesehen melden, Abonnieren.
static ACCOUNT: LazyLock<AtomicU32> = LazyLock::new(|| {
    AtomicU32::new(std::fs::read_to_string(account_file()).ok().and_then(|s| s.trim().parse().ok()).unwrap_or(0))
});

fn account_file() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".config/tubequ/account")
}

pub fn account() -> u32 {
    ACCOUNT.load(Ordering::Relaxed)
}

/// Konto wechseln und merken. Zwischengespeicherte Listen gehörten zum alten Konto.
pub fn set_account(n: u32) {
    ACCOUNT.store(n, Ordering::Relaxed);
    let file = account_file();
    let _ = std::fs::create_dir_all(file.parent().unwrap());
    let _ = std::fs::write(file, n.to_string());
    forget_cache();
}

/// Für yt-dlp: YouTube liefert mit diesem Header die Seite des gewählten Kontos, yt-dlp übernimmt
/// daraus `SESSION_INDEX` für alle weiteren Anfragen (getestet 2026-11-07).
fn account_header() -> String {
    format!("X-Goog-AuthUser:{}", account())
}

/// Zusätzliche `ytdl-raw-options` für mpv, um mit Browser-Login abzuspielen (altersbeschränkte Videos:
/// „Sign in to confirm your age“). `None` = kein Browser gefunden. Client `tv`: unterstützt Cookies und
/// braucht keinen PO-Token (`visionos` ignoriert Cookies).
pub fn login_ytdl_options() -> Option<String> {
    let c = COOKIES.as_deref()?;
    Some(format!("cookies-from-browser={c},add-header={},extractor-args=youtube:player_client=tv", account_header()))
}

#[derive(Clone, Debug, PartialEq)]
pub struct Account {
    pub index: u32,
    pub name: String,
    pub email: String,
    /// Profilbild (yt3.ggpht.com, über `avatar` geladen und auf der Platte zwischengespeichert).
    pub photo: Option<String>,
}

/// Bekannte Konten in `~/.cache/tubequ/accounts.tsv` (Nummer, Name, E-Mail, Bild), damit das
/// Bild des aktiven Kontos beim Start ohne Anfrage da ist.
fn accounts_file() -> PathBuf {
    cache_dir().join("accounts.tsv")
}

fn parse_accounts(text: &str) -> Vec<Account> {
    text.lines()
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            Some(Account {
                index: f.first()?.parse().ok()?,
                name: f.get(1)?.to_string(),
                email: f.get(2)?.to_string(),
                photo: f.get(3).filter(|p| !p.is_empty()).map(|p| p.to_string()),
            })
        })
        .collect()
}

fn saved_accounts() -> Vec<Account> {
    parse_accounts(&std::fs::read_to_string(accounts_file()).unwrap_or_default())
}

/// Konten in die Datei übernehmen (gleiche Nummer = ersetzt).
fn save_accounts(new: &[Account]) {
    let mut all = saved_accounts();
    all.retain(|a| !new.iter().any(|n| n.index == a.index));
    all.extend_from_slice(new);
    all.sort_by_key(|a| a.index);
    let text: String = all
        .iter()
        .map(|a| format!("{}\t{}\t{}\t{}\n", a.index, a.name.replace('\t', " "), a.email, a.photo.as_deref().unwrap_or("")))
        .collect();
    let _ = std::fs::write(accounts_file(), text);
}

/// Das aktive Konto: aus der Datei, sonst einmalig eine Anfrage (danach gespeichert).
pub async fn current_account() -> Result<Account, String> {
    let n = account();
    if let Some(a) = saved_accounts().into_iter().find(|a| a.index == n) {
        return Ok(a);
    }
    let cookies = browser_cookies().await?;
    let a = fetch_account(&cookies, n).await?.ok_or("No account found")?;
    save_accounts(std::slice::from_ref(&a));
    Ok(a)
}

/// Alle im Browser angemeldeten Konten. YouTube nennt pro Anfrage nur das Konto der mitgeschickten
/// Nummer: also 0, 1, 2 … abfragen, bis eine Nummer fehlschlägt oder ein Konto doppelt kommt.
/// Eine Anfrage pro Konto (+1), höchstens einmal pro Sitzung (erst wenn man die Liste aufklappt).
pub async fn accounts() -> Result<Vec<Account>, String> {
    static LIST: Mutex<Vec<Account>> = Mutex::const_new(Vec::new());
    let mut list = LIST.lock().await;
    if !list.is_empty() {
        return Ok(list.clone());
    }
    let cookies = browser_cookies().await?;
    // ponytail: höchstens 4 Konten, mehr hat kaum jemand im Browser angemeldet.
    for index in 0..4 {
        let Ok(Some(a)) = fetch_account(&cookies, index).await else { break };
        if list.iter().any(|b| b.email == a.email) {
            break;
        }
        list.push(a);
    }
    if list.is_empty() {
        return Err("No account found".into());
    }
    save_accounts(&list);
    Ok(list.clone())
}

/// Ein Konto über `account/accounts_list` (eine Anfrage). `None` = keins unter dieser Nummer.
async fn fetch_account(cookies: &[(String, String)], index: u32) -> Result<Option<Account>, String> {
    let body = serde_json::json!({ "context": innertube_context() });
    let text = innertube(cookies, index, "account/accounts_list", &body).await?;
    Ok(account_from_json(&serde_json::from_str(&text).unwrap_or_default(), index))
}

/// Name und E-Mail aus dem Kopf der Liste, Bild vom gewählten Eintrag (bei Marken-Kanälen
/// stehen mehrere Einträge darunter).
fn account_from_json(json: &serde_json::Value, index: u32) -> Option<Account> {
    let h = find_key(json, "googleAccountHeaderRenderer")?;
    let txt = |k: &str| h.pointer(&format!("/{k}/simpleText")).and_then(|v| v.as_str()).unwrap_or_default().to_string();
    let email = txt("email");
    if email.is_empty() {
        return None;
    }
    let mut items = Vec::new();
    collect_key(json, "accountItem", &mut items);
    let item = items.iter().find(|i| i["isSelected"] == true).or(items.first());
    // Bild in doppelter Größe (48 → 96 px), damit es auf HiDPI scharf bleibt.
    let photo = item
        .and_then(|i| i.pointer("/accountPhoto/thumbnails/0/url"))
        .and_then(|u| u.as_str())
        .map(|u| u.replace("=s48-", "=s96-"));
    Some(Account { index, name: txt("name"), email, photo })
}

/// Erstes Objekt unter dem Schlüssel `key`, egal wie tief.
fn find_key<'a>(v: &'a serde_json::Value, key: &str) -> Option<&'a serde_json::Value> {
    match v {
        serde_json::Value::Object(m) => m.get(key).or_else(|| m.values().find_map(|v| find_key(v, key))),
        serde_json::Value::Array(a) => a.iter().find_map(|v| find_key(v, key)),
        _ => None,
    }
}

/// Alle Objekte unter dem Schlüssel `key`, egal wie tief.
fn collect_key<'a>(v: &'a serde_json::Value, key: &str, out: &mut Vec<&'a serde_json::Value>) {
    match v {
        serde_json::Value::Object(m) => {
            if let Some(x) = m.get(key) {
                out.push(x);
            }
            m.values().for_each(|v| collect_key(v, key, out));
        }
        serde_json::Value::Array(a) => a.iter().for_each(|v| collect_key(v, key, out)),
        _ => {}
    }
}

/// Kanal abonnieren (`true`) oder deabonnieren (`false`) – über die interne YouTube-API, wie
/// der Knopf auf youtube.com. Genau eine Anfrage über `throttle()`, keine Wiederholung.
pub async fn subscribe(channel_id: String, on: bool) -> Result<(), String> {
    let cookies = browser_cookies().await?;
    let action = if on { "subscribe" } else { "unsubscribe" };
    let body = serde_json::json!({ "context": innertube_context(), "channelIds": [channel_id] });
    let text = innertube(&cookies, account(), &format!("subscription/{action}"), &body).await?;
    // HTTP 200 heißt nicht, dass es geklappt hat: YouTube meldet den neuen Zustand des Knopfs.
    let want = format!("\"subscribed\":{on}");
    if text.replace(' ', "").contains(&want) { Ok(()) } else { Err("YouTube didn't confirm the change".into()) }
}

fn innertube_context() -> serde_json::Value {
    serde_json::json!({ "client": { "clientName": "WEB", "clientVersion": "2.20261101.00.00", "hl": "en" } })
}

/// youtube.com-Cookies aus dem Browser. yt-dlp liest sie aus (kann auch Chromium entschlüsseln):
/// ohne URL bricht es mit Fehler ab, schreibt die Cookie-Datei aber trotzdem – ohne Netzwerk.
/// Die Datei liegt nur kurz im Cache.
async fn browser_cookies() -> Result<Vec<(String, String)>, String> {
    // Fernseher/Handy: vom Desktop übertragener Login (`pair`), dort gibt es keinen Browser.
    if let Some(l) = crate::pair::stored() {
        return Ok(l.cookies);
    }
    let browser = COOKIES.clone().ok_or("Not signed in to YouTube in a browser")?;
    let file = PathBuf::from(std::env::var("HOME").unwrap_or_default()).join(".cache/tubequ/cookies.tmp");
    tokio::process::Command::new("yt-dlp")
        .args(["--cookies-from-browser", &browser, "--cookies"])
        .arg(&file)
        .output()
        .await
        .map_err(|e| format!("Couldn't run yt-dlp: {e}"))?;
    let jar = std::fs::read_to_string(&file).unwrap_or_default();
    let _ = std::fs::remove_file(&file);
    let cookies = youtube_cookies(&jar);
    if cookies.iter().any(|(k, _)| k == "SAPISID") { Ok(cookies) } else { Err("Not signed in to YouTube in a browser".into()) }
}

/// Login für den Fernseher (`pair::send`): Browser-Cookies und gewähltes Konto (Name/Bild aus dem Konto-Cache),
/// ohne Anfrage an YouTube.
pub async fn login_for_tv() -> Result<crate::pair::Login, String> {
    let cookies = browser_cookies().await?;
    let account = account();
    let a = saved_accounts().into_iter().find(|a| a.index == account);
    Ok(crate::pair::Login {
        cookies,
        account,
        name: a.as_ref().map(|a| a.name.clone()).unwrap_or_default(),
        email: a.as_ref().map(|a| a.email.clone()).unwrap_or_default(),
        photo: a.and_then(|a| a.photo),
    })
}

/// Eine Anfrage an die interne YouTube-API mit Login (über `throttle()`), Antwort als Text.
/// `account` = Nummer des Google-Kontos im Browser (`X-Goog-AuthUser`).
async fn innertube(cookies: &[(String, String)], account: u32, endpoint: &str, body: &serde_json::Value) -> Result<String, String> {
    let now = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap_or_default().as_secs();
    let get = |name: &str| cookies.iter().find(|(k, _)| k == name).map(|(_, v)| v.clone());
    // Wie der Browser: alle drei Varianten, die neueren mit ihren eigenen Cookies.
    let auth = [("SAPISIDHASH", get("SAPISID")), ("SAPISID1PHASH", get("__Secure-1PAPISID")), ("SAPISID3PHASH", get("__Secure-3PAPISID"))]
        .into_iter()
        .filter_map(|(label, v)| Some(format!("{label} {}", sapisid_hash(&v?, now))))
        .collect::<Vec<_>>()
        .join(" ");
    let cookie = cookies.iter().map(|(k, v)| format!("{k}={v}")).collect::<Vec<_>>().join("; ");
    throttle().await?;
    let res = HTTP
        .post(format!("{YT_ORIGIN}/youtubei/v1/{endpoint}?prettyPrint=false"))
        .header("Authorization", auth)
        .header("Origin", YT_ORIGIN)
        .header("X-Origin", YT_ORIGIN)
        .header("X-Goog-AuthUser", account.to_string())
        .header("Cookie", cookie)
        .header("User-Agent", "Mozilla/5.0 (X11; Linux x86_64; rv:140.0) Gecko/20100101 Firefox/140.0")
        .header("Content-Type", "application/json")
        .body(body.to_string())
        .send()
        .await
        .map_err(|e| format!("Couldn't reach YouTube: {e}"))?;
    let status = res.status();
    if !status.is_success() {
        return Err(format!("YouTube refused ({status})"));
    }
    res.text().await.map_err(|e| e.to_string())
}

const YT_ORIGIN: &str = "https://www.youtube.com";

/// youtube.com-Cookies (Name, Wert) aus einer Netscape-Cookie-Datei.
fn youtube_cookies(jar: &str) -> Vec<(String, String)> {
    jar.lines()
        .map(|l| l.strip_prefix("#HttpOnly_").unwrap_or(l))
        .filter(|l| !l.starts_with('#'))
        .filter_map(|l| {
            let f: Vec<&str> = l.split('\t').collect();
            (f.len() == 7 && f[0].trim_start_matches('.').ends_with("youtube.com")).then(|| (f[5].to_string(), f[6].to_string()))
        })
        .collect()
}

/// Ein Teil des Login-Headers: `<zeit>_<sha1("<zeit> <cookie> <origin>")>`.
fn sapisid_hash(sapisid: &str, now: u64) -> String {
    use sha1::{Digest, Sha1};
    let hash = Sha1::digest(format!("{now} {sapisid} {YT_ORIGIN}"));
    format!("{now}_{}", hash.iter().map(|b| format!("{b:02x}")).collect::<String>())
}

static THUMB_SLOTS: Semaphore = Semaphore::const_new(THUMB_PARALLEL);
static HTTP: LazyLock<reqwest::Client> = LazyLock::new(reqwest::Client::new);

/// Thumbnail (480×360 JPEG, 16:9-Bild mit Balken oben/unten – `ContentFit::Cover` schneidet sie weg)
/// als Bytes; einmal geladen, kommt es von der Festplatte. Groß genug für 4 Spalten im Vollbild.
pub async fn thumbnail(id: String) -> Result<Vec<u8>, String> {
    image(format!("https://i.ytimg.com/vi/{id}/hqdefault.jpg"), format!("{id}-hq.jpg")).await
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
    #[test]
    fn video_id_from_links() {
        assert_eq!(super::video_id("https://www.youtube.com/watch?v=sF80I-TQiW0&t=3").as_deref(), Some("sF80I-TQiW0"));
        assert_eq!(super::video_id("https://youtu.be/sF80I-TQiW0").as_deref(), Some("sF80I-TQiW0"));
        assert_eq!(super::video_id("https://www.youtube.com/channel/UCxyz"), None);
    }

    #[test]
    fn stats_url_sets_position() {
        let u = super::stats_url("https://s.youtube.com/api/stats/watchtime?ns=yt&len=600.1&cmt=0&docid=abc", "xyz", 120.5, true);
        let q: std::collections::HashMap<_, _> = reqwest::Url::parse(&u).unwrap().query_pairs().into_owned().collect();
        assert_eq!((q["cmt"].as_str(), q["st"].as_str(), q["et"].as_str()), ("120.500", "0", "120.500"));
        assert_eq!((q["docid"].as_str(), q["cpn"].as_str(), q["el"].as_str(), q["len"].as_str()), ("abc", "xyz", "detailpage", "600.1"));
        let p = super::stats_url("https://s.youtube.com/api/stats/playback?docid=abc", "xyz", 0.0, false);
        assert!(!p.contains("et=") && p.contains("cmt=0.000"));
    }

    #[test]
    fn saved_video_round_trip() {
        let v = super::Video { id: "a".into(), title: "T".into(), channel: "C".into(), duration: Some(9), is_live: true, published_at: Some(5), ..Default::default() };
        let back = super::video_from_json(&super::video_to_json(&v)).unwrap();
        assert_eq!((back.id, back.title, back.duration, back.is_live, back.published_at), ("a".into(), "T".into(), Some(9), true, Some(5)));
    }

    #[test]
    fn storyboard_lookup() {
        let j = r#"{"formats":[
            {"format_note":"storyboard","width":48,"height":27,"columns":10,"rows":10,"fps":0.5,"fragments":[{"url":"s0"}]},
            {"format_note":"storyboard","width":160,"height":90,"columns":5,"rows":5,"fps":0.5,"fragments":[{"url":"a"},{"url":"b"}]},
            {"format_note":"1080p","width":1920}]}"#;
        let sb = super::storyboard_from_ytdlp(j).unwrap();
        assert_eq!((sb.width, sb.cols, sb.sheets.len()), (160, 5, 2));
        // 0,5 Bilder/s: t=0 -> Bild 0; t=14 -> Bild 7 (Reihe 1, Spalte 2); t=60 -> Bild 30 = Bogen 1, Bild 5.
        assert_eq!(sb.locate(0.0), (0, 0, 0));
        assert_eq!(sb.locate(14.0), (0, 2 * 160, 90));
        assert_eq!(sb.locate(60.0), (1, 0, 90));
        // Hinter dem Ende: letztes Bild.
        assert_eq!(sb.locate(9999.0), (1, 4 * 160, 4 * 90));
    }

    #[test]
    fn details_from_ytdlp_json() {
        let j = r#"{"id":"abc","title":"T","description":"Hi <b>\nhttps://x.y/a?b=1&c=2 more","view_count":12,"channel":"C","channel_id":"UC1","channel_follower_count":5}"#;
        let d = super::details_from_ytdlp(j).unwrap();
        assert_eq!((d.id.as_str(), d.views, d.subscribers, d.channel_id.as_str()), ("abc", 12, Some(5), "UC1"));
        assert_eq!(d.description, "Hi &lt;b&gt;\n<a href=\"https://x.y/a?b=1&amp;c=2\">https://x.y/a?b=1&amp;c=2</a> more");
        assert!(super::details_from_ytdlp("kaputt").is_none());
        // So kommt es von mpv: doppelt verpackt.
        let wrapped = serde_json::to_string(j).unwrap();
        assert_eq!(super::details_from_ytdlp(&wrapped).unwrap().id, "abc");
    }

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
        let members = serde_json::json!({"id": "m", "title": "M", "availability": "subscriber_only"});
        let public = serde_json::json!({"id": "p", "title": "P", "availability": "public"});
        assert!(video_from_json(&members).is_none() && video_from_json(&public).is_some());
    }

    #[test]
    fn requests_from_file() {
        let now = Instant::now();
        // Zu alt (> 1 h), Müll, Zukunft fliegen raus; Rest sortiert, Abstand zu jetzt bleibt.
        let h = recent_requests("1000\nxx\n9000\n10000\n8000\n99999\n", 10_000, now);
        assert_eq!(h.len(), 3);
        assert_eq!(now.duration_since(h[0]), Duration::from_secs(2000));
        assert_eq!(now.duration_since(h[2]), Duration::ZERO);
    }

    #[test]
    fn channel_parsing() {
        let j = serde_json::json!({
            "channel": "Mosch", "channel_id": "UCx", "channel_follower_count": 1200,
            "thumbnails": [{"url": "https://yt3/banner=w1060"}, {"url": "https://yt3/av=s0", "id": "avatar_uncropped"}],
            "entries": [
                {"id": "a", "title": "A", "duration": 61.0, "view_count": 5, "timestamp": 1_700_000_000},
                {"id": "b", "title": "Short", "url": "https://www.youtube.com/shorts/b"}
            ]
        });
        let (info, page) = channel_from_json(&j, "UCx", "u".into());
        assert_eq!((info.name.as_str(), info.avatar.as_deref()), ("Mosch", Some("https://yt3/av=s176")));
        assert_eq!(info.subscribers, Some(1200));
        assert_eq!(page.videos.len(), 1, "Shorts raus");
        assert_eq!((page.videos[0].duration, page.videos[0].views, page.videos[0].published_at), (Some(61), Some(5), Some(1_700_000_000)));
        assert!(page.next.is_none(), "keine volle Seite = Ende");
    }

    #[test]
    fn account_parsing() {
        let j: serde_json::Value = serde_json::from_str(r#"{"actions":[{"menu":{"header":{"googleAccountHeaderRenderer":{"name":{"simpleText":"Simon"},"email":{"simpleText":"s@x.com"}}},"items":[{"accountItem":{"isSelected":false,"accountPhoto":{"thumbnails":[{"url":"https://a/b=s48-c"}]}}},{"accountItem":{"isSelected":true,"accountPhoto":{"thumbnails":[{"url":"https://a/c=s48-c"}]}}}]}}]}"#).unwrap();
        let a = account_from_json(&j, 1).unwrap();
        assert_eq!(a, Account { index: 1, name: "Simon".into(), email: "s@x.com".into(), photo: Some("https://a/c=s96-c".into()) });
        assert!(account_from_json(&serde_json::json!({}), 0).is_none());
        assert_eq!(parse_accounts("1\tSimon\ts@x.com\thttps://a/c=s96-c\nxx\n0\tA\ta@x\t\n"), [a, Account { index: 0, name: "A".into(), email: "a@x".into(), photo: None }]);
    }

    #[test]
    fn subscribe_auth() {
        let jar = "# Netscape\n.youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tabc\n#HttpOnly_.youtube.com\tTRUE\t/\tTRUE\t0\tSID\tx\n.google.com\tTRUE\t/\tTRUE\t0\tNID\ty\n";
        assert_eq!(youtube_cookies(jar), [("SAPISID".into(), "abc".into()), ("SID".into(), "x".into())]);
        assert_eq!(sapisid_hash("abc", 1), "1_76945995212dc850298a9feccea18a02639c6f64");
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
        let (info, vids) = channel(cid.clone()).await.unwrap();
        println!("{info:#?}");
        assert!(!vids.videos.is_empty() && !vids.videos[0].channel.is_empty());
        assert!(!avatar(info.avatar.expect("Avatar")).await.unwrap().is_empty());
        assert!(vids.videos[0].published_at.is_some(), "ungefähres Datum");

        // Startseite mit Browser-Login (falls vorhanden)
        let d = details(p.videos[0].id.clone()).await.unwrap();
        println!("Details: {} · {} Aufrufe · {:?} Abos · {} Zeichen Beschreibung", d.channel_name, d.views, d.subscribers, d.description.len());
        assert!(!d.title.is_empty() && !d.channel_id.is_empty());

        if COOKIES.is_some() {
            let h = ytdlp_page(":ytrec".into(), 0).await.unwrap();
            println!("Startseite: {} Videos, erstes: {:?}", h.videos.len(), h.videos.first().map(|v| &v.title));
            assert!(!h.videos.is_empty());
        }
    }
}

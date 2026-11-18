//! tubequ-core für Kotlin (UniFFI). Nur Durchreichen: eigene Typen als `Record`, Netz-Aufrufe laufen auf
//! qu-cores tokio (`bg`), Kotlin wartet per `suspend`. Zuerst `init` aufrufen (Ordner, qu.toml).
//! Alle YouTube-Anfragen gehen weiter durch `youtube::throttle()` (Limit, Mindestabstand, Cache) – nichts zusätzlich.

use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};
use tubequ_core::{fmt, lists, progress, sponsorblock, youtube};

uniffi::setup_scaffolding!();

#[derive(Debug, uniffi::Error)]
pub enum TubeError {
    Failed { msg: String },
}

impl std::fmt::Display for TubeError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        let TubeError::Failed { msg } = self;
        f.write_str(msg)
    }
}

impl From<String> for TubeError {
    fn from(msg: String) -> Self {
        TubeError::Failed { msg }
    }
}

#[derive(Clone, uniffi::Record)]
pub struct Video {
    pub id: String,
    pub title: String,
    pub channel: String,
    pub channel_id: Option<String>,
    pub duration: Option<u32>,
    pub is_live: bool,
    pub views: Option<u64>,
    pub published: Option<String>,
    pub published_at: Option<i64>,
}

impl From<youtube::Video> for Video {
    fn from(v: youtube::Video) -> Self {
        Video {
            id: v.id,
            title: v.title,
            channel: v.channel,
            channel_id: v.channel_id,
            duration: v.duration,
            is_live: v.is_live,
            views: v.views,
            published: v.published,
            published_at: v.published_at,
        }
    }
}

impl From<Video> for youtube::Video {
    fn from(v: Video) -> Self {
        youtube::Video {
            id: v.id,
            title: v.title,
            channel: v.channel,
            channel_id: v.channel_id,
            duration: v.duration,
            is_live: v.is_live,
            views: v.views,
            published: v.published,
            published_at: v.published_at,
        }
    }
}

/// Fertige Karte wie am Desktop (`grid.rs`): Texte aus `fmt`, Vorschaubild, Fortschritt.
#[derive(uniffi::Record)]
pub struct Card {
    pub video: Video,
    /// „Kanal · 45K views · 3 days ago“.
    pub meta: String,
    /// Dauer oder „● LIVE“.
    pub badge: Option<String>,
    /// 16:9-Vorschaubild (Bilder-CDN, zählt nicht zum Limit).
    pub thumbnail: String,
    /// Geschauter Anteil, falls angefangen.
    pub fraction: Option<f64>,
}

fn card(v: youtube::Video) -> Card {
    let meta = std::iter::once(v.channel.clone()).filter(|c| !c.is_empty()).chain(fmt::meta(&v)).collect::<Vec<_>>().join(" · ");
    Card {
        meta,
        badge: fmt::badge(&v),
        thumbnail: format!("https://i.ytimg.com/vi/{}/mqdefault.jpg", v.id),
        fraction: progress::get(&v.id).map(|e| e.fraction()),
        video: v.into(),
    }
}

fn cards(v: Vec<youtube::Video>) -> Vec<Card> {
    v.into_iter().map(card).collect()
}

/// Eine Seite; `next` = Merker für `more` (die Fortsetzung selbst bleibt hier in Rust).
#[derive(uniffi::Record)]
pub struct Page {
    pub cards: Vec<Card>,
    pub next: Option<u64>,
}

static MORE: LazyLock<Mutex<HashMap<u64, youtube::More>>> = LazyLock::new(Default::default);

fn page(p: youtube::Page) -> Page {
    let next = p.next.map(|m| {
        let mut map = MORE.lock().unwrap();
        let id = map.len() as u64 + 1;
        map.insert(id, m);
        id
    });
    Page { cards: cards(p.videos), next }
}

#[derive(uniffi::Record)]
pub struct Segment {
    pub start: f64,
    pub end: f64,
    pub label: String,
}

/// Einmal beim Start: Ordner der App (`filesDir`, `cacheDir`) und Einstellungen aus qu.toml.
#[uniffi::export]
pub fn init(files_dir: String, cache_dir: String) {
    qu_core::app::set_dirs(files_dir.into(), cache_dir.into());
    qu_core::app::init("tubequ");
    qu_core::config::load(include_str!("../../qu.toml"));
}

/// Startseite: ohne Login eigene Empfehlungen aus ähnlichen Videos der zuletzt geschauten (wie am Desktop).
#[uniffi::export]
pub async fn home() -> Result<Page, TubeError> {
    Ok(page(qu_core::bg(youtube::home()).await?))
}

/// Letzte Startseite (sofort, ohne Netz).
#[uniffi::export]
pub fn saved_home() -> Option<Page> {
    youtube::saved_home().map(page)
}

/// Startseite neu holen lohnt erst nach Ablauf (sonst die gemerkte zeigen, spart Anfragen).
#[uniffi::export]
pub fn saved_home_fresh() -> bool {
    youtube::saved_home_fresh()
}

#[uniffi::export]
pub async fn search(query: String) -> Result<Page, TubeError> {
    Ok(page(qu_core::bg(youtube::search(query)).await?))
}

/// Nächste Seite zu `next` einer früheren Seite.
#[uniffi::export]
pub async fn more(next: u64) -> Result<Page, TubeError> {
    let m = MORE.lock().unwrap().remove(&next).ok_or_else(|| "Nothing more".to_string())?;
    Ok(page(qu_core::bg(youtube::more(m)).await?))
}

/// Vor jeder Extraktion in Kotlin (NewPipeExtractor) aufrufen: dasselbe Limit wie alle YouTube-Anfragen
/// (Mindestabstand, höchstens 200 pro Stunde, über Neustarts gemerkt). Fehler = Limit erreicht.
#[uniffi::export]
pub async fn throttle() -> Result<(), TubeError> {
    Ok(qu_core::bg(youtube::throttle()).await?)
}

/// SponsorBlock-Abschnitte (sponsor.ajay.app, keine YouTube-Anfrage).
#[uniffi::export]
pub async fn segments(id: String) -> Vec<Segment> {
    let v = qu_core::bg(sponsorblock::segments(id)).await;
    v.into_iter().map(|s| Segment { start: s.start, end: s.end, label: s.label.into() }).collect()
}

#[uniffi::export]
pub fn continue_watching(n: u32) -> Vec<Card> {
    cards(progress::continue_watching(n as usize))
}

#[uniffi::export]
pub fn history() -> Vec<Card> {
    cards(progress::history())
}

#[uniffi::export]
pub fn later() -> Vec<Card> {
    cards(lists::later())
}

#[uniffi::export]
pub fn is_later(id: String) -> bool {
    lists::is_later(&id)
}

/// Gibt zurück, ob das Video jetzt auf „Watch later“ steht.
#[uniffi::export]
pub fn toggle_later(v: Video) -> bool {
    lists::toggle_later(&v.into())
}

#[uniffi::export]
pub fn resume_at(id: String) -> Option<f64> {
    progress::resume_at(&id)
}

/// Fortschritt speichern (alle paar Sekunden und beim Verlassen).
#[uniffi::export]
pub fn record(v: Video, pos: f64, dur: f64) {
    progress::record(&v.into(), pos, dur);
}

/// Tempo pro Kanal (wie am Desktop).
#[uniffi::export]
pub fn speed(channel_id: String) -> f64 {
    lists::channel_speed(&channel_id)
}

/// Video-ID aus einem YouTube-Link (geteilt oder geöffnet).
#[uniffi::export]
pub fn video_id(url: String) -> Option<String> {
    youtube::video_id(&url)
}

#[uniffi::export]
pub fn searches() -> Vec<String> {
    qu_core::store::searches()
}

#[uniffi::export]
pub fn add_search(query: String) {
    qu_core::store::add_search(&query);
}


/// Konto, mit dem die App angemeldet ist (vom Desktop übertragen).
#[derive(uniffi::Record)]
pub struct Account {
    pub name: String,
    pub email: String,
    pub photo: Option<String>,
}

#[derive(uniffi::Record)]
pub struct PairCode {
    /// Für den Bildschirm, z. B. „112-4829-1374“.
    pub display: String,
    /// Zum Prüfen (nur an `pair_receive` geben).
    pub secret: String,
}

fn account_of(l: tubequ_core::pair::Login) -> Account {
    Account { name: l.name, email: l.email, photo: l.photo }
}

/// Angemeldet? (Login vom Desktop übertragen.)
#[uniffi::export]
pub fn signed_in() -> Option<Account> {
    tubequ_core::pair::stored().map(account_of)
}

#[uniffi::export]
pub fn sign_out() {
    tubequ_core::pair::forget();
}

/// Neuer Code zum Anzeigen; danach `pair_receive(secret)`.
#[uniffi::export]
pub fn pair_code() -> Result<PairCode, TubeError> {
    let (display, secret) = tubequ_core::pair::new_code()?;
    Ok(PairCode { display, secret })
}

/// Bricht die Hintergrund-Aufgabe ab, wenn Kotlin nicht mehr wartet (Seite verlassen): sonst bliebe der Port belegt.
struct AbortOnDrop<T>(tokio::task::JoinHandle<T>);

impl<T> Drop for AbortOnDrop<T> {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Auf den Login vom Desktop warten (keine Anfrage an YouTube).
#[uniffi::export]
pub async fn pair_receive(secret: String) -> Result<Account, TubeError> {
    let mut task = AbortOnDrop(qu_core::RT.spawn(tubequ_core::pair::receive(secret)));
    let login = (&mut task.0).await.map_err(|e| e.to_string())??;
    Ok(account_of(login))
}

/// Video in deinen YouTube-Verlauf (nur angemeldet; 1 Anfrage über `throttle`, wie am Desktop).
#[uniffi::export]
pub async fn mark_watched(id: String) {
    let _ = qu_core::bg(youtube::mark_watched(id)).await;
}

/// Geschaute Position an YouTube melden (beim Verlassen; Ping, zählt nicht zum Limit, wie am Desktop).
#[uniffi::export]
pub async fn report_position(id: String, pos: f64) {
    let _ = qu_core::bg(youtube::report_position(id, pos)).await;
}

/// Seiten-Cache leeren (vor einem Neuladen, wie am Desktop): sonst kämen 10 min lang dieselben Ergebnisse.
#[uniffi::export]
pub fn forget_cache() {
    youtube::forget_cache();
}

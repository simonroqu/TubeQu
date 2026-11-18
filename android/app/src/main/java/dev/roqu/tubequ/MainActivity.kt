// TubeQu für Android: Seiten der App in Compose; Bausteine (Bubble, Suche, Raster, Player, Theme) aus qu-core
// (`dev.roqu.qu`, android/src in qu-core), Daten aus Rust (tubequ-core über tubequ-ffi, `uniffi.tubequ_ffi`).
// Aussehen und Bedienung wie die Desktop-App; alles per Fernbedienung erreichbar (Android TV).
// Anonym (kein Browser-Login auf Android). Abspiel-Adressen über NewPipeExtractor (rustypipe kann nicht abspielen,
// yt-dlp gibt es hier nicht) – jede Extraktion vorher durch TubeQus Limit (`throttle`), wie alle YouTube-Anfragen.
package dev.roqu.tubequ

import android.os.Bundle
import androidx.activity.ComponentActivity
import androidx.activity.compose.setContent
import androidx.activity.enableEdgeToEdge
import androidx.compose.runtime.*
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.unit.dp
import androidx.media3.common.MimeTypes
import dev.roqu.qu.*
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import okhttp3.OkHttpClient
import okhttp3.RequestBody.Companion.toRequestBody
import org.schabi.newpipe.extractor.MediaFormat
import org.schabi.newpipe.extractor.NewPipe
import org.schabi.newpipe.extractor.ServiceList
import org.schabi.newpipe.extractor.downloader.Downloader
import org.schabi.newpipe.extractor.downloader.Request
import org.schabi.newpipe.extractor.downloader.Response
import org.schabi.newpipe.extractor.exceptions.ReCaptchaException
import org.schabi.newpipe.extractor.stream.StreamInfo
import org.schabi.newpipe.extractor.stream.StreamType
import uniffi.tubequ_ffi.*
import uniffi.tubequ_ffi.Video as Clip
import dev.roqu.qu.Video

/** Höchste Bildhöhe (wie `default_quality` in qu.toml). */
const val QUALITY = 1080

/** „Continue watching“ nach so vielen Reihen, höchstens so viele Videos (wie am Desktop). */
const val ROWS_BEFORE_SHELF = 4
const val SHELF_MAX = 3u

/** Karten mindestens so breit (16:9-Vorschaubild). */
val CARD = 260.dp

class MainActivity : ComponentActivity() {
    override fun onCreate(savedInstanceState: Bundle?) {
        super.onCreate(savedInstanceState)
        init(filesDir.path, cacheDir.path)
        NewPipe.init(Http)
        enableEdgeToEdge()
        val link = intent?.dataString
        setContent { QuTheme { App(link) } }
    }
}

/** HTTP für NewPipeExtractor (OkHttp). Ein „Bitte bestätigen“ von YouTube wird als Fehler gemeldet, nie umgangen. */
object Http : Downloader() {
    private val client = OkHttpClient()

    override fun execute(request: Request): Response {
        val body = request.dataToSend()?.toRequestBody()
        val b = okhttp3.Request.Builder().url(request.url()).method(request.httpMethod(), body)
        request.headers().forEach { (name, values) -> b.removeHeader(name); values.forEach { b.addHeader(name, it) } }
        client.newCall(b.build()).execute().use { r ->
            if (r.code == 429) throw ReCaptchaException("YouTube asks to confirm you're not a bot", request.url())
            return Response(r.code, r.message, r.headers.toMultimap(), r.body.string(), r.request.url.toString())
        }
    }
}

sealed interface Screen {
    data object Home : Screen
    data class Results(val query: String) : Screen
    data object Later : Screen
    data object History : Screen
    data class Watch(val video: Clip) : Screen
}

fun message(e: Exception) = when (e) {
    is TubeException.Failed -> e.msg
    is ReCaptchaException -> "YouTube asks to confirm you're not a bot. Try again later."
    else -> e.message ?: "Something went wrong"
}

/** Leeres Video aus einer ID (geöffneter Link). */
fun bare(id: String) = Clip(id, "", "", null, null, false, null, null, null)

@Composable
fun App(link: String?) {
    val nav = remember { Nav<Screen>(Screen.Home) }
    LaunchedEffect(link) {
        val id = link?.let { videoId(it) } ?: return@LaunchedEffect
        nav.open(Screen.Watch(bare(id)))
    }
    QuApp(
        nav,
        placeholder = "Search YouTube",
        searches = ::searches,
        onSearch = { addSearch(it); nav.open(Screen.Results(it)) },
        bubble = { it !is Screen.Watch },
        // Wie die Burger-Spalte am Desktop (`[nav] menu`: later, history); das Konto gibt es auf Android nicht.
        buttons = {
            IconButton(QuIcons.later, "Watch later") { nav.open(Screen.Later) }
            IconButton(QuIcons.history, "History") { nav.open(Screen.History) }
        },
    ) { page ->
        val play = { v: Clip -> nav.open(Screen.Watch(v)) }
        when (page) {
            Screen.Home -> HomePage(play)
            is Screen.Results -> FeedPage(page.query, { search(page.query) }, play)
            Screen.Later -> ListPage("Watch later", later(), "Nothing saved yet", play)
            Screen.History -> ListPage("History", history(), "Nothing watched yet", play)
            is Screen.Watch -> WatchPage(page.video)
        }
    }
}

@Composable
fun VideoCard(c: Card, play: (Clip) -> Unit) =
    PosterCard(c.video.title, c.thumbnail, ratio = 16f / 9f, label = c.meta, fraction = c.fraction, badge = c.badge) { play(c.video) }

/** Spalten wie das Raster (für „Continue watching“ nach 4 Reihen). */
@Composable
fun columns() = maxOf(1, (LocalConfiguration.current.screenWidthDp - 16) / (CARD.value.toInt() + 8))

/**
 * Raster mit Nachladen am Ende (eine Seite pro Erreichen des Endes, wie am Desktop). `first` = erste Seite;
 * `shelf`: „Continue watching“ nach `ROWS_BEFORE_SHELF` Reihen (nur Startseite).
 */
@Composable
fun Feed(title: String?, first: Page?, error: String?, shelf: List<Card>, play: (Clip) -> Unit) {
    var cards by remember(first) { mutableStateOf(first?.cards.orEmpty()) }
    var next by remember(first) { mutableStateOf(first?.next) }
    var moreError by remember { mutableStateOf<String?>(null) }
    var loading by remember { mutableStateOf(false) }
    val state = androidx.compose.foundation.lazy.grid.rememberLazyGridState()
    val end by remember { derivedStateOf { state.nearEnd() } }
    LaunchedEffect(end, next) {
        val n = next ?: return@LaunchedEffect
        if (!end || loading) return@LaunchedEffect
        loading = true
        try { more(n).let { cards = cards + it.cards; next = it.next } } catch (e: Exception) { moreError = message(e); next = null }
        loading = false
    }
    val cols = columns()
    val split = minOf(cards.size, cols * ROWS_BEFORE_SHELF)
    Grid(minWidth = CARD, state = state) {
        title?.let { header(it, 0.dp) }
        error?.let { note(it, Red) }
        if (first == null && error == null) ghosts(8, 16f / 9f)
        val top = cards.take(split)
        items(top.size, key = { "a" + top[it].video.id }) { VideoCard(top[it], play) }
        if (shelf.isNotEmpty() && cards.isNotEmpty()) {
            header("Continue watching")
            items(shelf.size, key = { "c" + shelf[it].video.id }) { VideoCard(shelf[it], play) }
        }
        val rest = cards.drop(split)
        items(rest.size, key = { "b" + rest[it].video.id }) { VideoCard(rest[it], play) }
        moreError?.let { note(it, Red) }
        if (loading) ghosts(cols, 16f / 9f)
    }
}

@Composable
fun HomePage(play: (Clip) -> Unit) {
    // Sofort die letzte Startseite; neu holen erst, wenn sie älter als 10 min ist (spart Anfragen, wie am Desktop).
    var page by remember { mutableStateOf(savedHome()) }
    var error by remember { mutableStateOf<String?>(null) }
    val shelf = remember { continueWatching(SHELF_MAX) }
    LaunchedEffect(Unit) {
        if (page != null && savedHomeFresh()) return@LaunchedEffect
        try { page = home() } catch (e: Exception) { if (page == null) error = message(e) }
    }
    Feed(null, page, error, shelf, play)
}

@Composable
fun FeedPage(title: String, load: suspend () -> Page, play: (Clip) -> Unit) {
    var page by remember { mutableStateOf<Page?>(null) }
    var error by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(title) {
        try { page = load() } catch (e: Exception) { error = message(e) }
    }
    Feed(title, page, error, emptyList(), play)
}

@Composable
fun ListPage(title: String, list: List<Card>, empty: String, play: (Clip) -> Unit) {
    Grid(minWidth = CARD) {
        header(title, 0.dp)
        if (list.isEmpty()) note(empty)
        items(list.size, key = { list[it].video.id }) { VideoCard(list[it], play) }
    }
}

@Composable
fun WatchPage(video: Clip) {
    // Aus einem Link geöffnet: Titel/Kanal erst aus der Extraktion (für Fortschritt und „Continue watching“).
    var current by remember { mutableStateOf(video) }
    VideoPlayer(
        load = {
            throttle()
            val info = withContext(Dispatchers.IO) {
                StreamInfo.getInfo(ServiceList.YouTube, "https://www.youtube.com/watch?v=${video.id}")
            }
            if (current.title.isEmpty()) {
                current = current.copy(title = info.name, channel = info.uploaderName, channelId = info.uploaderUrl.substringAfterLast('/'))
            }
            val live = info.streamType == StreamType.LIVE_STREAM
            if (live && info.hlsUrl.isNotEmpty()) {
                Video(url = info.hlsUrl, mime = MimeTypes.APPLICATION_M3U8, live = true)
            } else {
                // Bild und Ton getrennt: H.264 bevorzugt (läuft fast überall in Hardware), Ton AAC (m4a).
                val codec = { c: String? -> c.orEmpty().let { if (it.startsWith("avc")) 2 else if (it.startsWith("vp9")) 1 else 0 } }
                val v = info.videoOnlyStreams.filter { it.isUrl && it.height in 1..QUALITY }
                    .maxWithOrNull(compareBy({ it.height }, { codec(it.codec) }))
                val a = info.audioStreams.filter { it.isUrl }
                    .maxWithOrNull(compareBy({ it.format == MediaFormat.M4A }, { it.averageBitrate }))
                if (v != null && a != null) {
                    Video(url = v.content, audio = a.content, start = resumeAt(video.id),
                        speed = speed(current.channelId.orEmpty()).toFloat())
                } else {
                    val muxed = info.videoStreams.filter { it.isUrl }.maxByOrNull { it.height }
                        ?: throw IllegalStateException("No playable stream")
                    Video(url = muxed.content, start = resumeAt(video.id))
                }
            }
        },
        message = ::message,
        save = { pos, dur -> if (current.title.isNotEmpty()) record(current, pos, dur) },
        skips = { segments(video.id).map { Skip(it.start, it.end, it.label) } },
        autoSkip = true,
    )
}

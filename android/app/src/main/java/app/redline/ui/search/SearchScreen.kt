package app.redline.ui.search

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.rounded.ArrowBack
import androidx.compose.material.icons.automirrored.rounded.QueueMusic
import androidx.compose.material.icons.rounded.Bookmarks
import androidx.compose.material.icons.rounded.ChevronRight
import androidx.compose.material.icons.rounded.Download
import androidx.compose.material.icons.rounded.ErrorOutline
import androidx.compose.material.icons.rounded.History
import androidx.compose.material.icons.rounded.LibraryMusic
import androidx.compose.material.icons.rounded.MusicNote
import androidx.compose.material.icons.rounded.PhoneAndroid
import androidx.compose.material.icons.rounded.Search
import androidx.compose.material.icons.rounded.Settings
import androidx.compose.material.icons.rounded.SwapHoriz
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.vector.ImageVector
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import app.redline.core.Platform
import app.redline.core.TrackRecord
import app.redline.state.MusicState
import app.redline.ui.Tab
import app.redline.ui.components.InputField
import app.redline.ui.components.PageHeader
import app.redline.ui.components.PagePadding
import app.redline.ui.playlists.PlaylistsModel
import app.redline.ui.saved.SavedModel
import app.redline.ui.theme.LocalAccent
import app.redline.ui.theme.RedlineColors

private sealed interface SearchAction {
    data class PlaySaved(val platform: Platform, val index: Int) : SearchAction
    data class OpenCollection(val platform: Platform, val id: ULong) : SearchAction
    data class OpenPlaylist(val id: ULong) : SearchAction
    data class PlayPlaylistTrack(val id: ULong, val index: Int) : SearchAction
    data class PlayLocal(val track: TrackRecord) : SearchAction
    data class Go(val tab: Tab) : SearchAction
}

private data class SearchEntry(
    val title: String,
    val meta: String,
    val icon: ImageVector,
    val action: SearchAction,
)

private data class PageEntry(val label: String, val icon: ImageVector, val tab: Tab)

private val PAGE_ENTRIES = listOf(
    PageEntry("Download", Icons.Rounded.Download, Tab.Download),
    PageEntry("Convert", Icons.Rounded.SwapHoriz, Tab.Convert),
    PageEntry("Library", Icons.Rounded.LibraryMusic, Tab.Library),
    PageEntry("Queue", Icons.AutoMirrored.Rounded.QueueMusic, Tab.Queue),
    PageEntry("History", Icons.Rounded.History, Tab.History),
    PageEntry("Settings", Icons.Rounded.Settings, Tab.Settings),
)

@Composable
fun SearchScreen(
    music: MusicState,
    saved: SavedModel,
    playlists: PlaylistsModel,
    local: List<TrackRecord>,
    onBack: () -> Unit,
    onOpenSaved: () -> Unit,
    onOpenPlaylist: () -> Unit,
    onGo: (Tab) -> Unit,
) {
    BackHandler(onBack = onBack)
    var query by rememberSaveable { mutableStateOf("") }
    val focus = remember { FocusRequester() }
    val index by produceState<List<SearchEntry>?>(null, music, local) {
        value = music.onMusicSlow { m ->
            val out = mutableListOf<SearchEntry>()
            for (p in m.savedOverview().platforms) {
                for (c in m.savedPlatform(p.platform).collections) {
                    out += SearchEntry(
                        c.name,
                        c.platform.name + " · Collection · " + c.songCount + " songs",
                        Icons.Rounded.Bookmarks,
                        SearchAction.OpenCollection(c.platform, c.id),
                    )
                }
                for ((i, item) in m.savedAllSongs(p.platform).withIndex()) {
                    val track = item.track
                    out += SearchEntry(
                        track.title,
                        withArtist(
                            p.label + " · Saved song" + (if (item.removed) " · removed" else ""),
                            track.artist,
                        ),
                        Icons.Rounded.MusicNote,
                        SearchAction.PlaySaved(p.platform, i),
                    )
                }
            }
            for (p in m.playlists()) {
                out += SearchEntry(
                    p.name,
                    "Playlist · " + p.trackCount + " songs",
                    Icons.AutoMirrored.Rounded.QueueMusic,
                    SearchAction.OpenPlaylist(p.id),
                )
                val full = m.playlist(p.id)
                if (full != null) {
                    for ((i, track) in full.tracks.withIndex()) {
                        out += SearchEntry(
                            track.title,
                            withArtist("Playlist · " + p.name, track.artist),
                            Icons.Rounded.MusicNote,
                            SearchAction.PlayPlaylistTrack(p.id, i),
                        )
                    }
                }
            }
            out + local.map { track ->
                SearchEntry(
                    track.title,
                    withArtist("Local file", track.artist),
                    Icons.Rounded.PhoneAndroid,
                    SearchAction.PlayLocal(track),
                )
            }
        }
    }
    LaunchedEffect(Unit) { focus.requestFocus() }

    val q = query.trim()
    val results = remember(index, q) {
        if (q.isEmpty()) {
            PAGE_ENTRIES.map { SearchEntry(it.label, "Go to page", it.icon, SearchAction.Go(it.tab)) }
        } else {
            val pages = PAGE_ENTRIES.map { SearchEntry(it.label, "Go to page", it.icon, SearchAction.Go(it.tab)) }
            val all = pages + (index ?: emptyList())
            all.filter { rank(it.title, it.meta, q) != null }
                .sortedWith(compareBy({ rank(it.title, it.meta, q) ?: 9 }, { it.title.lowercase() }))
                .take(40)
        }
    }
    val loading = q.isNotEmpty() && index == null

    LazyColumn(Modifier.fillMaxSize().background(RedlineColors.Bg), contentPadding = PaddingValues(bottom = 24.dp)) {
        item {
            Row(
                Modifier
                    .fillMaxWidth()
                    .background(LocalAccent.current.headerTint)
                    .padding(start = 4.dp, top = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                IconButton(onClick = onBack) {
                    Icon(Icons.AutoMirrored.Rounded.ArrowBack, contentDescription = "Back")
                }
                Text("Back", style = MaterialTheme.typography.bodyMedium, color = RedlineColors.Muted)
            }
            PageHeader(
                "Search",
                "Find anything",
                "Saved music, playlists, local files and pages.",
            )
        }
        item {
            Column(Modifier.padding(horizontal = PagePadding), verticalArrangement = Arrangement.spacedBy(8.dp)) {
                Spacer(Modifier.height(8.dp))
                InputField(
                    value = query,
                    onChange = { query = it },
                    placeholder = "Search titles, artists, playlists…",
                    leading = Icons.Rounded.Search,
                    shapeRound = true,
                    keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                    modifier = Modifier.focusRequester(focus),
                )
                if (loading) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        CircularProgressIndicator(Modifier.size(16.dp), color = LocalAccent.current.base, strokeWidth = 2.dp)
                        Spacer(Modifier.width(8.dp))
                        Text("Reading your library…", color = RedlineColors.Muted, style = MaterialTheme.typography.bodySmall)
                    }
                } else if (q.isNotEmpty()) {
                    Text(
                        results.size.toString() + " result" + (if (results.size == 1) "" else "s"),
                        color = RedlineColors.Muted,
                        style = MaterialTheme.typography.bodySmall,
                    )
                }
            }
        }
        if (!loading && results.isEmpty()) {
            item {
                Row(
                    Modifier.padding(horizontal = PagePadding, vertical = 24.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Icon(Icons.Rounded.ErrorOutline, null, tint = RedlineColors.Muted)
                    Spacer(Modifier.width(8.dp))
                    Text("No matches.", color = RedlineColors.Muted)
                }
            }
        }
        items(results) { entry ->
            Row(
                Modifier
                    .fillMaxWidth()
                    .clickable {
                        when (val action = entry.action) {
                            is SearchAction.PlaySaved -> {
                                saved.playAll(action.platform, action.index)
                                onBack()
                            }
                            is SearchAction.OpenCollection -> {
                                saved.openPlatform(action.platform)
                                saved.openCollection(action.id)
                                onOpenSaved()
                                onBack()
                            }
                            is SearchAction.OpenPlaylist -> {
                                playlists.open(action.id)
                                onOpenPlaylist()
                                onBack()
                            }
                            is SearchAction.PlayPlaylistTrack -> {
                                playlists.play(action.id, action.index)
                                onBack()
                            }
                            is SearchAction.PlayLocal -> {
                                music.act { it.playTracks(listOf(action.track), 0u) }
                                onBack()
                            }
                            is SearchAction.Go -> {
                                onGo(action.tab)
                                onBack()
                            }
                        }
                    }
                    .padding(horizontal = PagePadding, vertical = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Icon(entry.icon, contentDescription = null, tint = LocalAccent.current.base, modifier = Modifier.size(22.dp))
                Spacer(Modifier.width(12.dp))
                Column(Modifier.weight(1f)) {
                    Text(
                        entry.title,
                        style = MaterialTheme.typography.bodyLarge,
                        color = RedlineColors.Text,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Text(
                        entry.meta,
                        style = MaterialTheme.typography.bodySmall,
                        color = RedlineColors.Muted,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                Icon(Icons.Rounded.ChevronRight, contentDescription = null, tint = RedlineColors.Subtle, modifier = Modifier.size(18.dp))
            }
        }
    }
}

private fun withArtist(meta: String, artist: String?): String =
    if (artist.isNullOrBlank()) meta else meta + " · " + artist

private fun rank(title: String, meta: String, q: String): Int? {
    val t = title.lowercase()
    val m = meta.lowercase()
    val query = q.lowercase()
    return when {
        t.startsWith(query) -> 0
        t.contains(query) -> 1
        m.contains(query) -> 2
        else -> null
    }
}

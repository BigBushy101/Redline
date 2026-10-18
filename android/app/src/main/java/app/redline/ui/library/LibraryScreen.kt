package app.redline.ui.library

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.Composable
import androidx.compose.ui.Modifier
import androidx.compose.ui.unit.dp
import app.redline.core.HistoryRecord
import app.redline.core.PlaylistSummaryRecord
import app.redline.state.MusicState
import app.redline.ui.local.LocalModel
import app.redline.ui.local.LocalScreen
import app.redline.ui.components.PagePadding
import app.redline.ui.components.SegmentedSwitch
import app.redline.ui.playlists.PlaylistsModel
import app.redline.ui.playlists.PlaylistsScreen
import app.redline.ui.saved.SavedModel
import app.redline.ui.saved.SavedScreen
import app.redline.ui.theme.LocalAccent

/** The Library's sections, the desktop sidebar's Saved, Playlists and Local. */
enum class LibrarySection(val label: String) { Saved("Saved"), Playlists("Playlists"), Local("Local") }

/**
 * The bottom bar's Library: the desktop sidebar's Saved, Playlists and Local, one tab between
 * them on a phone. The switch shows at the top of each section's first page; deeper pages have
 * Back.
 */
@Composable
fun LibraryScreen(
    section: LibrarySection,
    onSection: (LibrarySection) -> Unit,
    saved: SavedModel,
    playlistsModel: PlaylistsModel,
    local: LocalModel,
    music: MusicState,
    playlists: List<PlaylistSummaryRecord>,
    history: List<HistoryRecord>,
) {
    val atTop = when (section) {
        LibrarySection.Saved -> saved.platform == null
        LibrarySection.Playlists -> playlistsModel.openId == null
        LibrarySection.Local -> true
    }
    Column(Modifier.fillMaxSize()) {
        if (atTop) {
            // In the header's tint, so the page header's gradient carries on below it.
            Box(
                Modifier
                    .fillMaxWidth()
                    .background(LocalAccent.current.headerTint)
                    .padding(start = PagePadding, end = PagePadding, top = 12.dp),
            ) {
                SegmentedSwitch(section, LibrarySection.entries.map { it to it.label }, onSection)
            }
        }
        Box(Modifier.weight(1f)) {
            when (section) {
                LibrarySection.Saved -> SavedScreen(saved, music, history, playlists)
                LibrarySection.Playlists -> PlaylistsScreen(playlistsModel, music, playlists, history)
                LibrarySection.Local -> LocalScreen(local, music, playlists)
            }
        }
    }
}

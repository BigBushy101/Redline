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
import app.redline.ui.components.PagePadding
import app.redline.ui.components.SegmentedSwitch
import app.redline.ui.playlists.PlaylistsModel
import app.redline.ui.playlists.PlaylistsScreen
import app.redline.ui.saved.SavedModel
import app.redline.ui.saved.SavedScreen
import app.redline.ui.theme.LocalAccent

/**
 * The bottom bar's Library: the desktop sidebar's Saved and Playlists, one tab between them on
 * a phone. The switch shows at the top of each section's first page; deeper pages have Back.
 */
@Composable
fun LibraryScreen(
    showSaved: Boolean,
    onShowSaved: (Boolean) -> Unit,
    saved: SavedModel,
    playlistsModel: PlaylistsModel,
    music: MusicState,
    playlists: List<PlaylistSummaryRecord>,
    history: List<HistoryRecord>,
) {
    val atTop = if (showSaved) saved.platform == null else playlistsModel.openId == null
    Column(Modifier.fillMaxSize()) {
        if (atTop) {
            // In the header's tint, so the page header's gradient carries on below it.
            Box(
                Modifier
                    .fillMaxWidth()
                    .background(LocalAccent.current.headerTint)
                    .padding(start = PagePadding, end = PagePadding, top = 12.dp),
            ) {
                SegmentedSwitch(showSaved, listOf(true to "Saved", false to "Playlists"), onShowSaved)
            }
        }
        Box(Modifier.weight(1f)) {
            if (showSaved) {
                SavedScreen(saved, music, history, playlists)
            } else {
                PlaylistsScreen(playlistsModel, music, playlists, history)
            }
        }
    }
}

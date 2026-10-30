# Redline's yt-dlp worker: started on the link `redline-worker:`, it reads links from stdin,
# one per line, and answers each with one JSON line, {"ok": info} or {"error": message},
# extracted with the options yt-dlp was started with. Exits when stdin closes.
#
# It also keeps YouTube's player, once made ready for the JS challenge solver, in memory for
# the next link and on disk for the next worker: solving from it saves seconds per song on
# QuickJS (Android's), and persisting it saves the same once more after a launch. The files
# are pruned to a bounded size and age, which yt-dlp's own cache leaves off for.
import json
import os
import sys
import time

from yt_dlp.extractor.common import InfoExtractor
from yt_dlp.version import __version__

# Preprocessed players kept on disk: how much and how old.
PLAYER_CACHE_MAX_BYTES = 32 * 1024 * 1024
PLAYER_CACHE_MAX_AGE = 14 * 24 * 60 * 60

# What the player needs from a look-up; everything else in yt-dlp's info dict is formats,
# subtitles, thumbnails and comments, which made the reply 88 KB to 650 KB.
_STREAM_KEYS = (
    'url', 'http_headers', 'title', 'track', 'artist', 'uploader', 'channel', 'album',
    'duration', 'thumbnail',
)

# One downloader per worker, so one patch: the players made ready in this process, the cache
# section, and whether the look-up under way read one from disk.
_patched = False
_section = None
_players = {}
_used_disk = False
_pruned = False


def _player_folder(ydl):
    return os.path.join(ydl.cache._get_root_dir(), _section)


def _prune_players(ydl):
    """Keeps the preprocessed players within the age and size caps above."""
    global _pruned
    if _pruned:
        return
    _pruned = True
    try:
        folder = _player_folder(ydl)
        files = []
        for name in os.listdir(folder):
            if not name.startswith('player'):
                continue
            path = os.path.join(folder, name)
            stat = os.stat(path)
            files.append((stat.st_mtime, stat.st_size, path))
        total = sum(size for _, size, _ in files)
        now = time.time()
        for mtime, size, path in sorted(files):
            if now - mtime > PLAYER_CACHE_MAX_AGE or total > PLAYER_CACHE_MAX_BYTES:
                os.remove(path)
                total -= size
    except (OSError, TypeError, AttributeError):
        pass


def _clear_players(ydl):
    """Drops the preprocessed players, memory and disk, so a bad one is never used twice."""
    global _players
    _players = {}
    try:
        folder = _player_folder(ydl)
        for name in os.listdir(folder):
            if name.startswith('player'):
                os.remove(os.path.join(folder, name))
    except (OSError, TypeError, AttributeError):
        pass


def keep_players_in_memory(ydl, keep=2):
    """Turns on yt-dlp's cache of YouTube players made ready for the JS challenge solver: in
    memory for this worker, on disk for the next. yt-dlp leaves it off because the files are
    big and never cleaned up; here they are pruned. Safe to call for every link; the cache is
    patched once."""
    global _patched, _section, _used_disk
    if _patched:
        return
    try:
        from yt_dlp.extractor.youtube.jsc._builtin.quickjs import QuickJSJCP
    except ImportError:
        _patched = True
        return
    _section = QuickJSJCP._CACHE_SECTION
    cache = ydl.cache
    load, store = cache.load, cache.store

    def is_player(sec, key):
        return sec == _section and isinstance(key, str) and key.startswith('player:')

    def memory_load(sec, key, *args, **kwargs):
        global _used_disk
        if not is_player(sec, key):
            return load(sec, key, *args, **kwargs)
        data = _players.pop(key, None)
        if data is None:
            # A player from another yt-dlp version may not fit this one's solver; the cache
            # keeps the version with it, and _clear_players gives up on it if the solve fails.
            kwargs.setdefault('min_ver', __version__)
            try:
                data = load(sec, key, *args, **kwargs)
            except TypeError:
                # A cache without the version check is still usable; a wrong player is
                # cleared and solved again if it fails.
                kwargs.pop('min_ver', None)
                data = load(sec, key, *args, **kwargs)
            _used_disk = data is not None
        if data is None:
            return kwargs.get('default')
        _players[key] = data
        while len(_players) > keep:
            del _players[next(iter(_players))]
        return data

    def memory_store(sec, key, data, *args, **kwargs):
        if not is_player(sec, key):
            return store(sec, key, data, *args, **kwargs)
        _players.pop(key, None)
        if data is not None:
            _players[key] = data
            while len(_players) > keep:
                del _players[next(iter(_players))]
            store(sec, key, data, *args, **kwargs)

    cache.load, cache.store = memory_load, memory_store
    QuickJSJCP._ENABLE_PREPROCESSED_PLAYER_CACHE = True
    _patched = True
    _prune_players(ydl)


def stream_info(info):
    """What playback needs from a look-up, without the rest of yt-dlp's info dict."""
    out = {key: info[key] for key in _STREAM_KEYS if info.get(key) is not None}
    parts = info.get('requested_formats')
    if parts:
        trimmed = []
        for part in parts:
            if not part.get('url'):
                continue
            kept = {'url': part['url']}
            headers = part.get('http_headers') or info.get('http_headers')
            if headers:
                kept['http_headers'] = headers
            trimmed.append(kept)
        if trimmed:
            out['requested_formats'] = trimmed
    if 'thumbnail' not in out:
        thumbnails = info.get('thumbnails')
        if thumbnails and thumbnails[-1].get('url'):
            out['thumbnail'] = thumbnails[-1]['url']
    return out


class RedlineWorkerIE(InfoExtractor):
    IE_NAME = 'redline:worker'
    _VALID_URL = r'redline-worker:'

    def _real_extract(self, url):
        global _used_disk
        ydl = self._downloader
        # The command line's default skips failed extractions silently; the app wants why.
        ydl.params['ignoreerrors'] = False
        keep_players_in_memory(ydl)
        out = sys.stdout

        def reply(obj):
            out.write(json.dumps(obj) + '\n')
            out.flush()

        reply({'ready': True})
        for line in sys.stdin:
            link = line.strip()
            if not link:
                continue
            _used_disk = False
            info = None
            error = None
            try:
                info = ydl.extract_info(link, download=False)
            except Exception as e:
                error = e
                if _used_disk:
                    # The for-the-solver player kept on disk was unusable; solve it fresh
                    # rather than fail the song.
                    _clear_players(ydl)
                    _used_disk = False
                    error = None
                    try:
                        info = ydl.extract_info(link, download=False)
                    except Exception as again:
                        error = again
            if error is not None:
                reply({'error': str(error)})
            elif info is None:
                reply({'error': 'yt-dlp returned nothing for this link'})
            else:
                reply({'ok': stream_info(info)})
        out.flush()
        os._exit(0)

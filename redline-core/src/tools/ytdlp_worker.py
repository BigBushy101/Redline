# Redline's yt-dlp worker: started on the link `redline-worker:`, it reads links from stdin,
# one per line, and answers each with one JSON line, {"ok": info} or {"error": message},
# extracted with the options yt-dlp was started with. Exits when stdin closes.
import json
import os
import sys

from yt_dlp.extractor.common import InfoExtractor


def keep_players_in_memory(ydl, keep=2):
    """Turns on yt-dlp's cache of YouTube players made ready for the JS challenge solver, in
    memory, for QuickJS (Android's): solving from it saves seconds per song there, while Deno
    is as quick either way. yt-dlp leaves it off because the files are big and never cleaned
    up; here it lasts only as long as the worker, and holds the last `keep` players."""
    try:
        from yt_dlp.extractor.youtube.jsc._builtin.quickjs import QuickJSJCP
    except ImportError:
        return
    section = QuickJSJCP._CACHE_SECTION
    cache = ydl.cache
    load, store = cache.load, cache.store
    players = {}

    def is_player(sec, key):
        return sec == section and isinstance(key, str) and key.startswith('player:')

    def memory_load(sec, key, *args, **kwargs):
        if not is_player(sec, key):
            return load(sec, key, *args, **kwargs)
        data = players.pop(key, None)
        if data is None:
            return kwargs.get('default')
        players[key] = data
        return data

    def memory_store(sec, key, data, *args, **kwargs):
        if not is_player(sec, key):
            return store(sec, key, data, *args, **kwargs)
        players.pop(key, None)
        if data is not None:
            players[key] = data
            while len(players) > keep:
                del players[next(iter(players))]

    cache.load, cache.store = memory_load, memory_store
    QuickJSJCP._ENABLE_PREPROCESSED_PLAYER_CACHE = True


class RedlineWorkerIE(InfoExtractor):
    IE_NAME = 'redline:worker'
    _VALID_URL = r'redline-worker:'

    def _real_extract(self, url):
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
            try:
                info = ydl.extract_info(link, download=False)
                if info is None:
                    reply({'error': 'yt-dlp returned nothing for this link'})
                else:
                    reply({'ok': ydl.sanitize_info(info)})
            except Exception as e:
                reply({'error': str(e)})
        out.flush()
        os._exit(0)

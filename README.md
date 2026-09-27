# muse-player

A fast GTK4/libadwaita music player for Navidrome (Subsonic API),
with first-class AudioMuse-AI integration. Written in Rust.

## Goals

- **Instant start:** library list visible in under 0.5 s after launch
  (served from a local SQLite cache, synced with the server in the background).
- **Smooth scrolling** through the whole library (10k+ tracks):
  virtualized list views, cover art cached on disk in the size it is shown.
- **UI never waits for the network:** all requests run off the main thread.
- **Gapless playback:** the next track is prefetched before the current one ends.
- **Stand-out features:** AudioMuse-AI (similar songs, endless radio,
  search by description, song paths, music map), bit-perfect output
  via PipeWire, synced lyrics.

## Roadmap

1. [x] Subsonic client: token auth, ping, album list (CLI)
2. [ ] Playback: GStreamer, stream from Navidrome, simple queue (CLI)
3. [ ] GUI skeleton: gtk4-rs + libadwaita, login screen, album list
4. [ ] Player in GUI: now-playing bar, queue, MPRIS
5. [ ] Local library cache (SQLite) and cover art cache
6. [ ] AudioMuse-AI: check available API, similar songs, endless radio
7. [ ] Search by description, song paths, music map, bit-perfect, lyrics

## Layout

- `subsonic/` - Subsonic API client library
- `cli/` - command-line test tool for the engine

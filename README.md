# muse-player

A fast GTK4/libadwaita music player for Navidrome (Subsonic API),
with first-class AudioMuse-AI integration. Written in Rust.

## Goals

- **Instant start:** library visible in under 0.5 s after launch.
- **Instant search:** local full-text search over the whole library.
- **Smooth scrolling** through 10k+ tracks.
- **UI never waits for the network.**
- **Gapless playback** with the next track prefetched.
- **Stand-out features:** AudioMuse-AI (similar songs, endless radio,
  search by description, song paths, music map), bit-perfect output,
  synced lyrics.

## Architecture

- `subsonic/` - async Subsonic API client (tokio + reqwest), typed errors.
- `app/` - GTK4/libadwaita GUI. The UI reads from a local SQLite database;
  a background sync keeps it up to date with the server.
- `cli/` - command-line test tool for the engine.
- Built and run as a Flatpak on the GNOME 51 runtime.

## Roadmap

1. [x] Subsonic client: token auth, ping, album list (CLI)
2. [x] Playback in CLI: GStreamer, raw FLAC stream from Navidrome
3. [ ] GUI foundation
   - [x] Window on GNOME 51 (GTK 4.24, libadwaita 1.10) via Flatpak
   - [x] Welcome carousel and login form
   - [ ] Async client with typed errors
   - [ ] Real login (ping in background, clear error messages)
   - [ ] Credentials in the system keyring, settings, first-run flag
   - [ ] gettext, Blueprint UI files, Meson build
4. [ ] Player in GUI: now-playing bar, queue, MPRIS, background playback
5. [ ] Local library: SQLite cache, background sync, FTS5 search, cover cache
6. [ ] Scrobbling, play queue sync (savePlayQueue), ReplayGain, audio prefetch
7. [ ] AudioMuse-AI: similar songs, endless radio, search by description
8. [ ] Synced lyrics, bit-perfect output, offline downloads, music map

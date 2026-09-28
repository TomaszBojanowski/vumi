# muse-player

A fast GTK4/libadwaita music player for Navidrome (Subsonic API),
with first-class AudioMuse-AI integration. Written in Rust.

## Goals

- **Instant start:** library visible in under 0.5 s after launch.
- **Instant search:** local full-text search over the whole library.
- **Smooth scrolling** through 10k+ tracks.
- **UI never waits for the network.**
- **Gapless playback** with the next track prefetched.

## Signature features

1. **Music discovery with AudioMuse-AI:** search by description
   ("calm jazz for a rainy evening"), song paths that move smoothly from
   one track to another, an interactive map of the library grouped by
   sound, and endless radio when the queue runs out.
2. **Signal path:** shows the whole audio chain from the file on the server
   to the sound card, and whether playback is bit-perfect.
3. **GNOME Shell search provider:** find and play albums straight from
   the Activities overview.

Later: year in music, forgotten albums, time-of-day mixes, command palette
(Ctrl+K), adaptive layout for narrow windows and phones.

## Architecture

- `subsonic/` - async Subsonic API client (tokio + reqwest), typed errors.
- `app/` - GTK4/libadwaita GUI. The UI reads from a local SQLite database;
  a background sync keeps it up to date with the server.
- `cli/` - command-line test tool for the engine.
- Built and run as a Flatpak on the GNOME 51 runtime.

## Roadmap

1. [x] Subsonic client, CLI playback
2. [x] GUI foundation: GNOME 51, login, keyring, Meson, gettext, Blueprint
3. [x] Library: SQLite cache, instant start, covers, album and artist pages
4. [x] Player: GStreamer, sidebar card, compact bar on narrow windows
5. [ ] Shared data instead of copies (Arc client, Rc lists)
6. [ ] Remember the last playback (song, album, position)
7. [ ] Start page: continue listening, recently added, similar to what played last (AudioMuse)
8. [ ] AudioMuse-AI via Navidrome: similar songs, endless radio, album radio
8. [ ] Search in the sidebar (FTS5), with a description mode (AudioMuse CLAP)
9. [ ] Album page: full-width cover-coloured header, row states, track menu
10. [ ] Queue in a right-hand panel, later synced lyrics as a second tab
11. [ ] Gapless playback, MPRIS, playback after the window is closed
12. [ ] AudioMuse-AI direct: song paths, library map; signal path panel
13. [ ] HTTPS with trust for self-signed certificates; server settings, logout

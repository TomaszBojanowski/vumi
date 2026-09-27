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

1. [x] Subsonic client: token auth, ping, album list (CLI)
2. [x] Playback in CLI: GStreamer, raw FLAC stream from Navidrome
3. [x] GUI foundation
   - [x] Window on GNOME 51 (GTK 4.24, libadwaita 1.10) via Flatpak
   - [x] Welcome carousel and login form
   - [x] Async client with typed errors
   - [x] Real login (ping in background, clear error messages)
   - [x] Window size remembered (GSettings)
   - [x] Credentials in the system keyring, server and user in settings,
         welcome shown only on first run
   - [x] gettext, Blueprint UI files, Meson build
4. [ ] Player in GUI: now-playing bar, queue, MPRIS, background playback
5. [ ] Local library: SQLite cache, background sync, FTS5 search, cover cache
6. [ ] Scrobbling, play queue sync, ReplayGain, audio prefetch,
       HTTPS with trust for self-signed certificates
7. [ ] Signature 1: AudioMuse-AI discovery
8. [ ] Signature 2: signal path and bit-perfect output
9. [ ] Signature 3: GNOME Shell search provider
10. [ ] Synced lyrics, offline downloads, the "later" ideas

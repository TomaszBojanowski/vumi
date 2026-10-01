# Vumi

A fast GTK4/libadwaita music player for Navidrome and other Subsonic servers,
with first-class AudioMuse-AI integration. Written in Rust.

> Early development: used every day with the author's own library, but expect
> rough edges.

## Features

- **Instant start:** the library comes from a local SQLite cache, so it is on
  screen in a fraction of a second; the UI never waits for the network.
- **Local full-text search** (FTS5) over the whole library.
- **Albums, artists, playlists and favourites**, with composers on album pages.
- **Start page:** continue listening, a random album, recently added, most
  played, and songs similar to what played last.
- **Gapless playback**, repeat (queue or track), and optional volume
  normalization from ReplayGain tags. It is off by default, so the signal
  stays bit-perfect.
- **AudioMuse-AI through Navidrome:** similar songs, endless radio, album radio.
- **Keeps playing after the window is closed**, with MPRIS controls in GNOME Shell.
- **Scrobbling** to the server, and a live "now playing" with pause and position
  on servers with the OpenSubsonic `playbackReport` extension (Navidrome 0.62
  and later). Both can be turned off.
- **Light and dark style**, and colours taken from the album cover (optional).
- **Keyboard shortcuts:** Ctrl+? lists them all.

## Planned

1. **Signal path:** the whole audio chain from the file on the server to the
   sound card, and whether playback is bit-perfect.
2. **GNOME Shell search provider:** find and play albums straight from the
   Activities overview.
3. **AudioMuse-AI direct:** search by description ("calm jazz for a rainy
   evening"), song paths that move smoothly from one track to another, and an
   interactive map of the library grouped by sound.
4. Synced lyrics in the queue panel.
5. Self-signed certificates for home servers.

Later: year in music, forgotten albums, time-of-day mixes, a command palette
(Ctrl+K), and an adaptive layout for narrow windows and phones.

## Building

Vumi is built and run as a Flatpak:

```sh
flatpak-builder --user --install --install-deps-from=flathub --force-clean \
    flatpak-build io.github.tomaszbojanowski.Vumi.yml
flatpak run io.github.tomaszbojanowski.Vumi
```

## Architecture

- `subsonic/`: async Subsonic API client (tokio + reqwest), typed errors.
- `app/`: GTK4/libadwaita GUI. The UI reads from a local SQLite database;
  a background sync keeps it up to date with the server.
- `cli/`: command-line test tool for the engine.

## Translations

The interface is translated with gettext; the files are in `po/`.
Polish is complete.

## License

Vumi is free software, released under the GNU General Public License,
version 3 or later. See [LICENSE](LICENSE).

# Vumi

A fast GTK4/libadwaita music player for Navidrome and other Subsonic servers,
with first-class AudioMuse-AI integration. Written in Rust.

Website: [vumi.bojanowski.me](https://vumi.bojanowski.me/)

> Early development: used every day with the author's own library, but expect
> rough edges.

![The Start page with recently played albums and the Tuner](data/screenshots/1-start.png)

## Features

- **Instant start:** the library comes from a local SQLite cache, so it is on
  screen in a fraction of a second; the UI never waits for the network.
- **Local full-text search** (FTS5) over the whole library.
- **Albums, artists, playlists and favorites**, with composers on album pages.
- **Start page:** the albums played lately, each resumed where it stopped;
  the Tuner, a scale of the years in the library with Play This Year;
  recently added albums; and albums with songs similar to what played last.
- **Gapless playback**, repeat (queue or track), and optional volume
  normalization from ReplayGain tags (off by default).
- **AudioMuse-AI through Navidrome:** similar songs, endless radio, album radio.
- **Keeps playing after the window is closed**, with MPRIS controls in GNOME Shell.
- **Scrobbling** to the server, and a live "now playing" with pause and position
  on servers with the OpenSubsonic `playbackReport` extension (Navidrome 0.62
  and later). Both can be turned off.
- **Light and dark style**, and colors taken from the album cover (optional).
- **Narrow and low windows:** down to 366 px wide the sidebar folds away and
  a compact player bar stays at the bottom; in low windows the player card
  in the sidebar gets a small cover.
- **Keyboard shortcuts:** Ctrl+? lists them all.

<img src="data/screenshots/4-narrow.png" alt="A narrow window with the compact player bar" width="300">

## Speed

Vumi is a native GTK 4 app written in Rust, with no web browser inside. Measured on the
author's library: 651 albums and 11,333 songs on a Navidrome server in the local network.

| | |
|---|---|
| From start to the library on screen (saved login) | 0.46 s |
| From the first login to the albums | 0.56 s |
| The window's first frame | 179 ms (261 ms on a cold start) |
| CPU while idle | 0.07 % of one core |
| From a typed phrase to the search results | 186 ms (150 ms of it is the search field's typing delay) |
| CPU while music plays, window in the background | 0.45 % of one core |
| CPU while music plays, album page in front | 1.2 % of one core |

Vumi 0.1.2, medians of 5 runs; a PC with an AMD Ryzen 7 9700X, a test compositor at 120 Hz.

## Install

```sh
flatpak install --user https://flatpak.bojanowski.me/io.github.tomaszbojanowski.Vumi.flatpakref
```

The GNOME runtime it needs comes from Flathub. Updates arrive with
`flatpak update` or in GNOME Software.

You need a Navidrome server, or another server that speaks the Subsonic or
OpenSubsonic API (tested with Navidrome 0.64).

Without the repository: download `vumi-X.Y.Z.flatpak` from
[Releases](https://github.com/TomaszBojanowski/vumi/releases) and install it
with `flatpak install --user vumi-X.Y.Z.flatpak`. Such a bundle does not
update itself.

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

## How Vumi is made

Vumi's code is written with Claude Code, an AI coding assistant, under my
direction. I decide what Vumi does and how it looks, plan each change, and
test every release on my own library before it ships.

## License

Vumi is free software, released under the GNU General Public License,
version 3 or later. See [LICENSE](LICENSE).

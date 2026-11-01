//! Remembers the albums that played lately, newest first, each with the song
//! and the second it stopped at: the Last Played card resumes from there.
//! Saved when the song changes, on pause and when the app closes; not every
//! second, so the disk stays quiet.

use gtk::gio;
use gtk::glib::variant::ToVariant;
use gtk::prelude::*;

use crate::{player, APP_ID};

/// The settings key; one (album, song, seconds) entry per album.
const KEY: &str = "played-albums";

/// How many albums are remembered. The Last Played card needs the newest one
/// that is not in the player; the rest stand in for albums that have left
/// the library since.
const SIZE: usize = 10;

/// Where playback of one album stopped.
pub(crate) struct Played {
    pub(crate) album_id: String,
    pub(crate) song_id: String,
    pub(crate) seconds: u32,
}

/// The albums that played lately, newest first.
pub(crate) fn history() -> Vec<Played> {
    read(&gio::Settings::new(APP_ID))
        .into_iter()
        .map(|(album_id, song_id, seconds)| Played {
            album_id,
            song_id,
            seconds,
        })
        .collect()
}

/// Starts saving whenever the player changes. Call once, at startup.
pub(crate) fn watch() {
    let settings = gio::Settings::new(APP_ID);
    player::get().connect_changed(move || save(&settings));
}

/// Saves right away and makes sure it reaches the disk (the app is closing).
pub(crate) fn save_now() {
    save(&gio::Settings::new(APP_ID));
    gio::Settings::sync();
}

fn read(settings: &gio::Settings) -> Vec<(String, String, u32)> {
    settings.value(KEY).get().unwrap_or_default()
}

fn save(settings: &gio::Settings) {
    let player = player::get();
    // Only while a track is in progress (playing or paused). After an error or
    // the end of the queue there is no position any more, and saving 0 would
    // lose the real point to resume from.
    if !player.is_active() {
        return;
    }
    let Some(song) = player.current() else {
        return;
    };
    // A song without an album cannot be resumed from the card.
    let Some(album_id) = song.album_id.filter(|id| !id.is_empty()) else {
        return;
    };
    let position = player.position_secs().unwrap_or(0.0) as u32;
    // The album moves to the front with where it is now; its older entry goes.
    let old = read(settings);
    let mut played = old.clone();
    played.retain(|(album, _, _)| *album != album_id);
    played.insert(0, (album_id, song.id, position));
    played.truncate(SIZE);
    // A change that moved nothing (the volume, say) writes nothing.
    if played != old {
        let _ = settings.set_value(KEY, &played.to_variant());
    }
}

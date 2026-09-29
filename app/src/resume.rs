//! Remembers what played last (album, song, position), for "Continue listening"
//! and "Similar to what played last". Saved when the song changes, on pause and
//! when the app closes; not every second, so the disk stays quiet.

use gtk::gio;
use gtk::prelude::*;

use crate::{player, APP_ID};

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
    let position = player.position_secs().unwrap_or(0.0) as u32;
    let _ = settings.set_string("last-album-id", song.album_id.as_deref().unwrap_or(""));
    let _ = settings.set_string("last-song-id", &song.id);
    let _ = settings.set_uint("last-position", position);
}

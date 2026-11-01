//! Remembers the albums that played lately, newest first, each with the song
//! and the second it stopped at: the Start page resumes from there. Only
//! albums started as albums count (player::QueueOf::Album); radio, playlists
//! and other lists of songs leave the history as it is.
//! Saved when the song changes, on pause, when the player leaves a track by
//! hand (another album, Stop), when the app closes and when the session ends;
//! not every second, so the disk stays quiet.

use gtk::gio;
use gtk::glib::variant::ToVariant;
use gtk::prelude::*;

use crate::{APP_ID, player};

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
    let player = player::get();
    {
        let settings = settings.clone();
        player.connect_changed(move || save(&settings));
    }
    // Leaving a track by hand: where it was, read just before the player moves
    // on; the change that follows knows only the new track.
    player.connect_leaving(move |song, seconds| {
        if player::get().is_album_song(song) {
            remember(&settings, song, seconds);
        }
    });
}

/// Saves when the process is told to end (SIGTERM, as when the session ends),
/// then calls `quit`.
pub(crate) fn save_on_terminate(quit: impl Fn() + 'static) {
    use tokio::signal::unix::{SignalKind, signal};
    // Watched from here on: a SIGTERM before this would end the process
    // without saving.
    let terminate = {
        let _runtime = crate::runtime().enter();
        signal(SignalKind::terminate())
    };
    let Ok(mut terminate) = terminate else {
        eprintln!("Could not watch for SIGTERM");
        return;
    };
    let (told, heard) = tokio::sync::oneshot::channel();
    crate::runtime().spawn(async move {
        terminate.recv().await;
        let _ = told.send(());
    });
    // Back on the main thread, where the player lives.
    gtk::glib::spawn_future_local(async move {
        if heard.await.is_ok() {
            save_now();
            quit();
        }
    });
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
    if !player.is_album_song(&song) {
        return;
    }
    remember(settings, &song, player.resume_position().unwrap_or(0.0));
}

/// Puts the album of `song` first, at `song` and `seconds`; its older entry
/// goes.
fn remember(settings: &gio::Settings, song: &subsonic::Song, seconds: f64) {
    // A song without an album cannot be resumed from the card.
    let Some(album_id) = song.album_id.as_deref().filter(|id| !id.is_empty()) else {
        return;
    };
    let old = read(settings);
    let mut played = old.clone();
    played.retain(|(album, _, _)| album != album_id);
    played.insert(0, (album_id.to_owned(), song.id.clone(), seconds as u32));
    played.truncate(SIZE);
    // A change that moved nothing (the volume, say) writes nothing.
    if played != old {
        let _ = settings.set_value(KEY, &played.to_variant());
    }
}

#[cfg(test)]
mod tests {
    //! Where an album stopped is remembered when the player leaves it by hand,
    //! and only albums started as albums go into the history.
    //! A real playbin plays a silent WAV from a local HTTP server into a
    //! fakesink; the settings are kept in memory, so nothing outside the test
    //! changes.
    use super::*;
    use gtk::glib;
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::time::{Duration, Instant};

    /// `seconds` of silence: 8 kHz, mono, 16 bit.
    fn silent_wav(seconds: u32) -> Vec<u8> {
        let rate = 8000u32;
        let data = rate * 2 * seconds;
        let mut wav = Vec::with_capacity(44 + data as usize);
        wav.extend_from_slice(b"RIFF");
        wav.extend_from_slice(&(36 + data).to_le_bytes());
        wav.extend_from_slice(b"WAVEfmt ");
        wav.extend_from_slice(&16u32.to_le_bytes());
        wav.extend_from_slice(&1u16.to_le_bytes()); // PCM
        wav.extend_from_slice(&1u16.to_le_bytes()); // mono
        wav.extend_from_slice(&rate.to_le_bytes());
        wav.extend_from_slice(&(rate * 2).to_le_bytes());
        wav.extend_from_slice(&2u16.to_le_bytes());
        wav.extend_from_slice(&16u16.to_le_bytes());
        wav.extend_from_slice(b"data");
        wav.extend_from_slice(&data.to_le_bytes());
        wav.resize(44 + data as usize, 0);
        wav
    }

    /// Answers every request with `wav` (the stream URL and the server's other
    /// calls alike) and returns the server's address.
    fn serve(wav: Vec<u8>) -> String {
        let listener = TcpListener::bind("127.0.0.1:0").expect("no local port");
        let port = listener.local_addr().expect("no local address").port();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let wav = wav.clone();
                std::thread::spawn(move || answer(stream, &wav));
            }
        });
        format!("http://127.0.0.1:{port}")
    }

    fn answer(mut stream: TcpStream, wav: &[u8]) {
        let mut request = [0u8; 4096];
        let _ = stream.read(&mut request);
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: audio/x-wav\r\nContent-Length: {}\r\n\
             Connection: close\r\n\r\n",
            wav.len()
        );
        let _ = stream.write_all(head.as_bytes());
        let _ = stream.write_all(wav);
    }

    /// The app's schema, compiled into a directory of the test, and settings
    /// that live in memory. Must run before anything reads a setting.
    fn settings_in_memory() {
        let dir = std::env::temp_dir().join(format!("vumi-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).expect("no temporary directory");
        let status = std::process::Command::new("glib-compile-schemas")
            .arg("--targetdir")
            .arg(&dir)
            .arg(concat!(env!("CARGO_MANIFEST_DIR"), "/../data"))
            .status()
            .expect("glib-compile-schemas is missing");
        assert!(status.success(), "the schema does not compile");
        // SAFETY: the first thing this test does, before GStreamer, tokio or
        // GSettings start any thread that reads the environment; it is the only
        // test of this program that touches settings or the environment.
        unsafe {
            std::env::set_var("GSETTINGS_SCHEMA_DIR", &dir);
            std::env::set_var("GSETTINGS_BACKEND", "memory");
        }
    }

    /// Runs the main loop until `done` holds, at most `limit`.
    fn run_until(limit: Duration, done: impl Fn() -> bool) -> bool {
        let context = glib::MainContext::default();
        let start = Instant::now();
        while start.elapsed() < limit {
            while context.iteration(false) {}
            if done() {
                return true;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        done()
    }

    fn song(id: &str, album: &str) -> subsonic::Song {
        subsonic::Song {
            id: id.into(),
            title: id.into(),
            artist: None,
            track: None,
            disc_number: None,
            album: Some(album.into()),
            album_id: Some(album.into()),
            artist_id: None,
            duration: Some(30),
            suffix: None,
            bit_rate: None,
            cover_art: None,
            bit_depth: None,
            sampling_rate: None,
            starred: None,
            composer: None,
        }
    }

    /// A queue started as the album `id`.
    fn album(id: &str) -> player::QueueOf {
        player::QueueOf::Album(id.into())
    }

    /// The whole history, to compare before and after.
    fn snapshot() -> Vec<(String, String, u32)> {
        history()
            .into_iter()
            .map(|played| (played.album_id, played.song_id, played.seconds))
            .collect()
    }

    /// The song and the second remembered for `album`.
    fn remembered(album: &str) -> Option<(String, u32)> {
        history()
            .into_iter()
            .find(|played| played.album_id == album)
            .map(|played| (played.song_id, played.seconds))
    }

    #[test]
    fn only_albums_played_as_albums_go_into_the_history_and_keep_their_place() {
        settings_in_memory();
        let context = glib::MainContext::default();
        let _owner = context.acquire().expect("the main context is taken");
        gio::Settings::new(APP_ID)
            .set_boolean("report-listens", false)
            .expect("report-listens cannot be set");
        let client = subsonic::Client::new(&serve(silent_wav(30)), "user", "password");
        let player = player::get();
        player.use_fake_sink();
        watch();
        let long = Duration::from_secs(15);
        let played_for = |seconds: f64| {
            run_until(long, || {
                player.is_playing() && player.position_secs().unwrap_or(0.0) >= seconds
            })
        };
        let mut failures = Vec::new();

        // "Resume" on the Last Played card, then at once another album: the
        // first one keeps the second it was resumed at.
        player.play_from(
            client.clone(),
            vec![song("a1", "A"), song("a2", "A")].into(),
            1,
            20.0,
            album("A"),
        );
        player.play(client.clone(), vec![song("b1", "B")].into(), 0, album("B"));
        if remembered("A") != Some(("a2".into(), 20)) {
            failures.push(format!(
                "resumed and left at once: A is {:?}",
                remembered("A")
            ));
        }

        // Another album by hand, without a pause, after two seconds and more.
        assert!(played_for(2.3), "B did not play");
        player.play(client.clone(), vec![song("c1", "C")].into(), 0, album("C"));
        if !remembered("B").is_some_and(|(id, seconds)| id == "b1" && seconds >= 2) {
            failures.push(format!("another album started: B is {:?}", remembered("B")));
        }

        // Stop (MPRIS) after two seconds and more.
        assert!(played_for(2.3), "C did not play");
        player.stop();
        if !remembered("C").is_some_and(|(id, seconds)| id == "c1" && seconds >= 2) {
            failures.push(format!("stopped: C is {:?}", remembered("C")));
        }

        // SIGTERM (the session ends) after two seconds and more.
        player.play(client.clone(), vec![song("d1", "D")].into(), 0, album("D"));
        assert!(played_for(2.3), "D did not play");
        let quit = std::rc::Rc::new(std::cell::Cell::new(false));
        save_on_terminate({
            let quit = quit.clone();
            move || quit.set(true)
        });
        let sent = std::process::Command::new("kill")
            .args(["-TERM", &std::process::id().to_string()])
            .status()
            .expect("kill is missing");
        assert!(sent.success(), "SIGTERM was not sent");
        assert!(run_until(long, || quit.get()), "SIGTERM did not quit");
        if !remembered("D").is_some_and(|(id, seconds)| id == "d1" && seconds >= 2) {
            failures.push(format!("SIGTERM: D is {:?}", remembered("D")));
        }

        // A radio of three albums, skipped through by hand: the history of
        // played albums stays as it is.
        player.stop();
        let before = snapshot();
        player.play(
            client.clone(),
            vec![song("r1", "R1"), song("r2", "R2"), song("r3", "R3")].into(),
            0,
            player::QueueOf::Songs,
        );
        for id in ["r1", "r2", "r3"] {
            if id != "r1" {
                player.next();
            }
            assert!(
                run_until(long, || player.is_playing()
                    && player.current().is_some_and(|s| s.id == id)
                    && player.position_secs().unwrap_or(0.0) >= 0.5),
                "{id} did not play"
            );
        }
        player.stop();
        if snapshot() != before {
            failures.push(format!("a radio changed the history: {:?}", snapshot()));
        }

        // An album with a song of another album added to the queue: only the
        // album goes into the history.
        player.play(client.clone(), vec![song("e1", "E")].into(), 0, album("E"));
        assert!(played_for(0.5), "E did not play");
        player.enqueue(client.clone(), song("f1", "F"));
        player.next();
        assert!(
            run_until(long, || player.is_playing()
                && player.current().is_some_and(|s| s.id == "f1")
                && player.position_secs().unwrap_or(0.0) >= 0.5),
            "f1 did not play"
        );
        player.stop();
        if remembered("E").is_none() || remembered("F").is_some() {
            failures.push(format!(
                "an added song: E is {:?}, F is {:?}",
                remembered("E"),
                remembered("F")
            ));
        }

        // Twenty volume steps (a slider drag): the listeners of track and
        // state changes hear nothing of them, and the setting, which rewrites
        // the whole settings file, is written after the steps, not at each.
        let settings = gio::Settings::new(APP_ID);
        let changes = std::rc::Rc::new(std::cell::Cell::new(0));
        let counted = changes.clone();
        let listener = player.connect_changed(move || counted.set(counted.get() + 1));
        let saved = settings.double("volume");
        for step in 1..=20 {
            player.set_volume(0.3 + 0.01 * f64::from(step));
        }
        if changes.get() != 0 {
            failures.push(format!(
                "volume steps told {} change listeners",
                changes.get()
            ));
        }
        if settings.double("volume") != saved {
            failures.push("the volume was written at every step".into());
        }
        if !run_until(Duration::from_secs(2), || {
            (settings.double("volume") - 0.5).abs() < 1e-9
        }) {
            failures.push("the volume was not written after the steps".into());
        }
        player.disconnect(listener);

        assert!(failures.is_empty(), "{}", failures.join("; "));
    }
}

//! Tests with a real playbin: it plays a silent WAV from a local HTTP server
//! into a fakesink, with the settings kept in memory, so nothing outside the
//! test changes. The settings, the environment and the default main context
//! belong to the whole process, so one test runs every scenario in turn;
//! each scenario lives next to the code it tests.

use gtk::gio::prelude::*;
use gtk::glib;
use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

#[test]
fn scenarios_with_a_real_playbin() {
    let _schemas = settings_in_memory();
    let context = glib::MainContext::default();
    let _owner = context.acquire().expect("the main context is taken");
    crate::settings()
        .set_boolean("report-listens", false)
        .expect("report-listens cannot be set");
    crate::player::get().use_fake_sink();
    let mut failures = Vec::new();
    crate::resume::tests::only_albums_played_as_albums_go_into_the_history_and_keep_their_place(
        &mut failures,
    );
    crate::player::tests::a_failed_stream_says_why_and_play_goes_on_from_there(&mut failures);
    crate::player::tests::offline_leaves_the_queue_and_the_track_as_they_were(&mut failures);
    assert!(failures.is_empty(), "{}", failures.join("; "));
}

/// `seconds` of silence: 8 kHz, mono, 16 bit (16 000 bytes a second).
pub(crate) fn silent_wav(seconds: u32) -> Vec<u8> {
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

/// A local server that answers every request with one WAV: the stream URL
/// and the server's other calls alike (those answers are no Subsonic
/// response, but they show that the server is there). It can go away (its
/// port refuses connections, and streams on their way are cut off) and come
/// back on the same port, send streams at twice their pace instead of at
/// once, as over a real network, and answer with text instead of sound.
pub(crate) struct Server {
    pub(crate) url: String,
    up: Arc<AtomicBool>,
    listening: Arc<AtomicBool>,
    paced: Arc<AtomicBool>,
    not_audio: Arc<AtomicBool>,
}

impl Server {
    pub(crate) fn start(wav: Vec<u8>) -> Server {
        let listener = TcpListener::bind("127.0.0.1:0").expect("no local port");
        let port = listener.local_addr().expect("no local address").port();
        let server = Server {
            url: format!("http://127.0.0.1:{port}"),
            up: Arc::new(AtomicBool::new(true)),
            listening: Arc::new(AtomicBool::new(true)),
            paced: Arc::new(AtomicBool::new(false)),
            not_audio: Arc::new(AtomicBool::new(false)),
        };
        let up = server.up.clone();
        let listening = server.listening.clone();
        let paced = server.paced.clone();
        let not_audio = server.not_audio.clone();
        let wav: Arc<[u8]> = wav.into();
        std::thread::spawn(move || {
            let mut listener = Some(listener);
            loop {
                let want = up.load(Ordering::SeqCst);
                if want && listener.is_none() {
                    listener = TcpListener::bind(("127.0.0.1", port)).ok();
                } else if !want {
                    listener = None;
                }
                listening.store(listener.is_some(), Ordering::SeqCst);
                let accepted = listener.as_ref().and_then(|listener| {
                    let _ = listener.set_nonblocking(true);
                    listener.accept().ok()
                });
                let Some((stream, _)) = accepted else {
                    std::thread::sleep(Duration::from_millis(5));
                    continue;
                };
                let _ = stream.set_nonblocking(false);
                let (wav, up) = (wav.clone(), up.clone());
                let paced = paced.load(Ordering::SeqCst);
                let not_audio = not_audio.load(Ordering::SeqCst);
                std::thread::spawn(move || answer(stream, &wav, &up, paced, not_audio));
            }
        });
        server
    }

    /// The server goes away or comes back; returns once its port follows.
    pub(crate) fn set_up(&self, up: bool) {
        self.up.store(up, Ordering::SeqCst);
        let start = Instant::now();
        while self.listening.load(Ordering::SeqCst) != up
            && start.elapsed() < Duration::from_secs(2)
        {
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    /// Streams from now on come at twice their pace.
    pub(crate) fn set_paced(&self, paced: bool) {
        self.paced.store(paced, Ordering::SeqCst);
    }

    /// Answers from now on are text, not sound.
    pub(crate) fn set_not_audio(&self, not_audio: bool) {
        self.not_audio.store(not_audio, Ordering::SeqCst);
    }
}

fn answer(mut stream: TcpStream, wav: &[u8], up: &AtomicBool, paced: bool, not_audio: bool) {
    let mut request = [0u8; 4096];
    let read = stream.read(&mut request).unwrap_or(0);
    let request = String::from_utf8_lossy(&request[..read]);
    let (kind, whole): (&str, &[u8]) = if not_audio {
        ("text/plain", b"This is not a sound.")
    } else {
        ("audio/x-wav", wav)
    };
    // A seek asks for the rest from a byte on ("Range: bytes=N-").
    let from = request
        .lines()
        .find_map(|line| {
            line.to_ascii_lowercase()
                .strip_prefix("range: bytes=")
                .map(str::to_owned)
        })
        .and_then(|range| range.trim_end_matches('-').parse::<usize>().ok())
        .filter(|from| *from < whole.len());
    let body = &whole[from.unwrap_or(0)..];
    let status = match from {
        Some(from) => format!(
            "206 Partial Content\r\nContent-Range: bytes {from}-{}/{}",
            whole.len() - 1,
            whole.len()
        ),
        None => "200 OK\r\nAccept-Ranges: bytes".to_owned(),
    };
    let head = format!(
        "HTTP/1.1 {status}\r\nContent-Type: {kind}\r\nContent-Length: {}\r\n\
         Connection: close\r\n\r\n",
        body.len()
    );
    if stream.write_all(head.as_bytes()).is_err() {
        return;
    }
    if !paced {
        let _ = stream.write_all(body);
        return;
    }
    // A quarter of a second of sound every eighth of a second, until the
    // server goes: then the rest never comes.
    for chunk in body.chunks(4000) {
        if !up.load(Ordering::SeqCst) || stream.write_all(chunk).is_err() {
            return;
        }
        std::thread::sleep(Duration::from_millis(125));
    }
}

/// The directory of the compiled schema, removed when the test ends.
struct Schemas(std::path::PathBuf);

impl Drop for Schemas {
    fn drop(&mut self) {
        // GSettings keeps the file it read open; on Linux it stays
        // readable after the removal.
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// The app's schema, compiled into a directory of the test, and settings
/// that live in memory. Must run before anything reads a setting.
fn settings_in_memory() -> Schemas {
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
    Schemas(dir)
}

/// Runs the main loop until `done` holds, at most `limit`.
pub(crate) fn run_until(limit: Duration, done: impl Fn() -> bool) -> bool {
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

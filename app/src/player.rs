//! Playback engine: one GStreamer playbin for the whole app, plus the play queue.
//! It lives on the GTK main thread; GStreamer does the heavy work on its own threads
//! and reports back through its bus, which the GTK main loop watches.
//! Widgets that show the playback state register with connect_changed().

use gettextrs::gettext;
use gstreamer as gst;
use gst::prelude::*;
use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};
use std::sync::{Arc, Mutex};

/// What the player is doing. Paused and Ended look alike (nothing sounds),
/// but act differently: Play resumes a paused track and restarts an ended queue.
#[derive(Clone, Debug, PartialEq)]
enum State {
    /// Nothing has been played yet.
    Idle,
    Playing,
    Paused,
    /// The queue played to its end.
    Ended,
    /// Playback failed; the text is shown to the user.
    Failed(String),
}

pub(crate) struct Player {
    playbin: gst::Element,
    queue: RefCell<Rc<[subsonic::Song]>>,
    position: Cell<usize>,
    state: RefCell<State>,
    client: RefCell<Option<subsonic::Client>>,
    /// Called after every change of track or state.
    listeners: RefCell<Vec<(u64, Box<dyn Fn()>)>>,
    next_listener: Cell<u64>,
    /// Shows playback errors to the user (a toast in the window).
    error_handler: RefCell<Option<Box<dyn Fn(&str)>>>,
    /// The listen in progress, for scrobbling.
    listen: RefCell<Option<Listen>>,
    /// Whether the once-a-second listen timer runs already.
    listen_timer: std::cell::Cell<bool>,
    /// Whether the server takes live playback reports (OpenSubsonic
    /// playbackReport): None until asked, Some(false) while asking.
    report_support: std::cell::Cell<Option<bool>>,
    /// Told about every jump of the position (MPRIS sends Seeked).
    seek_handler: RefCell<Option<Box<dyn Fn(f64)>>>,
    /// Counts the user's commands (play, stop, changes to the queue). Requests
    /// to the server remember it; an answer for an older command is dropped.
    command: Cell<u64>,
    /// Where to jump once the new track is ready (resuming playback).
    pending_seek: Cell<Option<f64>>,
    /// Gapless: the next track (its id and address), read by GStreamer's
    /// about-to-finish signal (on its own thread) to start it without a gap.
    next_uri: Arc<Mutex<Option<(String, String)>>>,
    /// Set by about-to-finish to the id of the track it queued: the StreamStart
    /// that follows then means "that track has begun".
    switched_to: Arc<Mutex<Option<String>>>,
    /// Keeps the bus watch alive for as long as the player exists.
    _bus_watch: gst::bus::BusWatchGuard,
}

thread_local! {
    static PLAYER: OnceCell<Rc<Player>> = const { OnceCell::new() };
}

/// The one player of the app, created on first use.
pub(crate) fn get() -> Rc<Player> {
    PLAYER.with(|cell| cell.get_or_init(Player::new).clone())
}

impl Player {
    fn new() -> Rc<Self> {
        gst::init().expect("GStreamer could not start");
        let playbin = gst::ElementFactory::make("playbin")
            .build()
            .expect("GStreamer playbin is missing");
        let bus = playbin.bus().expect("playbin has a bus");

        // Gapless: a few seconds before a track ends, GStreamer asks for the next
        // one (on its own thread). The address was prepared on the main thread
        // (prepare_next), so the track after starts without any silence.
        let next_uri: Arc<Mutex<Option<(String, String)>>> = Arc::new(Mutex::new(None));
        let switched_to: Arc<Mutex<Option<String>>> = Arc::new(Mutex::new(None));
        {
            let next_uri = next_uri.clone();
            let switched_to = switched_to.clone();
            playbin.connect("about-to-finish", false, move |values| {
                let playbin = values[0].get::<gst::Element>().ok()?;
                let next = next_uri.lock().ok().and_then(|mut next| next.take());
                if let Some((id, uri)) = next {
                    playbin.set_property("uri", uri.as_str());
                    if let Ok(mut switched) = switched_to.lock() {
                        *switched = Some(id);
                    }
                }
                None
            });
        }

        Rc::new_cyclic(|weak: &Weak<Player>| {
            let weak = weak.clone();
            let bus_watch = bus
                .add_watch_local(move |_, message| {
                    if let Some(player) = weak.upgrade() {
                        player.on_message(message);
                    }
                    gst::glib::ControlFlow::Continue
                })
                .expect("could not watch the GStreamer bus");

            Player {
                playbin,
                queue: RefCell::new(Vec::new().into()),
                position: Cell::new(0),
                state: RefCell::new(State::Idle),
                client: RefCell::new(None),
                listeners: RefCell::new(Vec::new()),
                next_listener: Cell::new(0),
                error_handler: RefCell::new(None),
                listen: RefCell::new(None),
                listen_timer: std::cell::Cell::new(false),
                report_support: std::cell::Cell::new(None),
                seek_handler: RefCell::new(None),
                command: Cell::new(0),
                pending_seek: Cell::new(None),
                next_uri,
                switched_to,
                _bus_watch: bus_watch,
            }
        })
    }

    /// Registers `f`, called after every change; the returned id removes it again.
    pub(crate) fn connect_changed(&self, f: impl Fn() + 'static) -> u64 {
        let id = self.next_listener.get();
        self.next_listener.set(id + 1);
        self.listeners.borrow_mut().push((id, Box::new(f)));
        id
    }

    pub(crate) fn disconnect(&self, id: u64) {
        self.listeners
            .borrow_mut()
            .retain(|(listener_id, _)| *listener_id != id);
    }

    /// Sets how playback errors reach the user; the app shows them as toasts.
    pub(crate) fn set_error_handler(&self, f: impl Fn(&str) + 'static) {
        self.error_handler.replace(Some(Box::new(f)));
    }

    pub(crate) fn set_seek_handler(&self, f: impl Fn(f64) + 'static) {
        self.seek_handler.replace(Some(Box::new(f)));
    }

    fn notify(&self) {
        for (_, listener) in self.listeners.borrow().iter() {
            listener();
        }
    }

    fn set_state(&self, state: State) {
        let old = self.state.replace(state);
        let report = match (&old, &*self.state.borrow()) {
            (State::Playing, State::Paused) => Some("paused"),
            (State::Paused, State::Playing) => Some("playing"),
            (State::Playing | State::Paused, State::Ended | State::Failed(_) | State::Idle) => {
                Some("stopped")
            }
            _ => None,
        };
        if let Some(report) = report {
            self.report(report, None);
        }
        self.notify();
    }

    /// Stops, remembers the error and tells the user.
    fn fail(&self, message: String) {
        eprintln!("Playback error: {message}");
        let _ = self.playbin.set_state(gst::State::Null);
        self.set_state(State::Failed(message.clone()));
        if let Some(handler) = self.error_handler.borrow().as_ref() {
            handler(&message);
        }
    }

    /// A command that replaces or edits the queue, or picks a track by hand:
    /// an answer to an older request (a radio asked for before it) is dropped
    /// when it arrives. Automatic changes (gapless, the next track after one
    /// ends) and a pause do not count, so a radio asked for near the end of a
    /// track still plays.
    fn new_command(&self) {
        self.command.set(self.command.get() + 1);
    }

    /// Replaces the queue with `songs` and starts playing the one at `start`.
    pub(crate) fn play(&self, client: subsonic::Client, songs: Rc<[subsonic::Song]>, start: usize) {
        self.new_command();
        self.client.replace(Some(client));
        self.queue.replace(songs);
        self.play_at(start);
    }

    /// Like play(), but starts the track at `seconds` (resuming where playback
    /// stopped). GStreamer ignores seeks until the track is ready, so the seek
    /// waits for AsyncDone.

    pub(crate) fn play_from(
        &self,
        client: subsonic::Client,
        songs: Rc<[subsonic::Song]>,
        start: usize,
        seconds: f64,
    ) {
        self.play(client, songs, start);
        if seconds > 0.0 {
            self.pending_seek.set(Some(seconds));
        }
    }

    fn play_at(&self, index: usize) {
        // A new track: a seek meant for the previous one must not land here.
        self.pending_seek.set(None);
        // Nor may a gapless switch of the previous one move this track on.
        self.forget_switch();
        let song = self.queue.borrow().get(index).cloned();
        let client = self.client.borrow().clone();
        let (Some(song), Some(client)) = (song, client) else {
            // Endless radio: at the end of the queue, songs similar to the last
            // one are added instead of stopping.
            if index > 0
                && index == self.queue.borrow().len()
                && self.client.borrow().is_some()
                && endless_radio()
            {
                self.extend_with_similar();
                return;
            }
            // Past the end of the queue: stop. The last track stays current,
            // so it can still be shown and remembered.
            let _ = self.playbin.set_state(gst::State::Null);
            self.set_state(State::Ended);
            return;
        };

        self.position.set(index);
        let Ok(url) = client.stream_url(&song.id) else {
            self.fail(gettext("Could not play the track"));
            return;
        };
        let _ = self.playbin.set_state(gst::State::Null);
        self.playbin.set_property("uri", url.as_str());
        match self.playbin.set_state(gst::State::Playing) {
            Ok(_) => {
                self.set_state(State::Playing);
                self.begin_listen();
                self.prepare_next();
            }
            // Errors found later (server gone, broken file) arrive on the bus.
            Err(_) => self.fail(gettext("Could not start playback")),
        }
    }

    pub(crate) fn current(&self) -> Option<subsonic::Song> {
        self.queue.borrow().get(self.position.get()).cloned()
    }

    pub(crate) fn is_playing(&self) -> bool {
        *self.state.borrow() == State::Playing
    }

    pub(crate) fn is_paused(&self) -> bool {
        *self.state.borrow() == State::Paused
    }

    /// Playing or paused: the current track is in progress (not ended, not failed).
    pub(crate) fn is_active(&self) -> bool {
        self.is_playing() || self.is_paused()
    }

    /// The error text when playback has failed, `None` otherwise.
    pub(crate) fn failure(&self) -> Option<String> {
        match &*self.state.borrow() {
            State::Failed(message) => Some(message.clone()),
            _ => None,
        }
    }

    pub(crate) fn client(&self) -> Option<subsonic::Client> {
        self.client.borrow().clone()
    }

    /// Play/Pause: pauses or resumes; after the end of the queue it starts again,
    /// after an error it tries the same track once more.
    pub(crate) fn toggle_pause(&self) {
        let state = self.state.borrow().clone();
        match state {
            State::Playing => {
                let _ = self.playbin.set_state(gst::State::Paused);
                self.set_state(State::Paused);
            }
            State::Paused => match self.playbin.set_state(gst::State::Playing) {
                Ok(_) => self.set_state(State::Playing),
                Err(_) => self.fail(gettext("Could not start playback")),
            },
            State::Ended => self.play_at(0),
            State::Failed(_) => self.play_at(self.position.get()),
            State::Idle => {}
        }
    }

    pub(crate) fn next(&self) {
        self.new_command();
        let next = self.position.get() + 1;
        if next < self.queue.borrow().len() {
            self.play_at(next);
        }
    }

    /// Back to the start of the track, or to the previous track if we are at its start.
    pub(crate) fn previous(&self) {
        self.new_command();
        let position = self.position.get();
        if self.position_secs().unwrap_or(0.0) > 3.0 || position == 0 {
            self.seek(0.0);
        } else {
            self.play_at(position - 1);
        }
    }

    pub(crate) fn seek(&self, seconds: f64) {
        let target = gst::ClockTime::from_mseconds((seconds.max(0.0) * 1000.0) as u64);
        let sought = self
            .playbin
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, target);
        if sought.is_ok() {
            let state = if self.is_playing() { "playing" } else { "paused" };
            self.report(state, Some((seconds.max(0.0) * 1000.0) as u64));
            if let Some(handler) = self.seek_handler.borrow().as_ref() {
                handler(seconds.max(0.0));
            }
        }
    }

    pub(crate) fn position_secs(&self) -> Option<f64> {
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(|t| t.mseconds() as f64 / 1000.0)
    }

    fn on_message(&self, message: &gst::Message) {
        match message.view() {
            gst::MessageView::Eos(..) => self.play_at(self.position.get() + 1),
            // A track has begun: if about-to-finish queued it, this is the gapless
            // step to the next track in the queue.
            gst::MessageView::StreamStart(..) => {
                let begun = self.switched_to.lock().ok().and_then(|mut s| s.take());
                if let Some(id) = begun {
                    self.advance_gapless(&id);
                }
            }
            // The new track is ready: a pending seek (resuming) takes effect now.
            gst::MessageView::AsyncDone(..) => {
                if let Some(seconds) = self.pending_seek.take() {
                    self.seek(seconds);
                }
            }
            // Only the short error text: the debug details may contain the stream
            // URL, and with it the login token.
            gst::MessageView::Error(err) => {
                self.fail(format!("{}: {}", gettext("Could not play the track"), err.error()));
            }
            _ => {}
        }
    }
}

/// Stops playback cleanly when the app quits: GStreamer wants its elements in
/// the NULL state before they are destroyed. Does nothing if nothing was played.
pub(crate) fn shutdown() {
    PLAYER.with(|cell| {
        if let Some(player) = cell.get() {
            let _ = player.playbin.set_state(gst::State::Null);
        }
    });
}

/// One play of one song, as the server hears about it.
struct Listen {
    id: String,
    /// When it began, in milliseconds since the Unix epoch.
    started: u64,
    /// Seconds actually played: pauses and seeks do not count.
    played: u64,
    submitted: bool,
}

impl Player {
    /// A new track has begun: the server shows it as playing now, and the
    /// timer starts counting towards a finished listen.
    fn begin_listen(&self) {
        let Some(song) = self.current() else {
            return;
        };
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        self.listen.replace(Some(Listen {
            id: song.id.clone(),
            started,
            played: 0,
            submitted: false,
        }));
        if self.report_support.get() == Some(true) {
            self.report("starting", Some(0));
        } else {
            self.scrobble(song.id.clone(), false, None);
        }
        if self.report_support.get().is_none() {
            self.check_report_support();
        }

        if !self.listen_timer.replace(true) {
            gst::glib::timeout_add_seconds_local(1, || {
                get().tick_listen();
                gst::glib::ControlFlow::Continue
            });
        }
    }

    /// Once a second: counts the time played and submits the listen after half
    /// the track or four minutes, whichever comes first (the Last.fm rule).
    fn tick_listen(&self) {
        if !self.is_playing() {
            return;
        }
        let half = self
            .playbin
            .query_duration::<gst::ClockTime>()
            .map(|d| d.seconds() / 2)
            .unwrap_or(240);
        let (submit, heartbeat) = {
            let mut guard = self.listen.borrow_mut();
            let Some(listen) = guard.as_mut() else {
                return;
            };
            listen.played += 1;
            let submit = (!listen.submitted && listen.played >= half.min(240)).then(|| {
                listen.submitted = true;
                (listen.id.clone(), listen.started)
            });
            (submit, listen.played % 15 == 0)
        };
        if let Some((id, started)) = submit {
            self.scrobble(id, true, Some(started));
        }
        // Every 15 s: keeps the server's position bar in step.
        if heartbeat {
            self.report("playing", None);
        }
    }

    /// Live state for the server's "now playing", if it takes such reports.
    /// `position_ms` defaults to where playback is now.
    fn report(&self, state: &'static str, position_ms: Option<u64>) {
        if !report_listens() {
            return;
        }
        if self.report_support.get() != Some(true) {
            return;
        }
        let Some(id) = self.listen.borrow().as_ref().map(|l| l.id.clone()) else {
            return;
        };
        let Some(client) = self.client.borrow().clone() else {
            return;
        };
        let position_ms =
            position_ms.unwrap_or_else(|| (self.position_secs().unwrap_or(0.0) * 1000.0) as u64);
        crate::runtime().spawn(async move {
            if let Err(e) = client.report_playback(&id, state, position_ms).await {
                eprintln!("Could not report playback of {id}: {e}");
            }
        });
    }

    /// Asks the server once whether it takes live reports; until it answers,
    /// the old now-playing scrobble stands in.
    fn check_report_support(&self) {
        let Some(client) = self.client.borrow().clone() else {
            return;
        };
        self.report_support.set(Some(false));
        gst::glib::spawn_future_local(async move {
            let result = crate::runtime().spawn(async move { client.extensions().await }).await;
            let supported = matches!(&result, Ok(Ok(names)) if names.iter().any(|n| n == "playbackReport"));
            let player = get();
            // Logged out meanwhile: the answer is about another server.
            if supported && player.report_support.get() == Some(false) {
                player.report_support.set(Some(true));
                let state = if player.is_playing() { "playing" } else { "paused" };
                player.report(state, None);
            }
        });
    }

    /// Sends a scrobble in the background; a failure only goes to the log.
    fn scrobble(&self, id: String, submission: bool, time: Option<u64>) {
        if !report_listens() {
            return;
        }
        let Some(client) = self.client.borrow().clone() else {
            return;
        };
        crate::runtime().spawn(async move {
            if let Err(e) = client.scrobble(&id, submission, time).await {
                eprintln!("Could not scrobble {id}: {e}");
            }
        });
    }
}

/// "Share Listening Activity" in Preferences.
fn report_listens() -> bool {
    use gtk::gio::prelude::*;
    gtk::gio::Settings::new(crate::APP_ID).boolean("report-listens")
}

/// "Endless Radio" from the main menu.
fn endless_radio() -> bool {
    use gtk::gio::prelude::*;
    gtk::gio::Settings::new(crate::APP_ID).boolean("endless-radio")
}

impl Player {
    /// Endless radio: asks the server for songs that sound like the last one
    /// (AudioMuse-AI through Navidrome), adds those not in the queue yet and
    /// plays on. Stops if there are none. If something else was started while
    /// waiting, the answer is dropped.
    fn extend_with_similar(&self) {
        let queue = self.queue.borrow().clone();
        let command = self.command.get();
        let (Some(last), Some(client)) = (queue.last().cloned(), self.client.borrow().clone())
        else {
            return;
        };
        gst::glib::spawn_future_local(async move {
            let request = client.clone();
            let last_id = last.id.clone();
            let result = crate::runtime()
                .spawn(async move { request.similar_songs(&last_id, 50).await })
                .await;

            let player = get();
            // Something else happened meanwhile (another queue, Stop): this
            // answer is stale.
            if !player.is_current(command) || !Rc::ptr_eq(&*player.queue.borrow(), &queue) {
                return;
            }

            let known: std::collections::HashSet<&str> =
                queue.iter().map(|s| s.id.as_str()).collect();
            let new: Vec<subsonic::Song> = match result {
                Ok(Ok(songs)) => songs
                    .into_iter()
                    .filter(|s| !known.contains(s.id.as_str()))
                    .collect(),
                _ => Vec::new(),
            };
            if new.is_empty() {
                let _ = player.playbin.set_state(gst::State::Null);
                player.set_state(State::Ended);
                return;
            }

            let start = queue.len();
            let mut songs = queue.to_vec();
            songs.extend(new);
            player.queue.replace(songs.into());
            player.play_at(start);
        });
    }
}

impl Player {
    /// True if there is a track after the current one (MPRIS: CanGoNext).
    pub(crate) fn has_next(&self) -> bool {
        self.position.get() + 1 < self.queue.borrow().len()
    }

    /// Stop (MPRIS): like reaching the end of the queue; Play starts it again.
    pub(crate) fn stop(&self) {
        if self.current().is_some() {
            self.new_command();
            let _ = self.playbin.set_state(gst::State::Null);
            self.set_state(State::Ended);
        }
    }
}

impl Player {
    /// Gapless: the address of the track after the current one, ready for
    /// about-to-finish. None at the end of the queue (then the queue simply ends,
    /// or Endless Radio adds more).
    fn prepare_next(&self) {
        let next = self.position.get() + 1;
        let uri = self.queue.borrow().get(next).and_then(|song| {
            self.client
                .borrow()
                .as_ref()
                .and_then(|client| client.stream_url(&song.id).ok())
                .map(|url| (song.id.clone(), url.to_string()))
        });
        if let Ok(mut next_uri) = self.next_uri.lock() {
            *next_uri = uri;
        }
    }

    /// Gapless: the track GStreamer switched to has begun; the queue follows it.
    /// It is found by its id, not assumed to be "the next one": the queue may
    /// have changed since it was prepared (Play Next, drag and drop, removal).
    fn advance_gapless(&self, id: &str) {
        self.pending_seek.set(None);
        let queue = self.queue.borrow().clone();
        let from = self.position.get();
        // Only the song right after the current one counts (so A, A, B moves on
        // to the second A). Queue edits keep the handed-over track there, except
        // moving or removing it after about-to-finish: then the queue wins and
        // the song that is next now starts, with a short gap.
        match queue.get(from + 1) {
            Some(song) if song.id == id => {
                self.position.set(from + 1);
                self.set_state(State::Playing);
                self.begin_listen();
                self.prepare_next();
            }
            _ => self.play_at(from + 1),
        }
    }

    /// Gapless: the id of the track about-to-finish has already handed to
    /// GStreamer, which will play whatever the queue says now.
    fn pending_switch(&self) -> Option<String> {
        self.switched_to.lock().ok().and_then(|s| s.clone())
    }

    /// A switch queued for the previous track must not move the queue on.
    fn forget_switch(&self) {
        if let Ok(mut switched) = self.switched_to.lock() {
            *switched = None;
        }
    }
}

impl Player {
    /// The play queue (shared, not copied) and where in it playback is.
    pub(crate) fn queue(&self) -> Rc<[subsonic::Song]> {
        self.queue.borrow().clone()
    }

    pub(crate) fn position(&self) -> usize {
        self.position.get()
    }

    /// Jumps to another track of the current queue (the queue panel).
    pub(crate) fn play_index(&self, index: usize) {
        self.new_command();
        if index < self.queue.borrow().len() {
            self.play_at(index);
        }
    }
}

impl Player {
    /// Removes the track at `index` from the queue (the queue panel). Removing
    /// the playing track plays the next one, or ends the queue.
    pub(crate) fn remove(&self, index: usize) {
        let queue = self.queue.borrow().clone();
        if index >= queue.len() {
            return;
        }
        self.new_command();
        let mut songs = queue.to_vec();
        songs.remove(index);
        let position = self.position.get();
        self.queue.replace(songs.into());
        if index == position {
            // The next track now sits at this index; past the end, the queue ends.
            self.play_at(position);
        } else {
            if index < position {
                self.position.set(position - 1);
            }
            self.prepare_next();
            self.notify();
        }
    }

    /// Moves the track at `from` to `to` (drag and drop in the queue panel).
    /// The playing track keeps playing, wherever it ends up.
    pub(crate) fn move_track(&self, from: usize, to: usize) {
        let queue = self.queue.borrow().clone();
        if from >= queue.len() || to >= queue.len() || from == to {
            return;
        }
        self.new_command();
        let mut songs = queue.to_vec();
        let song = songs.remove(from);
        songs.insert(to, song);
        let position = self.position.get();
        let moved = if position == from {
            to
        } else if from < position && to >= position {
            position - 1
        } else if from > position && to <= position {
            position + 1
        } else {
            position
        };
        self.queue.replace(songs.into());
        self.position.set(moved);
        self.prepare_next();
        self.notify();
    }
}

impl Player {
    /// Puts `song` right after the playing track (Play Next). With nothing in
    /// progress, it simply plays.
    pub(crate) fn play_next(&self, client: subsonic::Client, song: subsonic::Song) {
        self.new_command();
        if !self.is_active() {
            self.play(client, vec![song].into(), 0);
            return;
        }
        let mut songs = self.queue.borrow().to_vec();
        let position = self.position.get();
        // Once the next track is with GStreamer it plays no matter what, so a
        // song put before it would be skipped: it goes right after that track,
        // wherever the queue has it by now.
        let index = match self.pending_switch() {
            Some(id) => songs
                .iter()
                .skip(position + 1)
                .position(|s| s.id == id)
                .map(|offset| position + offset + 2)
                .unwrap_or(position + 1),
            None => position + 1,
        }
        .min(songs.len());
        songs.insert(index, song);
        self.queue.replace(songs.into());
        self.prepare_next();
        self.notify();
    }

    /// Adds `song` at the end of the queue (Add to Queue). With nothing in
    /// progress, it simply plays.
    pub(crate) fn enqueue(&self, client: subsonic::Client, song: subsonic::Song) {
        self.new_command();
        if !self.is_active() {
            self.play(client, vec![song].into(), 0);
            return;
        }
        let mut songs = self.queue.borrow().to_vec();
        songs.push(song);
        self.queue.replace(songs.into());
        self.prepare_next();
        self.notify();
    }
}

impl Player {
    /// Forgets everything (logging out): the queue, the client with its
    /// credentials, the prepared next track. Play from MPRIS can then not
    /// start the old account's music again.
    pub(crate) fn reset(&self) {
        self.new_command();
        let _ = self.playbin.set_state(gst::State::Null);
        self.pending_seek.set(None);
        self.forget_switch();
        if let Ok(mut next_uri) = self.next_uri.lock() {
            *next_uri = None;
        }
        self.queue.replace(Vec::new().into());
        self.position.set(0);
        self.client.replace(None);
        self.report_support.set(None);
        self.set_state(State::Idle);
    }
}

impl Player {
    /// The current command number; see `command`. A request to the server keeps
    /// it and checks it against `is_current` when the answer arrives.
    pub(crate) fn command_id(&self) -> u64 {
        // A new command right away, so of two radios asked for in a row the
        // later one wins, whichever answer arrives first.
        self.command.set(self.command.get() + 1);
        self.command.get()
    }

    /// True if nothing has happened since command `id`.
    pub(crate) fn is_current(&self, id: u64) -> bool {
        self.command.get() == id
    }
}

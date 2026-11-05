//! Playback engine: one GStreamer playbin for the whole app, plus the play queue.
//! It lives on the GTK main thread; GStreamer does the heavy work on its own threads
//! and reports back through its bus, which the GTK main loop watches.
//! Widgets that show the playback state register with connect_changed().

use gettextrs::gettext;
use gst::prelude::*;
use gstreamer as gst;
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
    /// The album the queue was started as (QueueOf::Album), if it was.
    queue_album: RefCell<Option<String>>,
    position: Cell<usize>,
    state: RefCell<State>,
    client: RefCell<Option<subsonic::Client>>,
    /// Called after every change of track or state.
    listeners: Listeners<dyn Fn()>,
    /// Shows playback errors to the user (a toast in the window).
    error_handler: RefCell<Option<ErrorHandler>>,
    /// The listen in progress, for scrobbling.
    listen: RefCell<Option<Listen>>,
    /// The once-a-second listen timer; it runs only while a track plays.
    listen_timer: RefCell<Option<gst::glib::SourceId>>,
    /// Whether the server takes live playback reports (OpenSubsonic
    /// playbackReport): None until asked, Some(false) while asking.
    report_support: Cell<Option<bool>>,
    /// The ReplayGain mode the audio filter was built for.
    replay_gain_mode: Cell<Option<u32>>,
    /// Volume as on the slider (0 to 1); GStreamer gets its cube.
    volume: Cell<f64>,
    /// Told about every change of the volume, and only of it (the sidebar
    /// card, MPRIS): the listeners of track and state changes have nothing
    /// to do with the many steps of a slider drag.
    volume_listeners: Listeners<dyn Fn(f64)>,
    /// The volume waits to be written (VOLUME_SAVE_DELAY).
    volume_save: RefCell<Option<gst::glib::SourceId>>,
    /// Told about every jump of the position, with the new position in
    /// seconds (MPRIS sends Seeked, the sidebar card moves its slider).
    seek_listeners: Listeners<dyn Fn(f64)>,
    /// Called with the track in progress and the second it is at, just before
    /// the player leaves it by hand (another queue, Stop): the change that
    /// follows no longer knows where it was.
    leave_listeners: Listeners<LeaveListener>,
    /// Counts the user's commands (play, stop, changes to the queue). Requests
    /// to the server remember it; an answer for an older command is dropped.
    command: Cell<u64>,
    /// Where to jump once the new track is ready (resuming playback), and,
    /// after a failed stream, where Play goes on from.
    pending_seek: Cell<Option<f64>>,
    /// A stream has failed and the server is being asked whether it is still
    /// there (stream_failed); what the user is told waits for the answer.
    checking: Cell<bool>,
    /// Stopped because the server could not be reached (server_gone): the
    /// track waits for the connection, Play goes on once it is back.
    waits_for_server: Cell<bool>,
    /// The queue as it was before the user started another track, kept until
    /// that track plays: if the server turns out to be gone, it comes back.
    undo: RefCell<Option<Undo>>,
    /// A jump by hand on its way: where the track was before it. If the
    /// stream fails before the jump lands (the server gone), the track stays
    /// there.
    jumped_from: Cell<Option<f64>>,
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

/// The queue and the place in it before the user started another track
/// (Player::undo).
struct Undo {
    queue: Rc<[subsonic::Song]>,
    queue_album: Option<String>,
    position: usize,
    /// Where the track was: Play goes on from there.
    resume: Option<f64>,
    state: State,
}

/// Shows a playback error to the user.
type ErrorHandler = Box<dyn Fn(&str)>;
/// Told the track and the second it was at when the player leaves it by hand.
type LeaveListener = dyn Fn(&subsonic::Song, f64);

/// The callbacks of one kind, each with an id that removes it again.
struct Listeners<F: ?Sized> {
    list: RefCell<Vec<(u64, Rc<F>)>>,
    next: Cell<u64>,
}

impl<F: ?Sized> Listeners<F> {
    fn new() -> Self {
        Listeners {
            list: RefCell::new(Vec::new()),
            next: Cell::new(0),
        }
    }

    fn add(&self, f: Rc<F>) -> u64 {
        let id = self.next.get();
        self.next.set(id + 1);
        self.list.borrow_mut().push((id, f));
        id
    }

    fn remove(&self, id: u64) {
        self.list
            .borrow_mut()
            .retain(|(listener, _)| *listener != id);
    }

    /// Calls `call` with each callback. It goes through a copy of the list:
    /// a callback may add or remove callbacks (a page that a change of the
    /// player closes removes its own).
    fn each(&self, call: impl Fn(&F)) {
        let list: Vec<Rc<F>> = self.list.borrow().iter().map(|(_, f)| f.clone()).collect();
        for f in list {
            call(&f);
        }
    }
}

/// How long the volume has to stay put before it is written to the settings:
/// each write replaces the whole settings file (in Flatpak a key file), on
/// the main thread, and a slider drag is many steps.
const VOLUME_SAVE_DELAY: std::time::Duration = std::time::Duration::from_millis(500);

/// What a queue is started as. Only the songs of an album started as that
/// album go into the history of played albums (resume.rs); radio, playlists,
/// Favorites and other lists of songs leave it as it is.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum QueueOf {
    /// The songs of the album with this id.
    Album(String),
    /// Any other list of songs.
    Songs,
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
                queue_album: RefCell::new(None),
                position: Cell::new(0),
                state: RefCell::new(State::Idle),
                client: RefCell::new(None),
                listeners: Listeners::new(),
                error_handler: RefCell::new(None),
                listen: RefCell::new(None),
                listen_timer: RefCell::new(None),
                report_support: Cell::new(None),
                replay_gain_mode: Cell::new(None),
                volume: Cell::new(saved_volume()),
                volume_listeners: Listeners::new(),
                volume_save: RefCell::new(None),
                seek_listeners: Listeners::new(),
                leave_listeners: Listeners::new(),
                command: Cell::new(0),
                pending_seek: Cell::new(None),
                checking: Cell::new(false),
                waits_for_server: Cell::new(false),
                undo: RefCell::new(None),
                jumped_from: Cell::new(None),
                next_uri,
                switched_to,
                _bus_watch: bus_watch,
            }
        })
    }

    /// Registers `f`, called after every change; the returned id removes it again.
    pub(crate) fn connect_changed(&self, f: impl Fn() + 'static) -> u64 {
        self.listeners.add(Rc::new(f))
    }

    pub(crate) fn disconnect(&self, id: u64) {
        self.listeners.remove(id);
    }

    /// Sets how playback errors reach the user; the app shows them as toasts.
    pub(crate) fn set_error_handler(&self, f: impl Fn(&str) + 'static) {
        self.error_handler.replace(Some(Box::new(f)));
    }

    /// Calls `f` with the new position after every jump. Lighter than
    /// connect_changed: nothing else about the player has changed.
    pub(crate) fn connect_seeked(&self, f: impl Fn(f64) + 'static) {
        self.seek_listeners.add(Rc::new(f));
    }

    /// Registers `f`, called with the track in progress and where in it, just
    /// before the player leaves that track by hand (see `leave`).
    pub(crate) fn connect_leaving(&self, f: impl Fn(&subsonic::Song, f64) + 'static) {
        self.leave_listeners.add(Rc::new(f));
    }

    /// The track in progress is about to be left by hand: another queue
    /// replaces this one, or Stop. Tells the listeners where it is, while that
    /// can still be read; after an error or the end of the queue there is no
    /// place to tell.
    fn leave(&self) {
        if !self.is_active() {
            return;
        }
        let (Some(song), Some(seconds)) = (self.current(), self.resume_position()) else {
            return;
        };
        self.leave_listeners
            .each(|listener| listener(&song, seconds));
    }

    fn notify_seeked(&self, seconds: f64) {
        self.seek_listeners.each(|listener| listener(seconds));
    }

    fn notify(&self) {
        self.listeners.each(|listener| listener());
    }

    fn set_state(&self, state: State) {
        self.change_state(state, false);
    }

    /// set_state; `waits_for_server` (server_gone) is known to the listeners
    /// along with the new state.
    fn change_state(&self, state: State, waits_for_server: bool) {
        self.waits_for_server.set(waits_for_server);
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
        self.run_listen_timer(self.is_playing());
        self.notify();
    }

    /// While the server cannot be reached nothing new can play: says so (the
    /// toast of the error handler, nothing stops) and wakes the attempts to
    /// connect. True then.
    fn offline(&self) -> bool {
        if !crate::reconnect::is_offline() {
            return false;
        }
        if let Some(handler) = self.error_handler.borrow().as_ref() {
            handler(&gettext("Cannot play without a connection to the server"));
        }
        crate::reconnect::wake();
        true
    }

    /// Stops, remembers the error and tells the user (a track that could not
    /// even be handed to GStreamer; a failed stream goes to stream_failed).
    fn fail(&self, message: String) {
        eprintln!("Playback error: {message}");
        let _ = self.playbin.set_state(gst::State::Null);
        self.set_state(State::Failed(message.clone()));
        if let Some(handler) = self.error_handler.borrow().as_ref() {
            handler(&message);
        }
        // A stream may fail because the server went away: the main view asks it.
        crate::reconnect::request_failed();
    }

    /// A stream failed: the server went away, or the track cannot be played.
    /// Playback stops where it was, and Play goes on from there. Whether the
    /// server is still there decides what the user is told: unless Vumi
    /// already knows it is not, the server is asked (server_gone, or
    /// track_failed). `same_track`: the error is the current track's own, not
    /// that of a track handed to GStreamer ahead (gapless), which never began.
    /// `before_jump`: where the track was before a jump that failed.
    fn stream_failed(&self, same_track: bool, before_jump: Option<f64>) {
        let at = if same_track {
            before_jump.or_else(|| self.position_secs())
        } else {
            None
        };
        let _ = self.playbin.set_state(gst::State::Null);
        // A seek still waiting (resuming) is where the track was meant to be.
        let resume = self
            .pending_seek
            .get()
            .or(at)
            .filter(|seconds| *seconds >= 1.0);
        self.pending_seek.set(resume);
        if crate::reconnect::is_offline() {
            self.server_gone();
            return;
        }
        let Some(client) = self.client.borrow().clone() else {
            self.track_failed();
            return;
        };
        // The main view asks too, at the same time: a server that hangs
        // keeps both waiting for as long as one.
        crate::reconnect::request_failed();
        self.checking.set(true);
        self.set_state(State::Failed(gettext("Could not play the track")));
        let command = self.command.get();
        gst::glib::spawn_future_local(async move {
            // On a new connection, as the main view asks (check_connection).
            let answer = crate::runtime()
                .spawn(async move { client.ping_fresh().await })
                .await;
            let player = get();
            player.checking.set(false);
            // Something was started or stopped meanwhile: that counts now.
            if !player.is_current(command) || player.failure().is_none() {
                player.notify();
                return;
            }
            match answer {
                Ok(Err(subsonic::Error::Unreachable(_))) => player.server_gone(),
                _ => player.track_failed(),
            }
        });
    }

    /// The server cannot be reached: the user is told that playing waits for
    /// the connection, and the main view shows the offline state. A track the
    /// user started that never played leaves the queue and the place in it as
    /// they were before.
    fn server_gone(&self) {
        let message = gettext("Cannot play without a connection to the server");
        let mut state = State::Failed(message.clone());
        if let Some(undo) = self.undo.take() {
            self.queue.replace(undo.queue);
            self.queue_album.replace(undo.queue_album);
            self.position.set(undo.position);
            self.pending_seek.set(undo.resume);
            self.prepare_next();
            // Nothing had played yet, or the queue had played to its end: so
            // it stays (Play starts it from the first track).
            if matches!(undo.state, State::Idle | State::Ended) {
                state = undo.state;
            }
        }
        let waits = matches!(state, State::Failed(_));
        self.change_state(state, waits);
        if let Some(handler) = self.error_handler.borrow().as_ref() {
            handler(&message);
        }
        crate::reconnect::server_lost();
    }

    /// The server is there, the track is not playable (a broken or missing
    /// file, a format GStreamer cannot read).
    fn track_failed(&self) {
        // The queue the user started stays, to go on with the next track.
        self.undo.take();
        let message = gettext("Could not play the track");
        self.set_state(State::Failed(message.clone()));
        if let Some(handler) = self.error_handler.borrow().as_ref() {
            handler(&message);
        }
    }

    /// The user starts another track: the queue and the place in it are kept
    /// until that track plays, unless an earlier start still waits to.
    fn keep_undo(&self) {
        if self.undo.borrow().is_some() {
            return;
        }
        let undo = Undo {
            queue: self.queue.borrow().clone(),
            queue_album: self.queue_album.borrow().clone(),
            position: self.position.get(),
            resume: self.resume_position(),
            state: self.state.borrow().clone(),
        };
        self.undo.replace(Some(undo));
    }

    /// True from the user's start of a track until it plays: its stream may
    /// still fail, and the queue before it come back.
    pub(crate) fn is_starting(&self) -> bool {
        self.undo.borrow().is_some()
    }

    /// True when playback stopped because the server could not be reached:
    /// once it is back, Play goes on (nothing else is wrong).
    pub(crate) fn waits_for_server(&self) -> bool {
        self.waits_for_server.get()
    }

    /// True while a failed stream waits for the server's answer: the error
    /// the user will be told is not known yet.
    pub(crate) fn is_checking(&self) -> bool {
        self.checking.get()
    }

    /// A command that replaces or edits the queue, or picks a track by hand:
    /// an answer to an older request (a radio asked for before it) is dropped
    /// when it arrives. Automatic changes (gapless, the next track after one
    /// ends) and a pause do not count, so a radio asked for near the end of a
    /// track still plays.
    fn new_command(&self) {
        self.command.set(self.command.get() + 1);
    }

    /// Replaces the queue with `songs`, started as `of`, and starts playing
    /// the one at `start`.
    pub(crate) fn play(
        &self,
        client: subsonic::Client,
        songs: Rc<[subsonic::Song]>,
        start: usize,
        of: QueueOf,
    ) {
        self.start_queue(client, songs, start, of);
    }

    /// play(); false if nothing started (offline).
    fn start_queue(
        &self,
        client: subsonic::Client,
        songs: Rc<[subsonic::Song]>,
        start: usize,
        of: QueueOf,
    ) -> bool {
        if self.offline() {
            return false;
        }
        self.keep_undo();
        self.leave();
        self.new_command();
        self.client.replace(Some(client));
        self.queue.replace(songs);
        self.queue_album.replace(match of {
            QueueOf::Album(id) => Some(id),
            QueueOf::Songs => None,
        });
        self.play_at(start);
        true
    }

    /// True if the queue was started as the album of `song` (QueueOf::Album):
    /// only then does the song count for the history of played albums.
    pub(crate) fn is_album_song(&self, song: &subsonic::Song) -> bool {
        let album = self.queue_album.borrow();
        album.is_some() && song.album_id.as_deref() == album.as_deref()
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
        of: QueueOf,
    ) {
        if self.start_queue(client, songs, start, of) {
            self.seek_when_ready(Some(seconds));
        }
    }

    /// The track just loaded starts at `seconds` once it is ready.
    fn seek_when_ready(&self, seconds: Option<f64>) {
        if let Some(seconds) = seconds.filter(|seconds| *seconds > 0.0) {
            self.pending_seek.set(Some(seconds));
            // The load has told the listeners about the track at its start;
            // now they learn the second it is going to.
            self.notify();
        }
    }

    fn play_at(&self, index: usize) {
        self.load(index, true);
    }

    /// Makes the track at `index` the current one and plays it; with `play`
    /// false it is loaded but stays paused at its start.
    fn load(&self, index: usize, play: bool) {
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
        // Nothing can be fetched while offline: the track waits for the
        // server, and Play starts it once it is back. Only steps that come by
        // themselves get here (the end of a track, a removed one); commands by
        // hand stop earlier.
        if crate::reconnect::is_offline() {
            let _ = self.playbin.set_state(gst::State::Null);
            self.server_gone();
            return;
        }
        let Ok(url) = client.stream_url(&song.id) else {
            self.fail(gettext("Could not play the track"));
            return;
        };
        let _ = self.playbin.set_state(gst::State::Null);
        self.apply_replay_gain();
        self.apply_volume();
        self.playbin.set_property("uri", url.as_str());
        let target = if play {
            gst::State::Playing
        } else {
            gst::State::Paused
        };
        match self.playbin.set_state(target) {
            Ok(_) => {
                self.set_state(if play { State::Playing } else { State::Paused });
                self.begin_listen();
                if !play {
                    // begin_listen told the server the track is starting.
                    self.report("paused", Some(0));
                }
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

    /// A click on a track (the queue, search results, an album page): the
    /// track in progress pauses or resumes and true comes back; any other
    /// track is the caller's to start.
    pub(crate) fn toggle_if_current(&self, clicked: Clicked) -> bool {
        let current = self.current();
        let id = current.as_ref().map(|song| song.id.as_str());
        let is_current = is_current(self.is_active(), self.position.get(), id, &clicked);
        if is_current {
            self.toggle_pause();
        }
        is_current
    }

    /// Play/Pause: pauses or resumes; after the end of the queue it starts again,
    /// after an error it tries the same track once more, from where it stopped.
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
            State::Ended | State::Failed(_) if self.offline() => {}
            State::Ended => {
                self.new_command();
                self.keep_undo();
                self.play_at(0);
            }
            State::Failed(_) => {
                self.new_command();
                self.keep_undo();
                let resume = self.pending_seek.get();
                self.play_at(self.position.get());
                self.seek_when_ready(resume);
            }
            State::Idle => {}
        }
    }

    pub(crate) fn next(&self) {
        if self.offline() {
            return;
        }
        self.new_command();
        // By hand: on to the next track even when one track repeats; past the
        // end only when the whole queue does.
        let next = self.position.get() + 1;
        let len = self.queue.borrow().len();
        if next < len {
            self.keep_undo();
            self.play_at(next);
        } else if repeat_mode() == "queue" && len > 0 {
            self.keep_undo();
            self.play_at(0);
        }
    }

    /// Back to the start of the track, or to the previous track if we are at its start.
    pub(crate) fn previous(&self) {
        // Offline even the start of the track needs the server again.
        if self.offline() {
            return;
        }
        self.new_command();
        let position = self.position.get();
        if self.position_secs().unwrap_or(0.0) > 3.0 || position == 0 {
            self.seek(0.0);
        } else {
            self.keep_undo();
            self.play_at(position - 1);
        }
    }

    /// A jump by hand (the slider, MPRIS). Offline it would need the server:
    /// the stream comes again from the new place, and what was downloaded
    /// goes. So the track plays on from where it is, the slider goes back
    /// there, and the user is told.
    pub(crate) fn seek(&self, seconds: f64) {
        if self.offline() {
            if let Some(at) = self.resume_position() {
                self.notify_seeked(at);
            }
            return;
        }
        if self.is_active() {
            self.jumped_from.set(self.position_secs());
        }
        self.jump(seconds);
    }

    /// Moves the track in progress to `seconds`.
    fn jump(&self, seconds: f64) {
        // A position no ClockTime can hold (a client may send anything over
        // MPRIS) is not a place in any track.
        let Ok(target) = gst::ClockTime::try_from_seconds_f64(seconds.max(0.0)) else {
            return;
        };
        let sought = self
            .playbin
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, target);
        if sought.is_ok() {
            let state = if self.is_playing() {
                "playing"
            } else {
                "paused"
            };
            self.report(state, Some((seconds.max(0.0) * 1000.0) as u64));
            self.notify_seeked(seconds.max(0.0));
        }
    }

    pub(crate) fn position_secs(&self) -> Option<f64> {
        self.playbin
            .query_position::<gst::ClockTime>()
            .map(|t| t.mseconds() as f64 / 1000.0)
    }

    /// Where the current track is, to resume it later: a seek that waits for
    /// the track to be ready counts as done.
    pub(crate) fn resume_position(&self) -> Option<f64> {
        self.pending_seek.get().or_else(|| self.position_secs())
    }

    fn on_message(&self, message: &gst::Message) {
        match message.view() {
            gst::MessageView::Eos(..) => self.play_at(self.following(self.position.get())),
            // A track has begun: if about-to-finish queued it, this is the gapless
            // step to the next track in the queue.
            gst::MessageView::StreamStart(..) => {
                let begun = self.switched_to.lock().ok().and_then(|mut s| s.take());
                if let Some(id) = begun {
                    self.advance_gapless(&id);
                }
            }
            // The new track is ready: a pending seek (resuming) takes effect now.
            // A track the user started has begun: the queue before it is not
            // needed any more, and the listeners hear of it (the history of
            // played albums takes the album now, still with the second the
            // seek is going to).
            gst::MessageView::AsyncDone(..) => {
                // A jump by hand has landed too.
                self.jumped_from.set(None);
                if self.undo.take().is_some() {
                    self.notify();
                }
                if let Some(seconds) = self.pending_seek.take() {
                    self.jump(seconds);
                }
            }
            // Only the short error text, and only in the log: the debug details
            // may contain the stream URL, and with it the login token. The user
            // is told in words of Vumi's own (stream_failed): GStreamer's
            // ("Internal data stream error") say nothing to them.
            gst::MessageView::Error(err) => {
                eprintln!("Playback error: {}", err.error());
                // A jump that failed is the current track's own error, even if
                // GStreamer, its stream ended, has taken the next track meanwhile.
                let before_jump = self.jumped_from.take();
                let handed_over = if before_jump.is_some() {
                    self.forget_switch();
                    false
                } else {
                    self.blame_pending_switch()
                };
                self.stream_failed(!handed_over, before_jump);
            }
            _ => {}
        }
    }
}

/// Plays `songs` from the first, or in a random order (`shuffle`), as `of`
/// (the Play and Shuffle buttons of albums, playlists and Favorites).
pub(crate) fn play_list(
    client: subsonic::Client,
    songs: Rc<[subsonic::Song]>,
    shuffle: bool,
    of: QueueOf,
) {
    if songs.is_empty() {
        return;
    }
    let songs = if shuffle {
        // A new order needs its own copy, made only when Shuffle is pressed.
        let mut shuffled = songs.to_vec();
        fastrand::shuffle(&mut shuffled);
        shuffled.into()
    } else {
        songs
    };
    get().play(client, songs, 0, of);
}

/// Stops playback cleanly when the app quits: GStreamer wants its elements in
/// the NULL state before they are destroyed. Does nothing if nothing was played.
pub(crate) fn shutdown() {
    PLAYER.with(|cell| {
        if let Some(player) = cell.get() {
            player.save_volume_now();
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
    /// listen timer counts from zero towards a finished listen.
    fn begin_listen(&self) {
        let Some(song) = self.current() else {
            return;
        };
        let started = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_millis() as u64)
            .unwrap_or(0);
        let previous = self.listen.replace(Some(Listen {
            id: song.id.clone(),
            started,
            played: 0,
            submitted: false,
        }));
        // The same track again (Repeat Track, twice in the queue, Play after
        // an error): nothing else tells an MPRIS client that the position is
        // back at the start.
        if previous.is_some_and(|listen| listen.id == song.id) {
            self.notify_seeked(0.0);
        }
        if self.report_support.get() == Some(true) {
            self.report("starting", Some(0));
        } else {
            self.scrobble(song.id.clone(), false, None);
        }
        if self.report_support.get().is_none() {
            self.check_report_support();
        }
    }

    /// Starts or stops the listen timer: paused or stopped, there is no
    /// time played to count.
    fn run_listen_timer(&self, run: bool) {
        let mut timer = self.listen_timer.borrow_mut();
        if run && timer.is_none() {
            *timer = Some(gst::glib::timeout_add_seconds_local(1, || {
                get().tick_listen();
                gst::glib::ControlFlow::Continue
            }));
        } else if !run && let Some(timer) = timer.take() {
            timer.remove();
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
            let result = crate::runtime()
                .spawn(async move { client.extensions().await })
                .await;
            let supported =
                matches!(&result, Ok(Ok(names)) if names.iter().any(|n| n == "playbackReport"));
            let player = get();
            // Logged out meanwhile: the answer is about another server.
            if supported && player.report_support.get() == Some(false) {
                player.report_support.set(Some(true));
                let state = if player.is_playing() {
                    "playing"
                } else {
                    "paused"
                };
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

    /// ReplayGain from Preferences. playbin takes a new audio filter only while
    /// stopped, so this runs as a track starts. Off means no element at all,
    /// so no gain is added.
    fn apply_replay_gain(&self) {
        let mode = replay_gain();
        if self.replay_gain_mode.get() == Some(mode) {
            return;
        }
        let filter: Option<gst::Element> = match mode {
            0 => None,
            _ => gst::parse::bin_from_description(
                "audioconvert ! rgvolume name=rg ! rglimiter ! audioconvert",
                true,
            )
            .map_err(|e| eprintln!("ReplayGain: {e}"))
            .ok()
            .map(|bin| {
                if let Some(rg) = bin.by_name("rg") {
                    rg.set_property("album-mode", mode == 2);
                }
                bin.upcast()
            }),
        };
        self.playbin.set_property("audio-filter", filter.as_ref());
        self.replay_gain_mode.set(Some(mode));
    }

    pub(crate) fn volume(&self) -> f64 {
        self.volume.get()
    }

    /// From the slider, the scroll wheel or MPRIS; remembered for next time,
    /// once it stays put.
    pub(crate) fn set_volume(&self, volume: f64) {
        let volume = volume.clamp(0.0, 1.0);
        if (self.volume.get() - volume).abs() < 1e-4 {
            return;
        }
        self.volume.set(volume);
        self.apply_volume();
        self.notify_volume(volume);
        if let Some(source) = self.volume_save.take() {
            source.remove();
        }
        let source = gst::glib::timeout_add_local_once(VOLUME_SAVE_DELAY, || {
            let player = get();
            // This source is ending: forget it rather than remove it.
            player.volume_save.take();
            write_volume(player.volume.get());
        });
        self.volume_save.replace(Some(source));
    }

    /// Writes the volume now if it still waits (the app is closing).
    pub(crate) fn save_volume_now(&self) {
        if let Some(source) = self.volume_save.take() {
            source.remove();
            write_volume(self.volume.get());
        }
    }

    /// Registers `f`, told every new volume.
    pub(crate) fn connect_volume_changed(&self, f: impl Fn(f64) + 'static) {
        self.volume_listeners.add(Rc::new(f));
    }

    fn notify_volume(&self, volume: f64) {
        self.volume_listeners.each(|listener| listener(volume));
    }

    /// The slider is perceptual, GStreamer's volume linear: the cube makes half
    /// the slider sound like half as loud. 1 stays exactly 1, so at full volume
    /// the signal is unchanged.
    fn apply_volume(&self) {
        self.playbin
            .set_property("volume", self.volume.get().powi(3));
    }
}

fn write_volume(volume: f64) {
    use gtk::gio::prelude::*;
    let _ = crate::settings().set_double("volume", volume);
}

/// The track a click lands on: a place in the queue (a song can be there
/// twice), or a song elsewhere.
pub(crate) enum Clicked<'a> {
    Place(usize),
    Song(&'a str),
}

/// What a click on the song at `index` of `songs` names: its place when the
/// list is the queue itself (`queue`), so a second copy of a song is told
/// apart from the first; in any other list the song.
pub(crate) fn clicked<'a>(
    queue: &Rc<[subsonic::Song]>,
    songs: &'a Rc<[subsonic::Song]>,
    index: usize,
) -> Option<Clicked<'a>> {
    let song = songs.get(index)?;
    Some(if Rc::ptr_eq(queue, songs) {
        Clicked::Place(index)
    } else {
        Clicked::Song(&song.id)
    })
}

/// Whether the clicked track is the one in progress (playing or paused) at
/// `position`, with the id `current`. After the end or an error none is.
fn is_current(active: bool, position: usize, current: Option<&str>, clicked: &Clicked) -> bool {
    active
        && match clicked {
            Clicked::Place(index) => *index == position,
            Clicked::Song(id) => current == Some(*id),
        }
}

/// What plays on its own after the track at `index` of a queue of `len`
/// tracks with Repeat at `repeat`: the next, the first again ("queue"), the
/// same ("track"). `len` means the end.
fn following(index: usize, len: usize, repeat: &str) -> usize {
    match repeat {
        "track" => index,
        "queue" if len > 0 => (index + 1) % len,
        _ => index + 1,
    }
}

/// Whether a track follows the one at `position` (MPRIS: CanGoNext); with
/// Repeat at "queue" the first one does.
fn has_next(position: usize, len: usize, repeat: &str) -> bool {
    position + 1 < len || (repeat == "queue" && len > 0)
}

/// Where the track playing at `position` ends up when the track at `from` is
/// dragged to `to`: it keeps playing, wherever that is.
fn moved_position(position: usize, from: usize, to: usize) -> usize {
    if position == from {
        to
    } else if from < position && to >= position {
        position - 1
    } else if from > position && to <= position {
        position + 1
    } else {
        position
    }
}

/// Where Play Next puts a song: right after the playing track at `position`,
/// or, once the next track (`pending`, its id) is with GStreamer and plays no
/// matter what, right after that one, wherever the queue has it by now.
fn play_next_index(songs: &[subsonic::Song], position: usize, pending: Option<&str>) -> usize {
    match pending {
        Some(id) => songs
            .iter()
            .skip(position + 1)
            .position(|song| song.id == id)
            .map(|offset| position + offset + 2)
            .unwrap_or(position + 1),
        None => position + 1,
    }
    .min(songs.len())
}

/// Where Shuffle Queue starts: after the playing track, and after the next
/// one too if it is already with GStreamer (`pending`).
fn shuffle_start(songs: &[subsonic::Song], position: usize, pending: Option<&str>) -> usize {
    let from = (position + 1).min(songs.len());
    match pending {
        Some(id) if songs.get(from).is_some_and(|song| song.id == id) => from + 1,
        _ => from,
    }
}

/// The volume left last time.
fn saved_volume() -> f64 {
    use gtk::gio::prelude::*;
    crate::settings().double("volume")
}

/// "Volume Normalization" in Preferences: 0 off, 1 per track, 2 per album.
fn replay_gain() -> u32 {
    use gtk::gio::prelude::*;
    crate::settings().uint("replay-gain")
}

/// "Share Listening Activity" in Preferences.
fn report_listens() -> bool {
    use gtk::gio::prelude::*;
    crate::settings().boolean("report-listens")
}

impl Player {
    /// What plays on its own after the track at `index`: the next, the first
    /// again when the queue repeats, the same when the track repeats. The
    /// length of the queue means the end.
    fn following(&self, index: usize) -> usize {
        following(index, self.queue.borrow().len(), &repeat_mode())
    }

    /// Repeat changed: the track handed to GStreamer ahead follows it.
    pub(crate) fn repeat_changed(&self) {
        self.prepare_next();
        self.notify();
    }
}

/// "Repeat" from the main menu: none, queue or track.
fn repeat_mode() -> String {
    use gtk::gio::prelude::*;
    crate::settings().string("repeat").to_string()
}

/// "Endless Radio" from the main menu.
fn endless_radio() -> bool {
    use gtk::gio::prelude::*;
    crate::settings().boolean("endless-radio")
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
                Ok(Err(e)) => {
                    // Not a playback error: the queue ends as when nothing is
                    // found, but the window says why.
                    eprintln!("Could not extend the radio: {e}");
                    if let Some(handler) = player.error_handler.borrow().as_ref() {
                        handler(&crate::error_message(&e));
                    }
                    Vec::new()
                }
                Err(_) => Vec::new(),
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

    /// True if there is a track after the current one (MPRIS: CanGoNext).
    pub(crate) fn has_next(&self) -> bool {
        has_next(
            self.position.get(),
            self.queue.borrow().len(),
            &repeat_mode(),
        )
    }

    /// Stop (MPRIS): like reaching the end of the queue; Play starts it again.
    pub(crate) fn stop(&self) {
        if self.current().is_some() {
            self.leave();
            self.new_command();
            self.undo.take();
            let _ = self.playbin.set_state(gst::State::Null);
            self.set_state(State::Ended);
        }
    }

    /// Gapless: the address of the track after the current one, ready for
    /// about-to-finish. None at the end of the queue (then the queue simply ends,
    /// or Endless Radio adds more).
    fn prepare_next(&self) {
        let next = self.following(self.position.get());
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
        // Only the song that follows the current one counts: the next (so A, A, B
        // moves on to the second A), the first when the queue repeats, the same
        // one when the track repeats. Queue edits keep the handed-over track
        // there, except moving or removing it after about-to-finish, or changing
        // Repeat: then the queue wins and that song starts now, with a short gap.
        let expected = self.following(from);
        match queue.get(expected) {
            Some(song) if song.id == id => {
                self.position.set(expected);
                self.set_state(State::Playing);
                self.begin_listen();
                self.prepare_next();
            }
            _ => self.play_at(expected),
        }
    }

    /// Gapless: the id of the track about-to-finish has already handed to
    /// GStreamer, which will play whatever the queue says now.
    fn pending_switch(&self) -> Option<String> {
        self.switched_to.lock().ok().and_then(|s| s.clone())
    }

    /// Gapless: an error while a track handed to GStreamer has not begun yet
    /// is taken for that track's. It becomes the current one, so that Play
    /// tries it again rather than the track before it. True if there was one.
    fn blame_pending_switch(&self) -> bool {
        let Some(id) = self.switched_to.lock().ok().and_then(|mut s| s.take()) else {
            return false;
        };
        let expected = self.following(self.position.get());
        let handed_over = self
            .queue
            .borrow()
            .get(expected)
            .is_some_and(|song| song.id == id);
        if handed_over {
            self.position.set(expected);
        }
        true
    }

    /// A switch queued for the previous track must not move the queue on.
    fn forget_switch(&self) {
        if let Ok(mut switched) = self.switched_to.lock() {
            *switched = None;
        }
    }

    /// The play queue (shared, not copied) and where in it playback is.
    pub(crate) fn queue(&self) -> Rc<[subsonic::Song]> {
        self.queue.borrow().clone()
    }

    pub(crate) fn position(&self) -> usize {
        self.position.get()
    }

    /// Jumps to another track of the current queue (the queue panel).
    pub(crate) fn play_index(&self, index: usize) {
        if self.offline() {
            return;
        }
        self.new_command();
        if index < self.queue.borrow().len() {
            self.keep_undo();
            self.play_at(index);
        }
    }

    /// Removes the track at `index` from the queue (the queue panel). Removing
    /// the current track moves on to the next one, or ends the queue; the next
    /// one plays only if the removed one did, after a pause or Stop it waits.
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
            let waits = matches!(*self.state.borrow(), State::Paused | State::Ended);
            // The next track now sits at this index; past the end, the queue ends.
            self.load(position, !waits);
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
        self.queue.replace(songs.into());
        self.position
            .set(moved_position(self.position.get(), from, to));
        self.prepare_next();
        self.notify();
    }

    /// Puts `song` right after the playing track (Play Next). With nothing in
    /// progress, it simply plays; after an error the queue is still there, so
    /// the song joins it and nothing starts. False if nothing happened
    /// (offline: the user is told).
    pub(crate) fn play_next(&self, client: subsonic::Client, song: subsonic::Song) -> bool {
        if self.offline() {
            return false;
        }
        self.new_command();
        if !self.is_active() && self.failure().is_none() {
            return self.start_queue(client, vec![song].into(), 0, QueueOf::Songs);
        }
        let mut songs = self.queue.borrow().to_vec();
        let index = play_next_index(
            &songs,
            self.position.get(),
            self.pending_switch().as_deref(),
        );
        songs.insert(index, song);
        self.queue.replace(songs.into());
        self.prepare_next();
        self.notify();
        true
    }

    /// Puts what comes after the current track into a random order (Shuffle
    /// Queue). The track itself plays on; one already handed to GStreamer for
    /// a gapless start stays next, as it will play whatever the queue says.
    pub(crate) fn shuffle_rest(&self) {
        let mut songs = self.queue.borrow().to_vec();
        let from = shuffle_start(
            &songs,
            self.position.get(),
            self.pending_switch().as_deref(),
        );
        if songs.len().saturating_sub(from) < 2 {
            return;
        }
        self.new_command();
        fastrand::shuffle(&mut songs[from..]);
        self.queue.replace(songs.into());
        self.prepare_next();
        self.notify();
    }

    /// Empties the queue but for the current track, which plays on (Clear
    /// Queue).
    pub(crate) fn clear_rest(&self) {
        let Some(current) = self.current() else {
            return;
        };
        if self.queue.borrow().len() < 2 {
            return;
        }
        self.new_command();
        self.queue.replace(vec![current].into());
        self.position.set(0);
        self.prepare_next();
        self.notify();
    }

    /// Adds `song` at the end of the queue (Add to Queue). With nothing in
    /// progress, it simply plays; after an error the queue is still there, so
    /// the song joins it and nothing starts. False if nothing happened (it
    /// would have played, offline: the user is told).
    pub(crate) fn enqueue(&self, client: subsonic::Client, song: subsonic::Song) -> bool {
        self.new_command();
        if !self.is_active() && self.failure().is_none() {
            return self.start_queue(client, vec![song].into(), 0, QueueOf::Songs);
        }
        let mut songs = self.queue.borrow().to_vec();
        songs.push(song);
        self.queue.replace(songs.into());
        self.prepare_next();
        self.notify();
        true
    }

    /// Forgets everything (logging out): the queue, the client with its
    /// credentials, the prepared next track. Play from MPRIS can then not
    /// start the old account's music again.
    pub(crate) fn reset(&self) {
        self.new_command();
        let _ = self.playbin.set_state(gst::State::Null);
        self.pending_seek.set(None);
        self.undo.take();
        self.forget_switch();
        if let Ok(mut next_uri) = self.next_uri.lock() {
            *next_uri = None;
        }
        self.queue.replace(Vec::new().into());
        self.queue_album.replace(None);
        self.position.set(0);
        self.client.replace(None);
        self.report_support.set(None);
        self.set_state(State::Idle);
    }

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

#[cfg(test)]
impl Player {
    /// Tests: the sound goes nowhere, at the pace of a real sink.
    pub(crate) fn use_fake_sink(&self) {
        let sink = gst::ElementFactory::make("fakesink")
            .property("sync", true)
            .build()
            .expect("GStreamer fakesink is missing");
        self.playbin.set_property("audio-sink", &sink);
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    #[test]
    fn a_click_on_the_track_in_progress_pauses_or_resumes_it() {
        // In the queue the place counts: the second copy of a song starts.
        assert!(is_current(true, 2, Some("a"), &Clicked::Place(2)));
        assert!(!is_current(true, 2, Some("a"), &Clicked::Place(4)));
        // Elsewhere the song counts.
        assert!(is_current(true, 2, Some("a"), &Clicked::Song("a")));
        assert!(!is_current(true, 2, Some("a"), &Clicked::Song("b")));
        // After the end of the queue or an error a click starts it again.
        assert!(!is_current(false, 2, Some("a"), &Clicked::Place(2)));
        assert!(!is_current(false, 2, Some("a"), &Clicked::Song("a")));
        assert!(!is_current(true, 0, None, &Clicked::Song("a")));
    }

    #[test]
    fn a_click_in_the_queue_itself_names_the_place() {
        // A list that is the queue: a song in it twice is told apart, so a
        // click on the second copy starts it instead of pausing the first.
        let queue: Rc<[subsonic::Song]> = songs(&["a", "b", "a"]).into();
        let second_copy = clicked(&queue, &queue, 2).unwrap();
        assert!(matches!(second_copy, Clicked::Place(2)));
        assert!(!is_current(true, 0, Some("a"), &second_copy));
        // Any other list, even with the same songs: the song counts.
        let list: Rc<[subsonic::Song]> = songs(&["a", "b", "a"]).into();
        assert!(matches!(
            clicked(&queue, &list, 2),
            Some(Clicked::Song("a"))
        ));
        assert!(clicked(&queue, &list, 3).is_none());
    }

    #[test]
    fn a_listener_may_add_and_remove_listeners() {
        // A change of the player can close a page, which then removes its
        // listener while the listeners are being called.
        let listeners = Rc::new(Listeners::<dyn Fn()>::new());
        let calls = Rc::new(Cell::new(0));
        let first = {
            let listeners = Rc::downgrade(&listeners);
            let calls = calls.clone();
            Rc::new(move || {
                calls.set(calls.get() + 1);
                let Some(listeners) = listeners.upgrade() else {
                    return;
                };
                listeners.remove(0);
                let calls = calls.clone();
                listeners.add(Rc::new(move || calls.set(calls.get() + 10)));
            })
        };
        assert_eq!(listeners.add(first), 0);
        listeners.each(|listener| listener());
        assert_eq!(calls.get(), 1);
        listeners.each(|listener| listener());
        assert_eq!(calls.get(), 11);
    }

    /// With a real playbin (playbin_tests.rs runs it): a stream that fails
    /// is never shown in GStreamer's words. The server gone: playing waits
    /// for the connection, and once it is back Play goes on where the track
    /// stopped. The server there, the track not sound: it cannot be played.
    pub(crate) fn a_failed_stream_says_why_and_play_goes_on_from_there(failures: &mut Vec<String>) {
        use crate::playbin_tests::{Server, run_until, silent_wav};
        use std::time::Duration;
        let server = Server::start(silent_wav(60));
        let client = subsonic::Client::new(&server.url, "user", "password");
        let player = get();
        let told: Rc<RefCell<Vec<String>>> = Rc::default();
        {
            let told = told.clone();
            player.set_error_handler(move |message| told.borrow_mut().push(message.to_string()));
        }
        let offline = gettext("Cannot play without a connection to the server");
        let cannot = gettext("Could not play the track");
        let long = Duration::from_secs(15);

        // The server goes three seconds into a track.
        server.set_paced(true);
        player.play(
            client.clone(),
            songs(&["a1", "a2"]).into(),
            0,
            QueueOf::Songs,
        );
        if !run_until(long, || {
            player.is_playing() && player.position_secs().unwrap_or(0.0) >= 3.0
        }) {
            failures.push("a failed stream: a1 did not play".into());
            return;
        }
        server.set_up(false);
        let said = run_until(Duration::from_secs(40), || !told.borrow().is_empty());
        let resume = player.resume_position().unwrap_or(0.0);
        if !said
            || *told.borrow() != [offline.clone()]
            || player.failure().as_deref() != Some(offline.as_str())
            || player.current().map(|song| song.id).as_deref() != Some("a1")
            || resume < 2.5
        {
            failures.push(format!(
                "the server gone: told {:?}, failure {:?}, current {:?}, resumes at {resume:.1} s",
                told.borrow(),
                player.failure(),
                player.current().map(|song| song.id),
            ));
        }

        // Back: Play goes on from there, not from the start.
        server.set_up(true);
        player.toggle_pause();
        let playing = run_until(long, || {
            player.is_playing() && player.position_secs().unwrap_or(0.0) > 0.0
        });
        let at = player.position_secs().unwrap_or(0.0);
        if !playing
            || player.current().map(|song| song.id).as_deref() != Some("a1")
            || at < resume - 1.0
        {
            failures.push(format!(
                "Play after the server came back: playing {playing}, at {at:.1} s of {:?}, resume was {resume:.1} s",
                player.current().map(|song| song.id)
            ));
        }

        // The server is there, the track is text: it cannot be played, and
        // the queue stays as started, to go on with the next track.
        server.set_paced(false);
        server.set_not_audio(true);
        told.borrow_mut().clear();
        player.play(
            client.clone(),
            songs(&["b1", "b2"]).into(),
            0,
            QueueOf::Songs,
        );
        let said = run_until(long, || !told.borrow().is_empty());
        if !said
            || *told.borrow() != [cannot.clone()]
            || player.failure().as_deref() != Some(cannot.as_str())
            || player.current().map(|song| song.id).as_deref() != Some("b1")
        {
            failures.push(format!(
                "a track that is not sound: told {:?}, failure {:?}, current {:?}",
                told.borrow(),
                player.failure(),
                player.current().map(|song| song.id),
            ));
        }
        server.set_not_audio(false);
        player.stop();
    }

    /// With a real playbin (playbin_tests.rs runs it): offline, a click on a
    /// track, Resume, Next, Previous or a click in the queue only says that
    /// playing needs the connection, and the queue and the track in progress
    /// stay as they were. So too when Vumi does not know yet that the server
    /// is gone: the clicked track fails, the server is asked, and the queue
    /// and the place in it come back; once the server is there, Play goes on
    /// from that place. When the server goes while a track plays, the next one
    /// (handed to GStreamer ahead, for no gap) waits.
    pub(crate) fn offline_leaves_the_queue_and_the_track_as_they_were(failures: &mut Vec<String>) {
        use crate::playbin_tests::{Server, run_until, silent_wav};
        use std::time::Duration;
        let server = Server::start(silent_wav(60));
        let client = subsonic::Client::new(&server.url, "user", "password");
        let player = get();
        let told: Rc<RefCell<Vec<String>>> = Rc::default();
        {
            let told = told.clone();
            player.set_error_handler(move |message| told.borrow_mut().push(message.to_string()));
        }
        let offline = gettext("Cannot play without a connection to the server");
        let long = Duration::from_secs(15);
        let a: Rc<[subsonic::Song]> = songs(&["a1", "a2", "a3"]).into();
        let b: Rc<[subsonic::Song]> = songs(&["b1", "b2"]).into();
        player.play(client.clone(), a.clone(), 1, QueueOf::Album("A".into()));
        if !run_until(long, || {
            player.is_playing() && player.position_secs().unwrap_or(0.0) >= 4.0
        }) {
            failures.push("offline: a2 did not play".into());
            return;
        }
        let as_it_was = || Rc::ptr_eq(&player.queue(), &a) && player.position() == 1;

        // Offline, known.
        crate::reconnect::set_offline(true);
        type Command = Box<dyn Fn()>;
        let commands: Vec<(&str, Command)> = vec![
            ("a click on a track", {
                let (client, b) = (client.clone(), b.clone());
                Box::new(move || {
                    get().play(client.clone(), b.clone(), 0, QueueOf::Album("B".into()))
                })
            }),
            ("Resume", {
                let (client, b) = (client.clone(), b.clone());
                Box::new(move || {
                    get().play_from(
                        client.clone(),
                        b.clone(),
                        1,
                        20.0,
                        QueueOf::Album("B".into()),
                    )
                })
            }),
            ("Next", Box::new(|| get().next())),
            ("Previous", Box::new(|| get().previous())),
            ("a click in the queue", Box::new(|| get().play_index(2))),
        ];
        for (label, command) in commands {
            told.borrow_mut().clear();
            command();
            run_until(Duration::from_millis(500), || false);
            let at = player.resume_position().unwrap_or(0.0);
            if !as_it_was()
                || !player.is_playing()
                || *told.borrow() != [offline.clone()]
                || !(3.0..15.0).contains(&at)
            {
                failures.push(format!(
                    "offline, {label}: queue as it was {}, playing {}, at {at:.1} s, told {:?}",
                    as_it_was(),
                    player.is_playing(),
                    told.borrow()
                ));
            }
        }
        crate::reconnect::set_offline(false);

        // The server goes before Vumi knows: a click on another album's track.
        let at = player.position_secs().unwrap_or(0.0);
        server.set_up(false);
        told.borrow_mut().clear();
        player.play(client.clone(), b.clone(), 0, QueueOf::Album("B".into()));
        let said = run_until(long, || !told.borrow().is_empty());
        let resume = player.resume_position().unwrap_or(0.0);
        if !said
            || !as_it_was()
            || *told.borrow() != [offline.clone()]
            || player.failure().as_deref() != Some(offline.as_str())
            || (resume - at).abs() > 1.5
        {
            failures.push(format!(
                "a click with the server gone: queue as it was {}, current {:?}, told {:?}, failure {:?}, resumes at {resume:.1} s (was at {at:.1} s)",
                as_it_was(),
                player.current().map(|song| song.id),
                told.borrow(),
                player.failure()
            ));
        }

        // Back: Play goes on with a2 where it was.
        server.set_up(true);
        player.toggle_pause();
        let playing = run_until(long, || {
            player.is_playing() && player.position_secs().unwrap_or(0.0) > 0.0
        });
        let now = player.position_secs().unwrap_or(0.0);
        if !playing || !as_it_was() || now < resume - 1.0 {
            failures.push(format!(
                "Play after the server came back: playing {playing}, queue as it was {}, at {now:.1} s, resume was {resume:.1} s",
                as_it_was()
            ));
        }

        // The server goes while a short track plays: the next one waits.
        let short = Server::start(silent_wav(2));
        let short_client = subsonic::Client::new(&short.url, "user", "password");
        player.play(short_client, songs(&["c1", "c2"]).into(), 0, QueueOf::Songs);
        if run_until(long, || {
            player.is_playing() && player.position_secs().unwrap_or(0.0) >= 0.3
        }) {
            short.set_up(false);
            crate::reconnect::set_offline(true);
            told.borrow_mut().clear();
            let waits = run_until(long, || player.failure().is_some());
            if !waits
                || player.current().map(|song| song.id).as_deref() != Some("c2")
                || player.failure().as_deref() != Some(offline.as_str())
                || *told.borrow() != [offline.clone()]
            {
                failures.push(format!(
                    "a track ended offline: current {:?}, failure {:?}, told {:?}",
                    player.current().map(|song| song.id),
                    player.failure(),
                    told.borrow()
                ));
            }
            crate::reconnect::set_offline(false);
        } else {
            failures.push("offline: c1 did not play".into());
        }
        player.stop();
    }

    /// With a real playbin (playbin_tests.rs runs it): a jump in a track
    /// needs the server, the stream comes again from the new place. The
    /// server gone before Vumi knows: the jump fails, and the track stays
    /// where it was before it, never back at the start nor on the next track;
    /// once the server is there, Play goes on from that place. Offline, known:
    /// the track plays on from what was downloaded, and the user is told.
    pub(crate) fn a_jump_offline_leaves_the_track_where_it_was(failures: &mut Vec<String>) {
        use crate::playbin_tests::{Server, run_until, silent_wav};
        use std::time::Duration;
        let server = Server::start(silent_wav(60));
        server.set_paced(true);
        let client = subsonic::Client::new(&server.url, "user", "password");
        let player = get();
        let told: Rc<RefCell<Vec<String>>> = Rc::default();
        {
            let told = told.clone();
            player.set_error_handler(move |message| told.borrow_mut().push(message.to_string()));
        }
        let offline = gettext("Cannot play without a connection to the server");
        let long = Duration::from_secs(15);
        let current = || player.current().map(|song| song.id);
        player.play(
            client.clone(),
            songs(&["j1", "j2"]).into(),
            0,
            QueueOf::Songs,
        );
        if !run_until(long, || {
            player.is_playing() && player.position_secs().unwrap_or(0.0) >= 4.0
        }) {
            failures.push("a jump: j1 did not play".into());
            return;
        }

        // The server goes, Vumi does not know yet: a jump forward.
        let before = player.position_secs().unwrap_or(0.0);
        server.set_up(false);
        player.seek(before + 20.0);
        let said = run_until(long, || !told.borrow().is_empty());
        let resume = player.resume_position().unwrap_or(0.0);
        if !said
            || current().as_deref() != Some("j1")
            || player.failure().as_deref() != Some(offline.as_str())
            || (resume - before).abs() > 1.5
            || *told.borrow() != [offline.clone()]
        {
            failures.push(format!(
                "a jump with the server gone: current {:?}, failure {:?}, resumes at {resume:.1} s (was at {before:.1} s), told {:?}",
                current(),
                player.failure(),
                told.borrow()
            ));
        }

        // Back: Play goes on from there.
        server.set_up(true);
        player.toggle_pause();
        let playing = run_until(long, || {
            player.is_playing() && player.position_secs().unwrap_or(0.0) > 0.0
        });
        let at = player.position_secs().unwrap_or(0.0);
        if !playing || current().as_deref() != Some("j1") || at < resume - 1.0 {
            failures.push(format!(
                "Play after a failed jump: playing {playing}, {:?} at {at:.1} s, resume was {resume:.1} s",
                current()
            ));
        }

        // Offline, known: a jump only says so, and the track plays on. (The
        // server stays: without it this test server closes the stream at once,
        // where a network that went keeps it open and the track plays on from
        // what was downloaded.)
        run_until(long, || {
            player.position_secs().unwrap_or(0.0) >= resume + 2.0
        });
        crate::reconnect::set_offline(true);
        told.borrow_mut().clear();
        let before = player.position_secs().unwrap_or(0.0);
        player.seek(before + 20.0);
        run_until(Duration::from_millis(1500), || false);
        let at = player.position_secs().unwrap_or(0.0);
        if !player.is_playing()
            || current().as_deref() != Some("j1")
            || !(before..before + 3.0).contains(&at)
            || *told.borrow() != [offline.clone()]
        {
            failures.push(format!(
                "a jump offline: playing {}, {:?} at {at:.1} s (was at {before:.1} s), told {:?}",
                player.is_playing(),
                current(),
                told.borrow()
            ));
        }
        crate::reconnect::set_offline(false);
        player.stop();
    }

    fn songs(ids: &[&str]) -> Vec<subsonic::Song> {
        ids.iter()
            .map(|id| subsonic::Song {
                id: (*id).into(),
                title: (*id).into(),
                ..Default::default()
            })
            .collect()
    }

    #[test]
    fn what_follows_depends_on_repeat() {
        assert_eq!(following(1, 3, "off"), 2);
        assert_eq!(following(2, 3, "off"), 3, "the end");
        assert_eq!(following(2, 3, "queue"), 0);
        assert_eq!(following(1, 3, "track"), 1);
        assert_eq!(following(0, 0, "queue"), 1, "an empty queue ends");
        assert!(has_next(1, 3, "off"));
        assert!(!has_next(2, 3, "off"));
        assert!(has_next(2, 3, "queue"));
        assert!(!has_next(0, 0, "queue"));
    }

    #[test]
    fn the_playing_track_keeps_playing_when_tracks_are_dragged() {
        // Playing the third track (2) of five.
        assert_eq!(moved_position(2, 2, 4), 4, "the playing one moves");
        assert_eq!(moved_position(2, 0, 3), 1, "one before it goes after it");
        assert_eq!(moved_position(2, 4, 0), 3, "one after it goes before it");
        assert_eq!(moved_position(2, 3, 4), 2, "both after it");
        assert_eq!(moved_position(2, 0, 1), 2, "both before it");
        assert_eq!(
            moved_position(2, 0, 2),
            1,
            "one put in its place pushes it back"
        );
    }

    #[test]
    fn play_next_goes_after_the_track_already_with_gstreamer() {
        let queue = songs(&["a", "b", "c", "d"]);
        assert_eq!(play_next_index(&queue, 1, None), 2);
        // "c" is handed over: after it.
        assert_eq!(play_next_index(&queue, 1, Some("c")), 3);
        // Moved further meanwhile: still after it.
        assert_eq!(play_next_index(&queue, 1, Some("d")), 4);
        // Not in the queue any more: right after the playing one.
        assert_eq!(play_next_index(&queue, 1, Some("x")), 2);
        assert_eq!(play_next_index(&queue, 3, None), 4, "at the end");
        assert_eq!(play_next_index(&[], 0, None), 0);
    }

    #[test]
    fn shuffling_leaves_the_playing_and_the_handed_over_track() {
        let queue = songs(&["a", "b", "c", "d"]);
        assert_eq!(shuffle_start(&queue, 0, None), 1);
        assert_eq!(shuffle_start(&queue, 0, Some("b")), 2);
        assert_eq!(
            shuffle_start(&queue, 0, Some("d")),
            1,
            "only the next one counts"
        );
        assert_eq!(shuffle_start(&queue, 3, None), 4);
    }
}

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

    fn notify(&self) {
        // Every change the listeners hear about counts as a command.
        self.command.set(self.command.get() + 1);
        for (_, listener) in self.listeners.borrow().iter() {
            listener();
        }
    }

    fn set_state(&self, state: State) {
        self.state.replace(state);
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

    /// Replaces the queue with `songs` and starts playing the one at `start`.
    pub(crate) fn play(&self, client: subsonic::Client, songs: Rc<[subsonic::Song]>, start: usize) {
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
        let next = self.position.get() + 1;
        if next < self.queue.borrow().len() {
            self.play_at(next);
        }
    }

    /// Back to the start of the track, or to the previous track if we are at its start.
    pub(crate) fn previous(&self) {
        let position = self.position.get();
        if self.position_secs().unwrap_or(0.0) > 3.0 || position == 0 {
            self.seek(0.0);
        } else {
            self.play_at(position - 1);
        }
    }

    pub(crate) fn seek(&self, seconds: f64) {
        let target = gst::ClockTime::from_mseconds((seconds.max(0.0) * 1000.0) as u64);
        let _ = self
            .playbin
            .seek_simple(gst::SeekFlags::FLUSH | gst::SeekFlags::KEY_UNIT, target);
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
        let found = queue
            .iter()
            .skip(from)
            .position(|song| song.id == id)
            .map(|offset| offset + from)
            .or_else(|| queue.iter().position(|song| song.id == id));
        match found {
            Some(index) => {
                self.position.set(index);
                self.set_state(State::Playing);
                self.prepare_next();
            }
            // Removed from the queue meanwhile: play what is next now instead.
            None => self.play_at(from + 1),
        }
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
        if !self.is_active() {
            self.play(client, vec![song].into(), 0);
            return;
        }
        let mut songs = self.queue.borrow().to_vec();
        songs.insert(self.position.get() + 1, song);
        self.queue.replace(songs.into());
        self.prepare_next();
        self.notify();
    }

    /// Adds `song` at the end of the queue (Add to Queue). With nothing in
    /// progress, it simply plays.
    pub(crate) fn enqueue(&self, client: subsonic::Client, song: subsonic::Song) {
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
        let _ = self.playbin.set_state(gst::State::Null);
        self.pending_seek.set(None);
        self.forget_switch();
        if let Ok(mut next_uri) = self.next_uri.lock() {
            *next_uri = None;
        }
        self.queue.replace(Vec::new().into());
        self.position.set(0);
        self.client.replace(None);
        self.set_state(State::Idle);
    }
}

impl Player {
    /// The current command number; see `command`. A request to the server keeps
    /// it and checks it against `is_current` when the answer arrives.
    pub(crate) fn command_id(&self) -> u64 {
        self.command.get()
    }

    /// True if nothing has happened since command `id`.
    pub(crate) fn is_current(&self, id: u64) -> bool {
        self.command.get() == id
    }
}

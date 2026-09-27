//! Playback engine: one GStreamer playbin for the whole app, plus the play queue.
//! It lives on the GTK main thread; GStreamer does the heavy work on its own threads
//! and reports back through its bus, which the GTK main loop watches.

use gstreamer as gst;
use gst::prelude::*;
use std::cell::{Cell, OnceCell, RefCell};
use std::rc::{Rc, Weak};

pub(crate) struct Player {
    playbin: gst::Element,
    queue: RefCell<Vec<subsonic::Song>>,
    position: Cell<usize>,
    client: RefCell<Option<subsonic::Client>>,
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
                queue: RefCell::new(Vec::new()),
                position: Cell::new(0),
                client: RefCell::new(None),
                _bus_watch: bus_watch,
            }
        })
    }

    /// Replaces the queue with `songs` and starts playing the one at `start`.
    pub(crate) fn play(&self, client: subsonic::Client, songs: Vec<subsonic::Song>, start: usize) {
        self.client.replace(Some(client));
        self.queue.replace(songs);
        self.play_at(start);
    }

    fn play_at(&self, index: usize) {
        let song = self.queue.borrow().get(index).cloned();
        let client = self.client.borrow().clone();
        let (Some(song), Some(client)) = (song, client) else {
            // Past the end of the queue: stop.
            let _ = self.playbin.set_state(gst::State::Null);
            return;
        };
        let Ok(url) = client.stream_url(&song.id) else {
            return;
        };

        self.position.set(index);
        let _ = self.playbin.set_state(gst::State::Null);
        self.playbin.set_property("uri", url.as_str());
        if let Err(e) = self.playbin.set_state(gst::State::Playing) {
            eprintln!("Could not start playback: {e}");
        }
        eprintln!("Playing: {}", song.title);
    }

    fn on_message(&self, message: &gst::Message) {
        match message.view() {
            gst::MessageView::Eos(..) => self.play_at(self.position.get() + 1),
            // Only the short error text: the debug details may contain the stream
            // URL, and with it the login token.
            gst::MessageView::Error(err) => eprintln!("Playback error: {}", err.error()),
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

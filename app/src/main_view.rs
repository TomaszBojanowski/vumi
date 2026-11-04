//! Main view: the album grid, plus the connection state, which the offline
//! banner of every page shows (offline_banner).

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;
use std::sync::Arc;
use std::time::Instant;

use crate::album_card::AlbumCard;
use crate::album_page::AlbumPage;
use crate::artist_card::ArtistCard;
use crate::artist_page::ArtistPage;
use crate::{APP_NAME, covers, error_message, grid_fill, library, runtime, since_start};

/// One artist in the Artists grid.
struct ArtistItem {
    id: String,
    name: String,
    albums: u32,
    /// The cover id of the artist's picture, if the server has one.
    picture: Option<String>,
    /// The letter this artist is the first one under, if it is.
    letter: Option<String>,
}

/// What artists are sorted by: the name in lower case, with accented letters
/// as their base letters ("Óscar" → "oscar", "Åsa" → "asa", "Łukasz" →
/// "lukasz"), so they stand among those rather than after "z".
pub(crate) fn artist_sort_key(name: &str) -> String {
    // Default is the canonical decomposition (NFD).
    glib::normalize(name.to_lowercase(), glib::NormalizeMode::Default)
        .chars()
        // The accents, which the decomposition has split off the letters.
        .filter(|c| !matches!(*c as u32, 0x0300..=0x036F | 0x1AB0..=0x1AFF | 0x1DC0..=0x1DFF | 0x20D0..=0x20FF))
        // Letters of their own in Unicode, which no decomposition takes apart.
        .map(|c| match c {
            'ł' => 'l',
            'ø' => 'o',
            'đ' => 'd',
            c => c,
        })
        .collect()
}

/// The letter an artist is listed under, from its sort key: the first of the
/// name without its accent, or "#" for digits and symbols.
fn artist_letter(key: &str) -> String {
    key.chars()
        .next()
        .filter(|c| c.is_alphabetic())
        .map(|c| c.to_uppercase().to_string())
        .unwrap_or_else(|| "#".to_string())
}

/// What the last attempt to reach the server came to.
#[derive(Clone, Debug, Default, PartialEq)]
pub enum Connection {
    /// No account in use.
    #[default]
    None,
    /// Asked, no answer yet.
    Connecting,
    /// The server answered: its software (e.g. "navidrome") and version,
    /// where it says them.
    Connected {
        server: Option<String>,
        version: Option<String>,
    },
    /// No usable answer; the message says why.
    Offline(String),
}

impl Connection {
    pub(crate) fn from_response(r: &subsonic::Response) -> Self {
        Self::Connected {
            server: r.server_type.clone(),
            version: r.server_version.clone(),
        }
    }
}

/// How the view connects with the saved login; `true` for a quiet attempt.
type Reconnect = Rc<dyn Fn(bool)>;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/main_view.blp")]
    pub struct MainView {
        #[template_child]
        pub status: gtk::TemplateChild<adw::StatusPage>,
        #[template_child]
        pub stack: gtk::TemplateChild<gtk::Stack>,
        #[template_child]
        pub grid: gtk::TemplateChild<gtk::GridView>,
        #[template_child]
        pub player_slot: gtk::TemplateChild<adw::Bin>,
        #[template_child]
        pub mini_slot: gtk::TemplateChild<adw::Bin>,
        #[template_child]
        pub split: gtk::TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub queue_split: gtk::TemplateChild<adw::OverlaySplitView>,
        #[template_child]
        pub views: gtk::TemplateChild<adw::ViewStack>,
        #[template_child]
        pub sections: gtk::TemplateChild<adw::ViewSwitcherSidebar>,
        #[template_child]
        pub sidebar_bin: gtk::TemplateChild<adw::BreakpointBin>,
        #[template_child]
        pub low_window: gtk::TemplateChild<adw::Breakpoint>,
        #[template_child]
        pub library_nav: gtk::TemplateChild<adw::NavigationView>,
        #[template_child]
        pub albums_page: gtk::TemplateChild<adw::ViewStackPage>,
        #[template_child]
        pub artists_page: gtk::TemplateChild<adw::ViewStackPage>,
        #[template_child]
        pub artists_stack: gtk::TemplateChild<gtk::Stack>,
        #[template_child]
        pub artists_grid: gtk::TemplateChild<gtk::GridView>,
        #[template_child]
        pub artists_letters: gtk::TemplateChild<adw::WrapBox>,
        /// The artists in the grid, in the order shown.
        pub artist_items: OnceCell<gio::ListStore>,
        #[template_child]
        pub playlists_page: gtk::TemplateChild<adw::ViewStackPage>,
        #[template_child]
        pub playlists_stack: gtk::TemplateChild<gtk::Stack>,
        #[template_child]
        pub playlists_grid: gtk::TemplateChild<gtk::Grid>,
        #[template_child]
        pub playlists_wide: gtk::TemplateChild<adw::Breakpoint>,
        /// Whether the Playlists section has room for two columns of cards.
        pub playlists_two_columns: Cell<bool>,
        /// The playlists as the server lists them.
        pub playlists: RefCell<Vec<subsonic::Playlist>>,
        /// The artists as the server lists them (with their pictures); empty
        /// until the sync, when the albums in memory stand in.
        pub artists: RefCell<Vec<subsonic::Artist>>,
        /// What the grid shows now (id, album count, picture), so an unchanged
        /// list is not put into the grid again.
        pub artists_shown: RefCell<Vec<(String, u32, Option<String>)>>,
        #[template_child]
        pub search_entry: gtk::TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub sync_button: gtk::TemplateChild<gtk::Button>,
        /// True while albums and then songs are being synced.
        pub syncing: Cell<bool>,
        /// Bumped by every sync; a sync that is no longer the newest (another
        /// account was connected meanwhile) drops its results.
        pub sync_generation: Cell<u64>,
        /// The search results, made on first use and reused.
        pub search_page: OnceCell<crate::search_page::SearchPage>,
        /// Bumped by every search; results of an older one are dropped.
        pub search_generation: Cell<u32>,
        #[template_child]
        pub start_page: gtk::TemplateChild<crate::start_page::StartPage>,
        #[template_child]
        pub favorites: gtk::TemplateChild<crate::results_view::ResultsView>,
        #[template_child]
        pub favorites_narrow: gtk::TemplateChild<adw::Breakpoint>,
        #[template_child]
        pub favorites_toolbar: gtk::TemplateChild<adw::ToolbarView>,
        /// The colour behind the header of the Favorites page.
        pub favorites_layer: OnceCell<crate::tint_layer::TintLayer>,
        /// The top of the Favorites page, put into its list.
        pub favorites_header: OnceCell<crate::favorites_header::FavoritesHeader>,
        /// What the Favorites page shows, as read from the database (songs
        /// most recently starred first), and the client it came with.
        pub favorite_content: RefCell<crate::results_view::Content>,
        /// How the favourite songs are sorted: the switch on their heading.
        pub favorites_sort: OnceCell<adw::ToggleGroup>,
        /// The Favorites page is below its breakpoint.
        pub favorites_is_narrow: Cell<bool>,
        /// How the grid is sorted (see sort_albums).
        pub sort: RefCell<String>,
        /// Albums shown in the grid; items are subsonic::Album wrapped for GTK.
        pub albums: OnceCell<gio::ListStore>,
        /// A copy of what the grid shows, to skip redraws when nothing changed.
        pub current: RefCell<Vec<Arc<subsonic::Album>>>,
        /// The connected client, used by cards to fetch their covers.
        pub client: RefCell<Option<subsonic::Client>>,
        /// What the last attempt to reach the server came to.
        pub connection: RefCell<Connection>,
        /// Connects with the saved login (main.rs: auto_connect); `true` for a
        /// quiet attempt, which leaves the screen as it is unless it succeeds.
        pub reconnect: RefCell<Option<super::Reconnect>>,
        /// An attempt to connect is on its way: no second one meanwhile.
        pub attempting: Cell<bool>,
        /// Failed attempts since the server last answered (the retry delay).
        pub retry_attempt: Cell<u32>,
        /// The next attempt, while the server cannot be reached.
        pub retry_timer: RefCell<Option<glib::SourceId>>,
        /// Attempts started by the window becoming active, or by the network.
        pub activation_gate: RefCell<crate::reconnect::Gate>,
        pub network_gate: RefCell<crate::reconnect::Gate>,
        /// The offline state is a lost connection (not a server error): the
        /// banners say so in their own words.
        pub offline_lost: Cell<bool>,
        /// Retry was pressed and its attempt is on its way: "Connecting…".
        pub user_retrying: Cell<bool>,
        /// A check after a failed request: one at a time, at most every 5 s.
        pub checking: Cell<bool>,
        pub check_gate: RefCell<crate::reconnect::Gate>,
        /// Every card the grid created (only as many as fit on screen, plus a few).
        pub cards: RefCell<Vec<glib::WeakRef<AlbumCard>>>,
        /// Whether the startup time was already reported.
        pub reported: Cell<bool>,
        /// The account ("user@url") whose library is on screen, or empty.
        pub server: RefCell<String>,
        /// The stars changed in this session up to the start of the running
        /// sync (favorites::changes): once it has stored the server's stars,
        /// the earlier ones follow them (favorites::settle).
        pub stars_since: Cell<u64>,
        /// What the running full sync stores of the scan status it began with
        /// (library::scan_to_store).
        pub scan_to_store: RefCell<Option<String>>,
        /// True once the server's own album list was shown (even an empty one):
        /// the local copy read at the same time must not replace it.
        pub synced: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MainView {
        // Must match "template $VumiMainView" in the Blueprint file.
        const NAME: &'static str = "VumiMainView";
        type Type = super::MainView;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            // The Start page is used in this template, so its type must exist first.
            crate::start_page::StartPage::static_type();
            crate::results_view::ResultsView::static_type();
            crate::queue_page::QueuePage::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MainView {
        fn signals() -> &'static [glib::subclass::Signal] {
            static SIGNALS: std::sync::OnceLock<Vec<glib::subclass::Signal>> =
                std::sync::OnceLock::new();
            // "connection-changed": connection() gives another answer now.
            // "library-changed": a sync started or ended, or the artists came.
            SIGNALS.get_or_init(|| {
                vec![
                    glib::subclass::Signal::builder("connection-changed").build(),
                    glib::subclass::Signal::builder("library-changed").build(),
                ]
            })
        }
    }
    impl WidgetImpl for MainView {}
    impl NavigationPageImpl for MainView {}
}

glib::wrapper! {
    pub struct MainView(ObjectSubclass<imp::MainView>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MainView {
    pub(crate) fn new() -> Self {
        let view: Self = glib::Object::new();
        view.set_title(APP_NAME);
        view.setup_grid();
        view.setup_artists_grid();
        view.setup_queue();
        view.setup_sections();
        view.setup_playlists();
        view.setup_favorites();
        view.setup_sorting();
        view.setup_search();
        view.setup_retry();
        // An empty star, as on the star buttons of songs not yet starred.
        view.imp().favorites.set_empty(
            "non-starred-symbolic",
            &gettext("No Favorites"),
            &gettext("Star albums and songs to see them here."),
        );
        view
    }

    /// Connects the grid to the album store and tells it how to draw a card.
    fn setup_grid(&self) {
        let imp = self.imp();
        let albums = gio::ListStore::new::<glib::BoxedAnyObject>();

        let factory = gtk::SignalListItemFactory::new();
        // Called only for as many cards as fit on screen.
        let view = self.downgrade();
        factory.connect_setup(move |_, item| {
            if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
                let card = AlbumCard::new();
                // The grid makes its cards as wide as its cells.
                card.set_cover_grows(true);
                if let Some(view) = view.upgrade() {
                    view.imp().cards.borrow_mut().push(card.downgrade());
                }
                item.set_child(Some(&card));
            }
        });
        // Called whenever a card is (re)used for a given album.
        let view = self.downgrade();
        factory.connect_bind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let card = item.child().and_downcast::<AlbumCard>();
            let object = item.item().and_downcast::<glib::BoxedAnyObject>();
            let (Some(card), Some(object)) = (card, object) else {
                return;
            };

            // Covers come from the disk cache even before the server answers;
            // the client is only needed for covers that are not cached yet.
            let client = view.upgrade().and_then(|v| v.imp().client.borrow().clone());
            card.set_album(&object.borrow::<Arc<subsonic::Album>>(), client);
        });

        // A click on a card opens the album page on top of the library.
        let view = self.downgrade();
        imp.grid.connect_activate(move |grid, position| {
            let Some(view) = view.upgrade() else {
                return;
            };
            let Some(object) = grid
                .model()
                .and_then(|model| model.item(position))
                .and_downcast::<glib::BoxedAnyObject>()
            else {
                return;
            };
            let client = view.imp().client.borrow().clone();
            let album = (**object.borrow::<Arc<subsonic::Album>>()).clone();
            if let Some(nav) = grid
                .ancestor(adw::NavigationView::static_type())
                .and_downcast::<adw::NavigationView>()
            {
                crate::album_page::open(&nav, &album, client, view.imp().server.borrow().clone());
            }
        });

        // The grid gets its albums only when the section is first opened: with
        // them it makes cards (and loads covers) for a few hundred albums even
        // while hidden, which cost the start about 40 ms of work and frames of
        // up to 50 ms. Then they come in parts, as in the Artists grid.
        grid_fill::attach(&imp.grid, &albums, 48, 12);
        imp.grid.set_factory(Some(&factory));
        let _ = imp.albums.set(albums);
    }

    /// Connects the Artists grid to its store. As in the album grid, cards
    /// exist only for the artists on screen and are reused while scrolling.
    fn setup_artists_grid(&self) {
        let imp = self.imp();
        let artists = gio::ListStore::new::<glib::BoxedAnyObject>();

        let factory = gtk::SignalListItemFactory::new();
        factory.connect_setup(|_, item| {
            if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
                item.set_child(Some(&ArtistCard::new()));
            }
        });
        let view = self.downgrade();
        factory.connect_bind(move |_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let card = item.child().and_downcast::<ArtistCard>();
            let object = item.item().and_downcast::<glib::BoxedAnyObject>();
            let (Some(card), Some(object)) = (card, object) else {
                return;
            };
            let client = view.upgrade().and_then(|v| v.imp().client.borrow().clone());
            let artist = object.borrow::<ArtistItem>();
            card.set_artist(
                &artist.name,
                artist.albums,
                artist.picture.clone(),
                artist.letter.as_deref(),
                client,
            );
        });
        // Scrolled out of sight: the card lets go of the picture. The cover
        // cache still has it for a while, should the artist come back.
        factory.connect_unbind(|_, item| {
            let card = item
                .downcast_ref::<gtk::ListItem>()
                .and_then(|item| item.child())
                .and_downcast::<ArtistCard>();
            if let Some(card) = card {
                card.clear();
            }
        });

        // A click on a card opens the artist page on top of the list.
        let view = self.downgrade();
        imp.artists_grid.connect_activate(move |grid, position| {
            let Some(view) = view.upgrade() else {
                return;
            };
            let Some(object) = grid
                .model()
                .and_then(|model| model.item(position))
                .and_downcast::<glib::BoxedAnyObject>()
            else {
                return;
            };
            let Some(client) = view.imp().client.borrow().clone() else {
                return;
            };
            let artist = object.borrow::<ArtistItem>();
            view.open_artist_from(grid, &artist.id, &artist.name, client);
        });

        // The grid gets its artists only when the section is first opened: with
        // them it makes cards (and loads pictures) for a few hundred artists
        // even while hidden, and many never open this section. Then they come
        // in parts: about a screen of cards at once, then 7 a frame (measured:
        // a card on screen takes about 0.9 ms, one below it about 0.3 ms).
        grid_fill::attach(&imp.artists_grid, &artists, 36, 7);
        imp.artists_grid.set_factory(Some(&factory));
        let _ = imp.artist_items.set(artists);
    }

    /// Fills the grid from the local database, without touching the network.
    pub(crate) fn load_cached(&self, server: String) {
        covers::set_server(server.clone());
        self.imp().server.replace(server.clone());
        let account = server.clone();
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn_blocking(move || library::load_albums(&server))
                .await;
            let Some(view) = view.upgrade() else {
                return;
            };
            // Another account meanwhile (logged out, or in again), or the
            // server's own list is already on screen: this copy is not wanted.
            if *view.imp().server.borrow() != account || view.imp().synced.get() {
                return;
            }
            match result {
                Ok(Ok(albums)) if !albums.is_empty() => {
                    view.show_albums(albums.into_iter().map(Arc::new).collect())
                }
                Ok(Ok(_)) => {}
                Ok(Err(e)) => eprintln!("Could not read the library database: {e}"),
                Err(_) => {}
            }
        });
    }

    pub(crate) fn show_connecting(&self) {
        self.set_connection(Connection::Connecting);
        self.imp()
            .status
            .set_description(Some(&gettext("Connecting…")));
    }

    /// Called after a successful ping; syncs the album list in the background.
    pub(crate) fn show_connected(&self, r: &subsonic::Response, client: subsonic::Client) {
        let imp = self.imp();
        self.cancel_retry();
        imp.retry_attempt.set(0);
        self.set_connection(Connection::from_response(r));
        // Album pages opened offline get the client and refresh.
        for page in self.open_album_pages() {
            page.set_client(client.clone());
        }
        let text = match r.server_type.as_deref() {
            Some(server) => {
                // Translators: {server} is the server software (e.g. navidrome), {version} its version.
                let text = gettext("Connected to {server} {version}");
                text.replace("{server}", server)
                    .replace("{version}", r.server_version.as_deref().unwrap_or(""))
                    .trim_end()
                    .to_string()
            }
            None => gettext("Connected"),
        };
        // The description is markup; the server names itself.
        imp.status
            .set_description(Some(&glib::markup_escape_text(&text)));
        self.sync(client, false);
    }

    /// The server could not be reached, or did not answer as it should.
    pub(crate) fn show_offline(&self, error: &subsonic::Error) {
        self.go_offline(
            &error_message(error),
            matches!(error, subsonic::Error::Unreachable(_)),
        );
    }

    /// The same, with a message of its own (a failure that is not the server's).
    pub(crate) fn show_offline_message(&self, message: &str) {
        self.go_offline(message, false);
    }

    fn go_offline(&self, message: &str, lost: bool) {
        let imp = self.imp();
        imp.offline_lost.set(lost);
        self.set_connection(Connection::Offline(message.to_string()));
        imp.status.set_description(Some(&gettext("Offline")));
        self.refresh_banners();
        self.schedule_retry();
    }

    /// The banner on every page: shown while offline, with Retry; while the
    /// attempt Retry started is on its way, "Connecting…" without the button.
    fn refresh_banners(&self) {
        let imp = self.imp();
        // Retry's attempt (it shows "Connecting…" in Preferences too) keeps
        // the banners up, saying so.
        if imp.user_retrying.get() {
            crate::offline_banner::show(true, &gettext("Connecting…"), None);
            return;
        }
        let message = match &*imp.connection.borrow() {
            Connection::Offline(message) => message.clone(),
            _ => {
                crate::offline_banner::show(false, "", None);
                return;
            }
        };
        if imp.offline_lost.get() {
            crate::offline_banner::show(
                true,
                // Translators: the banner on every page while the server cannot be reached.
                &gettext("Offline — showing the library saved on this computer"),
                Some(&gettext("Retry")),
            );
        } else {
            // A server error or an answer that is not Subsonic: the error itself.
            crate::offline_banner::show(true, &message, Some(&gettext("Retry")));
        }
    }

    /// How Vumi connects with the saved login (main.rs), for Retry and for
    /// the attempts it makes by itself while the server cannot be reached.
    pub(crate) fn set_reconnect(&self, reconnect: impl Fn(bool) + 'static) {
        self.imp().reconnect.replace(Some(Rc::new(reconnect)));
    }

    /// Tries to connect now, unless an attempt is on its way. A `quiet` one
    /// (by itself) shows nothing until it succeeds; Retry and the start are
    /// not quiet.
    pub(crate) fn retry_now(&self, quiet: bool) {
        let imp = self.imp();
        // Retry while offline: the banners say an attempt is on its way, also
        // when one Vumi started by itself is (its answer counts).
        if !quiet && self.is_offline() {
            imp.user_retrying.set(true);
            self.refresh_banners();
        }
        if imp.attempting.get() {
            return;
        }
        let Some(reconnect) = imp.reconnect.borrow().clone() else {
            imp.user_retrying.set(false);
            return;
        };
        self.cancel_retry();
        imp.attempting.set(true);
        reconnect(quiet);
    }

    /// The attempt of retry_now has an answer (whatever it was). The banners
    /// follow from what comes next (connected, offline, logged out), so they
    /// never hide for a moment between "Connecting…" and the offline text.
    pub(crate) fn attempt_done(&self) {
        let imp = self.imp();
        imp.attempting.set(false);
        imp.user_retrying.set(false);
    }

    /// The next quiet attempt, later each time: 5 s, 15 s, 30 s, 60 s, then
    /// every 2 minutes (reconnect::retry_delay).
    fn schedule_retry(&self) {
        let imp = self.imp();
        if imp.reconnect.borrow().is_none() {
            return;
        }
        self.cancel_retry();
        let attempt = imp.retry_attempt.get();
        imp.retry_attempt.set(attempt.saturating_add(1));
        let view = self.downgrade();
        let id = glib::timeout_add_local_once(crate::reconnect::retry_delay(attempt), move || {
            if let Some(view) = view.upgrade() {
                // It has run: nothing left to remove.
                view.imp().retry_timer.take();
                view.retry_now(true);
            }
        });
        imp.retry_timer.replace(Some(id));
    }

    /// The album pages open in every section's navigation (Start, Albums,
    /// Favorites... each has its own).
    fn open_album_pages(&self) -> Vec<AlbumPage> {
        let mut found = Vec::new();
        let sections = self.imp().views.pages();
        for i in 0..sections.n_items() {
            let Some(nav) = sections
                .item(i)
                .and_downcast::<adw::ViewStackPage>()
                .and_then(|page| page.child().downcast::<adw::NavigationView>().ok())
            else {
                continue;
            };
            let stack = nav.navigation_stack();
            for j in 0..stack.n_items() {
                if let Some(page) = stack.item(j).and_downcast::<AlbumPage>() {
                    found.push(page);
                }
            }
        }
        found
    }

    fn cancel_retry(&self) {
        if let Some(id) = self.imp().retry_timer.take() {
            id.remove();
        }
    }

    fn is_offline(&self) -> bool {
        matches!(*self.imp().connection.borrow(), Connection::Offline(_))
    }

    /// The network came back, or went (GNetworkMonitor). Back: an attempt now,
    /// and the delays start over. Gone: the server is asked once; the monitor
    /// alone never decides (a server in the local network works without the
    /// internet, and the monitor may be wrong).
    pub(crate) fn network_changed(&self, available: bool) {
        let imp = self.imp();
        let now = Instant::now();
        if available && self.is_offline() {
            if imp
                .network_gate
                .borrow_mut()
                .allow(now, std::time::Duration::from_secs(2))
            {
                imp.retry_attempt.set(0);
                self.retry_now(true);
            }
        } else if !available {
            self.check_connection();
        }
    }

    /// The window became active again, or something wanted the server (Play):
    /// while offline, an attempt now (at most every 10 s).
    pub(crate) fn retry_soon(&self) {
        if self.is_offline()
            && self
                .imp()
                .activation_gate
                .borrow_mut()
                .allow(Instant::now(), std::time::Duration::from_secs(10))
        {
            self.retry_now(true);
        }
    }

    /// A request failed (reconnect::request_failed) or the network went: the
    /// server is asked once, at most every 5 s and one question at a time;
    /// only if it cannot be reached does the offline state show.
    pub(crate) fn check_connection(&self) {
        let imp = self.imp();
        if !matches!(*imp.connection.borrow(), Connection::Connected { .. }) || imp.checking.get() {
            return;
        }
        let Some(client) = imp.client.borrow().clone() else {
            return;
        };
        if !imp
            .check_gate
            .borrow_mut()
            .allow(Instant::now(), std::time::Duration::from_secs(5))
        {
            return;
        }
        imp.checking.set(true);
        let session = self.session();
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            // On a new connection: one kept from before may be dead (the
            // network went) and hold the answer up for the whole timeout.
            let result = runtime()
                .spawn(async move { client.ping_fresh().await })
                .await;
            let Some(view) = view.upgrade() else {
                return;
            };
            view.imp().checking.set(false);
            // Logged out, or connected anew, meanwhile.
            if view.session() != session {
                return;
            }
            if let Ok(Err(e @ subsonic::Error::Unreachable(_))) = result {
                view.show_offline(&e);
            }
        });
    }

    /// The player asked the server after a failed stream and got no answer:
    /// the view asks it too, now, however lately it did.
    pub(crate) fn server_lost(&self) {
        self.imp().check_gate.replace(Default::default());
        self.check_connection();
    }

    /// What is known about the connection now (Preferences shows it).
    pub(crate) fn connection(&self) -> Connection {
        self.imp().connection.borrow().clone()
    }

    /// Remembers the state and tells whoever listens ("connection-changed").
    fn set_connection(&self, connection: Connection) {
        if *self.imp().connection.borrow() == connection {
            return;
        }
        // Playing waits for the server while offline, and while the attempt
        // Retry started is on its way.
        let offline = match &connection {
            Connection::Offline(_) => true,
            Connection::Connecting => crate::reconnect::is_offline(),
            _ => false,
        };
        self.imp().connection.replace(connection);
        crate::reconnect::set_offline(offline);
        self.refresh_start_state();
        self.refresh_banners();
        self.emit_by_name::<()>("connection-changed", &[]);
    }

    /// Runs `f` whenever connection() changes, until the handler is disconnected.
    pub(crate) fn connect_connection_changed<F: Fn(&Self) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("connection-changed", false, move |values| {
            if let Ok(view) = values[0].get::<Self>() {
                f(&view);
            }
            None
        })
    }

    /// The client of the account in use, once the server has answered.
    pub(crate) fn client(&self) -> Option<subsonic::Client> {
        self.imp().client.borrow().clone()
    }

    /// Retry in the offline banners: an attempt now, shown as one. Every
    /// section's first page gets its banner here; other pages add theirs.
    fn setup_retry(&self) {
        let view = self.downgrade();
        crate::offline_banner::set_retry(move || {
            if let Some(view) = view.upgrade() {
                view.retry_now(false);
            }
        });
        let pages = self.imp().views.pages();
        for i in 0..pages.n_items() {
            let root = pages
                .item(i)
                .and_downcast::<adw::ViewStackPage>()
                .and_then(|page| page.child().downcast::<adw::NavigationView>().ok())
                .and_then(|nav| nav.navigation_stack().item(0))
                .and_downcast::<adw::NavigationPage>();
            if let Some(root) = root {
                crate::offline_banner::attach(&root);
            }
        }
    }

    /// Syncs the library of `client`'s account: in full when `full` (Ctrl+R),
    /// or when the server has scanned its library since the last full sync
    /// (getScanStatus); otherwise only what changes without a scan
    /// (sync_light).
    fn sync(&self, client: subsonic::Client, full: bool) {
        // A new sync supersedes a running one (another account, say): the
        // client changes right away, and the older sync drops its results.
        let generation = self.imp().sync_generation.get() + 1;
        self.imp().sync_generation.set(generation);
        self.imp().stars_since.set(crate::favorites::changes());
        self.set_syncing(true);
        self.imp().client.replace(Some(client.clone()));
        // Covers on the Start page can now come from the server too.
        self.refresh_start();
        self.refresh_favorites();
        // Results found before the connection get the client, to play songs.
        let text = self.imp().search_entry.text();
        if !text.trim().is_empty() {
            self.search_library(&text);
        }
        // Cards drawn before the connection could not fetch uncached covers:
        // every card that still shows the placeholder tries again now.
        for card in self.imp().cards.borrow().iter().filter_map(|c| c.upgrade()) {
            card.retry_cover(client.clone());
        }
        let server = client.server_key();
        covers::set_server(server.clone());
        self.imp().server.replace(server.clone());
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            let request = client.clone();
            let status = match runtime()
                .spawn(async move { request.scan_status().await })
                .await
            {
                Ok(Ok(status)) => Some(status),
                Ok(Err(e)) => {
                    eprintln!("Could not read the scan status: {e}");
                    None
                }
                Err(_) => None,
            };
            let account = server.clone();
            let stored = runtime()
                .spawn_blocking(move || library::last_scan(&account))
                .await
                .ok()
                .and_then(Result::ok)
                .flatten();
            let Some(view) = view
                .upgrade()
                .filter(|v| v.imp().sync_generation.get() == generation)
            else {
                return;
            };
            if !full && library::library_unchanged(stored.as_deref(), status.as_ref()) {
                view.sync_light(client, generation);
            } else {
                view.imp()
                    .scan_to_store
                    .replace(library::scan_to_store(status.as_ref()));
                view.sync_albums(client, generation);
            }
        });
    }

    /// Fetches the album list from the server, shows it and stores it locally;
    /// the songs, playlists and artists follow.
    fn sync_albums(&self, client: subsonic::Client, generation: u64) {
        let server = client.server_key();
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            let started = Instant::now();
            let result = runtime()
                .spawn(async move { client.all_albums().await })
                .await;
            let Some(view) = view.upgrade() else {
                return;
            };
            // Superseded meanwhile: the newer sync owns both the view and the
            // database, so this response is neither shown nor saved.
            let stale = view.imp().sync_generation.get() != generation;
            match result {
                Ok(Ok(albums)) => {
                    eprintln!(
                        "Synced {} albums in {:.2?}",
                        albums.len(),
                        started.elapsed()
                    );
                    // Shared, not copied: the grid and the save below point at the same albums.
                    let albums: Vec<Arc<subsonic::Album>> =
                        albums.into_iter().map(Arc::new).collect();
                    if stale {
                        return;
                    }
                    view.imp().synced.set(true);
                    let to_save = albums.clone();
                    runtime().spawn_blocking(move || {
                        if let Err(e) = library::save_albums(&server, &to_save) {
                            eprintln!("Could not save the library database: {e}");
                        }
                    });
                    view.show_albums(albums);
                    // Songs next, in the background: album pages open already filled.
                    view.sync_songs();
                    view.sync_playlists();
                }
                Ok(Err(e)) if !stale => {
                    view.show_offline(&e);
                    view.set_syncing(false);
                }
                Err(_) if !stale => view.set_syncing(false),
                _ => {}
            }
        });
    }

    fn show_albums(&self, albums: Vec<Arc<subsonic::Album>>) {
        let imp = self.imp();

        // Same list as on screen: nothing to redraw.
        if *imp.current.borrow() == albums {
            return;
        }

        imp.albums_page.set_badge_number(albums.len() as u32);
        imp.current.replace(albums);
        self.refresh_grid();
        self.refresh_artists();
        self.refresh_start();
        self.refresh_favorites();
        imp.stack.set_visible_child_name("library");

        if !imp.reported.replace(true) {
            eprintln!("Library on screen {:.0?} after start", since_start());
        }
    }

    /// Puts the now-playing card at the bottom of the sidebar; on a low window
    /// it takes a small cover (low_window).
    pub(crate) fn set_now_playing(&self, card: &crate::now_playing::NowPlaying) {
        let imp = self.imp();
        imp.player_slot.set_child(Some(card));
        let low = imp.low_window.get();
        let applied = imp.sidebar_bin.current_breakpoint().as_ref() == Some(&low);
        card.set_compact(applied);
        let weak = card.downgrade();
        low.connect_apply(move |_| {
            if let Some(card) = weak.upgrade() {
                card.set_compact(true);
            }
        });
        let weak = card.downgrade();
        low.connect_unapply(move |_| {
            if let Some(card) = weak.upgrade() {
                card.set_compact(false);
            }
        });
    }

    /// Puts the compact now-playing bar under the content (narrow windows).
    pub(crate) fn set_mini_player(&self, widget: &impl IsA<gtk::Widget>) {
        self.imp().mini_slot.set_child(Some(widget));
    }

    /// Slides the sidebar in, with the full now-playing card.
    pub(crate) fn show_sidebar(&self) {
        self.imp().split.set_show_sidebar(true);
    }

    /// Opens an album page in the Albums section, e.g. the album that plays
    /// (from the sidebar card, which belongs to no section).
    pub(crate) fn open_album(&self, id: &str, fallback: subsonic::Album) {
        let imp = self.imp();
        imp.views.set_visible_child_name("library");
        self.open_album_on(&imp.library_nav.get(), id, fallback);
    }

    /// The navigation view of the section `from` is in, where a page opened
    /// from it goes so that Back returns there. A widget outside the sections
    /// (a row of the queue panel) has none: its pages go to the Albums
    /// section, which comes to the front, and a queue lying over the content
    /// (narrow windows) gets out of the way.
    fn section_of(&self, from: &impl IsA<gtk::Widget>) -> adw::NavigationView {
        let imp = self.imp();
        let section = from
            .ancestor(adw::NavigationView::static_type())
            .and_downcast::<adw::NavigationView>()
            // The window has a navigation view too, around this whole view:
            // that one is not a section.
            .filter(|nav| nav.is_ancestor(self));
        section.unwrap_or_else(|| {
            imp.views.set_visible_child_name("library");
            if imp.queue_split.is_collapsed() {
                imp.queue_split.set_show_sidebar(false);
            }
            imp.library_nav.get()
        })
    }

    /// Opens an album page in the section `from` belongs to (Start, Favorites,
    /// search results), so that Back returns there.
    pub(crate) fn open_album_from(
        &self,
        from: &impl IsA<gtk::Widget>,
        id: &str,
        fallback: subsonic::Album,
    ) {
        let nav = self.section_of(from);
        self.open_album_on(&nav, id, fallback);
    }

    /// Opens an artist page in the section `from` belongs to, as
    /// open_album_from does for albums.
    pub(crate) fn open_artist_from(
        &self,
        from: &impl IsA<gtk::Widget>,
        id: &str,
        name: &str,
        client: subsonic::Client,
    ) {
        let imp = self.imp();
        let nav = self.section_of(from);
        if imp.split.is_collapsed() {
            imp.split.set_show_sidebar(false);
        }

        if !back_to::<ArtistPage>(&nav, |page| page.artist_id() == id) {
            nav.push(&ArtistPage::new(id, name, client));
        }
    }

    /// Uses the full album from the library if it is there, else `fallback`.
    fn open_album_on(&self, nav: &adw::NavigationView, id: &str, fallback: subsonic::Album) {
        let imp = self.imp();
        let album = imp
            .current
            .borrow()
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .unwrap_or_else(|| Arc::new(fallback));
        let client = imp.client.borrow().clone();
        if imp.split.is_collapsed() {
            imp.split.set_show_sidebar(false);
        }

        if !back_to::<AlbumPage>(nav, |page| page.album_id() == id) {
            crate::album_page::open(nav, &album, client, imp.server.borrow().clone());
        }
    }

    /// The cover to show for `song` (covers::song_cover): its album's, as the
    /// album page and the grid show it (and as the cache keeps it, offline
    /// too); the song's own when its album is not in the library. Tracks have
    /// cover ids of their own on the server, which the cache has only if they
    /// were shown.
    pub(crate) fn song_cover(&self, song: &subsonic::Song) -> Option<String> {
        let album = song.album_id.as_deref().and_then(|id| {
            self.imp()
                .current
                .borrow()
                .iter()
                .find(|album| album.id == id)
                .and_then(|album| album.cover_art.clone())
        });
        album.or_else(|| song.cover_art.clone())
    }

    /// Fills the grid from `current`, in the order chosen in the sort menu.
    fn refresh_grid(&self) {
        let imp = self.imp();
        let mut albums = imp.current.borrow().clone();
        sort_albums(&mut albums, &imp.sort.borrow());
        let objects: Vec<glib::BoxedAnyObject> =
            albums.into_iter().map(glib::BoxedAnyObject::new).collect();
        if let Some(store) = imp.albums.get() {
            // One change notification for the whole list, not one per album.
            store.splice(0, store.n_items(), &objects);
        }
    }

    /// The "library.sort" action behind the sort menu; the choice is remembered.
    fn setup_sorting(&self) {
        let settings = crate::settings();
        let mode = settings.string("album-sort").to_string();
        self.imp().sort.replace(mode.clone());

        let action = gio::SimpleAction::new_stateful(
            "sort",
            Some(glib::VariantTy::STRING),
            &mode.to_variant(),
        );
        let view = self.downgrade();
        action.connect_activate(move |action, parameter| {
            let Some(view) = view.upgrade() else {
                return;
            };
            let Some(mode) = parameter.and_then(|p| p.get::<String>()) else {
                return;
            };
            action.set_state(&mode.to_variant());
            let _ = settings.set_string("album-sort", &mode);
            view.imp().sort.replace(mode);
            view.refresh_grid();

            // A new order starts from the top (once the grid has its albums:
            // it gets them when first shown).
            let imp = view.imp();
            if imp.grid.model().is_some_and(|model| model.n_items() > 0) {
                imp.grid.scroll_to(0, gtk::ListScrollFlags::NONE, None);
            }
        });

        let group = gio::SimpleActionGroup::new();
        group.add_action(&action);
        self.insert_action_group("library", Some(&group));
    }
}

/// Sorts albums in place: "name", "artist" (then by year), "year-desc",
/// "year-asc" (albums without a year go last) or "added" (newest first).
/// Each sort key is computed once per album, not on every comparison.
fn sort_albums(albums: &mut [Arc<subsonic::Album>], mode: &str) {
    use std::cmp::Reverse;

    let lower = |text: &str| text.to_lowercase();
    match mode {
        "artist" => albums.sort_by_cached_key(|a| {
            (
                lower(a.artist.as_deref().unwrap_or("")),
                a.year,
                lower(&a.name),
            )
        }),
        "year-desc" => albums.sort_by_cached_key(|a| (Reverse(a.year), lower(&a.name))),
        "year-asc" => albums.sort_by_cached_key(|a| (a.year.unwrap_or(i32::MAX), lower(&a.name))),
        // Dates compare as text (ISO 8601), so no key needs to be built.
        "added" => albums.sort_by(|a, b| b.created.cmp(&a.created)),
        _ => albums.sort_by_cached_key(|a| lower(&a.name)),
    }
}

impl MainView {
    /// Fetches every song in the background and stores it locally, so album
    /// pages open already filled. Runs after the albums; nothing waits for it.
    fn sync_songs(&self) {
        let Some(client) = self.imp().client.borrow().clone() else {
            return;
        };
        let server = client.server_key();
        let generation = self.imp().sync_generation.get();
        let last_scan = self.imp().scan_to_store.borrow().clone();
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            // Whatever happens inside, the sync is over when this block ends.
            async {
                let started = std::time::Instant::now();
                let fetched = runtime()
                    .spawn(async move { client.all_songs().await })
                    .await;
                let songs = match fetched {
                    Ok(Ok(songs)) => songs,
                    Ok(Err(e)) => {
                        eprintln!("Could not sync songs: {e}");
                        return;
                    }
                    Err(_) => return,
                };
                // Superseded meanwhile: a newer sync may have saved fresher songs
                // already, so this response is dropped instead of written over them.
                if !view
                    .upgrade()
                    .is_some_and(|v| v.imp().sync_generation.get() == generation)
                {
                    return;
                }
                let count = songs.len();
                let saved = runtime()
                    // The songs come last: with them stored, the sync is done.
                    .spawn_blocking(move || {
                        library::save_songs(&server, &songs)?;
                        library::mark_synced(&server, last_scan.as_deref())
                    })
                    .await;
                match saved {
                    Ok(Ok(())) => {
                        eprintln!("Synced {count} songs in {:.2?}", started.elapsed());
                        // The "Continue" card may need songs that have only just arrived.
                        // Unless another account was connected meanwhile.
                        if let Some(view) = view
                            .upgrade()
                            .filter(|v| v.imp().sync_generation.get() == generation)
                        {
                            view.refresh_start();
                            // The server's stars are stored: the stars this
                            // session changed before the sync follow them.
                            view.read_favorites(Some(view.imp().stars_since.get()));
                            // After the songs, not alongside them: a busy server
                            // answers one big request at a time more reliably.
                            view.sync_artists();
                        }
                    }
                    Ok(Err(e)) => eprintln!("Could not save songs: {e}"),
                    Err(_) => {}
                }
            }
            .await;
            // A newer sync ends on its own; this one must not end it early.
            if let Some(view) = view
                .upgrade()
                .filter(|v| v.imp().sync_generation.get() == generation)
            {
                view.set_syncing(false);
            }
        });
    }
}

/// Goes back to the page of `nav` that `same` recognises, if one is open
/// (true then): opening it again does not stack another copy on top.
fn back_to<T: IsA<adw::NavigationPage> + IsA<glib::Object>>(
    nav: &adw::NavigationView,
    same: impl Fn(&T) -> bool,
) -> bool {
    let stack = nav.navigation_stack();
    for i in (0..stack.n_items()).rev() {
        if let Some(page) = stack.item(i).and_downcast::<T>()
            && same(&page)
        {
            nav.pop_to_page(&page);
            return true;
        }
    }
    false
}

/// Pops `nav` back to its first page, the main page of its section.
fn pop_to_root(nav: &adw::NavigationView) {
    if let Some(root) = nav
        .navigation_stack()
        .item(0)
        .and_downcast::<adw::NavigationPage>()
    {
        nav.pop_to_page(&root);
    }
}

/// Pops every navigation view under `widget` back to its first page.
fn pop_all_to_root(widget: &gtk::Widget) {
    if let Some(nav) = widget.downcast_ref::<adw::NavigationView>() {
        pop_to_root(nav);
    }
    let mut child = widget.first_child();
    while let Some(widget) = child {
        pop_all_to_root(&widget);
        child = widget.next_sibling();
    }
}

impl MainView {
    /// Fetches the artists with their pictures; the grid is rebuilt from them.
    /// Until then (and offline) the albums in memory stand in.
    fn sync_artists(&self) {
        let Some(client) = self.imp().client.borrow().clone() else {
            return;
        };
        let generation = self.imp().sync_generation.get();
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            let current = |view: &glib::WeakRef<MainView>| {
                view.upgrade()
                    .filter(|v| v.imp().sync_generation.get() == generation)
            };
            // A server busy with other work may time out once: one more try
            // after a pause, before the list made from the albums stays.
            let mut artists = None;
            for attempt in 1..=2 {
                if attempt > 1 {
                    glib::timeout_future_seconds(5).await;
                    if current(&view).is_none() {
                        return;
                    }
                }
                let client = client.clone();
                match runtime().spawn(async move { client.artists().await }).await {
                    Ok(Ok(list)) => {
                        artists = Some(list);
                        break;
                    }
                    Ok(Err(e)) => eprintln!("Could not fetch the artists (try {attempt}): {e}"),
                    Err(_) => return,
                }
            }
            let (Some(view), Some(artists)) = (current(&view), artists) else {
                return;
            };
            view.imp().artists.replace(artists);
            view.refresh_artists();
            view.emit_by_name::<()>("library-changed", &[]);
        });
    }

    /// Scrolls the Artists grid so that the artist at `position` is in its
    /// first row, and moves the keyboard focus to that card.
    fn scroll_to_artist(&self, position: u32) {
        let grid = &self.imp().artists_grid;
        // While the grid is still filling, all the artists first; the jump
        // needs their rows laid out, which the next frame does.
        if grid_fill::finish(grid) {
            let view = self.downgrade();
            let frames = Cell::new(0);
            grid.add_tick_callback(move |_, _| {
                frames.set(frames.get() + 1);
                if frames.get() < 2 {
                    return glib::ControlFlow::Continue;
                }
                if let Some(view) = view.upgrade() {
                    view.scroll_to_artist(position);
                }
                glib::ControlFlow::Break
            });
            return;
        }
        // scroll_to alone moves only as far as it must, which leaves an artist
        // further down in the last row on screen. All rows are as tall, so the
        // place of its row is known from the size of any one card.
        let mut card = None;
        let mut cell = grid.first_child();
        while let Some(widget) = cell {
            // The whole cell, with the padding the theme gives it.
            card = widget
                .compute_bounds(&grid.get())
                .filter(|bounds| bounds.width() >= 1.0 && bounds.height() >= 1.0);
            if card.is_some() {
                break;
            }
            cell = widget.next_sibling();
        }
        if let (Some(card), Some(adjustment)) = (card, grid.vadjustment()) {
            let columns = (grid.width() as f32 / card.width()).round().max(1.0) as u32;
            let top = f64::from(position / columns) * f64::from(card.height());
            adjustment.set_value(top.min(adjustment.upper() - adjustment.page_size()));
        }
        grid.scroll_to(position, gtk::ListScrollFlags::FOCUS, None);
    }

    /// The Artists section: the artists by name, each a card in the grid
    /// (picture or initials, name, number of albums). From the server's list
    /// when it is there, else from the albums in memory.
    fn refresh_artists(&self) {
        let imp = self.imp();

        // (id, name, album count, picture)
        let mut artists: Vec<(String, String, u32, Option<String>)> = {
            let listed = imp.artists.borrow();
            if listed.is_empty() {
                // Grouped by artist id; albums without one are left out.
                let current = imp.current.borrow();
                let mut by_id: std::collections::HashMap<&str, (&str, u32)> =
                    std::collections::HashMap::new();
                for album in current.iter() {
                    let (Some(id), Some(name)) =
                        (album.artist_id.as_deref(), album.artist.as_deref())
                    else {
                        continue;
                    };
                    by_id.entry(id).or_insert((name, 0)).1 += 1;
                }
                by_id
                    .into_iter()
                    .map(|(id, (name, count))| (id.to_owned(), name.to_owned(), count, None))
                    .collect()
            } else {
                listed
                    .iter()
                    .map(|a| {
                        (
                            a.id.clone(),
                            a.name.clone(),
                            a.album_count.unwrap_or(0),
                            a.cover_art.clone(),
                        )
                    })
                    .collect()
            }
        };
        // Names that differ only in accents or case keep one order of their own.
        artists.sort_by_cached_key(|(_, name, _, _)| (artist_sort_key(name), name.to_lowercase()));

        // The same artists as now: nothing to rebuild.
        let shown: Vec<(String, u32, Option<String>)> = artists
            .iter()
            .map(|(id, _, count, picture)| (id.clone(), *count, picture.clone()))
            .collect();
        if *imp.artists_shown.borrow() == shown {
            return;
        }
        imp.artists_shown.replace(shown);

        let count = artists.len();
        // Where each letter begins: its button above the grid scrolls there,
        // and the card there shows the letter.
        while let Some(button) = imp.artists_letters.first_child() {
            imp.artists_letters.remove(&button);
        }
        let mut current: Option<String> = None;
        let mut listed = std::collections::HashSet::new();
        let mut objects = Vec::with_capacity(count);
        for (position, (id, name, albums, picture)) in artists.into_iter().enumerate() {
            let letter = artist_letter(&artist_sort_key(&name));
            let first = current.as_deref() != Some(letter.as_str());
            // A letter can begin twice ("#": digits sort before the letters,
            // some symbols after them); its button goes to the first place.
            if first && listed.insert(letter.clone()) {
                let button = gtk::Button::builder()
                    .label(&letter)
                    .css_classes(["flat", "artists-letter"])
                    .build();
                let view = self.downgrade();
                button.connect_clicked(move |_| {
                    if let Some(view) = view.upgrade() {
                        view.scroll_to_artist(position as u32);
                    }
                });
                imp.artists_letters.append(&button);
            }
            if first {
                current = Some(letter.clone());
            }
            objects.push(glib::BoxedAnyObject::new(ArtistItem {
                id,
                name,
                albums,
                picture,
                letter: first.then_some(letter),
            }));
        }
        if let Some(store) = imp.artist_items.get() {
            // One change notification for the whole list, not one per artist.
            store.splice(0, store.n_items(), &objects);
        }
        imp.artists_page.set_badge_number(count as u32);
        imp.artists_stack
            .set_visible_child_name(if count == 0 { "empty" } else { "list" });
    }

    /// The playlists as last fetched (for "Add to Playlist").
    pub(crate) fn playlists(&self) -> Vec<subsonic::Playlist> {
        self.imp().playlists.borrow().clone()
    }

    /// Fetches the playlists; the list is rebuilt from them.
    pub(crate) fn sync_playlists(&self) {
        let Some(client) = self.imp().client.borrow().clone() else {
            return;
        };
        let generation = self.imp().sync_generation.get();
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn(async move { client.playlists().await })
                .await;
            let Some(view) = view
                .upgrade()
                .filter(|v| v.imp().sync_generation.get() == generation)
            else {
                return;
            };
            match result {
                Ok(Ok(playlists)) => {
                    view.imp().playlists.replace(playlists);
                    view.refresh_playlists();
                }
                Ok(Err(e)) => eprintln!("Could not fetch the playlists: {e}"),
                Err(_) => {}
            }
        });
    }

    /// The Favorites page gets its header: the first thing in its list, so it
    /// scrolls away with the albums and the songs. In a narrow page the
    /// header is smaller.
    fn setup_favorites(&self) {
        let imp = self.imp();
        let header = crate::favorites_header::FavoritesHeader::new();
        imp.favorites.set_header_top(Some(&header));
        imp.favorites.set_show_counts(true);
        // Recently starred, title or artist: the songs are sorted again
        // right away (show_favorites).
        let sort = adw::ToggleGroup::builder()
            .valign(gtk::Align::Center)
            .build();
        for label in [
            gettext("Recently Starred"),
            gettext("Title"),
            gettext("Artist"),
        ] {
            sort.add(adw::Toggle::builder().label(label).build());
        }
        sort.set_active(0);
        let view = self.downgrade();
        sort.connect_active_notify(move |_| {
            if let Some(view) = view.upgrade() {
                view.show_favorites();
            }
        });
        imp.favorites.set_songs_end(Some(&sort));
        let _ = imp.favorites_sort.set(sort);

        // A narrow page: a smaller header, less room around the list, the
        // sort switch under the heading, and no dates on the rows.
        let view = self.downgrade();
        imp.favorites_narrow.connect_apply(move |_| {
            if let Some(view) = view.upgrade() {
                view.set_favorites_narrow(true);
            }
        });
        let view = self.downgrade();
        imp.favorites_narrow.connect_unapply(move |_| {
            if let Some(view) = view.upgrade() {
                view.set_favorites_narrow(false);
            }
        });
        // The page takes the colour of the header's first cover, fading
        // into its own background below the header.
        if imp.favorites_layer.get().is_none()
            && let Some(layer) = crate::tint_layer::TintLayer::under(&imp.favorites_toolbar.get())
        {
            let _ = imp.favorites_layer.set(layer);
        }
        let view = self.downgrade();
        header.connect_color(move |color| {
            if let Some(view) = view.upgrade()
                && let Some(layer) = view.imp().favorites_layer.get()
            {
                layer.set_page(color);
            }
        });
        let _ = imp.favorites_header.set(header);
    }

    fn set_favorites_narrow(&self, narrow: bool) {
        let imp = self.imp();
        imp.favorites_is_narrow.set(narrow);
        if let Some(header) = imp.favorites_header.get() {
            header.set_compact(narrow);
        }
        imp.favorites.set_narrow(narrow);
        self.set_favorite_dates();
    }

    /// The date each song was starred, next to its length: only while the
    /// songs are sorted by it, and only where there is room for it.
    fn set_favorite_dates(&self) {
        let imp = self.imp();
        let by_date = imp
            .favorites_sort
            .get()
            .is_none_or(|sort| sort.active() == 0);
        if by_date && !imp.favorites_is_narrow.get() {
            imp.favorites.set_note(Some(|song: &subsonic::Song| {
                song.starred
                    .as_deref()
                    .map(crate::dates::starred_ago)
                    .unwrap_or_default()
            }));
        } else {
            imp.favorites
                .set_note(None::<fn(&subsonic::Song) -> String>);
        }
    }

    /// Shows the favourites as last read, the songs in the order the switch
    /// asks for. The header gets the same order: Play Songs plays them as
    /// listed.
    fn show_favorites(&self) {
        let imp = self.imp();
        let (albums, mut songs, client) = imp.favorite_content.borrow().clone();
        match imp.favorites_sort.get().map_or(0, |sort| sort.active()) {
            // As read: most recently starred first.
            0 => {}
            1 => songs.sort_by_cached_key(|song| artist_sort_key(&song.title)),
            _ => songs.sort_by_cached_key(|song| {
                (
                    artist_sort_key(song.artist.as_deref().unwrap_or("")),
                    artist_sort_key(&song.title),
                )
            }),
        }
        if let Some(header) = imp.favorites_header.get() {
            header.set_content(&albums, &songs, client.clone());
        }
        self.set_favorite_dates();
        imp.favorites.show(&albums, songs, client);
    }

    /// The playlist cards go into two columns where the section is wide
    /// enough for them, else into one: the grid is laid out again whenever
    /// that changes.
    fn setup_playlists(&self) {
        // The number of columns follows the width of the section itself (its
        // breakpoint), not the window or the queue next to it.
        let view = self.downgrade();
        self.imp().playlists_wide.connect_apply(move |_| {
            if let Some(view) = view.upgrade() {
                view.imp().playlists_two_columns.set(true);
                view.refresh_playlists();
            }
        });
        let view = self.downgrade();
        self.imp().playlists_wide.connect_unapply(move |_| {
            if let Some(view) = view.upgrade() {
                view.imp().playlists_two_columns.set(false);
                view.refresh_playlists();
            }
        });
    }

    /// The Playlists section: a wide card per playlist (cover, name, how many
    /// tracks and how long, the description) with a Play button; a click on
    /// the card opens the playlist page.
    fn refresh_playlists(&self) {
        let imp = self.imp();
        let playlists = imp.playlists.borrow().clone();
        let client = imp.client.borrow().clone();
        let columns: usize = if imp.playlists_two_columns.get() {
            2
        } else {
            1
        };

        while let Some(child) = imp.playlists_grid.first_child() {
            imp.playlists_grid.remove(&child);
        }
        for (index, playlist) in playlists.iter().enumerate() {
            let count = playlist.song_count.unwrap_or(0);
            let mut parts = vec![crate::format::tracks_text(count)];
            if let Some(seconds) = playlist.duration.filter(|d| *d > 0) {
                parts.push(crate::format::long_duration(seconds));
            }

            // The cover lies over a 96 px icon, which gives the frame its
            // size, and is made as small as it is on screen (cover_image.rs).
            let cover = crate::cover_image::CoverImage::new();
            let cover_frame = gtk::Overlay::builder()
                .child(
                    &gtk::Image::builder()
                        .pixel_size(96)
                        .icon_name("audio-x-generic-symbolic")
                        .build(),
                )
                .valign(gtk::Align::Start)
                .overflow(gtk::Overflow::Hidden)
                .css_classes(["playlist-cover"])
                .build();
            cover_frame.add_overlay(&cover);
            if let Some(id) = playlist.cover_art.clone() {
                covers::load_for(&cover, client.clone(), id, |cover, texture| {
                    if texture.is_some() {
                        cover.set_texture(texture.as_ref());
                    }
                });
            }

            // A width limit, or the card would ask for the whole name in one
            // line and the flow box would give every card that width.
            let name = gtk::Label::builder()
                .label(playlist.name.as_str())
                .tooltip_text(playlist.name.as_str())
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .width_chars(10)
                .max_width_chars(28)
                .css_classes(["title-4"])
                .build();
            let numbers = gtk::Label::builder()
                .label(parts.join(" · "))
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .css_classes(["dim-label"])
                .build();
            let text = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(4)
                .hexpand(true)
                .valign(gtk::Align::Center)
                .build();
            text.append(&name);
            text.append(&numbers);
            if let Some(comment) = playlist.comment.as_deref().filter(|c| !c.trim().is_empty()) {
                let comment = gtk::Label::builder()
                    .label(comment)
                    .xalign(0.0)
                    .ellipsize(gtk::pango::EllipsizeMode::End)
                    .css_classes(["caption", "dim-label"])
                    .build();
                text.append(&comment);
            }

            // Play: the whole playlist, without opening it.
            let play = gtk::Button::builder()
                .icon_name("media-playback-start-symbolic")
                .tooltip_text(gettext("Play"))
                .valign(gtk::Align::Center)
                .css_classes(["circular", "playlist-play"])
                .build();
            {
                let id = playlist.id.clone();
                let view = self.downgrade();
                play.connect_clicked(move |_| {
                    if let Some(view) = view.upgrade() {
                        view.play_playlist(&id);
                    }
                });
            }

            let row = gtk::Box::builder().spacing(16).build();
            row.append(&cover_frame);
            row.append(&text);
            row.append(&play);
            // The text expands inside the card, but the card itself must not:
            // hexpand-set stops it from inheriting the expansion, or the flow
            // box measures the cards too wide and fits only one per row.
            let card = gtk::Button::builder()
                .child(&row)
                .hexpand(true)
                .css_classes(["card", "playlist-card"])
                .build();
            {
                let id = playlist.id.clone();
                let name = playlist.name.clone();
                let view = self.downgrade();
                card.connect_clicked(move |button| {
                    if let Some(view) = view.upgrade() {
                        view.open_playlist_from(button, &id, &name);
                    }
                });
            }
            imp.playlists_grid.attach(
                &card,
                (index % columns) as i32,
                (index / columns) as i32,
                1,
                1,
            );
        }
        imp.playlists_page.set_badge_number(playlists.len() as u32);
        imp.playlists_stack
            .set_visible_child_name(if playlists.is_empty() {
                "empty"
            } else {
                "list"
            });
    }

    /// Plays a playlist from its first song, fetched from the server.
    fn play_playlist(&self, id: &str) {
        let Some(client) = self.imp().client.borrow().clone() else {
            return;
        };
        let id = id.to_owned();
        let command = crate::player::get().command_id();
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            let request = client.clone();
            let fetched = runtime()
                .spawn(async move { request.playlist(&id).await })
                .await;
            let playlist = match fetched {
                Ok(Ok(playlist)) => playlist,
                failed => {
                    let message = match failed {
                        Ok(Err(e)) => {
                            eprintln!("Could not fetch the playlist: {e}");
                            error_message(&e)
                        }
                        _ => gettext("Something went wrong. Please try again."),
                    };
                    if let Some(view) = view.upgrade() {
                        crate::toast(&view, &message);
                    }
                    return;
                }
            };
            // Something else was started meanwhile: this answer is stale.
            if !crate::player::get().is_current(command) || playlist.entry.is_empty() {
                return;
            }
            crate::player::get().play(
                client,
                playlist.entry.into(),
                0,
                crate::player::QueueOf::Songs,
            );
        });
    }

    /// Opens a playlist page in the section `from` belongs to, as
    /// open_artist_from does for artists.
    fn open_playlist_from(&self, from: &impl IsA<gtk::Widget>, id: &str, name: &str) {
        let imp = self.imp();
        let Some(client) = imp.client.borrow().clone() else {
            return;
        };
        let nav = from
            .ancestor(adw::NavigationView::static_type())
            .and_downcast::<adw::NavigationView>()
            .unwrap_or_else(|| imp.library_nav.get());

        if !back_to::<crate::playlist_page::PlaylistPage>(&nav, |page| page.playlist_id() == id) {
            nav.push(&crate::playlist_page::PlaylistPage::new(id, name, client));
        }
    }

    /// Refreshes the Start page from the albums in memory.
    fn refresh_start(&self) {
        let imp = self.imp();
        imp.start_page
            .update(&imp.current.borrow(), imp.client.borrow().clone());
        self.refresh_start_state();
    }

    /// Start before the first album: syncing, or no connection.
    fn refresh_start_state(&self) {
        let imp = self.imp();
        let offline = matches!(*imp.connection.borrow(), Connection::Offline(_));
        imp.start_page
            .show_state(imp.current.borrow().is_empty(), imp.syncing.get(), offline);
    }

    /// The search field in the sidebar: results update while typing; going
    /// back from the results page (or emptying the field) closes them.
    fn setup_search(&self) {
        let imp = self.imp();
        let view = self.downgrade();
        imp.search_entry.connect_search_changed(move |entry| {
            if let Some(view) = view.upgrade() {
                view.run_search(&entry.text());
            }
        });

        // Typing anywhere in the main view starts a search: keys that nothing
        // else wants go to the field, which then takes the focus (and slides
        // the sidebar in on narrow windows), so the rest is typed into it.
        // Shortcuts are not text, and a field with the focus keeps its keys.
        imp.search_entry.set_key_capture_widget(Some(self));
        let view = self.downgrade();
        imp.search_entry.connect_search_started(move |_| {
            if let Some(view) = view.upgrade() {
                view.focus_search();
            }
        });

        let entry = imp.search_entry.downgrade();
        imp.library_nav.connect_popped(move |_, page| {
            if page.is::<crate::search_page::SearchPage>()
                && let Some(entry) = entry.upgrade()
            {
                entry.set_text("");
            }
        });
    }

    fn run_search(&self, text: &str) {
        let imp = self.imp();
        let page = imp
            .search_page
            .get_or_init(crate::search_page::SearchPage::new)
            .clone();
        let nav = imp.library_nav.get();
        let page_widget = page.upcast_ref::<adw::NavigationPage>();

        // An empty field closes the results (if they are on top).
        if text.trim().is_empty() {
            if nav.visible_page().as_ref() == Some(page_widget) {
                nav.pop();
            }
            return;
        }

        // The results go on top of the library, once; typing more updates them.
        imp.views.set_visible_child_name("library");
        let stack = nav.navigation_stack();
        let on_stack = (0..stack.n_items()).any(|i| {
            stack
                .item(i)
                .and_downcast::<crate::search_page::SearchPage>()
                .is_some()
        });
        if !on_stack {
            // Straight in, without the slide: the results show as soon as
            // they are there (the slide added about a quarter of a second).
            let animate = nav.is_animate_transitions();
            nav.set_animate_transitions(false);
            nav.push(&page);
            nav.set_animate_transitions(animate);
        } else if nav.visible_page().as_ref() != Some(page_widget) {
            nav.pop_to_page(&page);
        }
        self.search_library(text);
    }

    /// The results of `text` on the search page. The library is searched in
    /// the local database: no connection is needed, only the account whose
    /// library is on screen. The client, once there, lets a song play and a
    /// missing cover come.
    fn search_library(&self, text: &str) {
        let imp = self.imp();
        let Some(page) = imp.search_page.get().cloned() else {
            return;
        };
        let server = imp.server.borrow().clone();
        if server.is_empty() {
            return;
        }
        let client = imp.client.borrow().clone();
        let generation = imp.search_generation.get() + 1;
        imp.search_generation.set(generation);
        let query = text.to_owned();
        let account = server.clone();
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn_blocking(move || library::search(&server, &query, 30))
                .await;
            let Some(view) = view.upgrade() else {
                return;
            };
            // A newer search has started meanwhile, or another account is on
            // screen: these results are not wanted.
            let imp = view.imp();
            if imp.search_generation.get() != generation || *imp.server.borrow() != account {
                return;
            }
            let Ok(Ok(results)) = result else {
                return;
            };
            let albums: Vec<Arc<subsonic::Album>> = {
                let current = view.imp().current.borrow();
                results
                    .album_ids
                    .iter()
                    .filter_map(|id| current.iter().find(|a| a.id == *id).cloned())
                    .collect()
            };
            page.show(&albums, results.songs, client);
        });
    }

    /// Ctrl+F: to the search field, sliding the sidebar in on narrow windows.
    pub(crate) fn focus_search(&self) {
        let imp = self.imp();
        if imp.split.is_collapsed() {
            imp.split.set_show_sidebar(true);
        }
        imp.search_entry.grab_focus();
    }

    /// Which session this is: bumped by a sync and by logging out, so a
    /// connection attempt started earlier can tell that its answer is stale.
    pub(crate) fn session(&self) -> u64 {
        self.imp().sync_generation.get()
    }

    /// Logging out: the pages opened for this account go, together with the
    /// client they would play and star with, and running syncs drop their
    /// results instead of showing or saving them.
    pub(crate) fn log_out(&self) {
        let imp = self.imp();
        self.cancel_retry();
        imp.retry_attempt.set(0);
        imp.user_retrying.set(false);
        imp.sync_generation.set(imp.sync_generation.get() + 1);
        imp.client.replace(None);
        self.set_connection(Connection::None);
        imp.server.replace(String::new());
        imp.synced.set(false);
        // The stars changed in this session belonged to that account.
        crate::favorites::forget();
        imp.artists.replace(Vec::new());
        imp.playlists.replace(Vec::new());
        self.refresh_playlists();
        self.set_syncing(false);
        // The old account's library leaves the screen; the next login shows
        // its own (load_cached, then the sync). The database keeps both.
        imp.current.replace(Vec::new());
        imp.albums_page.set_badge_number(0);
        self.refresh_grid();
        self.refresh_artists();
        self.refresh_start();
        imp.favorite_content.replace(Default::default());
        if let Some(header) = imp.favorites_header.get() {
            header.set_content(&[], &[], None);
        }
        imp.favorites.show(&[], Vec::new(), None);
        // Every section (Start, Albums, Favorites...) has a navigation view of
        // its own, so all of them go back to their first page.
        pop_all_to_root(self.upcast_ref());
    }

    /// The library is as at the last full sync: only what changes without a
    /// scan comes - the stars (into the database, then all that shows them),
    /// the playlists and the artists (kept in memory only).
    fn sync_light(&self, client: subsonic::Client, generation: u64) {
        eprintln!(
            "Library not scanned since the last full sync: stars, playlists and artists only"
        );
        let server = client.server_key();
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            let started = Instant::now();
            let fetched = runtime().spawn(async move { client.starred().await }).await;
            let saved = match fetched {
                Ok(Ok(starred)) => {
                    let account = server.clone();
                    runtime()
                        .spawn_blocking(move || {
                            library::set_stars(&account, &starred)?;
                            library::mark_checked(&account)?;
                            library::load_albums(&account)
                        })
                        .await
                        .map_err(|e| e.to_string())
                        .and_then(|saved| saved.map_err(|e| e.to_string()))
                }
                Ok(Err(e)) => Err(e.to_string()),
                Err(e) => Err(e.to_string()),
            };
            let Some(view) = view
                .upgrade()
                .filter(|v| v.imp().sync_generation.get() == generation)
            else {
                return;
            };
            match saved {
                Ok(albums) => {
                    eprintln!("Synced the stars in {:.2?}", started.elapsed());
                    // The albums as stored now, with their stars: what was read
                    // from the database at startup must not replace them.
                    view.imp().synced.set(true);
                    view.show_albums(albums.into_iter().map(Arc::new).collect());
                    view.read_favorites(Some(view.imp().stars_since.get()));
                }
                Err(e) => eprintln!("Could not sync the stars: {e}"),
            }
            view.sync_playlists();
            view.sync_artists();
            view.set_syncing(false);
        });
    }

    /// Syncs albums and then songs now (Preferences), as after connecting.
    /// While a sync runs, another click does nothing: the same account would
    /// only be fetched twice, and the first result thrown away.
    pub(crate) fn sync_now(&self) {
        if self.imp().syncing.get() {
            return;
        }
        // Taken out first: in `if let`, the borrow would last for the whole block,
        // while sync stores the client (borrow_mut).
        let client = self.imp().client.borrow().clone();
        match client {
            // Offline: connecting comes first (then the usual sync).
            Some(_) | None if self.is_offline() => self.retry_now(false),
            Some(client) => self.sync(client, true),
            None => {}
        }
    }

    /// The Favorites page: starred albums and songs from the local database,
    /// read in the background; refreshed with the library. Like a search, it
    /// needs no connection (run_search).
    pub(crate) fn refresh_favorites(&self) {
        self.read_favorites(None);
    }

    /// refresh_favorites; after a sync that started at `settle`
    /// (favorites::changes), the stars known in this session settle on the
    /// stored ones too.
    fn read_favorites(&self, settle: Option<u64>) {
        let imp = self.imp();
        let server = imp.server.borrow().clone();
        if server.is_empty() {
            return;
        }
        let client = imp.client.borrow().clone();
        let account = server.clone();
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn_blocking(move || library::load_starred(&server))
                .await;
            let (Some(view), Ok(Ok(found))) = (view.upgrade(), result) else {
                return;
            };
            // Another account is on screen now (logged out meanwhile).
            if *view.imp().server.borrow() != account {
                return;
            }
            if let Some(since) = settle {
                let albums = found.album_ids.iter().cloned().collect();
                let songs = found.songs.iter().map(|song| song.id.clone()).collect();
                crate::favorites::settle(since, &albums, &songs);
            }
            let albums: Vec<Arc<subsonic::Album>> = {
                let current = view.imp().current.borrow();
                found
                    .album_ids
                    .iter()
                    .filter_map(|id| current.iter().find(|a| a.id == *id).cloned())
                    .collect()
            };
            view.imp()
                .favorite_content
                .replace((albums, found.songs, client));
            view.show_favorites();
        });
    }

    /// The sync button shows a spinner while a sync runs, and cannot be clicked
    /// again until it is over.
    fn set_syncing(&self, syncing: bool) {
        let imp = self.imp();
        imp.syncing.set(syncing);
        // The refresh icon itself turns while a sync runs (style.css: .syncing).
        if syncing {
            imp.sync_button.add_css_class("syncing");
        } else {
            imp.sync_button.remove_css_class("syncing");
        }
        // Stays sensitive: an insensitive button would dim the spinner; a second
        // click is ignored by sync_now anyway.
        self.refresh_start_state();
        self.emit_by_name::<()>("library-changed", &[]);
    }

    /// True while albums and then songs are being synced.
    pub(crate) fn is_syncing(&self) -> bool {
        self.imp().syncing.get()
    }

    /// The account ("user@url") whose library is on screen, or empty.
    pub(crate) fn server_key(&self) -> String {
        self.imp().server.borrow().clone()
    }

    /// How many artists the server listed; 0 until they are fetched.
    pub(crate) fn artist_count(&self) -> usize {
        self.imp().artists.borrow().len()
    }

    /// Runs `f` when a sync starts or ends and when the artists arrive,
    /// until the handler is disconnected.
    pub(crate) fn connect_library_changed<F: Fn(&Self) + 'static>(
        &self,
        f: F,
    ) -> glib::SignalHandlerId {
        self.connect_local("library-changed", false, move |values| {
            if let Ok(view) = values[0].get::<Self>() {
                f(&view);
            }
            None
        })
    }

    /// Shows or hides the queue panel on the right.
    pub(crate) fn toggle_queue(&self) {
        let split = &self.imp().queue_split;
        split.set_show_sidebar(!split.shows_sidebar());
    }

    /// The sidebar: choosing the section already shown goes back to its main
    /// page (an album opened from Start leaves, and the main page keeps its
    /// scroll, as with Back). Another section shows as it was left. On a
    /// narrow window any choice folds the sidebar away, uncovering the page.
    fn setup_sections(&self) {
        let imp = self.imp();
        // Choosing another item switches the stack in the same click or key
        // press that then activates it; the mark lasts until the loop is idle.
        let switched = Rc::new(Cell::new(false));
        {
            let switched = switched.clone();
            imp.views.connect_visible_child_notify(move |_| {
                switched.set(true);
                let switched = switched.clone();
                glib::idle_add_local_once(move || switched.set(false));
            });
        }
        let view = self.downgrade();
        imp.sections.connect_activated(move |_| {
            let Some(view) = view.upgrade() else {
                return;
            };
            let imp = view.imp();
            if imp.split.is_collapsed() {
                imp.split.set_show_sidebar(false);
            }
            if switched.get() {
                return;
            }
            if let Some(nav) = imp
                .views
                .visible_child()
                .and_downcast::<adw::NavigationView>()
            {
                pop_to_root(&nav);
            }
        });
    }

    /// The queue panel is pinned (main_view.blp), so it stays as it was left.
    /// On a narrow window it hides, as it would cover the page; widening
    /// brings it back if it was open before, or was opened meanwhile.
    fn setup_queue(&self) {
        let was_open = Cell::new(false);
        self.imp()
            .queue_split
            .connect_collapsed_notify(move |split| {
                if split.is_collapsed() {
                    was_open.set(split.shows_sidebar());
                    split.set_show_sidebar(false);
                } else {
                    split.set_show_sidebar(was_open.get() || split.shows_sidebar());
                }
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn album(
        name: &str,
        artist: &str,
        year: Option<i32>,
        created: Option<&str>,
    ) -> Arc<subsonic::Album> {
        Arc::new(subsonic::Album {
            id: name.into(),
            name: name.into(),
            artist: Some(artist.into()),
            year,
            song_count: None,
            created: created.map(Into::into),
            artist_id: None,
            starred: None,
            cover_art: None,
        })
    }

    fn sorted(mode: &str) -> Vec<String> {
        let mut albums = vec![
            album("beta", "Zed", Some(2001), Some("2026-02-01T00:00:00Z")),
            album("Alpha", "abba", None, Some("2026-03-01T00:00:00Z")),
            album("gamma", "Abba", Some(1999), None),
            album("Delta", "Zed", Some(1999), Some("2026-01-01T00:00:00Z")),
        ];
        sort_albums(&mut albums, mode);
        albums.iter().map(|a| a.name.clone()).collect()
    }

    #[test]
    fn albums_sort_by_name_artist_year_or_date_added() {
        assert_eq!(sorted("name"), ["Alpha", "beta", "Delta", "gamma"]);
        // By artist (case ignored), then year, then name.
        assert_eq!(sorted("artist"), ["Alpha", "gamma", "Delta", "beta"]);
        // Newest first, the albums without a year last; oldest first, the same.
        assert_eq!(sorted("year-desc"), ["beta", "Delta", "gamma", "Alpha"]);
        assert_eq!(sorted("year-asc"), ["Delta", "gamma", "beta", "Alpha"]);
        // Added last first; without a date, at the end.
        assert_eq!(sorted("added"), ["Alpha", "beta", "Delta", "gamma"]);
    }

    #[test]
    fn artists_sort_and_letter_without_accents() {
        assert_eq!(artist_sort_key("Óscar"), "oscar");
        assert_eq!(artist_sort_key("Łukasz"), "lukasz");
        assert_eq!(artist_sort_key("Åsa"), "asa");
        assert_eq!(artist_letter(&artist_sort_key("Ølfred")), "O");
        assert_eq!(artist_letter(&artist_sort_key("2Pac")), "#");
    }
}

//! Main view: the album grid, plus the connection state and an offline
//! banner with a Retry button.

use std::sync::Arc;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::{gio, glib};
use std::cell::{Cell, OnceCell, RefCell};
use std::time::Instant;

use crate::album_card::AlbumCard;
use crate::album_page::AlbumPage;
use crate::artist_page::ArtistPage;
use crate::{covers, error_message, library, runtime, since_start, APP_ID, APP_NAME};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/main_view.blp")]
    pub struct MainView {
        #[template_child]
        pub banner: gtk::TemplateChild<adw::Banner>,
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
        pub library_nav: gtk::TemplateChild<adw::NavigationView>,
        #[template_child]
        pub albums_page: gtk::TemplateChild<adw::ViewStackPage>,
        #[template_child]
        pub artists_page: gtk::TemplateChild<adw::ViewStackPage>,
        #[template_child]
        pub artists_stack: gtk::TemplateChild<gtk::Stack>,
        #[template_child]
        pub artists_box: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub playlists_page: gtk::TemplateChild<adw::ViewStackPage>,
        #[template_child]
        pub playlists_stack: gtk::TemplateChild<gtk::Stack>,
        #[template_child]
        pub playlists_grid: gtk::TemplateChild<gtk::Grid>,
        /// The playlists as the server lists them.
        pub playlists: RefCell<Vec<subsonic::Playlist>>,
        /// The artists as the server lists them (with their pictures); empty
        /// until the sync, when the albums in memory stand in.
        pub artists: RefCell<Vec<subsonic::Artist>>,
        /// What the grid shows now (id, album count, picture), so an unchanged
        /// grid is not rebuilt.
        pub artists_shown: RefCell<Vec<(String, u32, Option<String>)>>,
        #[template_child]
        pub search_entry: gtk::TemplateChild<gtk::SearchEntry>,
        #[template_child]
        pub sync_button: gtk::TemplateChild<gtk::Button>,
        /// True while albums and then songs are being synced.
        pub syncing: std::cell::Cell<bool>,
        /// Bumped by every sync; a sync that is no longer the newest (another
        /// account was connected meanwhile) drops its results.
        pub sync_generation: std::cell::Cell<u64>,
        /// The search results, made on first use and reused.
        pub search_page: std::cell::OnceCell<crate::search_page::SearchPage>,
        /// Bumped by every search; results of an older one are dropped.
        pub search_generation: std::cell::Cell<u32>,
        #[template_child]
        pub start_page: gtk::TemplateChild<crate::start_page::StartPage>,
        #[template_child]
        pub favorites: gtk::TemplateChild<crate::results_view::ResultsView>,
        /// How the grid is sorted (see sort_albums).
        pub sort: RefCell<String>,
        /// Albums shown in the grid; items are subsonic::Album wrapped for GTK.
        pub albums: OnceCell<gio::ListStore>,
        /// A copy of what the grid shows, to skip redraws when nothing changed.
        pub current: RefCell<Vec<Arc<subsonic::Album>>>,
        /// The connected client, used by cards to fetch their covers.
        pub client: RefCell<Option<subsonic::Client>>,
        /// Every card the grid created (only as many as fit on screen, plus a few).
        pub cards: RefCell<Vec<glib::WeakRef<AlbumCard>>>,
        /// Whether the startup time was already reported.
        pub reported: Cell<bool>,
        /// The account ("user@url") whose library is on screen, or empty.
        pub server: RefCell<String>,
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

    impl ObjectImpl for MainView {}
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
        view.setup_queue();
        view.setup_playlists();
        view.setup_sorting();
        view.setup_search();
        view.imp().favorites.set_empty(
            "starred-symbolic",
            &gettext("No Favorites"),
            &gettext("Star albums and songs to see them here"),
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
                crate::album_page::open(&nav, &album, client);
            }
        });

        imp.grid
            .set_model(Some(&gtk::NoSelection::new(Some(albums.clone()))));
        imp.grid.set_factory(Some(&factory));
        let _ = imp.albums.set(albums);
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
        self.imp()
            .status
            .set_description(Some(&gettext("Connecting…")));
    }

    /// Called after a successful ping; syncs the album list in the background.
    pub(crate) fn show_connected(&self, r: &subsonic::Response, client: subsonic::Client) {
        let imp = self.imp();
        imp.banner.set_revealed(false);
        // Translators: {server} is the server software (e.g. navidrome), {version} its version.
        let text = gettext("Connected to {server} {version}")
            .replace("{server}", r.server_type.as_deref().unwrap_or("server"))
            .replace("{version}", r.server_version.as_deref().unwrap_or(""));
        imp.status.set_description(Some(&text));
        self.sync_albums(client);
    }

    pub(crate) fn show_offline(&self, message: &str) {
        let imp = self.imp();
        imp.banner.set_title(message);
        imp.banner.set_revealed(true);
        imp.status.set_description(Some(&gettext("Offline")));
    }

    /// Runs `f` when the user presses Retry in the offline banner.
    pub(crate) fn connect_retry<F: Fn() + 'static>(&self, f: F) {
        self.imp().banner.connect_button_clicked(move |_| f());
    }

    /// Fetches the album list from the server, shows it and stores it locally.
    fn sync_albums(&self, client: subsonic::Client) {
        // A new sync supersedes a running one (another account, say): the
        // client changes right away, and the older sync drops its results.
        let generation = self.imp().sync_generation.get() + 1;
        self.imp().sync_generation.set(generation);
        self.set_syncing(true);
        self.imp().client.replace(Some(client.clone()));
        // Covers on the Start page can now come from the server too.
        self.refresh_start();
        self.refresh_favorites();
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
                    eprintln!("Synced {} albums in {:.2?}", albums.len(), started.elapsed());
                    // Shared, not copied: the grid and the save below point at the same albums.
                    let albums: Vec<Arc<subsonic::Album>> = albums.into_iter().map(Arc::new).collect();
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
                    view.sync_frequent();
                }
                Ok(Err(e)) if !stale => {
                    view.show_offline(&error_message(&e));
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
}

impl MainView {
    /// Puts the now-playing card at the bottom of the sidebar.
    pub(crate) fn set_now_playing(&self, widget: &impl IsA<gtk::Widget>) {
        self.imp().player_slot.set_child(Some(widget));
    }
}

impl MainView {
    /// Puts the compact now-playing bar under the content (narrow windows).
    pub(crate) fn set_mini_player(&self, widget: &impl IsA<gtk::Widget>) {
        self.imp().mini_slot.set_child(Some(widget));
    }

    /// Slides the sidebar in, with the full now-playing card.
    pub(crate) fn show_sidebar(&self) {
        self.imp().split.set_show_sidebar(true);
    }
}

impl MainView {
    /// Opens an album page in the Albums section, e.g. the album that plays
    /// (from the sidebar card, which belongs to no section).
    pub(crate) fn open_album(&self, id: &str, fallback: subsonic::Album) {
        let imp = self.imp();
        imp.views.set_visible_child_name("library");
        self.open_album_on(&imp.library_nav.get(), id, fallback);
    }

    /// Opens an album page in the section `from` belongs to (Start, Favorites,
    /// search results), so that Back returns there.
    pub(crate) fn open_album_from(
        &self,
        from: &impl IsA<gtk::Widget>,
        id: &str,
        fallback: subsonic::Album,
    ) {
        let nav = from
            .ancestor(adw::NavigationView::static_type())
            .and_downcast::<adw::NavigationView>()
            .unwrap_or_else(|| self.imp().library_nav.get());
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
        let nav = from
            .ancestor(adw::NavigationView::static_type())
            .and_downcast::<adw::NavigationView>()
            .unwrap_or_else(|| imp.library_nav.get());
        if imp.split.is_collapsed() {
            imp.split.set_show_sidebar(false);
        }

        // Already open in this section: go back to it instead of stacking
        // another copy on top.
        let stack = nav.navigation_stack();
        for i in (0..stack.n_items()).rev() {
            if let Some(page) = stack.item(i).and_downcast::<ArtistPage>() {
                if page.artist_id() == id {
                    nav.pop_to_page(&page);
                    return;
                }
            }
        }
        nav.push(&ArtistPage::new(id, name, client));
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

        // Already open in this section: go back to it instead of stacking
        // another copy on top.
        let stack = nav.navigation_stack();
        for i in (0..stack.n_items()).rev() {
            if let Some(page) = stack.item(i).and_downcast::<AlbumPage>() {
                if page.album_id() == id {
                    nav.pop_to_page(&page);
                    return;
                }
            }
        }
        crate::album_page::open(nav, &album, client);
    }
}

impl MainView {
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
        let settings = gio::Settings::new(APP_ID);
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

            // A new order starts from the top.
            let imp = view.imp();
            if imp.albums.get().is_some_and(|store| store.n_items() > 0) {
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
        "year-asc" => {
            albums.sort_by_cached_key(|a| (a.year.unwrap_or(i32::MAX), lower(&a.name)))
        }
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
                .spawn_blocking(move || library::save_songs(&server, &songs))
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
                        view.refresh_favorites();
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

/// Pops every navigation view under `widget` back to its first page.
fn pop_all_to_root(widget: &gtk::Widget) {
    if let Some(nav) = widget.downcast_ref::<adw::NavigationView>() {
        if let Some(root) = nav
            .navigation_stack()
            .item(0)
            .and_downcast::<adw::NavigationPage>()
        {
            nav.pop_to_page(&root);
        }
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
        });
    }

    /// The Artists section: a card per artist (picture or initials, name,
    /// number of albums), grouped under the first letter of the name. From the
    /// server's list when it is there, else from the albums in memory.
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
                        (a.id.clone(), a.name.clone(), a.album_count.unwrap_or(0), a.cover_art.clone())
                    })
                    .collect()
            }
        };
        artists.sort_by_cached_key(|(_, name, _, _)| name.to_lowercase());

        // The same artists as now: nothing to rebuild.
        let shown: Vec<(String, u32, Option<String>)> = artists
            .iter()
            .map(|(id, _, count, picture)| (id.clone(), *count, picture.clone()))
            .collect();
        if *imp.artists_shown.borrow() == shown {
            return;
        }
        imp.artists_shown.replace(shown);

        while let Some(child) = imp.artists_box.first_child() {
            imp.artists_box.remove(&child);
        }
        let client = imp.client.borrow().clone();
        let mut letter: Option<String> = None;
        let mut flow: Option<gtk::FlowBox> = None;
        for (id, name, count, picture) in &artists {
            // A new heading when the first letter changes; digits and the like
            // go under "#".
            let first = name
                .chars()
                .next()
                .filter(|c| c.is_alphabetic())
                .map(|c| c.to_uppercase().to_string())
                .unwrap_or_else(|| "#".to_string());
            if letter.as_deref() != Some(first.as_str()) {
                let heading = gtk::Label::builder()
                    .label(&first)
                    .xalign(0.0)
                    .margin_top(12)
                    .css_classes(["title-2"])
                    .build();
                imp.artists_box.append(&heading);
                let box_ = gtk::FlowBox::builder()
                    .selection_mode(gtk::SelectionMode::None)
                    .homogeneous(true)
                    .column_spacing(12)
                    .row_spacing(12)
                    .max_children_per_line(12)
                    .build();
                imp.artists_box.append(&box_);
                flow = Some(box_);
                letter = Some(first);
            }
            let Some(flow) = flow.as_ref() else {
                continue;
            };

            let avatar = adw::Avatar::new(120, Some(name), true);
            if let (Some(picture), Some(client)) = (picture.clone(), client.clone()) {
                covers::load_for(&avatar, Some(client), picture, |avatar, texture| {
                    if let Some(texture) = texture {
                        avatar.set_custom_image(Some(&texture));
                    }
                });
            }
            let title = gtk::Label::builder()
                .label(name.as_str())
                .justify(gtk::Justification::Center)
                .wrap(true)
                .wrap_mode(gtk::pango::WrapMode::WordChar)
                .lines(2)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(16)
                .css_classes(["heading"])
                .build();
            // Translators: {n} is a number of albums.
            let subtitle = gtk::Label::builder()
                .label(ngettext("{n} album", "{n} albums", *count).replace("{n}", &count.to_string()))
                .css_classes(["caption", "dim-label"])
                .build();
            let card = gtk::Box::builder()
                .orientation(gtk::Orientation::Vertical)
                .spacing(6)
                .build();
            card.append(&avatar);
            card.append(&title);
            card.append(&subtitle);
            let button = gtk::Button::builder()
                .child(&card)
                .css_classes(["flat", "shelf-card"])
                .build();

            let id = id.clone();
            let name = name.clone();
            let view = self.downgrade();
            button.connect_clicked(move |button| {
                let Some(view) = view.upgrade() else {
                    return;
                };
                let Some(client) = view.imp().client.borrow().clone() else {
                    return;
                };
                view.open_artist_from(button, &id, &name, client);
            });
            flow.append(&button);
        }
        imp.artists_page.set_badge_number(artists.len() as u32);
        imp.artists_stack
            .set_visible_child_name(if artists.is_empty() { "empty" } else { "list" });
    }
}

impl MainView {
    /// Asks the server which albums are played most (the Start page shelf).
    fn sync_frequent(&self) {
        let Some(client) = self.imp().client.borrow().clone() else {
            return;
        };
        let generation = self.imp().sync_generation.get();
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn(async move { client.album_list_of("frequent", 24).await })
                .await;
            let Some(view) = view
                .upgrade()
                .filter(|v| v.imp().sync_generation.get() == generation)
            else {
                return;
            };
            match result {
                Ok(Ok(albums)) => {
                    let ids = albums.into_iter().map(|a| a.id).collect();
                    view.imp().start_page.set_frequent(ids);
                }
                Ok(Err(e)) => eprintln!("Could not fetch the most played albums: {e}"),
                Err(_) => {}
            }
        });
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
            let result = runtime().spawn(async move { client.playlists().await }).await;
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

    /// The playlist cards go into two columns, or one on medium and narrow
    /// windows (the breakpoint that also collapses the queue panel): the grid
    /// is laid out again whenever that changes.
    fn setup_playlists(&self) {
        let view = self.downgrade();
        self.imp().queue_split.connect_collapsed_notify(move |_| {
            if let Some(view) = view.upgrade() {
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
        let columns: usize = if imp.queue_split.is_collapsed() { 1 } else { 2 };

        while let Some(child) = imp.playlists_grid.first_child() {
            imp.playlists_grid.remove(&child);
        }
        for (index, playlist) in playlists.iter().enumerate() {
            let count = playlist.song_count.unwrap_or(0);
            let mut parts = vec![
                // Translators: {n} is a number of tracks.
                ngettext("{n} track", "{n} tracks", count).replace("{n}", &count.to_string()),
            ];
            if let Some(seconds) = playlist.duration.filter(|d| *d > 0) {
                parts.push(crate::album_page::format_duration(seconds));
            }

            // The cover as a 96 px thumbnail (covers::thumbnail): an image with
            // the texture itself would ask for the texture's 320 px, and the
            // flow box would then fit only one card per row.
            let cover = gtk::Image::builder()
                .pixel_size(96)
                .icon_name("audio-x-generic-symbolic")
                .build();
            let cover_frame = adw::Bin::builder()
                .child(&cover)
                .valign(gtk::Align::Start)
                .overflow(gtk::Overflow::Hidden)
                .css_classes(["playlist-cover"])
                .build();
            if let Some(id) = playlist.cover_art.clone() {
                covers::load_for(&cover, client.clone(), id, |image, texture| {
                    if let Some(thumb) = texture.as_ref().and_then(|t| covers::thumbnail(t, 96.0)) {
                        image.set_paintable(Some(&thumb));
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
            .set_visible_child_name(if playlists.is_empty() { "empty" } else { "list" });
    }

    /// Plays a playlist from its first song, fetched from the server.
    fn play_playlist(&self, id: &str) {
        let Some(client) = self.imp().client.borrow().clone() else {
            return;
        };
        let id = id.to_owned();
        let command = crate::player::get().command_id();
        glib::spawn_future_local(async move {
            let request = client.clone();
            let fetched = runtime().spawn(async move { request.playlist(&id).await }).await;
            let Ok(Ok(playlist)) = fetched else {
                return;
            };
            // Something else was started meanwhile: this answer is stale.
            if !crate::player::get().is_current(command) || playlist.entry.is_empty() {
                return;
            }
            crate::player::get().play(client, playlist.entry.into(), 0);
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

        // Already open in this section: go back to it instead of stacking
        // another copy on top.
        let stack = nav.navigation_stack();
        for i in (0..stack.n_items()).rev() {
            if let Some(page) = stack.item(i).and_downcast::<crate::playlist_page::PlaylistPage>() {
                if page.playlist_id() == id {
                    nav.pop_to_page(&page);
                    return;
                }
            }
        }
        nav.push(&crate::playlist_page::PlaylistPage::new(id, name, client));
    }
}

impl MainView {
    /// Refreshes the Start page from the albums in memory.
    fn refresh_start(&self) {
        let imp = self.imp();
        imp.start_page
            .update(&imp.current.borrow(), imp.client.borrow().clone());
    }
}

impl MainView {
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

        let entry = imp.search_entry.downgrade();
        imp.library_nav.connect_popped(move |_, page| {
            if page.is::<crate::search_page::SearchPage>() {
                if let Some(entry) = entry.upgrade() {
                    entry.set_text("");
                }
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
        let on_stack = (0..stack.n_items())
            .any(|i| stack.item(i).and_downcast::<crate::search_page::SearchPage>().is_some());
        if !on_stack {
            nav.push(&page);
        } else if nav.visible_page().as_ref() != Some(page_widget) {
            nav.pop_to_page(&page);
        }

        let Some(client) = imp.client.borrow().clone() else {
            return;
        };
        let generation = imp.search_generation.get() + 1;
        imp.search_generation.set(generation);
        let server = client.server_key();
        let query = text.to_owned();
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn_blocking(move || library::search(&server, &query, 30))
                .await;
            let Some(view) = view.upgrade() else {
                return;
            };
            // A newer search has started meanwhile: its results will come.
            if view.imp().search_generation.get() != generation {
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
            page.show(&albums, results.songs, Some(client));
        });
    }
}

impl MainView {
    /// Ctrl+F: to the search field, sliding the sidebar in on narrow windows.
    pub(crate) fn focus_search(&self) {
        let imp = self.imp();
        if imp.split.is_collapsed() {
            imp.split.set_show_sidebar(true);
        }
        imp.search_entry.grab_focus();
    }
}

impl MainView {
    /// Logging out: the pages opened for this account go, together with the
    /// client they would play and star with, and running syncs drop their
    /// results instead of showing or saving them.
    /// Which session this is: bumped by a sync and by logging out, so a
    /// connection attempt started earlier can tell that its answer is stale.
    pub(crate) fn session(&self) -> u64 {
        self.imp().sync_generation.get()
    }

    pub(crate) fn log_out(&self) {
        let imp = self.imp();
        imp.sync_generation.set(imp.sync_generation.get() + 1);
        imp.client.replace(None);
        imp.server.replace(String::new());
        imp.synced.set(false);
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
        imp.favorites.show(&[], Vec::new(), None);
        // Every section (Start, Albums, Favorites...) has a navigation view of
        // its own, so all of them go back to their first page.
        pop_all_to_root(self.upcast_ref());
    }

    /// Syncs albums and then songs now (Preferences), as after connecting.
    /// While a sync runs, another click does nothing: the same account would
    /// only be fetched twice, and the first result thrown away.
    pub(crate) fn sync_now(&self) {
        if self.imp().syncing.get() {
            return;
        }
        // Taken out first: in `if let`, the borrow would last for the whole block,
        // while sync_albums stores the client (borrow_mut).
        let client = self.imp().client.borrow().clone();
        if let Some(client) = client {
            self.sync_albums(client);
        }
    }
}

impl MainView {
    /// The Favorites page: starred albums and songs from the local database,
    /// read in the background; refreshed with the library.
    pub(crate) fn refresh_favorites(&self) {
        let imp = self.imp();
        let Some(client) = imp.client.borrow().clone() else {
            return;
        };
        let server = client.server_key();
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn_blocking(move || library::load_starred(&server))
                .await;
            let (Some(view), Ok(Ok(found))) = (view.upgrade(), result) else {
                return;
            };
            let albums: Vec<Arc<subsonic::Album>> = {
                let current = view.imp().current.borrow();
                found
                    .album_ids
                    .iter()
                    .filter_map(|id| current.iter().find(|a| a.id == *id).cloned())
                    .collect()
            };
            view.imp().favorites.show(&albums, found.songs, Some(client));
        });
    }
}

impl MainView {
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
    }
}

impl MainView {
    /// Shows or hides the queue panel on the right.
    pub(crate) fn toggle_queue(&self) {
        let split = &self.imp().queue_split;
        split.set_show_sidebar(!split.shows_sidebar());
    }

    /// The queue panel is pinned (main_view.blp), so it stays as it was left.
    /// On a narrow window it hides, as it would cover the page; widening
    /// brings it back if it was open before, or was opened meanwhile.
    fn setup_queue(&self) {
        let was_open = Cell::new(false);
        self.imp().queue_split.connect_collapsed_notify(move |split| {
            if split.is_collapsed() {
                was_open.set(split.shows_sidebar());
                split.set_show_sidebar(false);
            } else {
                split.set_show_sidebar(was_open.get() || split.shows_sidebar());
            }
        });
    }
}

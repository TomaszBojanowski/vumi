//! Main view: the album grid, plus the connection state and an offline
//! banner with a Retry button.

use std::sync::Arc;
use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use std::cell::{Cell, OnceCell, RefCell};
use std::time::Instant;

use crate::album_card::AlbumCard;
use crate::album_page::AlbumPage;
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
        pub views: gtk::TemplateChild<adw::ViewStack>,
        #[template_child]
        pub library_nav: gtk::TemplateChild<adw::NavigationView>,
        #[template_child]
        pub albums_page: gtk::TemplateChild<adw::ViewStackPage>,
        #[template_child]
        pub start_page: gtk::TemplateChild<crate::start_page::StartPage>,
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
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MainView {
        // Must match "template $MusePlayerMainView" in the Blueprint file.
        const NAME: &'static str = "MusePlayerMainView";
        type Type = super::MainView;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            // The Start page is used in this template, so its type must exist first.
            crate::start_page::StartPage::static_type();
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
        view.setup_sorting();
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
            let page = AlbumPage::new(&object.borrow::<Arc<subsonic::Album>>(), client);
            if let Some(nav) = grid
                .ancestor(adw::NavigationView::static_type())
                .and_downcast::<adw::NavigationView>()
            {
                nav.push(&page);
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
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn_blocking(move || library::load_albums(&server))
                .await;
            let Some(view) = view.upgrade() else {
                return;
            };
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
        self.imp().client.replace(Some(client.clone()));
        // Covers on the Start page can now come from the server too.
        self.refresh_start();
        // Cards drawn before the connection could not fetch uncached covers:
        // every card that still shows the placeholder tries again now.
        for card in self.imp().cards.borrow().iter().filter_map(|c| c.upgrade()) {
            card.retry_cover(client.clone());
        }
        let server = client.server_key();
        covers::set_server(server.clone());
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            let started = Instant::now();
            let result = runtime()
                .spawn(async move { client.all_albums().await })
                .await;
            let Some(view) = view.upgrade() else {
                return;
            };
            match result {
                Ok(Ok(albums)) => {
                    eprintln!("Synced {} albums in {:.2?}", albums.len(), started.elapsed());
                    // Shared, not copied: the grid and the save below point at the same albums.
                    let albums: Vec<Arc<subsonic::Album>> = albums.into_iter().map(Arc::new).collect();
                    let to_save = albums.clone();
                    runtime().spawn_blocking(move || {
                        if let Err(e) = library::save_albums(&server, &to_save) {
                            eprintln!("Could not save the library database: {e}");
                        }
                    });
                    view.show_albums(albums);
                    // Songs next, in the background: album pages open already filled.
                    view.sync_songs();
                }
                Ok(Err(e)) => view.show_offline(&error_message(&e)),
                Err(_) => {}
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
        self.refresh_start();
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
    /// Opens an album page in the library, e.g. the album that is playing.
    /// Uses the full album from the library if it is there, else `fallback`.
    pub(crate) fn open_album(&self, id: &str, fallback: subsonic::Album) {
        let imp = self.imp();
        let album = imp
            .current
            .borrow()
            .iter()
            .find(|a| a.id == id)
            .cloned()
            .unwrap_or_else(|| Arc::new(fallback));
        let client = imp.client.borrow().clone();

        imp.views.set_visible_child_name("library");
        if imp.split.is_collapsed() {
            imp.split.set_show_sidebar(false);
        }
        // Already open somewhere in the library: go back to it instead of
        // stacking another copy on top.
        let stack = imp.library_nav.navigation_stack();
        for i in (0..stack.n_items()).rev() {
            if let Some(page) = stack.item(i).and_downcast::<AlbumPage>() {
                if page.album_id() == id {
                    imp.library_nav.pop_to_page(&page);
                    return;
                }
            }
        }
        // Already open somewhere in the library: go back to it instead of
        // stacking another copy on top.
        let stack = imp.library_nav.navigation_stack();
        for i in (0..stack.n_items()).rev() {
            if let Some(page) = stack.item(i).and_downcast::<AlbumPage>() {
                if page.album_id() == id {
                    imp.library_nav.pop_to_page(&page);
                    return;
                }
            }
        }
        imp.library_nav.push(&AlbumPage::new(&album, client));
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
        let view = self.downgrade();
        glib::spawn_future_local(async move {
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
            let count = songs.len();
            let saved = runtime()
                .spawn_blocking(move || library::save_songs(&server, &songs))
                .await;
            match saved {
                Ok(Ok(())) => {
                    eprintln!("Synced {count} songs in {:.2?}", started.elapsed());
                    // The "Continue" card may need songs that have only just arrived.
                    if let Some(view) = view.upgrade() {
                        view.refresh_start();
                    }
                }
                Ok(Err(e)) => eprintln!("Could not save songs: {e}"),
                Err(_) => {}
            }
        });
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

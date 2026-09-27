//! Main view: the album grid, plus the connection state and an offline
//! banner with a Retry button.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use std::cell::{Cell, OnceCell, RefCell};
use std::time::Instant;

use crate::album_card::AlbumCard;
use crate::album_page::AlbumPage;
use crate::{covers, error_message, library, runtime, since_start, APP_NAME};

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
        /// Albums shown in the grid; items are subsonic::Album wrapped for GTK.
        pub albums: OnceCell<gio::ListStore>,
        /// A copy of what the grid shows, to skip redraws when nothing changed.
        pub current: RefCell<Vec<subsonic::Album>>,
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
            card.set_album(&object.borrow::<subsonic::Album>(), client);
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
            let page = AlbumPage::new(&object.borrow::<subsonic::Album>(), client);
            if let Some(nav) = view
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
                Ok(Ok(albums)) if !albums.is_empty() => view.show_albums(albums),
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
                    let to_save = albums.clone();
                    runtime().spawn_blocking(move || {
                        if let Err(e) = library::save_albums(&server, &to_save) {
                            eprintln!("Could not save the library database: {e}");
                        }
                    });
                    view.show_albums(albums);
                }
                Ok(Err(e)) => view.show_offline(&error_message(&e)),
                Err(_) => {}
            }
        });
    }

    fn show_albums(&self, albums: Vec<subsonic::Album>) {
        let imp = self.imp();

        // Same list as on screen: nothing to redraw.
        if *imp.current.borrow() == albums {
            return;
        }

        let objects: Vec<glib::BoxedAnyObject> =
            albums.iter().cloned().map(glib::BoxedAnyObject::new).collect();
        if let Some(store) = imp.albums.get() {
            // One change notification for the whole list, not one per album.
            store.splice(0, store.n_items(), &objects);
        }
        imp.current.replace(albums);
        imp.stack.set_visible_child_name("library");

        if !imp.reported.replace(true) {
            eprintln!("Library on screen {:.0?} after start", since_start());
        }
    }
}

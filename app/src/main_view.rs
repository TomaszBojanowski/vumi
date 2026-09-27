//! Main view: the album grid, plus the connection state and an offline
//! banner with a Retry button.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use std::cell::OnceCell;
use std::time::Instant;

use crate::album_card::AlbumCard;
use crate::{error_message, runtime, APP_NAME};

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
        @implements gtk::Buildable, gtk::ConstraintTarget;
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
        factory.connect_setup(|_, item| {
            if let Some(item) = item.downcast_ref::<gtk::ListItem>() {
                item.set_child(Some(&AlbumCard::new()));
            }
        });
        // Called whenever a card is (re)used for a given album.
        factory.connect_bind(|_, item| {
            let Some(item) = item.downcast_ref::<gtk::ListItem>() else {
                return;
            };
            let card = item.child().and_downcast::<AlbumCard>();
            let object = item.item().and_downcast::<glib::BoxedAnyObject>();
            if let (Some(card), Some(object)) = (card, object) {
                card.set_album(&object.borrow::<subsonic::Album>());
            }
        });

        imp.grid
            .set_model(Some(&gtk::NoSelection::new(Some(albums.clone()))));
        imp.grid.set_factory(Some(&factory));
        let _ = imp.albums.set(albums);
    }

    pub(crate) fn show_connecting(&self) {
        self.imp()
            .status
            .set_description(Some(&gettext("Connecting…")));
    }

    /// Called after a successful ping; loads the album list in the background.
    pub(crate) fn show_connected(&self, r: &subsonic::Response, client: subsonic::Client) {
        let imp = self.imp();
        imp.banner.set_revealed(false);
        // Translators: {server} is the server software (e.g. navidrome), {version} its version.
        let text = gettext("Connected to {server} {version}")
            .replace("{server}", r.server_type.as_deref().unwrap_or("server"))
            .replace("{version}", r.server_version.as_deref().unwrap_or(""));
        imp.status.set_description(Some(&text));
        self.load_albums(client);
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

    fn load_albums(&self, client: subsonic::Client) {
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
                    eprintln!("Loaded {} albums in {:.2?}", albums.len(), started.elapsed());
                    view.show_albums(albums);
                }
                Ok(Err(e)) => view.show_offline(&error_message(&e)),
                Err(_) => {}
            }
        });
    }

    fn show_albums(&self, albums: Vec<subsonic::Album>) {
        let imp = self.imp();
        let objects: Vec<glib::BoxedAnyObject> =
            albums.into_iter().map(glib::BoxedAnyObject::new).collect();
        if let Some(store) = imp.albums.get() {
            // One change notification for the whole list, not one per album.
            store.splice(0, store.n_items(), &objects);
        }
        imp.stack.set_visible_child_name("library");
    }
}

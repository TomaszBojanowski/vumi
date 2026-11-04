//! Artist page: photo, name, biography and the artist's albums.
//! The layout lives in artist_page.blp; this file only holds the behaviour.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use std::cell::RefCell;

use crate::album_card::AlbumCard;
use crate::{about, covers, error_message, runtime};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/artist_page.blp")]
    pub struct ArtistPage {
        #[template_child]
        pub avatar: gtk::TemplateChild<adw::Avatar>,
        #[template_child]
        pub name: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub details: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub about: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub about_text: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub about_more: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub albums: gtk::TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub albums_title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub error_status: gtk::TemplateChild<adw::StatusPage>,
        /// The albums in the same order as the cards, to know which one was clicked.
        pub album_list: RefCell<Vec<subsonic::Album>>,
        pub client: RefCell<Option<subsonic::Client>>,
        /// Which artist this page shows, so an open page can be found again.
        pub artist_id: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ArtistPage {
        // Must match "template $VumiArtistPage" in the Blueprint file.
        const NAME: &'static str = "VumiArtistPage";
        type Type = super::ArtistPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ArtistPage {}
    impl WidgetImpl for ArtistPage {}
    impl NavigationPageImpl for ArtistPage {}
}

/// What the library knows of an artist: the page shows it while offline.
#[derive(Default)]
pub(crate) struct LocalArtist {
    pub(crate) albums: Vec<subsonic::Album>,
    /// The cover id of the artist's picture, if the server gave one.
    pub(crate) picture: Option<String>,
}

glib::wrapper! {
    pub struct ArtistPage(ObjectSubclass<imp::ArtistPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ArtistPage {
    /// The id of the artist this page shows.
    pub(crate) fn artist_id(&self) -> String {
        self.imp().artist_id.borrow().clone()
    }

    /// `client` is None while offline: the page shows what the library
    /// knows (`local`), and set_client fills it from the server once it is
    /// back.
    pub(crate) fn new(
        id: &str,
        name: &str,
        client: Option<subsonic::Client>,
        local: LocalArtist,
    ) -> Self {
        let page: Self = glib::Object::new();
        crate::offline_banner::attach(&page);
        let imp = page.imp();

        page.set_title(name);
        imp.name.set_label(name);
        imp.avatar.set_text(Some(name));
        imp.client.replace(client.clone());
        imp.artist_id.replace(id.to_string());

        // A click on an album card opens that album on top of this page.
        let page_weak = page.downgrade();
        imp.albums.connect_child_activated(move |_, child| {
            let Some(page) = page_weak.upgrade() else {
                return;
            };
            let imp = page.imp();
            let album = imp.album_list.borrow().get(child.index() as usize).cloned();
            let Some(album) = album else {
                return;
            };
            // Back to the album's page if it is open already.
            if let Some(main) = page
                .ancestor(crate::main_view::MainView::static_type())
                .and_downcast::<crate::main_view::MainView>()
            {
                main.open_album_from(&page, &album.id.clone(), album);
            }
        });

        match client {
            Some(client) => {
                page.load_artist(client.clone(), id.to_string());
                page.load_biography(client, id.to_string());
            }
            None => page.show_albums(local.albums, None, local.picture),
        }
        page
    }

    /// The connection is back: a page made offline fetches the artist (its
    /// cards stay until the answer replaces them).
    pub(crate) fn set_client(&self, client: subsonic::Client) {
        let imp = self.imp();
        if imp.client.borrow().is_some() {
            return;
        }
        imp.client.replace(Some(client.clone()));
        let id = self.artist_id();
        self.load_artist(client.clone(), id.clone());
        self.load_biography(client, id);
    }

    fn load_artist(&self, client: subsonic::Client, id: String) {
        let page = self.downgrade();
        let client_for_cards = client.clone();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn(async move { client.artist(&id).await })
                .await;
            let Some(page) = page.upgrade() else {
                return;
            };
            match result {
                Ok(Ok(artist)) => page.show_artist(artist, client_for_cards),
                Ok(Err(e)) => {
                    eprintln!("Could not fetch the artist: {e}");
                    page.show_error(&error_message(&e));
                }
                Err(_) => page.show_error(&gettext("Something went wrong. Please try again.")),
            }
        });
    }

    /// The albums give way to why they could not be fetched.
    fn show_error(&self, message: &str) {
        let imp = self.imp();
        imp.albums_title.set_visible(false);
        imp.albums.set_visible(false);
        // The description is markup; a server's message may hold "&" or "<".
        imp.error_status
            .set_description(Some(&glib::markup_escape_text(message)));
        imp.error_status.set_visible(true);
    }

    fn show_artist(&self, artist: subsonic::ArtistWithAlbums, client: subsonic::Client) {
        self.show_albums(artist.album, Some(client), artist.cover_art);
    }

    /// The artist's albums as cards, and the photo; without a client (offline)
    /// the covers and the photo come from the disk cache. Cards shown before
    /// (the library's, offline) give way.
    fn show_albums(
        &self,
        albums: Vec<subsonic::Album>,
        client: Option<subsonic::Client>,
        picture: Option<String>,
    ) {
        let imp = self.imp();

        imp.details
            .set_label(&crate::format::albums_text(albums.len() as u32));

        imp.albums.remove_all();
        for album in &albums {
            let card = AlbumCard::new();
            card.set_album(album, client.clone());
            imp.albums.append(&card);
        }

        // The photo goes through the same disk cache as album covers.
        if let Some(cover) = picture {
            covers::load_for(&imp.avatar.get(), client, cover, |avatar, texture| {
                if let Some(texture) = texture {
                    avatar.set_custom_image(Some(&texture));
                }
            });
        }

        imp.album_list.replace(albums);
    }

    fn load_biography(&self, client: subsonic::Client, id: String) {
        let page = self.downgrade();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn(async move { client.artist_info(&id).await })
                .await;
            let (Some(page), Ok(Ok(info))) = (page.upgrade(), result) else {
                return;
            };
            let text = info
                .biography
                .as_deref()
                .map(about::plain_text)
                .unwrap_or_default();
            if !text.is_empty() {
                let imp = page.imp();
                about::fill(&imp.about, &imp.about_text, &imp.about_more, &text);
            }
        });
    }
}

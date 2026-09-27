//! Artist page: photo, name, biography and the artist's albums.
//! The layout lives in artist_page.blp; this file only holds the behaviour.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::ngettext;
use gtk::glib;
use std::cell::RefCell;

use crate::album_card::AlbumCard;
use crate::album_page::AlbumPage;
use crate::{about, covers, runtime};

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
        /// The albums in the same order as the cards, to know which one was clicked.
        pub album_list: RefCell<Vec<subsonic::Album>>,
        pub client: RefCell<Option<subsonic::Client>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ArtistPage {
        // Must match "template $MusePlayerArtistPage" in the Blueprint file.
        const NAME: &'static str = "MusePlayerArtistPage";
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

glib::wrapper! {
    pub struct ArtistPage(ObjectSubclass<imp::ArtistPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ArtistPage {
    pub(crate) fn new(id: &str, name: &str, client: subsonic::Client) -> Self {
        let page: Self = glib::Object::new();
        let imp = page.imp();

        page.set_title(name);
        imp.name.set_label(name);
        imp.avatar.set_text(Some(name));
        imp.client.replace(Some(client.clone()));

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
            let album_page = AlbumPage::new(&album, imp.client.borrow().clone());
            if let Some(nav) = page
                .ancestor(adw::NavigationView::static_type())
                .and_downcast::<adw::NavigationView>()
            {
                nav.push(&album_page);
            }
        });

        page.load_artist(client.clone(), id.to_string());
        page.load_biography(client, id.to_string());
        page
    }

    fn load_artist(&self, client: subsonic::Client, id: String) {
        let page = self.downgrade();
        let client_for_cards = client.clone();
        glib::spawn_future_local(async move {
            let result = runtime().spawn(async move { client.artist(&id).await }).await;
            let (Some(page), Ok(Ok(artist))) = (page.upgrade(), result) else {
                return;
            };
            page.show_artist(artist, client_for_cards);
        });
    }

    fn show_artist(&self, artist: subsonic::ArtistWithAlbums, client: subsonic::Client) {
        let imp = self.imp();

        let n = artist.album.len() as u32;
        // Translators: {n} is a number of albums.
        imp.details
            .set_label(&ngettext("{n} album", "{n} albums", n).replace("{n}", &n.to_string()));

        for album in &artist.album {
            let card = AlbumCard::new();
            card.set_album(album, Some(client.clone()));
            imp.albums.append(&card);
        }

        // The photo goes through the same disk cache as album covers.
        if let Some(cover) = artist.cover_art.clone() {
            let avatar = imp.avatar.get();
            glib::spawn_future_local(async move {
                if let Some(texture) = covers::load(Some(client), cover).await {
                    avatar.set_custom_image(Some(&texture));
                }
            });
        }

        imp.album_list.replace(artist.album);
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

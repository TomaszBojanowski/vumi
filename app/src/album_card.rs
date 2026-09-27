//! One album in the library grid. The grid reuses cards while scrolling,
//! so set_album() is called many times on the same card.

use adw::subclass::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;

use crate::covers;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/album_card.blp")]
    pub struct AlbumCard {
        #[template_child]
        pub cover: gtk::TemplateChild<gtk::Picture>,
        #[template_child]
        pub title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub artist: gtk::TemplateChild<gtk::Label>,
        /// Which cover this card is waiting for right now.
        pub cover_id: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumCard {
        // Must match "template $MusePlayerAlbumCard" in the Blueprint file.
        const NAME: &'static str = "MusePlayerAlbumCard";
        type Type = super::AlbumCard;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AlbumCard {}
    impl WidgetImpl for AlbumCard {}
    impl BoxImpl for AlbumCard {}
}

glib::wrapper! {
    pub struct AlbumCard(ObjectSubclass<imp::AlbumCard>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl AlbumCard {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    pub(crate) fn set_album(&self, album: &subsonic::Album) {
        let imp = self.imp();
        imp.title.set_label(&album.name);
        imp.artist.set_label(album.artist.as_deref().unwrap_or(""));
        self.set_tooltip_text(Some(&album.name));

        // Drop the previous album's cover right away, so it never shows under a new title.
        imp.cover.set_paintable(None::<&gtk::gdk::Paintable>);
        imp.cover_id.replace(album.cover_art.clone());
    }

    pub(crate) fn load_cover(&self, client: subsonic::Client, id: String) {
        let card = self.downgrade();
        glib::spawn_future_local(async move {
            let texture = covers::load(client, id.clone()).await;
            let Some(card) = card.upgrade() else {
                return;
            };
            // The card may have been reused for another album in the meantime.
            let imp = card.imp();
            if imp.cover_id.borrow().as_deref() == Some(id.as_str()) {
                imp.cover.set_paintable(texture.as_ref());
            }
        });
    }
}

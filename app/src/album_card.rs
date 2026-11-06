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
        pub cover_frame: gtk::TemplateChild<crate::cover_image::Square>,
        #[template_child]
        pub cover: gtk::TemplateChild<gtk::Picture>,
        #[template_child]
        pub title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub artist: gtk::TemplateChild<gtk::Label>,
        /// Which cover this card shows or is waiting for right now.
        pub cover_id: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumCard {
        // Must match "template $VumiAlbumCard" in the Blueprint file.
        const NAME: &'static str = "VumiAlbumCard";
        type Type = super::AlbumCard;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            // The template uses it.
            crate::cover_image::Square::static_type();
            klass.bind_template();
            // A group with a name ("19, Adele"): the grid's cell, a shelf's
            // button, whatever holds the card, is read by that name.
            klass.set_accessible_role(gtk::AccessibleRole::Group);
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AlbumCard {
        fn constructed(&self) {
            self.parent_constructed();
            // The whole title on hover, for one cut short. Given when asked
            // for, not as tooltip-text: GTK reads tooltip-text out as the
            // description of the cell or button around the card, the title a
            // second time.
            let card = self.obj();
            card.set_has_tooltip(true);
            card.connect_query_tooltip(|card, _, _, _, tooltip| {
                let title = card.imp().title.label();
                tooltip.set_text(Some(&title));
                !title.is_empty()
            });
        }
    }
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

    /// In the album grid the card is as wide as its cell, and the cover with
    /// it. Elsewhere (shelves, flow boxes) the cover keeps its 160 px.
    pub(crate) fn set_cover_grows(&self, grows: bool) {
        self.imp().cover_frame.set_grows(grows);
    }

    /// `client` is None until the app is connected; cached covers load anyway.
    pub(crate) fn set_album(&self, album: &subsonic::Album, client: Option<subsonic::Client>) {
        let imp = self.imp();
        imp.title.set_label(&album.name);
        let artist = album.artist.as_deref().unwrap_or("");
        imp.artist.set_label(artist);
        self.update_property(&[gtk::accessible::Property::Label(&crate::a11y::join([
            album.name.as_str(),
            artist,
        ]))]);

        // The same album came back to this card and its cover is on screen: keep it.
        let same = imp.cover_id.borrow().as_deref() == album.cover_art.as_deref();
        if same && imp.cover.paintable().is_some() {
            return;
        }

        // Drop the previous album's cover right away, so it never shows under a new title.
        covers::show(&imp.cover, None);
        imp.cover_id.replace(album.cover_art.clone());
        if let Some(id) = album.cover_art.clone() {
            self.load_cover(client, id);
        }
    }

    fn load_cover(&self, client: Option<subsonic::Client>, id: String) {
        covers::load_for(self, client, id.clone(), move |card, texture| {
            // The card may have been reused for another album in the meantime.
            let imp = card.imp();
            if imp.cover_id.borrow().as_deref() == Some(id.as_str()) {
                covers::show(&imp.cover, texture.as_ref());
            }
        });
    }
}

impl AlbumCard {
    /// Called once the app is connected: fetch the cover if this card still has none.
    pub(crate) fn retry_cover(&self, client: subsonic::Client) {
        let imp = self.imp();
        if imp.cover.paintable().is_some() {
            return;
        }
        let id = imp.cover_id.borrow().clone();
        if let Some(id) = id {
            self.load_cover(Some(client), id);
        }
    }
}

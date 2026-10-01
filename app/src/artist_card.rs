//! One artist in the Artists section: the picture or the initials, the name
//! and the number of albums.

use adw::subclass::prelude::*;
use gettextrs::ngettext;
use gtk::glib;
use std::cell::RefCell;

use crate::covers;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/artist_card.blp")]
    pub struct ArtistCard {
        #[template_child]
        pub avatar: gtk::TemplateChild<adw::Avatar>,
        #[template_child]
        pub name: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub albums: gtk::TemplateChild<gtk::Label>,
        /// Which picture this card shows or is waiting for right now.
        pub picture_id: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ArtistCard {
        // Must match "template $VumiArtistCard" in the Blueprint file.
        const NAME: &'static str = "VumiArtistCard";
        type Type = super::ArtistCard;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ArtistCard {}
    impl WidgetImpl for ArtistCard {}
    impl BoxImpl for ArtistCard {}
}

glib::wrapper! {
    pub struct ArtistCard(ObjectSubclass<imp::ArtistCard>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ArtistCard {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    /// `picture` is the cover id of the artist's picture, if the server has
    /// one; without a `client` (or a picture) the initials stay.
    pub(crate) fn set_artist(
        &self,
        name: &str,
        albums: u32,
        picture: Option<String>,
        client: Option<subsonic::Client>,
    ) {
        let imp = self.imp();
        imp.avatar.set_text(Some(name));
        imp.name.set_label(name);
        // Translators: {n} is a number of albums.
        imp.albums
            .set_label(&ngettext("{n} album", "{n} albums", albums).replace("{n}", &albums.to_string()));

        // The picture of whoever was here before must not show under a new name.
        imp.avatar.set_custom_image(None::<&gtk::gdk::Paintable>);
        imp.picture_id.replace(picture.clone());
        if let (Some(id), Some(client)) = (picture, client) {
            covers::load_for(self, Some(client), id.clone(), move |card, texture| {
                // The card may show another artist by now.
                let imp = card.imp();
                if imp.picture_id.borrow().as_deref() == Some(id.as_str()) {
                    imp.avatar.set_custom_image(texture.as_ref());
                }
            });
        }
    }
}

//! One album in the library grid. The grid reuses cards while scrolling,
//! so set_album() is called many times on the same card.

use adw::subclass::prelude::*;
use gtk::glib;
use gtk::prelude::*;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/album_card.blp")]
    pub struct AlbumCard {
        #[template_child]
        pub title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub artist: gtk::TemplateChild<gtk::Label>,
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
        @implements gtk::Buildable, gtk::ConstraintTarget;
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
    }
}

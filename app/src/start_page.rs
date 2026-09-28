//! The Start page: shelves that answer "what shall I listen to now".
//! For now "Recently added"; "Continue" and "Similar to…" come next.

use adw::subclass::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use std::sync::Arc;

use crate::album_card::AlbumCard;
use crate::main_view::MainView;

/// How many albums one shelf shows.
const SHELF_SIZE: usize = 12;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/start_page.blp")]
    pub struct StartPage {
        #[template_child]
        pub recent_shelf: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub recent_row: gtk::TemplateChild<gtk::Box>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for StartPage {
        // Must match "template $MusePlayerStartPage" in the Blueprint file.
        const NAME: &'static str = "MusePlayerStartPage";
        type Type = super::StartPage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for StartPage {}
    impl WidgetImpl for StartPage {}
    impl BinImpl for StartPage {}
}

glib::wrapper! {
    pub struct StartPage(ObjectSubclass<imp::StartPage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl StartPage {
    /// Rebuilds the shelves from the albums in the library.
    pub(crate) fn update(&self, albums: &[Arc<subsonic::Album>], client: Option<subsonic::Client>) {
        let imp = self.imp();

        // Newest first; albums without a date (not synced yet) are left out.
        let mut recent: Vec<&Arc<subsonic::Album>> =
            albums.iter().filter(|a| a.created.is_some()).collect();
        recent.sort_by(|a, b| b.created.cmp(&a.created));
        recent.truncate(SHELF_SIZE);

        fill_shelf(&imp.recent_row, &recent, client);
        imp.recent_shelf.set_visible(!recent.is_empty());
    }
}

/// Puts one album card per album into `row`; a click opens the album.
fn fill_shelf(
    row: &gtk::Box,
    albums: &[&Arc<subsonic::Album>],
    client: Option<subsonic::Client>,
) {
    while let Some(child) = row.first_child() {
        row.remove(&child);
    }
    for album in albums {
        let card = AlbumCard::new();
        card.set_album(album, client.clone());
        let button = gtk::Button::builder()
            .child(&card)
            .css_classes(["flat", "shelf-card"])
            .build();

        let album = Arc::clone(album);
        button.connect_clicked(move |button| {
            if let Some(main) = button
                .ancestor(MainView::static_type())
                .and_downcast::<MainView>()
            {
                main.open_album(&album.id, (*album).clone());
            }
        });
        row.append(&button);
    }
}

//! Search results: albums and songs from the local database (library::search).
//! Shown on top of the library; the search field in the sidebar updates it.

use adw::subclass::prelude::*;
use gtk::glib;
use adw::prelude::*;
use std::rc::Rc;
use std::sync::Arc;

use crate::album_card::AlbumCard;
use crate::main_view::MainView;
use crate::player;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/search_page.blp")]
    pub struct SearchPage {
        #[template_child]
        pub stack: gtk::TemplateChild<gtk::Stack>,
        #[template_child]
        pub albums_section: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub albums_box: gtk::TemplateChild<gtk::FlowBox>,
        #[template_child]
        pub songs_section: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub songs_list: gtk::TemplateChild<gtk::ListBox>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SearchPage {
        // Must match "template $MusePlayerSearchPage" in the Blueprint file.
        const NAME: &'static str = "MusePlayerSearchPage";
        type Type = super::SearchPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SearchPage {}
    impl WidgetImpl for SearchPage {}
    impl NavigationPageImpl for SearchPage {}
}

glib::wrapper! {
    pub struct SearchPage(ObjectSubclass<imp::SearchPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl SearchPage {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    /// Shows what a search found. A click on an album opens it on top of the
    /// results; a click on a song plays it, then the songs found after it.
    pub(crate) fn show(
        &self,
        albums: &[Arc<subsonic::Album>],
        songs: Vec<subsonic::Song>,
        client: Option<subsonic::Client>,
    ) {
        let imp = self.imp();

        imp.albums_box.remove_all();
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
            imp.albums_box.append(&button);
        }
        imp.albums_section.set_visible(!albums.is_empty());

        imp.songs_list.remove_all();
        let queue: Rc<[subsonic::Song]> = songs.into();
        for (index, song) in queue.iter().enumerate() {
            let subtitle = [song.artist.as_deref(), song.album.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            let row = adw::ActionRow::builder()
                .title(song.title.as_str())
                .subtitle(subtitle)
                .use_markup(false)
                .activatable(true)
                .build();
            if let Some(d) = song.duration {
                row.add_suffix(
                    &gtk::Label::builder()
                        .label(format!("{}:{:02}", d / 60, d % 60))
                        .css_classes(["dim-label", "numeric"])
                        .build(),
                );
            }
            let queue = queue.clone();
            let client = client.clone();
            row.connect_activated(move |_| {
                if let Some(client) = client.clone() {
                    player::get().play(client, queue.clone(), index);
                }
            });
            imp.songs_list.append(&row);
        }
        imp.songs_section.set_visible(!queue.is_empty());

        let nothing = albums.is_empty() && queue.is_empty();
        imp.stack
            .set_visible_child_name(if nothing { "empty" } else { "results" });
    }
}

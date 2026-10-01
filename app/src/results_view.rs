//! Albums as cards and songs as a list, with the playing song marked: used by
//! the search results and the Favorites page.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::glib;
use std::rc::Rc;
use std::sync::Arc;

use crate::album_card::AlbumCard;
use crate::main_view::MainView;
use crate::player;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/results_view.blp")]
    pub struct ResultsView {
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
        #[template_child]
        pub empty_page: gtk::TemplateChild<adw::StatusPage>,
        /// Song rows with their bars, to mark the one that is playing.
        pub rows: std::cell::RefCell<Vec<(String, adw::ActionRow, gtk::Box)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ResultsView {
        // Must match "template $VumiResultsView" in the Blueprint file.
        const NAME: &'static str = "VumiResultsView";
        type Type = super::ResultsView;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ResultsView {
        fn constructed(&self) {
            self.parent_constructed();
            // The playing song is marked, as on album pages. These views live as
            // long as the app, so they keep listening.
            let view = self.obj().downgrade();
            player::get().connect_changed(move || {
                if let Some(view) = view.upgrade() {
                    view.mark_playing();
                }
            });
        }
    }
    impl WidgetImpl for ResultsView {}
    impl BinImpl for ResultsView {}
}

glib::wrapper! {
    pub struct ResultsView(ObjectSubclass<imp::ResultsView>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ResultsView {
    /// What the empty page says (search and favourites differ).
    pub(crate) fn set_empty(&self, icon: &str, title: &str, description: &str) {
        let page = &self.imp().empty_page;
        page.set_icon_name(Some(icon));
        page.set_title(title);
        page.set_description(Some(description));
    }

    /// Shows albums and songs. A click on an album opens it; a click on a song
    /// plays it, then the songs listed after it.
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
                    main.open_album_from(button, &album.id, (*album).clone());
                }
            });
            imp.albums_box.append(&button);
        }
        imp.albums_section.set_visible(!albums.is_empty());

        imp.songs_list.remove_all();
        imp.rows.borrow_mut().clear();
        let queue: Rc<[subsonic::Song]> = songs.into();
        for (index, song) in queue.iter().enumerate() {
            let subtitle = [song.artist.as_deref(), song.album.as_deref()]
                .into_iter()
                .flatten()
                .collect::<Vec<_>>()
                .join(" · ");
            let row = adw::ActionRow::builder()
                .title(song.title.as_str())
                .use_markup(false)
                .activatable(true)
                // One line each: a long list of performers must not make a
                // row taller than the others.
                .subtitle_lines(1)
                .build();
            // Set after use_markup is off: names like "Simon & Garfunkel" are plain
            // text, not markup.
            row.set_subtitle(&subtitle);
            // Bars before the title, see-through unless the song plays, so the
            // titles stay aligned either way.
            let bars = crate::album_page::eq_bars();
            bars.set_opacity(0.0);
            row.add_prefix(&bars);
            imp.rows
                .borrow_mut()
                .push((song.id.clone(), row.clone(), bars));
            if let Some(d) = song.duration {
                row.add_suffix(
                    &gtk::Label::builder()
                        .label(format!("{}:{:02}", d / 60, d % 60))
                        .css_classes(["dim-label", "numeric"])
                        .build(),
                );
            }
            crate::song_menu::attach(&row, &queue, index, client.clone(), true);
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
        self.mark_playing();

        let nothing = albums.is_empty() && queue.is_empty();
        imp.stack
            .set_visible_child_name(if nothing { "empty" } else { "results" });
    }

    /// Bars and the highlight on the song that is playing (resting bars while
    /// paused), as on album pages.
    fn mark_playing(&self) {
        let player = player::get();
        let current = player
            .current()
            .filter(|_| player.is_active())
            .map(|song| song.id);
        for (id, row, bars) in self.imp().rows.borrow().iter() {
            let playing = current.as_deref() == Some(id.as_str());
            bars.set_opacity(if playing { 1.0 } else { 0.0 });
            if player.is_playing() {
                bars.remove_css_class("paused");
            } else {
                bars.add_css_class("paused");
            }
            if playing {
                row.add_css_class("playing");
            } else {
                row.remove_css_class("playing");
            }
        }
    }
}

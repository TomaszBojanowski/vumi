//! Playlist page: a compact header and the tracks, played in playlist order.
//! The layout lives in playlist_page.blp; this file only holds the behaviour.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

use crate::results_view::ResultsView;
use crate::{covers, error_message, player, runtime};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/playlist_page.blp")]
    pub struct PlaylistPage {
        #[template_child]
        pub cover: gtk::TemplateChild<crate::cover_image::CoverImage>,
        #[template_child]
        pub name: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub details: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub comment: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub play_button: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub shuffle_button: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub results: gtk::TemplateChild<ResultsView>,
        #[template_child]
        pub error_status: gtk::TemplateChild<adw::StatusPage>,
        /// Which playlist this page shows, so an open page can be found again.
        pub playlist_id: RefCell<String>,
        pub client: RefCell<Option<subsonic::Client>>,
        /// The songs in playlist order, read by Play and Shuffle when clicked.
        pub songs: RefCell<Option<Rc<[subsonic::Song]>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlaylistPage {
        // Must match "template $VumiPlaylistPage" in the Blueprint file.
        const NAME: &'static str = "VumiPlaylistPage";
        type Type = super::PlaylistPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            // The results view is used in this template, so its type must exist.
            ResultsView::static_type();
            crate::cover_image::CoverImage::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for PlaylistPage {}
    impl WidgetImpl for PlaylistPage {}
    impl NavigationPageImpl for PlaylistPage {}
}

glib::wrapper! {
    pub struct PlaylistPage(ObjectSubclass<imp::PlaylistPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PlaylistPage {
    /// The id of the playlist this page shows.
    pub(crate) fn playlist_id(&self) -> String {
        self.imp().playlist_id.borrow().clone()
    }

    pub(crate) fn new(id: &str, name: &str, client: subsonic::Client) -> Self {
        let page: Self = glib::Object::new();
        let imp = page.imp();

        page.set_title(name);
        imp.name.set_label(name);
        imp.playlist_id.replace(id.to_string());
        imp.client.replace(Some(client.clone()));
        imp.results.set_empty(
            "view-list-symbolic",
            &gettext("No Tracks"),
            &gettext("This playlist is empty"),
        );

        let weak = page.downgrade();
        imp.play_button.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade() {
                page.play(false);
            }
        });
        let weak = page.downgrade();
        imp.shuffle_button.connect_clicked(move |_| {
            if let Some(page) = weak.upgrade() {
                page.play(true);
            }
        });

        page.load(client, id.to_string());
        page
    }

    fn load(&self, client: subsonic::Client, id: String) {
        let page = self.downgrade();
        let client_for_page = client.clone();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn(async move { client.playlist(&id).await })
                .await;
            let Some(page) = page.upgrade() else {
                return;
            };
            match result {
                Ok(Ok(playlist)) => page.show(playlist, client_for_page),
                Ok(Err(e)) => {
                    eprintln!("Could not fetch the playlist: {e}");
                    page.show_error(&error_message(&e));
                }
                Err(_) => page.show_error(&gettext("Something went wrong. Please try again.")),
            }
        });
    }

    /// The tracks give way to why they could not be fetched; nothing to play.
    fn show_error(&self, message: &str) {
        let imp = self.imp();
        imp.results.set_visible(false);
        imp.play_button.set_sensitive(false);
        imp.shuffle_button.set_sensitive(false);
        // The description is markup; a server's message may hold "&" or "<".
        imp.error_status
            .set_description(Some(&glib::markup_escape_text(message)));
        imp.error_status.set_visible(true);
    }

    fn show(&self, playlist: subsonic::PlaylistWithSongs, client: subsonic::Client) {
        let imp = self.imp();

        let count = playlist.entry.len() as u32;
        let mut parts = vec![crate::format::tracks_text(count)];
        let seconds: u32 = playlist.entry.iter().filter_map(|s| s.duration).sum();
        if seconds > 0 {
            parts.push(crate::format::long_duration(seconds));
        }
        imp.details.set_label(&parts.join(" · "));

        if let Some(comment) = playlist.comment.as_deref().filter(|c| !c.trim().is_empty()) {
            imp.comment.set_label(comment);
            imp.comment.set_visible(true);
        }

        if let Some(cover) = playlist.cover_art.clone() {
            covers::load_for(
                &imp.cover.get(),
                Some(client.clone()),
                cover,
                |cover, texture| {
                    if texture.is_some() {
                        cover.set_texture(texture.as_ref());
                    }
                },
            );
        }

        let songs: Rc<[subsonic::Song]> = playlist.entry.as_slice().into();
        imp.songs.replace(Some(songs));
        imp.results.show(&[], playlist.entry, Some(client));
    }

    /// Plays the whole playlist from its first track, or in a random order.
    fn play(&self, shuffle: bool) {
        let imp = self.imp();
        let (Some(songs), Some(client)) = (imp.songs.borrow().clone(), imp.client.borrow().clone())
        else {
            return;
        };
        player::play_list(client, songs, shuffle, player::QueueOf::Songs);
    }
}

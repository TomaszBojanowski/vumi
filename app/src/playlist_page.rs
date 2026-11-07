//! Playlist page: a compact header and the tracks, played in playlist order.
//! The layout lives in playlist_page.blp; this file only holds the behaviour.
//!
//! Where the user may, the header's ⋮ renames and deletes the playlist.
//! Nothing waits for a connection: while offline the ⋮ is off.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::rc::Rc;

use crate::main_view::MainView;
use crate::playlist_edit::{self, Rights};
use crate::results_view::ResultsView;
use crate::{covers, error_message, player, reconnect, runtime};

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
        pub actions: gtk::TemplateChild<gtk::Box>,
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
        /// The playlist's name now.
        pub name_now: RefCell<String>,
        /// What the user may do with the playlist.
        pub(crate) rights: Cell<Rights>,
        /// The header's ⋮: Rename and Delete.
        pub more: RefCell<Option<gtk::Button>>,
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
        crate::offline_banner::attach(&page);
        let imp = page.imp();

        page.set_title(name);
        imp.name.set_label(name);
        imp.name_now.replace(name.to_string());
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

        page.setup_more();
        // Offline nothing can be changed: the ⋮ follows the connection.
        let weak = page.downgrade();
        reconnect::watch_offline(move |offline| {
            let Some(page) = weak.upgrade() else {
                return false;
            };
            page.set_online(!offline);
            true
        });

        page.load(client, id.to_string());
        page
    }

    /// The header's ⋮ (Rename…, Delete…), after Shuffle; shown where the user
    /// may rename and delete the playlist.
    fn setup_more(&self) {
        let imp = self.imp();
        let actions = gio::SimpleActionGroup::new();
        let rename = gio::SimpleAction::new("rename", None);
        let page = self.downgrade();
        rename.connect_activate(move |_, _| {
            if let Some(page) = page.upgrade() {
                page.ask_rename();
            }
        });
        actions.add_action(&rename);
        let delete = gio::SimpleAction::new("delete", None);
        let page = self.downgrade();
        delete.connect_activate(move |_, _| {
            if let Some(page) = page.upgrade() {
                page.ask_delete();
            }
        });
        actions.add_action(&delete);
        self.insert_action_group("playlist-page", Some(&actions));

        let more = crate::song_menu::more_button(|| {
            let menu = gio::Menu::new();
            menu.append(Some(&gettext("Rename…")), Some("playlist-page.rename"));
            menu.append(Some(&gettext("Delete…")), Some("playlist-page.delete"));
            Some(menu)
        });
        more.add_css_class("album-action");
        more.set_visible(false);
        imp.actions.append(&more);
        imp.more.replace(Some(more));
        self.set_online(!reconnect::is_offline());
    }

    /// Online, changes can be made; offline the ⋮ is off, saying why.
    fn set_online(&self, online: bool) {
        let imp = self.imp();
        if let Some(more) = imp.more.borrow().as_ref() {
            more.set_sensitive(online);
            more.set_tooltip_text(Some(&if online {
                gettext("More")
            } else {
                gettext("Needs a connection to the server")
            }));
        }
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

        self.show_details(&playlist.entry);

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

        imp.name_now.replace(playlist.name.clone());
        self.show_songs(playlist, client);
    }

    /// The songs of `playlist` as the server has them, and what may be done
    /// with the playlist.
    fn show_songs(&self, playlist: subsonic::PlaylistWithSongs, client: subsonic::Client) {
        let imp = self.imp();
        let rights =
            playlist_edit::rights(playlist.owner.as_deref(), playlist.readonly, client.user());
        imp.rights.set(rights);
        if let Some(more) = imp.more.borrow().as_ref() {
            more.set_visible(rights.manage);
        }
        self.show_details(&playlist.entry);
        let songs: Rc<[subsonic::Song]> = playlist.entry.as_slice().into();
        imp.songs.replace(Some(songs));
        imp.results.show(&[], playlist.entry, Some(client));
    }

    /// How many songs and how long, under the name.
    fn show_details(&self, songs: &[subsonic::Song]) {
        let mut parts = vec![crate::format::tracks_text(songs.len() as u32)];
        let seconds: u32 = songs.iter().filter_map(|s| s.duration).sum();
        if seconds > 0 {
            parts.push(crate::format::long_duration(seconds));
        }
        self.imp().details.set_label(&parts.join(" · "));
    }

    /// Rename…: a new name for the playlist.
    fn ask_rename(&self) {
        let imp = self.imp();
        let current = imp.name_now.borrow().clone();
        let entry = gtk::Entry::builder()
            .text(current.as_str())
            .placeholder_text(gettext("Name"))
            .activates_default(true)
            .build();
        let dialog = adw::AlertDialog::new(Some(&gettext("Rename Playlist")), None);
        dialog.set_extra_child(Some(&entry));
        dialog.add_responses(&[
            ("cancel", &gettext("Cancel")),
            ("rename", &gettext("Rename")),
        ]);
        dialog.set_response_appearance("rename", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("rename"));
        dialog.set_close_response("cancel");
        // A new name, not empty.
        dialog.set_response_enabled("rename", false);
        let dialog_weak = dialog.downgrade();
        entry.connect_changed(move |entry| {
            if let Some(dialog) = dialog_weak.upgrade() {
                let name = entry.text();
                let name = name.trim();
                dialog.set_response_enabled("rename", !name.is_empty() && name != current);
            }
        });
        let page = self.downgrade();
        dialog.connect_response(Some("rename"), move |_, _| {
            let name = entry.text().trim().to_string();
            if let Some(page) = page.upgrade() {
                page.rename(name);
            }
        });
        dialog.present(Some(self));
    }

    fn rename(&self, name: String) {
        let Some(client) = self.imp().client.borrow().clone() else {
            return;
        };
        let id = self.playlist_id();
        let page = self.downgrade();
        glib::spawn_future_local(async move {
            let new_name = name.clone();
            let result = runtime()
                .spawn(async move { client.rename_playlist(&id, &new_name).await })
                .await;
            let Some(page) = page.upgrade() else {
                return;
            };
            match result {
                Ok(Ok(())) => {
                    let imp = page.imp();
                    page.set_title(&name);
                    imp.name.set_label(&name);
                    imp.name_now.replace(name);
                    if let Some(main) = page
                        .ancestor(MainView::static_type())
                        .and_downcast::<MainView>()
                    {
                        main.sync_playlists();
                    }
                }
                Ok(Err(e)) => {
                    eprintln!("Could not rename the playlist: {e}");
                    page.failed_note(&e, &gettext("Could not change the playlist"));
                }
                Err(_) => crate::toast(&page, &gettext("Could not change the playlist")),
            }
        });
    }

    /// Delete…: asks first; the server cannot bring a playlist back.
    fn ask_delete(&self) {
        let name = self.imp().name_now.borrow().clone();
        // Translators: {name} is the name of a playlist.
        let body = gettext("“{name}” will be deleted from the server. This cannot be undone.")
            .replace("{name}", &name);
        let dialog = adw::AlertDialog::new(Some(&gettext("Delete Playlist?")), Some(&body));
        dialog.add_responses(&[
            ("cancel", &gettext("Cancel")),
            ("delete", &gettext("Delete")),
        ]);
        dialog.set_response_appearance("delete", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        let page = self.downgrade();
        dialog.connect_response(Some("delete"), move |_, _| {
            if let Some(page) = page.upgrade() {
                page.delete();
            }
        });
        dialog.present(Some(self));
    }

    fn delete(&self) {
        let Some(client) = self.imp().client.borrow().clone() else {
            return;
        };
        let id = self.playlist_id();
        let page = self.downgrade();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn(async move { client.delete_playlist(&id).await })
                .await;
            let Some(page) = page.upgrade() else {
                return;
            };
            match result {
                Ok(Ok(())) => page.deleted(),
                Ok(Err(e)) => {
                    eprintln!("Could not delete the playlist: {e}");
                    page.failed_note(&e, &gettext("Could not delete the playlist"));
                }
                Err(_) => crate::toast(&page, &gettext("Could not delete the playlist")),
            }
        });
    }

    /// The playlist is gone: the page closes, the Playlists section follows,
    /// a note says what happened. A queue started from it plays on.
    fn deleted(&self) {
        let name = self.imp().name_now.borrow().clone();
        let main = self
            .ancestor(MainView::static_type())
            .and_downcast::<MainView>();
        if let Some(nav) = self
            .ancestor(adw::NavigationView::static_type())
            .and_downcast::<adw::NavigationView>()
        {
            if nav.visible_page().as_ref() == Some(self.upcast_ref()) {
                nav.pop();
            } else {
                nav.remove(self);
            }
        }
        if let Some(main) = main {
            main.sync_playlists();
            // Translators: {name} is the name of a playlist.
            crate::toast_with_name(&main, &gettext("Deleted “{name}”"), &name);
        }
    }

    /// A note for a failed request: the offline text when the server could
    /// not be reached, `text` otherwise.
    fn failed_note(&self, e: &subsonic::Error, text: &str) {
        if matches!(e, subsonic::Error::Unreachable(_)) {
            reconnect::request_failed();
            crate::toast(self, &crate::error_note(e));
        } else {
            crate::toast(self, text);
        }
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

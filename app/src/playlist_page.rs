//! Playlist page: a compact header and the tracks, played in playlist order.
//! The layout lives in playlist_page.blp; this file only holds the behaviour.
//!
//! Where the user may, the page changes the playlist: the header's ⋮ renames
//! and deletes it; the songs are removed (with Undo) and moved (Move Up, Move
//! Down, dragging by a handle). A change shows at once and goes to the server
//! after it, one at a time, with the checks of playlist_edit.rs; what fails
//! is undone on the page, with a note. Nothing waits for a connection: while
//! offline the controls are off.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::{gio, glib};
use std::cell::{Cell, RefCell};
use std::collections::{HashSet, VecDeque};
use std::rc::Rc;

use crate::main_view::MainView;
use crate::playlist_edit::{self, Edit, Rights};
use crate::results_view::{Editing, ResultsView};
use crate::{covers, error_message, player, reconnect, runtime};

/// A change waiting to go to the server, with its number (for Undo).
struct Queued {
    edit: Edit<subsonic::Song>,
    number: u64,
}

/// How a change went on the server.
enum Outcome {
    /// Done: the playlist as the server has it now (the same as the page).
    Done(subsonic::PlaylistWithSongs),
    /// Not sent: the server's list is not the page's (changed elsewhere), or
    /// not whole. The page shows the server's.
    Changed(subsonic::PlaylistWithSongs),
    /// Failed (`error`: the request's; None: the server did something else
    /// than asked, and the list before was put back). `server`: the playlist
    /// as the server has it, if known.
    Failed {
        error: Option<subsonic::Error>,
        server: Option<subsonic::PlaylistWithSongs>,
    },
}

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
        pub unavailable: gtk::TemplateChild<gtk::Label>,
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
        /// The songs as the server last had them, after the changes it took.
        pub server_songs: RefCell<Vec<subsonic::Song>>,
        /// Changes shown on the page, waiting to go to the server.
        pub(super) queue: RefCell<VecDeque<super::Queued>>,
        /// The change on its way to the server (its number), if one is.
        pub sending: Cell<Option<u64>>,
        pub next_number: Cell<u64>,
        /// Removals that did not happen: their Undo does nothing.
        pub failed: RefCell<HashSet<u64>>,
        /// The note of the last removal, with its number: taken away when
        /// the removal fails.
        pub undo_note: RefCell<Option<(u64, adw::Toast)>>,
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
        // Offline nothing can be changed: the controls follow the connection.
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

    /// Online, changes can be made; offline the controls are off, the ⋮
    /// saying why.
    fn set_online(&self, online: bool) {
        let imp = self.imp();
        imp.results.set_edits_on(online);
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
    /// with them.
    fn show_songs(&self, playlist: subsonic::PlaylistWithSongs, client: subsonic::Client) {
        let imp = self.imp();
        let rights =
            playlist_edit::rights(playlist.owner.as_deref(), playlist.readonly, client.user());
        let whole = playlist_edit::whole(playlist.entry.len(), playlist.song_count);
        imp.rights.set(rights);
        imp.unavailable.set_visible(rights.songs && !whole);
        if let Some(more) = imp.more.borrow().as_ref() {
            more.set_visible(rights.manage);
        }
        imp.results
            .set_editing((rights.songs && whole).then(|| self.editing()));
        imp.server_songs.replace(playlist.entry.clone());
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

    /// What the list's Remove, Move Up, Move Down and dragging do.
    fn editing(&self) -> Rc<Editing> {
        let remove = self.downgrade();
        let move_to = self.downgrade();
        Rc::new(Editing {
            remove: Box::new(move |index| {
                if let Some(page) = remove.upgrade() {
                    page.remove(index);
                }
            }),
            move_to: Box::new(move |from, to| {
                if let Some(page) = move_to.upgrade() {
                    page.move_song(from, to);
                }
            }),
        })
    }

    /// Takes the song at `index` out, with a note that offers Undo.
    fn remove(&self, index: usize) {
        let Some(song) = self.shown().and_then(|songs| songs.get(index).cloned()) else {
            return;
        };
        let Some(number) = self.change(Edit::Remove(index)) else {
            return;
        };
        let page = self.downgrade();
        let note = crate::toast_with_button(
            self,
            &gettext("Removed from the playlist"),
            &gettext("Undo"),
            move || {
                if let Some(page) = page.upgrade() {
                    page.undo(number, index, song.clone());
                }
            },
        );
        if let Some(note) = note {
            self.imp().undo_note.replace(Some((number, note)));
        }
    }

    /// Undo of a removal: the song goes back to its place, unless the
    /// removal failed (the song is back already).
    fn undo(&self, number: u64, index: usize, song: subsonic::Song) {
        if self.imp().failed.borrow().contains(&number) {
            return;
        }
        self.change(Edit::Restore { index, song });
    }

    /// Moves the song at `from` to `to`; the focus goes with it, and a
    /// screen reader hears its new place.
    fn move_song(&self, from: usize, to: usize) {
        if self.change(Edit::Move { from, to }).is_none() {
            return;
        }
        let imp = self.imp();
        imp.results.focus_song(to);
        let total = self.shown().map_or(0, |songs| songs.len());
        let text = gettext("Position {n} of {total}")
            .replace("{n}", &(to + 1).to_string())
            .replace("{total}", &total.to_string());
        if let Some(window) = self.root() {
            window
                .upcast_ref::<gtk::Widget>()
                .announce(&text, gtk::AccessibleAnnouncementPriority::Medium);
        }
    }

    /// The songs shown now (with the changes not yet on the server).
    fn shown(&self) -> Option<Rc<[subsonic::Song]>> {
        self.imp().results.songs()
    }

    /// Shows `edit` on the page and queues it for the server; its number.
    /// None when it does not fit the list, or changes cannot be made now.
    fn change(&self, edit: Edit<subsonic::Song>) -> Option<u64> {
        let imp = self.imp();
        if reconnect::is_offline() {
            return None;
        }
        let shown = self.shown()?;
        let after = edit.apply(&shown)?;
        imp.results.apply_edit(&edit);
        self.show_details(&after);
        imp.songs.replace(Some(after.as_slice().into()));
        let number = imp.next_number.get() + 1;
        imp.next_number.set(number);
        imp.queue.borrow_mut().push_back(Queued { edit, number });
        self.send_next();
        Some(number)
    }

    /// Sends the changes waiting, one after another.
    fn send_next(&self) {
        let imp = self.imp();
        if imp.sending.get().is_some() {
            return;
        }
        let Some(Queued { edit, number }) = imp.queue.borrow_mut().pop_front() else {
            // All done: the Playlists section shows the new count, length
            // and cover.
            if let Some(main) = self
                .ancestor(MainView::static_type())
                .and_downcast::<MainView>()
            {
                main.sync_playlists();
            }
            return;
        };
        let Some(client) = imp.client.borrow().clone() else {
            return;
        };
        imp.sending.set(Some(number));
        let id = self.playlist_id();
        let before = imp.server_songs.borrow().clone();
        let page = self.downgrade();
        glib::spawn_future_local(async move {
            let request = client.clone();
            let outcome = runtime()
                .spawn(async move { send(request, id, edit, before).await })
                .await
                .unwrap_or(Outcome::Failed {
                    error: None,
                    server: None,
                });
            let Some(page) = page.upgrade() else {
                return;
            };
            page.imp().sending.set(None);
            page.sent(outcome, number, client);
        });
    }

    /// What the server did with change `number`.
    fn sent(&self, outcome: Outcome, number: u64, client: subsonic::Client) {
        let imp = self.imp();
        match outcome {
            Outcome::Done(now) => {
                imp.server_songs.replace(now.entry);
                self.send_next();
            }
            Outcome::Changed(now) => {
                self.give_up(number);
                self.show_songs(now, client);
                crate::toast(self, &gettext("The playlist changed on the server"));
                self.send_next();
            }
            Outcome::Failed { error, server } => {
                self.give_up(number);
                match server {
                    Some(server) => self.show_songs(server, client),
                    None => {
                        let before = imp.server_songs.borrow().clone();
                        imp.songs.replace(Some(before.as_slice().into()));
                        self.show_details(&before);
                        imp.results.show(&[], before, Some(client));
                    }
                }
                let note = match &error {
                    Some(e @ subsonic::Error::Unreachable(_)) => {
                        reconnect::request_failed();
                        crate::error_note(e)
                    }
                    _ => gettext("Could not change the playlist"),
                };
                if let Some(e) = &error {
                    eprintln!("Could not change the playlist: {e}");
                }
                crate::toast(self, &note);
                self.send_next();
            }
        }
    }

    /// Change `number` and every one after it are dropped: the page goes
    /// back to the server's list. Their removals' Undo does nothing, and the
    /// note of the last one goes.
    fn give_up(&self, number: u64) {
        let imp = self.imp();
        let mut failed = imp.failed.borrow_mut();
        failed.insert(number);
        for queued in imp.queue.borrow_mut().drain(..) {
            failed.insert(queued.number);
        }
        let note = imp.undo_note.borrow().clone();
        if let Some((removal, note)) = note
            && failed.contains(&removal)
        {
            note.dismiss();
        }
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

/// One change of a playlist's songs on the server, with the checks of
/// playlist_edit.rs: the server's list must be the page's list before the
/// change (`before`) and whole; a replace is sent only for a list with the
/// same songs (a move) or one song put back (Undo); what the server has
/// afterwards must be the list asked for, or the list before goes back.
async fn send(
    client: subsonic::Client,
    id: String,
    edit: Edit<subsonic::Song>,
    before: Vec<subsonic::Song>,
) -> Outcome {
    fn ids(songs: &[subsonic::Song]) -> Vec<&str> {
        songs.iter().map(|song| song.id.as_str()).collect()
    }
    let fetched = match client.playlist(&id).await {
        Ok(playlist) => playlist,
        Err(error) => {
            return Outcome::Failed {
                error: Some(error),
                server: None,
            };
        }
    };
    if ids(&fetched.entry) != ids(&before)
        || !playlist_edit::whole(fetched.entry.len(), fetched.song_count)
    {
        return Outcome::Changed(fetched);
    }
    let Some(after) = edit.apply(&before) else {
        return Outcome::Failed {
            error: None,
            server: Some(fetched),
        };
    };
    let sent = match &edit {
        Edit::Remove(index) => match client.remove_from_playlist(&id, *index).await {
            Ok(()) => client.playlist(&id).await,
            Err(error) => Err(error),
        },
        _ => {
            let edit_ids = edit.map(|song| song.id.as_str());
            if !playlist_edit::safe_replace(&edit_ids, &ids(&before), &ids(&after)) {
                return Outcome::Failed {
                    error: None,
                    server: Some(fetched),
                };
            }
            client.set_playlist_songs(&id, &ids(&after)).await
        }
    };
    match sent {
        Ok(now)
            if ids(&now.entry) == ids(&after)
                && playlist_edit::whole(now.entry.len(), now.song_count) =>
        {
            Outcome::Done(now)
        }
        // Not what was asked for (a server that did part of it, say): the
        // list before goes back; it was whole (checked above).
        Ok(now) => {
            let server = match client.set_playlist_songs(&id, &ids(&before)).await {
                Ok(back) => back,
                Err(_) => now,
            };
            Outcome::Failed {
                error: None,
                server: Some(server),
            }
        }
        Err(error) => Outcome::Failed {
            error: Some(error),
            server: None,
        },
    }
}

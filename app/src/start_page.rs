//! The Start page: shelves that answer "what shall I listen to now".
//! For now "Recently added"; "Continue" and "Similar to…" come next.

use adw::subclass::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use gtk::gio;
use std::cell::RefCell;
use std::rc::Rc;
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
        #[template_child]
        pub continue_card: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub continue_cover: gtk::TemplateChild<gtk::Picture>,
        #[template_child]
        pub continue_title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub continue_subtitle: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub continue_progress: gtk::TemplateChild<gtk::ProgressBar>,
        #[template_child]
        pub continue_time: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub continue_button: gtk::TemplateChild<gtk::Button>,
        /// What "Continue" plays: the album's songs, the track, the second.
        pub(crate) resume: RefCell<Option<super::Resume>>,
        /// The library and the client, so the card can follow the player.
        pub albums: RefCell<Vec<Arc<subsonic::Album>>>,
        pub client: RefCell<Option<subsonic::Client>>,
        /// The album on the card, so its cover is reloaded only when it changes.
        pub continue_album: RefCell<String>,
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

    impl ObjectImpl for StartPage {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }
    }
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

        fill_shelf(&imp.recent_row, &recent, client.clone());
        imp.albums.replace(albums.to_vec());
        imp.client.replace(client);
        self.update_continue();
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

/// Where "Continue" picks up: the album's songs, the track and the second.
pub(crate) struct Resume {
    client: subsonic::Client,
    songs: Rc<[subsonic::Song]>,
    index: usize,
    seconds: f64,
}

impl StartPage {
    /// Connects the Continue button (once, when the page is created).
    fn setup(&self) {
        // The card is for picking up the previous session: it hides as soon as
        // something is loaded in the player.
        {
            let page = self.downgrade();
            let listener = crate::player::get().connect_changed(move || {
                let Some(page) = page.upgrade() else {
                    return;
                };
                // Once something is loaded in the player, the sidebar card shows it
                // and controls it: a second card here would only repeat it.
                if crate::player::get().current().is_some() {
                    page.imp().continue_card.set_visible(false);
                }
            });
            self.connect_destroy(move |_| crate::player::get().disconnect(listener));
        }

        let page = self.downgrade();
        self.imp().continue_button.connect_clicked(move |_| {
            let Some(page) = page.upgrade() else {
                return;
            };
            // Copied out first: playing notifies the card, which replaces `resume`,
            // so it must not still be borrowed here.
            let resume = page
                .imp()
                .resume
                .borrow()
                .as_ref()
                .map(|r| (r.client.clone(), r.songs.clone(), r.index, r.seconds));
            if let Some((client, songs, index, seconds)) = resume {
                crate::player::get().play_from(client, songs, index, seconds);
            }
        });
    }

    /// The "Continue" card after startup: what played last, as resume.rs saved it.
    fn update_continue(&self) {
        let settings = gio::Settings::new(crate::APP_ID);
        let album_id = settings.string("last-album-id");
        let song_id = settings.string("last-song-id");
        let seconds = settings.uint("last-position") as f64;
        self.show_continue(&album_id, &song_id, seconds);
    }

    /// Fills the "Continue" card with a song of an album from the library, or
    /// hides it if the album or the song cannot be found.
    fn show_continue(&self, album_id: &str, song_id: &str, seconds: f64) {
        let imp = self.imp();
        imp.continue_card.set_visible(false);
        imp.resume.replace(None);

        let Some(client) = imp.client.borrow().clone() else {
            return;
        };
        let Some(album) = imp.albums.borrow().iter().find(|a| a.id == album_id).cloned() else {
            return;
        };
        let songs = crate::library::load_album_songs(&client.server_key(), &album.id)
            .unwrap_or_default();
        let Some(index) = songs.iter().position(|s| s.id == song_id) else {
            return;
        };

        imp.continue_title.set_label(&album.name);
        // What plays, and where in it: the song with its artist, and a progress bar.
        let song = &songs[index];
        let artist = song.artist.as_deref().or(album.artist.as_deref());
        let subtitle = match artist {
            Some(artist) => format!("{} · {artist}", song.title),
            None => song.title.clone(),
        };
        imp.continue_subtitle.set_label(&subtitle);
        let duration = song.duration.unwrap_or(0) as f64;
        let fraction = if duration > 0.0 {
            (seconds / duration).clamp(0.0, 1.0)
        } else {
            0.0
        };
        imp.continue_progress.set_fraction(fraction);
        imp.continue_time
            .set_label(&format!("{} / {}", clock(seconds as u64), clock(duration as u64)));

        // A new cover only for a new album: no flicker between its tracks.
        let new_album = *imp.continue_album.borrow() != album.id;
        imp.continue_album.replace(album.id.clone());
        if new_album {
            imp.continue_cover.set_paintable(None::<&gtk::gdk::Paintable>);
        }
        if let Some(cover) = album.cover_art.clone().filter(|_| new_album) {
            crate::covers::load_for(
                &imp.continue_cover.get(),
                Some(client.clone()),
                cover,
                |picture, texture| picture.set_paintable(texture.as_ref()),
            );
        }

        imp.resume.replace(Some(Resume {
            client,
            songs: songs.into(),
            index,
            seconds,
        }));
        imp.continue_card.set_visible(true);
    }
}

/// Seconds as "m:ss".
fn clock(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

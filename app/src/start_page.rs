//! The Start page: shelves that answer "what shall I listen to now".
//! For now "Recently added"; "Continue" and "Similar to…" come next.

use adw::subclass::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use gettextrs::gettext;
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
        pub recent_scroll: gtk::TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub recent_prev: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub recent_next: gtk::TemplateChild<gtk::Button>,
        /// The albums on each shelf now, so an unchanged shelf is not rebuilt.
        pub recent_ids: RefCell<Vec<String>>,
        pub similar_ids: RefCell<Vec<String>>,
        #[template_child]
        pub continue_card: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub continue_cover: gtk::TemplateChild<gtk::Picture>,
        #[template_child]
        pub continue_title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub continue_subtitle: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub continue_button: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub continue_content: gtk::TemplateChild<adw::ButtonContent>,
        #[template_child]
        pub similar_shelf: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub similar_title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub similar_radio: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub similar_row: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub similar_scroll: gtk::TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub similar_prev: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub similar_next: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub random_card: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub random_cover: gtk::TemplateChild<gtk::Picture>,
        #[template_child]
        pub random_title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub random_subtitle: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub random_play: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub random_next: gtk::TemplateChild<gtk::Button>,
        /// The album on the "Random Album" card.
        pub random_album: RefCell<Option<Arc<subsonic::Album>>>,
        /// The tint of the hero card, after its cover (tint.rs).
        pub(crate) hero_tint: std::cell::OnceCell<crate::tint::Dynamic>,
        /// The similar songs, played by the radio button next to the shelf title.
        pub similar: RefCell<Option<Rc<[subsonic::Song]>>>,
        /// The song they are similar to, so the server is asked once per song.
        pub similar_seed: RefCell<String>,
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

        fill_shelf(&imp.recent_row, &imp.recent_ids, &recent, client.clone());
        imp.albums.replace(albums.to_vec());
        imp.client.replace(client);
        // A new pick only when there is none yet, or it left the library.
        let keep = imp
            .random_album
            .borrow()
            .as_ref()
            .is_some_and(|a| albums.iter().any(|b| b.id == a.id));
        if !keep {
            self.show_random();
        }
        self.update_continue();
        imp.recent_shelf.set_visible(!recent.is_empty());
    }
}

/// Puts one album card per album into `row`; a click opens the album.
fn fill_shelf(
    row: &gtk::Box,
    shown: &RefCell<Vec<String>>,
    albums: &[&Arc<subsonic::Album>],
    client: Option<subsonic::Client>,
) {
    // The same albums as now: keep the cards, so nothing blinks. They only
    // fetch covers that were not cached, now that there may be a client.
    let ids: Vec<String> = albums.iter().map(|a| a.id.clone()).collect();
    if *shown.borrow() == ids {
        if let Some(client) = &client {
            let mut child = row.first_child();
            while let Some(button) = child {
                if let Some(card) = button
                    .downcast_ref::<gtk::Button>()
                    .and_then(|b| b.child())
                    .and_downcast::<AlbumCard>()
                {
                    card.retry_cover(client.clone());
                }
                child = button.next_sibling();
            }
        }
        return;
    }
    shown.replace(ids);

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
                main.open_album_from(button, &album.id, (*album).clone());
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
        {
            let imp = self.imp();
            connect_arrows(&imp.recent_scroll, &imp.recent_prev, &imp.recent_next);
            connect_arrows(&imp.similar_scroll, &imp.similar_prev, &imp.similar_next);
            connect_wheel(&imp.recent_scroll);
            connect_wheel(&imp.similar_scroll);
            // The hero card takes the colour of its cover (tint_hero).
            let tint = imp.hero_tint.get_or_init(crate::tint::Dynamic::new);
            imp.continue_card.add_css_class(tint.class());
        }

        // The "Random Album" card: play it, or pick another one.
        let page = self.downgrade();
        self.imp().random_play.connect_clicked(move |_| {
            if let Some(page) = page.upgrade() {
                page.play_random();
            }
        });
        let page = self.downgrade();
        self.imp().random_next.connect_clicked(move |_| {
            if let Some(page) = page.upgrade() {
                page.show_random();
            }
        });
        // The radio button next to "Similar to …" plays all the similar songs.
        {
            let page = self.downgrade();
            self.imp().similar_radio.connect_clicked(move |_| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let imp = page.imp();
                let songs = imp.similar.borrow().clone();
                let client = imp.client.borrow().clone();
                if let (Some(songs), Some(client)) = (songs, client) {
                    crate::player::get().play(client, songs, 0);
                }
            });
        }

        // The card follows the player: what plays now, or what played last.
        {
            let page = self.downgrade();
            let listener = crate::player::get().connect_changed(move || {
                let Some(page) = page.upgrade() else {
                    return;
                };
                // The song and where in it at the moment of the change (a new
                // song, pause, resume); the sidebar card has the live progress.
                let player = crate::player::get();
                if let Some(song) = player.current() {
                    let seconds = player.position_secs().unwrap_or(0.0);
                    page.show_continue(song.album_id.as_deref().unwrap_or(""), &song.id, seconds);
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
                let player = crate::player::get();
                // Already in the player: the card is a pause button then, as its
                // label says. Starting over would rewind the track and replace
                // whatever queue is playing with the album.
                let in_player = player
                    .current()
                    .is_some_and(|now| songs.get(index).is_some_and(|song| song.id == now.id));
                if in_player {
                    player.toggle_pause();
                } else {
                    player.play_from(client, songs, index, seconds);
                }
            }
        });
    }

    /// The "Continue" card after startup: what played last, as resume.rs saved it.
    fn update_continue(&self) {
        // Something plays: that, rather than what was saved a while ago.
        let player = crate::player::get();
        if let Some(song) = player.current() {
            let seconds = player.position_secs().unwrap_or(0.0);
            self.show_continue(song.album_id.as_deref().unwrap_or(""), &song.id, seconds);
            return;
        }
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
        // The card keeps what it shows until the new content is known, so a
        // refresh (after connecting, after a sync) does not make it blink. It
        // hides only when there is nothing to show.
        let Some(client) = imp.client.borrow().clone() else {
            imp.continue_card.set_visible(false);
            return;
        };
        let Some(album) = imp.albums.borrow().iter().find(|a| a.id == album_id).cloned() else {
            imp.continue_card.set_visible(false);
            return;
        };
        // The songs come from the database in the background; the rest of the
        // card follows once they are there.
        let page = self.downgrade();
        let song_id = song_id.to_owned();
        let server = client.server_key();
        let album_id = album.id.clone();
        glib::spawn_future_local(async move {
            let songs = crate::runtime()
                .spawn_blocking(move || crate::library::load_album_songs(&server, &album_id))
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            if let Some(page) = page.upgrade() {
                page.fill_continue(&album, client, songs, &song_id, seconds);
            }
        });
    }

    /// The rest of show_continue, once the album's songs are known.
    fn fill_continue(
        &self,
        album: &Arc<subsonic::Album>,
        client: subsonic::Client,
        songs: Vec<subsonic::Song>,
        song_id: &str,
        seconds: f64,
    ) {
        let imp = self.imp();
        let Some(index) = songs.iter().position(|s| s.id == song_id) else {
            imp.continue_card.set_visible(false);
            return;
        };

        imp.continue_title.set_label(&album.name);
        // What plays: the song with its artist.
        let song = &songs[index];
        let artist = song.artist.as_deref().or(album.artist.as_deref());
        let subtitle = match artist {
            Some(artist) => format!("{} · {artist}", song.title),
            None => song.title.clone(),
        };
        imp.continue_subtitle.set_label(&subtitle);
        // The button says what a click does: pause or resume the song while it
        // is in the player, otherwise continue from where it stopped.
        let player = crate::player::get();
        let in_player = player.current().is_some_and(|now| now.id == song.id);
        let (icon, label) = if in_player && player.is_playing() {
            ("media-playback-pause-symbolic", gettext("Pause"))
        } else if in_player && player.is_paused() {
            ("media-playback-start-symbolic", gettext("Resume"))
        } else {
            ("media-playback-start-symbolic", gettext("Continue"))
        };
        imp.continue_content.set_icon_name(icon);
        imp.continue_content.set_label(&label);
        self.update_similar(song.id.clone(), song.title.clone(), client.clone());

        // A new cover only for a new album: no flicker between its tracks.
        let new_album = *imp.continue_album.borrow() != album.id;
        imp.continue_album.replace(album.id.clone());
        if new_album {
            imp.continue_cover.set_paintable(None::<&gtk::gdk::Paintable>);
            self.tint_hero(None);
        }
        if let Some(cover) = album.cover_art.clone().filter(|_| new_album) {
            crate::covers::load_for(self, Some(client.clone()), cover, |page, texture| {
                page.imp().continue_cover.set_paintable(texture.as_ref());
                page.tint_hero(texture.as_ref());
            });
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

impl StartPage {
    /// "Similar to …": asks the server for songs that sound like `song_id` and
    /// shows the albums they come from. Asked once per song, not on every refresh.
    fn update_similar(&self, song_id: String, title: String, client: subsonic::Client) {
        let imp = self.imp();
        if *imp.similar_seed.borrow() == song_id {
            return;
        }
        imp.similar_seed.replace(song_id.clone());

        let page = self.downgrade();
        glib::spawn_future_local(async move {
            let request = client.clone();
            let result = crate::runtime()
                .spawn(async move { request.similar_songs(&song_id, 40).await })
                .await;
            let Some(page) = page.upgrade() else {
                return;
            };
            let songs = match result {
                Ok(Ok(songs)) => songs,
                _ => Vec::new(),
            };
            page.show_similar(&title, songs, client);
        });
    }

    /// One card per album the similar songs come from, most similar first.
    fn show_similar(&self, title: &str, songs: Vec<subsonic::Song>, client: subsonic::Client) {
        let imp = self.imp();
        let albums = imp.albums.borrow();
        // The album already on the "Continue" card is left out: it is right there.
        let current_album = imp.continue_album.borrow().clone();
        let mut seen = std::collections::HashSet::new();
        let shelf: Vec<&Arc<subsonic::Album>> = songs
            .iter()
            .filter_map(|s| s.album_id.as_deref())
            .filter(|id| *id != current_album)
            .filter(|id| seen.insert(id.to_string()))
            .filter_map(|id| albums.iter().find(|a| a.id == id))
            .take(SHELF_SIZE)
            .collect();

        // Translators: {title} is the title of the song that played last.
        imp.similar_title
            .set_label(&gettext("Similar to “{title}”").replace("{title}", title));
        fill_shelf(&imp.similar_row, &imp.similar_ids, &shelf, Some(client));
        imp.similar_shelf.set_visible(!shelf.is_empty());
        let songs: Rc<[subsonic::Song]> = songs.into();
        imp.similar.replace(Some(songs));
    }
}

/// The ‹ › buttons of a shelf: each moves the row by what is visible, and is
/// greyed out at its end of the row; both hide when the whole row fits.
fn connect_arrows(scroll: &gtk::ScrolledWindow, prev: &gtk::Button, next: &gtk::Button) {
    let adj = scroll.hadjustment();
    let update = {
        let prev = prev.downgrade();
        let next = next.downgrade();
        move |adj: &gtk::Adjustment| {
            let (Some(prev), Some(next)) = (prev.upgrade(), next.upgrade()) else {
                return;
            };
            let end = adj.upper() - adj.page_size();
            let overflow = end - adj.lower() > 1.0;
            // Opacity rather than visibility: this runs while the row is being
            // laid out, and showing or hiding widgets then upsets GTK. The
            // buttons keep their room, so the header does not change height.
            let opacity = if overflow { 1.0 } else { 0.0 };
            prev.set_opacity(opacity);
            next.set_opacity(opacity);
            prev.set_sensitive(overflow && adj.value() > adj.lower() + 1.0);
            next.set_sensitive(overflow && adj.value() < end - 1.0);
        }
    };
    adj.connect_value_changed(update.clone());
    // Also when the row or the window changes size.
    adj.connect_changed(update.clone());
    update(&adj);

    for (button, direction) in [(prev, -1.0), (next, 1.0)] {
        let adj = adj.clone();
        button.connect_clicked(move |button| {
            let to = (adj.value() + direction * adj.page_size())
                .clamp(adj.lower(), adj.upper() - adj.page_size());
            let target = adw::CallbackAnimationTarget::new({
                let adj = adj.clone();
                move |value| adj.set_value(value)
            });
            let animation = adw::TimedAnimation::new(button, adj.value(), to, 250, target);
            animation.set_easing(adw::Easing::EaseOutCubic);
            adw::prelude::AnimationExt::play(&animation);
        });
    }
}

/// The mouse wheel over a shelf moves it sideways. At the end of the row the
/// wheel scrolls the page again, and a touchpad keeps its own two-finger
/// scrolling.
fn connect_wheel(scroll: &gtk::ScrolledWindow) {
    let adj = scroll.hadjustment();
    let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    // Before the row's own scrolling, which would pass the wheel to the page.
    wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
    wheel.connect_scroll(move |controller, _, dy| {
        let touchpad = controller
            .current_event_device()
            .is_some_and(|device| device.source() == gtk::gdk::InputSource::Touchpad);
        if touchpad {
            return gtk::glib::Propagation::Proceed;
        }
        let end = adj.upper() - adj.page_size();
        let to = (adj.value() + dy * adj.page_size() / 3.0).clamp(adj.lower(), end);
        // Nothing left to move in that direction: the page scrolls instead.
        if (to - adj.value()).abs() < 0.5 {
            return gtk::glib::Propagation::Proceed;
        }
        adj.set_value(to);
        gtk::glib::Propagation::Stop
    });
    scroll.add_controller(wheel);
}

impl StartPage {
    /// Fills the "Random Album" card with an album from the library, other
    /// than the one it shows now.
    fn show_random(&self) {
        let imp = self.imp();
        let albums = imp.albums.borrow().clone();
        let current = imp.random_album.borrow().as_ref().map(|a| a.id.clone());
        let choices: Vec<&Arc<subsonic::Album>> = albums
            .iter()
            .filter(|a| Some(&a.id) != current.as_ref())
            .collect();
        if choices.is_empty() {
            imp.random_card.set_visible(!albums.is_empty());
            return;
        }
        let pick = glib::random_int_range(0, choices.len() as i32) as usize;
        let album = Arc::clone(choices[pick]);

        imp.random_title.set_label(&album.name);
        imp.random_subtitle
            .set_label(album.artist.as_deref().unwrap_or_default());
        imp.random_cover.set_paintable(None::<&gtk::gdk::Paintable>);
        if let Some(cover) = album.cover_art.clone() {
            crate::covers::load_for(
                &imp.random_cover.get(),
                imp.client.borrow().clone(),
                cover,
                |picture, texture| picture.set_paintable(texture.as_ref()),
            );
        }
        imp.random_album.replace(Some(album));
        imp.random_card.set_visible(true);
    }

    /// Plays the album on the "Random Album" card from its first song. The
    /// songs come from the database in the background.
    fn play_random(&self) {
        let imp = self.imp();
        let album = imp.random_album.borrow().clone();
        let client = imp.client.borrow().clone();
        let (Some(album), Some(client)) = (album, client) else {
            return;
        };
        let server = client.server_key();
        let command = crate::player::get().command_id();
        glib::spawn_future_local(async move {
            let album_id = album.id.clone();
            let songs = crate::runtime()
                .spawn_blocking(move || crate::library::load_album_songs(&server, &album_id))
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            // Something else was started meanwhile: this answer is stale.
            if songs.is_empty() || !crate::player::get().is_current(command) {
                return;
            }
            crate::player::get().play(client, songs.into(), 0);
        });
    }
}

impl StartPage {
    /// Colours the hero card after its cover (tint.rs); no cover, no colour.
    fn tint_hero(&self, texture: Option<&gtk::gdk::Texture>) {
        let Some(tint) = self.imp().hero_tint.get() else {
            return;
        };
        match texture.and_then(crate::tint::dominant_color) {
            Some(color) => tint.set_gradient(color),
            None => tint.clear(),
        }
    }
}

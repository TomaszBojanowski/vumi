//! The Start page: shelves that answer "what shall I listen to now".
//! For now "Recently added"; "Continue" and "Similar to…" come next.

use adw::prelude::BreakpointBinExt;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;
use std::rc::Rc;
use std::sync::Arc;

use crate::album_card::AlbumCard;
use crate::main_view::MainView;

/// How many albums one shelf shows.
const SHELF_SIZE: usize = 12;

/// How many times the "Random Album" card draws before it takes an album it
/// should not show (one in the player or on a card): enough for a library of
/// more than a few albums, and a small library cannot hold it up.
const RANDOM_TRIES: usize = 5;

/// How many suggestions of the "Random Album" card ‹ can go back to, and ›
/// forward to again. Kept in memory only, for this run.
const RANDOM_HISTORY: usize = 10;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/start_page.blp")]
    pub struct StartPage {
        #[template_child]
        pub breakpoint_bin: gtk::TemplateChild<adw::BreakpointBin>,
        #[template_child]
        pub narrow_breakpoint: gtk::TemplateChild<adw::Breakpoint>,
        #[template_child]
        pub recent_shelf: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub recent_row: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub recent_scroll: gtk::TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub recent_arrows: gtk::TemplateChild<gtk::Box>,
        /// The albums on each shelf now, so an unchanged shelf is not rebuilt.
        pub recent_ids: RefCell<Vec<String>>,
        pub frequent_ids: RefCell<Vec<String>>,
        pub similar_ids: RefCell<Vec<String>>,
        #[template_child]
        pub frequent_shelf: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub frequent_row: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub frequent_scroll: gtk::TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub frequent_arrows: gtk::TemplateChild<gtk::Box>,
        /// The ids of the most played albums, as the server ranks them.
        pub frequent: RefCell<Vec<String>>,
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
        pub similar_arrows: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub random_card: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub random_covers: gtk::TemplateChild<gtk::Stack>,
        #[template_child]
        pub random_names: gtk::TemplateChild<gtk::Stack>,
        #[template_child]
        pub random_play: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub random_shuffle: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub random_arrows: gtk::TemplateChild<gtk::Box>,
        /// The album on the "Random Album" card.
        pub random_album: RefCell<Option<Arc<subsonic::Album>>>,
        /// The card's suggestions as ids, like a browser's history: before the
        /// one shown (oldest first) and after it, once ‹ has gone back (the
        /// nearest last); at most RANDOM_HISTORY each. Its ‹ button is greyed
        /// out when none before can be shown.
        pub random_back: RefCell<Vec<String>>,
        pub random_forward: RefCell<Vec<String>>,
        pub random_prev: std::cell::OnceCell<gtk::Button>,
        /// The card's two copies of cover and names, and which one is shown.
        pub(crate) random_faces: std::cell::OnceCell<[super::RandomFace; 2]>,
        pub random_face: std::cell::Cell<usize>,
        /// The tints of the two cards, after their covers (tint.rs).
        pub(crate) continue_tint: std::cell::OnceCell<crate::tint::Dynamic>,
        pub(crate) random_tint: std::cell::OnceCell<crate::tint::Dynamic>,
        /// The similar songs, played by the radio button next to the shelf title.
        pub similar: RefCell<Option<Rc<[subsonic::Song]>>>,
        /// The song they are similar to, so the server is asked once per song.
        pub similar_seed: RefCell<String>,
        /// What "Resume" plays: the album's songs, the track, the second.
        pub(crate) resume: RefCell<Option<super::Resume>>,
        /// The album on the card and its songs as last read from the database,
        /// so a change that keeps the album (pause, the next track, the volume)
        /// does not read them again.
        pub continue_songs: RefCell<Option<(String, Rc<[subsonic::Song]>)>>,
        /// Bumped by every show_continue; a database answer for an older call
        /// is dropped.
        pub continue_generation: std::cell::Cell<u32>,
        /// The library and the client, so the cards can follow the player.
        pub albums: RefCell<Vec<Arc<subsonic::Album>>>,
        pub client: RefCell<Option<subsonic::Client>>,
        /// The album on the card, so its cover is reloaded only when it changes.
        pub continue_album: RefCell<String>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for StartPage {
        // Must match "template $VumiStartPage" in the Blueprint file.
        const NAME: &'static str = "VumiStartPage";
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
        self.show_frequent();
        // No library (logged out): the pick goes too, or the card would stay
        // hidden after logging in again with the same album still remembered.
        if albums.is_empty() {
            imp.random_album.replace(None);
            imp.random_back.borrow_mut().clear();
            imp.random_forward.borrow_mut().clear();
        }
        // A new pick only when there is none yet, or it left the library.
        let keep = imp
            .random_album
            .borrow()
            .as_ref()
            .is_some_and(|a| albums.iter().any(|b| b.id == a.id));
        if !keep {
            self.show_random();
        } else {
            // The first call comes from the database, before the server answers:
            // a cover that was not on disk then can be fetched now.
            self.load_random_cover();
        }
        // The library may have changed (a sync, another account), so the Last
        // Played card reads its album's songs from the database again.
        self.update_top(true);
        imp.recent_shelf.set_visible(!recent.is_empty());
    }
}

impl StartPage {
    /// "Most Played": the server's ranking, kept as ids; the shelf shows the
    /// matching albums from the library, in that order.
    pub(crate) fn set_frequent(&self, ids: Vec<String>) {
        self.imp().frequent.replace(ids);
        self.show_frequent();
    }

    fn show_frequent(&self) {
        let imp = self.imp();
        let albums = imp.albums.borrow();
        let shelf: Vec<&Arc<subsonic::Album>> = imp
            .frequent
            .borrow()
            .iter()
            .filter_map(|id| albums.iter().find(|a| a.id == *id))
            .take(SHELF_SIZE)
            .collect();
        fill_shelf(
            &imp.frequent_row,
            &imp.frequent_ids,
            &shelf,
            imp.client.borrow().clone(),
        );
        imp.frequent_shelf.set_visible(!shelf.is_empty());
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

/// Where "Resume" picks up: the album's songs, the track and the second.
pub(crate) struct Resume {
    client: subsonic::Client,
    songs: Rc<[subsonic::Song]>,
    index: usize,
    seconds: f64,
}

/// The two cards at the top of the page.
#[derive(Clone, Copy)]
enum Card {
    Continue,
    Random,
}

/// One copy of what the "Random Album" card shows of its album: the cover in
/// its frame, the title and the artist. The card has two, one part in each
/// of its stacks (start_page.blp): a new pick goes into the hidden copy and
/// the stacks crossfade to it.
pub(crate) struct RandomFace {
    frame: gtk::Overlay,
    cover: gtk::Picture,
    names: gtk::Box,
    title: gtk::Label,
    subtitle: gtk::Label,
}

impl RandomFace {
    /// Makes one copy and adds its cover and its names to the two stacks.
    fn new(covers: &gtk::Stack, names_stack: &gtk::Stack) -> Self {
        // A disc until the cover is there; the stack sets the frame's size.
        let placeholder = gtk::Image::builder()
            .icon_name("media-optical-symbolic")
            .pixel_size(40)
            .css_classes(["dim-label"])
            .build();
        let cover = gtk::Picture::builder()
            .content_fit(gtk::ContentFit::Contain)
            .build();
        let frame = gtk::Overlay::builder()
            .child(&placeholder)
            .overflow(gtk::Overflow::Hidden)
            .css_classes(["continue-cover"])
            .build();
        frame.add_overlay(&cover);

        // Long names shorten, so the card never grows past its width.
        let label = |class: &str| {
            gtk::Label::builder()
                .xalign(0.0)
                .ellipsize(gtk::pango::EllipsizeMode::End)
                .max_width_chars(24)
                .css_classes([class])
                .build()
        };
        let title = label("title-3");
        let subtitle = label("dim-label");
        let names = gtk::Box::builder()
            .orientation(gtk::Orientation::Vertical)
            .spacing(4)
            .build();
        names.append(&title);
        names.append(&subtitle);

        covers.add_child(&frame);
        names_stack.add_child(&names);
        Self {
            frame,
            cover,
            names,
            title,
            subtitle,
        }
    }
}

/// The album in the player, if there is one: the sidebar shows it, so the
/// cards at the top never do.
fn album_in_player() -> Option<String> {
    crate::player::get()
        .current()
        .and_then(|song| song.album_id)
        .filter(|id| !id.is_empty())
}

impl StartPage {
    /// Connects the buttons and the player (once, when the page is created).
    fn setup(&self) {
        {
            let imp = self.imp();
            let (previous, next) = (gettext("Previous Albums"), gettext("Next Albums"));
            for (place, scroll) in [
                (&imp.recent_arrows, &imp.recent_scroll),
                (&imp.frequent_arrows, &imp.frequent_scroll),
                (&imp.similar_arrows, &imp.similar_scroll),
            ] {
                let (prev_button, next_button) = add_arrows(place, &previous, &next);
                connect_arrows(scroll, &prev_button, &next_button);
            }
            connect_wheel(&imp.recent_scroll);
            connect_wheel(&imp.frequent_scroll);
            connect_wheel(&imp.similar_scroll);
            connect_drag(&imp.recent_scroll);
            connect_drag(&imp.frequent_scroll);
            connect_drag(&imp.similar_scroll);
            // Both cards take the colour of their cover (show_card_cover).
            for (card, tint) in [
                (&imp.continue_card, &imp.continue_tint),
                (&imp.random_card, &imp.random_tint),
            ] {
                card.add_css_class(tint.get_or_init(crate::tint::Dynamic::new).class());
            }
            let _ = imp.random_faces.set([
                RandomFace::new(&imp.random_covers, &imp.random_names),
                RandomFace::new(&imp.random_covers, &imp.random_names),
            ]);
        }

        // Where the random album card goes depends on the Last Played card
        // and on the width (place_random_card).
        {
            let page = self.downgrade();
            self.imp().continue_card.connect_visible_notify(move |_| {
                if let Some(page) = page.upgrade() {
                    page.place_random_card();
                }
            });
            let page = self.downgrade();
            self.imp()
                .breakpoint_bin
                .connect_current_breakpoint_notify(move |_| {
                    if let Some(page) = page.upgrade() {
                        page.place_random_card();
                    }
                });
            self.place_random_card();
        }

        // The "Random Album" card: play it in order or shuffled, or go to the
        // suggestion before (‹) or another one (›) without playing.
        for (button, shuffle) in [
            (&self.imp().random_play, false),
            (&self.imp().random_shuffle, true),
        ] {
            let page = self.downgrade();
            button.connect_clicked(move |_| {
                if let Some(page) = page.upgrade() {
                    page.play_random(shuffle);
                }
            });
        }
        {
            let (prev_button, next_button) = add_arrows(
                &self.imp().random_arrows,
                &gettext("Show Previous Suggestion"),
                &gettext("Show Next Suggestion"),
            );
            prev_button.set_sensitive(false);
            let page = self.downgrade();
            prev_button.connect_clicked(move |_| {
                if let Some(page) = page.upgrade() {
                    page.previous_random();
                }
            });
            let page = self.downgrade();
            next_button.connect_clicked(move |_| {
                if let Some(page) = page.upgrade() {
                    page.next_random();
                }
            });
            let _ = self.imp().random_prev.set(prev_button);
        }
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

        // The cards follow the player: neither may show the album in it.
        // "Similar to …" follows what plays now.
        {
            let page = self.downgrade();
            let listener = crate::player::get().connect_changed(move || {
                let Some(page) = page.upgrade() else {
                    return;
                };
                page.update_top(false);
                let client = page.imp().client.borrow().clone();
                if let (Some(song), Some(client)) = (crate::player::get().current(), client) {
                    page.update_similar(&song, client);
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
            // The card never shows the album in the player, so this always
            // starts its album again, from the song and the second it stopped at.
            if let Some((client, songs, index, seconds)) = resume {
                crate::player::get().play_from(client, songs, index, seconds);
            }
        });
    }

    /// The random album card beside or under the Last Played card at its own
    /// width, or across the whole row: when it is alone there, and in narrow
    /// windows. Set here rather than by the breakpoints, which know only the
    /// width and would put back their own value when they stop applying.
    fn place_random_card(&self) {
        let imp = self.imp();
        let alone = !imp.continue_card.is_visible();
        let narrow =
            imp.breakpoint_bin.current_breakpoint().as_ref() == Some(&*imp.narrow_breakpoint);
        imp.random_card.set_hexpand(alone);
        imp.random_card.set_halign(if alone || narrow {
            gtk::Align::Fill
        } else {
            gtk::Align::Start
        });
    }

    /// What the Last Played card shows: the newest album played that is not
    /// `in_player` and is in the library.
    fn last_played(&self, in_player: Option<&str>) -> Option<crate::resume::Played> {
        let albums = self.imp().albums.borrow();
        crate::resume::history().into_iter().find(|played| {
            Some(played.album_id.as_str()) != in_player
                && albums.iter().any(|album| album.id == played.album_id)
        })
    }

    /// The albums the "Random Album" card must not show: the one in the player
    /// and the one on the Last Played card.
    fn taken_albums(&self) -> Vec<String> {
        let in_player = album_in_player();
        let last = self
            .last_played(in_player.as_deref())
            .map(|played| played.album_id);
        in_player.into_iter().chain(last).collect()
    }

    /// Both cards at the top, after a change in the player or in the library
    /// (`reload`: read the Last Played album's songs again). The Last Played
    /// card moves on to the newest album other than the one in the player, and
    /// a random album that is now in the player or on that card gives way.
    fn update_top(&self, reload: bool) {
        let in_player = album_in_player();
        let played = self.last_played(in_player.as_deref());
        let taken = [
            in_player.as_deref(),
            played.as_ref().map(|p| p.album_id.as_str()),
        ];
        let random_taken = self
            .imp()
            .random_album
            .borrow()
            .as_ref()
            .is_some_and(|album| taken.contains(&Some(album.id.as_str())));
        self.show_continue(played.as_ref(), reload);
        if random_taken {
            self.show_random();
        }
        self.update_random_arrows();
    }

    /// Fills the Last Played card with `played`, or hides it when there is
    /// none or its song is not in the library. The album's songs are read from
    /// the database only for another album than last time, or when `reload`
    /// asks for it.
    fn show_continue(&self, played: Option<&crate::resume::Played>, reload: bool) {
        let imp = self.imp();
        // This call is the newest now: an answer still on its way from the
        // database belongs to an older one and must not replace the card.
        let generation = imp.continue_generation.get().wrapping_add(1);
        imp.continue_generation.set(generation);
        // The card keeps what it shows until the new content is known, so a
        // refresh (after connecting, after a sync) does not make it blink. It
        // hides only when there is nothing to show.
        let Some(client) = imp.client.borrow().clone() else {
            // Logged out: the songs remembered belong to that account.
            imp.continue_songs.replace(None);
            imp.continue_card.set_visible(false);
            return;
        };
        let Some(played) = played else {
            imp.continue_card.set_visible(false);
            return;
        };
        let Some(album) = imp
            .albums
            .borrow()
            .iter()
            .find(|a| a.id == played.album_id)
            .cloned()
        else {
            imp.continue_card.set_visible(false);
            return;
        };
        let seconds = played.seconds as f64;
        let remembered = imp
            .continue_songs
            .borrow()
            .as_ref()
            .filter(|(id, _)| !reload && *id == album.id)
            .map(|(_, songs)| songs.clone());
        if let Some(songs) = remembered {
            self.fill_continue(&album, client, songs, &played.song_id, seconds);
            return;
        }
        // The songs come from the database in the background; the rest of the
        // card follows once they are there.
        let page = self.downgrade();
        let song_id = played.song_id.clone();
        let server = client.server_key();
        let album_id = album.id.clone();
        glib::spawn_future_local(async move {
            let read = crate::runtime()
                .spawn_blocking(move || crate::library::load_album_songs(&server, &album_id))
                .await
                .ok()
                .and_then(Result::ok);
            let Some(page) = page.upgrade() else {
                return;
            };
            let imp = page.imp();
            if imp.continue_generation.get() != generation {
                return;
            }
            // Only a read that worked is remembered: after an error the next
            // change asks the database again.
            let songs: Rc<[subsonic::Song]> = match read {
                Some(songs) => {
                    let songs: Rc<[subsonic::Song]> = songs.into();
                    imp.continue_songs
                        .replace(Some((album.id.clone(), songs.clone())));
                    songs
                }
                None => Vec::new().into(),
            };
            page.fill_continue(&album, client, songs, &song_id, seconds);
        });
    }

    /// The rest of show_continue, once the album's songs are known.
    fn fill_continue(
        &self,
        album: &Arc<subsonic::Album>,
        client: subsonic::Client,
        songs: Rc<[subsonic::Song]>,
        song_id: &str,
        seconds: f64,
    ) {
        let imp = self.imp();
        let Some(index) = songs.iter().position(|s| s.id == song_id) else {
            imp.continue_card.set_visible(false);
            return;
        };

        imp.continue_title.set_label(&album.name);
        // Where it stopped: the song with its artist.
        let song = &songs[index];
        let artist = song.artist.as_deref().or(album.artist.as_deref());
        let subtitle = match artist {
            Some(artist) => format!("{} · {artist}", song.title),
            None => song.title.clone(),
        };
        imp.continue_subtitle.set_label(&subtitle);
        // With nothing in the player, "Similar to …" follows this song, the one
        // that played last; otherwise the one in the player (setup).
        if crate::player::get().current().is_none() {
            self.update_similar(song, client.clone());
        }

        // A new cover only for a new album: no flicker when the card refreshes.
        let new_album = *imp.continue_album.borrow() != album.id;
        imp.continue_album.replace(album.id.clone());
        if new_album {
            crate::covers::show(&imp.continue_cover, None);
            self.show_card_cover(Card::Continue, &imp.continue_cover, album);
        }

        imp.resume.replace(Some(Resume {
            client,
            songs,
            index,
            seconds,
        }));
        imp.continue_card.set_visible(true);
    }

    /// Shows `album`'s cover in `picture`, on one of the two cards at the top,
    /// and colours that card after it. The card keeps its old colour until the
    /// cover is there, so the colour changes once; a cover that arrives after
    /// the card has moved on to another album is dropped.
    fn show_card_cover(&self, card: Card, picture: &gtk::Picture, album: &subsonic::Album) {
        let Some(cover) = album.cover_art.clone() else {
            if let Some(tint) = self.card_tint(card) {
                tint.set_gradient_from(None);
            }
            return;
        };
        let client = self.imp().client.borrow().clone();
        let album_id = album.id.clone();
        let picture = picture.downgrade();
        crate::covers::load_for(self, client, cover, move |page, texture| {
            if !page.card_shows(card, &album_id) {
                return;
            }
            if let Some(picture) = picture.upgrade() {
                crate::covers::show(&picture, texture.as_ref());
            }
            if let Some(tint) = page.card_tint(card) {
                tint.set_gradient_from(texture.as_ref());
            }
        });
    }

    fn card_tint(&self, card: Card) -> Option<&crate::tint::Dynamic> {
        let imp = self.imp();
        match card {
            Card::Continue => imp.continue_tint.get(),
            Card::Random => imp.random_tint.get(),
        }
    }

    /// True if `card` still shows the album `album_id`.
    fn card_shows(&self, card: Card, album_id: &str) -> bool {
        let imp = self.imp();
        match card {
            Card::Continue => *imp.continue_album.borrow() == album_id,
            Card::Random => imp
                .random_album
                .borrow()
                .as_ref()
                .is_some_and(|album| album.id == album_id),
        }
    }
}

impl StartPage {
    /// "Similar to …": asks the server for songs that sound like `song` and
    /// shows the albums they come from. Asked once per song, not on every refresh.
    fn update_similar(&self, song: &subsonic::Song, client: subsonic::Client) {
        let imp = self.imp();
        if *imp.similar_seed.borrow() == song.id {
            return;
        }
        imp.similar_seed.replace(song.id.clone());

        let song_id = song.id.clone();
        let title = song.title.clone();
        let album_id = song.album_id.clone().unwrap_or_default();
        let page = self.downgrade();
        glib::spawn_future_local(async move {
            let request = client.clone();
            let seed_id = song_id.clone();
            let result = crate::runtime()
                .spawn(async move { request.similar_songs(&seed_id, 40).await })
                .await;
            let Some(page) = page.upgrade() else {
                return;
            };
            // Another song has played meanwhile and asked for its own shelf:
            // this answer would show the wrong albums under its title.
            if *page.imp().similar_seed.borrow() != song_id {
                return;
            }
            let songs = match result {
                Ok(Ok(songs)) => songs,
                _ => Vec::new(),
            };
            page.show_similar(&title, &album_id, songs, client);
        });
    }

    /// One card per album the similar songs come from, most similar first.
    fn show_similar(
        &self,
        title: &str,
        album_id: &str,
        songs: Vec<subsonic::Song>,
        client: subsonic::Client,
    ) {
        let imp = self.imp();
        let albums = imp.albums.borrow();
        // The album of the song itself is left out: it is right there, in the
        // player or on the Last Played card.
        let mut seen = std::collections::HashSet::new();
        let shelf: Vec<&Arc<subsonic::Album>> = songs
            .iter()
            .filter_map(|s| s.album_id.as_deref())
            .filter(|id| *id != album_id)
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

/// Puts `id` on top of a history stack of the "Random Album" card; the oldest
/// entries go beyond RANDOM_HISTORY.
fn keep(stack: &RefCell<Vec<String>>, id: String) {
    let mut stack = stack.borrow_mut();
    stack.push(id);
    let extra = stack.len().saturating_sub(RANDOM_HISTORY);
    stack.drain(..extra);
}

/// Adds the ‹ › pair to `place`, the same on every shelf and on the "Random
/// Album" card: flat round buttons with the arrows of the icon theme.
fn add_arrows(place: &gtk::Box, previous: &str, next: &str) -> (gtk::Button, gtk::Button) {
    place.set_spacing(6);
    let button = |icon: &str, tooltip: &str| {
        let button = gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(tooltip)
            .valign(gtk::Align::Center)
            .css_classes(["circular", "flat"])
            .build();
        place.append(&button);
        button
    };
    (
        button("go-previous-symbolic", previous),
        button("go-next-symbolic", next),
    )
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

/// Shift + the mouse wheel over a shelf moves it sideways; the plain wheel
/// scrolls the page, and a touchpad keeps its own two-finger scrolling.
fn connect_wheel(scroll: &gtk::ScrolledWindow) {
    let adj = scroll.hadjustment();
    let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    // Before the row's own scrolling, which would pass the wheel to the page.
    wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
    wheel.connect_scroll(move |controller, _, dy| {
        let touchpad = controller
            .current_event_device()
            .is_some_and(|device| device.source() == gtk::gdk::InputSource::Touchpad);
        // The plain wheel scrolls the page, as anywhere else; only with Shift
        // held does it move the row sideways (the ‹ › buttons do that too).
        let shift = controller
            .current_event_state()
            .contains(gtk::gdk::ModifierType::SHIFT_MASK);
        if touchpad || !shift {
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

/// Dragging a shelf with the mouse moves it sideways, as a finger would on a
/// touch screen. The drag claims the pointer only after a few pixels, so a
/// plain click on a cover still opens the album.
fn connect_drag(scroll: &gtk::ScrolledWindow) {
    let adj = scroll.hadjustment();
    let drag = gtk::GestureDrag::new();
    drag.set_button(1);
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    let start = std::rc::Rc::new(std::cell::Cell::new(0.0));
    {
        let start = start.clone();
        let adj = adj.clone();
        drag.connect_drag_begin(move |_, _, _| start.set(adj.value()));
    }
    drag.connect_drag_update(move |gesture, offset_x, _| {
        if offset_x.abs() < 8.0 {
            return;
        }
        gesture.set_state(gtk::EventSequenceState::Claimed);
        let end = adj.upper() - adj.page_size();
        adj.set_value((start.get() - offset_x).clamp(adj.lower(), end));
    });
    scroll.add_controller(drag);
}

impl StartPage {
    /// Loads the cover of the album already on the "Random Album" card again.
    fn load_random_cover(&self) {
        let imp = self.imp();
        let album = imp.random_album.borrow().clone();
        let cover = imp
            .random_faces
            .get()
            .map(|faces| faces[imp.random_face.get()].cover.clone());
        if let (Some(album), Some(cover)) = (album, cover) {
            self.show_card_cover(Card::Random, &cover, &album);
        }
    }

    /// Puts a newly drawn album from the library on the "Random Album" card. It
    /// is drawn again when it is the one on the card now, the one in the player
    /// or the one on the Last Played card, up to RANDOM_TRIES draws. The album
    /// it replaces is kept for ‹; the suggestions ‹ had gone back from are
    /// dropped, as a browser drops its forward history.
    fn show_random(&self) {
        let imp = self.imp();
        let mut avoid = self.taken_albums();
        let shown = imp.random_album.borrow().as_ref().map(|a| a.id.clone());
        avoid.extend(shown.clone());
        let album = {
            let albums = imp.albums.borrow();
            if albums.is_empty() {
                drop(albums);
                imp.random_album.replace(None);
                imp.random_back.borrow_mut().clear();
                imp.random_forward.borrow_mut().clear();
                imp.random_card.set_visible(false);
                self.update_random_arrows();
                return;
            }
            let mut album = &albums[0];
            for _ in 0..RANDOM_TRIES {
                album = &albums[fastrand::usize(..albums.len())];
                if !avoid.contains(&album.id) {
                    break;
                }
            }
            Arc::clone(album)
        };
        // Every draw gave the album on the card (a library of one or two):
        // it stays, without fading into itself.
        if shown.as_ref() == Some(&album.id) {
            imp.random_card.set_visible(true);
            return;
        }
        imp.random_forward.borrow_mut().clear();
        if let Some(shown) = shown {
            keep(&imp.random_back, shown);
        }
        self.put_random(album);
        self.update_random_arrows();
    }

    /// ›: forward again to the suggestion ‹ went back from, or, at the end of
    /// the history, a newly drawn album.
    fn next_random(&self) {
        let imp = self.imp();
        match self.usable(&imp.random_forward) {
            Some(album) => {
                if let Some(shown) = imp.random_album.borrow().as_ref().map(|a| a.id.clone()) {
                    keep(&imp.random_back, shown);
                }
                self.put_random(album);
                self.update_random_arrows();
            }
            None => self.show_random(),
        }
    }

    /// ‹: back to the suggestion before; the one shown now can be reached
    /// again with ›.
    fn previous_random(&self) {
        let imp = self.imp();
        if let Some(album) = self.usable(&imp.random_back) {
            if let Some(shown) = imp.random_album.borrow().as_ref().map(|a| a.id.clone()) {
                keep(&imp.random_forward, shown);
            }
            self.put_random(album);
        }
        self.update_random_arrows();
    }

    /// Takes the nearest suggestion off `stack` that can be shown: one that is
    /// now in the player or on the Last Played card, or no longer in the
    /// library, is skipped and forgotten.
    fn usable(&self, stack: &RefCell<Vec<String>>) -> Option<Arc<subsonic::Album>> {
        let taken = self.taken_albums();
        loop {
            let id = stack.borrow_mut().pop()?;
            if taken.contains(&id) {
                continue;
            }
            let album = self
                .imp()
                .albums
                .borrow()
                .iter()
                .find(|a| a.id == id)
                .cloned();
            if album.is_some() {
                return album;
            }
        }
    }

    /// ‹ is greyed out when none of the earlier suggestions can be shown.
    fn update_random_arrows(&self) {
        let imp = self.imp();
        let Some(prev_button) = imp.random_prev.get() else {
            return;
        };
        let taken = self.taken_albums();
        let albums = imp.albums.borrow();
        let possible = imp
            .random_back
            .borrow()
            .iter()
            .any(|id| !taken.contains(id) && albums.iter().any(|a| a.id == *id));
        prev_button.set_sensitive(possible);
    }

    /// Shows `album` on the "Random Album" card: it goes into the copy of
    /// cover and names that is hidden now, and the stacks fade over to it.
    fn put_random(&self, album: Arc<subsonic::Album>) {
        let imp = self.imp();
        let Some(faces) = imp.random_faces.get() else {
            return;
        };
        let next = 1 - imp.random_face.get();
        let face = &faces[next];
        face.title.set_label(&album.name);
        face.subtitle
            .set_label(album.artist.as_deref().unwrap_or_default());
        crate::covers::show(&face.cover, None);
        imp.random_album.replace(Some(Arc::clone(&album)));
        self.show_card_cover(Card::Random, &face.cover, &album);
        imp.random_covers.set_visible_child(&face.frame);
        imp.random_names.set_visible_child(&face.names);
        imp.random_face.set(next);
        imp.random_card.set_visible(true);
    }

    /// Puts the album on the "Random Album" card into the player, from its
    /// first song or shuffled, and another album on the card right away. The
    /// songs come from the database in the background.
    fn play_random(&self, shuffle: bool) {
        let imp = self.imp();
        let album = imp.random_album.borrow().clone();
        let client = imp.client.borrow().clone();
        let (Some(album), Some(client)) = (album, client) else {
            return;
        };
        // The album played is the card's own now, so the next draw leaves it
        // out; once it plays, the player has it.
        self.show_random();
        let server = client.server_key();
        let command = crate::player::get().command_id();
        glib::spawn_future_local(async move {
            let album_id = album.id.clone();
            let mut songs = crate::runtime()
                .spawn_blocking(move || crate::library::load_album_songs(&server, &album_id))
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            // Something else was started meanwhile: this answer is stale.
            if songs.is_empty() || !crate::player::get().is_current(command) {
                return;
            }
            if shuffle {
                fastrand::shuffle(&mut songs);
            }
            crate::player::get().play(client, songs.into(), 0);
        });
    }
}

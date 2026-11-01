//! The Start page: shelves that answer "what shall I listen to now":
//! "Recently Played", "Recently Added" and "Similar to …".

use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::collections::HashMap;
use std::rc::Rc;
use std::sync::Arc;

use crate::album_card::AlbumCard;
use crate::continue_tile::{ContinueTile, TileState};
use crate::main_view::MainView;
use crate::player::QueueOf;
use crate::resume::Played;

/// How many albums one shelf shows.
const SHELF_SIZE: usize = 12;

/// How many albums "Recently Played" shows: as many as the history keeps.
const TILES: usize = 10;

/// Where the album of a "Recently Played" tile stopped, and its songs.
pub(crate) struct TileData {
    song_id: String,
    seconds: u32,
    songs: Rc<[subsonic::Song]>,
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/start_page.blp")]
    pub struct StartPage {
        #[template_child]
        pub continue_shelf: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub continue_row: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub continue_scroll: gtk::TemplateChild<gtk::ScrolledWindow>,
        #[template_child]
        pub continue_arrows: gtk::TemplateChild<gtk::Box>,
        /// The tiles of "Recently Played" by album id, in their order.
        pub tiles: RefCell<Vec<(String, ContinueTile)>>,
        /// Where each tile's album stopped, for its button.
        pub(crate) tile_data: RefCell<HashMap<String, super::TileData>>,
        /// The songs of the tiles' albums as read from the database; read
        /// again after a sync.
        pub tile_songs: RefCell<HashMap<String, Rc<[subsonic::Song]>>>,
        /// Bumped by every refresh_tiles; an older database answer is dropped.
        pub tiles_generation: Cell<u32>,
        /// A refresh is waiting for the main loop (schedule_tiles).
        pub tiles_pending: Cell<bool>,
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
        pub similar_ids: RefCell<Vec<String>>,
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
        /// The similar songs, played by the radio button next to the shelf title.
        pub similar: RefCell<Option<Rc<[subsonic::Song]>>>,
        /// The song they are similar to, so the server is asked once per song.
        pub similar_seed: RefCell<String>,
        /// The library and the client, so the shelves can follow the player.
        pub albums: RefCell<Vec<Arc<subsonic::Album>>>,
        pub client: RefCell<Option<subsonic::Client>>,
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
        if crate::player::get().current().is_none() {
            self.similar_to_last_played();
        }
        // The library may have changed (a sync, another account): the albums'
        // songs are read again.
        self.refresh_tiles(true);
        imp.recent_shelf.set_visible(!recent.is_empty());
    }
}

impl StartPage {
    /// Refreshes "Recently Played" once the current change of the player is
    /// handled: resume.rs writes the history on that change after this page.
    fn schedule_tiles(&self) {
        if self.imp().tiles_pending.replace(true) {
            return;
        }
        let page = self.downgrade();
        glib::idle_add_local_once(move || {
            if let Some(page) = page.upgrade() {
                page.imp().tiles_pending.set(false);
                page.refresh_tiles(false);
            }
        });
    }

    /// "Recently Played" from the history. The albums' songs come from the
    /// database, once per album (again with `reload`, after a sync); then
    /// fill_tiles shows them.
    fn refresh_tiles(&self, reload: bool) {
        let imp = self.imp();
        let generation = imp.tiles_generation.get().wrapping_add(1);
        imp.tiles_generation.set(generation);
        let Some(client) = imp.client.borrow().clone() else {
            // Logged out: nothing of that account stays.
            imp.tile_songs.borrow_mut().clear();
            self.fill_tiles(Vec::new(), None);
            return;
        };
        if reload {
            imp.tile_songs.borrow_mut().clear();
        }
        let recent = recent_albums(crate::resume::history(), &imp.albums.borrow());
        let missing: Vec<String> = {
            let songs = imp.tile_songs.borrow();
            recent
                .iter()
                .map(|(album, _)| album.id.clone())
                .filter(|id| !songs.contains_key(id))
                .collect()
        };
        if missing.is_empty() {
            self.fill_tiles(recent, Some(client));
            return;
        }
        let server = client.server_key();
        let page = self.downgrade();
        glib::spawn_future_local(async move {
            // Every album gets its list, empty after an error, so none is
            // asked for again and again.
            let read = crate::runtime()
                .spawn_blocking(move || {
                    missing
                        .into_iter()
                        .map(|id| {
                            let songs = crate::library::load_album_songs(&server, &id);
                            (id, songs.unwrap_or_default())
                        })
                        .collect::<Vec<_>>()
                })
                .await;
            let (Some(page), Ok(read)) = (page.upgrade(), read) else {
                return;
            };
            let imp = page.imp();
            if imp.tiles_generation.get() != generation {
                return;
            }
            {
                let mut songs = imp.tile_songs.borrow_mut();
                for (id, list) in read {
                    songs.insert(id, list.into());
                }
            }
            page.refresh_tiles(false);
        });
    }

    /// Shows `recent` as tiles. The row is rebuilt only when the albums
    /// change; where each album is, how far through it and where it resumes
    /// follow every time. An entry whose song has left the album is left out.
    fn fill_tiles(
        &self,
        recent: Vec<(Arc<subsonic::Album>, Played)>,
        client: Option<subsonic::Client>,
    ) {
        let imp = self.imp();
        let songs = imp.tile_songs.borrow().clone();
        let tiles: Vec<(Arc<subsonic::Album>, TileData)> = recent
            .into_iter()
            .filter_map(|(album, played)| {
                let list = songs.get(&album.id)?.clone();
                list.iter().any(|song| song.id == played.song_id).then(|| {
                    let data = TileData {
                        song_id: played.song_id,
                        seconds: played.seconds,
                        songs: list,
                    };
                    (album, data)
                })
            })
            .collect();

        let ids: Vec<String> = tiles.iter().map(|(album, _)| album.id.clone()).collect();
        let shown: Vec<String> = imp
            .tiles
            .borrow()
            .iter()
            .map(|(id, _)| id.clone())
            .collect();
        if ids != shown {
            while let Some(child) = imp.continue_row.first_child() {
                imp.continue_row.remove(&child);
            }
            let mut made = Vec::new();
            for (album, _) in &tiles {
                let tile = ContinueTile::new();
                tile.set_album(album, client.clone());
                let opened = Arc::clone(album);
                tile.connect_open(move |button| {
                    if let Some(main) = button
                        .ancestor(MainView::static_type())
                        .and_downcast::<MainView>()
                    {
                        main.open_album_from(button, &opened.id, (*opened).clone());
                    }
                });
                let page = self.downgrade();
                let album_id = album.id.clone();
                tile.connect_play(move || {
                    if let Some(page) = page.upgrade() {
                        page.play_tile(&album_id);
                    }
                });
                imp.continue_row.append(&tile);
                made.push((album.id.clone(), tile));
            }
            imp.tiles.replace(made);
        } else if let Some(client) = &client {
            // The same tiles: covers not on disk before can be fetched now.
            for (_, tile) in imp.tiles.borrow().iter() {
                tile.retry_cover(client.clone());
            }
        }

        let player = crate::player::get();
        let in_player = album_in_player();
        for ((album, data), (_, tile)) in tiles.iter().zip(imp.tiles.borrow().iter()) {
            let state = tile_state(
                &album.id,
                in_player
                    .as_ref()
                    .map(|(id, playing)| (id.as_str(), *playing)),
            );
            tile.set_state(state);
            // The album in the player: where it is now, not where it was saved.
            let progress = match (state, player.current()) {
                (TileState::Saved, _) | (_, None) => {
                    album_progress(&data.songs, &data.song_id, f64::from(data.seconds))
                }
                (_, Some(song)) => {
                    album_progress(&data.songs, &song.id, player.position_secs().unwrap_or(0.0))
                }
            };
            tile.set_progress(progress);
        }
        imp.tile_data.replace(
            tiles
                .into_iter()
                .map(|(album, data)| (album.id.clone(), data))
                .collect(),
        );
        imp.continue_shelf
            .set_visible(!imp.tiles.borrow().is_empty());
    }

    /// The round button of a tile: pauses or resumes the album in the player,
    /// or plays the album again from the song and the second it stopped at.
    fn play_tile(&self, album_id: &str) {
        let player = crate::player::get();
        if album_in_player().is_some_and(|(id, _)| id == album_id) {
            player.toggle_pause();
            return;
        }
        let imp = self.imp();
        let data = imp
            .tile_data
            .borrow()
            .get(album_id)
            .map(|data| (data.songs.clone(), data.song_id.clone(), data.seconds));
        let client = imp.client.borrow().clone();
        let (Some((songs, song_id, seconds)), Some(client)) = (data, client) else {
            return;
        };
        let Some(index) = songs.iter().position(|song| song.id == song_id) else {
            return;
        };
        player.play_from(
            client,
            songs,
            index,
            f64::from(seconds),
            QueueOf::Album(album_id.to_owned()),
        );
    }
}

/// The albums of the history (newest first) that are in the library, each
/// with where it stopped; at most TILES.
fn recent_albums(
    history: Vec<Played>,
    albums: &[Arc<subsonic::Album>],
) -> Vec<(Arc<subsonic::Album>, Played)> {
    history
        .into_iter()
        .filter_map(|played| {
            let album = albums.iter().find(|album| album.id == played.album_id)?;
            Some((Arc::clone(album), played))
        })
        .take(TILES)
        .collect()
}

/// How far through an album playback is: the songs before `song_id` and
/// `seconds` of it, over the length of the whole album. A song without a
/// length counts as none; None when the song is not there or no length is
/// known at all.
fn album_progress(songs: &[subsonic::Song], song_id: &str, seconds: f64) -> Option<f64> {
    let index = songs.iter().position(|song| song.id == song_id)?;
    let length = |song: &subsonic::Song| f64::from(song.duration.unwrap_or(0));
    let total: f64 = songs.iter().map(length).sum();
    if total <= 0.0 {
        return None;
    }
    let before: f64 = songs[..index].iter().map(length).sum();
    let within = seconds.clamp(0.0, length(&songs[index]));
    Some(((before + within) / total).clamp(0.0, 1.0))
}

/// Where the album `album_id` is, given the album in the player as an album
/// and whether it plays.
fn tile_state(album_id: &str, in_player: Option<(&str, bool)>) -> TileState {
    match in_player {
        Some((id, true)) if id == album_id => TileState::Playing,
        Some((id, false)) if id == album_id => TileState::Paused,
        _ => TileState::Saved,
    }
}

/// The album in the player, if it was started as that album and its track
/// is in progress (playing or paused), and whether it plays. Radio and other
/// lists of songs have none.
fn album_in_player() -> Option<(String, bool)> {
    let player = crate::player::get();
    let song = player.current()?;
    if !player.is_active() || !player.is_album_song(&song) {
        return None;
    }
    let playing = player.is_playing();
    Some((song.album_id?, playing))
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

impl StartPage {
    /// Connects the shelves and the player (once, when the page is created).
    fn setup(&self) {
        {
            let imp = self.imp();
            let (previous, next) = (gettext("Previous Albums"), gettext("Next Albums"));
            for (place, scroll) in [
                (&imp.continue_arrows, &imp.continue_scroll),
                (&imp.recent_arrows, &imp.recent_scroll),
                (&imp.similar_arrows, &imp.similar_scroll),
            ] {
                let (prev_button, next_button) = add_arrows(place, &previous, &next);
                connect_arrows(scroll, &prev_button, &next_button);
                connect_wheel(scroll);
                connect_drag(scroll);
            }
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
                    crate::player::get().play(client, songs, 0, crate::player::QueueOf::Songs);
                }
            });
        }

        // "Recently Played" follows the player and the history; "Similar to …"
        // follows what plays now, or with nothing in the player, what played
        // last.
        {
            let page = self.downgrade();
            let listener = crate::player::get().connect_changed(move || {
                let Some(page) = page.upgrade() else {
                    return;
                };
                page.schedule_tiles();
                let client = page.imp().client.borrow().clone();
                match (crate::player::get().current(), client) {
                    (Some(song), Some(client)) => page.update_similar(&song, client),
                    (None, Some(_)) => page.similar_to_last_played(),
                    _ => {}
                }
            });
            self.connect_destroy(move |_| crate::player::get().disconnect(listener));
        }

        // While the album in the player plays, the line on its tile moves on.
        {
            let page = self.downgrade();
            glib::timeout_add_seconds_local(10, move || {
                let Some(page) = page.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                if page.is_mapped() && crate::player::get().is_playing() {
                    page.refresh_tiles(false);
                }
                glib::ControlFlow::Continue
            });
        }
    }

    /// With nothing in the player, "Similar to …" follows the song that played
    /// last: the newest entry of the history whose album is in the library.
    /// The album's songs come from the database in the background.
    fn similar_to_last_played(&self) {
        let imp = self.imp();
        let Some(client) = imp.client.borrow().clone() else {
            return;
        };
        let played = {
            let albums = imp.albums.borrow();
            crate::resume::history()
                .into_iter()
                .find(|played| albums.iter().any(|album| album.id == played.album_id))
        };
        let Some(played) = played else {
            return;
        };
        let page = self.downgrade();
        let server = client.server_key();
        glib::spawn_future_local(async move {
            let album_id = played.album_id.clone();
            let songs = crate::runtime()
                .spawn_blocking(move || crate::library::load_album_songs(&server, &album_id))
                .await
                .ok()
                .and_then(Result::ok)
                .unwrap_or_default();
            let Some(page) = page.upgrade() else {
                return;
            };
            // Something has started meanwhile: the shelf follows that instead.
            if crate::player::get().current().is_some() {
                return;
            }
            if let Some(song) = songs.iter().find(|song| song.id == played.song_id) {
                page.update_similar(song, client);
            }
        });
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
        // The album of the song itself is left out: it is the one in the
        // player, or the one that played last.
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

/// Adds the ‹ › pair to `place`, the same on every shelf: flat round buttons
/// with the arrows of the icon theme.
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

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str, seconds: Option<u32>) -> subsonic::Song {
        subsonic::Song {
            id: id.into(),
            title: id.into(),
            artist: None,
            track: None,
            disc_number: None,
            album: None,
            album_id: None,
            artist_id: None,
            duration: seconds,
            suffix: None,
            bit_rate: None,
            cover_art: None,
            bit_depth: None,
            sampling_rate: None,
            starred: None,
            composer: None,
        }
    }

    fn album(id: &str) -> Arc<subsonic::Album> {
        Arc::new(subsonic::Album {
            id: id.into(),
            name: id.into(),
            artist: None,
            year: None,
            song_count: None,
            created: None,
            artist_id: None,
            starred: None,
            cover_art: None,
        })
    }

    fn played(album: &str) -> Played {
        Played {
            album_id: album.into(),
            song_id: "s".into(),
            seconds: 0,
        }
    }

    fn ids(tiles: &[(Arc<subsonic::Album>, Played)]) -> Vec<&str> {
        tiles.iter().map(|(album, _)| album.id.as_str()).collect()
    }

    #[test]
    fn progress_counts_the_songs_before_and_the_seconds_into_the_song() {
        let songs = [
            song("a", Some(100)),
            song("b", Some(200)),
            song("c", Some(100)),
        ];
        assert_eq!(album_progress(&songs, "b", 50.0), Some(0.375));
        assert_eq!(album_progress(&songs, "a", 0.0), Some(0.0));
        assert_eq!(album_progress(&songs, "c", 100.0), Some(1.0));
    }

    #[test]
    fn progress_stays_within_the_song_and_the_album() {
        let songs = [
            song("a", Some(100)),
            song("b", Some(200)),
            song("c", Some(100)),
        ];
        assert_eq!(album_progress(&songs, "b", 250.0), Some(0.75));
        assert_eq!(album_progress(&songs, "a", -5.0), Some(0.0));
    }

    #[test]
    fn progress_needs_the_song_and_some_length() {
        let songs = [song("a", Some(100)), song("b", None), song("c", Some(100))];
        assert_eq!(album_progress(&songs, "c", 50.0), Some(0.75));
        assert_eq!(album_progress(&songs, "x", 50.0), None);
        assert_eq!(album_progress(&[song("a", None)], "a", 10.0), None);
        assert_eq!(album_progress(&[], "a", 10.0), None);
    }

    #[test]
    fn recent_albums_keep_the_order_and_leave_out_albums_not_in_the_library() {
        let albums = [album("x"), album("z")];
        let recent = recent_albums(vec![played("x"), played("y"), played("z")], &albums);
        assert_eq!(ids(&recent), ["x", "z"]);
    }

    #[test]
    fn recent_albums_stop_at_ten() {
        let albums: Vec<_> = (0..12).map(|n| album(&n.to_string())).collect();
        let history = (0..12).map(|n| played(&n.to_string())).collect();
        let recent = recent_albums(history, &albums);
        assert_eq!(recent.len(), TILES);
        assert_eq!(ids(&recent)[..2], ["0", "1"]);
    }

    #[test]
    fn only_the_album_in_the_player_plays_or_pauses() {
        assert_eq!(tile_state("a", Some(("a", true))), TileState::Playing);
        assert_eq!(tile_state("a", Some(("a", false))), TileState::Paused);
        assert_eq!(tile_state("a", Some(("b", true))), TileState::Saved);
        assert_eq!(tile_state("a", None), TileState::Saved);
    }
}

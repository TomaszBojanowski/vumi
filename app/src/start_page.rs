//! The Start page: shelves that answer "what shall I listen to now":
//! "Recently Added" and "Similar to …".

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

impl StartPage {
    /// Connects the shelves and the player (once, when the page is created).
    fn setup(&self) {
        {
            let imp = self.imp();
            let (previous, next) = (gettext("Previous Albums"), gettext("Next Albums"));
            for (place, scroll) in [
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

        // "Similar to …" follows what plays now, or with nothing in the
        // player, what played last.
        {
            let page = self.downgrade();
            let listener = crate::player::get().connect_changed(move || {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let client = page.imp().client.borrow().clone();
                match (crate::player::get().current(), client) {
                    (Some(song), Some(client)) => page.update_similar(&song, client),
                    (None, Some(_)) => page.similar_to_last_played(),
                    _ => {}
                }
            });
            self.connect_destroy(move |_| crate::player::get().disconnect(listener));
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

//! Album page: big cover, album info and the track list, grouped by disc.
//! The layout lives in album_page.blp; this file only holds the behaviour.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;

use crate::eq_bars::Mode;
use crate::favorites::{Favorite, star_button};
use crate::{about, covers, error_message, format, player, runtime, tint};

/// Heavy work that waits for the end of the page's slide-in animation.
type AfterShown = Box<dyn FnOnce(&AlbumPage)>;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/album_page.blp")]
    pub struct AlbumPage {
        #[template_child]
        pub cover: gtk::TemplateChild<gtk::Picture>,
        #[template_child]
        pub toolbar: gtk::TemplateChild<adw::ToolbarView>,
        #[template_child]
        pub title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub artist: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub artist_button: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub composer: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub details: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub quality: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub actions: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub about: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub about_text: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub about_more: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub tracks_stack: gtk::TemplateChild<gtk::Stack>,
        #[template_child]
        pub tracks: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub error_status: gtk::TemplateChild<adw::StatusPage>,
        /// Which album this page shows, so it is not opened twice.
        pub album_id: RefCell<String>,
        /// Track rows, to mark the one that is playing.
        pub(crate) rows: RefCell<Vec<super::TrackRow>>,
        /// True once the page has fully slid into view.
        pub shown: Cell<bool>,
        /// Heavy work waiting for the end of the slide-in animation.
        pub after_shown: RefCell<Vec<super::AfterShown>>,
        /// Running fade-in animations, kept alive until the page goes away.
        pub animations: RefCell<Vec<adw::TimedAnimation>>,
        /// Layer under the whole page that carries the cover-coloured gradient.
        pub backdrop: RefCell<Option<crate::tint_layer::TintLayer>>,
        /// The album this page was opened with (star and artist id included).
        pub album: RefCell<Option<subsonic::Album>>,
        /// The play queue: the album's songs, read by the buttons when clicked.
        pub queue: RefCell<Option<Rc<[subsonic::Song]>>>,
        /// The client to play and star with; None while offline (set_client
        /// gives it when the connection comes back). Read when clicked.
        pub client: RefCell<Option<subsonic::Client>>,
        /// The album's year and artist, for a refresh after set_client.
        pub year: Cell<Option<i32>>,
        pub album_artist: RefCell<Option<String>>,
        /// The album's star button, replaced when the server knows better.
        pub album_star: RefCell<Option<gtk::Widget>>,
        /// True once the parts made once per page exist (Play row, listener).
        pub tracks_shown: Cell<bool>,
        /// True once the artist name opens the artist page.
        pub artist_linked: Cell<bool>,
        /// Bumped by every show_tracks; older row batches stop.
        pub row_generation: Cell<u32>,
        /// The icon and the names of the Play pill, which becomes Pause /
        /// Resume for this album.
        pub play: RefCell<Option<(gtk::Image, gtk::Stack)>>,
        /// The track row under the mouse pointer: its star takes clicks.
        pub hovered: RefCell<Option<gtk::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumPage {
        // Must match "template $VumiAlbumPage" in the Blueprint file.
        const NAME: &'static str = "VumiAlbumPage";
        type Type = super::AlbumPage;
        type ParentType = adw::NavigationPage;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for AlbumPage {}
    impl WidgetImpl for AlbumPage {}
    impl NavigationPageImpl for AlbumPage {}
}

glib::wrapper! {
    pub struct AlbumPage(ObjectSubclass<imp::AlbumPage>)
        @extends adw::NavigationPage, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl AlbumPage {
    /// `client` is None while offline: the cover still comes from the disk cache.
    /// Use open() instead: it reads the local songs off the main thread first.
    fn new(
        album: &subsonic::Album,
        client: Option<subsonic::Client>,
        local: Vec<subsonic::Song>,
    ) -> Self {
        let page: Self = glib::Object::new();
        let imp = page.imp();
        crate::offline_banner::attach(&page);

        page.set_title(&album.name);
        imp.album_id.replace(album.id.clone());
        imp.album.replace(Some(album.clone()));
        // The cover-coloured gradient gets its own layer under the page, so it
        // can fade in (a libadwaita animation) instead of popping in.
        page.add_backdrop();
        // The spinner only if loading takes longer than half a second, so a fast
        // server never flashes it.
        let page_weak = page.downgrade();
        glib::timeout_add_local_once(std::time::Duration::from_millis(500), move || {
            if let Some(page) = page_weak.upgrade() {
                let stack = &page.imp().tracks_stack;
                if stack.visible_child_name().as_deref() == Some("empty") {
                    stack.set_visible_child_name("loading");
                }
            }
        });

        // Heavy work (all track rows, the restyle for the cover colour) waits
        // until the slide-in animation has finished, so it keeps its frames.
        page.connect_shown(|page| {
            let imp = page.imp();
            imp.shown.set(true);
            for f in imp.after_shown.take() {
                f(page);
            }
        });
        imp.title.set_label(&album.name);
        imp.artist.set_label(album.artist.as_deref().unwrap_or(""));
        imp.details
            .set_label(&album.year.map(|y| y.to_string()).unwrap_or_default());

        if let Some(id) = album.cover_art.clone() {
            page.load_cover(client.clone(), id);
        }
        imp.client.replace(client.clone());
        imp.year.set(album.year);
        imp.album_artist.replace(album.artist.clone());

        match client {
            Some(client) => {
                page.load_about(client.clone(), album.id.clone());
                page.load_tracks(
                    client,
                    album.id.clone(),
                    album.year,
                    album.artist.clone(),
                    local,
                );
            }
            // Offline: the tracks of the local copy, when there is one; the
            // server refreshes them once the connection is back (set_client).
            None => match local_album_of(album, local) {
                Some(local) => page.show_tracks(&local, album.year, album.artist.as_deref(), None),
                None => page.show_error(&gettext("Connect to the server to see the tracks.")),
            },
        }
        page
    }

    /// The connection is back: the page gets the client and refreshes as at a
    /// normal open (the tracks shown, then the server's), so its rows, stars
    /// and menus play and star again.
    pub(crate) fn set_client(&self, client: subsonic::Client) {
        let imp = self.imp();
        if imp.client.borrow().is_some() {
            return;
        }
        imp.client.replace(Some(client.clone()));
        let id = self.album_id();
        let local = imp
            .queue
            .borrow()
            .as_ref()
            .map(|queue| queue.to_vec())
            .unwrap_or_default();
        self.load_about(client.clone(), id.clone());
        self.load_tracks(
            client,
            id,
            imp.year.get(),
            imp.album_artist.borrow().clone(),
            local,
        );
    }

    fn load_cover(&self, client: Option<subsonic::Client>, id: String) {
        covers::load_for(self, client, id, |page, texture| {
            covers::show(&page.imp().cover, texture.as_ref());
            if let Some(color) = texture.as_ref().and_then(tint::dominant_color) {
                page.after_shown(move |page| page.show_tint(color));
            }
        });
    }

    fn load_tracks(
        &self,
        client: subsonic::Client,
        id: String,
        year: Option<i32>,
        album_artist: Option<String>,
        local: Vec<subsonic::Song>,
    ) {
        // The local copy first (read in the background by open()), so the page
        // slides in already filled.
        let local_album = if local.is_empty() {
            None
        } else {
            self.local_album(local)
        };
        if let Some(album) = &local_album {
            self.show_tracks(album, year, album_artist.as_deref(), Some(client.clone()));
        }

        // Then the server, which may know better: new tracks, stars, disc titles.
        let page = self.downgrade();
        let client_for_page = client.clone();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn(async move { client.album(&id).await })
                .await;
            let Some(page) = page.upgrade() else {
                return;
            };
            // The server did not answer: the main view asks it once whether it
            // is still there.
            if matches!(result, Ok(Err(subsonic::Error::Unreachable(_)))) {
                crate::reconnect::request_failed();
            }
            match result {
                Ok(Ok(album)) => {
                    // Nothing new: the page stays exactly as it is.
                    if local_album
                        .as_ref()
                        .is_some_and(|local| same_album(local, &album))
                    {
                        return;
                    }
                    page.after_shown(move |page| {
                        page.show_tracks(
                            &album,
                            year,
                            album_artist.as_deref(),
                            Some(client_for_page),
                        )
                    })
                }
                // With the local copy on screen, a failed refresh needs no error page.
                Ok(Err(_)) | Err(_) if local_album.is_some() => {}
                Ok(Err(e)) => page.show_error(&error_message(&e)),
                Err(_) => page.show_error(&gettext("Something went wrong. Please try again.")),
            }
        });
    }

    /// Shows the album's tracks. Called with the local copy (before the page
    /// slides in) and again if the server's answer differs: the parts made once
    /// per page are made only once, the rows and the star are rebuilt.
    fn show_tracks(
        &self,
        album: &subsonic::AlbumWithSongs,
        year: Option<i32>,
        album_artist: Option<&str>,
        client: Option<subsonic::Client>,
    ) {
        let imp = self.imp();
        let songs = &album.song;

        // The whole album as one shared play queue; the buttons read it when clicked.
        let queue: Rc<[subsonic::Song]> = songs.as_slice().into();
        imp.queue.replace(Some(queue.clone()));

        if !imp.tracks_shown.replace(true) {
            self.build_actions(&album.id);
            self.install_track_action();
            self.follow_pointer();
            // Mark the playing track whenever the player changes; the page stops
            // listening when it is closed.
            let page = self.downgrade();
            let listener = player::get().connect_changed(move || {
                if let Some(page) = page.upgrade() {
                    page.mark_playing();
                }
            });
            self.connect_destroy(move |_| player::get().disconnect(listener));
        }
        self.connect_artist(album);

        // The album's star is replaced: the server may know better than the local copy.
        if let Some(old) = imp.album_star.take() {
            imp.actions.remove(&old);
        }
        let star = star_button(
            album.starred.is_some(),
            client.clone(),
            Favorite::Album(album.id.clone()),
        );
        imp.actions.append(&star);
        imp.album_star.replace(Some(star.upcast::<gtk::Widget>()));

        // Headers only when the album really has more than one disc.
        let discs: BTreeSet<u32> = songs.iter().filter_map(|s| s.disc_number).collect();
        let multi_disc = discs.len() > 1;

        // The composer named on at least half of the tracks goes into the
        // header (unless it is the album artist); rows name theirs only where
        // it differs, so a band's album with two covers marks just the covers.
        // Composers compare as sets of names: "A • B" and "B • A" are the same.
        let mut counts: std::collections::HashMap<String, (usize, &str)> =
            std::collections::HashMap::new();
        for composer in songs
            .iter()
            .filter_map(|s| s.composer.as_deref())
            .filter(|c| !c.trim().is_empty())
        {
            counts
                .entry(composer_key(composer))
                .or_insert((0, composer))
                .0 += 1;
        }
        let dominant = counts
            .iter()
            .max_by_key(|(_, (count, _))| *count)
            .filter(|(_, (count, _))| count * 2 >= songs.len())
            .map(|(key, (_, text))| (key.clone(), (*text).to_string()));
        let album_artist_key = album_artist.map(composer_key);
        match dominant
            .as_ref()
            .filter(|(key, _)| Some(key) != album_artist_key.as_ref())
        {
            Some((_, name)) => {
                let name = format!("<i>{}</i>", glib::markup_escape_text(name));
                // Translators: {name} is the composer of (most of) the album's tracks.
                let label = gettext("Composer: {name}");
                imp.composer.set_use_markup(true);
                imp.composer
                    .set_label(&glib::markup_escape_text(&label).replace("{name}", &name));
                imp.composer.set_visible(true);
            }
            None => imp.composer.set_visible(false),
        }
        let hidden_composer = dominant.map(|(key, _)| key);

        // The rows start over (a refresh); batches of an older call stop by themselves.
        while let Some(child) = imp.tracks.first_child() {
            imp.tracks.remove(&child);
        }
        imp.rows.replace(Vec::new());
        let generation = imp.row_generation.get() + 1;
        imp.row_generation.set(generation);
        let builder = Rc::new(RowBuilder {
            queue,
            album_artist: album_artist.map(str::to_owned),
            client: client.clone(),
            disc_titles: disc_titles(album),
            multi_disc,
            hidden_composer,
            generation,
            next: Cell::new(0),
            group: RefCell::new(None),
        });
        // The first screenful now, so the page slides in filled; the rest once
        // the slide-in is over, a few rows at a time, so no frame waits long.
        if self.build_rows(&builder, ROW_BATCH) {
            self.after_shown(move |page| {
                let page = page.downgrade();
                glib::idle_add_local(move || {
                    // The page was closed meanwhile: stop building.
                    let Some(page) = page.upgrade() else {
                        return glib::ControlFlow::Break;
                    };
                    let more = page.build_rows(&builder, ROW_BATCH_LATER);
                    page.mark_playing();
                    if more {
                        glib::ControlFlow::Continue
                    } else {
                        glib::ControlFlow::Break
                    }
                });
            });
        }
        self.mark_playing();

        let seconds: u32 = songs.iter().filter_map(|s| s.duration).sum();
        let parts = {
            // The genre and the label as the library has them (offline too).
            let album = imp.album.borrow();
            info_parts(
                year,
                album.as_ref().and_then(|a| a.genre.as_deref()),
                album.as_ref().and_then(|a| a.record_label.as_deref()),
                if multi_disc { discs.len() as u32 } else { 1 },
                songs.len() as u32,
                seconds,
            )
        };
        imp.details.set_label(&info_line(&parts));
        self.show_quality(songs);

        // The list fades in only when it arrives while the page is on screen; the
        // local copy is there before the page slides in and needs no fade.
        let already_visible = imp.tracks_stack.visible_child_name().as_deref() == Some("tracks");
        imp.tracks_stack.set_visible_child_name("tracks");
        if !already_visible && imp.shown.get() {
            self.fade_in(&*imp.tracks);
            self.fade_in(&*imp.actions);
        }
    }

    fn show_quality(&self, songs: &[subsonic::Song]) {
        let quality = &self.imp().quality;
        if let Some((text, hi_res)) = quality_text(songs) {
            quality.set_label(&text);
            if hi_res {
                quality.add_css_class("hi-res");
            }
            quality.set_visible(true);
        }
    }

    fn show_error(&self, message: &str) {
        let imp = self.imp();
        // The description is markup; a server's message may hold "&" or "<".
        imp.error_status
            .set_description(Some(&glib::markup_escape_text(message)));
        imp.tracks_stack.set_visible_child_name("error");
    }
}

/// A track row and the widgets that show whether it is playing. The row is
/// built like the inside of an Adw.ActionRow (a box "header" with the parts
/// in it), so the theme draws it the same way, but holds only what it shows:
/// an ActionRow brings boxes and an icon of its own, and every row had its
/// own bars.
pub(crate) struct TrackRow {
    song_id: String,
    row: adw::PreferencesRow,
    /// The track number, and over it the bars while the track is the
    /// current one.
    slot: gtk::Overlay,
    number: gtk::Label,
    track: String,
    bars: Option<crate::eq_bars::EqBars>,
    /// Seen when the track is a favourite, under the pointer and with the
    /// keyboard focus in the row (style.css); it takes clicks only then
    /// (see AlbumPage::follow_pointer).
    star: gtk::ToggleButton,
}

impl TrackRow {
    /// The bars instead of the number and the highlight while the track is
    /// the current one; the bars are made then and go when it stops being it.
    fn set_playing(&mut self, playing: bool, mode: Mode) {
        if playing {
            if self.bars.is_none() {
                // Three bars that bounce while the track plays and rest while
                // it is paused. The number keeps its width, so the titles stay
                // aligned.
                let bars = crate::eq_bars::EqBars::new();
                self.slot.add_overlay(&bars);
                self.number.set_label("");
                self.bars = Some(bars);
            }
            if let Some(bars) = &self.bars {
                bars.set_mode(mode);
            }
            self.row.add_css_class("playing");
        } else {
            if let Some(bars) = self.bars.take() {
                self.slot.remove_overlay(&bars);
                self.number.set_label(&self.track);
            }
            self.row.remove_css_class("playing");
        }
    }
}

/// One track: number (or the bars while it plays), title, duration,
/// favourite star, ⋮; the artist only if it differs from the album's, the
/// composer only if it is not the one named in the header and not one of the
/// track's performers. A click plays the album from this track (the page's
/// "album.play-track").
fn track_row(
    song: &subsonic::Song,
    album_artist: Option<&str>,
    hidden_composer: Option<&str>,
    client: Option<&subsonic::Client>,
    index: usize,
    queue: &Rc<[subsonic::Song]>,
) -> TrackRow {
    let row = adw::PreferencesRow::builder()
        .activatable(true)
        .css_classes(["track"])
        .build();
    row.set_action_name(Some("album.play-track"));
    row.set_action_target_value(Some(&(index as u32).to_variant()));
    let header = gtk::Box::builder()
        .valign(gtk::Align::Center)
        .css_classes(["header"])
        .build();

    // The track number; the theme gives the "prefixes" the room after them.
    let track = song.track.map(|n| n.to_string()).unwrap_or_default();
    let number = gtk::Label::builder()
        .label(track.as_str())
        .width_chars(3)
        .xalign(1.0)
        .css_classes(["dim-label", "numeric"])
        .build();
    let slot = gtk::Overlay::builder()
        .child(&number)
        .css_classes(["prefixes"])
        .build();
    header.append(&slot);

    let title_box = gtk::Box::builder()
        .orientation(gtk::Orientation::Vertical)
        .valign(gtk::Align::Center)
        .hexpand(true)
        .css_classes(["title"])
        .build();
    // Plain text: names like "Simon & Garfunkel" are not markup.
    let title = gtk::Label::builder()
        .label(song.title.as_str())
        .wrap(true)
        .wrap_mode(gtk::pango::WrapMode::WordChar)
        .xalign(0.0)
        .css_classes(["title"])
        .build();
    title_box.append(&title);

    // Markup for the composer in italics: the artist is escaped.
    let mut subtitle: Vec<String> = Vec::new();
    if let Some(artist) = song
        .artist
        .as_deref()
        .filter(|artist| !of_the_album(artist, album_artist))
    {
        subtitle.push(glib::markup_escape_text(artist).to_string());
    }
    if let Some(composer) = song.composer.as_deref().filter(|c| !c.trim().is_empty()) {
        // Left out when it is the header's composer, or when the composers
        // all perform the track themselves.
        let performers = names_of(song.artist.as_deref().unwrap_or(""));
        let performs = names_of(composer)
            .iter()
            .all(|name| performers.contains(name));
        if Some(composer_key(composer).as_str()) != hidden_composer && !performs {
            // In italics, so the composer is told from the performer.
            subtitle.push(format!(
                "<i>{}</i>",
                glib::markup_escape_text(&short_names(composer))
            ));
        }
    }
    let mut described_by = Vec::new();
    if !subtitle.is_empty() {
        // One line: a long list of performers must not make a row taller
        // than the others.
        let line = gtk::Label::builder()
            .label(subtitle.join(" · "))
            .use_markup(true)
            .wrap(true)
            .wrap_mode(gtk::pango::WrapMode::WordChar)
            .lines(1)
            .ellipsize(gtk::pango::EllipsizeMode::End)
            .xalign(0.0)
            .css_classes(["subtitle"])
            .build();
        title_box.append(&line);
        described_by.push(line.upcast::<gtk::Widget>());
    }
    header.append(&title_box);

    if let Some(d) = song.duration {
        let duration = gtk::Label::builder()
            .label(format::clock(d.into()))
            .css_classes(["dim-label", "numeric"])
            .build();
        header.append(&duration);
    }

    let star = star_button(
        song.starred.is_some(),
        client.cloned(),
        Favorite::Song(song.id.clone()),
    );
    star.add_css_class("track-star");
    star.set_can_target(star.is_active());
    header.append(&star);

    // The ⋮ menu, last in the row: after the time and the star. The list is
    // shared, not copied; the song is taken from it when the menu is used.
    let songs = queue.clone();
    let client = client.cloned();
    header.append(&crate::song_menu::menu_button(
        &row,
        move || songs.get(index).cloned().map(|song| (song, client.clone())),
        false,
    ));

    row.set_child(Some(&header));
    // Read as an Adw.ActionRow is: the title, the line below it as the
    // description.
    let accessible = row.upcast_ref::<gtk::Widget>();
    accessible.update_relation(&[gtk::accessible::Relation::LabelledBy(&[title.upcast_ref()])]);
    if !described_by.is_empty() {
        let described_by: Vec<&gtk::Accessible> =
            described_by.iter().map(|w| w.upcast_ref()).collect();
        accessible.update_relation(&[gtk::accessible::Relation::DescribedBy(&described_by)]);
    }

    TrackRow {
        song_id: song.id.clone(),
        row,
        slot,
        number,
        track,
        bars: None,
        star,
    }
}

/// True when `artist` (one name or several) is the album's artist or among
/// its artists: the row then names no artist under its title.
fn of_the_album(artist: &str, album_artist: Option<&str>) -> bool {
    let album = names_of(album_artist.unwrap_or(""));
    names_of(artist).iter().all(|name| album.contains(name))
}

/// The parts of the album's info line: the year, the genre, the record
/// label, the discs (when more than one), the tracks and the length.
fn info_parts(
    year: Option<i32>,
    genre: Option<&str>,
    label: Option<&str>,
    discs: u32,
    tracks: u32,
    seconds: u32,
) -> Vec<String> {
    let mut parts = Vec::new();
    if let Some(year) = year {
        parts.push(year.to_string());
    }
    for text in [genre, label].into_iter().flatten() {
        if !text.trim().is_empty() {
            parts.push(text.trim().to_owned());
        }
    }
    if discs > 1 {
        // Translators: {n} is a number of discs.
        parts.push(ngettext("{n} disc", "{n} discs", discs).replace("{n}", &discs.to_string()));
    }
    parts.push(format::tracks_text(tracks));
    // Songs without a length (from some servers) add up to none.
    if seconds > 0 {
        parts.push(format::long_duration(seconds));
    }
    parts
}

/// The album's info line, e.g. "1995 · Classique · Decca · 10 tracks · 58
/// minutes": the parts are joined with no-break spaces inside them and
/// before each "·", so a narrow window breaks the line only after a "·".
fn info_line(parts: &[String]) -> String {
    parts
        .iter()
        .map(|part| part.replace(' ', "\u{a0}"))
        .collect::<Vec<_>>()
        .join("\u{a0}· ")
}

/// E.g. "FLAC · 24 bit / 96 kHz" or "MP3 · 320 kbps", plus whether it is hi-res.
/// None if the server did not say anything useful.
pub(crate) fn quality_text(songs: &[subsonic::Song]) -> Option<(String, bool)> {
    let first = songs.first()?;
    let format = first.suffix.as_deref()?.to_uppercase();

    // Lossy files report a bit depth of 0; treat that as "not applicable".
    let depth = songs
        .iter()
        .filter_map(|s| s.bit_depth)
        .filter(|&d| d > 0)
        .max();
    let rate = songs
        .iter()
        .filter_map(|s| s.sampling_rate)
        .filter(|&r| r > 0)
        .max();

    let quality = match (depth, rate) {
        (Some(depth), Some(rate)) => {
            format!("{} / {}", format::bits_text(depth), format::khz_text(rate))
        }
        _ => format::kbps_text(songs.iter().filter_map(|s| s.bit_rate).max()?),
    };

    let uniform = songs.iter().all(|s| {
        s.suffix == first.suffix
            && s.bit_depth == first.bit_depth
            && s.sampling_rate == first.sampling_rate
    });
    let text = if uniform {
        format!("{format} · {quality}")
    } else {
        // Translators: tracks of one album differ in quality, e.g. "FLAC · up to 24 bit / 96 kHz".
        gettext("{format} · up to {quality}")
            .replace("{format}", &format)
            .replace("{quality}", &quality)
    };

    let hi_res = depth.unwrap_or(0) > 16 || rate.unwrap_or(0) > 48_000;
    Some((text, hi_res))
}

impl AlbumPage {
    /// Fetched next to the track list; the section stays hidden if there is no text.
    fn load_about(&self, client: subsonic::Client, id: String) {
        let page = self.downgrade();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn(async move { client.album_info(&id).await })
                .await;
            let (Some(page), Ok(Ok(info))) = (page.upgrade(), result) else {
                return;
            };
            let text = info
                .notes
                .as_deref()
                .map(about::plain_text)
                .unwrap_or_default();
            if !text.is_empty() {
                page.show_about(&text);
            }
        });
    }

    fn show_about(&self, text: &str) {
        let imp = self.imp();
        about::fill(&imp.about, &imp.about_text, &imp.about_more, text);
    }
}

impl AlbumPage {
    pub(crate) fn album_id(&self) -> String {
        self.imp().album_id.borrow().clone()
    }
}

impl AlbumPage {
    /// Shows the bouncing bars and the highlight on the track that is playing
    /// (resting bars while paused), and the number on all the others.
    fn mark_playing(&self) {
        let player = player::get();
        let current = player
            .current()
            .filter(|_| player.is_active())
            .map(|song| song.id);
        let mode = if player.is_playing() {
            Mode::Moving
        } else {
            Mode::Paused
        };
        for track in self.imp().rows.borrow_mut().iter_mut() {
            let playing = current.as_deref() == Some(track.song_id.as_str());
            track.set_playing(playing, mode);
        }

        // The Play pill follows too: Pause while this album plays.
        let imp = self.imp();
        if let Some((icon, names)) = imp.play.borrow().as_ref() {
            let album_id = imp.album_id.borrow();
            let album_playing = player
                .current()
                .is_some_and(|s| s.album_id.as_deref() == Some(album_id.as_str()));
            let (image, name) = if album_playing && player.is_playing() {
                ("media-playback-pause-symbolic", "pause")
            } else if album_playing && player.is_paused() {
                ("media-playback-start-symbolic", "resume")
            } else {
                ("media-playback-start-symbolic", "play")
            };
            icon.set_icon_name(Some(image));
            names.set_visible_child_name(name);
        }
    }
}

impl AlbumPage {
    /// Runs `f` once the page has fully slid into view, or right away if it
    /// already has.
    fn after_shown(&self, f: impl FnOnce(&AlbumPage) + 'static) {
        let imp = self.imp();
        if imp.shown.get() {
            f(self);
        } else {
            imp.after_shown.borrow_mut().push(Box::new(f));
        }
    }
}

/// How many track rows are built before the page slides in (about one screenful).
const ROW_BATCH: usize = 15;

/// How many of the other rows are built at a time, after the slide-in: each
/// batch and its layout fit well inside one frame at 120 Hz.
const ROW_BATCH_LATER: usize = 5;

/// Where building the track rows has got to (see show_tracks).
struct RowBuilder {
    queue: Rc<[subsonic::Song]>,
    album_artist: Option<String>,
    client: Option<subsonic::Client>,
    /// Disc number and title, for albums whose discs have names.
    disc_titles: Vec<(u32, String)>,
    multi_disc: bool,
    /// The composer named in the header (as a key, see composer_key), or the
    /// album artist's: rows leave it out and name only a different one.
    hidden_composer: Option<String>,
    /// Which call of show_tracks this belongs to; batches of an older one stop.
    generation: u32,
    /// The next track to build a row for.
    next: Cell<usize>,
    /// The disc group rows are being added to.
    group: RefCell<Option<(u32, adw::PreferencesGroup)>>,
}

impl AlbumPage {
    /// Adds up to `count` more track rows; returns true while rows remain.
    /// The server lists tracks disc by disc, so a new group starts when the
    /// disc changes, whichever batch that happens in.
    fn build_rows(&self, builder: &RowBuilder, count: usize) -> bool {
        let imp = self.imp();
        if builder.generation != imp.row_generation.get() {
            return false;
        }
        let start = builder.next.get();
        let end = (start + count).min(builder.queue.len());

        for index in start..end {
            let song = &builder.queue[index];
            let disc = song.disc_number.unwrap_or(1);
            let same_disc = builder.group.borrow().as_ref().map(|(d, _)| *d) == Some(disc);
            if !same_disc {
                // The tracks of a disc on a card (the group's boxed list).
                let group = adw::PreferencesGroup::new();
                if builder.multi_disc {
                    // Translators: header above the tracks of one disc, e.g. "Disc 2".
                    group.set_title(&gettext("Disc {n}").replace("{n}", &disc.to_string()));
                    if let Some((_, title)) = builder.disc_titles.iter().find(|(d, _)| *d == disc) {
                        group.set_description(Some(&glib::markup_escape_text(title)));
                    }
                }
                imp.tracks.append(&group);
                builder.group.replace(Some((disc, group)));
            }
            if let Some((_, group)) = builder.group.borrow().as_ref() {
                let track = track_row(
                    song,
                    builder.album_artist.as_deref(),
                    builder.hidden_composer.as_deref(),
                    builder.client.as_ref(),
                    index,
                    &builder.queue,
                );
                group.add(&track.row);
                self.star_follows_pointer(&track);
                imp.rows.borrow_mut().push(track);
            }
        }

        builder.next.set(end);
        end < builder.queue.len()
    }
}

impl AlbumPage {
    /// Fades `widget` in over a quarter of a second. A libadwaita animation,
    /// like the page slide itself, so it follows "Reduce animations" in GNOME.
    fn fade_in(&self, widget: &impl IsA<gtk::Widget>) {
        // As a plain gtk::Widget, which the compiler knows to be a GObject.
        let widget = widget.upcast_ref::<gtk::Widget>();
        widget.set_opacity(0.0);
        let target = adw::PropertyAnimationTarget::new(widget, "opacity");
        let animation = adw::TimedAnimation::builder()
            .widget(widget)
            .target(&target)
            .value_from(0.0)
            .value_to(1.0)
            .duration(250)
            .easing(adw::Easing::EaseOutCubic)
            .build();
        animation.play();
        self.imp().animations.borrow_mut().push(animation);
    }
}

impl AlbumPage {
    /// Puts a layer under the whole page, header bar included, for the
    /// cover-coloured gradient; the toolbar view on top of it is transparent.
    fn add_backdrop(&self) {
        let imp = self.imp();
        if let Some(backdrop) = crate::tint_layer::TintLayer::under(&imp.toolbar.get()) {
            imp.backdrop.replace(Some(backdrop));
        }
    }

    /// Applies the cover colour and fades it in.
    fn show_tint(&self, color: (u8, u8, u8)) {
        let backdrop = self.imp().backdrop.borrow().clone();
        if let Some(backdrop) = backdrop {
            backdrop.set_page(Some(color));
            self.fade_in(&backdrop);
        }
    }
}

impl AlbumPage {
    /// "album.play-track" (the track's index): a click on a track row. The
    /// rows only name their index, so a row needs no handler of its own.
    fn install_track_action(&self) {
        let actions = gtk::gio::SimpleActionGroup::new();
        let play = gtk::gio::SimpleAction::new("play-track", Some(glib::VariantTy::UINT32));
        let page = self.downgrade();
        play.connect_activate(move |_, index| {
            let (Some(page), Some(index)) = (page.upgrade(), index.and_then(|i| i.get::<u32>()))
            else {
                return;
            };
            page.play_track(index as usize);
        });
        actions.add_action(&play);
        self.insert_action_group("album", Some(&actions));
    }

    /// The star of the track row under the mouse pointer takes clicks; the
    /// hidden stars of the other rows do not. A touch has no pointer that
    /// shows the star first, so there a tap on a hidden star plays the track
    /// (the ⋮ menu stars it).
    fn follow_pointer(&self) {
        let motion = gtk::EventControllerMotion::new();
        let page = self.downgrade();
        motion.connect_motion(move |motion, x, y| {
            let Some(page) = page.upgrade() else {
                return;
            };
            let touch = motion
                .current_event_device()
                .is_some_and(|device| device.source() == gtk::gdk::InputSource::Touchscreen);
            let row = if touch {
                None
            } else {
                page.imp()
                    .tracks
                    .pick(x, y, gtk::PickFlags::DEFAULT)
                    .and_then(|widget| widget.ancestor(adw::PreferencesRow::static_type()))
            };
            page.set_hovered(row);
        });
        let page = self.downgrade();
        motion.connect_leave(move |_| {
            if let Some(page) = page.upgrade() {
                page.set_hovered(None);
            }
        });
        self.imp().tracks.add_controller(motion);
    }

    /// `row` is now the track row under the pointer (None: no row).
    fn set_hovered(&self, row: Option<gtk::Widget>) {
        let imp = self.imp();
        let old = imp.hovered.replace(row.clone());
        if old == row {
            return;
        }
        for track in imp.rows.borrow().iter() {
            let widget = track.row.upcast_ref::<gtk::Widget>();
            let now = row.as_ref() == Some(widget);
            if now || old.as_ref() == Some(widget) {
                track.star.set_can_target(now || track.star.is_active());
            }
        }
    }

    /// A star changed elsewhere (another page, the menu, the server) keeps
    /// taking clicks only where it shows.
    fn star_follows_pointer(&self, track: &TrackRow) {
        let page = self.downgrade();
        let row = track.row.clone().upcast::<gtk::Widget>().downgrade();
        track.star.connect_toggled(move |star| {
            let hovered = page
                .upgrade()
                .is_some_and(|page| page.imp().hovered.borrow().as_ref() == row.upgrade().as_ref());
            star.set_can_target(star.is_active() || hovered);
        });
    }

    /// Plays the album from the track at `index`; the playing track pauses or
    /// resumes instead of starting again.
    fn play_track(&self, index: usize) {
        let imp = self.imp();
        let Some(queue) = imp.queue.borrow().clone() else {
            return;
        };
        let Some(song) = queue.get(index) else {
            return;
        };
        let player = player::get();
        if player.toggle_if_current(player::Clicked::Song(&song.id)) {
            return;
        }
        let client = imp.client.borrow().clone();
        match client {
            Some(client) => player.play(
                client,
                queue.clone(),
                index,
                player::QueueOf::Album(self.album_id()),
            ),
            // Shown offline from the local copy: playing needs the server.
            None => crate::cannot_play(self),
        }
    }

    /// The round Play and Shuffle buttons (once per page). They read the queue
    /// and the client when clicked, so after a refresh they play the current
    /// track list, and once the connection is back they play at all.
    fn build_actions(&self, album_id: &str) {
        let imp = self.imp();
        // Radio: a random song of this album, then songs that sound like it.
        let radio_button = gtk::Button::builder()
            .icon_name("vumi-radio-symbolic")
            .tooltip_text(gettext("Radio"))
            .valign(gtk::Align::Center)
            .css_classes(["circular", "flat", "album-action"])
            .build();
        {
            let page = self.downgrade();
            radio_button.connect_clicked(move |button| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let client = page.imp().client.borrow().clone();
                match client {
                    Some(client) => page.start_radio(client, button),
                    None => crate::cannot_play(button),
                }
            });
        }
        // Play as a pill with its name; the names share one width, so the
        // pill keeps its size when it becomes Pause or Resume.
        let play_icon = gtk::Image::from_icon_name("media-playback-start-symbolic");
        let play_names = gtk::Stack::new();
        for (name, text) in [
            ("play", gettext("Play")),
            ("pause", gettext("Pause")),
            ("resume", gettext("Resume")),
        ] {
            play_names.add_named(&gtk::Label::new(Some(&text)), Some(name));
        }
        // Spaced as Adw.ButtonContent spaces the Play pills of playlists and
        // Favorites.
        let play_content = gtk::Box::builder()
            .spacing(6)
            .halign(gtk::Align::Center)
            .build();
        play_content.append(&play_icon);
        play_content.append(&play_names);
        let play_button = gtk::Button::builder()
            .child(&play_content)
            .valign(gtk::Align::Center)
            .css_classes(["pill", "suggested-action", "album-pill"])
            .build();
        imp.play.replace(Some((play_icon, play_names)));
        let shuffle_button = gtk::Button::builder()
            .icon_name("media-playlist-shuffle-symbolic")
            .tooltip_text(gettext("Shuffle"))
            .valign(gtk::Align::Center)
            .css_classes(["circular", "flat", "album-action"])
            .build();
        {
            let album_id = album_id.to_owned();
            let page = self.downgrade();
            play_button.connect_clicked(move |button| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let player = player::get();
                // This album is already playing: the button pauses and resumes it.
                let album_playing = player
                    .current()
                    .is_some_and(|s| s.album_id.as_deref() == Some(album_id.as_str()));
                if album_playing && player.is_active() {
                    player.toggle_pause();
                    return;
                }
                let client = page.imp().client.borrow().clone();
                let Some(client) = client else {
                    crate::cannot_play(button);
                    return;
                };
                if let Some(queue) = page.imp().queue.borrow().clone() {
                    player.play(client, queue, 0, player::QueueOf::Album(album_id.clone()));
                }
            });
        }
        {
            let page = self.downgrade();
            let album_id = album_id.to_owned();
            shuffle_button.connect_clicked(move |button| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let Some(queue) = page.imp().queue.borrow().clone() else {
                    return;
                };
                let client = page.imp().client.borrow().clone();
                let Some(client) = client else {
                    crate::cannot_play(button);
                    return;
                };
                // Still the album, in another order: it goes into the history.
                player::play_list(
                    client,
                    queue,
                    true,
                    player::QueueOf::Album(album_id.clone()),
                );
            });
        }
        imp.actions.append(&play_button);
        imp.actions.append(&shuffle_button);
        imp.actions.append(&radio_button);
    }

    /// The artist name opens the artist page, once the artist id is known
    /// (and the server can be asked for it).
    fn connect_artist(&self, album: &subsonic::AlbumWithSongs) {
        let imp = self.imp();
        if imp.artist_linked.get() {
            return;
        }
        let Some(artist_id) = album.artist_id.clone() else {
            return;
        };
        imp.artist_linked.set(true);
        let name = album.artist.clone().unwrap_or_default();
        let page = self.downgrade();
        imp.artist_button.connect_clicked(move |button| {
            let Some(page) = page.upgrade() else {
                return;
            };
            // None offline: the artist page shows the albums in the library.
            let client = page.imp().client.borrow().clone();
            // Back to the artist's page if it is open already (album, artist,
            // album, artist... does not stack up pages).
            if let Some(main) = button
                .ancestor(crate::main_view::MainView::static_type())
                .and_downcast::<crate::main_view::MainView>()
            {
                main.open_artist_from(button, &artist_id, &name, client.clone());
            }
        });
    }

    /// The album as the local database knows it: the album row from the library
    /// and its songs. Disc titles come only from the server.
    fn local_album(&self, songs: Vec<subsonic::Song>) -> Option<subsonic::AlbumWithSongs> {
        let album = self.imp().album.borrow().clone()?;
        local_album_of(&album, songs)
    }
}

/// The album as the local database knows it (the album row and its songs), or
/// None when the database has none of its songs. Disc titles come only from
/// the server.
fn local_album_of(
    album: &subsonic::Album,
    songs: Vec<subsonic::Song>,
) -> Option<subsonic::AlbumWithSongs> {
    if songs.is_empty() {
        return None;
    }
    Some(subsonic::AlbumWithSongs {
        artist_id: album.artist_id.clone(),
        starred: album.starred.clone(),
        id: album.id.clone(),
        name: album.name.clone(),
        artist: album.artist.clone(),
        song: songs,
        disc_titles: Vec::new(),
    })
}

/// The names in an artist or composer string ("Lennon • McCartney",
/// "Lennon/McCartney"), trimmed and in lower case.
fn names_of(text: &str) -> Vec<String> {
    text.split(['•', '/', ';'])
        .map(|name| name.trim().to_lowercase())
        .filter(|name| !name.is_empty())
        .collect()
}

/// The names sorted and joined, so the same people in another order compare
/// equal.
fn composer_key(text: &str) -> String {
    let mut names = names_of(text);
    names.sort();
    names.join("|")
}

/// At most three names for a row: "A • B • …" when there are more. The full
/// list is in Song Information.
fn short_names(text: &str) -> String {
    let names: Vec<&str> = text
        .split(['•', '/', ';'])
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .collect();
    if names.len() <= 3 {
        text.to_string()
    } else {
        format!("{} • {} • …", names[0], names[1])
    }
}

/// The discs that have a name, with it. The server lists every disc of some
/// albums with an empty title (Rubber Soul (Super Deluxe)): those have none.
fn disc_titles(album: &subsonic::AlbumWithSongs) -> Vec<(u32, String)> {
    album
        .disc_titles
        .iter()
        .filter(|t| !t.title.trim().is_empty())
        .map(|t| (t.disc, t.title.clone()))
        .collect()
}

/// True if the server's answer shows nothing the local copy does not.
fn same_album(local: &subsonic::AlbumWithSongs, server: &subsonic::AlbumWithSongs) -> bool {
    local.name == server.name
        && local.artist == server.artist
        && local.starred == server.starred
        && local.artist_id == server.artist_id
        // The local copy has no disc titles.
        && disc_titles(server).is_empty()
        && local.song.len() == server.song.len()
        && local.song.iter().zip(&server.song).all(|(a, b)| same_song(a, b))
}

/// Every field of the two songs but the cover id: the ids too, which "Go to
/// Artist" uses. The server gives a song a new cover id now and then without
/// a new scan, and the light sync keeps the old one; the page never shows a
/// song's own cover, and the old id gives the same picture.
fn same_song(local: &subsonic::Song, server: &subsonic::Song) -> bool {
    let without_cover = |song: &subsonic::Song| subsonic::Song {
        cover_art: None,
        ..song.clone()
    };
    without_cover(local) == without_cover(server)
}

/// Opens an album page on `nav`. The album's songs are read from the local
/// database in the background first, so the page slides in already filled and
/// the window never waits for the disk.
/// `server` is the account's key ("user@url"), known offline too.
pub(crate) fn open(
    nav: &adw::NavigationView,
    album: &subsonic::Album,
    client: Option<subsonic::Client>,
    server: String,
) {
    let nav = nav.downgrade();
    let album = album.clone();
    glib::spawn_future_local(async move {
        let id = album.id.clone();
        let local = runtime()
            .spawn_blocking(move || crate::library::load_album_songs(&server, &id))
            .await
            .ok()
            .and_then(Result::ok)
            .unwrap_or_default();
        if let Some(nav) = nav.upgrade() {
            nav.push(&AlbumPage::new(&album, client, local));
        }
    });
}

impl AlbumPage {
    /// Radio: a random song of this album, then about 50 songs that sound like
    /// it (AudioMuse-AI through Navidrome). A short note in the window if the
    /// server finds none, e.g. when the album has not been analysed yet.
    fn start_radio(&self, client: subsonic::Client, button: &gtk::Button) {
        let Some(queue) = self.imp().queue.borrow().clone() else {
            return;
        };
        if queue.is_empty() {
            return;
        }
        let seed = queue[fastrand::usize(..queue.len())].clone();
        // One request at a time: the button waits for the answer.
        button.set_sensitive(false);
        let button = button.downgrade();
        crate::song_menu::play_radio(self, client, seed, move || {
            if let Some(button) = button.upgrade() {
                button.set_sensitive(true);
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song(id: &str) -> subsonic::Song {
        subsonic::Song {
            id: id.into(),
            title: format!("Song {id}"),
            artist: Some("Artist".into()),
            track: Some(1),
            disc_number: Some(1),
            album: Some("Album".into()),
            album_id: Some("al-1".into()),
            artist_id: Some("ar-1".into()),
            duration: Some(200),
            suffix: Some("flac".into()),
            bit_rate: Some(900),
            cover_art: Some("al-1".into()),
            bit_depth: Some(16),
            sampling_rate: Some(44_100),
            ..Default::default()
        }
    }

    fn album(songs: Vec<subsonic::Song>) -> subsonic::AlbumWithSongs {
        subsonic::AlbumWithSongs {
            artist_id: Some("ar-1".into()),
            starred: None,
            id: "al-1".into(),
            name: "Album".into(),
            artist: Some("Artist".into()),
            song: songs,
            disc_titles: Vec::new(),
        }
    }

    #[test]
    fn offline_an_album_shows_its_local_songs_and_only_without_them_the_message() {
        let row = subsonic::Album {
            id: "al-1".into(),
            name: "Album".into(),
            artist: Some("Artist".into()),
            year: Some(2001),
            song_count: Some(2),
            created: None,
            artist_id: Some("ar-1".into()),
            starred: None,
            cover_art: None,
            genre: None,
            record_label: None,
        };
        let local = local_album_of(&row, vec![song("1"), song("2")]).expect("tracks");
        assert_eq!(local.id, "al-1");
        assert_eq!(local.song.len(), 2);
        assert!(local.disc_titles.is_empty());
        assert!(local_album_of(&row, Vec::new()).is_none());
    }

    #[test]
    fn the_server_answer_is_new_when_any_field_of_a_song_differs() {
        let local = album(vec![song("1"), song("2")]);
        assert!(same_album(&local, &album(vec![song("1"), song("2")])));

        // "Go to Artist" and "Go to Album" read these ids from the song.
        let mut moved = song("2");
        moved.album_id = Some("al-2".into());
        assert!(!same_album(&local, &album(vec![song("1"), moved])));
        let mut other_artist = song("2");
        other_artist.artist_id = Some("ar-2".into());
        assert!(!same_album(&local, &album(vec![song("1"), other_artist])));
        let mut renamed = song("2");
        renamed.album = Some("Album (Remastered)".into());
        assert!(!same_album(&local, &album(vec![song("1"), renamed])));
    }

    #[test]
    fn a_track_names_its_artist_only_when_not_one_of_the_albums() {
        let album = Some("Adiemus • Karl Jenkins • London Philharmonic Orchestra");
        assert!(of_the_album("Adiemus", album));
        assert!(of_the_album("Karl Jenkins • Adiemus", album));
        assert!(of_the_album("", album));
        assert!(!of_the_album("Miriam Stockley", album));
        assert!(!of_the_album("Adiemus • Miriam Stockley", album));
        assert!(of_the_album("The Beatles", Some("The Beatles")));
        assert!(!of_the_album("The Beatles", None));
    }

    #[test]
    fn the_info_line_has_the_genre_and_the_label_when_the_library_has_them() {
        // No translations in tests: the English texts.
        let full = info_parts(Some(1965), Some("Rock"), Some("UMC"), 4, 68, 0);
        assert_eq!(full, ["1965", "Rock", "UMC", "4 discs", "68 tracks"]);
        let bare = info_parts(None, None, Some(" "), 1, 10, 0);
        assert_eq!(bare, ["10 tracks"]);
        assert_eq!(
            info_parts(Some(1995), Some("Classique"), None, 1, 10, 3480).len(),
            4
        );
    }

    #[test]
    fn the_info_line_breaks_only_after_a_dot() {
        let parts = [
            "1965".to_string(),
            "UMC (Universal Music Catalogue)".to_string(),
            "68 tracks".to_string(),
        ];
        assert_eq!(
            info_line(&parts),
            "1965\u{a0}· UMC\u{a0}(Universal\u{a0}Music\u{a0}Catalogue)\u{a0}· 68\u{a0}tracks"
        );
        // The only ordinary spaces follow the dots.
        assert_eq!(info_line(&parts).matches(' ').count(), 2);
        assert_eq!(info_line(&parts[..1]), "1965");
    }

    #[test]
    fn a_new_cover_id_of_a_song_is_no_change() {
        // The server gives a song a new cover id now and then without a new
        // scan; the light sync keeps the old one, which gives the same picture.
        let local = album(vec![song("1"), song("2")]);
        let mut new_id = song("2");
        new_id.cover_art = Some("al-1_5de644142867f459".into());
        assert!(same_album(&local, &album(vec![song("1"), new_id])));
        // A song more or less is a change.
        assert!(!same_album(&local, &album(vec![song("1")])));
        assert!(!same_album(
            &local,
            &album(vec![song("1"), song("2"), song("3")])
        ));
    }

    #[test]
    fn discs_with_empty_titles_have_no_titles() {
        // The server lists every disc of some albums with an empty title
        // (Rubber Soul (Super Deluxe): four); the local copy has no titles.
        let local = album(vec![song("1"), song("2")]);
        let mut server = album(vec![song("1"), song("2")]);
        server.disc_titles = (1..=4)
            .map(|disc| subsonic::DiscTitle {
                disc,
                title: String::new(),
            })
            .collect();
        assert!(same_album(&local, &server));
        assert!(disc_titles(&server).is_empty());

        // A disc with a name: the page shows it.
        server.disc_titles[1].title = "Bonus Tracks".into();
        assert!(!same_album(&local, &server));
        assert_eq!(disc_titles(&server), [(2, "Bonus Tracks".to_string())]);
    }
}

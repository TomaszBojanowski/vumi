//! Album page: big cover, album info and the track list, grouped by disc.
//! The layout lives in album_page.blp; this file only holds the behaviour.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::collections::BTreeSet;
use std::rc::Rc;

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
        /// The album's star button, replaced when the server knows better.
        pub album_star: RefCell<Option<gtk::Widget>>,
        /// True once the parts made once per page exist (Play row, listener).
        pub tracks_shown: Cell<bool>,
        /// True once the artist name opens the artist page.
        pub artist_linked: Cell<bool>,
        /// Bumped by every show_tracks; older row batches stop.
        pub row_generation: Cell<u32>,
        /// The round Play button, which becomes Pause / Resume for this album.
        pub play_button: RefCell<Option<gtk::Button>>,
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
            None => page.show_error(&gettext("Connect to the server to see the tracks.")),
        }
        page
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
            self.show_tracks(album, year, album_artist.as_deref(), client.clone());
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
                        page.show_tracks(&album, year, album_artist.as_deref(), client_for_page)
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
        client: subsonic::Client,
    ) {
        let imp = self.imp();
        let songs = &album.song;

        // The whole album as one shared play queue; the buttons read it when clicked.
        let queue: Rc<[subsonic::Song]> = songs.as_slice().into();
        imp.queue.replace(Some(queue.clone()));

        if !imp.tracks_shown.replace(true) {
            self.build_actions(&album.id, client.clone());
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
        self.connect_artist(album, &client);

        // The album's star is replaced: the server may know better than the local copy.
        if let Some(old) = imp.album_star.take() {
            imp.actions.remove(&old);
        }
        let star = star_button(
            album.starred.is_some(),
            Some(client.clone()),
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
            album_id: album.id.clone(),
            queue,
            album_artist: album_artist.map(str::to_owned),
            client: client.clone(),
            disc_titles: album
                .disc_titles
                .iter()
                .map(|t| (t.disc, t.title.clone()))
                .collect(),
            multi_disc,
            hidden_composer,
            generation,
            next: Cell::new(0),
            group: RefCell::new(None),
        });
        if self.build_rows(&builder, ROW_BATCH) {
            let page = self.downgrade();
            glib::idle_add_local(move || {
                // The page was closed meanwhile: stop building.
                let Some(page) = page.upgrade() else {
                    return glib::ControlFlow::Break;
                };
                let more = page.build_rows(&builder, ROW_BATCH);
                page.mark_playing();
                if more {
                    glib::ControlFlow::Continue
                } else {
                    glib::ControlFlow::Break
                }
            });
        }
        self.mark_playing();

        let count = songs.len() as u32;
        let seconds: u32 = songs.iter().filter_map(|s| s.duration).sum();

        let mut parts = Vec::new();
        if let Some(year) = year {
            parts.push(year.to_string());
        }
        if multi_disc {
            let n = discs.len() as u32;
            // Translators: {n} is a number of discs.
            parts.push(ngettext("{n} disc", "{n} discs", n).replace("{n}", &n.to_string()));
        }
        parts.push(format::tracks_text(count));
        // Songs without a length (from some servers) add up to none.
        if seconds > 0 {
            parts.push(format::long_duration(seconds));
        }
        imp.details.set_label(&parts.join(" · "));
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

/// A track row and the widgets that show whether it is playing.
pub(crate) struct TrackRow {
    song_id: String,
    row: adw::ActionRow,
    slot: gtk::Stack,
    bars: gtk::Box,
}

/// One track: number (or a playing icon), title, duration, favourite star;
/// the artist only if it differs from the album's, the composer only if it is
/// not the one named in the header and not one of the track's performers.
fn track_row(
    album_id: &str,
    song: &subsonic::Song,
    album_artist: Option<&str>,
    hidden_composer: Option<&str>,
    client: &subsonic::Client,
    index: usize,
    queue: &Rc<[subsonic::Song]>,
) -> TrackRow {
    // Markup for the composer in italics: the title and the artist are escaped.
    let row = adw::ActionRow::builder()
        .title(glib::markup_escape_text(&song.title))
        .use_markup(true)
        .subtitle_lines(1)
        .build();

    // A click on a row plays the album from this track.
    row.set_activatable(true);
    let client_for_play = client.clone();
    let play_queue = queue.clone();
    let song_id = song.id.clone();
    let album_id = album_id.to_owned();
    row.connect_activated(move |_| {
        let player = player::get();
        // The playing track: pause or resume it instead of starting it again.
        if !player.toggle_if_current(player::Clicked::Song(&song_id)) {
            player.play(
                client_for_play.clone(),
                play_queue.clone(),
                index,
                player::QueueOf::Album(album_id.clone()),
            );
        }
    });

    let mut subtitle: Vec<String> = Vec::new();
    if let Some(artist) = song.artist.as_deref().filter(|a| Some(*a) != album_artist) {
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
    if !subtitle.is_empty() {
        row.set_subtitle(&subtitle.join(" · "));
    }

    // The track number, swapped for an icon while this track plays. The stack
    // keeps one width for both, so the titles stay aligned.
    let number = gtk::Label::builder()
        .label(song.track.map(|n| n.to_string()).unwrap_or_default())
        .width_chars(3)
        .xalign(1.0)
        .css_classes(["dim-label", "numeric"])
        .build();
    // Three bars that bounce while the track plays and rest while it is paused.
    let bars = crate::song_row::eq_bars();
    let slot = gtk::Stack::new();
    slot.add_named(&number, Some("number"));
    slot.add_named(&bars, Some("icon"));
    slot.set_visible_child_name("number");
    row.add_prefix(&slot);

    if let Some(d) = song.duration {
        let duration = gtk::Label::builder()
            .label(format::clock(d.into()))
            .css_classes(["dim-label", "numeric"])
            .build();
        row.add_suffix(&duration);
    }

    row.add_suffix(&star_button(
        song.starred.is_some(),
        Some(client.clone()),
        Favorite::Song(song.id.clone()),
    ));

    // The ⋮ menu, last in the row: after the time and the star.
    crate::song_menu::attach(&row, queue, index, Some(client.clone()), false);

    TrackRow {
        song_id: song.id.clone(),
        row,
        slot,
        bars,
    }
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
        for track in self.imp().rows.borrow().iter() {
            let playing = current.as_deref() == Some(track.song_id.as_str());
            track
                .slot
                .set_visible_child_name(if playing { "icon" } else { "number" });
            if player.is_playing() {
                track.bars.remove_css_class("paused");
            } else {
                track.bars.add_css_class("paused");
            }
            if playing {
                track.row.add_css_class("playing");
            } else {
                track.row.remove_css_class("playing");
            }
        }

        // The round Play button follows too: Pause while this album plays.
        let imp = self.imp();
        if let Some(button) = imp.play_button.borrow().as_ref() {
            let album_id = imp.album_id.borrow();
            let album_playing = player
                .current()
                .is_some_and(|s| s.album_id.as_deref() == Some(album_id.as_str()));
            let (icon, tooltip) = if album_playing && player.is_playing() {
                ("media-playback-pause-symbolic", gettext("Pause"))
            } else if album_playing && player.is_paused() {
                ("media-playback-start-symbolic", gettext("Resume"))
            } else {
                ("media-playback-start-symbolic", gettext("Play"))
            };
            button.set_icon_name(icon);
            button.set_tooltip_text(Some(&tooltip));
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

/// How many track rows are built at a time (about one screenful).
const ROW_BATCH: usize = 15;

/// Where building the track rows has got to (see show_tracks).
struct RowBuilder {
    /// The album the queue is started as when a row is played.
    album_id: String,
    queue: Rc<[subsonic::Song]>,
    album_artist: Option<String>,
    client: subsonic::Client,
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
                let group = adw::PreferencesGroup::new();
                // Rows without the card frame (style.css): lighter under the
                // large header.
                group.add_css_class("flat-tracks");
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
                    &builder.album_id,
                    song,
                    builder.album_artist.as_deref(),
                    builder.hidden_composer.as_deref(),
                    &builder.client,
                    index,
                    &builder.queue,
                );
                group.add(&track.row);
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
    /// Puts an empty layer under the whole page, header bar included, for the
    /// cover-coloured gradient; the toolbar view on top of it is transparent.
    fn add_backdrop(&self) {
        let imp = self.imp();
        let toolbar = imp.toolbar.get();
        let Some(parent) = toolbar.parent() else {
            return;
        };
        let backdrop = crate::tint_layer::TintLayer::new();
        let overlay = gtk::Overlay::new();
        overlay.set_child(Some(&backdrop));

        // Move the toolbar view from its parent onto the new layer.
        let reparent = |set_child: &dyn Fn(Option<&gtk::Widget>)| {
            set_child(None);
            overlay.add_overlay(&toolbar);
            overlay.set_measure_overlay(&toolbar, true);
            set_child(Some(overlay.upcast_ref()));
        };
        if let Some(bin) = parent.downcast_ref::<adw::BreakpointBin>() {
            reparent(&|child| bin.set_child(child));
        } else if let Some(page) = parent.downcast_ref::<adw::NavigationPage>() {
            reparent(&|child| page.set_child(child));
        } else {
            // Unknown layout: keep the old way (gradient on the toolbar, no fade).
            return;
        }
        imp.backdrop.replace(Some(backdrop));
    }

    /// Applies the cover colour and fades it in.
    fn show_tint(&self, color: (u8, u8, u8)) {
        let imp = self.imp();
        let backdrop = imp.backdrop.borrow().clone();
        match backdrop {
            Some(backdrop) => {
                backdrop.set_page(Some(color));
                self.fade_in(&backdrop);
            }
            None => tint::apply(&imp.toolbar.get(), color),
        }
    }
}

impl AlbumPage {
    /// The round Play and Shuffle buttons (once per page). They read the queue
    /// when clicked, so after a refresh they play the current track list.
    fn build_actions(&self, album_id: &str, client: subsonic::Client) {
        let imp = self.imp();
        // Radio: a random song of this album, then songs that sound like it.
        let radio_button = gtk::Button::builder()
            .icon_name("vumi-radio-symbolic")
            .tooltip_text(gettext("Radio"))
            .valign(gtk::Align::Center)
            .css_classes(["circular", "flat", "album-action"])
            .build();
        {
            let client = client.clone();
            let page = self.downgrade();
            radio_button.connect_clicked(move |button| {
                if let Some(page) = page.upgrade() {
                    page.start_radio(client.clone(), button);
                }
            });
        }
        let play_button = gtk::Button::builder()
            .icon_name("media-playback-start-symbolic")
            .tooltip_text(gettext("Play"))
            .valign(gtk::Align::Center)
            .css_classes(["circular", "album-play"])
            .build();
        imp.play_button.replace(Some(play_button.clone()));
        let shuffle_button = gtk::Button::builder()
            .icon_name("media-playlist-shuffle-symbolic")
            .tooltip_text(gettext("Shuffle"))
            .valign(gtk::Align::Center)
            .css_classes(["circular", "flat", "album-action"])
            .build();
        {
            let client = client.clone();
            let album_id = album_id.to_owned();
            let page = self.downgrade();
            play_button.connect_clicked(move |_| {
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
                } else if let Some(queue) = page.imp().queue.borrow().clone() {
                    player.play(
                        client.clone(),
                        queue,
                        0,
                        player::QueueOf::Album(album_id.clone()),
                    );
                }
            });
        }
        {
            let page = self.downgrade();
            let album_id = album_id.to_owned();
            shuffle_button.connect_clicked(move |_| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let Some(queue) = page.imp().queue.borrow().clone() else {
                    return;
                };
                // Still the album, in another order: it goes into the history.
                player::play_list(
                    client.clone(),
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

    /// The artist name opens the artist page, once the artist id is known.
    fn connect_artist(&self, album: &subsonic::AlbumWithSongs, client: &subsonic::Client) {
        let imp = self.imp();
        if imp.artist_linked.get() {
            return;
        }
        let Some(artist_id) = album.artist_id.clone() else {
            return;
        };
        imp.artist_linked.set(true);
        let name = album.artist.clone().unwrap_or_default();
        let client = client.clone();
        imp.artist_button.connect_clicked(move |button| {
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
        Some(subsonic::AlbumWithSongs {
            artist_id: album.artist_id,
            starred: album.starred,
            id: album.id,
            name: album.name,
            artist: album.artist,
            song: songs,
            disc_titles: Vec::new(),
        })
    }
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

/// True if the server's answer shows nothing the local copy does not.
fn same_album(local: &subsonic::AlbumWithSongs, server: &subsonic::AlbumWithSongs) -> bool {
    local.name == server.name
        && local.artist == server.artist
        && local.starred == server.starred
        && local.artist_id == server.artist_id
        && server.disc_titles.is_empty()
        // Every field of every song: the ids too, which "Go to Artist" uses.
        && local.song == server.song
}

/// Opens an album page on `nav`. The album's songs are read from the local
/// database in the background first, so the page slides in already filled and
/// the window never waits for the disk.
pub(crate) fn open(
    nav: &adw::NavigationView,
    album: &subsonic::Album,
    client: Option<subsonic::Client>,
) {
    let nav = nav.downgrade();
    let album = album.clone();
    glib::spawn_future_local(async move {
        let local = match &client {
            Some(client) => {
                let server = client.server_key();
                let id = album.id.clone();
                runtime()
                    .spawn_blocking(move || crate::library::load_album_songs(&server, &id))
                    .await
                    .ok()
                    .and_then(Result::ok)
                    .unwrap_or_default()
            }
            None => Vec::new(),
        };
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
}

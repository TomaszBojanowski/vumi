//! Album page: big cover, album info and the track list, grouped by disc.
//! The layout lives in album_page.blp; this file only holds the behaviour.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::glib;
use std::collections::BTreeSet;
use std::rc::Rc;

use crate::artist_page::ArtistPage;
use crate::favorites::{star_button, Favorite};
use crate::{about, covers, error_message, player, runtime, tint};

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
        pub album_id: std::cell::RefCell<String>,
        /// Track rows, to mark the one that is playing.
        pub(crate) rows: std::cell::RefCell<Vec<super::TrackRow>>,
        /// The round Play button, which becomes Pause / Resume for this album.
        pub play_button: std::cell::RefCell<Option<gtk::Button>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for AlbumPage {
        // Must match "template $MusePlayerAlbumPage" in the Blueprint file.
        const NAME: &'static str = "MusePlayerAlbumPage";
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
    pub(crate) fn new(album: &subsonic::Album, client: Option<subsonic::Client>) -> Self {
        let page: Self = glib::Object::new();
        let imp = page.imp();

        page.set_title(&album.name);
        imp.album_id.replace(album.id.clone());
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
                page.load_tracks(client, album.id.clone(), album.year, album.artist.clone());
            }
            None => page.show_error(&gettext("Connect to the server to see the tracks.")),
        }
        page
    }

    fn load_cover(&self, client: Option<subsonic::Client>, id: String) {
        let page = self.downgrade();
        glib::spawn_future_local(async move {
            let texture = covers::load(client, id).await;
            if let Some(page) = page.upgrade() {
                page.imp().cover.set_paintable(texture.as_ref());
                if let Some(color) = texture.as_ref().and_then(tint::dominant_color) {
                    tint::apply(&page.imp().toolbar.get(), color);
                }
            }
        });
    }

    fn load_tracks(
        &self,
        client: subsonic::Client,
        id: String,
        year: Option<i32>,
        album_artist: Option<String>,
    ) {
        let page = self.downgrade();
        let client_for_page = client.clone();
        glib::spawn_future_local(async move {
            let result = runtime().spawn(async move { client.album(&id).await }).await;
            let Some(page) = page.upgrade() else {
                return;
            };
            match result {
                Ok(Ok(album)) => {
                    page.show_tracks(&album, year, album_artist.as_deref(), client_for_page)
                }
                Ok(Err(e)) => page.show_error(&error_message(&e)),
                Err(_) => page.show_error(&gettext("Something went wrong. Please try again.")),
            }
        });
    }

    fn show_tracks(
        &self,
        album: &subsonic::AlbumWithSongs,
        year: Option<i32>,
        album_artist: Option<&str>,
        client: subsonic::Client,
    ) {
        let imp = self.imp();
        let songs = &album.song;

        // The whole album as one shared play queue: buttons and rows point at it.
        let queue: Rc<[subsonic::Song]> = songs.as_slice().into();

        // Play the whole album, in order or shuffled: round icon buttons, so their
        // size never changes, whatever the state or the language.
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
            let queue = queue.clone();
            let album_id = album.id.clone();
            play_button.connect_clicked(move |_| {
                let player = player::get();
                // This album is already playing: the button pauses and resumes it.
                let album_playing = player
                    .current()
                    .is_some_and(|s| s.album_id.as_deref() == Some(album_id.as_str()));
                if album_playing {
                    player.toggle_pause();
                } else {
                    player.play(client.clone(), queue.clone(), 0);
                }
            });
        }
        {
            let client = client.clone();
            let queue = queue.clone();
            shuffle_button.connect_clicked(move |_| {
                // A new order needs its own copy, made only when Shuffle is pressed.
                let mut shuffled = queue.to_vec();
                fastrand::shuffle(&mut shuffled);
                player::get().play(client.clone(), shuffled.into(), 0);
            });
        }
        imp.actions.append(&play_button);
        imp.actions.append(&shuffle_button);

        imp.actions.append(&star_button(
            album.starred.is_some(),
            Some(client.clone()),
            Favorite::Album(album.id.clone()),
        ));

        // The artist name opens the artist page.
        if let Some(artist_id) = album.artist_id.clone() {
            let name = album.artist.clone().unwrap_or_default();
            let client = client.clone();
            let page = self.downgrade();
            imp.artist_button.connect_clicked(move |_| {
                let Some(page) = page.upgrade() else {
                    return;
                };
                let artist_page = ArtistPage::new(&artist_id, &name, client.clone());
                if let Some(nav) = page
                    .ancestor(adw::NavigationView::static_type())
                    .and_downcast::<adw::NavigationView>()
                {
                    nav.push(&artist_page);
                }
            });
        }

        // Headers only when the album really has more than one disc.
        let discs: BTreeSet<u32> = songs.iter().filter_map(|s| s.disc_number).collect();
        let multi_disc = discs.len() > 1;

        // The server lists tracks disc by disc, so a new group starts when the disc changes.
        let mut current: Option<(u32, adw::PreferencesGroup)> = None;
        let mut rows = Vec::new();
        for (index, song) in songs.iter().enumerate() {
            let disc = song.disc_number.unwrap_or(1);
            if current.as_ref().map(|(d, _)| *d) != Some(disc) {
                let group = adw::PreferencesGroup::new();
                if multi_disc {
                    // Translators: header above the tracks of one disc, e.g. "Disc 2".
                    group.set_title(&gettext("Disc {n}").replace("{n}", &disc.to_string()));
                    if let Some(t) = album.disc_titles.iter().find(|t| t.disc == disc) {
                        group.set_description(Some(&glib::markup_escape_text(&t.title)));
                    }
                }
                imp.tracks.append(&group);
                current = Some((disc, group));
            }
            if let Some((_, group)) = &current {
                let track = track_row(song, album_artist, &client, index, &queue);
                group.add(&track.row);
                rows.push(track);
            }
        }

        imp.rows.replace(rows);

        // Mark the playing track now and whenever the player changes; the page
        // stops listening when it is closed.
        self.mark_playing();
        let page = self.downgrade();
        let listener = player::get().connect_changed(move || {
            if let Some(page) = page.upgrade() {
                page.mark_playing();
            }
        });
        self.connect_destroy(move |_| player::get().disconnect(listener));

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
        // Translators: {n} is a number of tracks.
        parts.push(ngettext("{n} track", "{n} tracks", count).replace("{n}", &count.to_string()));
        parts.push(format_duration(seconds));
        imp.details.set_label(&parts.join(" · "));
        self.show_quality(songs);
        imp.tracks_stack.set_visible_child_name("tracks");
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
        imp.error_status.set_description(Some(message));
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
/// the artist only if it differs from the album's.
fn track_row(
    song: &subsonic::Song,
    album_artist: Option<&str>,
    client: &subsonic::Client,
    index: usize,
    queue: &Rc<[subsonic::Song]>,
) -> TrackRow {
    let row = adw::ActionRow::builder()
        .title(song.title.as_str())
        .use_markup(false)
        .build();

    // A click on a row plays the album from this track.
    row.set_activatable(true);
    let client_for_play = client.clone();
    let queue = queue.clone();
    let song_id = song.id.clone();
    row.connect_activated(move |_| {
        let player = player::get();
        // The playing track: pause or resume it instead of starting it again.
        if player.current().is_some_and(|s| s.id == song_id) {
            player.toggle_pause();
        } else {
            player.play(client_for_play.clone(), queue.clone(), index);
        }
    });

    if let Some(artist) = song.artist.as_deref() {
        if Some(artist) != album_artist {
            row.set_subtitle(artist);
        }
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
    let bars = gtk::Box::builder()
        .spacing(2)
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .css_classes(["eq"])
        .build();
    for _ in 0..3 {
        bars.append(&gtk::Box::builder().css_classes(["eq-bar"]).build());
    }
    let slot = gtk::Stack::new();
    slot.add_named(&number, Some("number"));
    slot.add_named(&bars, Some("icon"));
    slot.set_visible_child_name("number");
    row.add_prefix(&slot);

    if let Some(d) = song.duration {
        let duration = gtk::Label::builder()
            .label(format!("{}:{:02}", d / 60, d % 60))
            .css_classes(["dim-label", "numeric"])
            .build();
        row.add_suffix(&duration);
    }

    row.add_suffix(&star_button(
        song.starred.is_some(),
        Some(client.clone()),
        Favorite::Song(song.id.clone()),
    ));

    TrackRow {
        song_id: song.id.clone(),
        row,
        slot,
        bars,
    }
}

/// "47 minutes" for most albums, "6 h 9 min" for long ones.
fn format_duration(seconds: u32) -> String {
    let minutes = (seconds + 30) / 60;
    if minutes < 60 {
        // Translators: {n} is a number of minutes.
        ngettext("{n} minute", "{n} minutes", minutes).replace("{n}", &minutes.to_string())
    } else {
        // Translators: total length of a long album, e.g. "6 h 9 min".
        gettext("{hours} h {minutes} min")
            .replace("{hours}", &(minutes / 60).to_string())
            .replace("{minutes}", &(minutes % 60).to_string())
    }
}

/// E.g. "FLAC · 24 bit / 96 kHz" or "MP3 · 320 kbps", plus whether it is hi-res.
/// None if the server did not say anything useful.
pub(crate) fn quality_text(songs: &[subsonic::Song]) -> Option<(String, bool)> {
    let first = songs.first()?;
    let format = first.suffix.as_deref()?.to_uppercase();

    // Lossy files report a bit depth of 0; treat that as "not applicable".
    let depth = songs.iter().filter_map(|s| s.bit_depth).filter(|&d| d > 0).max();
    let rate = songs.iter().filter_map(|s| s.sampling_rate).filter(|&r| r > 0).max();

    let quality = match (depth, rate) {
        (Some(depth), Some(rate)) => format!("{} / {}", bits_text(depth), khz_text(rate)),
        _ => {
            let kbps = songs.iter().filter_map(|s| s.bit_rate).max()?;
            format!("{kbps} kbps")
        }
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

fn bits_text(depth: u32) -> String {
    // Translators: audio bit depth, e.g. "24 bit".
    gettext("{n} bit").replace("{n}", &depth.to_string())
}

fn khz_text(rate: u32) -> String {
    let whole = rate / 1000;
    let fraction = (rate % 1000) / 100;
    if fraction == 0 {
        // Translators: sample rate, e.g. "96 kHz".
        gettext("{khz} kHz").replace("{khz}", &whole.to_string())
    } else {
        // Translators: sample rate with one decimal, e.g. "44.1 kHz"; use your decimal separator.
        gettext("{whole}.{fraction} kHz")
            .replace("{whole}", &whole.to_string())
            .replace("{fraction}", &fraction.to_string())
    }
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
            let text = info.notes.as_deref().map(about::plain_text).unwrap_or_default();
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
        let current = player.current().map(|song| song.id);
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
            } else if album_playing {
                ("media-playback-start-symbolic", gettext("Resume"))
            } else {
                ("media-playback-start-symbolic", gettext("Play"))
            };
            button.set_icon_name(icon);
            button.set_tooltip_text(Some(&tooltip));
        }
    }
}

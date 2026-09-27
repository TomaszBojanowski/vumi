//! Album page: big cover, album info and the track list.
//! The layout lives in album_page.blp; this file only holds the behaviour.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::glib;

use crate::{covers, error_message, runtime};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/album_page.blp")]
    pub struct AlbumPage {
        #[template_child]
        pub cover: gtk::TemplateChild<gtk::Picture>,
        #[template_child]
        pub title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub artist: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub details: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub quality: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub tracks_stack: gtk::TemplateChild<gtk::Stack>,
        #[template_child]
        pub tracks: gtk::TemplateChild<gtk::ListBox>,
        #[template_child]
        pub error_status: gtk::TemplateChild<adw::StatusPage>,
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
        imp.title.set_label(&album.name);
        imp.artist.set_label(album.artist.as_deref().unwrap_or(""));
        imp.details
            .set_label(&album.year.map(|y| y.to_string()).unwrap_or_default());

        if let Some(id) = album.cover_art.clone() {
            page.load_cover(client.clone(), id);
        }

        match client {
            Some(client) => {
                page.load_tracks(client, album.id.clone(), album.year, album.artist.clone())
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
        glib::spawn_future_local(async move {
            let result = runtime().spawn(async move { client.album(&id).await }).await;
            let Some(page) = page.upgrade() else {
                return;
            };
            match result {
                Ok(Ok(album)) => page.show_tracks(&album.song, year, album_artist.as_deref()),
                Ok(Err(e)) => page.show_error(&error_message(&e)),
                Err(_) => page.show_error(&gettext("Something went wrong. Please try again.")),
            }
        });
    }

    fn show_tracks(&self, songs: &[subsonic::Song], year: Option<i32>, album_artist: Option<&str>) {
        let imp = self.imp();
        for song in songs {
            imp.tracks.append(&track_row(song, album_artist));
        }

        let count = songs.len() as u32;
        let seconds: u32 = songs.iter().filter_map(|s| s.duration).sum();

        let mut parts = Vec::new();
        if let Some(year) = year {
            parts.push(year.to_string());
        }
        // Translators: {n} is a number of tracks.
        parts.push(ngettext("{n} track", "{n} tracks", count).replace("{n}", &count.to_string()));
        parts.push(format_duration(seconds));
        imp.details.set_label(&parts.join(" · "));
        self.show_quality(songs);
        imp.tracks_stack.set_visible_child_name("tracks");
    }

    fn show_error(&self, message: &str) {
        let imp = self.imp();
        imp.error_status.set_description(Some(message));
        imp.tracks_stack.set_visible_child_name("error");
    }
}

/// One track: number, title, duration; the artist only if it differs from the album's.
fn track_row(song: &subsonic::Song, album_artist: Option<&str>) -> adw::ActionRow {
    let row = adw::ActionRow::builder()
        .title(song.title.as_str())
        .use_markup(false)
        .build();

    if let Some(artist) = song.artist.as_deref() {
        if Some(artist) != album_artist {
            row.set_subtitle(artist);
        }
    }

    let number = gtk::Label::builder()
        .label(song.track.map(|n| n.to_string()).unwrap_or_default())
        .width_chars(3)
        .xalign(1.0)
        .css_classes(["dim-label", "numeric"])
        .build();
    row.add_prefix(&number);

    if let Some(d) = song.duration {
        let duration = gtk::Label::builder()
            .label(format!("{}:{:02}", d / 60, d % 60))
            .css_classes(["dim-label", "numeric"])
            .build();
        row.add_suffix(&duration);
    }
    row
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

impl AlbumPage {
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
}

/// E.g. "FLAC · 24 bit / 96 kHz" or "MP3 · 320 kbps", plus whether it is hi-res.
/// None if the server did not say anything useful.
fn quality_text(songs: &[subsonic::Song]) -> Option<(String, bool)> {
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

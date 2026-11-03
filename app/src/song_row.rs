//! One song in a list (search results, Favorites, playlists, the queue): bars
//! that show while it plays, the title, a line below it, a note where the
//! list has one, the length and whatever the list adds at the end (the ⋮ menu). It is the content of a list
//! view's row, not the row itself, and carries the look of the row
//! (style.css). A list view reuses its rows while scrolling, so set_song() is
//! called many times on the same row.

use adw::subclass::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{OnceCell, RefCell};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/song_row.blp")]
    pub struct SongRow {
        #[template_child]
        pub prefixes: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub subtitle: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub suffixes: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub note: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub duration: gtk::TemplateChild<gtk::Label>,
        /// The bars before the title, see-through unless the song plays, so
        /// the titles stay aligned either way.
        pub bars: OnceCell<gtk::Box>,
        /// The song the row shows now.
        pub song: RefCell<Option<subsonic::Song>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SongRow {
        // Must match "template $VumiSongRow" in the Blueprint file.
        const NAME: &'static str = "VumiSongRow";
        type Type = super::SongRow;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for SongRow {
        fn constructed(&self) {
            self.parent_constructed();
            let bars = eq_bars();
            bars.set_opacity(0.0);
            self.prefixes.append(&bars);
            let _ = self.bars.set(bars);
        }
    }
    impl WidgetImpl for SongRow {}
    impl BoxImpl for SongRow {}
}

glib::wrapper! {
    pub struct SongRow(ObjectSubclass<imp::SongRow>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

/// What a screen reader says for the list item of `song`: "title, artist".
/// The row is only the item's content, and its menu button would be read
/// along with the text, so each list sets this on the item itself.
pub(crate) fn accessible_label(song: &subsonic::Song) -> String {
    match song.artist.as_deref().filter(|artist| !artist.is_empty()) {
        Some(artist) => format!("{}, {}", song.title, artist),
        None => song.title.clone(),
    }
}

impl SongRow {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    /// Shows `song`, with `subtitle` under its title (empty: no second line).
    pub(crate) fn set_song(&self, song: &subsonic::Song, subtitle: &str) {
        let imp = self.imp();
        imp.song.replace(Some(song.clone()));
        // Plain text: names like "Simon & Garfunkel" are not markup.
        imp.title.set_label(&song.title);
        imp.subtitle.set_label(subtitle);
        imp.subtitle.set_visible(!subtitle.is_empty());
        match song.duration {
            Some(d) => imp.duration.set_label(&crate::format::clock(d.into())),
            None => imp.duration.set_label(""),
        }
        imp.duration.set_visible(song.duration.is_some());
    }

    /// A few words before the length (empty: none). The row is reused, so
    /// the list sets this for every song it shows.
    pub(crate) fn set_note(&self, note: &str) {
        let label = &self.imp().note;
        label.set_label(note);
        label.set_visible(!note.is_empty());
    }

    /// How many lines the title may take before it is cut short with an
    /// ellipsis; 0 (the default) lets it wrap as far as it needs.
    pub(crate) fn set_title_lines(&self, lines: i32) {
        let title = &self.imp().title;
        title.set_lines(lines);
        title.set_ellipsize(if lines > 0 {
            gtk::pango::EllipsizeMode::End
        } else {
            gtk::pango::EllipsizeMode::None
        });
    }

    /// The song the row shows now, if any.
    pub(crate) fn song(&self) -> Option<subsonic::Song> {
        self.imp().song.borrow().clone()
    }

    /// The row is off screen: it shows no song, so nothing marks it as the
    /// one that is playing.
    pub(crate) fn clear(&self) {
        self.imp().song.replace(None);
        self.set_playing(false, false);
    }

    /// Puts `widget` at the end of the row, after the length.
    pub(crate) fn add_suffix(&self, widget: &impl IsA<gtk::Widget>) {
        self.imp().suffixes.append(widget);
    }

    /// Bars and the highlight while the song is the current one; the bars
    /// rest while the player is paused.
    pub(crate) fn set_playing(&self, playing: bool, paused: bool) {
        if let Some(bars) = self.imp().bars.get() {
            bars.set_opacity(if playing { 1.0 } else { 0.0 });
            if paused {
                bars.add_css_class("paused");
            } else {
                bars.remove_css_class("paused");
            }
        }
        if playing {
            self.add_css_class("playing");
        } else {
            self.remove_css_class("playing");
        }
    }
}

/// Three bars that bounce while a track plays (style.css: .eq); the class
/// "paused" makes them rest. Used by these rows (search results, Favorites,
/// playlists, the queue) and by the track rows of album pages.
pub(crate) fn eq_bars() -> gtk::Box {
    let bars = gtk::Box::builder()
        .spacing(2)
        .halign(gtk::Align::End)
        .valign(gtk::Align::Center)
        .css_classes(["eq"])
        .build();
    for _ in 0..3 {
        bars.append(&gtk::Box::builder().css_classes(["eq-bar"]).build());
    }
    bars
}

#[cfg(test)]
mod tests {
    /// The CSS rules of style.css as (selector, body), outside @keyframes.
    fn rules(css: &str) -> Vec<(String, String)> {
        css.split('}')
            .filter_map(|block| block.rsplit_once('{'))
            .map(|(selector, body)| {
                let selector = selector.rsplit("*/").next().unwrap_or(selector);
                (selector.trim().to_string(), body.trim().to_string())
            })
            .collect()
    }

    /// Each moving frame redraws the window: the bars move only in the row
    /// that plays, never hidden in the other rows, and stand still while the
    /// window is in the background.
    #[test]
    fn only_the_playing_row_moves_its_bars_and_not_in_the_background() {
        let css = include_str!("style.css");
        let all = rules(css);
        let moving: Vec<&String> = all
            .iter()
            .filter(|(_, body)| body.contains("animation: eq-bounce"))
            .map(|(selector, _)| selector)
            .collect();
        assert!(!moving.is_empty());
        for selector in moving {
            assert!(selector.contains(".playing"), "{selector}");
        }
        assert!(
            all.iter()
                .any(|(selector, body)| selector.contains(".eq-bar:backdrop")
                    && body.contains("animation: none")),
            "no rule stops the bars in the background"
        );
    }
}

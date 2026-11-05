//! One song in a list (search results, Favorites, playlists, the queue): bars
//! that show while it plays, the title, a line below it, a note where the
//! list has one, the length and whatever the list adds at the end (the ⋮ menu). It is the content of a list
//! view's row, not the row itself, and carries the look of the row
//! (style.css). A list view reuses its rows while scrolling, so set_song() is
//! called many times on the same row.
//!
//! A row holds only what it shows: the bars exist in the playing row alone,
//! the note only in lists that have notes (style.css keeps the room of the
//! bars before every title, so the titles stay aligned).

use adw::subclass::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;

use crate::eq_bars::{EqBars, Mode};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/song_row.blp")]
    pub struct SongRow {
        #[template_child]
        pub title_box: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub subtitle: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub duration: gtk::TemplateChild<gtk::Label>,
        /// The bars before the title, while the song is the current one.
        pub bars: RefCell<Option<EqBars>>,
        /// A few words before the length, made when a list first gives some.
        pub note: RefCell<Option<gtk::Label>>,
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

    impl ObjectImpl for SongRow {}
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
        let imp = self.imp();
        let mut slot = imp.note.borrow_mut();
        if slot.is_none() && note.is_empty() {
            return;
        }
        let label = slot.get_or_insert_with(|| {
            let label = gtk::Label::builder()
                .margin_end(12)
                .css_classes(["dim-label"])
                .build();
            label.insert_after(self, Some(&*imp.title_box));
            label
        });
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
        self.append(widget);
    }

    /// Bars and the highlight while the song is the current one; the bars
    /// rest while the player is paused. Only the current row has bars: they
    /// are made when it becomes the current one and go when it stops being it.
    pub(crate) fn set_playing(&self, playing: bool, paused: bool) {
        let mut bars = self.imp().bars.borrow_mut();
        if playing {
            let bars = bars.get_or_insert_with(|| {
                let bars = EqBars::new();
                self.prepend(&bars);
                bars
            });
            bars.set_mode(if paused { Mode::Paused } else { Mode::Moving });
        } else if let Some(bars) = bars.take() {
            self.remove(&bars);
        }
        if playing {
            self.add_css_class("playing");
        } else {
            self.remove_css_class("playing");
        }
    }
}

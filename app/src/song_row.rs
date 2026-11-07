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
use std::cell::{Cell, RefCell};

use crate::a11y::Now;
use crate::eq_bars::{EqBars, Mode};
use crate::favorites::Favorite;

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
        /// A handle at the start to drag the row by (playlists whose songs
        /// can be moved), made when first shown.
        pub handle: RefCell<Option<gtk::Image>>,
        /// Actions the list adds to the row's menu (a playlist's Move Up…).
        pub list_actions: RefCell<Option<gtk::gio::SimpleActionGroup>>,
        /// A few words before the length, made when a list first gives some.
        pub note: RefCell<Option<gtk::Label>>,
        /// The song the row shows now.
        pub song: RefCell<Option<subsonic::Song>>,
        /// The list item showing the row: a screen reader hears its name.
        pub(crate) item: RefCell<Option<glib::WeakRef<gtk::ListItem>>>,
        /// Whether the song is the player's current one.
        pub(crate) now: Cell<Now>,
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
            // A star changed elsewhere (a menu, a sync): the name follows.
            let row = self.obj().downgrade();
            crate::favorites::watch(move |key, _| {
                let Some(row) = row.upgrade() else {
                    return false;
                };
                if row
                    .song()
                    .is_some_and(|song| Favorite::Song(song.id).key() == key)
                {
                    row.speak();
                }
                true
            });
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
        self.speak();
    }

    /// The list item that shows the row: a screen reader hears its name,
    /// made from what the row shows (speak()). Lists set it once the row is
    /// in an item.
    pub(crate) fn set_list_item(&self, item: &gtk::ListItem) {
        self.imp().item.replace(Some(item.downgrade()));
        self.speak();
    }

    /// The full title on hover, for lists whose titles are cut to one line.
    /// Given when asked for: tooltip-text would be read out as the item's
    /// description, the title a second time.
    pub(crate) fn show_title_on_hover(&self) {
        self.set_has_tooltip(true);
        self.connect_query_tooltip(|row, _, _, _, tooltip| {
            let title = row.imp().title.label();
            tooltip.set_text(Some(&title));
            !title.is_empty()
        });
    }

    /// The name a screen reader hears for the row: all it shows, in order
    /// ("City Sleep, Boerd, 4:12"), then "playing" or "paused" for the
    /// current song and "favorite" for a starred one (no star on these rows).
    fn speak(&self) {
        let imp = self.imp();
        let Some(item) = imp.item.borrow().as_ref().and_then(|item| item.upgrade()) else {
            return;
        };
        // The item may show another row by now.
        if item.child().as_ref() != Some(self.upcast_ref()) {
            return;
        }
        let Some(song) = imp.song.borrow().clone() else {
            item.set_accessible_label("");
            return;
        };
        let title = imp.title.label();
        let subtitle = imp.subtitle.label();
        let note = imp
            .note
            .borrow()
            .as_ref()
            .filter(|note| note.is_visible())
            .map(|note| note.label());
        let duration = imp.duration.label();
        let shown = [
            Some(title.as_str()),
            imp.subtitle.is_visible().then_some(subtitle.as_str()),
            note.as_deref(),
            Some(duration.as_str()),
        ];
        let favorite =
            crate::favorites::is_starred(&Favorite::Song(song.id.clone()), song.starred.is_some());
        item.set_accessible_label(&crate::a11y::song(
            &crate::a11y::join(shown.into_iter().flatten()),
            imp.now.get(),
            favorite,
        ));
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
                // In the item's name (speak()).
                .accessible_role(gtk::AccessibleRole::Presentation)
                .build();
            label.insert_after(self, Some(&*imp.title_box));
            label
        });
        label.set_label(note);
        label.set_visible(!note.is_empty());
        drop(slot);
        self.speak();
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

    /// The actions the list gave the row for its menu, if it did.
    pub(crate) fn list_actions(&self) -> Option<gtk::gio::SimpleActionGroup> {
        self.imp().list_actions.borrow().clone()
    }

    pub(crate) fn set_list_actions(&self, actions: &gtk::gio::SimpleActionGroup) {
        self.imp().list_actions.replace(Some(actions.clone()));
    }

    /// A handle at the start of the row to drag it by: `shown`, and whether
    /// it can be used now (`sensitive`: not offline). Returns the handle when
    /// it is made, so the list gives it what dragging does.
    pub(crate) fn set_handle(&self, shown: bool, sensitive: bool) -> Option<gtk::Image> {
        let imp = self.imp();
        let mut slot = imp.handle.borrow_mut();
        if slot.is_none() && !shown {
            return None;
        }
        let mut made = None;
        let handle = slot.get_or_insert_with(|| {
            let handle = gtk::Image::builder()
                .icon_name("list-drag-handle-symbolic")
                .css_classes(["song-handle"])
                // Dragging is for the pointer; the ⋮ menu moves songs too
                // (Move Up, Move Down), for the keyboard and screen readers.
                .accessible_role(gtk::AccessibleRole::Presentation)
                .build();
            handle.set_cursor_from_name(Some("grab"));
            self.prepend(&handle);
            made = Some(handle.clone());
            handle
        });
        handle.set_visible(shown);
        handle.set_sensitive(sensitive);
        made
    }

    /// While the row is dragged: a small label with the title under the
    /// pointer, not a copy of the whole row (the queue and playlists). Set
    /// in the drag source's prepare: by drag-begin GTK has chosen its own.
    pub(crate) fn set_drag_icon(&self, source: &gtk::DragSource) {
        let Some(title) = self.song().map(|song| song.title) else {
            return;
        };
        let layout = self.create_pango_layout(Some(&title));
        layout.set_width(240 * gtk::pango::SCALE);
        layout.set_ellipsize(gtk::pango::EllipsizeMode::End);
        let (text_w, text_h) = layout.pixel_size();
        let (pad_x, pad_y) = (12.0, 6.0);
        let width = text_w as f32 + 2.0 * pad_x;
        let height = text_h as f32 + 2.0 * pad_y;

        let snapshot = gtk::Snapshot::new();
        let dark = adw::StyleManager::default().is_dark();
        let background = if dark {
            gtk::gdk::RGBA::new(0.2, 0.2, 0.2, 0.97)
        } else {
            gtk::gdk::RGBA::new(1.0, 1.0, 1.0, 0.97)
        };
        let rect = gtk::graphene::Rect::new(0.0, 0.0, width, height);
        snapshot.push_rounded_clip(&gtk::gsk::RoundedRect::from_rect(rect, 8.0));
        snapshot.append_color(&background, &rect);
        snapshot.pop();
        snapshot.translate(&gtk::graphene::Point::new(pad_x, pad_y));
        snapshot.append_layout(&layout, &self.color());
        if let Some(icon) = snapshot.to_paintable(Some(&gtk::graphene::Size::new(width, height))) {
            source.set_icon(Some(&icon), pad_x as i32, (height / 2.0) as i32);
        }
    }

    /// Bars and the highlight while the song is the current one; the bars
    /// rest while the player is paused. Only the current row has bars: they
    /// are made when it becomes the current one and go when it stops being it.
    pub(crate) fn set_playing(&self, playing: bool, paused: bool) {
        let mut bars = self.imp().bars.borrow_mut();
        if playing {
            let bars = bars.get_or_insert_with(|| {
                let bars = EqBars::new();
                // After the handle, where a row has one.
                bars.insert_after(self, self.imp().handle.borrow().as_ref());
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
        drop(bars);
        self.imp().now.set(match (playing, paused) {
            (false, _) => Now::Not,
            (true, false) => Now::Playing,
            (true, true) => Now::Paused,
        });
        self.speak();
    }
}

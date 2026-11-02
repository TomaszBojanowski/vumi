//! The play queue in a panel on the right: the tracks in order, the playing one
//! marked; a click jumps to a track. A list view over a model: rows exist only
//! for the tracks on screen (and a few hundred around them) and are reused
//! while scrolling. The model changes only when the queue itself does (a new
//! album, radio, a track added or moved), not on every track change, and then
//! only where the queue changed: a track added at the end is one new row. And
//! only while the panel is on screen: opening it brings it up to date.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext, pgettext};
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use std::cell::{Cell, OnceCell, RefCell};
use std::rc::Rc;

use crate::favorites::{self, Favorite};
use crate::player;
use crate::song_row::SongRow;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/queue_page.blp")]
    pub struct QueuePage {
        #[template_child]
        pub summary: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub actions: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub shuffle_button: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub save_button: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub clear_button: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub stack: gtk::TemplateChild<gtk::Stack>,
        #[template_child]
        pub list: gtk::TemplateChild<gtk::ListView>,
        /// The tracks in the list, one object per track of the queue.
        pub tracks: OnceCell<gio::ListStore>,
        /// The queue the model was filled from, to tell a new queue from a new track.
        pub shown: RefCell<Option<Rc<[subsonic::Song]>>>,
        /// The rows that exist, each with the list item it belongs to (which
        /// knows the place in the queue), to mark the playing track.
        pub rows: RefCell<Vec<(glib::WeakRef<SongRow>, glib::WeakRef<gtk::ListItem>)>>,
        /// What the rows were last placed for: the place of the current track
        /// (none while nothing is loaded) and whether it plays.
        pub placed: Cell<Option<(Option<usize>, bool)>>,
        /// The queue as it was when a row was picked up to be dragged. A drop
        /// counts only while the queue is still that one: the index a drag
        /// carries means nothing in another queue.
        pub dragged_from: RefCell<Option<Rc<[subsonic::Song]>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for QueuePage {
        // Must match "template $VumiQueuePage" in the Blueprint file.
        const NAME: &'static str = "VumiQueuePage";
        type Type = super::QueuePage;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for QueuePage {
        fn constructed(&self) {
            self.parent_constructed();
            // The page lays out its child itself (size_allocate below), and
            // what does not fit in a panel cut short is not drawn.
            self.obj().set_layout_manager(None::<gtk::LayoutManager>);
            self.obj().set_overflow(gtk::Overflow::Hidden);
            self.obj().setup_list();
            self.shuffle_button
                .connect_clicked(|_| player::get().shuffle_rest());
            let page = self.obj().downgrade();
            self.save_button.connect_clicked(move |_| {
                if let Some(page) = page.upgrade() {
                    page.save_as_playlist();
                }
            });
            let page = self.obj().downgrade();
            self.clear_button.connect_clicked(move |_| {
                if let Some(page) = page.upgrade() {
                    page.clear();
                }
            });
            let page = self.obj().downgrade();
            player::get().connect_changed(move || {
                if let Some(page) = page.upgrade() {
                    page.update();
                }
            });
            // What changed while the panel was closed is shown as it opens.
            self.obj().connect_map(|page| page.update());
            // The time left moves on while a track plays, and jumps with a
            // seek. Minutes are shown, so a look every few seconds will do,
            // and only while the panel is on screen.
            let page = self.obj().downgrade();
            player::get().connect_seeked(move |_| {
                if let Some(page) = page.upgrade() {
                    page.update_subtitle();
                }
            });
            let page = self.obj().downgrade();
            glib::timeout_add_seconds_local(10, move || match page.upgrade() {
                Some(page) => {
                    if page.is_mapped() && player::get().is_playing() {
                        page.update_subtitle();
                    }
                    glib::ControlFlow::Continue
                }
                None => glib::ControlFlow::Break,
            });
            self.obj().update();
        }
    }
    impl WidgetImpl for QueuePage {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            match self.obj().child() {
                Some(child) => child.measure(orientation, for_size),
                None => (0, 0, -1, -1),
            }
        }

        // Never narrower than the panel's minimum. A split view that is not
        // collapsed gives its side panel only what the content leaves, and
        // when the window jumps to a narrow size (or starts narrow) that is
        // nothing for one frame, until the breakpoint collapses the view.
        // GTK warns about every widget laid out below its minimum.
        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().child() {
                let (minimum, ..) = child.measure(gtk::Orientation::Horizontal, -1);
                child.allocate(width.max(minimum), height, baseline, None);
            }
        }
    }
    impl BinImpl for QueuePage {}
}

glib::wrapper! {
    pub struct QueuePage(ObjectSubclass<imp::QueuePage>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl QueuePage {
    /// Connects the list to its store and tells it how to fill a row.
    fn setup_list(&self) {
        let imp = self.imp();
        let tracks = gio::ListStore::new::<glib::BoxedAnyObject>();

        let factory = gtk::SignalListItemFactory::new();
        let page = self.downgrade();
        factory.connect_setup(move |_, item| {
            let (Some(page), Some(item)) = (page.upgrade(), item.downcast_ref::<gtk::ListItem>())
            else {
                return;
            };
            let row = page.new_row(item);
            page.imp()
                .rows
                .borrow_mut()
                .push((row.downgrade(), item.downgrade()));
            item.set_child(Some(&row));
        });
        let page = self.downgrade();
        factory.connect_bind(move |_, item| {
            let (Some(page), Some(item)) = (page.upgrade(), item.downcast_ref::<gtk::ListItem>())
            else {
                return;
            };
            let row = item.child().and_downcast::<SongRow>();
            let object = item.item().and_downcast::<glib::BoxedAnyObject>();
            let (Some(row), Some(object)) = (row, object) else {
                return;
            };
            let song = object.borrow::<subsonic::Song>();
            row.set_song(&song, song.artist.as_deref().unwrap_or(""));
            // One line each, the full title in the tooltip: rows stay even.
            row.set_tooltip_text(Some(&song.title));
            item.set_accessible_label(&crate::song_row::accessible_label(&song));
            page.place_row(&row, item);
        });
        factory.connect_unbind(|_, item| {
            let row = item
                .downcast_ref::<gtk::ListItem>()
                .and_then(|item| item.child())
                .and_downcast::<SongRow>();
            if let Some(row) = row {
                row.clear();
            }
        });

        // A click jumps to the track.
        imp.list
            .connect_activate(|_, position| player::get().play_index(position as usize));

        imp.list
            .set_model(Some(&gtk::NoSelection::new(Some(tracks.clone()))));
        imp.list.set_factory(Some(&factory));
        let _ = imp.tracks.set(tracks);
    }

    /// A row for `item`, made once and then reused for whatever track comes
    /// to that item. Its menu and its drag and drop ask the item where in the
    /// queue it is only when they are used.
    fn new_row(&self, item: &gtk::ListItem) -> SongRow {
        let row = SongRow::new();
        row.set_title_lines(1);

        // A ⋮ menu at the end of the row. Its actions are made on the first
        // click (as in song_menu.rs): most rows are never clicked there. The
        // menu itself is put together at every click: one of its items says
        // whether the track is a favourite now.
        let button = gtk::MenuButton::builder()
            .icon_name("view-more-symbolic")
            .tooltip_text(gettext("More"))
            .valign(gtk::Align::Center)
            .css_classes(["flat", "circular"])
            .build();
        let item_weak = item.downgrade();
        let row_weak = row.downgrade();
        let made = Cell::new(false);
        button.set_create_popup_func(move |button| {
            let Some(row) = row_weak.upgrade() else {
                return;
            };
            if !made.replace(true) {
                install_actions(&row, &item_weak);
            }
            let starred = row.song().is_some_and(|song| {
                favorites::is_starred(&Favorite::Song(song.id.clone()), song.starred.is_some())
            });
            button.set_menu_model(Some(&menu(starred)));
        });
        row.add_suffix(&button);

        // Drag and drop: a row carries its index; dropping it on another row
        // moves the track there. The whole row can be grabbed.
        let source = gtk::DragSource::builder()
            .actions(gdk::DragAction::MOVE)
            .build();
        // While dragging: a small label with the title under the pointer,
        // not a copy of the whole row.
        // The icon must be set here, in prepare: by drag-begin GTK has already
        // chosen its default one (a copy of the row).
        let item_weak = item.downgrade();
        let page = self.downgrade();
        source.connect_prepare(move |source, _, _| {
            let index = position_of(&item_weak)?;
            page.upgrade()?
                .imp()
                .dragged_from
                .replace(Some(player::get().queue()));
            let row = source.widget().and_downcast::<SongRow>()?;
            let title = row.song()?.title;
            let layout = row.create_pango_layout(Some(&title));
            layout.set_width(240 * gtk::pango::SCALE);
            layout.set_ellipsize(gtk::pango::EllipsizeMode::End);
            let (text_w, text_h) = layout.pixel_size();
            let (pad_x, pad_y) = (12.0, 6.0);
            let width = text_w as f32 + 2.0 * pad_x;
            let height = text_h as f32 + 2.0 * pad_y;

            let snapshot = gtk::Snapshot::new();
            let dark = adw::StyleManager::default().is_dark();
            let background = if dark {
                gdk::RGBA::new(0.2, 0.2, 0.2, 0.97)
            } else {
                gdk::RGBA::new(1.0, 1.0, 1.0, 0.97)
            };
            let rect = gtk::graphene::Rect::new(0.0, 0.0, width, height);
            snapshot.push_rounded_clip(&gtk::gsk::RoundedRect::from_rect(rect, 8.0));
            snapshot.append_color(&background, &rect);
            snapshot.pop();
            snapshot.translate(&gtk::graphene::Point::new(pad_x, pad_y));
            snapshot.append_layout(&layout, &row.color());
            if let Some(icon) =
                snapshot.to_paintable(Some(&gtk::graphene::Size::new(width, height)))
            {
                source.set_icon(Some(&icon), pad_x as i32, (height / 2.0) as i32);
            }
            Some(gdk::ContentProvider::for_value(&(index as i32).to_value()))
        });
        row.add_controller(source);
        let target = gtk::DropTarget::new(i32::static_type(), gdk::DragAction::MOVE);
        let item_weak = item.downgrade();
        let page = self.downgrade();
        target.connect_drop(move |_, value, _, _| {
            // The queue changed under the drag (another album, the radio, a
            // track removed): the index it carries is of the old queue, and
            // would move some other track now.
            let dragged_from = page
                .upgrade()
                .and_then(|page| page.imp().dragged_from.take());
            let stale =
                !dragged_from.is_some_and(|queue| Rc::ptr_eq(&queue, &player::get().queue()));
            match (value.get::<i32>(), position_of(&item_weak)) {
                (Ok(from), Some(index)) if !stale => {
                    player::get().move_track(from as usize, index);
                    true
                }
                _ => false,
            }
        });
        row.add_controller(target);
        row
    }

    fn update(&self) {
        let imp = self.imp();
        let queue = player::get().queue();

        self.update_subtitle();
        imp.stack
            .set_visible_child_name(if queue.is_empty() { "empty" } else { "queue" });
        // The buttons for the whole queue: each only while it would do something.
        let player = player::get();
        let after = queue.len().saturating_sub(player.position() + 1);
        imp.actions.set_visible(!queue.is_empty());
        imp.shuffle_button.set_sensitive(after > 1);
        imp.save_button.set_sensitive(player.client().is_some());
        imp.clear_button.set_sensitive(queue.len() > 1);

        // A closed panel shows nothing: the rows wait until it opens.
        if !self.is_mapped() {
            return;
        }
        let same = imp
            .shown
            .borrow()
            .as_ref()
            .is_some_and(|shown| Rc::ptr_eq(shown, &queue));
        if !same {
            self.fill(&queue);
            imp.shown.replace(Some(queue));
        }
        // The player reports every change, the volume too: the rows are gone
        // through only when the queue, the current track or the play state
        // is another one.
        let state = (
            player.is_active().then(|| player.position()),
            player.is_playing(),
        );
        if !same || imp.placed.get() != Some(state) {
            imp.placed.set(Some(state));
            self.place_rows();
        }
    }

    /// Clear Queue: everything but the current track goes. A long queue takes
    /// a while to put together, so that one is not thrown away on one click.
    fn clear(&self) {
        let removed = player::get().queue().len().saturating_sub(1) as u32;
        if removed < CONFIRM_ABOVE {
            player::get().clear_rest();
            return;
        }
        // Translators: {n} is a number of tracks.
        let body = ngettext(
            "{n} track will be removed from the queue.",
            "{n} tracks will be removed from the queue.",
            removed,
        )
        .replace("{n}", &removed.to_string());
        let dialog = adw::AlertDialog::new(Some(&gettext("Clear Queue?")), Some(&body));
        dialog.add_responses(&[("cancel", &gettext("Cancel")), ("clear", &gettext("Clear"))]);
        dialog.set_response_appearance("clear", adw::ResponseAppearance::Destructive);
        dialog.set_default_response(Some("cancel"));
        dialog.set_close_response("cancel");
        dialog.connect_response(Some("clear"), |_, _| player::get().clear_rest());
        dialog.present(Some(self));
    }

    /// Save as Playlist: asks for a name and makes a playlist on the server
    /// with the tracks of the queue, in their order.
    fn save_as_playlist(&self) {
        let player = player::get();
        let Some(client) = player.client() else {
            return;
        };
        let ids: Vec<String> = player.queue().iter().map(|song| song.id.clone()).collect();
        if ids.is_empty() {
            return;
        }

        let entry = gtk::Entry::builder()
            .placeholder_text(gettext("Name"))
            .activates_default(true)
            .build();
        let dialog = adw::AlertDialog::new(Some(&gettext("New Playlist")), None);
        dialog.set_extra_child(Some(&entry));
        dialog.add_responses(&[("cancel", &gettext("Cancel")), ("save", &gettext("Save"))]);
        dialog.set_response_appearance("save", adw::ResponseAppearance::Suggested);
        dialog.set_default_response(Some("save"));
        dialog.set_close_response("cancel");
        // A playlist needs a name.
        dialog.set_response_enabled("save", false);
        let dialog_weak = dialog.downgrade();
        entry.connect_changed(move |entry| {
            if let Some(dialog) = dialog_weak.upgrade() {
                dialog.set_response_enabled("save", !entry.text().trim().is_empty());
            }
        });

        let page = self.downgrade();
        dialog.connect_response(Some("save"), move |_, _| {
            let name = entry.text().trim().to_string();
            let (Some(page), false) = (page.upgrade(), name.is_empty()) else {
                return;
            };
            let request = client.clone();
            let ids = ids.clone();
            glib::spawn_future_local(async move {
                let result = crate::runtime()
                    .spawn(async move {
                        let ids: Vec<&str> = ids.iter().map(String::as_str).collect();
                        request.create_playlist(&name, &ids).await
                    })
                    .await;
                match result {
                    Ok(Ok(playlist)) => {
                        crate::toast(
                            &page,
                            // Translators: {name} is the name of a playlist.
                            &gettext("Saved as “{name}”").replace("{name}", &playlist.name),
                        );
                        // The new playlist shows in the Playlists section.
                        if let Some(main) = page
                            .ancestor(crate::main_view::MainView::static_type())
                            .and_downcast::<crate::main_view::MainView>()
                        {
                            main.sync_playlists();
                        }
                    }
                    _ => crate::toast(&page, &gettext("Could not save the playlist")),
                }
            });
        });
        dialog.present(Some(self));
    }

    /// Under the header: how many tracks, how long they are together and how
    /// much of that is still to come, the rest of the current track included
    /// ("122 tracks · 8 h 12 min · 7 h 40 min left"). Nothing is left to come
    /// when nothing is in progress (the queue has ended, playback failed).
    fn update_subtitle(&self) {
        let player = player::get();
        let queue = player.queue();
        let mut parts = Vec::new();
        if !queue.is_empty() {
            let count = queue.len() as u32;
            // Translators: {n} is a number of tracks in the queue.
            let tracks = ngettext("{n} track", "{n} tracks", count);
            parts.push(tracks.replace("{n}", &count.to_string()));
            let seconds = |songs: &[subsonic::Song]| -> u32 {
                songs.iter().filter_map(|song| song.duration).sum()
            };
            let total = seconds(&queue);
            if total > 0 {
                parts.push(crate::album_page::format_duration(total));
            }
            if player.is_active() {
                let position = player.position().min(queue.len() - 1);
                let played = player.position_secs().unwrap_or(0.0) as u32;
                let left = seconds(&queue[position..]).saturating_sub(played);
                if left > 0 {
                    // Translators: {time} is a short duration, e.g. "12 min" or "1 h 5 min".
                    parts.push(gettext("{time} left").replace("{time}", &short_duration(left)));
                }
            }
        }
        // In a narrow panel the line wraps: between the parts, never inside
        // one (no-break spaces), and the dot stays with the part before it.
        let parts: Vec<String> = parts
            .iter()
            .map(|part| part.replace(' ', "\u{a0}"))
            .collect();
        let subtitle = parts.join("\u{a0}· ");
        // Asked for often (a timer, every change of the player): the label
        // is touched only when the text is another one.
        let summary = &self.imp().summary;
        if summary.label() != subtitle {
            summary.set_label(&subtitle);
            summary.set_visible(!subtitle.is_empty());
        }
    }

    /// Brings the model in line with `queue`: one object per track. Only the
    /// part that differs from what is shown is replaced (what the two have in
    /// common at the start and at the end stays), so the rows of the tracks
    /// that did not change stay too, and the list keeps its place.
    fn fill(&self, queue: &Rc<[subsonic::Song]>) {
        let imp = self.imp();
        let Some(tracks) = imp.tracks.get() else {
            return;
        };
        let shown = imp.shown.borrow();
        let old: &[subsonic::Song] = shown.as_deref().unwrap_or(&[]);
        let start = old
            .iter()
            .zip(queue.iter())
            .take_while(|(a, b)| a == b)
            .count();
        let end = old[start..]
            .iter()
            .rev()
            .zip(queue[start..].iter().rev())
            .take_while(|(a, b)| a == b)
            .count();
        let removed = old.len() - start - end;
        let added: Vec<glib::BoxedAnyObject> = queue[start..queue.len() - end]
            .iter()
            .cloned()
            .map(glib::BoxedAnyObject::new)
            .collect();
        if removed > 0 || !added.is_empty() {
            // One change notification for the whole stretch, not one per track.
            tracks.splice(start as u32, removed as u32, &added);
        }
    }

    /// What depends on a row's place in the queue, for every row that exists:
    /// the rounded ends of the card and the mark on the playing track. A row
    /// keeps its track when others are added or removed around it, but its
    /// place changes; a row that comes into view is placed as it is filled.
    fn place_rows(&self) {
        let mut rows = self.imp().rows.borrow_mut();
        rows.retain(|(row, item)| row.upgrade().is_some() && item.upgrade().is_some());
        for (row, item) in rows.iter() {
            if let (Some(row), Some(item)) = (row.upgrade(), item.upgrade()) {
                self.place_row(&row, &item);
            }
        }
    }

    fn place_row(&self, row: &SongRow, item: &gtk::ListItem) {
        // The tracks look like one card: the first and the last row round
        // its corners (style.css).
        let last = self.imp().tracks.get().map_or(0, |tracks| tracks.n_items());
        for (class, on) in [
            ("first", item.position() == 0),
            ("last", item.position() + 1 == last),
        ] {
            if on {
                row.add_css_class(class);
            } else {
                row.remove_css_class(class);
            }
        }
        let player = player::get();
        let current = player.is_active().then(|| player.position());
        // A row without a track (off screen) is nobody's.
        let playing = row.song().is_some() && current == Some(item.position() as usize);
        row.set_playing(playing, !player.is_playing());
    }
}

/// The actions of a queue row's menu, put on `row`: the song menu's own
/// (group "song": add to a playlist, similar songs, album, artist,
/// information) and those of the queue (group "queue").
fn install_actions(row: &SongRow, item: &glib::WeakRef<gtk::ListItem>) {
    let row_weak = row.downgrade();
    let current: crate::song_menu::Current = Rc::new(move || {
        let song = row_weak.upgrade()?.song()?;
        Some((song, player::get().client()?))
    });
    crate::song_menu::install_actions(row.upcast_ref(), &current);

    let actions = gio::SimpleActionGroup::new();
    // Play Next: the entry moves to right after the current track. Taking a
    // track out from before the current one moves that one up a place.
    let play_next = gio::SimpleAction::new("play-next", None);
    let item_weak = item.clone();
    play_next.connect_activate(move |_, _| {
        let Some(index) = position_of(&item_weak) else {
            return;
        };
        let position = player::get().position();
        let to = if index > position {
            position + 1
        } else {
            position
        };
        if index != position && index != to {
            player::get().move_track(index, to);
        }
    });
    actions.add_action(&play_next);

    let remove = gio::SimpleAction::new("remove", None);
    let item_weak = item.clone();
    remove.connect_activate(move |_, _| {
        if let Some(index) = position_of(&item_weak) {
            player::get().remove(index);
        }
    });
    actions.add_action(&remove);

    // Adds the track to the favourites, or takes it out if it is one.
    let favorite = gio::SimpleAction::new("favorite", None);
    let row_weak = row.downgrade();
    favorite.connect_activate(move |_, _| {
        let Some(row) = row_weak.upgrade() else {
            return;
        };
        let (Some(song), Some(client)) = (row.song(), player::get().client()) else {
            return;
        };
        let item = Favorite::Song(song.id.clone());
        let starred = favorites::is_starred(&item, song.starred.is_some());
        favorites::set(&row, client, item, !starred);
    });
    actions.add_action(&favorite);
    row.insert_action_group("queue", Some(&actions));
}

/// The menu of a queue row: its place in the queue, keeping the track, where
/// it comes from, more like it. No "Add to Queue": it is in the queue.
fn menu(starred: bool) -> gio::Menu {
    let menu = gio::Menu::new();
    let place = gio::Menu::new();
    // Translators: in the queue, moves the track to right after the current
    // one (elsewhere "Play Next" adds a track to the queue).
    let play_next = pgettext("queue", "Play Next");
    place.append(Some(&play_next), Some("queue.play-next"));
    place.append(Some(&gettext("Remove from Queue")), Some("queue.remove"));
    menu.append_section(None, &place);
    let keep = gio::Menu::new();
    keep.append(
        Some(&gettext("Add to Playlist…")),
        Some("song.add-to-playlist"),
    );
    let favorite = if starred {
        gettext("Remove from Favorites")
    } else {
        gettext("Add to Favorites")
    };
    keep.append(Some(&favorite), Some("queue.favorite"));
    menu.append_section(None, &keep);
    let navigation = gio::Menu::new();
    navigation.append(Some(&gettext("Go to Album")), Some("song.go-to-album"));
    navigation.append(Some(&gettext("Go to Artist")), Some("song.go-to-artist"));
    navigation.append(Some(&gettext("Song Information")), Some("song.info"));
    menu.append_section(None, &navigation);
    let radio = gio::Menu::new();
    radio.append(Some(&gettext("Play Similar")), Some("song.play-similar"));
    menu.append_section(None, &radio);
    menu
}

/// Clear Queue asks first when it would remove more tracks than this.
const CONFIRM_ABOVE: u32 = 20;

/// A duration in few characters, a started minute counted as a whole one:
/// "12 min", "1 h 5 min".
fn short_duration(seconds: u32) -> String {
    let minutes = seconds.div_ceil(60);
    if minutes < 60 {
        // Translators: a short duration in minutes, e.g. "12 min".
        gettext("{n} min").replace("{n}", &minutes.to_string())
    } else {
        // Translators: total length of a long album, e.g. "6 h 9 min".
        gettext("{hours} h {minutes} min")
            .replace("{hours}", &(minutes / 60).to_string())
            .replace("{minutes}", &(minutes % 60).to_string())
    }
}

/// Where in the queue the track of this list item is now, if it shows one.
fn position_of(item: &glib::WeakRef<gtk::ListItem>) -> Option<usize> {
    let item = item.upgrade()?;
    item.item()?;
    Some(item.position() as usize)
}

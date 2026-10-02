//! The play queue in a panel on the right: the tracks in order, the playing one
//! marked; a click jumps to a track. A list view over a model: rows exist only
//! for the tracks on screen (and a few hundred around them) and are reused
//! while scrolling. The model changes only when the queue itself does (a new
//! album, radio, a track added or moved), not on every track change, and only
//! while the panel is on screen: opening it brings it up to date.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use std::cell::{OnceCell, RefCell};
use std::rc::Rc;

use crate::player;
use crate::song_row::SongRow;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/queue_page.blp")]
    pub struct QueuePage {
        #[template_child]
        pub title: gtk::TemplateChild<adw::WindowTitle>,
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
            self.obj().setup_list();
            let page = self.obj().downgrade();
            player::get().connect_changed(move || {
                if let Some(page) = page.upgrade() {
                    page.update();
                }
            });
            // What changed while the panel was closed is shown as it opens.
            self.obj().connect_map(|page| page.update());
            self.obj().update();
        }
    }
    impl WidgetImpl for QueuePage {}
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
            page.imp().rows.borrow_mut().push((row.downgrade(), item.downgrade()));
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
            // The tracks look like one card: the first and the last row
            // round its corners (style.css).
            let last = page.imp().tracks.get().map_or(0, |tracks| tracks.n_items());
            for (class, on) in [("first", item.position() == 0), ("last", item.position() + 1 == last)] {
                if on {
                    row.add_css_class(class);
                } else {
                    row.remove_css_class(class);
                }
            }
            page.mark_row(&row, item);
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

        // A ⋮ menu at the end of the row: "Remove from Queue".
        let actions = gio::SimpleActionGroup::new();
        let remove = gio::SimpleAction::new("remove", None);
        let item_weak = item.downgrade();
        remove.connect_activate(move |_, _| {
            if let Some(index) = position_of(&item_weak) {
                player::get().remove(index);
            }
        });
        actions.add_action(&remove);
        row.insert_action_group("queue", Some(&actions));
        let menu = gio::Menu::new();
        menu.append(Some(&gettext("Remove from Queue")), Some("queue.remove"));
        row.add_suffix(
            &gtk::MenuButton::builder()
                .icon_name("view-more-symbolic")
                .menu_model(&menu)
                .tooltip_text(gettext("More"))
                .valign(gtk::Align::Center)
                .css_classes(["flat", "circular"])
                .build(),
        );

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
        source.connect_prepare(move |source, _, _| {
            let index = position_of(&item_weak)?;
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
        target.connect_drop(move |_, value, _, _| {
            match (value.get::<i32>(), position_of(&item_weak)) {
                (Ok(from), Some(index)) => {
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

        let count = queue.len() as u32;
        // Translators: {n} is a number of tracks in the queue.
        let subtitle = ngettext("{n} track", "{n} tracks", count).replace("{n}", &count.to_string());
        imp.title.set_subtitle(if count > 0 { &subtitle } else { "" });
        imp.stack
            .set_visible_child_name(if count > 0 { "queue" } else { "empty" });

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
        self.mark_playing();
    }

    /// One object per track; the list view makes rows for those in sight.
    fn fill(&self, queue: &Rc<[subsonic::Song]>) {
        let objects: Vec<glib::BoxedAnyObject> =
            queue.iter().cloned().map(glib::BoxedAnyObject::new).collect();
        if let Some(tracks) = self.imp().tracks.get() {
            // One change notification for the whole list, not one per track.
            tracks.splice(0, tracks.n_items(), &objects);
        }
    }

    /// Bars and the highlight on the playing track. Only rows that exist need
    /// it; a row that comes into view is marked as it is filled.
    fn mark_playing(&self) {
        let mut rows = self.imp().rows.borrow_mut();
        rows.retain(|(row, item)| row.upgrade().is_some() && item.upgrade().is_some());
        for (row, item) in rows.iter() {
            if let (Some(row), Some(item)) = (row.upgrade(), item.upgrade()) {
                self.mark_row(&row, &item);
            }
        }
    }

    fn mark_row(&self, row: &SongRow, item: &gtk::ListItem) {
        let player = player::get();
        let current = player.is_active().then(|| player.position());
        // A row without a track (off screen) is nobody's.
        let playing = row.song().is_some() && current == Some(item.position() as usize);
        row.set_playing(playing, !player.is_playing());
    }
}

/// Where in the queue the track of this list item is now, if it shows one.
fn position_of(item: &glib::WeakRef<gtk::ListItem>) -> Option<usize> {
    let item = item.upgrade()?;
    item.item()?;
    Some(item.position() as usize)
}

//! The play queue in a panel on the right: the tracks in order, the playing one
//! marked; a click jumps to a track. Rebuilt only when the queue itself changes
//! (a new album, radio), not on every track change.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::gdk;
use gtk::gio;
use gtk::glib;
use std::cell::RefCell;
use std::rc::Rc;

use crate::player;

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
        pub list: gtk::TemplateChild<gtk::ListBox>,
        /// The queue the rows were built from, to tell a new queue from a new track.
        pub shown: RefCell<Option<Rc<[subsonic::Song]>>>,
        /// Rows with their bars, to mark the playing track.
        pub rows: RefCell<Vec<(adw::ActionRow, gtk::Box)>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for QueuePage {
        // Must match "template $MusePlayerQueuePage" in the Blueprint file.
        const NAME: &'static str = "MusePlayerQueuePage";
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
            let page = self.obj().downgrade();
            player::get().connect_changed(move || {
                if let Some(page) = page.upgrade() {
                    page.update();
                }
            });
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
    fn update(&self) {
        let imp = self.imp();
        let queue = player::get().queue();
        let same = imp
            .shown
            .borrow()
            .as_ref()
            .is_some_and(|shown| Rc::ptr_eq(shown, &queue));
        if !same {
            self.rebuild(&queue);
            imp.shown.replace(Some(queue));
        }
        self.mark_playing();
    }

    /// One row per track; a click jumps to it.
    fn rebuild(&self, queue: &Rc<[subsonic::Song]>) {
        let imp = self.imp();
        imp.list.remove_all();
        imp.rows.borrow_mut().clear();
        for (index, song) in queue.iter().enumerate() {
            let row = adw::ActionRow::builder()
                .title(song.title.as_str())
                .use_markup(false)
                .activatable(true)
                // One line each, the full title in the tooltip: rows stay even.
                .title_lines(1)
                .subtitle_lines(1)
                .tooltip_text(song.title.as_str())
                .css_classes(["queue-row"])
                .build();
            row.set_subtitle(song.artist.as_deref().unwrap_or(""));
            let bars = crate::album_page::eq_bars();
            bars.set_opacity(0.0);
            row.add_prefix(&bars);
            row.connect_activated(move |_| player::get().play_index(index));

            if let Some(d) = song.duration {
                row.add_suffix(
                    &gtk::Label::builder()
                        .label(format!("{}:{:02}", d / 60, d % 60))
                        .css_classes(["dim-label", "numeric"])
                        .build(),
                );
            }

            // A ⋮ menu at the end of the row: "Remove from Queue".
            let actions = gio::SimpleActionGroup::new();
            let remove = gio::SimpleAction::new("remove", None);
            remove.connect_activate(move |_, _| player::get().remove(index));
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
            let title = song.title.clone();
            // The icon must be set here, in prepare: by drag-begin GTK has already
            // chosen its default one (a copy of the row).
            source.connect_prepare(move |source, _, _| {
                let row = source.widget()?;
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
            target.connect_drop(move |_, value, _, _| match value.get::<i32>() {
                Ok(from) => {
                    player::get().move_track(from as usize, index);
                    true
                }
                Err(_) => false,
            });
            row.add_controller(target);
            imp.list.append(&row);
            imp.rows.borrow_mut().push((row, bars));
        }

        let count = queue.len() as u32;
        // Translators: {n} is a number of tracks in the queue.
        let subtitle = ngettext("{n} track", "{n} tracks", count).replace("{n}", &count.to_string());
        imp.title.set_subtitle(if count > 0 { &subtitle } else { "" });
        imp.stack
            .set_visible_child_name(if count > 0 { "queue" } else { "empty" });
    }

    fn mark_playing(&self) {
        let player = player::get();
        let current = player.is_active().then(|| player.position());
        for (index, (row, bars)) in self.imp().rows.borrow().iter().enumerate() {
            let playing = current == Some(index);
            bars.set_opacity(if playing { 1.0 } else { 0.0 });
            if player.is_playing() {
                bars.remove_css_class("paused");
            } else {
                bars.add_css_class("paused");
            }
            if playing {
                row.add_css_class("playing");
            } else {
                row.remove_css_class("playing");
            }
        }
    }
}

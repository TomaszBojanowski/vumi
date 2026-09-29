//! The play queue in a panel on the right: the tracks in order, the playing one
//! marked; a click jumps to a track. Rebuilt only when the queue itself changes
//! (a new album, radio), not on every track change.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::ngettext;
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
                .build();
            row.set_subtitle(song.artist.as_deref().unwrap_or(""));
            let bars = crate::album_page::eq_bars();
            bars.set_opacity(0.0);
            row.add_prefix(&bars);
            if let Some(d) = song.duration {
                row.add_suffix(
                    &gtk::Label::builder()
                        .label(format!("{}:{:02}", d / 60, d % 60))
                        .css_classes(["dim-label", "numeric"])
                        .build(),
                );
            }
            row.connect_activated(move |_| player::get().play_index(index));
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

//! Compact now-playing bar for narrow windows: cover, title and play/pause.
//! It follows the player like the sidebar card; a click opens the sidebar.

use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;

use crate::{covers, player};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/mini_player.blp")]
    pub struct MiniPlayer {
        #[template_child]
        pub revealer: gtk::TemplateChild<gtk::Revealer>,
        #[template_child]
        pub bar: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub text: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub cover: gtk::TemplateChild<gtk::Picture>,
        #[template_child]
        pub title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub artist: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub play_pause: gtk::TemplateChild<gtk::Button>,
        /// The cover on screen, so it is only reloaded when the album changes.
        pub cover_id: RefCell<Option<String>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for MiniPlayer {
        // Must match "template $VumiMiniPlayer" in the Blueprint file.
        const NAME: &'static str = "VumiMiniPlayer";
        type Type = super::MiniPlayer;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for MiniPlayer {}
    impl WidgetImpl for MiniPlayer {}
    impl BinImpl for MiniPlayer {}
}

glib::wrapper! {
    pub struct MiniPlayer(ObjectSubclass<imp::MiniPlayer>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl MiniPlayer {
    pub(crate) fn new() -> Self {
        let mini: Self = glib::Object::new();
        mini.imp()
            .play_pause
            .connect_clicked(|_| player::get().toggle_pause());

        let mini_weak = mini.downgrade();
        player::get().connect_changed(move || {
            if let Some(mini) = mini_weak.upgrade() {
                mini.update();
            }
        });
        mini
    }

    /// Runs `f` when the bar is clicked (the play/pause button keeps its own
    /// click), and when Enter or Space is pressed on its text, which takes the
    /// keyboard focus and is a button to a screen reader ("Show Player").
    pub(crate) fn connect_open(&self, f: impl Fn() + 'static) {
        let f = std::rc::Rc::new(f);
        let click = gtk::GestureClick::new();
        let on_click = f.clone();
        click.connect_released(move |_, _, _, _| on_click());
        self.imp().bar.add_controller(click);
        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(move |_, key, _, _| {
            use gtk::gdk::Key;
            if matches!(
                key,
                Key::Return | Key::KP_Enter | Key::ISO_Enter | Key::space | Key::KP_Space
            ) {
                f();
                glib::Propagation::Stop
            } else {
                glib::Propagation::Proceed
            }
        });
        let text = self.imp().text.get();
        text.add_controller(keys);
        text.update_property(&[gtk::accessible::Property::Label(&gettext("Show Player"))]);
    }

    fn update(&self) {
        let imp = self.imp();
        let player = player::get();
        let Some(song) = player.current() else {
            imp.revealer.set_reveal_child(false);
            return;
        };

        imp.revealer.set_reveal_child(true);
        imp.title.set_label(&song.title);
        imp.artist.set_label(song.artist.as_deref().unwrap_or(""));
        imp.text
            .update_property(&[gtk::accessible::Property::Description(&crate::a11y::join(
                [song.title.as_str(), song.artist.as_deref().unwrap_or("")],
            ))]);

        if player.is_playing() {
            imp.play_pause
                .set_icon_name("media-playback-pause-symbolic");
            imp.play_pause.set_tooltip_text(Some(&gettext("Pause")));
        } else {
            imp.play_pause
                .set_icon_name("media-playback-start-symbolic");
            imp.play_pause.set_tooltip_text(Some(&gettext("Play")));
        }

        // A new cover only when the album changes: the album's, as on the card
        // (covers::song_cover). The old cover stays until the new one is ready.
        let key = song.album_id.clone().or_else(|| song.cover_art.clone());
        if *imp.cover_id.borrow() != key {
            imp.cover_id.replace(key.clone());
            match (covers::song_cover(&song), key) {
                (Some(id), Some(key)) => {
                    covers::load_for(self, player.client(), id, move |mini, texture| {
                        let imp = mini.imp();
                        if imp.cover_id.borrow().as_deref() == Some(key.as_str()) {
                            covers::show(&imp.cover, texture.as_ref());
                        }
                    });
                }
                _ => covers::show(&imp.cover, None),
            }
        }
    }
}

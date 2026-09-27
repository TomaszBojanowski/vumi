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
        // Must match "template $MusePlayerMiniPlayer" in the Blueprint file.
        const NAME: &'static str = "MusePlayerMiniPlayer";
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

    /// Runs `f` when the bar is clicked (the play/pause button keeps its own click).
    pub(crate) fn connect_open(&self, f: impl Fn() + 'static) {
        let click = gtk::GestureClick::new();
        click.connect_released(move |_, _, _, _| f());
        self.imp().bar.add_controller(click);
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

        if player.is_playing() {
            imp.play_pause.set_icon_name("media-playback-pause-symbolic");
            imp.play_pause.set_tooltip_text(Some(&gettext("Pause")));
        } else {
            imp.play_pause.set_icon_name("media-playback-start-symbolic");
            imp.play_pause.set_tooltip_text(Some(&gettext("Play")));
        }

        // A new cover only when the album changes.
        if *imp.cover_id.borrow() != song.cover_art {
            imp.cover_id.replace(song.cover_art.clone());
            imp.cover.set_paintable(None::<&gtk::gdk::Paintable>);
            if let Some(id) = song.cover_art.clone() {
                let mini = self.downgrade();
                let client = player.client();
                glib::spawn_future_local(async move {
                    let texture = covers::load(client, id.clone()).await;
                    let Some(mini) = mini.upgrade() else {
                        return;
                    };
                    let imp = mini.imp();
                    if imp.cover_id.borrow().as_deref() == Some(id.as_str()) {
                        imp.cover.set_paintable(texture.as_ref());
                    }
                });
            }
        }
    }
}

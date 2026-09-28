//! Now-playing card at the bottom of the sidebar: cover, title, album,
//! quality, progress and controls, tinted after the album cover.
//! Only while music plays does it refresh the progress, twice a second.

use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{OnceCell, RefCell};
use std::time::Duration;

use crate::album_page::quality_text;
use crate::main_view::MainView;
use crate::{covers, player, tint};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/now_playing.blp")]
    pub struct NowPlaying {
        #[template_child]
        pub revealer: gtk::TemplateChild<gtk::Revealer>,
        #[template_child]
        pub pill: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub progress: gtk::TemplateChild<gtk::Scale>,
        #[template_child]
        pub cover: gtk::TemplateChild<gtk::Picture>,
        #[template_child]
        pub cover_frame: gtk::TemplateChild<gtk::AspectFrame>,
        #[template_child]
        pub title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub artist: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub album: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub quality: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub time: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub previous: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub play_pause: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub next: gtk::TemplateChild<gtk::Button>,
        /// The cover on screen, so it is only reloaded when the album changes.
        pub cover_id: RefCell<Option<String>>,
        /// The progress timer; it exists only while music plays.
        pub timer: RefCell<Option<glib::SourceId>>,
        /// The pill's own colour class, rewritten for every new cover.
        pub(crate) tint: OnceCell<tint::Dynamic>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NowPlaying {
        // Must match "template $MusePlayerNowPlaying" in the Blueprint file.
        const NAME: &'static str = "MusePlayerNowPlaying";
        type Type = super::NowPlaying;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for NowPlaying {}
    impl WidgetImpl for NowPlaying {}
    impl BinImpl for NowPlaying {}
}

glib::wrapper! {
    pub struct NowPlaying(ObjectSubclass<imp::NowPlaying>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl NowPlaying {
    pub(crate) fn new() -> Self {
        let bar: Self = glib::Object::new();
        let imp = bar.imp();

        let tint = tint::Dynamic::new();
        imp.pill.add_css_class(tint.class());
        let _ = imp.tint.set(tint);

        imp.play_pause.connect_clicked(|_| player::get().toggle_pause());
        imp.previous.connect_clicked(|_| player::get().previous());
        imp.next.connect_clicked(|_| player::get().next());

        // A click on the cover opens the album that is playing.
        let click = gtk::GestureClick::new();
        let bar_weak = bar.downgrade();
        click.connect_released(move |_, _, _, _| {
            let Some(bar) = bar_weak.upgrade() else {
                return;
            };
            let Some(song) = player::get().current() else {
                return;
            };
            let Some(album_id) = song.album_id.clone() else {
                return;
            };
            let fallback = subsonic::Album {
                id: album_id.clone(),
                name: song.album.clone().unwrap_or_default(),
                artist: song.artist.clone(),
                year: None,
                song_count: None,
                cover_art: song.cover_art.clone(),
                created: None,
            };
            if let Some(main) = bar
                .ancestor(MainView::static_type())
                .and_downcast::<MainView>()
            {
                main.open_album(&album_id, fallback);
            }
        });
        imp.cover_frame.add_controller(click);
        imp.cover_frame.set_cursor_from_name(Some("pointer"));
        imp.cover_frame
            .set_tooltip_text(Some(&gettext("Go to album")));

        // Only a drag by the user seeks; updates from the timer do not.
        imp.progress.connect_change_value(|_, _, value| {
            player::get().seek(value);
            glib::Propagation::Proceed
        });

        let bar_weak = bar.downgrade();
        player::get().connect_changed(move || {
            if let Some(bar) = bar_weak.upgrade() {
                bar.update();
            }
        });
        bar
    }

    fn update(&self) {
        let imp = self.imp();
        let player = player::get();
        let Some(song) = player.current() else {
            imp.revealer.set_reveal_child(false);
            self.stop_timer();
            return;
        };

        imp.revealer.set_reveal_child(true);
        imp.title.set_label(&song.title);
        imp.artist.set_label(song.artist.as_deref().unwrap_or(""));
        imp.album.set_label(song.album.as_deref().unwrap_or(""));
        match quality_text(std::slice::from_ref(&song)) {
            Some((text, hi_res)) => {
                imp.quality.set_label(&text);
                if hi_res {
                    imp.quality.add_css_class("hi-res");
                } else {
                    imp.quality.remove_css_class("hi-res");
                }
                imp.quality.set_visible(true);
            }
            None => imp.quality.set_visible(false),
        }
        imp.progress
            .adjustment()
            .set_upper(song.duration.unwrap_or(0).max(1) as f64);

        if player.is_playing() {
            imp.play_pause.set_icon_name("media-playback-pause-symbolic");
            imp.play_pause.set_tooltip_text(Some(&gettext("Pause")));
            self.start_timer();
        } else {
            imp.play_pause.set_icon_name("media-playback-start-symbolic");
            imp.play_pause.set_tooltip_text(Some(&gettext("Play")));
            self.stop_timer();
        }
        self.tick();

        // A new cover, and a new colour, only when the album changes. Tracks of
        // one album have their own cover ids on the server, so the album decides.
        let key = song.album_id.clone().or_else(|| song.cover_art.clone());
        if *imp.cover_id.borrow() != key {
            imp.cover_id.replace(key.clone());
            match (song.cover_art.clone(), key) {
                // The old cover stays until the new one is ready: no blank flash.
                (Some(id), Some(key)) => self.load_cover(player.client(), id, key),
                _ => {
                    imp.cover.set_paintable(None::<&gtk::gdk::Paintable>);
                    if let Some(tint) = imp.tint.get() {
                        tint.clear();
                    }
                }
            }
        }
    }

    fn load_cover(&self, client: Option<subsonic::Client>, id: String, key: String) {
        covers::load_for(self, client, id, move |bar, texture| {
            let imp = bar.imp();
            if imp.cover_id.borrow().as_deref() != Some(key.as_str()) {
                return;
            }
            imp.cover.set_paintable(texture.as_ref());
            if let Some(tint) = imp.tint.get() {
                match texture.as_ref().and_then(tint::dominant_color) {
                    Some(color) => tint.set_solid(color),
                    None => tint.clear(),
                }
            }
        });
    }

    fn start_timer(&self) {
        let imp = self.imp();
        if imp.timer.borrow().is_some() {
            return;
        }
        let bar = self.downgrade();
        let id = glib::timeout_add_local(Duration::from_millis(500), move || match bar.upgrade() {
            Some(bar) => {
                bar.tick();
                glib::ControlFlow::Continue
            }
            None => glib::ControlFlow::Break,
        });
        imp.timer.replace(Some(id));
    }

    fn stop_timer(&self) {
        if let Some(id) = self.imp().timer.take() {
            id.remove();
        }
    }

    /// Moves the progress line and the time label to the current position.
    fn tick(&self) {
        let imp = self.imp();
        let position = player::get().position_secs().unwrap_or(0.0);
        imp.progress.set_value(position);
        let duration = imp.progress.adjustment().upper() as u64;
        imp.time
            .set_label(&format!("{} / {}", clock(position as u64), clock(duration)));
    }
}

/// Seconds as "m:ss".
fn clock(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

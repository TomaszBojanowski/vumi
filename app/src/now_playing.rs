//! Now-playing card at the bottom of the sidebar: cover, title, album,
//! progress and controls, tinted after the album cover.
//! Only while music plays does it refresh the progress, twice a second.

use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{OnceCell, RefCell};
use std::time::Duration;

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
        pub elapsed: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub total: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub star_slot: gtk::TemplateChild<adw::Bin>,
        #[template_child]
        pub volume_button: gtk::TemplateChild<gtk::MenuButton>,
        #[template_child]
        pub volume_scale: gtk::TemplateChild<gtk::Scale>,
        /// The song whose star is in `star_slot`.
        pub star_song: RefCell<Option<String>>,
        #[template_child]
        pub queue_button: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub previous: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub play_pause: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub next: gtk::TemplateChild<gtk::Button>,
        /// The cover on screen, so it is only reloaded when the album changes.
        pub cover_id: RefCell<Option<String>>,
        /// A seek waiting for the drag to pause; the timer leaves the slider alone meanwhile.
        pub seek_pending: std::cell::RefCell<Option<glib::SourceId>>,
        /// The progress timer; it exists only while music plays.
        pub timer: RefCell<Option<glib::SourceId>>,
        /// The pill's own colour class, rewritten for every new cover.
        pub(crate) tint: OnceCell<tint::Dynamic>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NowPlaying {
        // Must match "template $VumiNowPlaying" in the Blueprint file.
        const NAME: &'static str = "VumiNowPlaying";
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

        imp.play_pause
            .connect_clicked(|_| player::get().toggle_pause());
        imp.volume_scale
            .connect_value_changed(|scale| player::get().set_volume(scale.value()));
        // The scroll wheel over the button: 5% a step, without opening it.
        let scroll = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
        scroll.connect_scroll(|_, _, dy| {
            let player = player::get();
            player.set_volume(player.volume() - dy * 0.05);
            glib::Propagation::Stop
        });
        imp.volume_button.add_controller(scroll);
        imp.previous.connect_clicked(|_| player::get().previous());
        imp.next.connect_clicked(|_| player::get().next());
        imp.queue_button.connect_clicked(|button| {
            if let Some(main) = button
                .ancestor(MainView::static_type())
                .and_downcast::<MainView>()
            {
                main.toggle_queue();
            }
        });

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
                artist_id: None,
                starred: None,
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
            .set_tooltip_text(Some(&gettext("Go to Album")));

        // Only a drag by the user seeks; updates from the timer do not. The seek
        // waits until the drag pauses for a moment: seeking on every step
        // flushes the stream and asks the server again each time.
        let bar_weak = bar.downgrade();
        imp.progress.connect_change_value(move |_, _, value| {
            let Some(bar) = bar_weak.upgrade() else {
                return glib::Propagation::Proceed;
            };
            if let Some(id) = bar.imp().seek_pending.take() {
                id.remove();
            }
            let bar_weak = bar.downgrade();
            let id = glib::timeout_add_local_once(Duration::from_millis(150), move || {
                if let Some(bar) = bar_weak.upgrade() {
                    bar.imp().seek_pending.take();
                }
                player::get().seek(value);
            });
            bar.imp().seek_pending.replace(Some(id));
            glib::Propagation::Proceed
        });

        let bar_weak = bar.downgrade();
        player::get().connect_changed(move || {
            if let Some(bar) = bar_weak.upgrade() {
                bar.update();
            }
        });
        // The volume, whoever changed it (here, the scroll wheel, MPRIS).
        let bar_weak = bar.downgrade();
        player::get().connect_volume_changed(move |volume| {
            if let Some(bar) = bar_weak.upgrade() {
                bar.show_volume(volume);
            }
        });
        bar.show_volume(player::get().volume());
        // A jump while paused: the timer is not running, and the player
        // still answers with the old position until the seek has landed.
        let bar_weak = bar.downgrade();
        player::get().connect_seeked(move |seconds| {
            if let Some(bar) = bar_weak.upgrade() {
                bar.show_position(seconds);
            }
        });
        bar
    }

    /// The slider and the icon of the volume button.
    fn show_volume(&self, volume: f64) {
        let imp = self.imp();
        if (imp.volume_scale.value() - volume).abs() > 1e-4 {
            imp.volume_scale.set_value(volume);
        }
        imp.volume_button.set_icon_name(match volume {
            v if v <= 0.0 => "audio-volume-muted-symbolic",
            v if v < 0.34 => "audio-volume-low-symbolic",
            v if v < 0.67 => "audio-volume-medium-symbolic",
            _ => "audio-volume-high-symbolic",
        });
    }

    fn update(&self) {
        let imp = self.imp();
        let player = player::get();
        let Some(song) = player.current() else {
            imp.revealer.set_reveal_child(false);
            self.stop_timer();
            return;
        };

        imp.title.set_label(&song.title);
        imp.artist.set_label(song.artist.as_deref().unwrap_or(""));
        imp.album.set_label(song.album.as_deref().unwrap_or(""));

        // A star for the song, made anew only when the song changes: a rebuild
        // while it waits for the server would lose that wait.
        if imp.star_song.borrow().as_deref() != Some(song.id.as_str()) {
            imp.star_song.replace(Some(song.id.clone()));
            let star = crate::favorites::star_button(
                song.starred.is_some(),
                player.client(),
                crate::favorites::Favorite::Song(song.id.clone()),
            );
            adw::prelude::BinExt::set_child(&imp.star_slot.get(), Some(&star));
        }
        imp.progress
            .adjustment()
            .set_upper(song.duration.unwrap_or(0).max(1) as f64);

        if player.is_playing() {
            imp.play_pause
                .set_icon_name("media-playback-pause-symbolic");
            imp.play_pause.set_tooltip_text(Some(&gettext("Pause")));
            self.start_timer();
        } else {
            imp.play_pause
                .set_icon_name("media-playback-start-symbolic");
            imp.play_pause.set_tooltip_text(Some(&gettext("Play")));
            self.stop_timer();
        }
        self.tick();

        // A new cover, and a new colour, only when the album changes. Tracks of
        // one album have their own cover ids on the server, so the album decides.
        let key = song.album_id.clone().or_else(|| song.cover_art.clone());
        // A hidden card waits for its cover before it slides in, so the
        // placeholder never shows first.
        let mut waiting = false;
        if *imp.cover_id.borrow() != key {
            imp.cover_id.replace(key.clone());
            match (song.cover_art.clone(), key) {
                // The old cover stays until the new one is ready: no blank flash.
                (Some(id), Some(key)) => {
                    waiting = !imp.revealer.reveals_child();
                    self.load_cover(player.client(), id, key);
                }
                _ => {
                    covers::show(&imp.cover, None);
                    if let Some(tint) = imp.tint.get() {
                        tint.clear();
                    }
                }
            }
        }
        if waiting {
            // A slow server must not hold the card back for long, though.
            let bar = self.downgrade();
            glib::timeout_add_local_once(Duration::from_millis(500), move || {
                if let Some(bar) = bar.upgrade()
                    && player::get().current().is_some()
                {
                    bar.imp().revealer.set_reveal_child(true);
                }
            });
        } else {
            imp.revealer.set_reveal_child(true);
        }
    }

    fn load_cover(&self, client: Option<subsonic::Client>, id: String, key: String) {
        covers::load_for(self, client, id, move |bar, texture| {
            let imp = bar.imp();
            if imp.cover_id.borrow().as_deref() != Some(key.as_str()) {
                return;
            }
            covers::show(&imp.cover, texture.as_ref());
            if let Some(tint) = imp.tint.get() {
                match texture.as_ref().and_then(tint::dominant_color) {
                    Some(color) => tint.set_solid(color),
                    None => tint.clear(),
                }
            }
            // The cover is in: a card that waited for it slides in now.
            imp.revealer.set_reveal_child(true);
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
        self.show_position(player::get().position_secs().unwrap_or(0.0));
    }

    fn show_position(&self, position: f64) {
        let imp = self.imp();
        // The user is dragging the slider: do not pull it back.
        if imp.seek_pending.borrow().is_some() {
            return;
        }
        imp.progress.set_value(position);
        let duration = imp.progress.adjustment().upper() as u64;
        imp.elapsed.set_label(&clock(position as u64));
        imp.total.set_label(&clock(duration));
    }
}

/// Seconds as "m:ss".
fn clock(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

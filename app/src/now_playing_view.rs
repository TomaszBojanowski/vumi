//! Full "now playing" view: big cover, title, artist, album, quality,
//! progress and controls. Built once, shown again every time it opens;
//! the progress refreshes only while it is open.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use std::cell::{OnceCell, RefCell};
use std::time::Duration;

use crate::album_page::quality_text;
use crate::{covers, player, tint};

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/now_playing_view.blp")]
    pub struct NowPlayingView {
        #[template_child]
        pub toolbar: gtk::TemplateChild<adw::ToolbarView>,
        #[template_child]
        pub cover: gtk::TemplateChild<gtk::Picture>,
        #[template_child]
        pub title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub artist: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub album: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub quality: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub progress: gtk::TemplateChild<gtk::Scale>,
        #[template_child]
        pub elapsed: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub total: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub previous: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub play_pause: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub next: gtk::TemplateChild<gtk::Button>,
        pub cover_id: RefCell<Option<String>>,
        pub timer: RefCell<Option<glib::SourceId>>,
        pub(crate) tint: OnceCell<tint::Dynamic>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for NowPlayingView {
        // Must match "template $MusePlayerNowPlayingView" in the Blueprint file.
        const NAME: &'static str = "MusePlayerNowPlayingView";
        type Type = super::NowPlayingView;
        type ParentType = adw::Dialog;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for NowPlayingView {}
    impl WidgetImpl for NowPlayingView {}
    impl AdwDialogImpl for NowPlayingView {}
}

glib::wrapper! {
    pub struct NowPlayingView(ObjectSubclass<imp::NowPlayingView>)
        @extends adw::Dialog, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl NowPlayingView {
    pub(crate) fn new() -> Self {
        let view: Self = glib::Object::new();
        let imp = view.imp();

        let tint = tint::Dynamic::new();
        imp.toolbar.add_css_class(tint.class());
        let _ = imp.tint.set(tint);

        imp.play_pause.connect_clicked(|_| player::get().toggle_pause());
        imp.previous.connect_clicked(|_| player::get().previous());
        imp.next.connect_clicked(|_| player::get().next());

        // Only a drag by the user seeks; updates from the timer do not.
        imp.progress.connect_change_value(|_, _, value| {
            player::get().seek(value);
            glib::Propagation::Proceed
        });

        let view_weak = view.downgrade();
        player::get().connect_changed(move || {
            if let Some(view) = view_weak.upgrade() {
                view.update();
            }
        });

        // The progress timer runs only while the view is open.
        view.connect_closed(|view| view.stop_timer());
        view
    }

    /// Shows the view over `parent`, up to date from the first frame.
    pub(crate) fn open(&self, parent: &impl IsA<gtk::Widget>) {
        self.update();
        self.start_timer();
        self.present(Some(parent));
    }

    fn update(&self) {
        let imp = self.imp();
        let player = player::get();
        let Some(song) = player.current() else {
            self.close();
            return;
        };

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

        let duration = song.duration.unwrap_or(0);
        imp.progress.adjustment().set_upper(duration.max(1) as f64);
        imp.total.set_label(&clock(duration as u64));

        if player.is_playing() {
            imp.play_pause.set_icon_name("media-playback-pause-symbolic");
            imp.play_pause.set_tooltip_text(Some(&gettext("Pause")));
        } else {
            imp.play_pause.set_icon_name("media-playback-start-symbolic");
            imp.play_pause.set_tooltip_text(Some(&gettext("Play")));
        }
        self.tick();

        // A new cover, and a new background colour, only when the album changes.
        if *imp.cover_id.borrow() != song.cover_art {
            imp.cover_id.replace(song.cover_art.clone());
            imp.cover.set_paintable(None::<&gtk::gdk::Paintable>);
            match song.cover_art.clone() {
                Some(id) => self.load_cover(player.client(), id),
                None => {
                    if let Some(tint) = imp.tint.get() {
                        tint.clear();
                    }
                }
            }
        }
    }

    fn load_cover(&self, client: Option<subsonic::Client>, id: String) {
        let view = self.downgrade();
        glib::spawn_future_local(async move {
            let texture = covers::load(client, id.clone()).await;
            let Some(view) = view.upgrade() else {
                return;
            };
            let imp = view.imp();
            if imp.cover_id.borrow().as_deref() != Some(id.as_str()) {
                return;
            }
            imp.cover.set_paintable(texture.as_ref());
            if let Some(tint) = imp.tint.get() {
                match texture.as_ref().and_then(tint::dominant_color) {
                    Some(color) => tint.set_gradient(color),
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
        let view = self.downgrade();
        let id = glib::timeout_add_local(Duration::from_millis(500), move || match view.upgrade() {
            Some(view) => {
                view.tick();
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

    fn tick(&self) {
        let imp = self.imp();
        let position = player::get().position_secs().unwrap_or(0.0);
        imp.progress.set_value(position);
        imp.elapsed.set_label(&clock(position as u64));
    }
}

/// Seconds as "m:ss".
fn clock(seconds: u64) -> String {
    format!("{}:{:02}", seconds / 60, seconds % 60)
}

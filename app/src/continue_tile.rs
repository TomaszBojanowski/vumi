//! One album of "Recently Played" on the Start page (continue_tile.blp): its
//! cover with how far through it playback is, its name and artist, and a
//! round button that resumes it, or pauses it while it plays.

use adw::subclass::prelude::*;
use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};

use crate::cover_image::CoverImage;
use crate::covers;

/// Where the album of a tile is.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TileState {
    /// Not in the player: the button resumes it where it stopped.
    Saved,
    /// In the player and playing: the button pauses it.
    Playing,
    /// In the player and paused: the button resumes it.
    Paused,
}

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/continue_tile.blp")]
    pub struct ContinueTile {
        #[template_child]
        pub tile_button: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub cover: gtk::TemplateChild<CoverImage>,
        #[template_child]
        pub progress: gtk::TemplateChild<gtk::ProgressBar>,
        #[template_child]
        pub now_label: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub title: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub artist: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub play_button: gtk::TemplateChild<gtk::Button>,
        /// The cover's id, and whether it is on screen (to fetch it again once
        /// the app is connected).
        pub cover_id: RefCell<Option<String>>,
        pub cover_shown: Cell<bool>,
        /// The state shown, so an unchanged one is not set again.
        pub(crate) state: Cell<Option<TileState>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ContinueTile {
        // Must match "template $VumiContinueTile" in the Blueprint file.
        const NAME: &'static str = "VumiContinueTile";
        type Type = super::ContinueTile;
        type ParentType = adw::Bin;

        fn class_init(klass: &mut Self::Class) {
            CoverImage::static_type();
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ContinueTile {}
    impl WidgetImpl for ContinueTile {}
    impl BinImpl for ContinueTile {}
}

glib::wrapper! {
    pub struct ContinueTile(ObjectSubclass<imp::ContinueTile>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ContinueTile {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    /// The album's name, artist and cover.
    pub(crate) fn set_album(&self, album: &subsonic::Album, client: Option<subsonic::Client>) {
        let imp = self.imp();
        imp.title.set_label(&album.name);
        let artist = album.artist.as_deref().unwrap_or_default();
        imp.artist.set_label(artist);
        // Read out as "title, artist", like the rows of songs.
        let name = if artist.is_empty() {
            album.name.clone()
        } else {
            format!("{}, {artist}", album.name)
        };
        imp.tile_button
            .update_property(&[gtk::accessible::Property::Label(&name)]);
        imp.cover_id.replace(album.cover_art.clone());
        if let Some(id) = album.cover_art.clone() {
            self.load_cover(client, id);
        }
    }

    /// Called once the app is connected: fetch the cover if it is not there.
    pub(crate) fn retry_cover(&self, client: subsonic::Client) {
        let imp = self.imp();
        if imp.cover_shown.get() {
            return;
        }
        let id = imp.cover_id.borrow().clone();
        if let Some(id) = id {
            self.load_cover(Some(client), id);
        }
    }

    fn load_cover(&self, client: Option<subsonic::Client>, id: String) {
        covers::load_for(self, client, id, |tile, texture| {
            tile.imp().cover.set_texture(texture.as_ref());
            tile.imp().cover_shown.set(texture.is_some());
        });
    }

    /// "Now Playing" and the button, after where the album is.
    pub(crate) fn set_state(&self, state: TileState) {
        let imp = self.imp();
        if imp.state.replace(Some(state)) == Some(state) {
            return;
        }
        imp.now_label.set_visible(state != TileState::Saved);
        let (icon, tip) = match state {
            TileState::Playing => ("media-playback-pause-symbolic", gettext("Pause")),
            TileState::Saved | TileState::Paused => {
                ("media-playback-start-symbolic", gettext("Resume"))
            }
        };
        imp.play_button.set_icon_name(icon);
        imp.play_button.set_tooltip_text(Some(&tip));
        imp.play_button
            .update_property(&[gtk::accessible::Property::Label(&tip)]);
    }

    /// How far through the album playback is (0 to 1); no line when unknown.
    pub(crate) fn set_progress(&self, fraction: Option<f64>) {
        let imp = self.imp();
        imp.progress.set_visible(fraction.is_some());
        imp.progress.set_fraction(fraction.unwrap_or(0.0));
    }

    /// Calls `f` when the tile is clicked (it opens the album).
    pub(crate) fn connect_open(&self, f: impl Fn(&gtk::Button) + 'static) {
        self.imp().tile_button.connect_clicked(f);
    }

    /// Calls `f` when the round button is clicked.
    pub(crate) fn connect_play(&self, f: impl Fn() + 'static) {
        self.imp().play_button.connect_clicked(move |_| f());
    }
}

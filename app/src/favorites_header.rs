//! The top of the Favorites page: a collage of the starred albums' covers,
//! the name of the page, a line that says how much it holds, and buttons
//! that play the starred songs, in order or shuffled.

use adw::subclass::prelude::*;
use gettextrs::ngettext;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use crate::cover_image::CoverImage;
use crate::{covers, player};

/// The side of the collage: next to the text, and in a narrow window.
const COLLAGE: i32 = 200;
const COLLAGE_COMPACT: i32 = 120;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/favorites_header.blp")]
    pub struct FavoritesHeader {
        #[template_child]
        pub top_row: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub text: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub buttons: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub collage_frame: gtk::TemplateChild<gtk::Overlay>,
        #[template_child]
        pub collage: gtk::TemplateChild<gtk::Grid>,
        #[template_child]
        pub summary: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub play_button: gtk::TemplateChild<gtk::Button>,
        #[template_child]
        pub shuffle_button: gtk::TemplateChild<gtk::Button>,
        /// The covers in the collage now: the same ones are not loaded again.
        pub covers: RefCell<Vec<String>>,
        /// The starred songs, in the order of the list, and the client to
        /// play them with.
        pub songs: RefCell<Option<Rc<[subsonic::Song]>>>,
        pub client: RefCell<Option<subsonic::Client>>,
        pub compact: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for FavoritesHeader {
        // Must match "template $VumiFavoritesHeader" in the Blueprint file.
        const NAME: &'static str = "VumiFavoritesHeader";
        type Type = super::FavoritesHeader;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            klass.bind_template();
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for FavoritesHeader {
        fn constructed(&self) {
            self.parent_constructed();
            let header = self.obj().downgrade();
            self.play_button.connect_clicked(move |_| {
                if let Some(header) = header.upgrade() {
                    header.play(false);
                }
            });
            let header = self.obj().downgrade();
            self.shuffle_button.connect_clicked(move |_| {
                if let Some(header) = header.upgrade() {
                    header.play(true);
                }
            });
        }
    }
    impl WidgetImpl for FavoritesHeader {}
    impl BoxImpl for FavoritesHeader {}
}

glib::wrapper! {
    pub struct FavoritesHeader(ObjectSubclass<imp::FavoritesHeader>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget, gtk::Orientable;
}

impl FavoritesHeader {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    /// Shows what the Favorites hold: the starred albums (for the collage
    /// and the count) and the starred songs (the count, the length, and what
    /// the buttons play).
    pub(crate) fn set_content(
        &self,
        albums: &[Arc<subsonic::Album>],
        songs: &[subsonic::Song],
        client: Option<subsonic::Client>,
    ) {
        let imp = self.imp();

        let mut parts = Vec::new();
        if !albums.is_empty() {
            let n = albums.len() as u32;
            parts.push(ngettext("{n} album", "{n} albums", n).replace("{n}", &n.to_string()));
        }
        if !songs.is_empty() {
            let n = songs.len() as u32;
            // Translators: {n} is a number of tracks.
            parts.push(ngettext("{n} track", "{n} tracks", n).replace("{n}", &n.to_string()));
            let seconds: u32 = songs.iter().filter_map(|song| song.duration).sum();
            if seconds > 0 {
                parts.push(crate::album_page::format_duration(seconds));
            }
        }
        imp.summary.set_label(&parts.join(" · "));

        // The buttons play the songs: without any, there is nothing to play.
        let playable = !songs.is_empty() && client.is_some();
        imp.play_button.set_sensitive(playable);
        imp.shuffle_button.set_sensitive(playable);
        imp.songs.replace(Some(songs.into()));
        imp.client.replace(client.clone());

        // The covers of the starred albums, then of the albums of the starred
        // songs, each once: four make a collage, fewer show the first alone.
        let mut ids: Vec<String> = Vec::new();
        let album_covers = albums.iter().filter_map(|album| album.cover_art.clone());
        let song_covers = songs.iter().filter_map(|song| song.cover_art.clone());
        for id in album_covers.chain(song_covers) {
            if !ids.contains(&id) {
                ids.push(id);
            }
            if ids.len() == 4 {
                break;
            }
        }
        if ids.len() < 4 {
            ids.truncate(1);
        }
        if *imp.covers.borrow() == ids {
            return;
        }
        while let Some(child) = imp.collage.first_child() {
            imp.collage.remove(&child);
        }
        for (index, id) in ids.iter().enumerate() {
            let cover = CoverImage::new();
            let (column, row, span) = if ids.len() == 1 {
                (0, 0, 2)
            } else {
                (index as i32 % 2, index as i32 / 2, 1)
            };
            imp.collage.attach(&cover, column, row, span, span);
            covers::load_for(&cover, client.clone(), id.clone(), |cover, texture| {
                cover.set_texture(texture.as_ref());
            });
        }
        imp.covers.replace(ids);
    }

    /// A narrow window: the collage is smaller, to leave the text its room,
    /// and the buttons go under the collage and the text.
    pub(crate) fn set_compact(&self, compact: bool) {
        let imp = self.imp();
        if imp.compact.replace(compact) == compact {
            return;
        }
        let side = if compact { COLLAGE_COMPACT } else { COLLAGE };
        imp.collage_frame.set_size_request(side, side);
        imp.top_row.set_spacing(if compact { 18 } else { 24 });
        let buttons = imp.buttons.get();
        if compact {
            imp.text.remove(&buttons);
            buttons.set_margin_top(0);
            self.append(&buttons);
        } else {
            self.remove(&buttons);
            buttons.set_margin_top(6);
            imp.text.append(&buttons);
        }
    }

    fn play(&self, shuffle: bool) {
        let imp = self.imp();
        let (Some(songs), Some(client)) = (imp.songs.borrow().clone(), imp.client.borrow().clone())
        else {
            return;
        };
        if songs.is_empty() {
            return;
        }
        if shuffle {
            // A new order needs its own copy, made only when Shuffle is pressed.
            let mut shuffled = songs.to_vec();
            fastrand::shuffle(&mut shuffled);
            player::get().play(client, shuffled.into(), 0);
        } else {
            player::get().play(client, songs, 0);
        }
    }
}

//! One artist in the Artists grid: the picture or the initials, the name and
//! the number of albums. The grid reuses cards while scrolling, so
//! set_artist() is called many times on the same card.
//!
//! A card holds only what it shows: the avatar is ours (artist_avatar.rs),
//! the letter is made only on the first card of a letter, and the room of a
//! two-line name is a height, not an invisible copy of the labels.

use adw::subclass::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use std::cell::RefCell;

use crate::artist_avatar::ArtistAvatar;
use crate::covers;

/// Between the name and the number of albums (artist_card.blp).
const SPACING: i32 = 6;

mod imp {
    use super::*;

    #[derive(Default, gtk::CompositeTemplate)]
    #[template(file = "src/artist_card.blp")]
    pub struct ArtistCard {
        #[template_child]
        pub overlay: gtk::TemplateChild<gtk::Overlay>,
        #[template_child]
        pub avatar: gtk::TemplateChild<ArtistAvatar>,
        #[template_child]
        pub names: gtk::TemplateChild<gtk::Box>,
        #[template_child]
        pub name: gtk::TemplateChild<gtk::Label>,
        #[template_child]
        pub albums: gtk::TemplateChild<gtk::Label>,
        /// Where a letter begins (made on the first card that shows one).
        pub letter: RefCell<Option<gtk::Label>>,
        pub picture_id: RefCell<Option<String>>,
        /// Text size changes in the settings, to measure the names again.
        pub settings: RefCell<Vec<glib::SignalHandlerId>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ArtistCard {
        const NAME: &'static str = "VumiArtistCard";
        type Type = super::ArtistCard;
        type ParentType = gtk::Box;

        fn class_init(klass: &mut Self::Class) {
            // The template uses it.
            ArtistAvatar::static_type();
            klass.bind_template();
            // A group with a name ("Adele, 5 albums"): the grid's cell is read
            // by that name; the avatar and the letter only decorate.
            klass.set_accessible_role(gtk::AccessibleRole::Group);
        }

        fn instance_init(obj: &glib::subclass::InitializingObject<Self>) {
            obj.init_template();
        }
    }

    impl ObjectImpl for ArtistCard {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            // The labels have their fonts only in a window, and these change
            // with the text size: the room for the name follows both.
            obj.connect_root_notify(|card| card.fit_names());
            let settings = obj.settings();
            let mut handlers = self.settings.borrow_mut();
            for property in ["gtk-xft-dpi", "gtk-font-name"] {
                let card = obj.downgrade();
                handlers.push(settings.connect_notify_local(Some(property), move |_, _| {
                    if let Some(card) = card.upgrade() {
                        card.fit_names();
                    }
                }));
            }
        }

        fn dispose(&self) {
            let settings = self.obj().settings();
            for handler in self.settings.take() {
                settings.disconnect(handler);
            }
        }
    }

    impl WidgetImpl for ArtistCard {}
    impl BoxImpl for ArtistCard {}
}

glib::wrapper! {
    pub struct ArtistCard(ObjectSubclass<imp::ArtistCard>)
        @extends gtk::Box, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl ArtistCard {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    pub(crate) fn set_artist(
        &self,
        name: &str,
        albums: u32,
        picture: Option<String>,
        letter: Option<&str>,
        client: Option<subsonic::Client>,
    ) {
        let imp = self.imp();
        self.set_letter(letter);
        imp.avatar.set_text(name);
        imp.name.set_label(name);
        let count = crate::format::albums_text(albums);
        imp.albums.set_label(&count);
        self.update_property(&[gtk::accessible::Property::Label(&crate::a11y::join([
            name,
            count.as_str(),
        ]))]);
        self.fit_names();

        imp.avatar.set_picture(None);
        imp.picture_id.replace(picture.clone());
        if let (Some(id), Some(client)) = (picture, client) {
            covers::load_for(self, Some(client), id.clone(), move |card, texture| {
                let imp = card.imp();
                if imp.picture_id.borrow().as_deref() == Some(id.as_str()) {
                    imp.avatar.set_picture(
                        texture
                            .as_ref()
                            .map(|t| t.upcast_ref::<gtk::gdk::Paintable>()),
                    );
                }
            });
        }
    }

    pub(crate) fn clear(&self) {
        let imp = self.imp();
        imp.avatar.set_picture(None);
        imp.picture_id.replace(None);
    }

    /// On the first card of a letter: the letter, over the avatar's corner.
    fn set_letter(&self, letter: Option<&str>) {
        let imp = self.imp();
        let Some(letter) = letter else {
            if let Some(label) = imp.letter.borrow().as_ref() {
                label.set_visible(false);
            }
            return;
        };
        let label = imp
            .letter
            .borrow_mut()
            .get_or_insert_with(|| {
                // A mark for the eye; the letter bar says the letters.
                let label = gtk::Label::builder()
                    .halign(gtk::Align::Start)
                    .valign(gtk::Align::Start)
                    .css_classes(["title-3"])
                    .accessible_role(gtk::AccessibleRole::Presentation)
                    .build();
                imp.overlay.add_overlay(&label);
                label
            })
            .clone();
        label.set_label(letter);
        label.set_visible(true);
    }

    /// The names' box as tall as a two-line name and the number of albums,
    /// whatever the name: every card as tall, so every row of the grid is
    /// (main_view.rs counts on it to scroll to a letter).
    fn fit_names(&self) {
        let imp = self.imp();
        let (_, two_lines) = imp.name.create_pango_layout(Some("\n")).pixel_size();
        let (_, albums, _, _) = imp.albums.measure(gtk::Orientation::Vertical, -1);
        let height = two_lines + SPACING + albums;
        if imp.names.height_request() != height {
            imp.names.set_height_request(height);
        }
    }
}

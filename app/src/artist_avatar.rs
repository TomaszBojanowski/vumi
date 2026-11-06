//! An artist's round picture, or the initials of the name on the colour
//! libadwaita gives that name: AdwAvatar's look (its "avatar" style node, the
//! same letters, colour, size and place), drawn by a widget of our own.
//!
//! Why not AdwAvatar's own initials: it fits their font size to every name,
//! so nearly every card on the Artists grid asked for a font size of its own,
//! a new font instance with its glyphs drawn anew. Measured on 2026-11-05 (a
//! library of 224 artists, the grid's first show, medians of 5): 71.8 ms with
//! AdwAvatar's initials, 38.7 ms with one size for all. Here the size follows
//! AdwAvatar's rule, rounded to whole steps of STEP pixels, so a few sizes
//! serve every name. Measure before going back to AdwAvatar.
//!
//! The label of the initials is made only for an artist without a picture,
//! the image only for one with a picture.

use adw::subclass::prelude::*;
use gtk::prelude::*;
use gtk::{gdk, glib, graphene, pango};
use std::cell::{Cell, RefCell};

/// The avatars' size, as AdwAvatar's "size".
pub(crate) const SIZE: i32 = 120;

/// The initials' font sizes come in steps of this many pixels.
const STEP: f64 = 4.0;

/// libadwaita's avatar colours: color1 to color14 (its NUMBER_OF_COLORS).
const COLORS: u32 = 14;

/// The diagonal of a square over its side, as AdwAvatar writes it: its value,
/// not SQRT_2, so the sizes come out as AdwAvatar's.
#[allow(clippy::approx_constant)]
const DIAGONAL: f64 = 1.4142;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct ArtistAvatar {
        pub text: RefCell<String>,
        /// The colour class, 1 to COLORS (0: none yet).
        pub color: Cell<u32>,
        pub label: RefCell<Option<gtk::Label>>,
        pub image: RefCell<Option<gtk::Image>>,
        pub picture: RefCell<Option<gdk::Paintable>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for ArtistAvatar {
        const NAME: &'static str = "VumiArtistAvatar";
        type Type = super::ArtistAvatar;
        type ParentType = gtk::Widget;

        fn class_init(klass: &mut Self::Class) {
            // libadwaita's style for avatars (round, bold, the colours).
            klass.set_css_name("avatar");
            klass.set_accessible_role(gtk::AccessibleRole::Img);
            klass.set_layout_manager_type::<gtk::BinLayout>();
        }
    }

    impl ObjectImpl for ArtistAvatar {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.set_size_request(SIZE, SIZE);
            obj.set_halign(gtk::Align::Center);
            obj.set_valign(gtk::Align::Center);
            obj.set_overflow(gtk::Overflow::Hidden);
            // As AdwAvatar: the initials are fitted again once the avatar is
            // in a window (only there the label has its font), the picture
            // drawn again for a surface of another scale.
            obj.connect_root_notify(|avatar| avatar.fit_initials());
            obj.connect_scale_factor_notify(|avatar| avatar.update_picture());
        }

        fn dispose(&self) {
            if let Some(label) = self.label.take() {
                label.unparent();
            }
            if let Some(image) = self.image.take() {
                image.unparent();
            }
        }
    }

    impl WidgetImpl for ArtistAvatar {}
}

glib::wrapper! {
    pub struct ArtistAvatar(ObjectSubclass<imp::ArtistAvatar>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for ArtistAvatar {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl ArtistAvatar {
    /// The artist's name: the colour, the initials and what a screen reader
    /// says ("Avatar of …", AdwAvatar's words, translated as AdwAvatar has
    /// them: GLib's dgettext, which leaves them in English where the app
    /// itself has no translation).
    pub(crate) fn set_text(&self, text: &str) {
        let imp = self.imp();
        if *imp.text.borrow() == text && imp.color.get() != 0 {
            return;
        }
        imp.text.replace(text.to_owned());
        let color = color_class(text);
        if imp.color.get() != color {
            if imp.color.get() != 0 {
                self.remove_css_class(&format!("color{}", imp.color.get()));
            }
            self.add_css_class(&format!("color{color}"));
            imp.color.set(color);
        }
        if text.is_empty() {
            self.reset_property(gtk::AccessibleProperty::Label);
        } else {
            let label = glib::dgettext(Some("libadwaita"), "Avatar of %s").replace("%s", text);
            self.update_property(&[gtk::accessible::Property::Label(&label)]);
        }
        self.update_visibility();
    }

    /// The artist's picture; None: the initials.
    pub(crate) fn set_picture(&self, picture: Option<&gdk::Paintable>) {
        let imp = self.imp();
        imp.picture.replace(picture.cloned());
        if picture.is_some() {
            self.add_css_class("image");
        } else {
            self.remove_css_class("image");
        }
        self.update_picture();
        self.update_visibility();
    }

    /// The picture or the initials, whichever the artist has; each part is
    /// made the first time it is needed.
    fn update_visibility(&self) {
        let imp = self.imp();
        let picture = imp.picture.borrow().is_some();
        if let Some(image) = imp.image.borrow().as_ref() {
            image.set_visible(picture);
        }
        let initials = initials(&imp.text.borrow());
        if picture || initials.is_empty() {
            if let Some(label) = imp.label.borrow().as_ref() {
                label.set_visible(false);
            }
            return;
        }
        let label = imp
            .label
            .borrow_mut()
            .get_or_insert_with(|| {
                let label = gtk::Label::builder()
                    .accessible_role(gtk::AccessibleRole::Presentation)
                    .build();
                label.set_parent(self);
                label
            })
            .clone();
        label.set_label(&initials);
        label.set_visible(true);
        self.fit_initials();
    }

    /// AdwAvatar's rule for the initials' size (update_font_size), rounded
    /// to a step: the text at its own size measured, then made to fill the
    /// square inside the circle, less a padding that grows with the size.
    fn fit_initials(&self) {
        let Some(label) = self.imp().label.borrow().clone() else {
            return;
        };
        if !label.is_visible() {
            return;
        }
        label.set_attributes(Some(&pango::AttrList::new()));
        let (width, height) = label.layout().pixel_size();
        if width <= 0 || height <= 0 {
            return;
        }
        let size = initials_size(width, height, SIZE);
        let attributes = pango::AttrList::new();
        attributes.insert(pango::AttrSize::new_size_absolute(
            (size * pango::SCALE as f64) as i32,
        ));
        label.set_attributes(Some(&attributes));
    }

    /// The picture as AdwAvatar draws it (update_custom_image_snapshot):
    /// filling the square, centred, at the pixels of the screen; a texture
    /// with AdwAvatar's scaling filter (trilinear down, nearest up).
    fn update_picture(&self) {
        let imp = self.imp();
        let Some(source) = imp.picture.borrow().clone() else {
            if let Some(image) = imp.image.borrow().as_ref() {
                image.set_paintable(None::<&gdk::Paintable>);
            }
            return;
        };
        let image = imp
            .image
            .borrow_mut()
            .get_or_insert_with(|| {
                let image = gtk::Image::builder()
                    .accessible_role(gtk::AccessibleRole::Presentation)
                    .pixel_size(SIZE)
                    .build();
                image.set_parent(self);
                image
            })
            .clone();
        let (width, height) = (source.intrinsic_width(), source.intrinsic_height());
        let texture = source.downcast_ref::<gdk::Texture>();
        // A square that is not a texture is shown as it is.
        if width == height && texture.is_none() {
            image.set_paintable(Some(&source));
            return;
        }
        if width <= 0 || height <= 0 {
            image.set_paintable(None::<&gdk::Paintable>);
            return;
        }
        let size = (SIZE * self.scale_factor()) as f32;
        let (scaled_width, scaled_height) = if width > height {
            (width as f32 * size / height as f32, size)
        } else if width < height {
            (size, height as f32 * size / width as f32)
        } else {
            (size, size)
        };
        let snapshot = gtk::Snapshot::new();
        snapshot.translate(&graphene::Point::new(
            (size - scaled_width) / 2.0,
            (size - scaled_height) / 2.0,
        ));
        match texture {
            Some(texture) => {
                let filter = if scaled_width > width as f32 || scaled_height > height as f32 {
                    gtk::gsk::ScalingFilter::Nearest
                } else {
                    gtk::gsk::ScalingFilter::Trilinear
                };
                snapshot.append_scaled_texture(
                    texture,
                    filter,
                    &graphene::Rect::new(0.0, 0.0, scaled_width, scaled_height),
                );
            }
            None => source.snapshot(&snapshot, scaled_width as f64, scaled_height as f64),
        }
        let square = snapshot.to_paintable(Some(&graphene::Size::new(size, size)));
        image.set_paintable(square.as_ref());
    }
}

/// The initials of `text` as AdwAvatar takes them (extract_initials_from_text):
/// upper case, the first character, and the first after the last space.
pub(crate) fn initials(text: &str) -> String {
    let upper = text.to_uppercase();
    // g_strstrip: ASCII white space only.
    let stripped = upper.trim_matches(|c: char| c.is_ascii_whitespace() || c == '\u{b}');
    let normalized = glib::normalize(stripped, glib::NormalizeMode::DefaultCompose);
    let mut initials = String::new();
    if let Some(first) = normalized.chars().next() {
        initials.push(first);
    }
    if let Some(space) = normalized.rfind(' ')
        && let Some(next) = normalized[space + 1..].chars().next()
    {
        initials.push(next);
    }
    initials
}

/// AdwAvatar's colour of a name: color1 to color14 by GLib's string hash.
pub(crate) fn color_class(text: &str) -> u32 {
    str_hash(text) % COLORS + 1
}

/// GLib's g_str_hash: djb2 over the bytes as signed chars.
fn str_hash(text: &str) -> u32 {
    text.bytes().fold(5381u32, |hash, byte| {
        hash.wrapping_mul(33).wrapping_add(byte as i8 as i32 as u32)
    })
}

/// The initials' font size in pixels for a text `width` x `height` pixels at
/// its own size, in an avatar `size` pixels wide: AdwAvatar's size, rounded
/// to a whole step, and never past the largest it allows.
pub(crate) fn initials_size(width: i32, height: i32, size: i32) -> f64 {
    let square = size as f64 / DIAGONAL;
    let padding = (size as f64 * 0.4 - 5.0).max(0.0);
    let largest = square - padding;
    let fitted = (height as f64 * (largest / width as f64)).clamp(0.0, largest);
    if fitted >= largest {
        return largest;
    }
    ((fitted / STEP).round() * STEP).clamp(STEP, largest)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_initials_are_adwaitas() {
        assert_eq!(initials("The Beatles"), "TB");
        assert_eq!(initials("adele"), "A");
        assert_eq!(initials("10cc"), "1");
        assert_eq!(initials("  Борис Гребенщиков "), "БГ");
        assert_eq!(initials("坂本龍一"), "坂");
        assert_eq!(initials("😀 Smile"), "😀S");
        assert_eq!(initials("x"), "X");
        assert_eq!(initials("AC/DC"), "A");
        assert_eq!(initials("Trailing "), "T");
        assert_eq!(initials(""), "");
    }

    #[test]
    fn the_colour_is_glibs_hash_of_the_name() {
        // g_str_hash as GLib computes it (bytes as signed chars).
        assert_eq!(str_hash(""), 5381);
        assert_eq!(str_hash("X"), 177661);
        assert_eq!(str_hash("The Beatles"), 2595018534);
        assert_eq!(str_hash("Кино"), 243086034);
        assert_eq!(str_hash("坂本龍一"), 65227623);
        assert_eq!(color_class("The Beatles"), 11);
        assert_eq!(color_class("😀 Smile"), 1);
        assert_eq!(color_class("Кино"), 3);
    }

    #[test]
    fn the_size_follows_adwaitas_rule_in_whole_steps() {
        // At 120 px the largest is 120 / 1.4142 - (48 - 5) = 41.85 px.
        let largest = 120.0 / DIAGONAL - 43.0;
        // One narrow letter: as large as it may be, exactly.
        assert_eq!(initials_size(10, 18, 120), largest);
        // Two letters: AdwAvatar's 18 * 41.85 / 22 = 34.2 px, here 36 px.
        assert_eq!(initials_size(22, 18, 120), 36.0);
        // Wide ones: 18 * 41.85 / 30 = 25.1 px, here 24 px.
        assert_eq!(initials_size(30, 18, 120), 24.0);
        // Never more than two pixels from AdwAvatar's.
        for width in 11..60 {
            let exact = (18.0 * largest / width as f64).min(largest);
            assert!((initials_size(width, 18, 120) - exact).abs() <= 2.0);
        }
    }
}

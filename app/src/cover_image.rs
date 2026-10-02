//! Two small widgets for covers.
//!
//! `CoverImage` draws a cover brought down to the very pixels of the screen:
//! the size of the widget times the scale of its surface, also a fractional
//! one. GTK's own downscaling is soft where the picture is much smaller than
//! its source (it goes through mipmaps), which showed in the small playlist
//! covers. The cover is shown whole and centred, never cropped; what a cover
//! that is not square leaves of the frame is a neutral dark.
//!
//! `Square` is a square frame: of its child's own size, or (set_grows) as
//! wide as it is given and as tall as that.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gtk::{gdk, glib, graphene};
use std::cell::{Cell, RefCell};

/// Behind a cover that is not square: the same in the light and dark style
/// (style.css has it for pictures: picture.letterbox).
const BARS: gdk::RGBA = gdk::RGBA::new(0.141, 0.141, 0.141, 1.0);

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct CoverImage {
        pub texture: RefCell<Option<gdk::Texture>>,
        /// The cover at the size it has on screen, in device pixels, kept
        /// until that size or the cover changes.
        pub scaled: RefCell<Option<gdk::Texture>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for CoverImage {
        const NAME: &'static str = "VumiCoverImage";
        type Type = super::CoverImage;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for CoverImage {}

    impl WidgetImpl for CoverImage {
        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            let widget = self.obj();
            let Some(texture) = self.texture.borrow().clone() else {
                return;
            };
            let (width, height) = (widget.width() as f32, widget.height() as f32);
            let (source_width, source_height) = (texture.width(), texture.height());
            if width <= 0.0 || height <= 0.0 || source_width <= 0 || source_height <= 0 {
                return;
            }
            if source_width != source_height {
                snapshot.append_color(&BARS, &graphene::Rect::new(0.0, 0.0, width, height));
            }

            let native = widget.native();
            let scale = native
                .as_ref()
                .and_then(|native| native.surface())
                .map_or(1.0, |surface| surface.scale()) as f32;
            // Whole and centred, in device pixels.
            let fit = (width / source_width as f32).min(height / source_height as f32) * scale;
            let device_width = ((source_width as f32 * fit).round() as i32).max(1);
            let device_height = ((source_height as f32 * fit).round() as i32).max(1);

            let shown = if device_width < source_width {
                let mut scaled = self.scaled.borrow_mut();
                match scaled.as_ref() {
                    Some(have)
                        if have.width() == device_width && have.height() == device_height =>
                    {
                        have.clone()
                    }
                    _ => {
                        let new = downscale(&texture, device_width, device_height);
                        scaled.replace(new.clone());
                        new
                    }
                }
            } else {
                // The screen has room for every pixel of the cover.
                texture
            };

            // One pixel of the picture on one pixel of the screen: the corner
            // goes onto a whole device pixel, or the picture would be drawn
            // between pixels and blurred again.
            let x = (width - device_width as f32 / scale) / 2.0;
            let y = (height - device_height as f32 / scale) / 2.0;
            let origin = native
                .and_then(|native| widget.compute_point(&native, &graphene::Point::new(x, y)))
                .map_or((0.0, 0.0), |point| (point.x() * scale, point.y() * scale));
            let x = x + (origin.0.round() - origin.0) / scale;
            let y = y + (origin.1.round() - origin.1) / scale;
            snapshot.append_texture(
                &shown,
                &graphene::Rect::new(
                    x,
                    y,
                    device_width as f32 / scale,
                    device_height as f32 / scale,
                ),
            );
        }
    }

    #[derive(Default)]
    pub struct Square {
        /// As wide as the place it gets (the album grid stretches its cards),
        /// not of the child's own size.
        pub grows: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for Square {
        // Must match "$VumiSquare" in the Blueprint files.
        const NAME: &'static str = "VumiSquare";
        type Type = super::Square;
        type ParentType = adw::Bin;
    }

    impl ObjectImpl for Square {
        fn constructed(&self) {
            self.parent_constructed();
            // Measured and laid out below.
            self.obj().set_layout_manager(None::<gtk::LayoutManager>);
        }
    }

    impl WidgetImpl for Square {
        fn request_mode(&self) -> gtk::SizeRequestMode {
            if self.grows.get() {
                gtk::SizeRequestMode::HeightForWidth
            } else {
                gtk::SizeRequestMode::ConstantSize
            }
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some(child) = self.obj().child() else {
                return (0, 0, -1, -1);
            };
            let (minimum, natural, _, _) = child.measure(gtk::Orientation::Horizontal, -1);
            match orientation {
                gtk::Orientation::Vertical if self.grows.get() && for_size >= 0 => {
                    // As tall as the width it gets.
                    let side = for_size.max(minimum);
                    (side, side, -1, -1)
                }
                _ => (minimum, natural, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            if let Some(child) = self.obj().child() {
                // A square in any case: where the frame got more room one way
                // (a card in a wide cell), the child stays at the start.
                let side = width.min(height);
                let x = if self.obj().direction() == gtk::TextDirection::Rtl {
                    width - side
                } else {
                    0
                };
                let transform = gtk::gsk::Transform::new()
                    .translate(&graphene::Point::new(x as f32, 0.0));
                child.allocate(side, side, baseline, Some(transform));
            }
        }
    }

    impl BinImpl for Square {}
}

glib::wrapper! {
    pub struct CoverImage(ObjectSubclass<imp::CoverImage>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

glib::wrapper! {
    pub struct Square(ObjectSubclass<imp::Square>)
        @extends adw::Bin, gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Square {
    /// Whether the square takes the width it is given (and that height),
    /// instead of its child's own size.
    pub(crate) fn set_grows(&self, grows: bool) {
        self.imp().grows.set(grows);
        self.queue_resize();
    }
}

impl CoverImage {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    pub(crate) fn set_texture(&self, texture: Option<&gdk::Texture>) {
        let imp = self.imp();
        imp.texture.replace(texture.cloned());
        imp.scaled.replace(None);
        self.queue_draw();
    }
}

/// `texture` brought down to `width` x `height` pixels with a Lanczos filter
/// (three lobes), the usual choice for making photos smaller: sharp, and
/// without the jagged edges of plain sampling.
fn downscale(texture: &gdk::Texture, width: i32, height: i32) -> gdk::Texture {
    let (source_width, source_height) = (texture.width() as usize, texture.height() as usize);
    let (width, height) = (width as usize, height as usize);
    let stride = source_width * 4;
    let mut source = vec![0u8; stride * source_height];
    // Premultiplied, so a see-through edge does not bleed its colour.
    texture.download(&mut source, stride);

    // Rows first, into fractions; then columns, back into bytes.
    let across = weights(source_width, width);
    let mut rows = vec![0f32; source_height * width * 4];
    for y in 0..source_height {
        let line = &source[y * stride..(y + 1) * stride];
        for (x, (first, weights)) in across.iter().enumerate() {
            let mut sum = [0f32; 4];
            for (offset, weight) in weights.iter().enumerate() {
                let pixel = &line[(first + offset) * 4..(first + offset) * 4 + 4];
                for channel in 0..4 {
                    sum[channel] += pixel[channel] as f32 * weight;
                }
            }
            rows[(y * width + x) * 4..(y * width + x) * 4 + 4].copy_from_slice(&sum);
        }
    }
    let down = weights(source_height, height);
    let mut pixels = vec![0u8; width * height * 4];
    for (y, (first, weights)) in down.iter().enumerate() {
        for x in 0..width {
            let mut sum = [0f32; 4];
            for (offset, weight) in weights.iter().enumerate() {
                let pixel = &rows[((first + offset) * width + x) * 4..((first + offset) * width + x) * 4 + 4];
                for channel in 0..4 {
                    sum[channel] += pixel[channel] * weight;
                }
            }
            for channel in 0..4 {
                pixels[(y * width + x) * 4 + channel] = sum[channel].round().clamp(0.0, 255.0) as u8;
            }
        }
    }

    // What Texture::download gives: Cairo's ARGB32, in the byte order of the
    // machine.
    let format = if cfg!(target_endian = "little") {
        gdk::MemoryFormat::B8g8r8a8Premultiplied
    } else {
        gdk::MemoryFormat::A8r8g8b8Premultiplied
    };
    gdk::MemoryTexture::new(
        width as i32,
        height as i32,
        format,
        &glib::Bytes::from_owned(pixels),
        width * 4,
    )
    .upcast()
}

/// For each of `to` pixels: the first of the `from` pixels it is made of and
/// how much each of them counts (the weights add up to 1).
fn weights(from: usize, to: usize) -> Vec<(usize, Vec<f32>)> {
    let ratio = from as f32 / to as f32;
    let reach = 3.0 * ratio;
    (0..to)
        .map(|index| {
            let centre = (index as f32 + 0.5) * ratio;
            let first = (centre - reach).floor().max(0.0) as usize;
            let last = ((centre + reach).ceil() as usize).min(from);
            let mut weights: Vec<f32> = (first..last)
                .map(|pixel| lanczos((pixel as f32 + 0.5 - centre) / ratio))
                .collect();
            let sum: f32 = weights.iter().sum();
            for weight in &mut weights {
                *weight /= sum;
            }
            (first, weights)
        })
        .collect()
}

fn lanczos(x: f32) -> f32 {
    if x == 0.0 {
        1.0
    } else if x.abs() >= 3.0 {
        0.0
    } else {
        let p = std::f32::consts::PI * x;
        3.0 * p.sin() * (p / 3.0).sin() / (p * p)
    }
}

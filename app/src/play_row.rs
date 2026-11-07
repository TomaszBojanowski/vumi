//! The Tuner's "‹ › Play This Year" (tuner.blp): the play button beside the
//! arrows when its whole name fits there, otherwise, under the year on a
//! narrow page, on a line of its own under them, as wide as the row. Decided
//! from the children's own sizes at every layout, so it follows the language,
//! the text size and the font by itself, and the row always asks for the
//! height of the layout it will take (a breakpoint bin cannot: it reports no
//! minimum height, and the scrolled Start page gives its children their
//! minimum).
//!
//! Two children, in this order: the arrows, then the play button.

use adw::subclass::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct PlayRow {
        /// Between the arrows and the button, across or down.
        pub spacing: Cell<i32>,
        /// Whether the button may go under the arrows (a narrow page).
        pub stacking: Cell<bool>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for PlayRow {
        // Must match "$VumiPlayRow" in the Blueprint files.
        const NAME: &'static str = "VumiPlayRow";
        type Type = super::PlayRow;
        type ParentType = gtk::Widget;
        type Interfaces = (gtk::Buildable,);
    }

    // The template adds the children here. No layout manager: with one, GTK
    // would measure and allocate through it instead of the methods below.
    impl BuildableImpl for PlayRow {
        fn add_child(&self, builder: &gtk::Builder, child: &glib::Object, type_: Option<&str>) {
            match child.downcast_ref::<gtk::Widget>() {
                Some(widget) => widget.set_parent(&*self.obj()),
                None => self.parent_add_child(builder, child, type_),
            }
        }
    }

    impl ObjectImpl for PlayRow {
        fn dispose(&self) {
            while let Some(child) = self.obj().first_child() {
                child.unparent();
            }
        }
    }

    impl WidgetImpl for PlayRow {
        // The height depends on the width (one line or two): without this GTK
        // asks for the height without the width and gets the one line's.
        fn request_mode(&self) -> gtk::SizeRequestMode {
            gtk::SizeRequestMode::HeightForWidth
        }

        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let Some((arrows, play)) = self.obj().children() else {
                return (0, 0, -1, -1);
            };
            let spacing = self.spacing.get();
            let (a_min, a_nat) = width(&arrows);
            let (p_min, p_nat) = width(&play);
            if orientation == gtk::Orientation::Horizontal {
                return if self.stacking.get() {
                    (a_min.max(p_min), a_nat + spacing + p_nat, -1, -1)
                } else {
                    (a_min + spacing + p_min, a_nat + spacing + p_nat, -1, -1)
                };
            }
            let stacked = self.stacks(for_size);
            let (a_h, _) = height(&arrows, -1);
            let play_width = if for_size < 0 {
                -1
            } else if stacked {
                for_size
            } else {
                (for_size - a_nat - spacing).max(p_min)
            };
            let (p_h, _) = height(&play, play_width);
            let h = if stacked {
                a_h + spacing + p_h
            } else {
                a_h.max(p_h)
            };
            (h, h, -1, -1)
        }

        fn size_allocate(&self, width_now: i32, height_now: i32, _baseline: i32) {
            let obj = self.obj();
            let Some((arrows, play)) = obj.children() else {
                return;
            };
            let spacing = self.spacing.get();
            let rtl = obj.direction() == gtk::TextDirection::Rtl;
            let place = |child: &gtk::Widget, x: i32, y: i32, w: i32, h: i32| {
                let x = if rtl { width_now - x - w } else { x };
                child.size_allocate(&gtk::Allocation::new(x, y, w, h), -1);
            };
            let (_, a_nat) = width(&arrows);
            let (a_h, _) = height(&arrows, a_nat);
            if self.stacks(width_now) {
                place(&arrows, 0, 0, a_nat, a_h);
                let (p_h, _) = height(&play, width_now);
                place(&play, 0, a_h + spacing, width_now, p_h);
            } else {
                let (p_min, p_nat) = width(&play);
                let p_w = (width_now - a_nat - spacing).clamp(p_min, p_nat.max(p_min));
                let (p_h, _) = height(&play, p_w);
                place(&arrows, 0, (height_now - a_h) / 2, a_nat, a_h);
                place(&play, a_nat + spacing, (height_now - p_h) / 2, p_w, p_h);
            }
        }
    }

    impl PlayRow {
        /// Whether the button goes under the arrows at `width`: only where
        /// it may, and when the arrows and its whole name do not fit.
        fn stacks(&self, width_now: i32) -> bool {
            let Some((arrows, play)) = self.obj().children() else {
                return false;
            };
            self.stacking.get()
                && width_now >= 0
                && width_now < width(&arrows).1 + self.spacing.get() + width(&play).1
        }
    }

    fn width(widget: &gtk::Widget) -> (i32, i32) {
        let (min, nat, _, _) = widget.measure(gtk::Orientation::Horizontal, -1);
        (min, nat)
    }

    fn height(widget: &gtk::Widget, for_width: i32) -> (i32, i32) {
        let (min, nat, _, _) = widget.measure(gtk::Orientation::Vertical, for_width);
        (min, nat)
    }
}

glib::wrapper! {
    pub struct PlayRow(ObjectSubclass<imp::PlayRow>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl PlayRow {
    /// The arrows and the play button, in the order the template adds them.
    fn children(&self) -> Option<(gtk::Widget, gtk::Widget)> {
        let arrows = self.first_child()?;
        let play = arrows.next_sibling()?;
        Some((arrows, play))
    }

    pub(crate) fn set_spacing(&self, spacing: i32) {
        if self.imp().spacing.replace(spacing) != spacing {
            self.queue_resize();
        }
    }

    /// On a narrow page the button may go under the arrows; beside the year
    /// it keeps its place and its whole width.
    pub(crate) fn set_stacking(&self, stacking: bool) {
        if self.imp().stacking.replace(stacking) != stacking {
            self.queue_resize();
        }
    }
}

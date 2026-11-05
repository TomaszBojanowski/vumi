//! The content beside the queue, laid out once per slide. While the queue
//! slides in or out beside the content, libadwaita's split view gives the
//! content a new width every frame, and a grid laid out again every frame
//! steps through every column count on the way, its covers growing and
//! shrinking (a quarter of a second, about 28 layouts). This bin lays its
//! child out at once at the width it will have when the slide ends, and keeps
//! that width until then:
//!
//! - opening: the page takes its final columns in the first frame; the room
//!   left for the queue is filled at once with a pane of the queue's own look
//!   (the classes of the split view's sidebar), and the queue's content slides
//!   in over it;
//! - closing: the page takes the full width in the first frame, under the
//!   queue, which slides away over it (the bin clips what is still covered).

use adw::subclass::prelude::*;
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};

/// The longest a slide is waited for: after it the child follows the bin
/// again (the slide was cut short, or the window resized during it).
const LONGEST: i64 = 1_000_000;

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct SteadyBin {
        pub child: RefCell<Option<gtk::Widget>>,
        /// The room left for the queue while it slides in.
        pub pane: RefCell<Option<gtk::Widget>>,
        /// The width the child is laid out at until the slide ends (0: none).
        pub target: Cell<i32>,
        /// True while the queue slides in (the pane shows).
        pub opening: Cell<bool>,
        /// When the slide began (glib::monotonic_time).
        pub since: Cell<i64>,
        /// How far the bin was from the target at the last layout: a slide
        /// only comes closer, a resized window goes away from it.
        pub distance: Cell<i32>,
        /// Runs once the slide is over.
        pub done: RefCell<Option<Box<dyn FnOnce()>>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for SteadyBin {
        const NAME: &'static str = "VumiSteadyBin";
        type Type = super::SteadyBin;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for SteadyBin {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            // What the child still has under the queue while it slides away.
            obj.set_overflow(gtk::Overflow::Hidden);
            let pane = gtk::Box::builder()
                .css_classes(["sidebar-pane", "end"])
                .can_target(false)
                .build();
            pane.set_parent(&*obj);
            pane.set_child_visible(false);
            self.pane.replace(Some(pane.upcast()));
        }

        fn dispose(&self) {
            for widget in [self.child.take(), self.pane.take()].into_iter().flatten() {
                widget.unparent();
            }
        }
    }

    impl WidgetImpl for SteadyBin {
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            match self.child.borrow().as_ref() {
                Some(child) => child.measure(orientation, for_size),
                None => (0, 0, -1, -1),
            }
        }

        fn size_allocate(&self, width: i32, height: i32, baseline: i32) {
            let target = self.target.get();
            if target > 0 {
                let distance = (width - target).abs();
                let late = glib::monotonic_time() - self.since.get() > LONGEST;
                if distance == 0 || distance > self.distance.get() || late {
                    self.obj().release();
                } else {
                    self.distance.set(distance);
                }
            }
            let target = self.target.get();
            let laid_out = if target > 0 { target } else { width };
            if let Some(child) = self.child.borrow().as_ref() {
                child.allocate(laid_out, height, baseline, None);
            }
            if let Some(pane) = self.pane.borrow().as_ref() {
                // The room right of the child that the queue is sliding into.
                let room = width - laid_out;
                let shown = self.opening.get() && target > 0 && room > 0;
                pane.set_child_visible(shown);
                if shown {
                    let at = gtk::gsk::Transform::new()
                        .translate(&gtk::graphene::Point::new(laid_out as f32, 0.0));
                    pane.allocate(room, height, -1, Some(at));
                }
            }
        }
    }
}

glib::wrapper! {
    pub struct SteadyBin(ObjectSubclass<imp::SteadyBin>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl Default for SteadyBin {
    fn default() -> Self {
        glib::Object::new()
    }
}

impl SteadyBin {
    /// Puts `child` in the bin (under the pane).
    pub(crate) fn set_child(&self, child: &impl IsA<gtk::Widget>) {
        let imp = self.imp();
        if let Some(old) = imp.child.take() {
            old.unparent();
        }
        let child = child.upcast_ref::<gtk::Widget>();
        child.insert_after(self, None::<&gtk::Widget>);
        imp.child.replace(Some(child.clone()));
    }

    /// A slide begins: the child is laid out at `width` until the bin has it
    /// too (or the slide is cut short); `opening` fills the room left for the
    /// queue meanwhile. `done` runs when it is over.
    pub(crate) fn hold(&self, width: i32, opening: bool, done: impl FnOnce() + 'static) {
        let imp = self.imp();
        // A slide still running is over now.
        self.release();
        imp.target.set(width.max(1));
        imp.opening.set(opening);
        imp.since.set(glib::monotonic_time());
        imp.distance.set(i32::MAX);
        imp.done.replace(Some(Box::new(done)));
        self.queue_allocate();
    }

    /// The child follows the bin again. `done` runs after the layout under
    /// way, which it may change.
    fn release(&self) {
        let imp = self.imp();
        imp.target.set(0);
        imp.opening.set(false);
        if let Some(done) = imp.done.take() {
            glib::idle_add_local_once(done);
        }
    }
}

/// The queue's width beside content `total` wide, as libadwaita's split view
/// sizes its sidebar: `fraction` of the width, between `lower` and `upper`
/// (in px), and never under the queue's own minimum `own_minimum`.
pub(crate) fn queue_width(
    total: i32,
    fraction: f64,
    lower: f64,
    upper: f64,
    own_minimum: i32,
) -> i32 {
    let lower = lower.max(own_minimum as f64);
    let upper = upper.max(own_minimum as f64);
    (total as f64 * fraction).clamp(lower, upper) as i32
}

#[cfg(test)]
mod tests {
    use super::queue_width;

    #[test]
    fn the_queue_is_a_quarter_of_the_width_from_280_to_380() {
        // As measured at windows of 2048, 1575 and 1200 px (the content
        // beside the 300 px sidebar: 1748, 1275, 900 px).
        assert_eq!(queue_width(1748, 0.25, 280.0, 380.0, 0), 380);
        assert_eq!(queue_width(1275, 0.25, 280.0, 380.0, 0), 318);
        assert_eq!(queue_width(900, 0.25, 280.0, 380.0, 0), 280);
        // A queue that needs more takes more.
        assert_eq!(queue_width(900, 0.25, 280.0, 380.0, 300), 300);
    }
}

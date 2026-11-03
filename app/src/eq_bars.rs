//! Three bars that bounce while a track plays, moved by the widget itself
//! about twelve times a second. A CSS animation redrew the window at the
//! display's rate for these few pixels: 120 times a second on a 120 Hz screen.
//! The bars keep their CSS look (style.css: .eq-bar); each frame gives them
//! the transform the animation's scaleY gave them, so they are drawn as
//! before (drawn by hand, the short ones came out thinner at scale 1.25).

use adw::subclass::prelude::*;
use gtk::prelude::*;
use gtk::{glib, graphene, gsk};
use std::time::Duration;

/// What the bars of a song show.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum Mode {
    /// Not the song that plays: its row hides the bars.
    #[default]
    Still,
    /// The song plays: the bars bounce.
    Moving,
    /// The song is paused: the bars rest, low.
    Paused,
}

/// Between two bars, as the Box that held them had it.
const GAP: i32 = 2;

/// Between two frames of the bounce: about twelve a second.
const FRAME: Duration = Duration::from_millis(83);

/// Each bar's tempo and start in seconds: one way up or down, and how far
/// into its bounce it starts, so the three never move as one.
const TEMPO: [(f64, f64); 3] = [(0.6, 0.0), (0.45, 0.2), (0.75, 0.4)];

/// The lowest a bar goes, of its full height; paused bars stay there.
const LOW: f32 = 0.3;

/// Bars that play without moving (a window in the background, "Reduce
/// animations" in GNOME): still, at different heights, unlike paused ones.
const STILL: [f32; 3] = [0.65, 1.0, 0.45];

/// How tall each bar is, from 0 to 1 of its full height, `seconds` after the
/// bars began to move.
pub(crate) fn pose(mode: Mode, backdrop: bool, animate: bool, seconds: f64) -> [f32; 3] {
    match mode {
        Mode::Paused => [LOW; 3],
        Mode::Moving if animate && !backdrop => {
            TEMPO.map(|(tempo, start)| bounce(seconds + start, tempo))
        }
        _ => STILL,
    }
}

/// One bar `seconds` into its bounce: up from LOW in `tempo` seconds, then
/// down again, easing in and out at both ends.
fn bounce(seconds: f64, tempo: f64) -> f32 {
    let phase = seconds.max(0.0) / tempo;
    let mut progress = phase.fract();
    // Every other way is down.
    if phase as u64 % 2 == 1 {
        progress = 1.0 - progress;
    }
    LOW + (1.0 - LOW) * ease_in_out(progress) as f32
}

/// CSS's ease-in-out, cubic-bezier(0.42, 0, 0.58, 1), as the CSS animation
/// eased: the point of the curve at `t` along, found by halving. (Adwaita's
/// own easing needs libadwaita started, which the tests cannot do.)
fn ease_in_out(t: f64) -> f64 {
    let curve = |s: f64, a: f64, b: f64| {
        3.0 * (1.0 - s) * (1.0 - s) * s * a + 3.0 * (1.0 - s) * s * s * b + s * s * s
    };
    let (mut low, mut high) = (0.0, 1.0);
    for _ in 0..40 {
        let middle = (low + high) / 2.0;
        if curve(middle, 0.42, 0.58) < t {
            low = middle;
        } else {
            high = middle;
        }
    }
    curve((low + high) / 2.0, 0.0, 1.0)
}

mod imp {
    use super::*;
    use std::cell::{Cell, RefCell};

    #[derive(Default)]
    pub struct EqBars {
        pub(super) mode: Cell<Mode>,
        /// When the bars began to move (glib::monotonic_time).
        pub(super) started: Cell<i64>,
        /// Asks for the next frame while the bars move on screen.
        pub(super) timer: RefCell<Option<glib::SourceId>>,
        /// Animations were on at the last tick: a change moves them once more.
        pub(super) animate: Cell<bool>,
        pub(super) bars: RefCell<Vec<gtk::Widget>>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for EqBars {
        const NAME: &'static str = "VumiEqBars";
        type Type = super::EqBars;
        type ParentType = gtk::Widget;
    }

    impl ObjectImpl for EqBars {
        fn constructed(&self) {
            self.parent_constructed();
            let obj = self.obj();
            obj.add_css_class("eq");
            obj.set_halign(gtk::Align::End);
            obj.set_valign(gtk::Align::Center);
            let bars: Vec<gtk::Widget> = (0..3)
                .map(|_| gtk::Box::builder().css_classes(["eq-bar"]).build().upcast())
                .collect();
            for bar in &bars {
                bar.set_parent(&*obj);
            }
            self.bars.replace(bars);
        }

        fn dispose(&self) {
            if let Some(timer) = self.timer.take() {
                timer.remove();
            }
            for bar in self.bars.take() {
                bar.unparent();
            }
        }
    }

    impl WidgetImpl for EqBars {
        /// The bars side by side, GAP apart, as tall as the tallest.
        fn measure(&self, orientation: gtk::Orientation, for_size: i32) -> (i32, i32, i32, i32) {
            let bars = self.bars.borrow();
            let sizes = bars.iter().map(|bar| bar.measure(orientation, for_size));
            let (minimum, natural) = match orientation {
                gtk::Orientation::Horizontal => {
                    let gaps = GAP * (bars.len() as i32 - 1).max(0);
                    sizes.fold((gaps, gaps), |(m, n), (bm, bn, _, _)| (m + bm, n + bn))
                }
                _ => sizes.fold((0, 0), |(m, n), (bm, bn, _, _)| (m.max(bm), n.max(bn))),
            };
            (minimum, natural, -1, -1)
        }

        /// Each bar grows from its middle, as the animation's scaleY made it
        /// (GTK puts a CSS transform around the middle of the widget).
        fn size_allocate(&self, _width: i32, height: i32, _baseline: i32) {
            let obj = self.obj();
            let backdrop = obj.state_flags().contains(gtk::StateFlags::BACKDROP);
            let animate = obj.settings().is_gtk_enable_animations();
            let seconds = (glib::monotonic_time() - self.started.get()) as f64 / 1e6;
            let heights = pose(self.mode.get(), backdrop, animate, seconds);
            let mut x = 0;
            for (bar, scale) in self.bars.borrow().iter().zip(heights) {
                let width = bar.measure(gtk::Orientation::Horizontal, -1).1;
                let middle = graphene::Point::new(width as f32 / 2.0, height as f32 / 2.0);
                let transform = gsk::Transform::new()
                    .translate(&graphene::Point::new(x as f32, 0.0))
                    .translate(&middle)
                    .scale(1.0, scale)
                    .translate(&graphene::Point::new(-middle.x(), -middle.y()));
                bar.allocate(width, height, -1, Some(transform));
                x += width + GAP;
            }
        }

        fn map(&self) {
            self.parent_map();
            self.obj().update_timer();
        }

        fn unmap(&self) {
            self.parent_unmap();
            self.obj().update_timer();
        }

        fn state_flags_changed(&self, old: &gtk::StateFlags) {
            self.parent_state_flags_changed(old);
            let obj = self.obj();
            if old.contains(gtk::StateFlags::BACKDROP)
                != obj.state_flags().contains(gtk::StateFlags::BACKDROP)
            {
                obj.update_timer();
                obj.queue_allocate();
            }
        }
    }
}

glib::wrapper! {
    /// The bars before a song's title (song_row.rs, album pages).
    pub struct EqBars(ObjectSubclass<imp::EqBars>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::Buildable, gtk::ConstraintTarget;
}

impl EqBars {
    pub(crate) fn new() -> Self {
        glib::Object::new()
    }

    /// Moving while the song plays, resting while it is paused.
    pub(crate) fn set_mode(&self, mode: Mode) {
        let imp = self.imp();
        if imp.mode.replace(mode) == mode {
            return;
        }
        if mode == Mode::Moving {
            imp.started.set(glib::monotonic_time());
        }
        self.update_timer();
        self.queue_allocate();
    }

    /// Frames only while the bars move where they can be seen: on screen,
    /// in a window in front.
    fn update_timer(&self) {
        let imp = self.imp();
        let wanted = imp.mode.get() == Mode::Moving
            && self.is_mapped()
            && !self.state_flags().contains(gtk::StateFlags::BACKDROP);
        if !wanted {
            if let Some(timer) = imp.timer.take() {
                timer.remove();
            }
            return;
        }
        if imp.timer.borrow().is_some() {
            return;
        }
        imp.animate.set(self.settings().is_gtk_enable_animations());
        let bars = self.downgrade();
        let timer = glib::timeout_add_local(FRAME, move || {
            let Some(bars) = bars.upgrade() else {
                return glib::ControlFlow::Break;
            };
            // "Reduce animations": the still bars, placed once.
            let animate = bars.settings().is_gtk_enable_animations();
            let animated = bars.imp().animate.replace(animate);
            if animate || animated {
                bars.queue_allocate();
            }
            glib::ControlFlow::Continue
        });
        imp.timer.replace(Some(timer));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Heights over three seconds, 50 times a second.
    fn over_time(mode: Mode, backdrop: bool, animate: bool) -> Vec<[f32; 3]> {
        (0..150)
            .map(|i| pose(mode, backdrop, animate, f64::from(i) / 50.0))
            .collect()
    }

    #[test]
    fn only_bars_that_play_in_front_move() {
        let moving = over_time(Mode::Moving, false, true);
        for bar in 0..3 {
            let heights: Vec<f32> = moving.iter().map(|pose| pose[bar]).collect();
            let low = heights.iter().cloned().fold(1.0, f32::min);
            let high = heights.iter().cloned().fold(0.0, f32::max);
            assert!(low < 0.35 && high > 0.95, "bar {bar}: {low}..{high}");
        }
        for (mode, backdrop, animate) in [
            (Mode::Moving, true, true),
            (Mode::Moving, false, false),
            (Mode::Still, false, true),
            (Mode::Paused, false, true),
            (Mode::Paused, true, true),
        ] {
            let poses = over_time(mode, backdrop, animate);
            assert!(
                poses.iter().all(|pose| *pose == poses[0]),
                "{mode:?}, backdrop {backdrop}, animate {animate}"
            );
        }
    }

    #[test]
    fn paused_bars_rest_low_and_still_ones_stand_apart() {
        assert_eq!(pose(Mode::Paused, false, true, 1.0), [0.3; 3]);
        // Paused wins over a window in the background.
        assert_eq!(pose(Mode::Paused, true, true, 1.0), [0.3; 3]);
        assert_eq!(pose(Mode::Moving, true, true, 1.0), [0.65, 1.0, 0.45]);
        assert_eq!(pose(Mode::Moving, false, false, 1.0), [0.65, 1.0, 0.45]);
    }

    #[test]
    fn the_easing_is_css_ease_in_out() {
        assert!(ease_in_out(0.0).abs() < 1e-9);
        assert!((ease_in_out(1.0) - 1.0).abs() < 1e-9);
        assert!((ease_in_out(0.5) - 0.5).abs() < 1e-9, "symmetric");
        // Slow at both ends, fast in the middle.
        assert!(ease_in_out(0.1) < 0.1 && ease_in_out(0.9) > 0.9);
        assert!((ease_in_out(0.25) + ease_in_out(0.75) - 1.0).abs() < 1e-9);
    }

    #[test]
    fn a_bar_bounces_from_low_to_full_and_back() {
        assert!((bounce(0.0, 0.6) - 0.3).abs() < 1e-6);
        assert!((bounce(0.6, 0.6) - 1.0).abs() < 1e-6);
        assert!(
            (bounce(0.3, 0.6) - 0.65).abs() < 1e-6,
            "half way at half time"
        );
        assert!((bounce(1.2, 0.6) - 0.3).abs() < 1e-6);
        // The bars start apart: the second and third are already moving.
        let [first, second, third] = pose(Mode::Moving, false, true, 0.0);
        assert_eq!(first, 0.3);
        assert!(second > 0.3 && third > 0.3);
    }
}

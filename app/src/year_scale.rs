//! The year scale of the Tuner card on the Start page: a tick over every year
//! with albums, as tall as the square root of their number, the years labelled
//! as often as the width allows, and a needle on the year chosen. It snaps to
//! years with albums, by mouse and by keyboard, and reads out as a slider.

use adw::prelude::*;
use adw::subclass::prelude::*;
use gettextrs::{gettext, ngettext};
use gtk::{gdk, glib, graphene, gsk};
use std::cell::{Cell, OnceCell, RefCell};
use std::collections::BTreeMap;
use std::sync::Arc;

/// The height of the ticks' band: the needle is as tall, the ticks at most
/// TICK_MAX, at least TICK_MIN.
const BAND: f64 = 58.0;
const TICK_MAX: f64 = 44.0;
const TICK_MIN: f64 = 6.0;
/// Room at each end of the scale, so the ticks and the needle of the first
/// and the last year fit.
const PAD: f64 = 2.0;
/// Between the baseline and the labels, and between two labels.
const LABEL_TOP: f64 = 8.0;
const LABEL_GAP: f64 = 10.0;

/// Albums per year. Albums without a year, or with a year of 0 or less, are
/// left out.
pub(crate) fn year_counts(albums: &[Arc<subsonic::Album>]) -> BTreeMap<i32, usize> {
    let mut counts = BTreeMap::new();
    for year in albums.iter().filter_map(|album| album.year) {
        if year > 0 {
            *counts.entry(year).or_insert(0) += 1;
        }
    }
    counts
}

/// The nearest year with albums after `year` (`forward`) or before it.
pub(crate) fn neighbour(counts: &BTreeMap<i32, usize>, year: i32, forward: bool) -> Option<i32> {
    if forward {
        counts.range(year + 1..).next().map(|(year, _)| *year)
    } else {
        counts.range(..year).next_back().map(|(year, _)| *year)
    }
}

/// The year with albums nearest to `year`; of two as near, the earlier.
pub(crate) fn nearest(counts: &BTreeMap<i32, usize>, year: i32) -> Option<i32> {
    let before = counts.range(..=year).next_back().map(|(year, _)| *year);
    let after = counts.range(year..).next().map(|(year, _)| *year);
    match (before, after) {
        (Some(before), Some(after)) if year - before <= after - year => Some(before),
        (_, Some(after)) => Some(after),
        (before, None) => before,
    }
}

/// Page Up / Page Down: `by` years on, then the first year with albums from
/// there on in that direction; past the end of the scale, its last (or first)
/// year.
pub(crate) fn jump(counts: &BTreeMap<i32, usize>, year: i32, by: i32) -> Option<i32> {
    let target = year + by;
    if by >= 0 {
        counts
            .range(target..)
            .next()
            .or_else(|| counts.iter().next_back())
            .map(|(year, _)| *year)
    } else {
        counts
            .range(..=target)
            .next_back()
            .or_else(|| counts.iter().next())
            .map(|(year, _)| *year)
    }
}

/// Where `year` is on a scale `width` wide from `first` to `last`.
pub(crate) fn x_of(year: i32, first: i32, last: i32, width: f64) -> f64 {
    if last <= first {
        return width / 2.0;
    }
    f64::from(year - first) / f64::from(last - first) * width
}

/// The year at `x` on a scale `width` wide from `first` to `last`.
pub(crate) fn year_at(x: f64, first: i32, last: i32, width: f64) -> i32 {
    if last <= first || width <= 0.0 {
        return first;
    }
    let along = (x / width).clamp(0.0, 1.0);
    first + (along * f64::from(last - first)).round() as i32
}

/// How many years apart the labelled years are: the smallest of 10, 20, 50
/// and 100 at which labels `label_width` wide, `gap` apart, fit side by side.
pub(crate) fn label_step(first: i32, last: i32, width: f64, label_width: f64, gap: f64) -> i32 {
    let per_year = if last > first {
        width / f64::from(last - first)
    } else {
        width
    };
    [10, 20, 50]
        .into_iter()
        .find(|step| per_year * f64::from(*step) >= label_width + gap)
        .unwrap_or(100)
}

/// The labelled years: the first one, then every `step` years, and the last
/// one. A label starts at its year and the last one ends there, so the label
/// before the last is left out when the two would touch.
pub(crate) fn labelled_years(
    first: i32,
    last: i32,
    step: i32,
    width: f64,
    label_width: f64,
    gap: f64,
) -> Vec<i32> {
    let mut years: Vec<i32> = (first..=last).step_by(step.max(1) as usize).collect();
    if years.last() != Some(&last) {
        let room = |a: i32, b: i32| x_of(b, first, last, width) - x_of(a, first, last, width);
        if years.len() > 1
            && years
                .last()
                .is_some_and(|before| room(*before, last) < 2.0 * label_width + gap)
        {
            years.pop();
        }
        years.push(last);
    }
    years
}

/// The height of the tick over a year with `count` albums when the most
/// albums in a year are `most`: the square root of their share between `min`
/// and `max`, so that a year of one album stays visible next to a full one.
pub(crate) fn tick_height(count: usize, most: usize, min: f64, max: f64) -> f64 {
    if count == 0 || most == 0 {
        return 0.0;
    }
    let share = (count.min(most) as f64 / most as f64).sqrt();
    min + (max - min) * share
}

/// The year the scale starts on: that of the album in the player, else that
/// of the newest album of the history, else a year with albums that
/// `random` picks (it gets how many there are and gives an index).
pub(crate) fn start_year(
    counts: &BTreeMap<i32, usize>,
    player_year: Option<i32>,
    history_years: impl IntoIterator<Item = Option<i32>>,
    random: impl FnOnce(usize) -> usize,
) -> Option<i32> {
    let has_albums = |year: &i32| counts.contains_key(year);
    if let Some(year) = player_year.filter(has_albums) {
        return Some(year);
    }
    if let Some(year) = history_years.into_iter().flatten().find(has_albums) {
        return Some(year);
    }
    if counts.is_empty() {
        return None;
    }
    let index = random(counts.len()).min(counts.len() - 1);
    counts.keys().nth(index).copied()
}

mod imp {
    use super::*;

    #[derive(Default)]
    pub struct YearScale {
        /// Albums per year, as year_counts gives them.
        pub counts: RefCell<BTreeMap<i32, usize>>,
        /// The year chosen; None until the owner sets one.
        pub year: Cell<Option<i32>>,
        /// Where the needle is, in years: between two while it moves.
        pub needle_at: Cell<f64>,
        /// The needle: a child of its own, so CSS gives its colour.
        pub needle: OnceCell<gtk::Box>,
        pub animation: RefCell<Option<adw::TimedAnimation>>,
        /// Told the new year after every change.
        pub(crate) listeners: RefCell<Vec<super::Listener>>,
        /// Where a drag began.
        pub drag_from: Cell<f64>,
    }

    #[glib::object_subclass]
    impl ObjectSubclass for YearScale {
        const NAME: &'static str = "VumiYearScale";
        type Type = super::YearScale;
        type ParentType = gtk::Widget;
        // A screen reader can set the value too (set_current_value).
        type Interfaces = (gtk::AccessibleRange,);

        fn class_init(klass: &mut Self::Class) {
            klass.set_css_name("yearscale");
            klass.set_accessible_role(gtk::AccessibleRole::Slider);
        }
    }

    impl ObjectImpl for YearScale {
        fn constructed(&self) {
            self.parent_constructed();
            self.obj().setup();
        }

        fn dispose(&self) {
            if let Some(needle) = self.needle.get() {
                needle.unparent();
            }
        }
    }

    impl WidgetImpl for YearScale {
        fn measure(&self, orientation: gtk::Orientation, _for_size: i32) -> (i32, i32, i32, i32) {
            match orientation {
                gtk::Orientation::Horizontal => (120, 120, -1, -1),
                _ => {
                    let (_, label_height) = self.obj().label_size();
                    let height = (BAND + LABEL_TOP + label_height).ceil() as i32;
                    (height, height, -1, -1)
                }
            }
        }

        fn size_allocate(&self, width: i32, _height: i32, _baseline: i32) {
            self.obj().place_needle(f64::from(width));
        }

        fn snapshot(&self, snapshot: &gtk::Snapshot) {
            self.obj().draw(snapshot);
        }
    }

    impl AccessibleImpl for YearScale {}

    impl AccessibleRangeImpl for YearScale {
        /// A screen reader sets a value: the nearest year with albums.
        fn set_current_value(&self, value: f64) -> bool {
            let obj = self.obj();
            if obj.range().is_none() {
                return false;
            }
            obj.set_year(value.round() as i32, true);
            true
        }
    }
}

/// Told the year after every change.
pub(crate) type Listener = Box<dyn Fn(i32)>;

glib::wrapper! {
    pub struct YearScale(ObjectSubclass<imp::YearScale>)
        @extends gtk::Widget,
        @implements gtk::Accessible, gtk::AccessibleRange, gtk::Buildable, gtk::ConstraintTarget;
}

impl YearScale {
    fn setup(&self) {
        self.set_focusable(true);
        self.update_property(&[
            gtk::accessible::Property::Label(&gettext("Year")),
            gtk::accessible::Property::Orientation(gtk::Orientation::Horizontal),
        ]);
        let needle = gtk::Box::builder().css_classes(["needle"]).build();
        needle.set_parent(self);
        let _ = self.imp().needle.set(needle);

        // A click puts the needle on the year with albums nearest to it; a
        // drag takes it along.
        let click = gtk::GestureClick::new();
        click.connect_pressed(glib::clone!(
            #[weak(rename_to = scale)]
            self,
            move |_, _, x, _| {
                scale.grab_focus();
                scale.set_year_at(x, true);
            }
        ));
        self.add_controller(click);
        let drag = gtk::GestureDrag::new();
        drag.connect_drag_begin(glib::clone!(
            #[weak(rename_to = scale)]
            self,
            move |_, x, _| scale.imp().drag_from.set(x)
        ));
        drag.connect_drag_update(glib::clone!(
            #[weak(rename_to = scale)]
            self,
            move |_, dx, _| scale.set_year_at(scale.imp().drag_from.get() + dx, false)
        ));
        self.add_controller(drag);

        let keys = gtk::EventControllerKey::new();
        keys.connect_key_pressed(glib::clone!(
            #[weak(rename_to = scale)]
            self,
            #[upgrade_or]
            glib::Propagation::Proceed,
            move |_, key, _, _| {
                if scale.press_key(key) {
                    glib::Propagation::Stop
                } else {
                    glib::Propagation::Proceed
                }
            }
        ));
        self.add_controller(keys);
    }

    /// Calls `f` with the year after every change.
    pub(crate) fn connect_changed(&self, f: impl Fn(i32) + 'static) {
        self.imp().listeners.borrow_mut().push(Box::new(f));
    }

    pub(crate) fn year(&self) -> Option<i32> {
        self.imp().year.get()
    }

    /// How many albums the library has from `year`.
    pub(crate) fn count(&self, year: i32) -> usize {
        self.imp().counts.borrow().get(&year).copied().unwrap_or(0)
    }

    /// The year with albums before or after `year` (‹ ›).
    pub(crate) fn neighbour_of(&self, year: i32, forward: bool) -> Option<i32> {
        neighbour(&self.imp().counts.borrow(), year, forward)
    }

    /// New counts, after a sync. The year stays, or goes to the nearest year
    /// with albums if its own has none any more; without any years there is
    /// no year, and the owner chooses one when there are again.
    pub(crate) fn set_counts(&self, counts: BTreeMap<i32, usize>) {
        let imp = self.imp();
        if *imp.counts.borrow() == counts {
            return;
        }
        let empty = counts.is_empty();
        imp.counts.replace(counts);
        self.queue_draw();
        self.queue_allocate();
        match imp.year.get() {
            _ if empty => imp.year.set(None),
            Some(year) if !imp.counts.borrow().contains_key(&year) => {
                imp.year.set(None);
                self.set_year(year, false);
            }
            _ => {}
        }
        self.update_accessible();
    }

    /// Puts the needle on `year`, or on the year with albums nearest to it.
    pub(crate) fn set_year(&self, year: i32, animate: bool) {
        let imp = self.imp();
        let Some(year) = nearest(&imp.counts.borrow(), year) else {
            return;
        };
        if imp.year.get() == Some(year) {
            return;
        }
        imp.year.set(Some(year));
        self.move_needle(f64::from(year), animate);
        self.update_accessible();
        for listener in imp.listeners.borrow().iter() {
            listener(year);
        }
    }

    /// ‹ › and the arrow keys: the year with albums before or after this one.
    pub(crate) fn step(&self, forward: bool) {
        let Some(year) = self.year() else {
            return;
        };
        if let Some(to) = self.neighbour_of(year, forward) {
            self.set_year(to, true);
        }
    }

    /// The keys of a slider: ← ↓ and → ↑ the year before and after, Home and
    /// End the first and the last, Page Up and Page Down ten years on and
    /// back. True if the key was one of them.
    pub(crate) fn press_key(&self, key: gdk::Key) -> bool {
        let Some(year) = self.year() else {
            return false;
        };
        let counts = self.imp().counts.borrow().clone();
        let to = match key {
            gdk::Key::Left | gdk::Key::KP_Left | gdk::Key::Down | gdk::Key::KP_Down => {
                neighbour(&counts, year, false)
            }
            gdk::Key::Right | gdk::Key::KP_Right | gdk::Key::Up | gdk::Key::KP_Up => {
                neighbour(&counts, year, true)
            }
            gdk::Key::Home | gdk::Key::KP_Home => counts.keys().next().copied(),
            gdk::Key::End | gdk::Key::KP_End => counts.keys().next_back().copied(),
            gdk::Key::Page_Up | gdk::Key::KP_Page_Up => jump(&counts, year, 10),
            gdk::Key::Page_Down | gdk::Key::KP_Page_Down => jump(&counts, year, -10),
            _ => return false,
        };
        if let Some(to) = to {
            self.set_year(to, true);
        }
        true
    }

    /// The first and the last year, if there are any.
    fn range(&self) -> Option<(i32, i32)> {
        let counts = self.imp().counts.borrow();
        Some((*counts.keys().next()?, *counts.keys().next_back()?))
    }

    fn set_year_at(&self, x: f64, animate: bool) {
        let Some((first, last)) = self.range() else {
            return;
        };
        let track = (f64::from(self.width()) - 2.0 * PAD).max(1.0);
        self.set_year(year_at(x - PAD, first, last, track), animate);
    }

    fn move_needle(&self, to: f64, animate: bool) {
        let imp = self.imp();
        if let Some(animation) = imp.animation.take() {
            animation.pause();
        }
        if !animate || !self.is_mapped() {
            imp.needle_at.set(to);
            self.queue_allocate();
            return;
        }
        let target = adw::CallbackAnimationTarget::new(glib::clone!(
            #[weak(rename_to = scale)]
            self,
            move |at| {
                scale.imp().needle_at.set(at);
                scale.queue_allocate();
            }
        ));
        let animation = adw::TimedAnimation::new(self, imp.needle_at.get(), to, 250, target);
        animation.set_easing(adw::Easing::EaseOutCubic);
        animation.play();
        imp.animation.replace(Some(animation));
    }

    fn update_accessible(&self) {
        let Some((first, last)) = self.range() else {
            return;
        };
        let mut properties = vec![
            gtk::accessible::Property::ValueMin(f64::from(first)),
            gtk::accessible::Property::ValueMax(f64::from(last)),
        ];
        let text;
        if let Some(year) = self.year() {
            let n = self.count(year) as u32;
            // Translators: what a screen reader says of the year chosen on
            // the scale: the year, and how many albums the library has from it.
            text = ngettext("{year}, {n} album", "{year}, {n} albums", n)
                .replace("{year}", &year.to_string())
                .replace("{n}", &n.to_string());
            properties.push(gtk::accessible::Property::ValueNow(f64::from(year)));
            properties.push(gtk::accessible::Property::ValueText(&text));
        }
        self.update_property(&properties);
    }

    /// How wide and tall a year's label is.
    fn label_size(&self) -> (f64, f64) {
        let (width, height) = self.create_pango_layout(Some("8888")).pixel_size();
        (f64::from(width), f64::from(height))
    }

    /// The scale of the surface, also a fractional one: edges are put on
    /// whole pixels of the screen, so the thin ticks stay sharp.
    fn device_scale(&self) -> f64 {
        self.native()
            .and_then(|native| native.surface())
            .map_or(1.0, |surface| surface.scale())
    }

    fn place_needle(&self, width: f64) {
        let Some(needle) = self.imp().needle.get() else {
            return;
        };
        let Some((first, last)) = self.range().filter(|_| self.year().is_some()) else {
            needle.set_child_visible(false);
            return;
        };
        needle.set_child_visible(true);
        let track = (width - 2.0 * PAD).max(1.0);
        let at = self.imp().needle_at.get();
        let x = if last > first {
            PAD + (at - f64::from(first)) / f64::from(last - first) * track
        } else {
            PAD + track / 2.0
        };
        let needle_width = needle.measure(gtk::Orientation::Horizontal, -1).0.max(1);
        let scale = self.device_scale();
        let left = ((x - f64::from(needle_width) / 2.0) * scale).round() / scale;
        let place = gsk::Transform::new().translate(&graphene::Point::new(left as f32, 0.0));
        needle.allocate(needle_width, BAND as i32, -1, Some(place));
    }

    fn draw(&self, snapshot: &gtk::Snapshot) {
        let Some((first, last)) = self.range() else {
            return;
        };
        let counts = self.imp().counts.borrow();
        let width = f64::from(self.width());
        let track = (width - 2.0 * PAD).max(1.0);
        let scale = self.device_scale();
        let snap = |value: f64| (value * scale).round() / scale;
        let color = self.color();
        let shade = |alpha: f32| {
            gdk::RGBA::new(
                color.red(),
                color.green(),
                color.blue(),
                color.alpha() * alpha,
            )
        };
        let rect = |x: f64, y: f64, w: f64, h: f64| {
            graphene::Rect::new(x as f32, y as f32, w as f32, h as f32)
        };

        // The baseline under the ticks.
        snapshot.append_color(
            &shade(0.25),
            &rect(0.0, BAND, width, snap(1.0).max(1.0 / scale)),
        );

        let (label_width, _) = self.label_size();
        let step = label_step(first, last, track, label_width, LABEL_GAP);
        let labelled = labelled_years(first, last, step, track, label_width, LABEL_GAP);
        let most = counts.values().copied().max().unwrap_or(0);
        let per_year = if last > first {
            track / f64::from(last - first)
        } else {
            track
        };
        let tick_width = if per_year < 4.0 { 1.0 } else { 2.0 };
        for (&year, &count) in counts.iter() {
            let x = snap(PAD + x_of(year, first, last, track) - tick_width / 2.0);
            let height = snap(tick_height(count, most, TICK_MIN, TICK_MAX));
            let alpha = if labelled.contains(&year) { 0.75 } else { 0.4 };
            snapshot.append_color(&shade(alpha), &rect(x, BAND - height, tick_width, height));
        }

        // A label starts at its year; the last one ends there, and a single
        // year has its label in the middle.
        for &year in &labelled {
            let layout = self.create_pango_layout(Some(&year.to_string()));
            let label = f64::from(layout.pixel_size().0);
            let x = PAD + x_of(year, first, last, track);
            let left = if first == last {
                x - label / 2.0
            } else if year == last && labelled.len() > 1 {
                x - label
            } else {
                x
            };
            snapshot.save();
            snapshot.translate(&graphene::Point::new(
                snap(left.clamp(0.0, (width - label).max(0.0))) as f32,
                snap(BAND + LABEL_TOP) as f32,
            ));
            snapshot.append_layout(&layout, &shade(0.7));
            snapshot.restore();
        }
        drop(counts);
        if let Some(needle) = self.imp().needle.get() {
            self.snapshot_child(needle, snapshot);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 1955, 1965 (3), 1966, 1970 (2), 2018 (56), 2026.
    fn counts() -> BTreeMap<i32, usize> {
        [
            (1955, 1),
            (1965, 3),
            (1966, 1),
            (1970, 2),
            (2018, 56),
            (2026, 1),
        ]
        .into_iter()
        .collect()
    }

    fn album(year: Option<i32>) -> Arc<subsonic::Album> {
        Arc::new(subsonic::Album {
            id: String::new(),
            name: String::new(),
            artist: None,
            year,
            song_count: None,
            created: None,
            artist_id: None,
            starred: None,
            cover_art: None,
            genre: None,
            record_label: None,
        })
    }

    #[test]
    fn years_are_counted_without_albums_lacking_a_year() {
        let albums = [
            album(Some(1965)),
            album(Some(1965)),
            album(None),
            album(Some(0)),
            album(Some(-3)),
            album(Some(2018)),
        ];
        let counts = year_counts(&albums);
        assert_eq!(
            counts.into_iter().collect::<Vec<_>>(),
            [(1965, 2), (2018, 1)]
        );
    }

    #[test]
    fn neighbours_skip_years_without_albums() {
        let c = counts();
        assert_eq!(neighbour(&c, 1965, true), Some(1966));
        assert_eq!(neighbour(&c, 1970, true), Some(2018));
        assert_eq!(neighbour(&c, 2018, false), Some(1970));
        assert_eq!(neighbour(&c, 1980, false), Some(1970));
        assert_eq!(neighbour(&c, 2026, true), None);
        assert_eq!(neighbour(&c, 1955, false), None);
    }

    #[test]
    fn the_nearest_year_with_albums_wins_and_a_tie_goes_earlier() {
        let c = counts();
        assert_eq!(nearest(&c, 1965), Some(1965));
        assert_eq!(nearest(&c, 1968), Some(1966));
        assert_eq!(nearest(&c, 1960), Some(1955));
        assert_eq!(nearest(&c, 1994), Some(1970));
        assert_eq!(nearest(&c, 1995), Some(2018));
        assert_eq!(nearest(&c, 1900), Some(1955));
        assert_eq!(nearest(&c, 2100), Some(2026));
        assert_eq!(nearest(&BTreeMap::new(), 1965), None);
    }

    #[test]
    fn page_keys_jump_ten_years_and_stop_at_the_ends() {
        let c = counts();
        assert_eq!(jump(&c, 1955, 10), Some(1965));
        assert_eq!(jump(&c, 1965, 10), Some(2018));
        assert_eq!(jump(&c, 2018, 10), Some(2026));
        assert_eq!(jump(&c, 2026, 10), Some(2026));
        assert_eq!(jump(&c, 2026, -10), Some(1970));
        assert_eq!(jump(&c, 1966, -10), Some(1955));
        assert_eq!(jump(&c, 1955, -10), Some(1955));
    }

    #[test]
    fn a_year_and_its_place_on_the_scale_match() {
        assert_eq!(x_of(1955, 1955, 2026, 710.0), 0.0);
        assert_eq!(x_of(2026, 1955, 2026, 710.0), 710.0);
        assert_eq!(x_of(1965, 1955, 2026, 710.0), 100.0);
        for year in 1955..=2026 {
            let x = x_of(year, 1955, 2026, 710.0);
            assert_eq!(year_at(x, 1955, 2026, 710.0), year);
        }
        assert_eq!(year_at(-50.0, 1955, 2026, 710.0), 1955);
        assert_eq!(year_at(5000.0, 1955, 2026, 710.0), 2026);
        assert_eq!(year_at(104.0, 1955, 2026, 710.0), 1965);
        // One year only: in the middle.
        assert_eq!(x_of(1965, 1965, 1965, 300.0), 150.0);
        assert_eq!(year_at(10.0, 1965, 1965, 300.0), 1965);
    }

    #[test]
    fn labels_are_as_dense_as_the_width_allows() {
        // 72 years with 32 px labels and 10 px between them.
        assert_eq!(label_step(1955, 2026, 1200.0, 32.0, 10.0), 10);
        assert_eq!(label_step(1955, 2026, 600.0, 32.0, 10.0), 10);
        assert_eq!(label_step(1955, 2026, 286.0, 32.0, 10.0), 20);
        assert_eq!(label_step(1955, 2026, 120.0, 32.0, 10.0), 50);
        assert_eq!(label_step(1955, 2026, 40.0, 32.0, 10.0), 100);
    }

    #[test]
    fn labels_start_at_the_first_year_and_end_with_the_last() {
        // 2025 and 2026 would touch: 2025 goes.
        assert_eq!(
            labelled_years(1955, 2026, 10, 1200.0, 32.0, 10.0),
            [1955, 1965, 1975, 1985, 1995, 2005, 2015, 2026]
        );
        // Narrow: every 20 years, and 2015 is too close to 2026.
        assert_eq!(
            labelled_years(1955, 2026, 20, 286.0, 32.0, 10.0),
            [1955, 1975, 1995, 2026]
        );
        assert_eq!(labelled_years(1965, 1965, 10, 300.0, 32.0, 10.0), [1965]);
    }

    #[test]
    fn ticks_grow_with_the_square_root_from_a_visible_minimum() {
        assert_eq!(tick_height(0, 56, 6.0, 44.0), 0.0);
        assert_eq!(tick_height(56, 56, 6.0, 44.0), 44.0);
        assert_eq!(tick_height(1, 1, 6.0, 44.0), 44.0);
        assert_eq!(tick_height(14, 56, 6.0, 44.0), 6.0 + 38.0 * 0.5);
        let one = tick_height(1, 56, 6.0, 44.0);
        let three = tick_height(3, 56, 6.0, 44.0);
        assert!(one > 6.0 && one < three && three < 44.0);
        assert!((one - 11.08).abs() < 0.01, "{one}");
    }

    #[test]
    fn the_start_year_comes_from_the_player_then_the_history_then_chance() {
        let c = counts();
        let never = |_: usize| -> usize { panic!("no random year needed") };
        assert_eq!(start_year(&c, Some(1965), [Some(2018)], never), Some(1965));
        assert_eq!(
            start_year(&c, None, [None, Some(2018), Some(1955)], never),
            Some(2018)
        );
        // A year without albums does not count: the next source decides.
        assert_eq!(
            start_year(&c, Some(1980), [Some(1990), Some(1970)], never),
            Some(1970)
        );
        assert_eq!(start_year(&c, None, [], |n| n - 1), Some(2026));
        assert_eq!(start_year(&c, None, [], |_| 0), Some(1955));
        assert_eq!(start_year(&BTreeMap::new(), Some(1965), [], |_| 0), None);
    }
}

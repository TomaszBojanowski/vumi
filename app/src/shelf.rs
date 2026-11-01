//! The shelves of the Start page: a row of album cards with ‹ › to move it
//! by what is visible, Shift + the wheel and dragging to move it sideways;
//! and LazyRow, a row that builds its cards as it is scrolled.

use adw::prelude::*;
use gtk::glib;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::sync::Arc;

use crate::album_card::AlbumCard;
use crate::main_view::MainView;

/// Puts one album card per album into `row`; a click opens the album.
pub(crate) fn fill_shelf(
    row: &gtk::Box,
    shown: &RefCell<Vec<String>>,
    albums: &[&Arc<subsonic::Album>],
    client: Option<subsonic::Client>,
) {
    // The same albums as now: keep the cards, so nothing blinks. They only
    // fetch covers that were not cached, now that there may be a client.
    let ids: Vec<String> = albums.iter().map(|a| a.id.clone()).collect();
    if *shown.borrow() == ids {
        if let Some(client) = &client {
            for_each_card(row, |card| card.retry_cover(client.clone()));
        }
        return;
    }
    shown.replace(ids);

    while let Some(child) = row.first_child() {
        row.remove(&child);
    }
    for album in albums {
        row.append(&album_button(album, client.clone()));
    }
}

/// One album card in a flat button; a click opens the album.
fn album_button(album: &Arc<subsonic::Album>, client: Option<subsonic::Client>) -> gtk::Button {
    let card = AlbumCard::new();
    card.set_album(album, client);
    let button = gtk::Button::builder()
        .child(&card)
        .css_classes(["flat", "shelf-card"])
        .build();

    let album = Arc::clone(album);
    button.connect_clicked(move |button| {
        if let Some(main) = button
            .ancestor(MainView::static_type())
            .and_downcast::<MainView>()
        {
            main.open_album_from(button, &album.id, (*album).clone());
        }
    });
    button
}

/// Calls `f` with every album card in `row`.
fn for_each_card(row: &gtk::Box, f: impl Fn(&AlbumCard)) {
    let mut child = row.first_child();
    while let Some(button) = child {
        if let Some(card) = button
            .downcast_ref::<gtk::Button>()
            .and_then(|b| b.child())
            .and_downcast::<AlbumCard>()
        {
            f(&card);
        }
        child = button.next_sibling();
    }
}

/// Adds the ‹ › pair to `place`, the same on every shelf: flat round buttons
/// with the arrows of the icon theme.
pub(crate) fn add_arrows(
    place: &gtk::Box,
    previous: &str,
    next: &str,
) -> (gtk::Button, gtk::Button) {
    place.set_spacing(6);
    let button = |icon: &str, tooltip: &str| {
        let button = gtk::Button::builder()
            .icon_name(icon)
            .tooltip_text(tooltip)
            .valign(gtk::Align::Center)
            .css_classes(["circular", "flat"])
            .build();
        place.append(&button);
        button
    };
    (
        button("go-previous-symbolic", previous),
        button("go-next-symbolic", next),
    )
}

/// The ‹ › buttons of a shelf: each moves the row by what is visible, and is
/// greyed out at its end of the row; both hide when the whole row fits.
pub(crate) fn connect_arrows(scroll: &gtk::ScrolledWindow, prev: &gtk::Button, next: &gtk::Button) {
    let adj = scroll.hadjustment();
    let update = {
        let prev = prev.downgrade();
        let next = next.downgrade();
        move |adj: &gtk::Adjustment| {
            let (Some(prev), Some(next)) = (prev.upgrade(), next.upgrade()) else {
                return;
            };
            let end = adj.upper() - adj.page_size();
            let overflow = end - adj.lower() > 1.0;
            // Opacity rather than visibility: this runs while the row is being
            // laid out, and showing or hiding widgets then upsets GTK. The
            // buttons keep their room, so the header does not change height.
            let opacity = if overflow { 1.0 } else { 0.0 };
            prev.set_opacity(opacity);
            next.set_opacity(opacity);
            prev.set_sensitive(overflow && adj.value() > adj.lower() + 1.0);
            next.set_sensitive(overflow && adj.value() < end - 1.0);
        }
    };
    adj.connect_value_changed(update.clone());
    // Also when the row or the window changes size.
    adj.connect_changed(update.clone());
    update(&adj);

    for (button, direction) in [(prev, -1.0), (next, 1.0)] {
        let adj = adj.clone();
        button.connect_clicked(move |button| {
            let to = (adj.value() + direction * adj.page_size())
                .clamp(adj.lower(), adj.upper() - adj.page_size());
            let target = adw::CallbackAnimationTarget::new({
                let adj = adj.clone();
                move |value| adj.set_value(value)
            });
            let animation = adw::TimedAnimation::new(button, adj.value(), to, 250, target);
            animation.set_easing(adw::Easing::EaseOutCubic);
            adw::prelude::AnimationExt::play(&animation);
        });
    }
}

/// Shift + the mouse wheel over a shelf moves it sideways; the plain wheel
/// scrolls the page, and a touchpad keeps its own two-finger scrolling.
pub(crate) fn connect_wheel(scroll: &gtk::ScrolledWindow) {
    let adj = scroll.hadjustment();
    let wheel = gtk::EventControllerScroll::new(gtk::EventControllerScrollFlags::VERTICAL);
    // Before the row's own scrolling, which would pass the wheel to the page.
    wheel.set_propagation_phase(gtk::PropagationPhase::Capture);
    wheel.connect_scroll(move |controller, _, dy| {
        let touchpad = controller
            .current_event_device()
            .is_some_and(|device| device.source() == gtk::gdk::InputSource::Touchpad);
        // The plain wheel scrolls the page, as anywhere else; only with Shift
        // held does it move the row sideways (the ‹ › buttons do that too).
        let shift = controller
            .current_event_state()
            .contains(gtk::gdk::ModifierType::SHIFT_MASK);
        if touchpad || !shift {
            return gtk::glib::Propagation::Proceed;
        }
        let end = adj.upper() - adj.page_size();
        let to = (adj.value() + dy * adj.page_size() / 3.0).clamp(adj.lower(), end);
        // Nothing left to move in that direction: the page scrolls instead.
        if (to - adj.value()).abs() < 0.5 {
            return gtk::glib::Propagation::Proceed;
        }
        adj.set_value(to);
        gtk::glib::Propagation::Stop
    });
    scroll.add_controller(wheel);
}

/// Dragging a shelf with the mouse moves it sideways, as a finger would on a
/// touch screen. The drag claims the pointer only after a few pixels, so a
/// plain click on a cover still opens the album.
pub(crate) fn connect_drag(scroll: &gtk::ScrolledWindow) {
    let adj = scroll.hadjustment();
    let drag = gtk::GestureDrag::new();
    drag.set_button(1);
    drag.set_propagation_phase(gtk::PropagationPhase::Capture);
    let start = std::rc::Rc::new(std::cell::Cell::new(0.0));
    {
        let start = start.clone();
        let adj = adj.clone();
        drag.connect_drag_begin(move |_, _, _| start.set(adj.value()));
    }
    drag.connect_drag_update(move |gesture, offset_x, _| {
        if offset_x.abs() < 8.0 {
            return;
        }
        gesture.set_state(gtk::EventSequenceState::Claimed);
        let end = adj.upper() - adj.page_size();
        adj.set_value((start.get() - offset_x).clamp(adj.lower(), end));
    });
    scroll.add_controller(drag);
}

/// The space one shelf card takes in its row until a card is measured: the
/// 160 px cover, the card's margins and the button's padding (6 px a side
/// each), and the spacing.
const CARD_STEP: i32 = 196;

/// The spacing between the cards of a row, as on the shelves (start_page.blp).
const ROW_SPACING: i32 = 12;

/// The cards a LazyRow builds before it has a width.
const FIRST_CARDS: usize = 12;

/// A row of album cards that builds only the cards near what shows: those
/// that show, one more width of the row after them and one before. Empty
/// boxes as wide as the cards they stand for keep the place of the rest, so
/// the row scrolls as if all were there, and cards scrolled far away are
/// dropped with their covers. The Tuner's row of a year can hold hundreds of
/// albums; this keeps it to a few dozen cards. The cards are those of the
/// shelves, and the row moves like a shelf (Shift + the wheel, dragging),
/// without ‹ ›.
#[derive(Clone)]
pub(crate) struct LazyRow(Rc<LazyRowInner>);

struct LazyRowInner {
    scroll: gtk::ScrolledWindow,
    row: gtk::Box,
    /// The places of the cards not built before and after those built.
    lead: gtk::Box,
    tail: gtk::Box,
    albums: RefCell<Vec<Arc<subsonic::Album>>>,
    client: RefCell<Option<subsonic::Client>>,
    /// The albums whose cards are built, first to last (not included); the
    /// cards stand between `lead` and `tail` in that order.
    window: Cell<(usize, usize)>,
    /// The width of one card with the spacing, once measured on a card.
    step: Cell<Option<i32>>,
    /// The window will follow the row once the main loop is idle.
    pending: Cell<bool>,
}

impl LazyRow {
    pub(crate) fn new() -> Self {
        let row = gtk::Box::builder().spacing(ROW_SPACING).build();
        let lead = gtk::Box::builder().visible(false).build();
        let tail = gtk::Box::builder().visible(false).build();
        row.append(&lead);
        row.append(&tail);
        let scroll = gtk::ScrolledWindow::builder()
            .vscrollbar_policy(gtk::PolicyType::Never)
            // Dragging and Shift + the wheel move the row; no scrollbar over
            // the album names.
            .hscrollbar_policy(gtk::PolicyType::External)
            .child(&row)
            .build();
        connect_wheel(&scroll);
        connect_drag(&scroll);
        let inner = Rc::new(LazyRowInner {
            scroll,
            row,
            lead,
            tail,
            albums: RefCell::default(),
            client: RefCell::default(),
            window: Cell::new((0, 0)),
            step: Cell::new(None),
            pending: Cell::new(false),
        });
        // Scrolled, by any means (also by the keyboard focus moving to the
        // last card built), or resized: the window follows.
        let adj = inner.scroll.hadjustment();
        let follow = |weak: std::rc::Weak<LazyRowInner>| {
            move |_: &gtk::Adjustment| {
                if let Some(inner) = weak.upgrade() {
                    inner.schedule_follow();
                }
            }
        };
        adj.connect_value_changed(follow(Rc::downgrade(&inner)));
        adj.connect_changed(follow(Rc::downgrade(&inner)));
        Self(inner)
    }

    /// The widget to place: the scrolled row.
    pub(crate) fn widget(&self) -> &gtk::ScrolledWindow {
        &self.0.scroll
    }

    /// The albums of the row, in its order.
    pub(crate) fn album_ids(&self) -> Vec<String> {
        self.0
            .albums
            .borrow()
            .iter()
            .map(|a| a.id.clone())
            .collect()
    }

    /// Shows `albums` from the start of the row. The cards that fill two
    /// widths of `width` are built at once, so the row is full when it
    /// shows; the others as it is scrolled.
    pub(crate) fn set_albums(
        &self,
        albums: Vec<Arc<subsonic::Album>>,
        client: Option<subsonic::Client>,
        width: f64,
    ) {
        self.clear();
        let inner = &self.0;
        let total = albums.len();
        inner.albums.replace(albums);
        inner.client.replace(client);
        let window = if width > 0.0 {
            card_window(total, 0.0, width, f64::from(inner.step()))
        } else {
            (0, FIRST_CARDS.min(total))
        };
        inner.set_window(window);
    }

    /// Called once the app is connected: the built cards fetch the covers
    /// they could not get before.
    pub(crate) fn retry_covers(&self, client: subsonic::Client) {
        self.0.client.replace(Some(client.clone()));
        for_each_card(&self.0.row, |card| card.retry_cover(client.clone()));
    }

    /// Empties the row and scrolls it back to its start.
    pub(crate) fn clear(&self) {
        let inner = &self.0;
        inner.albums.borrow_mut().clear();
        inner.set_window((0, 0));
        inner.scroll.hadjustment().set_value(0.0);
    }
}

impl LazyRowInner {
    /// Builds the cards of `first..last` and drops the others; the cards
    /// both windows share stay.
    fn set_window(&self, (first, last): (usize, usize)) {
        let (old_first, old_last) = self.window.get();
        let (keep_first, keep_last) = (first.max(old_first), last.min(old_last));
        if keep_first < keep_last {
            for _ in old_first..keep_first {
                self.remove_card(self.lead.next_sibling());
            }
            for _ in keep_last..old_last {
                self.remove_card(self.tail.prev_sibling());
            }
            self.add_cards(first..keep_first, true);
            self.add_cards(keep_last..last, false);
        } else {
            // Nothing in common: all the cards go.
            for _ in old_first..old_last {
                self.remove_card(self.lead.next_sibling());
            }
            self.add_cards(first..last, false);
        }
        self.window.set((first, last));
        self.place_spacers();
    }

    /// Builds the cards of `range`, after `lead` (`front`) or before `tail`.
    fn add_cards(&self, range: std::ops::Range<usize>, front: bool) {
        let albums = self.albums.borrow();
        let client = self.client.borrow().clone();
        let albums = &albums[range];
        if front {
            for album in albums.iter().rev() {
                let button = album_button(album, client.clone());
                self.row.insert_child_after(&button, Some(&self.lead));
                self.measure(&button);
            }
        } else {
            for album in albums {
                let button = album_button(album, client.clone());
                self.row
                    .insert_child_after(&button, self.tail.prev_sibling().as_ref());
                self.measure(&button);
            }
        }
    }

    fn remove_card(&self, card: Option<gtk::Widget>) {
        if let Some(card) = card.filter(|card| card != &self.lead && card != &self.tail) {
            self.row.remove(&card);
        }
    }

    /// The width of a card with the spacing: measured on the first card
    /// built, CARD_STEP until then.
    fn step(&self) -> i32 {
        self.step.get().unwrap_or(CARD_STEP)
    }

    fn measure(&self, button: &gtk::Button) {
        if self.step.get().is_some() {
            return;
        }
        let (_, natural, _, _) = button.measure(gtk::Orientation::Horizontal, -1);
        if natural > 0 {
            self.step.set(Some(natural + ROW_SPACING));
        }
    }

    /// `lead` and `tail` as wide as the cards they stand for, less the
    /// spacing the row puts after or before each.
    fn place_spacers(&self) {
        let (first, last) = self.window.get();
        let total = self.albums.borrow().len();
        let step = self.step();
        let width = |cards: usize| {
            i32::try_from(cards)
                .unwrap_or(i32::MAX)
                .saturating_mul(step)
        };
        let lead = width(first) - ROW_SPACING;
        let tail = width(total.saturating_sub(last)) - ROW_SPACING;
        self.lead.set_visible(lead > 0);
        self.lead.set_size_request(lead.max(0), -1);
        self.tail.set_visible(tail > 0);
        self.tail.set_size_request(tail.max(0), -1);
    }

    /// Moves the window after the row, once the main loop is idle: this runs
    /// while the row is laid out, when adding widgets would upset GTK.
    fn schedule_follow(self: &Rc<Self>) {
        if self.albums.borrow().is_empty() || self.pending.replace(true) {
            return;
        }
        let weak = Rc::downgrade(self);
        glib::idle_add_local_once(move || {
            let Some(inner) = weak.upgrade() else {
                return;
            };
            inner.pending.set(false);
            let adj = inner.scroll.hadjustment();
            if adj.page_size() <= 0.0 {
                return;
            }
            let total = inner.albums.borrow().len();
            let window = card_window(total, adj.value(), adj.page_size(), f64::from(inner.step()));
            if window != inner.window.get() {
                inner.set_window(window);
            }
        });
    }
}

/// Which cards of a row of `total` to build, first to last (not included):
/// those that show, one more width of the row after them and one before; the
/// row is scrolled to `value`, `page` of it shows, and one card with its
/// spacing is `step` wide. None before the row has a width.
pub(crate) fn card_window(total: usize, value: f64, page: f64, step: f64) -> (usize, usize) {
    if page <= 0.0 || step <= 0.0 {
        return (0, 0);
    }
    let last = (((value + 2.0 * page) / step).ceil().max(0.0) as usize).min(total);
    let first = (((value - page) / step).floor().max(0.0) as usize).min(last);
    (first, last)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_long_row_at_its_start_builds_two_widths_of_cards() {
        // 1092 px show 5.6 cards of 196 px; two widths are 11.1 cards.
        assert_eq!(card_window(1000, 0.0, 1092.0, 196.0), (0, 12));
        // A narrow window: 318 px.
        assert_eq!(card_window(1000, 0.0, 318.0, 196.0), (0, 4));
    }

    #[test]
    fn a_scrolled_row_keeps_one_width_before_and_after_what_shows() {
        // 1000 px in: less than a width before it, so from the first card.
        assert_eq!(card_window(1000, 1000.0, 1092.0, 196.0), (0, 17));
        // 5000 px in: from 3908 px (card 19) to 7184 px (card 37).
        assert_eq!(card_window(1000, 5000.0, 1092.0, 196.0), (19, 37));
        // At the end of the row.
        assert_eq!(card_window(1000, 194_904.0, 1092.0, 196.0), (988, 1000));
    }

    #[test]
    fn a_short_row_builds_all_its_cards_and_no_more() {
        assert_eq!(card_window(3, 0.0, 1092.0, 196.0), (0, 3));
        assert_eq!(card_window(0, 0.0, 1092.0, 196.0), (0, 0));
        let (first, last) = card_window(20, 5000.0, 1092.0, 196.0);
        assert!(first <= last && last == 20, "{first} {last}");
    }

    #[test]
    fn a_row_without_a_width_builds_nothing() {
        assert_eq!(card_window(1000, 0.0, 0.0, 196.0), (0, 0));
        assert_eq!(card_window(1000, 0.0, 1092.0, 0.0), (0, 0));
    }
}

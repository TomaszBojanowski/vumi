//! The shelves of the Start page: a row of album cards with ‹ › to move it
//! by what is visible, Shift + the wheel and dragging to move it sideways.

use adw::prelude::*;
use std::cell::RefCell;
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
            let mut child = row.first_child();
            while let Some(button) = child {
                if let Some(card) = button
                    .downcast_ref::<gtk::Button>()
                    .and_then(|b| b.child())
                    .and_downcast::<AlbumCard>()
                {
                    card.retry_cover(client.clone());
                }
                child = button.next_sibling();
            }
        }
        return;
    }
    shown.replace(ids);

    while let Some(child) = row.first_child() {
        row.remove(&child);
    }
    for album in albums {
        let card = AlbumCard::new();
        card.set_album(album, client.clone());
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
        row.append(&button);
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

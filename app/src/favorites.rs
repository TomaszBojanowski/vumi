//! Star toggle for favourites. It flips at once, then waits for the server
//! before it can be changed again; if the server refuses, it flips back and
//! the window shows a short message.

use gettextrs::gettext;
use gtk::glib;
use gtk::prelude::*;
use std::cell::Cell;
use std::rc::Rc;

use crate::runtime;

#[derive(Clone)]
pub(crate) enum Favorite {
    Song(String),
    Album(String),
}

impl Favorite {
    fn as_starrable(&self) -> subsonic::Starrable<'_> {
        match self {
            Favorite::Song(id) => subsonic::Starrable::Song(id),
            Favorite::Album(id) => subsonic::Starrable::Album(id),
        }
    }
}

/// `client` is None while offline: the star is shown but cannot be changed.
pub(crate) fn star_button(
    starred: bool,
    client: Option<subsonic::Client>,
    item: Favorite,
) -> gtk::ToggleButton {
    let button = gtk::ToggleButton::builder()
        .active(starred)
        .valign(gtk::Align::Center)
        .sensitive(client.is_some())
        .css_classes(["flat", "circular", "star-button"])
        .build();
    update_icon(&button);

    // Set while flipping back after an error, so that flip is not sent to the server.
    let reverting = Rc::new(Cell::new(false));
    button.connect_toggled(move |button| {
        update_icon(button);
        if reverting.get() {
            return;
        }
        let Some(client) = client.clone() else {
            return;
        };
        let starred = button.is_active();
        // For the local copy, once the server has confirmed the change.
        let server = client.server_key();
        let (is_album, id) = match &item {
            Favorite::Album(id) => (true, id.clone()),
            Favorite::Song(id) => (false, id.clone()),
        };

        // One request at a time: no second click can overtake this one.
        button.set_sensitive(false);

        let item = item.clone();
        let button = button.downgrade();
        let reverting = reverting.clone();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn(async move { client.set_starred(item.as_starrable(), starred).await })
                .await;
            let confirmed = matches!(result, Ok(Ok(())));
            if confirmed {
                // Confirmed: the local database follows now, whether or not the
                // button still exists (the page may have been left meanwhile).
                let _ = runtime()
                    .spawn_blocking(move || crate::library::set_starred(&server, is_album, &id, starred))
                    .await;
            }
            let Some(button) = button.upgrade() else {
                return;
            };
            button.set_sensitive(true);

            if !confirmed {
                reverting.set(true);
                button.set_active(!starred);
                reverting.set(false);
                show_toast(&button, &gettext("Could not change favorites. Please try again."));
            } else {
                // The Favorites page follows the database, without waiting for
                // the next sync.
                if let Some(main) = button
                    .ancestor(crate::main_view::MainView::static_type())
                    .and_downcast::<crate::main_view::MainView>()
                {
                    main.refresh_favorites();
                }
            }
        });
    });
    button
}

fn update_icon(button: &gtk::ToggleButton) {
    if button.is_active() {
        button.set_icon_name("starred-symbolic");
        button.set_tooltip_text(Some(&gettext("Remove from favorites")));
    } else {
        button.set_icon_name("non-starred-symbolic");
        button.set_tooltip_text(Some(&gettext("Add to favorites")));
    }
}

/// Shows `message` in the nearest toast overlay above `widget` (the whole window).
fn show_toast(widget: &impl IsA<gtk::Widget>, message: &str) {
    if let Some(overlay) = widget
        .ancestor(adw::ToastOverlay::static_type())
        .and_downcast::<adw::ToastOverlay>()
    {
        overlay.add_toast(adw::Toast::new(message));
    }
}

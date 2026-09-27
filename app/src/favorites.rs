//! Star toggle for favourites. It flips at once and talks to the server in the
//! background; if the server refuses, it flips back.

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
        let item = item.clone();
        let button = button.downgrade();
        let reverting = reverting.clone();
        glib::spawn_future_local(async move {
            let result = runtime()
                .spawn(async move { client.set_starred(item.as_starrable(), starred).await })
                .await;
            if !matches!(result, Ok(Ok(()))) {
                eprintln!("Could not change the favourite on the server");
                if let Some(button) = button.upgrade() {
                    reverting.set(true);
                    button.set_active(!starred);
                    reverting.set(false);
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

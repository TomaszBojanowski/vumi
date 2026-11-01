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

    fn key(&self) -> String {
        match self {
            Favorite::Song(id) => format!("song:{id}"),
            Favorite::Album(id) => format!("album:{id}"),
        }
    }
}

thread_local! {
    /// Star buttons on screen, by item: a change confirmed through one shows on all.
    static BUTTONS: std::cell::RefCell<std::collections::HashMap<String, Vec<glib::WeakRef<gtk::ToggleButton>>>> =
        Default::default();
    /// Stars changed in this session, by item: older copies of the data (the
    /// queue, a page built earlier) may still carry the old state.
    static KNOWN: std::cell::RefCell<std::collections::HashMap<String, bool>> = Default::default();
    /// Set while buttons are flipped to match: those flips are not sent.
    static SYNCING: Cell<bool> = const { Cell::new(false) };
}

/// A change the server has confirmed: remembered, and every other star for the
/// same item follows it.
fn share(key: &str, starred: bool) {
    KNOWN.with(|known| known.borrow_mut().insert(key.to_string(), starred));
    let buttons: Vec<gtk::ToggleButton> = BUTTONS.with(|buttons| {
        let mut buttons = buttons.borrow_mut();
        let Some(list) = buttons.get_mut(key) else {
            return Vec::new();
        };
        list.retain(|weak| weak.upgrade().is_some());
        list.iter().filter_map(|weak| weak.upgrade()).collect()
    });
    SYNCING.with(|syncing| syncing.set(true));
    for button in buttons {
        if button.is_active() != starred {
            button.set_active(starred);
        }
    }
    SYNCING.with(|syncing| syncing.set(false));
}

/// `client` is None while offline: the star is shown but cannot be changed.
pub(crate) fn star_button(
    starred: bool,
    client: Option<subsonic::Client>,
    item: Favorite,
) -> gtk::ToggleButton {
    let key = item.key();
    let starred = KNOWN
        .with(|known| known.borrow().get(&key).copied())
        .unwrap_or(starred);
    let button = gtk::ToggleButton::builder()
        .active(starred)
        .valign(gtk::Align::Center)
        .sensitive(client.is_some())
        .css_classes(["flat", "circular", "star-button"])
        .build();
    update_icon(&button);
    BUTTONS.with(|buttons| {
        let mut buttons = buttons.borrow_mut();
        let list = buttons.entry(key).or_default();
        list.retain(|weak| weak.upgrade().is_some());
        list.push(button.downgrade());
    });

    // Set while flipping back after an error, so that flip is not sent to the server.
    let reverting = Rc::new(Cell::new(false));
    button.connect_toggled(move |button| {
        update_icon(button);
        if reverting.get() || SYNCING.with(Cell::get) {
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
        let key = item.key();
        // Found now, while the button is still in a window: the Favorites page
        // is refreshed even if the button is gone by the time the server answers.
        let main = button
            .ancestor(crate::main_view::MainView::static_type())
            .and_downcast::<crate::main_view::MainView>()
            .map(|m| m.downgrade());
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
                    .spawn_blocking(move || {
                        crate::library::set_starred(&server, is_album, &id, starred)
                    })
                    .await;
                // The Favorites page follows the database, without waiting for
                // the next sync.
                if let Some(main) = main.as_ref().and_then(|m| m.upgrade()) {
                    main.refresh_favorites();
                }
                // Every other star for the same item follows.
                share(&key, starred);
            }
            let Some(button) = button.upgrade() else {
                return;
            };
            button.set_sensitive(true);

            if !confirmed {
                reverting.set(true);
                button.set_active(!starred);
                reverting.set(false);
                show_toast(
                    &button,
                    &gettext("Could not change favorites. Please try again."),
                );
            }
        });
    });
    button
}

/// Whether `item` is a favourite now: what this session knows, else what the
/// caller's (possibly older) copy of the data says.
pub(crate) fn is_starred(item: &Favorite, fallback: bool) -> bool {
    KNOWN
        .with(|known| known.borrow().get(&item.key()).copied())
        .unwrap_or(fallback)
}

/// Stars or unstars `item` without a star button (a menu item): the server
/// first, then the local database, the Favorites page and every star button
/// on screen for the same item. `widget` is where a failure is reported.
pub(crate) fn set(
    widget: &impl IsA<gtk::Widget>,
    client: subsonic::Client,
    item: Favorite,
    starred: bool,
) {
    let server = client.server_key();
    let (is_album, id) = match &item {
        Favorite::Album(id) => (true, id.clone()),
        Favorite::Song(id) => (false, id.clone()),
    };
    let key = item.key();
    let main = widget
        .ancestor(crate::main_view::MainView::static_type())
        .and_downcast::<crate::main_view::MainView>();
    let widget = widget.upcast_ref::<gtk::Widget>().downgrade();
    glib::spawn_future_local(async move {
        let result = runtime()
            .spawn(async move { client.set_starred(item.as_starrable(), starred).await })
            .await;
        if !matches!(result, Ok(Ok(()))) {
            if let Some(widget) = widget.upgrade() {
                show_toast(
                    &widget,
                    &gettext("Could not change favorites. Please try again."),
                );
            }
            return;
        }
        let _ = runtime()
            .spawn_blocking(move || crate::library::set_starred(&server, is_album, &id, starred))
            .await;
        if let Some(main) = main {
            main.refresh_favorites();
        }
        share(&key, starred);
    });
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
        overlay.add_toast(crate::plain_toast(message));
    }
}

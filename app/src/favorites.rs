//! Star toggle for favourites. It flips at once, then waits for the server
//! before it can be changed again; if the server refuses, it flips back and
//! the window shows a short message.

use gettextrs::{gettext, pgettext};
use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
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

    pub(crate) fn key(&self) -> String {
        match self {
            Favorite::Song(id) => format!("song:{id}"),
            Favorite::Album(id) => format!("album:{id}"),
        }
    }
}

thread_local! {
    /// Star buttons on screen, by item: a change confirmed through one shows on all.
    static BUTTONS: RefCell<std::collections::HashMap<String, Vec<glib::WeakRef<gtk::ToggleButton>>>> =
        Default::default();
    /// Stars changed in this session, by item: older copies of the data (the
    /// queue, a page built earlier) may still carry the old state.
    static KNOWN: RefCell<Known> = Default::default();
    /// Set while buttons are flipped to match: those flips are not sent.
    static SYNCING: Cell<bool> = const { Cell::new(false) };
    /// Who wants to hear of a changed star (watch()).
    static WATCHERS: RefCell<Vec<Watcher>> = const { RefCell::new(Vec::new()) };
}

/// Hears of a changed star ("song:id", starred); false: no longer wanted.
type Watcher = Box<dyn Fn(&str, bool) -> bool>;

/// `watcher` hears of every star that changes from now on, as the buttons do
/// (a confirmed change, a sync settling), until it returns false: rows without
/// a star button of their own, which say "favorite" to a screen reader.
pub(crate) fn watch(watcher: impl Fn(&str, bool) -> bool + 'static) {
    WATCHERS.with(|watchers| watchers.borrow_mut().push(Box::new(watcher)));
}

/// Tells the watchers; those that answer false go.
fn tell_watchers(key: &str, starred: bool) {
    // Taken out while they run, so one may add another.
    let watchers = WATCHERS.with(|watchers| std::mem::take(&mut *watchers.borrow_mut()));
    let kept: Vec<Watcher> = watchers
        .into_iter()
        .filter(|watcher| watcher(key, starred))
        .collect();
    WATCHERS.with(|watchers| {
        let mut watchers = watchers.borrow_mut();
        let added = std::mem::take(&mut *watchers);
        *watchers = kept;
        watchers.extend(added);
    });
}

/// A change the server has confirmed: remembered, and every other star for the
/// same item follows it.
fn share(key: &str, starred: bool) {
    KNOWN.with(|known| known.borrow_mut().record(key, starred));
    flip_buttons(key, starred);
    tell_watchers(key, starred);
}

/// The number of the last star changed in this session; a sync takes it when
/// it starts (settle).
pub(crate) fn changes() -> u64 {
    KNOWN.with(|known| known.borrow().changes())
}

/// A sync that started at `since` (changes()) has stored the server's stars,
/// `albums` and `songs` the ids starred there: the stars changed in this
/// session before it take the server's (another client may have changed them
/// since), and the buttons on screen follow. Those changed while it ran stay:
/// the sync may have read the server before them.
pub(crate) fn settle(
    since: u64,
    albums: &std::collections::HashSet<String>,
    songs: &std::collections::HashSet<String>,
) {
    let changed = KNOWN.with(|known| {
        known
            .borrow_mut()
            .settle(since, |key| match key.split_once(':') {
                Some(("album", id)) => albums.contains(id),
                Some(("song", id)) => songs.contains(id),
                _ => false,
            })
    });
    for (key, starred) in changed {
        flip_buttons(&key, starred);
        tell_watchers(&key, starred);
    }
}

/// Logging out: the stars of this session belonged to that account.
pub(crate) fn forget() {
    KNOWN.with(|known| known.borrow_mut().clear());
}

/// Every star button on screen for `key` shows `starred`, without asking the
/// server.
fn flip_buttons(key: &str, starred: bool) {
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
        .with(|known| known.borrow().get(&key))
        .unwrap_or(starred);
    let button = gtk::ToggleButton::builder()
        .active(starred)
        .valign(gtk::Align::Center)
        .sensitive(client.is_some())
        .css_classes(["flat", "circular", "star-button"])
        .build();
    // A screen reader hears "Favorite" with the button's state, pressed or
    // not; the tooltip tells what a click does. It is given when asked for:
    // tooltip-text would be read out after the name as well.
    button.update_property(&[gtk::accessible::Property::Label(
        // Translators: a screen reader's name for the star button of a song
        // or an album, said with its state (pressed or not).
        &pgettext("star button", "Favorite"),
    )]);
    button.set_has_tooltip(true);
    button.connect_query_tooltip(|button, _, _, _, tooltip| {
        tooltip.set_text(Some(&if button.is_active() {
            gettext("Remove from Favorites")
        } else {
            gettext("Add to Favorites")
        }));
        true
    });
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
                let saved = runtime()
                    .spawn_blocking(move || {
                        crate::library::set_starred(&server, is_album, &id, starred)
                    })
                    .await;
                if let Ok(Err(e)) = saved {
                    eprintln!("Could not save the star in the library database: {e}");
                }
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
                crate::toast(&button, &gettext("Could not change favorites"));
            }
        });
    });
    button
}

/// Whether `item` is a favourite now: what this session knows, else what the
/// caller's (possibly older) copy of the data says.
pub(crate) fn is_starred(item: &Favorite, fallback: bool) -> bool {
    KNOWN
        .with(|known| known.borrow().get(&item.key()))
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
                crate::toast(&widget, &gettext("Could not change favorites"));
            }
            return;
        }
        let saved = runtime()
            .spawn_blocking(move || crate::library::set_starred(&server, is_album, &id, starred))
            .await;
        if let Ok(Err(e)) = saved {
            eprintln!("Could not save the star in the library database: {e}");
        }
        if let Some(main) = main {
            main.refresh_favorites();
        }
        share(&key, starred);
    });
}

fn update_icon(button: &gtk::ToggleButton) {
    button.set_icon_name(if button.is_active() {
        "starred-symbolic"
    } else {
        "non-starred-symbolic"
    });
}

/// The stars changed in this session, by item ("song:id", "album:id"), each
/// with the number of its change: a sync can tell the changes made before it
/// started from those made while it ran.
#[derive(Default)]
struct Known {
    stars: std::collections::HashMap<String, (bool, u64)>,
    changes: u64,
}

impl Known {
    fn record(&mut self, key: &str, starred: bool) {
        self.changes += 1;
        self.stars.insert(key.to_owned(), (starred, self.changes));
    }

    fn get(&self, key: &str) -> Option<bool> {
        self.stars.get(key).map(|(starred, _)| *starred)
    }

    /// The number of the last change; a sync takes it when it starts.
    fn changes(&self) -> u64 {
        self.changes
    }

    /// A sync that started at `since` (changes()) has stored the server's
    /// stars: the changes made before it take the server's star
    /// (`starred(key)`), those made while it ran stay. Returns the items whose
    /// star changed.
    fn settle(&mut self, since: u64, starred: impl Fn(&str) -> bool) -> Vec<(String, bool)> {
        let mut changed = Vec::new();
        for (key, (value, number)) in &mut self.stars {
            let now = starred(key);
            if *number <= since && *value != now {
                *value = now;
                changed.push((key.clone(), now));
            }
        }
        changed
    }

    fn clear(&mut self) {
        self.stars.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_change_made_before_a_sync_takes_the_servers_star_after_it() {
        let mut known = Known::default();
        known.record("song:a", true);
        known.record("album:b", false);
        let since = known.changes();
        // The server: song a not starred (unstarred elsewhere), album b starred.
        let changed = known.settle(since, |key| key == "album:b");
        assert_eq!(known.get("song:a"), Some(false));
        assert_eq!(known.get("album:b"), Some(true));
        let mut changed = changed;
        changed.sort();
        assert_eq!(
            changed,
            [("album:b".to_string(), true), ("song:a".to_string(), false)]
        );
    }

    #[test]
    fn a_change_made_while_a_sync_runs_stays() {
        let mut known = Known::default();
        known.record("song:a", true);
        let since = known.changes();
        // Clicked while the sync runs; the sync read the server before that.
        known.record("song:b", true);
        let changed = known.settle(since, |_| false);
        assert_eq!(known.get("song:a"), Some(false));
        assert_eq!(known.get("song:b"), Some(true));
        assert_eq!(changed, [("song:a".to_string(), false)]);
    }

    #[test]
    fn an_unchanged_star_is_not_reported_and_logging_out_forgets_all() {
        let mut known = Known::default();
        known.record("song:a", true);
        assert!(known.settle(known.changes(), |_| true).is_empty());
        assert_eq!(known.get("song:a"), Some(true));
        known.clear();
        assert_eq!(known.get("song:a"), None);
        assert_eq!(known.get("song:c"), None);
    }
}
